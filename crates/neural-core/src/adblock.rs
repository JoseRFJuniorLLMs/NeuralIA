//! Bloqueio de anuncios por lista de dominios (adblock, plano 2.3).
//!
//! Nada vem embutido no repositorio nem no instalador (decisao do dono,
//! OQ3): a lista de Peter Lowe (`pgl.yoyo.org`, ~4 500 dominios) so e
//! baixada quando o utilizador clica em "Ativar", e renovada no maximo uma
//! vez por semana, so com o bloqueio ativo e uma superficie web aberta --
//! nunca na Home, nunca no modo privado.
//!
//! - `DomainSet`: os dominios (ate 500 000 e 8 MiB de nomes), normalizados
//!   como o WebView os pede (`domains::normalize_domain`: minusculas, IDN em
//!   punycode). `find` e o casamento por sufixo de rotulos
//!   (`domains::label_suffix_match`): uma consulta por rotulo do host.
//! - `parse_domain_list`: linhas simples, `0.0.0.0 x` (formato hosts) e
//!   `||x^`; ignora comentarios, caminhos, curingas, regras cosmeticas, de
//!   excecao ou com opcoes, IPs e rotulos soltos.
//! - `validate_list`: o que chega da rede so e aceite com pelo menos
//!   `MIN_LIST_DOMAINS` dominios, ate `LIST_MAX_BYTES`, e nunca HTML (nem
//!   pelo `Content-Type` nem pelo primeiro byte).
//! - `ListClient`: o GET da lista com a politica do transporte das chamadas
//!   de IA (`llm::transport::policy_agent`: HTTPS so no host fixado, sem
//!   proxy, sem redirects, certificados do sistema, resolvedor publico),
//!   20 s de prazo e 2 MiB de tecto do corpo descodificado. Nao conta como
//!   chamada de IA em lado nenhum. O loopback so existe nos testes.
//! - `AdblockRules::decide`: o veredicto de um pedido. NUNCA bloqueia o
//!   proprio documento, um pedido que nao e web, o loopback (`localhost`,
//!   `*.localhost`, `127.0.0.0/8`, `::1`; as origens `neuralia-*`), uma
//!   pagina de IA ou de login (do registo dos provedores:
//!   `search::is_ai_provider_host` e `search::is_login_host`, nunca uma
//!   lista a mao), um site da lista de permitidos, nem um pedido do proprio
//!   site da pagina.
//! - `refresh_due`: quando a lista se renova.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::domains::{is_label_suffix, label_suffix_match, normalize_domain, without_www};
use crate::llm::errors::ApiError;
use crate::llm::transport::{Overflow, header, map_transport_error, policy_agent, read_body};
use crate::search::{is_ai_provider_host, is_login_host};
use crate::security::Locality;

/// O host fixado da lista.
pub const LIST_HOST: &str = "pgl.yoyo.org";
/// O caminho da lista: so os nomes, sem cabecalho, em texto simples.
pub const LIST_PATH: &str =
    "/adservers/serverlist.php?hostformat=nohtml&showintro=0&mimetype=plaintext";
