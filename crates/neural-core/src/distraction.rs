//! Anti-distracao (plano 2.4, item anti-distracao; §7: script injetado
//! novo, com o sim do dono em principio -- cada PR espera o sim dele com a
//! sabotagem).
//!
//! Esconde avisos de cookies, janelas de newsletter e barras fixas grandes
//! nas paginas da internet. Nos CMPs conhecidos (`CMP_RULES`) pode CLICAR,
//! e so em «Rejeitar» / «So necessarios»: o clique pede as tres coisas ao
//! mesmo tempo -- o seletor de recusa do CMP casa, o texto visivel do botao
//! tem uma frase de `REJECT_WORDS` e nao tem nenhuma palavra de
//! `ACCEPT_WORDS`. Faltando uma, o aviso so e escondido. Nunca aceita.
//!
//! Este modulo e puro (sem rede, sem disco, sem janelas): as regras, as
//! listas de palavras e marcadores, a politica por site
//! (`DistractionPolicy`, guardada no campo `distraction` do
//! `adblock-settings.json`) e a decisao `distraction_config` -- NUNCA nas
//! paginas das IAs nem de login (registo dos provedores:
//! `search::is_ai_provider_host` e `search::is_login_host`), nas nossas
//! origens (`localhost`, `*.localhost`, IPs), nos paineis de servico, no
//! Leitor, no PDF, nos livros nem na Home. O script que embarca
//! (`NEURALIA_DISTRACTION_SCRIPT`, no `neural-app`) recebe
//! `script_config(policy)` e repete a mesma decisao no inicio do documento,
//! onde o endereco e conhecido; os gates do app correm-no no `node:vm` e
//! comparam-no com `distraction_config`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::{Host, Url};

use crate::domains::{normalize_domain, without_www};
use crate::search::{ProviderId, is_ai_provider_host, is_login_host, login_hosts};

// ===================== as regras dos CMPs =====================

/// Um CMP (plataforma de consentimento) que o script reconhece.
///
/// - `detect`: seletor no documento que diz que o CMP esta na pagina (nos
///   CMPs com shadow DOM, o proprio anfitriao).
/// - `reject`: o botao de recusa, procurado no documento ou, com
///   `shadow_host`, dentro da shadow root aberta desse anfitriao. `None`:
///   nunca se clica neste CMP.
/// - `banner`: o que se esconde (sempre no documento), e so o proprio
///   aviso: nunca um anfitriao que fica na pagina e mostra tambem as
///   definicoes que o utilizador abre depois pelo rodape
///   (`PERSISTENT_CMP_HOSTS`: o centro de preferencias do OneTrust, o
///   `#didomi-host`, a raiz do Usercentrics). O script so age quando um
///   deles esta visivel. `None`: nunca se esconde nada -- a pagina
///   `consent.google.com` E o aviso, e o Usercentrics vive todo na shadow
///   root de um anfitriao persistente; ai o sinal de que ha um aviso e o
///   botao de recusa visivel.
/// - `frame_only`: o aviso vive numa moldura de outra origem (Sourcepoint,
///   TrustArc), onde o script da pagina de topo nao entra nem le -- nem um
///   "pague ou aceite" (pur abo, contentpass). Nunca se clica; so se
///   esconde a moldura pequena (menos de `FRAME_WALL_RATIO` da altura da
///   janela) numa pagina que rola, e esconde-la nunca devolve a rolagem nem
///   tira fundos. A que tapa a pagina fica, e com ela na pagina o script
///   deixa a rolagem e os fundos como estao; a pequena numa pagina com a
///   rolagem presa tambem fica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmpRule {
    pub id: &'static str,
    pub detect: &'static str,
    pub reject: Option<&'static str>,
    pub banner: Option<&'static str>,
    pub shadow_host: Option<&'static str>,
    pub frame_only: bool,
}

