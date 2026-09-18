use std::time::{Duration, Instant};

use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::io::Read;
use ureq::{
    Agent, Body,
    config::Config,
    http::Response,
    http::Uri,
    tls::{RootCerts, TlsConfig},
    unversioned::{
        resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver},
        transport::{DefaultConnector, NextTimeout},
    },
};

use url::Url;

use crate::{
    NeuralError, Result,
    security::{
        is_forbidden_ip, is_local_network_target, validate_redirect_target, validate_web_url,
    },
};

const MAX_REDIRECTS: usize = 5;
const MAX_BLOCKS: usize = 600;
const MAX_TITLE_CHARS: usize = 512;
const MAX_BYLINE_CHARS: usize = 256;
const MAX_EXCERPT_CHARS: usize = 2_000;
const MAX_BLOCK_CHARS: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReaderArticle {
    pub source_url: String,
    pub title: String,
    pub byline: Option<String>,
    pub excerpt: Option<String>,
    pub blocks: Vec<ReaderBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReaderBlock {
    Heading { level: u8, text: String },
    Paragraph(String),
    Quote(String),
    Code(String),
    ListItem(String),
}

#[derive(Debug, Default)]
struct PublicResolver {
    inner: DefaultResolver,
}

impl Resolver for PublicResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> std::result::Result<ResolvedSocketAddrs, ureq::Error> {
        let resolved = self.inner.resolve(uri, config, timeout)?;
        let mut safe = self.inner.empty();

        for address in &resolved {
            if !is_forbidden_ip(address.ip()) {
                safe.push(*address);
            }
        }

        if safe.is_empty() {
            Err(ureq::Error::HostNotFound)
        } else {
            Ok(safe)
        }
    }
}

#[derive(Clone)]
pub struct ReaderClient {
    public_agent: Agent,
    local_agent: Agent,
    max_bytes: usize,
    timeout: Duration,
}