/// O prazo do download inteiro.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// O tecto do corpo descodificado da lista.
pub const LIST_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Uma lista com menos dominios do que isto nao e a lista (uma pagina de
/// erro, um corte a meio).
pub const MIN_LIST_DOMAINS: usize = 1_000;
/// Os tectos do conjunto: entradas e bytes de nomes.
pub const MAX_DOMAINS: usize = 500_000;
pub const MAX_DOMAIN_BYTES: usize = 8 * 1024 * 1024;

const HOUR_MS: u64 = 60 * 60 * 1000;
/// A lista renova-se no maximo uma vez por semana.
pub const REFRESH_EVERY_MS: u64 = 7 * 24 * HOUR_MS;
/// Depois de um download falhado, a proxima tentativa automatica espera isto.
pub const RETRY_AFTER_FAILURE_MS: u64 = HOUR_MS;

/// O maior numero de sites com anuncios permitidos que se guarda.
pub const MAX_ALLOW_SITES: usize = 10_000;
/// A versao e o tecto de `adblock-settings.json`.
pub const SETTINGS_VERSION: u32 = 1;
pub const SETTINGS_MAX_BYTES: u64 = 1024 * 1024;
/// A versao e o tecto do ficheiro da lista baixada (`\n` vira `\\n` no JSON).
pub const STORED_LIST_VERSION: u32 = 1;
pub const STORED_LIST_MAX_BYTES: u64 = (MAX_DOMAIN_BYTES + 2 * MAX_DOMAINS + 64 * 1024) as u64;

// ===================== o conjunto de dominios =====================

/// Os dominios da lista, ja normalizados. Os tectos (`MAX_DOMAINS`,
/// `MAX_DOMAIN_BYTES`) valem para qualquer origem: a rede ou o disco.
#[derive(Debug, Clone, Default)]
pub struct DomainSet {
    domains: HashSet<Box<str>>,
    bytes: usize,
}

impl DomainSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.domains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }

    /// Os bytes dos nomes guardados.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Acrescenta um dominio JA normalizado. `false` se ja estava ou se
    /// passava de um dos tectos (nesse caso nada muda).
    pub fn insert(&mut self, domain: &str) -> bool {
        if self.domains.contains(domain)
            || self.domains.len() >= MAX_DOMAINS
            || self.bytes + domain.len() > MAX_DOMAIN_BYTES
        {
            return false;
        }
        self.bytes += domain.len();
        self.domains.insert(domain.into())
    }

    /// Cabe mais este dominio?
    fn has_room_for(&self, domain: &str) -> bool {
        self.domains.len() < MAX_DOMAINS && self.bytes + domain.len() <= MAX_DOMAIN_BYTES
    }

    /// O dominio da lista de que `host` (normalizado) e ele proprio ou um
    /// subdominio.
    pub fn find<'a>(&self, host: &'a str) -> Option<&'a str> {
        self.find_counting(host, &mut 0)
    }

    /// `find`, contando as consultas ao conjunto: uma por rotulo do host,
    /// por maior que a lista seja.
    pub fn find_counting<'a>(&self, host: &'a str, lookups: &mut usize) -> Option<&'a str> {
        label_suffix_match(host, |suffix| {
            *lookups += 1;
            self.domains.contains(suffix)
        })
    }

    /// Os dominios por ordem, um por linha: o que se grava no disco.
    pub fn to_text(&self) -> String {
        let mut sorted: Vec<&str> = self.domains.iter().map(|domain| &**domain).collect();
        sorted.sort_unstable();
        sorted.join("\n")
    }
}

// ===================== o parser da lista =====================

/// O que `parse_domain_list` tirou de um texto.
#[derive(Debug, Clone, Default)]
pub struct ParsedList {
    pub domains: DomainSet,
    /// Linhas com conteudo que nao sao um dominio simples (caminhos,
    /// curingas, regras cosmeticas, opcoes, IPs, rotulos soltos).
    pub ignored: usize,
    /// Parou num tecto do `DomainSet`.
    pub truncated: bool,
}

/// Os IPs com que o formato hosts aponta um nome para lado nenhum.
const HOSTS_SINKS: &[&str] = &["0.0.0.0", "127.0.0.1", "::", "::0", "::1"];

enum LineKind<'a> {
    /// Vazia, comentario ou cabecalho: nao conta.
    Skip,
    /// Uma regra que esta lista nao entende.
    Ignored,
    Candidates(Vec<&'a str>),
}

fn classify_line(line: &str) -> LineKind<'_> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(['#', '!', '[']) {
        return LineKind::Skip;
    }
    // Regras cosmeticas (`##`, `#@#`, `#?#`, `#$#`) e de excecao (`@@`).
    if line.starts_with("@@")
        || ["##", "#@#", "#?#", "#$#"]
            .iter()
            .any(|marker| line.contains(marker))
    {
        return LineKind::Ignored;
    }
    if let Some(rule) = line.strip_prefix("||") {
        // `||x^` e so isso: com opcoes (`$`), caminho ou outro `^`/`|` e
        // outra regra.
        let name = rule.strip_suffix('^').unwrap_or(rule);
        if name.contains(['^', '$', '|', '/', '*']) {
            return LineKind::Ignored;
        }
        return LineKind::Candidates(vec![name]);
    }
    let mut tokens = line
        .split_whitespace()
        .take_while(|token| !token.starts_with('#'));
    let Some(first) = tokens.next() else {
        return LineKind::Skip;
    };
    if HOSTS_SINKS.contains(&first) {
        let names: Vec<&str> = tokens.collect();
        return if names.is_empty() {
            LineKind::Ignored
        } else {
            LineKind::Candidates(names)
        };
    }
    if tokens.next().is_some() {
        return LineKind::Ignored;
    }
    LineKind::Candidates(vec![first])
}

