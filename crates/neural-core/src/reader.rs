use std::time::Duration;

use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    NeuralError, Result,
    security::{validate_redirect_target, validate_web_url},
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

#[derive(Clone)]
pub struct ReaderClient {
    agent: ureq::Agent,
    max_bytes: usize,
}

impl ReaderClient {
    pub fn new(timeout_secs: u64, max_bytes: usize) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(timeout_secs)))
            // Redirects are handled manually so every Location can be validated
            // before the next network request.
            .max_redirects(0)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            max_bytes,
        }
    }

    pub fn fetch(&self, input: &str) -> Result<ReaderArticle> {
        let mut current = validate_web_url(input)?;
        let user_agent = format!(
            "NeuralIA/{} (+https://github.com/JoseRFJuniorLLMs/NeuralIA)",
            env!("CARGO_PKG_VERSION")
        );

        for redirect_count in 0..=MAX_REDIRECTS {
            let mut response = self
                .agent
                .get(current.as_str())
                .header("Accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1")
                .header("User-Agent", user_agent.as_str())
                .call()?;

            if response.status().is_redirection() {
                if redirect_count == MAX_REDIRECTS {
                    return Err(NeuralError::RedirectLimit);
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        NeuralError::InvalidRedirect("resposta sem cabeçalho Location".into())
                    })?;
                current = validate_redirect_target(&current, location)?;
                continue;
            }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();

            if !content_type.is_empty()
                && !content_type.contains("text/html")
                && !content_type.contains("application/xhtml+xml")
            {
                return Err(NeuralError::UnsupportedContentType(content_type));
            }

            if let Some(declared) = response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
            {
                if declared > self.max_bytes as u64 {
                    return Err(NeuralError::ResponseTooLarge {
                        declared,
                        limit: self.max_bytes as u64,
                    });
                }
            }

            let html = response
                .body_mut()
                .with_config()
                .limit(self.max_bytes as u64)
                // Charset conversion (feature = charset) runs before this.
                .lossy_utf8(true)
                .read_to_string()?;

            return extract_article(&current, &html);
        }

        Err(NeuralError::RedirectLimit)
    }
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
    let root = document
        .select(&candidate_selector)
        .filter(|candidate| !inside_ignored_container(candidate))
        .max_by_key(|candidate| score_candidate(candidate, &link_selector))
        .unwrap_or_else(|| document.root_element());

    let block_selector =
        Selector::parse("h1,h2,h3,h4,h5,h6,p,blockquote,pre,li").expect("static selector");
    let mut blocks = Vec::new();
    let mut previous = String::new();

    for node in root.select(&block_selector) {
        if inside_ignored_container(&node) {
            continue;
        }
        let text = truncate_chars(
            normalize_text(node.text().collect::<Vec<_>>().join(" ")),
            MAX_BLOCK_CHARS,
        );
        if text.chars().count() < 2 || text == previous {
            continue;
        }

        let tag = node.value().name();
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

    if blocks.is_empty() {
        let fallback = truncate_chars(
            normalize_text(root.text().collect::<Vec<_>>().join(" ")),
            MAX_BLOCK_CHARS,
        );
        if fallback.chars().count() < 40 {
            return Err(NeuralError::ReaderExtraction);
        }
        blocks.push(ReaderBlock::Paragraph(fallback));
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

fn score_candidate(candidate: &ElementRef<'_>, link_selector: &Selector) -> usize {
    let total = normalize_text(candidate.text().collect::<Vec<_>>().join(" "))
        .chars()
        .count();
    let link_text: usize = candidate
        .select(link_selector)
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
}