/// Os CMPs conhecidos.
///
/// Proveniencia: os ids, classes e atributos abaixo sao os que cada
/// fornecedor poe no DOM do seu proprio aviso (a documentacao publica de
/// cada um e as paginas de demonstracao deles), tal como aparecem tambem
/// nas regras publicas do Consent-O-Matic (MIT) e do autoconsent do
/// DuckDuckGo (MPL-2.0) -- so os nomes dos seletores, nenhum codigo nem
/// texto delas. Um CMP muda o DOM sem aviso: um seletor que deixa de casar
/// faz o script so esconder (ou nada), nunca clicar noutro botao, porque o
/// clique pede tambem o texto certo. Cada linha tem uma fixture nos gates
/// do app (`every_cmp_rule_has_a_fixture`); os seletores usam so `#id`,
/// `.classe`, `tag`, `[a]`, `[a="v"]`, `[a^="v"]`, `[a*="v"]`, `[a$="v"]`,
/// o descendente e o `>` (o que o DOM falso dos gates entende).
pub const CMP_RULES: &[CmpRule] = &[
    CmpRule {
        id: "onetrust",
        detect: "#onetrust-consent-sdk, #onetrust-banner-sdk",
        // So o aviso: o centro de preferencias (`#onetrust-pc-sdk`) e o
        // fundo `.onetrust-pc-dark-filter` ficam na pagina para as
        // definicoes do rodape (o fundo ao lado do aviso escondido sai pela
        // regra dos fundos do script).
        reject: Some("#onetrust-reject-all-handler"),
        banner: Some("#onetrust-banner-sdk"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "cookiebot",
        detect: "#CybotCookiebotDialog",
        reject: Some(
            "#CybotCookiebotDialogBodyButtonDecline, #CybotCookiebotDialogBodyLevelButtonLevelOptinDeclineAll",
        ),
        banner: Some("#CybotCookiebotDialog, #CybotCookiebotDialogBodyUnderlay"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "didomi",
        detect: "#didomi-host, #didomi-notice",
        reject: Some("#didomi-notice-disagree-button"),
        // O `#didomi-host` fica: e onde o Didomi mostra as preferencias.
        banner: Some("#didomi-notice, .didomi-popup-backdrop"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "usercentrics",
        detect: "#usercentrics-root, #usercentrics-cmp-ui",
        reject: Some(r#"[data-testid="uc-deny-all-button"]"#),
        // O anfitriao e persistente (mostra tambem as definicoes): nunca se
        // esconde; so a recusa com o texto certo.
        banner: None,
        shadow_host: Some("#usercentrics-root, #usercentrics-cmp-ui"),
        frame_only: false,
    },
    CmpRule {
        id: "quantcast",
        detect: "#qc-cmp2-ui, .qc-cmp2-container",
        reject: Some(r#".qc-cmp2-summary-buttons button[mode="secondary"]"#),
        banner: Some(".qc-cmp2-container, #qc-cmp2-container"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "cookieyes",
        detect: ".cky-consent-container",
        reject: Some(".cky-btn-reject"),
        banner: Some(".cky-consent-container, .cky-overlay, .cky-modal"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "complianz",
        detect: "#cmplz-cookiebanner-container, .cmplz-cookiebanner",
        reject: Some(".cmplz-cookiebanner .cmplz-deny"),
        banner: Some("#cmplz-cookiebanner-container, .cmplz-cookiebanner, .cmplz-soft-cookiewall"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "iubenda",
        detect: "#iubenda-cs-banner",
        reject: Some(".iubenda-cs-reject-btn"),
        banner: Some("#iubenda-cs-banner, .iubenda-cs-overlay"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "osano",
        detect: ".osano-cm-window",
        reject: Some(".osano-cm-denyAll, .osano-cm-button--type_denyAll"),
        banner: Some(".osano-cm-window"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "axeptio",
        detect: "#axeptio_overlay",
        reject: Some("#axeptio_btn_dismiss"),
        banner: Some("#axeptio_overlay"),
        shadow_host: None,
        frame_only: false,
    },
    CmpRule {
        id: "termly",
        detect: r#"#termly-code-snippet-support, [data-tid="banner-decline"]"#,
        reject: Some(r#"[data-tid="banner-decline"]"#),
        banner: Some("#termly-code-snippet-support"),
        shadow_host: None,
        frame_only: false,
    },
    // A pagina consent.google.com (e o dialogo que o www.google.com da no
    // EEE) e o proprio aviso: nunca se esconde. As duas formas enviam para
    // `/save`; so o botao com o texto de recusa e clicado.
    CmpRule {
        id: "google-consent",
        detect: r#"form[action*="consent.google.com"], #W0wltc"#,
        reject: Some(r#"form[action$="/save"] button, #W0wltc"#),
        banner: None,
        shadow_host: None,
        frame_only: false,
    },
    // Molduras de outra origem: nunca clicar; so esconder a pequena (ver
    // `frame_only`). O "pague ou aceite" destes CMPs vive dentro da
    // moldura, onde o script nao le.
    CmpRule {
        id: "sourcepoint",
        detect: r#"[id^="sp_message_container_"], iframe[id^="sp_message_iframe_"]"#,
        reject: None,
        banner: Some(r#"[id^="sp_message_container_"]"#),
        shadow_host: None,
        frame_only: true,
    },
    CmpRule {
        id: "trustarc",
        detect: r#".truste_box_overlay, iframe[src*="consent-pref.trustarc.com"]"#,
        reject: None,
        banner: Some(".truste_box_overlay, .truste_overlay"),
        shadow_host: None,
        frame_only: true,
    },
];

// ===================== as palavras =====================
//
// Todas ja normalizadas como o script normaliza o texto de um botao
// (`normalize_label`: minusculas; o que nao e letra nem digito vira um
// espaco; espacos seguidos viram um). Gate `word_lists_are_normalized`.

/// As frases de recusa (pt, en, es, fr, de, it). O texto de um botao casa
/// quando tem uma delas como sequencia de palavras inteiras
/// («Rejeitar tudo» tem «rejeitar»).
pub const REJECT_WORDS: &[&str] = &[
    // pt
    "rejeitar",
    "recusar",
    "só necessários",
    "so necessarios",
    "apenas necessários",
    "apenas necessarios",
    "apenas os necessários",
    "somente necessários",
    "somente necessarios",
    "apenas essenciais",
    "só essenciais",
    // en
    "reject",
    "decline",
    "deny",
    "refuse",
    "disagree",
    "necessary only",
    "only necessary",
    "necessary cookies only",
    "essential only",
    "only essential",
    // es
    "rechazar",
    "denegar",
    "solo necesarias",
    "sólo necesarias",
    "solo las necesarias",
    "solo esenciales",
    // fr
    "refuser",
    "rejeter",
    "nécessaires uniquement",
    "necessaires uniquement",
    "uniquement les nécessaires",
    // de
    "ablehnen",
    "verweigern",
    "nur notwendige",
    "nur erforderliche",
    "nur essenzielle",
    // it
    "rifiuta",
    "rifiuto",
    "rifiutare",
    "solo necessari",
    "solo essenziali",
];

/// As palavras de aceitar (pt, en, es, fr, de, it). Uma entrada com 4 ou
/// mais caracteres casa no INICIO de qualquer palavra do texto (`aceit`
/// apanha aceitar, aceito, aceitá-los; `agree` apanha agreed, e nao
/// disagree); uma mais curta so casa a palavra inteira (`ok`, `sim`,
/// `si`). Um texto com qualquer uma nunca e clicado -- nem «Rejeitar e
/// aceitar», nem «Continuar sem aceitar»: nesses o aviso so e escondido.
pub const ACCEPT_WORDS: &[&str] = &[
    // pt
    "aceit",
    "concord",
    "permit",
    "autoriz",
    "entend",
    "aprov",
    "sim",
    // en
    "accept",
    "agree",
    "allow",
    "consent",
    "continu",
    "understood",
    "got it",
    "okay",
    "ok",
    "yes",
    // es
    "acept",
    "acuerdo",
    "vale",
    "si",
    "sí",
    // fr
    "accord",
    "autoris",
    "oui",
    // de
    "akzept",
    "zustimm",
    "einverstanden",
    "erlaub",
    "annehm",
    "ja",
    // it
    "accett",
    "acconsent",
];

/// Marcadores de paywall (texto ou `id`/`class`, normalizados; no
/// documento, `id`/`class` sem olhar a maiusculas): um candidato com um
/// deles fica intocado, e depois de o script ver um (num candidato, num
/// aviso de CMP ou no documento) nunca mais devolve a rolagem nem esconde
/// um fundo nessa pagina -- e a rolagem que ja tinha devolvido volta a ser
/// a da pagina.
pub const PAYWALL_MARKERS: &[&str] = &[
    "paywall",
    "regwall",
    "tp modal",
    "tp backdrop",
    "tp iframe wrapper",
    "poool",
    "contentpass",
    "pur abo",
    "subscriber only",
    "subscribers only",
    "assine para continuar",
    "assine para ler",
    "exclusivo para assinantes",
    "subscribe to continue",
    "subscribe to read",
    "suscríbete para seguir",
    "suscribete para seguir",
    "abonnez vous pour",
    "réservé aux abonnés",
    "reserve aux abonnes",
    "jetzt abonnieren",
    "abbonati per",
    "riservato agli abbonati",
];

/// Marcadores de uma janela de newsletter (normalizados). No `id`/`class`
/// de uma janela ou barra fixa bastam; no TEXTO de um dialogo so contam
/// quando o unico campo dele e um e-mail (um checkout com a caixa «receber
/// a newsletter» fica). Sem o «boletim» do pt: e tambem o boletim de
/// ocorrencia, o escolar, o de voto.
pub const NEWSLETTER_MARKERS: &[&str] = &[
    "newsletter",
    "boletín",
    "boletin",
    "infolettre",
    "lettre d information",
    "email capture",
    "emailcapture",
    "signup popup",
];

/// Os anfitrioes que um CMP deixa na pagina e reusa para as definicoes
/// que o utilizador abre depois (o rodape «Definicoes de cookies»): nenhum
/// `banner` de `CMP_RULES` os nomeia (gate
/// `cmp_banners_never_name_a_persistent_host`).
pub const PERSISTENT_CMP_HOSTS: &[&str] = &[
    "#onetrust-consent-sdk",
    "#onetrust-pc-sdk",
    ".onetrust-pc-dark-filter",
    "#didomi-host",
    "#usercentrics-root",
    "#usercentrics-cmp-ui",
];

/// Uma barra fixa (ou presa) na janela, encostada ao topo ou ao fundo, so
/// e escondida com pelo menos esta fracao da altura da janela.
pub const STICKY_MIN_RATIO: f64 = 0.25;
/// Acima desta fracao ja nao e uma barra: e a pagina (um layout fixo).
pub const STICKY_MAX_RATIO: f64 = 0.9;
/// Uma moldura de outra origem (`frame_only`) com pelo menos esta fracao
/// da altura da janela tapa a pagina: pode ser um "pague ou aceite" que o
/// script nao le, e fica.
pub const FRAME_WALL_RATIO: f64 = 0.5;
/// O exame dos elementos de cada passagem, em ms, antes de ceder a vez.
pub const FRAME_BUDGET_MS: u32 = 8;
/// A procura dos CMPs conhecidos (umas consultas ao documento inteiro)
/// corre no maximo uma vez neste intervalo, em ms, numa pagina que muda
/// sem parar.
pub const CMP_PASS_EVERY_MS: u32 = 250;
/// O script para de observar a pagina depois disto sem mudancas.
pub const IDLE_STOP_MS: u32 = 30_000;
/// O que aparece ate este tempo depois de um clique ou de uma tecla do
/// utilizador foi ele que abriu: nunca e escondido nem clicado.
pub const USER_OPENED_MS: u32 = 1_500;
/// Um botao com mais do que isto de texto (normalizado) nunca e clicado.
pub const MAX_LABEL_CHARS: usize = 60;
/// Os sites com escolha propria na politica.
pub const MAX_DISTRACTION_SITES: usize = 1_000;

/// O texto de um botao como o script o compara: minusculas; o que nao e
/// letra nem digito vira um espaco; espacos seguidos viram um; sem espacos
/// nas pontas. (O script faz o mesmo com uma letra = `lower != upper`,
/// que nas letras latinas destas listas da o mesmo resultado.)
pub fn normalize_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        } else {
            space = true;
        }
    }
    out
}

/// Os pedacos de um seletor, como nomes: `#didomi-notice-disagree-button`
/// da `didomi`, `notice`, `disagree`, `button`; o camelCase tambem parte
/// (`ButtonDecline` da `button`, `decline`).
pub fn selector_tokens(selector: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in selector.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && previous_lower && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
            current.extend(ch.to_lowercase());
        } else {
            previous_lower = false;
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Uma palavra de `ACCEPT_WORDS` no inicio (ou, curta, inteira) de um
/// destes pedacos.
pub fn names_accept(tokens: &[String]) -> Option<&'static str> {
    ACCEPT_WORDS.iter().copied().find(|word| {
        tokens.iter().any(|token| {
            if word.chars().count() >= 4 {
                token.starts_with(word)
            } else {
                token == word
            }
        })
    })
}

// ===================== a politica por site =====================

/// A chave do padrao no mapa guardado: nunca um site valido.
const DEFAULT_KEY: &str = "*";

/// «Ocultar distracoes»: ligado por omissao, com a escolha de cada site.
/// Guardada no campo `distraction` do `adblock-settings.json` (loja
/// `Setting`) com a forma que a 2.3 reservou -- um mapa `site -> bool`; o
/// padrao desligado vai como `"*": false`. Um ficheiro da 2.3 (`{}`) le-se
/// como tudo ligado, e a 2.3 le o desta versao sem perder nada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "BTreeMap<String, bool>", into = "BTreeMap<String, bool>")]
pub struct DistractionPolicy {
    /// «Ocultar distracoes (cookies, newsletter, barras fixas)».
    pub default_on: bool,
    /// Os sites (host sem `www.`) com escolha propria, diferente do padrao.
    pub sites: BTreeMap<String, bool>,
}

impl Default for DistractionPolicy {
    fn default() -> Self {
        Self {
            default_on: true,
            sites: BTreeMap::new(),
        }
    }
}

impl From<BTreeMap<String, bool>> for DistractionPolicy {
    fn from(mut map: BTreeMap<String, bool>) -> Self {
        let default_on = map.remove(DEFAULT_KEY).unwrap_or(true);
        Self {
            default_on,
            sites: map,
        }
    }
}

impl From<DistractionPolicy> for BTreeMap<String, bool> {
    fn from(policy: DistractionPolicy) -> Self {
        let mut map = policy.sites;
        map.remove(DEFAULT_KEY);
        if !policy.default_on {
            map.insert(DEFAULT_KEY.to_string(), false);
        }
        map
    }
}

/// O que uma escolha fez na politica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteChange {
    Changed,
    Unchanged,
    /// Nao e um site que se guarde (um IP, `localhost`), ou a lista esta
    /// cheia.
    Refused,
}

/// Um site que a politica guarda: um nome de dominio normalizado com pelo
/// menos dois rotulos, nunca `*.localhost` (as nossas origens).
pub fn distraction_site_key(site: &str) -> Option<String> {
    let site = normalize_domain(site).filter(|domain| domain.contains('.'))?;
    let site = without_www(&site).to_string();
    (!is_local_name(&site)).then_some(site)
}

fn is_local_name(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost")
}

impl DistractionPolicy {
    /// Sem entradas que nao sejam um site valido (um ficheiro editado a
    /// mao), sem as que repetem o padrao e dentro do tecto.
    pub fn sanitized(self) -> Self {
        let default_on = self.default_on;
        let sites = self
            .sites
            .into_iter()
            .filter(|(site, on)| {
                *on != default_on && distraction_site_key(site).as_deref() == Some(site.as_str())
            })
            .take(MAX_DISTRACTION_SITES)
            .collect();
        Self { default_on, sites }
    }

    /// Ligado neste site (a chave de `distraction_site_key`)?
    pub fn site_on(&self, site: &str) -> bool {
        self.sites.get(site).copied().unwrap_or(self.default_on)
    }

    /// A escolha «Ocultar distracoes neste site». Igual ao padrao, o site
    /// deixa de ter escolha propria (segue o padrao se ele mudar).
    pub fn set_site(&mut self, site: &str, on: bool) -> SiteChange {
        let Some(site) = distraction_site_key(site) else {
            return SiteChange::Refused;
        };
        if self.site_on(&site) == on {
            return SiteChange::Unchanged;
        }
        if on == self.default_on {
            self.sites.remove(&site);
        } else {
            if self.sites.len() >= MAX_DISTRACTION_SITES {
                return SiteChange::Refused;
            }
            self.sites.insert(site, on);
        }
        SiteChange::Changed
    }

    /// Esta politica com as escolhas feitas no Split privado por cima (so
    /// em memoria; nunca gravadas).
    pub fn with_overlay(&self, overlay: &BTreeMap<String, bool>) -> Self {
        let mut policy = self.clone();
        for (site, on) in overlay {
            if *on == policy.default_on {
                policy.sites.remove(site);
            } else {
                policy.sites.insert(site.clone(), *on);
            }
        }
        policy
    }
}

// ===================== a decisao =====================

/// Onde a pagina esta. So a `Web` (as colunas, a fonte ao lado -- normal ou
/// privada -- e a Web completa) recebe o script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistractionSurface {
    Web,
    Home,
    Reader,
    Pdf,
    Books,
    /// Meet, WhatsApp, YouTube, Gmail, Respiracao; o monitor do Gmail.
    ServicePanel,
    /// Os paineis nossos: historico, notas, Gemini Live.
    AppPanel,
}

/// O que o script faria nesta pagina: o site (a chave da politica).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistractionConfig {
    pub site: String,
}

/// A pagina e de uma IA ou de um login (registo dos provedores). O host
/// com o ponto final (`chatgpt.com.`) e o mesmo site sem ele.
pub fn page_is_exempt(url: &Url) -> bool {
    if is_ai_provider_host(url) || url.host_str().is_some_and(is_login_host) {
        return true;
    }
    let Some(host) = url.host_str().and_then(|host| host.strip_suffix('.')) else {
        return false;
    };
    let mut plain = url.clone();
    plain.set_host(Some(host)).is_ok() && (is_ai_provider_host(&plain) || is_login_host(host))
}

/// O site de uma pagina onde o script pode agir, sem olhar para a
/// politica: http(s), um nome de dominio (nao um IP, nao `localhost` nem
/// `*.localhost`), fora das IAs e dos logins, numa superficie `Web`.
pub fn distraction_site(url: &Url, surface: DistractionSurface) -> Option<String> {
    if surface != DistractionSurface::Web || !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if !matches!(url.host(), Some(Host::Domain(_))) || page_is_exempt(url) {
        return None;
    }
    distraction_site_key(url.host_str()?)
}

/// A decisao: `None` nas IAs e nos logins, nas nossas origens, fora da
/// superficie `Web` (paineis de servico, Leitor, PDF, livros, Home) e nos
/// sites com a escolha desligada.
pub fn distraction_config(
    url: &Url,
    policy: &DistractionPolicy,
    surface: DistractionSurface,
) -> Option<DistractionConfig> {
    let site = distraction_site(url, surface)?;
    policy.site_on(&site).then_some(DistractionConfig { site })
}

/// Um seletor que acha um elemento com um destes marcadores no `id` ou na
/// `class`, sem olhar a maiusculas (`PaywallModal`); os espacos viram `-`.
fn marker_selector(markers: &[&str]) -> String {
    let mut parts = Vec::new();
    for marker in markers {
        let marker = marker.replace(' ', "-");
        parts.push(format!(r#"[class*="{marker}" i]"#));
        parts.push(format!(r#"[id*="{marker}" i]"#));
    }
    parts.join(", ")
}

/// Tudo o que o script recebe, gravado no texto dele quando e ligado a uma
/// WebView (`fill_distraction_script` no app): a politica, as regras do
/// registo dos provedores e dos logins (a decisao no inicio do documento),
/// os CMPs, as palavras, os marcadores e os limites.
pub fn script_config(policy: &DistractionPolicy) -> Value {
    let never: Vec<Value> = ProviderId::all()
        .iter()
        .flat_map(|provider| provider.hosts())
        .map(|rule| json!([rule.host, rule.subdomains, rule.require_udm50]))
        .collect();
    let cmp: Vec<Value> = CMP_RULES
        .iter()
        .map(|rule| {
            json!({
                "id": rule.id,
                "detect": rule.detect,
                "reject": rule.reject,
                "banner": rule.banner,
                "shadow": rule.shadow_host,
                "frame": rule.frame_only,
            })
        })
        .collect();
    let sites: serde_json::Map<String, Value> = policy
        .sites
        .iter()
        .map(|(site, on)| (site.clone(), Value::Bool(*on)))
        .collect();
    json!({
        "on": policy.default_on,
        "sites": sites,
        "never": never,
        "login": login_hosts(),
        "cmp": cmp,
        "reject": REJECT_WORDS,
        "accept": ACCEPT_WORDS,
        "paywall": PAYWALL_MARKERS,
        "paywallSel": marker_selector(PAYWALL_MARKERS),
        "newsletter": NEWSLETTER_MARKERS,
        "stickyMin": STICKY_MIN_RATIO,
        "stickyMax": STICKY_MAX_RATIO,
        "frameWall": FRAME_WALL_RATIO,
        "budgetMs": FRAME_BUDGET_MS,
        "cmpMs": CMP_PASS_EVERY_MS,
        "idleMs": IDLE_STOP_MS,
        "userMs": USER_OPENED_MS,
        "maxLabel": MAX_LABEL_CHARS,
    })
}

#[cfg(test)]
mod tests;