/// Um dominio da lista: normalizado e com pelo menos dois rotulos.
fn list_domain(candidate: &str) -> Option<String> {
    normalize_domain(candidate).filter(|domain| domain.contains('.'))
}

/// Le uma lista de dominios: uma por linha, `0.0.0.0 x` ou `||x^`.
pub fn parse_domain_list(text: &str) -> ParsedList {
    let mut parsed = ParsedList::default();
    for line in text.lines() {
        let candidates = match classify_line(line) {
            LineKind::Skip => continue,
            LineKind::Ignored => {
                parsed.ignored += 1;
                continue;
            }
            LineKind::Candidates(candidates) => candidates,
        };
        for candidate in candidates {
            let Some(domain) = list_domain(candidate) else {
                parsed.ignored += 1;
                continue;
            };
            if !parsed.domains.has_room_for(&domain) {
                parsed.truncated = true;
                return parsed;
            }
            parsed.domains.insert(&domain);
        }
    }
    parsed
}

// ===================== a validacao do que chega da rede =====================

/// Porque uma lista baixada foi recusada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    /// Veio HTML (pelo `Content-Type` ou pelo primeiro byte): uma pagina, nao
    /// a lista.
    Html,
    /// Menos de `MIN_LIST_DOMAINS` dominios.
    TooFew { found: usize },
    /// Mais de `LIST_MAX_BYTES`.
    TooLarge,
    /// Nao e texto UTF-8.
    NotText,
}

impl ListError {
    pub fn pt_br_message(&self) -> String {
        match self {
            Self::Html => "A lista veio como página, não como lista".to_string(),
            Self::TooFew { found } => {
                format!("A lista veio só com {found} domínios (mínimo {MIN_LIST_DOMAINS})")
            }
            Self::TooLarge => "A lista passa de 2 MiB".to_string(),
            Self::NotText => "A lista não é texto".to_string(),
        }
    }
}

impl fmt::Display for ListError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.pt_br_message())
    }
}

/// O tipo de um `Content-Type` (sem parametros), em minusculas.
fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// O corpo comeca por `<` (depois de BOM e espacos): e marcacao.
fn looks_like_markup(body: &[u8]) -> bool {
    let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    body.iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'<')
}

/// Aceita a resposta da rede como lista, ou diz porque nao.
pub fn validate_list(content_type: Option<&str>, body: &[u8]) -> Result<ParsedList, ListError> {
    if body.len() > LIST_MAX_BYTES {
        return Err(ListError::TooLarge);
    }
    let html_type = content_type
        .map(media_type)
        .is_some_and(|kind| kind == "text/html" || kind == "application/xhtml+xml");
    if html_type || looks_like_markup(body) {
        return Err(ListError::Html);
    }
    let text = std::str::from_utf8(body).map_err(|_| ListError::NotText)?;
    let parsed = parse_domain_list(text);
    if parsed.domains.len() < MIN_LIST_DOMAINS {
        return Err(ListError::TooFew {
            found: parsed.domains.len(),
        });
    }
    Ok(parsed)
}

// ===================== o download =====================

/// A origem do download. Em release so existe a fixada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEndpoint {
    origin: String,
    /// O host fixado por HTTPS; `false` so no loopback dos testes.
    pinned: bool,
    locality: Locality,
}

impl ListEndpoint {
    /// O unico construtor que embarca: `https://pgl.yoyo.org`, resolvido so
    /// para enderecos publicos.
    pub fn pinned() -> Self {
        Self {
            origin: format!("https://{LIST_HOST}"),
            pinned: true,
            locality: Locality::Public,
        }
    }