impl ReaderClient {
    pub fn new(timeout_secs: u64, max_bytes: usize) -> Self {
        let config = Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();

        let public_agent = Agent::with_parts(
            config.clone(),
            DefaultConnector::new(),
            PublicResolver::default(),
        );
        let local_agent = Agent::new_with_config(config);

        Self {
            public_agent,
            local_agent,
            max_bytes,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    pub fn fetch(&self, input: &str) -> Result<ReaderArticle> {
        self.fetch_cancellable(input, &|| false)
    }

    /// Como `fetch`, mas desiste assim que `cancelled()` passa a ser verdade.
    /// O Reader corre numa thread propria e a ligacao nao se pode abortar de
    /// fora, por isso a desistencia e cooperativa: testada antes de cada pedido
    /// e entre blocos do corpo.
    pub fn fetch_cancellable(
        &self,
        input: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ReaderArticle> {
        let (current, mut response, started) = self.resolve(input, cancelled)?;

        let content_type = header(&response, "content-type").to_ascii_lowercase();
        let media_type = content_type.split(';').next().unwrap_or("").trim();
        if !media_type.is_empty()
            && media_type != "text/html"
            && media_type != "application/xhtml+xml"
        {
            return Err(NeuralError::UnsupportedContentType(content_type));
        }

        reject_declared_oversize(&response, self.max_bytes)?;
        let body = self.read_body(&mut response, started, self.max_bytes, true, cancelled)?;

        // O BodyReader ja entrega UTF-8 (conversao de charset incluida), por
        // isso juntamos os blocos e convertemos uma vez so: um caratere
        // partido entre blocos nao se estraga.
        let html = String::from_utf8_lossy(&body).into_owned();
        extract_article(&current, &html)
    }

    /// Descarrega um documento binario -- por exemplo `application/pdf` -- ate
    /// `max_bytes`, com o mesmo filtro de rede, os mesmos redirects e o mesmo
    /// prazo do Reader. Devolve os bytes tal como vieram, sem conversao nenhuma.
    pub fn fetch_document(
        &self,
        input: &str,
        media_type: &str,
        max_bytes: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>> {
        let (_, mut response, started) = self.resolve(input, cancelled)?;

        let content_type = header(&response, "content-type").to_ascii_lowercase();
        let actual = content_type.split(';').next().unwrap_or("").trim();
        if actual != media_type {
            return Err(NeuralError::UnsupportedContentType(if content_type.is_empty() {
                "cabeçalho Content-Type ausente".to_string()
            } else {
                content_type
            }));
        }

        reject_declared_oversize(&response, max_bytes)?;
        let body = self.read_body(&mut response, started, max_bytes, false, cancelled)?;
        if media_type == "application/pdf" {
            let head = &body[..body.len().min(1024)];
            if !head.windows(5).any(|window| window == b"%PDF-") {
                return Err(NeuralError::UnsupportedContentType(
                    "application/pdf sem assinatura %PDF-".to_string(),
                ));
            }
        }
        Ok(body)
    }

    /// Segue os redirects ate a resposta final, com o filtro de rede aplicado
    /// a cada salto e um unico prazo para a cadeia inteira.
    fn resolve(
        &self,
        input: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Url, Response<Body>, Instant)> {
        let mut current = validate_web_url(input)?;
        let started = Instant::now();
        let user_agent = format!(
            "NeuralIA/{} (+https://github.com/JoseRFJuniorLLMs/NeuralIA)",
            env!("CARGO_PKG_VERSION")
        );

        for redirect_count in 0..=MAX_REDIRECTS {
            if cancelled() {
                return Err(NeuralError::ReaderCancelled);
            }

            let remaining = self
                .timeout
                .checked_sub(started.elapsed())
                .filter(|duration| !duration.is_zero())
                .ok_or(NeuralError::ReaderDeadline)?;

            let agent = if is_local_network_target(&current) {
                &self.local_agent
            } else {
                &self.public_agent
            };

            let response = agent
                .get(current.as_str())
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/pdf;q=0.9,*/*;q=0.1",
                )
                .header("User-Agent", user_agent.as_str())
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call()?;

            if response.status().is_redirection() {
                if redirect_count == MAX_REDIRECTS {
                    return Err(NeuralError::RedirectLimit);
                }
                let location = header(&response, "location");
                if location.is_empty() {
                    return Err(NeuralError::InvalidRedirect(
                        "resposta sem cabeçalho Location".into(),
                    ));
                }
                current = validate_redirect_target(&current, location)?;
                continue;
            }

            return Ok((current, response, started));
        }

        Err(NeuralError::RedirectLimit)
    }

    /// Le o corpo em blocos, para haver onde desistir e onde verificar o prazo:
    /// sem isto uma leitura em curso continua a puxar bytes depois de o
    /// utilizador ja ter voltado a Home.
    fn read_body(
        &self,
        response: &mut Response<Body>,
        started: Instant,
        max_bytes: usize,
        text: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>> {
        let mut reader = response
            .body_mut()
            .with_config()
            // O limite interno do ureq não é a nossa fronteira de confiança:
            // dependendo do Content-Encoding ele pode contar bytes numa fase
            // diferente da decodificação. Lemos no máximo limite+1 e aplicamos
            // abaixo um teto explícito sobre os bytes que chegam ao chamador.
            .limit((max_bytes as u64).saturating_add(1))
            .lossy_utf8(text)
            .reader();

        let mut body = Vec::with_capacity(max_bytes.min(64 * 1024));
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if cancelled() {
                return Err(NeuralError::ReaderCancelled);
            }
            if started.elapsed() > self.timeout {
                return Err(NeuralError::ReaderDeadline);
            }
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            let next_len = body.len().saturating_add(read);
            if next_len > max_bytes {
                return Err(NeuralError::ResponseTooLarge {
                    declared: next_len as u64,
                    limit: max_bytes as u64,
                });
            }
            body.extend_from_slice(&chunk[..read]);
        }
        drop(reader);

        if started.elapsed() > self.timeout {
            return Err(NeuralError::ReaderDeadline);
        }
        Ok(body)
    }
}

fn header<'a>(response: &'a Response<Body>, name: &str) -> &'a str {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

/// Um `Content-Length` acima do limite e rejeitado antes de se ler um byte.
fn reject_declared_oversize(response: &Response<Body>, limit: usize) -> Result<()> {
    if let Ok(declared) = header(response, "content-length").parse::<u64>()
        && declared > limit as u64
    {
        return Err(NeuralError::ResponseTooLarge {
            declared,
            limit: limit as u64,
        });
    }
    Ok(())
}

impl Default for ReaderClient {
    fn default() -> Self {
        Self::new(12, 2 * 1024 * 1024)
    }
}

pub fn extract_article(url: &Url, html: &str) -> Result<ReaderArticle> {
    let document = Html::parse_document(html);
    let title = meta_content(&document, "meta[property='og:title']")
        .or_else(|| text_of_first(&document, "title"))
        .or_else(|| text_of_first(&document, "h1"))
        .unwrap_or_else(|| url.host_str().unwrap_or("Untitled").to_string());
    let byline = meta_content(&document, "meta[name='author']");
    let excerpt = meta_content(&document, "meta[name='description']")
        .or_else(|| meta_content(&document, "meta[property='og:description']"));

    let candidate_selector = Selector::parse(
        "article,main,[role='main'],.article,.post,.entry-content,.post-content,.article-body,.story-body,.content",
    )
    .expect("static selector");
    let link_selector = Selector::parse("a").expect("static selector");
    let block_selector =
        Selector::parse("h1,h2,h3,h4,h5,h6,p,blockquote,pre,li").expect("static selector");
    let root = document
        .select(&candidate_selector)
        .filter(|candidate| !inside_ignored_container(candidate))
        .max_by_key(|candidate| score_candidate(candidate, &link_selector, &block_selector))
        .unwrap_or_else(|| document.root_element());
    let mut blocks = Vec::new();
    let mut previous = String::new();

    for node in root.select(&block_selector) {
        if inside_ignored_container(&node) {
            continue;
        }

        let tag = node.value().name();
        let text = if tag == "pre" {
            truncate_chars(
                normalize_code(node.text().collect::<Vec<_>>().join("")),
                MAX_BLOCK_CHARS,
            )
        } else {
            truncate_chars(
                normalize_text(node.text().collect::<Vec<_>>().join(" ")),
                MAX_BLOCK_CHARS,
            )
        };

        if text.chars().count() < 2 || text == previous {
            continue;
        }

        let block = match tag {
            "h1" => ReaderBlock::Heading {
                level: 1,
                text: text.clone(),
            },
            "h2" => ReaderBlock::Heading {
                level: 2,
                text: text.clone(),
            },
            "h3" => ReaderBlock::Heading {
                level: 3,
                text: text.clone(),
            },
            "h4" => ReaderBlock::Heading {
                level: 4,
                text: text.clone(),
            },
            "h5" => ReaderBlock::Heading {
                level: 5,
                text: text.clone(),
            },
            "h6" => ReaderBlock::Heading {
                level: 6,
                text: text.clone(),
            },
            "blockquote" => ReaderBlock::Quote(text.clone()),
            "pre" => ReaderBlock::Code(text.clone()),
            "li" => ReaderBlock::ListItem(text.clone()),
            _ => ReaderBlock::Paragraph(text.clone()),
        };

        previous = text;
        blocks.push(block);
        if blocks.len() >= MAX_BLOCKS {
            break;
        }
    }

    // Não usar root.text() como fallback. Em aplicações JS-heavy isso inclui
    // <script>, JSON de hidratação e estado interno, que pode ser muito maior
    // do que o conteúdo visível. Sem blocos semânticos reais, falhamos de forma
    // limpa e deixamos o utilizador abrir a página completa.
    if blocks.is_empty() {
        return Err(NeuralError::ReaderExtraction);
    }

    Ok(ReaderArticle {
        source_url: url.to_string(),
        title: truncate_chars(normalize_text(title), MAX_TITLE_CHARS),
        byline: byline
            .map(normalize_text)
            .map(|value| truncate_chars(value, MAX_BYLINE_CHARS))
            .filter(|value| !value.is_empty()),
        excerpt: excerpt
            .map(normalize_text)
            .map(|value| truncate_chars(value, MAX_EXCERPT_CHARS))
            .filter(|value| !value.is_empty()),
        blocks,
    })
}

fn score_candidate(
    candidate: &ElementRef<'_>,
    link_selector: &Selector,
    block_selector: &Selector,
) -> usize {
    let total: usize = candidate
        .select(block_selector)
        .filter(|node| !inside_ignored_container(node))
        .map(|node| {
            normalize_text(node.text().collect::<Vec<_>>().join(" "))
                .chars()
                .count()
        })
        .sum();
    let link_text: usize = candidate
        .select(link_selector)
        .filter(|link| !inside_ignored_container(link))
        .map(|link| {
            normalize_text(link.text().collect::<Vec<_>>().join(" "))
                .chars()
                .count()
        })
        .sum();
    total.saturating_sub(link_text.saturating_mul(2))
}

fn inside_ignored_container(node: &ElementRef<'_>) -> bool {
    is_hidden_element(node)
        || node
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|ancestor| {
                matches!(
                    ancestor.value().name(),
                    "nav"
                        | "footer"
                        | "aside"
                        | "script"
                        | "style"
                        | "form"
                        | "template"
                        | "noscript"
                ) || is_hidden_element(&ancestor)
            })
}

fn is_hidden_element(element: &ElementRef<'_>) -> bool {
    let value = element.value();

    if value.attr("hidden").is_some()
        || value
            .attr("aria-hidden")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    {
        return true;
    }

    value.attr("style").is_some_and(|style| {
        let compact: String = style
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        compact.contains("display:none") || compact.contains("visibility:hidden")
    })
}

fn meta_content(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr("content"))
        .map(ToString::to_string)
}

fn text_of_first(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .map(|node| normalize_text(node.text().collect::<Vec<_>>().join(" ")))
        .filter(|text| !text.is_empty())
}

fn normalize_text(input: String) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_code(input: String) -> String {
    let newline = char::from(10).to_string();
    input.lines().collect::<Vec<_>>().join(&newline)
}

fn truncate_chars(value: String, limit: usize) -> String {
    if value.chars().count() <= limit {
        value
    } else {
        value.chars().take(limit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_article() {
        let html = r#"<html><head><title>Teste</title><meta name="author" content="Eva"></head>
        <body><nav>menu</nav><article><h1>Um título</h1>
        <p>Este é um parágrafo suficientemente útil.</p>
        <blockquote>Uma citação.</blockquote><pre>let x = 1;</pre></article></body></html>"#;
        let url = Url::parse("https://example.com/a").unwrap();
        let article = extract_article(&url, html).unwrap();
        assert_eq!(article.title, "Teste");
        assert_eq!(article.byline.as_deref(), Some("Eva"));
        assert!(article.blocks.len() >= 4);
    }

    #[test]
    fn preserves_code_whitespace() {
        let html = "<article><pre>fn main() {\n    println!(\"hi\");\n}</pre></article>";
        let url = Url::parse("https://example.com/code").unwrap();
        let article = extract_article(&url, html).unwrap();
        let ReaderBlock::Code(code) = &article.blocks[0] else {
            panic!("expected code block");
        };
        assert!(code.contains("\n    println!"));
    }

    #[test]
    fn excludes_hidden_and_navigation_content() {
        let html = r#"<html><head><title>Visible</title></head><body>
        <nav><p>menu menu menu menu menu menu menu</p></nav>
        <article>
          <p hidden>segredo escondido</p>
          <div aria-hidden="true"><p>não deve aparecer</p></div>
          <div style="display: none"><p>também não</p></div>
          <p>Conteúdo real e suficientemente útil para o leitor.</p>
        </article></body></html>"#;
        let url = Url::parse("https://example.com/").unwrap();
        let article = extract_article(&url, html).unwrap();
        let rendered = format!("{:?}", article.blocks);
        assert!(rendered.contains("Conteúdo real"));
        assert!(!rendered.contains("segredo"));
        assert!(!rendered.contains("não deve"));
        assert!(!rendered.contains("também não"));
    }

    #[test]
    fn unicode_scoring_is_character_based() {
        let html = r#"<html><head><title>日本語</title></head><body>
        <article><p>これは十分に長い本文です。これは文字数で扱われます。</p></article>
        </body></html>"#;
        let url = Url::parse("https://example.jp/").unwrap();
        assert!(extract_article(&url, html).is_ok());
    }

    #[test]
    fn script_only_page_is_not_exposed_as_reader_text() {
        let html = r#"<html><head><title>Aplicação</title></head><body>
        <script>window.ytInitialData = {"secret":"hydration payload"};</script>
        <style>.hidden{display:none}</style>
        <div id="app"></div>
        </body></html>"#;
        let url = Url::parse("https://example.com/app").unwrap();
        assert!(matches!(
            extract_article(&url, html),
            Err(NeuralError::ReaderExtraction)
        ));
    }
}
