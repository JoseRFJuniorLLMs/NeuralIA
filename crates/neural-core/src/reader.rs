use std::time::{Duration, Instant};

use scraper::{CaseSensitivity, ElementRef, Html, Node, Selector, node::Element};
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
// Tectos independentes do algoritmo: mesmo que um passo volte a ficar caro
// por elemento, o trabalho fica proporcional a estes numeros e nao ao HTML.
// Candidatos: os primeiros MAX_CANDIDATES em ordem de documento (o exterior
// abre primeiro, por isso um <article> com centenas de .post continua a ser
// pontuado). Profundidade: elementos com mais de MAX_DEPTH antepassados nao
// sao candidatos, blocos nem texto de fallback (o Chromium corta a 512).
// Aninhamento: `text()` de um bloco le a subarvore toda, logo cada no seria
// lido uma vez por bloco antepassado -- blockquote dentro de blockquote sem
// fim voltava a ser quadratico.
const MAX_CANDIDATES: usize = 256;
const MAX_DEPTH: usize = 256;
const MAX_BLOCK_NESTING: usize = 16;
const BUDGET_CHECK_INTERVAL: usize = 512;
const CANDIDATE_CLASSES: [&str; 7] = [
    "article",
    "post",
    "entry-content",
    "post-content",
    "article-body",
    "story-body",
    "content",
];

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
    /// fora, por isso a desistencia e cooperativa: testada antes de cada
    /// pedido, entre blocos do corpo e, na extraccao, a cada
    /// `BUDGET_CHECK_INTERVAL` elementos da arvore.
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

        // O BodyReader ja entrega UTF-8 nos `text/*` (conversao de charset e
        // substituicao lossy incluidas), por isso a conversao e por movimento:
        // sem copia e sem manter os 2 MiB do Vec vivos durante o parse. O
        // `application/xhtml+xml` nao passa pelo decoder lossy, dai o recurso
        // ao `from_utf8_lossy` so quando os bytes nao sao UTF-8.
        let html = String::from_utf8(body)
            .unwrap_or_else(|invalid| String::from_utf8_lossy(invalid.as_bytes()).into_owned());

        // O prazo e a desistencia tambem cobrem a extraccao: sem isto um HTML
        // hostil prendia a thread neural-reader depois de o corpo ter chegado.
        extract_article_bounded(
            &current,
            &html,
            started.checked_add(self.timeout),
            cancelled,
        )
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
            return Err(NeuralError::UnsupportedContentType(
                if content_type.is_empty() {
                    "missing Content-Type".to_string()
                } else {
                    content_type
                },
            ));
        }

        reject_declared_oversize(&response, max_bytes)?;
        self.read_body(&mut response, started, max_bytes, false, cancelled)
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
        // O Content-Length declarado ja foi validado contra `max_bytes`; com
        // Content-Encoding o ureq devolve None e o Vec cresce como antes.
        let capacity = response
            .body()
            .content_length()
            .map_or(0, |length| length.min(max_bytes as u64) as usize);
        let mut reader = response
            .body_mut()
            .with_config()
            .limit(max_bytes as u64)
            .lossy_utf8(text)
            .reader();

        let mut body = Vec::with_capacity(capacity);
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
            // O `.limit()` do ureq conta bytes ANTES do gzip/brotli e da
            // conversao de charset (o LimitReader e a camada mais interna do
            // BodyReader), por isso esta verificacao nao e redundante: e a
            // unica que trava o corpo DESCODIFICADO -- uma bomba de
            // descompressao com 2 MiB comprimidos passava o limite do ureq.
            let decoded = body.len().saturating_add(read);
            if decoded > max_bytes {
                return Err(NeuralError::ResponseTooLarge {
                    declared: decoded as u64,
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

/// Prazo e desistencia cooperativos da extraccao, verificados a cada
/// `BUDGET_CHECK_INTERVAL` elementos visitados: um HTML hostil nao pode
/// prender a thread neural-reader depois de o utilizador carregar em Esc.
struct Budget<'a> {
    deadline: Option<Instant>,
    cancelled: &'a dyn Fn() -> bool,
    visited: usize,
}

impl Budget<'_> {
    fn check(&self) -> Result<()> {
        if (self.cancelled)() {
            return Err(NeuralError::ReaderCancelled);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(NeuralError::ReaderDeadline);
        }
        Ok(())
    }

    fn tick(&mut self) -> Result<()> {
        self.visited += 1;
        if self.visited.is_multiple_of(BUDGET_CHECK_INTERVAL) {
            self.check()
        } else {
            Ok(())
        }
    }
}

pub fn extract_article(url: &Url, html: &str) -> Result<ReaderArticle> {
    extract_article_bounded(url, html, None, &|| false)
}

/// Como `extract_article`, mas com prazo e desistencia: as travessias da
/// arvore testam `cancelled()` e `deadline` a cada `BUDGET_CHECK_INTERVAL`
/// elementos. So o `Html::parse_document` fica fora do controlo -- e linear
/// e o corpo ja chega limitado a `reader_max_bytes`.
pub fn extract_article_bounded(
    url: &Url,
    html: &str,
    deadline: Option<Instant>,
    cancelled: &dyn Fn() -> bool,
) -> Result<ReaderArticle> {
    let mut budget = Budget {
        deadline,
        cancelled,
        visited: 0,
    };
    budget.check()?;
    let document = Html::parse_document(html);
    budget.check()?;

    let title = meta_content(&document, "meta[property='og:title']")
        .or_else(|| text_of_first(&document, "title"))
        .or_else(|| text_of_first(&document, "h1"))
        .unwrap_or_else(|| url.host_str().unwrap_or("Untitled").to_string());
    let byline = meta_content(&document, "meta[name='author']");
    let excerpt = meta_content(&document, "meta[name='description']")
        .or_else(|| meta_content(&document, "meta[property='og:description']"));

    let root = pick_root(&document, &mut budget)?;
    let mut blocks = collect_blocks(root, &mut budget)?;

    if blocks.is_empty() {
        let fallback = truncate_chars(fallback_visible_text(root, &mut budget)?, MAX_BLOCK_CHARS);
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

/// Um elemento aberto durante a passagem unica. `ignored` vem do proprio
/// elemento ou de um antepassado -- propaga-se de cima para baixo pela
/// pilha, nunca subindo a arvore -- e os contadores de texto sobem de baixo
/// para cima quando o elemento fecha.
struct Frame<'a> {
    element: ElementRef<'a>,
    ignored: bool,
    candidate: bool,
    order: usize,
    text: usize,
    blocks: usize,
    links: usize,
}

/// Escolhe o contentor do artigo numa unica passagem O(n). A pontuacao e a
/// mesma heuristica de antes -- texto dos blocos menos duas vezes o texto das
/// ligacoes, blocos aninhados contam duas vezes -- mas acumulada ao fechar
/// cada elemento em vez de percorrer a subarvore de cada candidato. Em
/// empate ganha o ultimo em ordem de documento, como o `max_by_key` antigo.
/// Sem `<html>` como raiz nao ha candidatos e devolve-se a raiz, como antes.
///
/// `descendants()` e pre-ordem; o fecho de um elemento acontece quando o
/// proximo no visitado ja nao esta dentro dele, isto e, quando o pai desse
/// no nao e o topo da pilha.
fn pick_root<'a>(document: &'a Html, budget: &mut Budget<'_>) -> Result<ElementRef<'a>> {
    let html = document.root_element();
    let mut stack: Vec<Frame<'a>> = Vec::new();
    let mut best: Option<(usize, usize, ElementRef<'a>)> = None;
    let mut scored = 0usize;
    let mut order = 0usize;

    for node in html.descendants() {
        let parent = node.parent().map(|parent| parent.id());
        while let Some(frame) = stack.pop_if(|frame| Some(frame.element.id()) != parent) {
            close_frame(frame, stack.last_mut(), &mut best);
        }

        match node.value() {
            Node::Element(element) => {
                budget.tick()?;
                let Some(element_ref) = ElementRef::wrap(node) else {
                    continue;
                };
                let ignored = stack.last().is_some_and(|frame| frame.ignored)
                    || ignored_tag(element.name())
                    || is_hidden_element(element);
                let candidate = !ignored
                    && stack.len() <= MAX_DEPTH
                    && scored < MAX_CANDIDATES
                    && is_candidate(element);
                scored += usize::from(candidate);
                order += 1;
                stack.push(Frame {
                    element: element_ref,
                    ignored,
                    candidate,
                    order,
                    text: 0,
                    blocks: 0,
                    links: 0,
                });
            }
            Node::Text(text) => {
                if let Some(frame) = stack.last_mut()
                    && !frame.ignored
                {
                    frame.text += visible_chars(text);
                }
            }
            _ => {}
        }
    }
    while let Some(frame) = stack.pop() {
        close_frame(frame, stack.last_mut(), &mut best);
    }

    Ok(best.map_or(html, |(_, _, element)| element))
}

fn close_frame<'a>(
    frame: Frame<'a>,
    parent: Option<&mut Frame<'a>>,
    best: &mut Option<(usize, usize, ElementRef<'a>)>,
) {
    if frame.candidate {
        let score = frame.blocks.saturating_sub(frame.links.saturating_mul(2));
        if best
            .as_ref()
            .is_none_or(|&(top, order, _)| (score, frame.order) > (top, order))
        {
            *best = Some((score, frame.order, frame.element));
        }
    }
    if let Some(parent) = parent
        && !frame.ignored
    {
        let name = frame.element.value().name();
        parent.text += frame.text;
        parent.blocks += frame.blocks + if block_tag(name) { frame.text } else { 0 };
        parent.links += frame.links + if name == "a" { frame.text } else { 0 };
    }
}

/// Os mesmos contentores do selector antigo (`article,main,[role='main'],
/// .article,.post,.entry-content,.post-content,.article-body,.story-body,
/// .content`), testados a mao para nao pagar o motor de selectores em cada
/// elemento da arvore.
fn is_candidate(element: &Element) -> bool {
    matches!(element.name(), "article" | "main")
        || element.attr("role") == Some("main")
        || CANDIDATE_CLASSES
            .iter()
            .any(|class| element.has_class(class, CaseSensitivity::CaseSensitive))
}

fn block_tag(name: &str) -> bool {
    matches!(
        name,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "blockquote" | "pre" | "li"
    )
}

fn ignored_tag(name: &str) -> bool {
    matches!(
        name,
        "head" | "nav" | "footer" | "aside" | "script" | "style" | "form" | "template" | "noscript"
    )
}

/// Percorre os descendentes de `root` em pre-ordem e chama `visit` so nos
/// elementos visiveis ate `MAX_DEPTH` (nem ignorados nem dentro de um
/// ignorado). A visibilidade propaga-se pela pilha de elementos abertos, por
/// isso nunca se sobe aos antepassados: O(n). `visit` recebe quantos blocos
/// visiveis envolvem o elemento e devolve `false` para parar.
fn walk_visible<'a>(
    root: ElementRef<'a>,
    budget: &mut Budget<'_>,
    mut visit: impl FnMut(ElementRef<'a>, usize) -> bool,
) -> Result<()> {
    // (elemento, ignorado, bloco visivel) por elemento aberto abaixo do root
    let mut open: Vec<(ElementRef<'a>, bool, bool)> = Vec::new();
    let base_depth = root
        .ancestors()
        .filter(|node| node.value().is_element())
        .count();
    let mut ignored = 0usize;
    let mut nesting = 0usize;

    for node in root.descendants().skip(1) {
        let parent = node.parent().map(|parent| parent.id());
        while let Some((_, hidden, block)) =
            open.pop_if(|(element, _, _)| Some(element.id()) != parent)
        {
            ignored -= usize::from(hidden);
            nesting -= usize::from(block);
        }

        let Some(element) = ElementRef::wrap(node) else {
            continue;
        };
        budget.tick()?;
        let hidden = ignored > 0
            || ignored_tag(element.value().name())
            || is_hidden_element(element.value());
        let block = !hidden && block_tag(element.value().name());
        if hidden {
            ignored += 1;
        } else if base_depth + open.len() < MAX_DEPTH && !visit(element, nesting) {
            return Ok(());
        }
        nesting += usize::from(block);
        open.push((element, hidden, block));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum BlockKind {
    Heading(u8),
    Paragraph,
    Quote,
    Code,
    ListItem,
}

impl BlockKind {
    fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "h1" => Self::Heading(1),
            "h2" => Self::Heading(2),
            "h3" => Self::Heading(3),
            "h4" => Self::Heading(4),
            "h5" => Self::Heading(5),
            "h6" => Self::Heading(6),
            "blockquote" => Self::Quote,
            "pre" => Self::Code,
            "li" => Self::ListItem,
            "p" => Self::Paragraph,
            _ => return None,
        })
    }

    fn into_reader_block(self, text: String) -> ReaderBlock {
        match self {
            Self::Heading(level) => ReaderBlock::Heading { level, text },
            Self::Paragraph => ReaderBlock::Paragraph(text),
            Self::Quote => ReaderBlock::Quote(text),
            Self::Code => ReaderBlock::Code(text),
            Self::ListItem => ReaderBlock::ListItem(text),
        }
    }
}

struct PendingBlock {
    kind: BlockKind,
    own_raw: String,
    fallback_raw: String,
    has_own_visible: bool,
}

impl PendingBlock {
    fn append_raw(kind: BlockKind, raw: &mut String, text: &str) {
        if matches!(kind, BlockKind::Code) {
            raw.push_str(text);
        } else {
            if !raw.is_empty() {
                raw.push(' ');
            }
            raw.push_str(text);
        }
    }

    fn push_fallback(&mut self, text: &str) {
        if !self.has_own_visible {
            Self::append_raw(self.kind, &mut self.fallback_raw, text);
        }
    }

    fn push_own(&mut self, text: &str) {
        if !self.has_own_visible && text.split_whitespace().next().is_some() {
            self.has_own_visible = true;
            self.fallback_raw = String::new();
        }
        Self::append_raw(self.kind, &mut self.own_raw, text);
    }

    fn normalized(self) -> String {
        let raw = if self.has_own_visible {
            self.own_raw
        } else {
            self.fallback_raw
        };
        if matches!(self.kind, BlockKind::Code) {
            normalize_code(raw)
        } else {
            normalize_text(raw)
        }
    }
}

/// Recolhe blocos numa única passagem. Texto próprio pertence ao bloco emitível
/// mais interno; os ancestrais guardam o mesmo texto apenas como fallback
/// enquanto ainda não tiverem texto próprio. Assim um <li> pai com texto não
/// absorve o <li> filho, mas um wrapper sem texto próprio como
/// <blockquote><p>...</p></blockquote> conserva a semântica Quote.
///
/// Quando a árvore passa de MAX_BLOCK_NESTING, blocos mais fundos deixam de ser
/// emitidos e o texto continua no fallback do último bloco elegível, portanto
/// conteúdo profundo não desaparece. PendingBlock nasce em pré-ordem e é
/// materializado no fim sem revarrer subárvores.
fn collect_blocks(root: ElementRef<'_>, budget: &mut Budget<'_>) -> Result<Vec<ReaderBlock>> {
    // (elemento, ignorado, é bloco visível, índice emitível) por elemento aberto
    let mut open: Vec<(ElementRef<'_>, bool, bool, Option<usize>)> = Vec::new();
    let mut active_blocks: Vec<usize> = Vec::new();
    let mut pending: Vec<PendingBlock> = Vec::new();
    let base_depth = root
        .ancestors()
        .filter(|node| node.value().is_element())
        .count();
    let mut ignored = 0usize;
    let mut nesting = 0usize;

    for node in root.descendants().skip(1) {
        let parent = node.parent().map(|parent| parent.id());
        while let Some((_, hidden, block, pending_index)) =
            open.pop_if(|(element, _, _, _)| Some(element.id()) != parent)
        {
            ignored -= usize::from(hidden);
            nesting -= usize::from(block);
            if let Some(index) = pending_index {
                debug_assert_eq!(active_blocks.pop(), Some(index));
            }
        }

        match node.value() {
            Node::Element(element) => {
                budget.tick()?;
                let Some(element_ref) = ElementRef::wrap(node) else {
                    continue;
                };
                let hidden =
                    ignored > 0 || ignored_tag(element.name()) || is_hidden_element(element);
                let block = !hidden && block_tag(element.name());
                let pending_index =
                    if block && nesting <= MAX_BLOCK_NESTING && base_depth + open.len() < MAX_DEPTH
                    {
                        let Some(kind) = BlockKind::from_tag(element.name()) else {
                            open.push((element_ref, hidden, block, None));
                            nesting += usize::from(block);
                            continue;
                        };
                        let index = pending.len();
                        pending.push(PendingBlock {
                            kind,
                            own_raw: String::new(),
                            fallback_raw: String::new(),
                            has_own_visible: false,
                        });
                        active_blocks.push(index);
                        Some(index)
                    } else {
                        None
                    };

                if hidden {
                    ignored += 1;
                }
                nesting += usize::from(block);
                open.push((element_ref, hidden, block, pending_index));
            }
            Node::Text(text) if ignored == 0 => {
                for &index in &active_blocks {
                    pending[index].push_fallback(text);
                }
                if let Some(index) = active_blocks.last().copied() {
                    pending[index].push_own(text);
                }
            }
            _ => {}
        }
    }

    let mut blocks = Vec::new();
    let mut previous = String::new();
    for block in pending {
        let kind = block.kind;
        let text = truncate_chars(block.normalized(), MAX_BLOCK_CHARS);
        if text.chars().count() < 2 || text == previous {
            continue;
        }
        previous = text.clone();
        blocks.push(kind.into_reader_block(text));
        if blocks.len() >= MAX_BLOCKS {
            break;
        }
    }
    Ok(blocks)
}

fn fallback_visible_text(root: ElementRef<'_>, budget: &mut Budget<'_>) -> Result<String> {
    let mut pieces: Vec<String> = Vec::new();

    walk_visible(root, budget, |node, _| {
        if node.children().any(|child| child.value().is_element()) {
            return true;
        }
        let text = normalize_text(node.text().collect::<Vec<_>>().join(" "));
        if text.chars().count() >= 2 && pieces.last() != Some(&text) {
            pieces.push(text);
        }
        true
    })?;

    Ok(normalize_text(pieces.join(" ")))
}

fn is_hidden_element(element: &Element) -> bool {
    if element.attr("hidden").is_some()
        || element
            .attr("aria-hidden")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    {
        return true;
    }

    element.attr("style").is_some_and(|style| {
        let compact: String = style
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        compact.contains("display:none") || compact.contains("visibility:hidden")
    })
}

/// Comprimento que `normalize_text` daria a este texto, sem construir a
/// string: as palavras mais os espacos entre elas.
fn visible_chars(text: &str) -> usize {
    let (words, chars) = text
        .split_whitespace()
        .fold((0usize, 0usize), |(words, chars), word| {
            (words + 1, chars + word.chars().count())
        });
    chars + words.saturating_sub(1)
}

fn meta_content(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr("content"))
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
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
    fn fallback_excludes_script_and_style_payloads() {
        let html = r#"<html><head><title>Aplicacao</title></head><body>
        <div>Conteudo visivel suficientemente longo para o Reader usar como fallback sem executar JavaScript.</div>
        <script>window.ytInitialData = "segredo que nao pode aparecer no Reader";</script>
        <style>.x{content:"tambem nao"}</style>
        </body></html>"#;
        let url = Url::parse("https://example.com/app").unwrap();
        let article = extract_article(&url, html).unwrap();
        let rendered = format!("{:?}", article.blocks);
        assert!(rendered.contains("Conteudo visivel"));
        assert!(!rendered.contains("ytInitialData"));
        assert!(!rendered.contains("segredo"));
        assert!(!rendered.contains("tambem nao"));
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
    fn prefers_the_container_with_most_block_text() {
        let html = r#"<html><head><title>Dois</title></head><body>
        <div class="content"><p>Curto.</p><a href="/x">ligacao ligacao ligacao ligacao</a></div>
        <article><p>Paragrafo longo o suficiente para ganhar a pontuacao do contentor.</p>
        <ul><li>Um item de lista com texto.</li></ul></article>
        <aside><div class="post"><p>Barra lateral ignorada mesmo sendo enorme enorme enorme enorme enorme enorme.</p></div></aside>
        </body></html>"#;
        let url = Url::parse("https://example.com/").unwrap();
        let article = extract_article(&url, html).unwrap();
        let rendered = format!("{:?}", article.blocks);
        assert!(rendered.contains("Paragrafo longo"));
        assert!(rendered.contains("Um item de lista"));
        assert!(!rendered.contains("Curto"));
        assert!(!rendered.contains("Barra lateral"));
    }

    #[test]
    fn hidden_state_propagates_without_ancestor_walk() {
        let html = r#"<html><head><title>Oculto</title></head><body><article>
        <div hidden><div><div><p>fundo escondido escondido escondido escondido escondido</p></div></div></div>
        <div><div><div><p>fundo visivel e suficientemente longo para o leitor.</p></div></div></div>
        </article></body></html>"#;
        let url = Url::parse("https://example.com/").unwrap();
        let article = extract_article(&url, html).unwrap();
        let rendered = format!("{:?}", article.blocks);
        assert!(rendered.contains("fundo visivel"));
        assert!(!rendered.contains("fundo escondido"));
    }

    #[test]
    fn nested_list_item_text_is_not_repeated_by_its_parent() {
        let html = r#"<html><head><title>Lista</title></head><body><article>
        <ul>
          <li>Item pai com contexto suficiente
            <ul><li>Item filho aparece uma vez apenas</li></ul>
          </li>
        </ul>
        </article></body></html>"#;
        let url = Url::parse("https://example.com/lista").unwrap();
        let article = extract_article(&url, html).unwrap();

        let items: Vec<&str> = article
            .blocks
            .iter()
            .filter_map(|block| match block {
                ReaderBlock::ListItem(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0], "Item pai com contexto suficiente");
        assert_eq!(items[1], "Item filho aparece uma vez apenas");
        assert_eq!(
            items
                .iter()
                .filter(|text| text.contains("Item filho aparece uma vez apenas"))
                .count(),
            1
        );
    }

    #[test]
    fn semantic_wrapper_keeps_its_type_without_duplicate_inner_paragraph() {
        let html = r#"<html><head><title>Semântica</title></head><body><article>
        <ul><li><p>Item de lista embrulhado em parágrafo</p></li></ul>
        <blockquote><p>Citação embrulhada em parágrafo</p></blockquote>
        </article></body></html>"#;
        let url = Url::parse("https://example.com/semantica").unwrap();
        let article = extract_article(&url, html).unwrap();

        assert!(article.blocks.iter().any(|block| {
            matches!(block, ReaderBlock::ListItem(text) if text == "Item de lista embrulhado em parágrafo")
        }));
        assert!(article.blocks.iter().any(|block| {
            matches!(block, ReaderBlock::Quote(text) if text == "Citação embrulhada em parágrafo")
        }));
        assert!(!article.blocks.iter().any(|block| {
            matches!(block, ReaderBlock::Paragraph(text)
                if text == "Item de lista embrulhado em parágrafo"
                    || text == "Citação embrulhada em parágrafo")
        }));
    }

    #[test]
    fn empty_meta_content_does_not_block_title_or_excerpt_fallbacks() {
        let html = r#"<html><head>
        <meta property="og:title" content="   ">
        <title>Título real da página</title>
        <meta name="description" content="">
        <meta property="og:description" content="Resumo real da página">
        </head><body><article>
        <p>Conteúdo suficientemente longo para a extração do Reader.</p>
        </article></body></html>"#;
        let url = Url::parse("https://example.com/meta").unwrap();
        let article = extract_article(&url, html).unwrap();

        assert_eq!(article.title, "Título real da página");
        assert_eq!(article.excerpt.as_deref(), Some("Resumo real da página"));
    }

    #[test]
    fn fallback_body_never_contains_document_title() {
        let html = r#"<html><head><title>TÍTULO NÃO É CORPO</title></head><body>
        <div><span>Este é o conteúdo visível suficientemente longo para o Reader usar como texto de fallback do artigo.</span></div>
        </body></html>"#;
        let url = Url::parse("https://example.com/fallback").unwrap();
        let article = extract_article(&url, html).unwrap();
        let rendered = format!("{:?}", article.blocks);

        assert!(rendered.contains("Este é o conteúdo visível"));
        assert!(!rendered.contains("TÍTULO NÃO É CORPO"));
    }
}