    #[cfg(test)]
    pub(crate) fn loopback(port: u16) -> Self {
        Self {
            origin: format!("http://127.0.0.1:{port}"),
            pinned: false,
            locality: Locality::Loopback,
        }
    }

    pub fn locality(&self) -> Locality {
        self.locality
    }
}

/// Porque o download da lista falhou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// A rede, o estado HTTP (um 3xx nunca se segue), o prazo, o tecto.
    Transport(ApiError),
    /// Chegou, mas nao e a lista.
    List(ListError),
}

impl FetchError {
    pub fn pt_br_message(&self) -> String {
        match self {
            Self::Transport(error) => error.pt_br_message(),
            Self::List(error) => error.pt_br_message(),
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.pt_br_message())
    }
}

/// O cliente do download da lista.
pub struct ListClient {
    agent: ureq::Agent,
    endpoint: ListEndpoint,
}

impl ListClient {
    pub fn new(endpoint: ListEndpoint) -> Self {
        Self {
            agent: policy_agent(endpoint.pinned, endpoint.locality),
            endpoint,
        }
    }

    pub fn endpoint(&self) -> &ListEndpoint {
        &self.endpoint
    }

    /// A configuracao do agente (so leitura), para o gate da politica.
    pub fn agent_config(&self) -> &ureq::config::Config {
        self.agent.config()
    }

    /// Baixa e valida a lista, com o prazo de `LIST_TIMEOUT`.
    pub fn fetch(&self, cancelled: &dyn Fn() -> bool) -> Result<ParsedList, FetchError> {
        self.fetch_within(LIST_TIMEOUT, cancelled)
    }

    fn fetch_within(
        &self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ParsedList, FetchError> {
        let (content_type, body) = self
            .get(timeout, cancelled)
            .map_err(FetchError::Transport)?;
        validate_list(content_type.as_deref(), &body).map_err(FetchError::List)
    }

    /// O GET: `Accept: text/plain`, um 2xx ate `LIST_MAX_BYTES`
    /// descodificados; qualquer outro estado e erro sem ler o corpo.
    fn get(
        &self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Option<String>, Vec<u8>), ApiError> {
        if cancelled() {
            return Err(ApiError::Cancelled);
        }
        let deadline = Instant::now() + timeout;
        let url = format!("{}{LIST_PATH}", self.endpoint.origin);
        let mut response = self
            .agent
            .get(&url)
            .header("accept", "text/plain")
            .config()
            .timeout_global(Some(timeout))
            .build()
            .call()
            .map_err(map_transport_error)?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(match status {
                408 | 504 => ApiError::Timeout,
                _ => ApiError::ServiceUnavailable {
                    status: Some(status),
                },
            });
        }
        if let Some(declared) =
            header(&response, "content-length").and_then(|value| value.parse::<u64>().ok())
            && declared > LIST_MAX_BYTES as u64
        {
            return Err(ApiError::TooLarge {
                limit: LIST_MAX_BYTES as u64,
            });
        }
        let content_type = header(&response, "content-type").map(str::to_owned);
        let body = read_body(
            &mut response,
            LIST_MAX_BYTES,
            deadline,
            cancelled,
            Overflow::Fail,
        )?;
        Ok((content_type, body))
    }
}

// ===================== a decisao por pedido =====================

/// O tipo do recurso pedido (o `ResourceContext` do WebView2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// Um documento: a pagina de topo ou o de uma moldura.
    Document,
    Stylesheet,
    Image,
    Media,
    Font,
    Script,
    Xhr,
    Fetch,
    Other,
}

/// Porque um pedido passou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allow {
    /// O proprio documento: nunca se bloqueia uma navegacao.
    Document,
    /// Nao e http(s)/ws(s): `data:`, `blob:`, os esquemas `neuralia-*`.
    NotWeb,
    /// Loopback, `localhost`, `*.localhost` (as origens `neuralia-*` do wry).
    Local,
    /// A pagina e de uma IA ou de login (registo dos provedores).
    AiOrLogin,
    /// O utilizador permitiu anuncios neste site.
    SiteAllowed,
    /// Um pedido do proprio site da pagina.
    FirstParty,
    /// O host nao esta na lista.
    NotListed,
}

/// O veredicto de um pedido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow(Allow),
    /// Bloqueado por este dominio da lista.
    Block {
        listed: String,
    },
}

impl Decision {
    pub fn blocks(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
}

/// O site de uma pagina para a lista de permitidos e para o "proprio site":
/// o host sem `www.`, so em http(s).
pub fn site_key(url: &Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = request_host(url)?;
    Some(without_www(&host).to_string())
}

/// O host de um endereco, em minusculas e sem o ponto final.
fn request_host(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host).to_string();
    (!host.is_empty()).then_some(host)
}

/// Loopback: `localhost`, `*.localhost` (onde o WebView2 poe as origens
/// `neuralia-*` do wry), `127.0.0.0/8`, `::1` e o `::ffff:127.x`.
fn is_local_request(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => {
            let domain = domain.to_ascii_lowercase();
            let domain = domain.strip_suffix('.').unwrap_or(&domain);
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
        }
        None => false,
    }
}

/// Uma pagina onde o bloqueio nunca liga: uma IA ou um login, pelo registo
/// dos provedores. E tambem o que o menu mostra como "sempre desligado".
pub fn page_is_always_exempt(top: &Url) -> bool {
    if !matches!(top.scheme(), "http" | "https") {
        return false;
    }
    is_ai_provider_host(top) || top.host_str().is_some_and(is_login_host)
}

/// A lista baixada e a lista de permitidos, como o despachante as le.
#[derive(Debug, Clone, Default)]
pub struct AdblockRules {
    domains: Arc<DomainSet>,
    allow_sites: BTreeSet<String>,
}

impl AdblockRules {
    pub fn new(domains: Arc<DomainSet>, allow_sites: BTreeSet<String>) -> Self {
        Self {
            domains,
            allow_sites,
        }
    }

    pub fn domains(&self) -> &Arc<DomainSet> {
        &self.domains
    }

    pub fn allow_sites(&self) -> &BTreeSet<String> {
        &self.allow_sites
    }

    /// O utilizador permitiu anuncios no site desta pagina?
    pub fn site_allowed(&self, top: &Url) -> bool {
        site_key(top).is_some_and(|site| self.allow_sites.contains(&site))
    }

    /// O veredicto de um pedido a `request` feito pela pagina `top` (`None`:
    /// a pagina nao e web, como `about:blank`). Cada excecao do NEVER_BLOCK
    /// vem antes da lista; so o que sobra e procurado nela.
    pub fn decide(&self, request: &Url, top: Option<&Url>, kind: ResourceKind) -> Decision {
        if kind == ResourceKind::Document {
            return Decision::Allow(Allow::Document);
        }
        if !matches!(request.scheme(), "http" | "https" | "ws" | "wss") {
            return Decision::Allow(Allow::NotWeb);
        }
        if is_local_request(request) {
            return Decision::Allow(Allow::Local);
        }
        let Some(host) = request_host(request) else {
            return Decision::Allow(Allow::NotWeb);
        };
        if let Some(top) = top {
            if page_is_always_exempt(top) {
                return Decision::Allow(Allow::AiOrLogin);
            }
            if let Some(site) = site_key(top) {
                if self.allow_sites.contains(&site) {
                    return Decision::Allow(Allow::SiteAllowed);
                }
                if is_label_suffix(&host, &site) || is_label_suffix(&site, &host) {
                    return Decision::Allow(Allow::FirstParty);
                }
            }
        }
        match self.domains.find(&host) {
            Some(listed) => Decision::Block {
                listed: listed.to_string(),
            },
            None => Decision::Allow(Allow::NotListed),
        }
    }
}

// ===================== o que se guarda =====================

/// `adblock-settings.json` (loja `Setting`: so muda por uma escolha num
/// menu ou definicao).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdblockSettings {
    /// O utilizador clicou em "Ativar" (e nao desativou depois).
    #[serde(default)]
    pub enabled: bool,
    /// Os sites (host sem `www.`) com anuncios permitidos.
    #[serde(default)]
    pub allow_sites: BTreeSet<String>,
    /// Reservado para anti-distracao: `sites[host] = false` desliga ali.
    #[serde(default)]
    pub distraction: BTreeMap<String, bool>,
}

impl AdblockSettings {
    /// Sem entradas que nao sejam um site valido (um ficheiro editado a
    /// mao) e dentro do tecto.
    pub fn sanitized(mut self) -> Self {
        self.allow_sites = self
            .allow_sites
            .into_iter()
            .filter_map(|site| list_domain(&site).filter(|clean| *clean == site))
            .take(MAX_ALLOW_SITES)
            .collect();
        self
    }

    /// Bloquear (ou permitir) anuncios em `site`. `false` se nada mudou ou
    /// se o site nao e valido ou a lista esta cheia.
    pub fn set_site_blocking(&mut self, site: &str, block: bool) -> bool {
        let Some(site) = list_domain(site) else {
            return false;
        };
        if block {
            self.allow_sites.remove(&site)
        } else if self.allow_sites.len() >= MAX_ALLOW_SITES {
            false
        } else {
            self.allow_sites.insert(site)
        }
    }
}

/// A lista baixada no disco (loja `Automatic`): quando e os dominios, um
/// por linha. Ao ler, volta a passar pelo parser e pelos tectos.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredList {
    #[serde(default)]
    pub fetched_ms: u64,
    #[serde(default)]
    pub domains: String,
}

impl StoredList {
    pub fn from_set(domains: &DomainSet, fetched_ms: u64) -> Self {
        Self {
            fetched_ms,
            domains: domains.to_text(),
        }
    }

    /// Nada guardado (a loja sem ficheiro).
    pub fn is_empty(&self) -> bool {
        self.domains.trim().is_empty()
    }

    pub fn parse(&self) -> ParsedList {
        parse_domain_list(&self.domains)
    }
}

// ===================== a renovacao =====================

/// Onde o utilizador esta quando se pergunta pela renovacao.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshSurface {
    /// A Home: nunca se baixa nada aqui.
    Home,
    /// Uma superficie web aberta (comparador, Web completa).
    Web,
    /// O Leitor, o PDF, os livros.
    Local,
}

/// O que a renovacao precisa de saber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshState {
    pub enabled: bool,
    /// Quando a lista guardada foi baixada; `None` sem lista.
    pub fetched_ms: Option<u64>,
    /// Um download ja esta a correr.
    pub in_flight: bool,
    /// A ultima tentativa automatica falhada.
    pub last_failure_ms: Option<u64>,
    /// Modo privado: as lojas `Automatic` nao se escrevem, a lista nao se
    /// renova.
    pub private: bool,
}

/// A lista guardada tem mais de uma semana (ou nao existe, ou o relogio
/// andou para tras).
fn list_is_stale(now_ms: u64, fetched_ms: Option<u64>) -> bool {
    match fetched_ms {
        None => true,
        Some(fetched) => now_ms < fetched || now_ms - fetched >= REFRESH_EVERY_MS,
    }
}

/// A renovacao automatica: so com o bloqueio ativo, fora do modo privado,
/// numa superficie web (nunca na Home), sem outro download a correr, no
/// maximo uma vez por semana e, depois de uma falha, so passada uma hora.
pub fn refresh_due(now_ms: u64, surface: RefreshSurface, state: &RefreshState) -> bool {
    if !state.enabled || state.private || state.in_flight {
        return false;
    }
    if surface != RefreshSurface::Web {
        return false;
    }
    if state
        .last_failure_ms
        .is_some_and(|failed| now_ms >= failed && now_ms - failed < RETRY_AFTER_FAILURE_MS)
    {
        return false;
    }
    list_is_stale(now_ms, state.fetched_ms)
}

/// O clique em "Ativar": baixa so se a lista guardada nao serve (nenhuma,
/// ou com mais de uma semana) e nunca no modo privado.
pub fn download_on_activation(now_ms: u64, fetched_ms: Option<u64>, private: bool) -> bool {
    !private && list_is_stale(now_ms, fetched_ms)
}

#[cfg(test)]
mod tests;
