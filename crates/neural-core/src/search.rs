use crate::{NeuralError, Result};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use url::Url;

/// Identificador único para cada provedor de IA suportado pelo NeuralIA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProviderId {
    GoogleAi,
    ChatGpt,
    Claude,
    Perplexity,
    Gemini,
    DeepSeek,
    Copilot,
    Grok,
    Mistral,
}

/// Um host de um provedor, como o registro o reconhece.
///
/// `host` vale sempre por inteiro; com `subdomains`, também qualquer
/// `<algo>.host` (o ponto faz parte da regra: `evilchatgpt.com` não é
/// subdomínio de `chatgpt.com`). Com `require_udm50`, o endereço só conta se
/// o PRIMEIRO `udm` da query for `50` (o Modo IA do Google), tal como o
/// `searchParams.get('udm')` do script das colunas lê.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostRule {
    pub host: &'static str,
    pub subdomains: bool,
    pub require_udm50: bool,
}

impl HostRule {
    /// Só este host.
    pub const fn exact(host: &'static str) -> Self {
        Self {
            host,
            subdomains: false,
            require_udm50: false,
        }
    }

    /// Este host e os seus subdomínios.
    pub const fn with_subdomains(host: &'static str) -> Self {
        Self {
            host,
            subdomains: true,
            require_udm50: false,
        }
    }

    /// Só este host, e só com o primeiro `udm` igual a `50`.
    pub const fn udm50(host: &'static str) -> Self {
        Self {
            host,
            subdomains: false,
            require_udm50: true,
        }
    }

    /// O nome de host (já em minúsculas) é este, ou um subdomínio dele quando
    /// a regra os aceita.
    pub fn matches_host(&self, host: &str) -> bool {
        host == self.host
            || (self.subdomains
                && host
                    .strip_suffix(self.host)
                    .and_then(|label| label.strip_suffix('.'))
                    .is_some_and(|label| !label.is_empty()))
    }

    /// O URL é http(s), o host bate e, se a regra pede, o primeiro `udm` é 50.
    pub fn matches(&self, url: &Url) -> bool {
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        self.matches_host(&host.to_ascii_lowercase())
            && (!self.require_udm50 || first_query_value(url, "udm").as_deref() == Some("50"))
    }
}

/// O primeiro valor de `name` na query, como o `URLSearchParams.get` do
/// navegador (um `udm=14&udm=50` é 14).
fn first_query_value(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// Metadados estáticos de um provedor de IA.
#[derive(Debug, Clone, Copy)]
pub struct ProviderInfo {
    pub id: ProviderId,
    pub key: &'static str,
    pub display_name: &'static str,
    pub is_selectable: bool,
    pub default_slot: Option<usize>,
    /// A fonte única dos hosts: `from_url`, `is_ai_provider_host` e os gates
    /// leem daqui.
    pub hosts: &'static [HostRule],
    /// A fonte única dos nomes próprios: `all_self_names` junta-os daqui.
    pub self_names: &'static [&'static str],
    pub answer_selector: Option<&'static str>,
}

/// O registro. Só os 3 slots padrão são selecionáveis até ao item
/// `providers` (Wave 5), que torna cada um dos outros selecionável depois do
/// seu smoke test; até lá, as outras linhas servem para reconhecer os hosts e
/// os nomes (adblock, anti-distração, anonimizador do juiz).
///
/// Hosts (brief `infra-provider-registry`): `chatgpt.com` e subdomínios,
/// `chat.openai.com`, `claude.ai`, `gemini.google.com`, `google.com` e
/// `www.google.com` só com `udm=50`, `perplexity.ai`, `chat.deepseek.com`,
/// `copilot.microsoft.com`, `grok.com`, `chat.mistral.ai`. Duas extensões,
/// ambas o que o script das colunas que embarca já aceita hoje:
/// `*.claude.ai` (o `onProviderPage` aceita-o) e `www.perplexity.ai` (o
/// endereço que `perplexity_search_url` abre).
static PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: ProviderId::GoogleAi,
        key: "google",
        display_name: "Google IA",
        is_selectable: true,
        default_slot: Some(0),
        hosts: &[
            HostRule::udm50("google.com"),
            HostRule::udm50("www.google.com"),
        ],
        self_names: &["Google IA", "AI Mode", "Modo IA"],
        answer_selector: Some(r#"main, [data-message-author-role="assistant"], [role="article"]"#),
    },
    ProviderInfo {
        id: ProviderId::ChatGpt,
        key: "chatgpt",
        display_name: "ChatGPT",
        is_selectable: true,
        default_slot: Some(1),
        hosts: &[
            HostRule::with_subdomains("chatgpt.com"),
            HostRule::exact("chat.openai.com"),
        ],
        self_names: &[
            "ChatGPT", "OpenAI", "GPT-4", "GPT-3", "GPT-5", "GPT-o1", "GPT-4o", "GPT-n", "GPT",
        ],
        answer_selector: Some(r#"[data-message-author-role="assistant"]"#),
    },
    ProviderInfo {
        id: ProviderId::Claude,
        key: "claude",
        display_name: "Claude",
        is_selectable: true,
        default_slot: Some(2),
        hosts: &[HostRule::with_subdomains("claude.ai")],
        self_names: &["Claude", "Anthropic"],
        answer_selector: Some(r#"[data-message-author-role="assistant"]"#),
    },
    ProviderInfo {
        id: ProviderId::Perplexity,
        key: "perplexity",
        display_name: "Perplexity",
        is_selectable: false,
        default_slot: None,
        hosts: &[
            HostRule::exact("perplexity.ai"),
            HostRule::exact("www.perplexity.ai"),
        ],
        self_names: &["Perplexity"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Gemini,
        key: "gemini",
        display_name: "Gemini",
        is_selectable: false,
        default_slot: None,
        hosts: &[HostRule::exact("gemini.google.com")],
        self_names: &["Gemini", "Bard"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::DeepSeek,
        key: "deepseek",
        display_name: "DeepSeek",
        is_selectable: false,
        default_slot: None,
        hosts: &[HostRule::exact("chat.deepseek.com")],
        self_names: &["DeepSeek"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Copilot,
        key: "copilot",
        display_name: "Copilot",
        is_selectable: false,
        default_slot: None,
        hosts: &[HostRule::exact("copilot.microsoft.com")],
        self_names: &["Copilot", "Microsoft Copilot", "Microsoft"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Grok,
        key: "grok",
        display_name: "Grok",
        is_selectable: false,
        default_slot: None,
        hosts: &[HostRule::exact("grok.com")],
        self_names: &["Grok", "xAI"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Mistral,
        key: "mistral",
        display_name: "Mistral",
        is_selectable: false,
        default_slot: None,
        hosts: &[HostRule::exact("chat.mistral.ai")],
        self_names: &["Mistral", "Le Chat"],
        answer_selector: None,
    },
];

static LOGIN_HOSTS: &[&str] = &[
    "accounts.google.com",
    "login.live.com",
    "login.microsoftonline.com",
    "appleid.apple.com",
    "auth.openai.com",
    "auth0.openai.com",
];

impl ProviderId {
    /// Todos os provedores cadastrados no registro: os 8 do design v1 de
    /// provedores e o Mistral, que nunca é um slot.
    pub const fn all() -> &'static [ProviderId] {
        &[
            ProviderId::GoogleAi,
            ProviderId::ChatGpt,
            ProviderId::Claude,
            ProviderId::Perplexity,
            ProviderId::Gemini,
            ProviderId::DeepSeek,
            ProviderId::Copilot,
            ProviderId::Grok,
            ProviderId::Mistral,
        ]
    }

    /// Os provedores que se podem escolher hoje: só os 3 slots padrão. Os
    /// outros 5 do design v1 (Perplexity, Gemini, DeepSeek, Copilot e Grok)
    /// ficam no registro para serem reconhecidos e passam a selecionáveis no
    /// item `providers` (Wave 5), cada um depois do seu smoke test.
    pub const fn selectable() -> &'static [ProviderId] {
        &[
            ProviderId::GoogleAi,
            ProviderId::ChatGpt,
            ProviderId::Claude,
        ]
    }

    /// As 3 IAs das colunas padrão do comparador (slots 0, 1, 2).
    pub const fn default_slots() -> [ProviderId; 3] {
        [
            ProviderId::GoogleAi,
            ProviderId::ChatGpt,
            ProviderId::Claude,
        ]
    }

    /// Informações completas do provedor.
    pub fn info(self) -> &'static ProviderInfo {
        PROVIDERS
            .iter()
            .find(|p| p.id == self)
            .expect("provedor cadastrado")
    }

    pub fn key(self) -> &'static str {
        self.info().key
    }

    pub fn display_name(self) -> &'static str {
        self.info().display_name
    }

    pub fn is_selectable(self) -> bool {
        self.info().is_selectable
    }

    pub fn default_slot(self) -> Option<usize> {
        self.info().default_slot
    }

    /// As regras de host da linha deste provedor no registro.
    pub fn hosts(self) -> &'static [HostRule] {
        self.info().hosts
    }

    pub fn self_names(self) -> &'static [&'static str] {
        self.info().self_names
    }

    pub fn answer_selector(self) -> Option<&'static str> {
        self.info().answer_selector
    }

    pub fn is_unreadable(self) -> bool {
        self.answer_selector().is_none()
    }

    pub fn from_key(key: &str) -> Option<ProviderId> {
        PROVIDERS.iter().find(|p| p.key == key).map(|p| p.id)
    }

    /// Identifica o provedor de IA a partir de um URL, pelas regras de host
    /// do registro: só http(s), host exato ou subdomínio quando a regra os
    /// aceita, e o Google só com o primeiro `udm` igual a `50`. Nenhum host
    /// está em duas linhas (gate `registry_is_unique`), por isso a ordem das
    /// linhas não decide nada.
    pub fn from_url(url: &Url) -> Option<ProviderId> {
        PROVIDERS
            .iter()
            .find(|provider| provider.hosts.iter().any(|rule| rule.matches(url)))
            .map(|provider| provider.id)
    }

    /// Sugestões a priori do roteador inteligente para uma categoria de pergunta.
    pub fn router_prior_for_category(category: &str) -> &'static [ProviderId] {
        match category.to_ascii_lowercase().as_str() {
            "code" | "codigo" | "código" | "math" | "matematica" | "matemática" => {
                &[ProviderId::ChatGpt, ProviderId::Claude]
            }
            "news" | "atualidades" => &[ProviderId::GoogleAi],
            "sensitive" | "saude" | "saúde" | "direito" | "financas" | "finanças" => &[
                ProviderId::GoogleAi,
                ProviderId::ChatGpt,
                ProviderId::Claude,
            ],
            "writing" | "escrita" | "traducao" | "tradução" => {
                &[ProviderId::Claude, ProviderId::ChatGpt]
            }
            _ => &[],
        }
    }

    /// Monta o URL de busca/pergunta para este provedor.
    pub fn search_url(self, query: &str, language: &str) -> Result<Url> {
        match self {
            ProviderId::GoogleAi => google_ai_url(query, language),
            ProviderId::ChatGpt => chatgpt_search_url(query),
            ProviderId::Claude => claude_search_url(query),
            ProviderId::Perplexity => perplexity_search_url(query),
            ProviderId::Gemini => gemini_search_url(query),
            ProviderId::DeepSeek => deepseek_search_url(query),
            ProviderId::Copilot => copilot_search_url(query),
            ProviderId::Grok => grok_search_url(query),
            ProviderId::Mistral => mistral_search_url(query),
        }
    }
}

/// Todos os nomes próprios de provedores para uso pelo anonimizador do juiz,
/// juntados das linhas do registro (a ordem das linhas; um nome repetido
/// entra uma vez).
pub fn all_self_names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: Vec<&'static str> = Vec::new();
        for name in PROVIDERS.iter().flat_map(|provider| provider.self_names) {
            if !names.contains(name) {
                names.push(name);
            }
        }
        names
    })
}

/// Verifica se um URL pertence a um host oficial de IA provedora cadastrado.
pub fn is_ai_provider_host(url: &Url) -> bool {
    ProviderId::from_url(url).is_some()
}

/// Lista oficial de hosts de autenticação/login de provedores de IA.
pub fn login_hosts() -> &'static [&'static str] {
    LOGIN_HOSTS
}

/// Verifica se um hostname pertence aos hosts de autenticação autorizados.
pub fn is_login_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    LOGIN_HOSTS.iter().any(|&lh| {
        host == lh || (host.ends_with(lh) && host[..host.len() - lh.len()].ends_with('.'))
    })
}

pub fn google_ai_url(query: &str, language: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://www.google.com/search")
        .map_err(|_| NeuralError::InvalidUrl("Google search endpoint".into()))?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("udm", "50")
        .append_pair("hl", language);
    Ok(url)
}

pub fn chatgpt_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://chatgpt.com/")
        .map_err(|_| NeuralError::InvalidUrl("ChatGPT search endpoint".into()))?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("hints", "search");
    Ok(url)
}

pub fn claude_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://claude.ai/new")
        .map_err(|_| NeuralError::InvalidUrl("Claude search endpoint".into()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

pub fn perplexity_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://www.perplexity.ai/search")
        .map_err(|_| NeuralError::InvalidUrl("Perplexity search endpoint".into()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

pub fn gemini_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://gemini.google.com/app")
        .map_err(|_| NeuralError::InvalidUrl("Gemini search endpoint".into()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

pub fn deepseek_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://chat.deepseek.com/")
        .map_err(|_| NeuralError::InvalidUrl("DeepSeek search endpoint".into()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

pub fn copilot_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://copilot.microsoft.com/")
        .map_err(|_| NeuralError::InvalidUrl("Copilot search endpoint".into()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

pub fn grok_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://grok.com/")
        .map_err(|_| NeuralError::InvalidUrl("Grok search endpoint".into()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

pub fn mistral_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://chat.mistral.ai/")
        .map_err(|_| NeuralError::InvalidUrl("Mistral search endpoint".into()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect(raw)
    }

    /// O endereço de uma regra, com `udm=50` quando ela o pede.
    fn rule_url(scheme: &str, host: &str, rule: &HostRule) -> Url {
        let query = if rule.require_udm50 {
            "search?q=x&udm=50"
        } else {
            ""
        };
        url(&format!("{scheme}://{host}/{query}"))
    }

    #[test]
    fn ai_mode_url() {
        let u = google_ai_url("raft consensus", "pt-BR").unwrap();
        assert_eq!(u.host_str(), Some("www.google.com"));
        assert!(u.as_str().contains("udm=50"));
        assert_eq!(ProviderId::from_url(&u), Some(ProviderId::GoogleAi));
    }

    #[test]
    fn chatgpt_url() {
        let u = chatgpt_search_url("raft consensus").unwrap();
        assert_eq!(u.host_str(), Some("chatgpt.com"));
        assert!(u.as_str().contains("hints=search"));
    }

    #[test]
    fn claude_url() {
        let u = claude_search_url("raft consensus").unwrap();
        assert_eq!(u.host_str(), Some("claude.ai"));
        assert_eq!(u.path(), "/new");
        assert!(u.as_str().contains("q=raft+consensus"));
    }

    #[test]
    fn perplexity_url() {
        let u = perplexity_search_url("raft consensus").unwrap();
        assert_eq!(u.host_str(), Some("www.perplexity.ai"));
        assert_eq!(u.path(), "/search");
    }

    #[test]
    fn every_search_url_is_recognised_as_its_own_provider() {
        for &pid in ProviderId::all() {
            let u = pid.search_url("raft", "pt-BR").unwrap();
            assert_eq!(ProviderId::from_url(&u), Some(pid), "{u}");
        }
    }

    #[test]
    fn registry_is_unique() {
        assert_eq!(PROVIDERS.len(), ProviderId::all().len());
        let mut keys = HashSet::new();
        let mut seen = HashSet::new();
        for &pid in ProviderId::all() {
            assert!(keys.insert(pid.key()), "chave repetida: {}", pid.key());
            assert_eq!(ProviderId::from_key(pid.key()), Some(pid));
            assert!(!pid.hosts().is_empty(), "{pid:?} sem host");
            for rule in pid.hosts() {
                assert!(
                    seen.insert(rule.host),
                    "Host '{}' cadastrado em mais de um provedor!",
                    rule.host
                );
                assert_eq!(rule.host, rule.host.to_ascii_lowercase());
                assert!(!rule.host.starts_with('.') && !rule.host.ends_with('.'));
                // O host da própria regra é reconhecido como deste provedor.
                assert_eq!(
                    ProviderId::from_url(&rule_url("https", rule.host, rule)),
                    Some(pid),
                    "{}",
                    rule.host
                );
            }
        }
        // Nenhum host de uma linha cai na regra de subdomínios de outra: a
        // ordem das linhas em `from_url` não decide nada.
        for &a in ProviderId::all() {
            for &b in ProviderId::all() {
                if a == b {
                    continue;
                }
                for ra in a.hosts() {
                    for rb in b.hosts() {
                        assert!(
                            !rb.matches_host(ra.host),
                            "{} ({a:?}) cai na regra {} de {b:?}",
                            ra.host,
                            rb.host
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn only_the_default_slots_are_selectable_until_the_providers_item() {
        let defaults = ProviderId::default_slots();
        assert_eq!(ProviderId::selectable(), defaults.as_slice());
        for &pid in ProviderId::all() {
            assert_eq!(
                pid.is_selectable(),
                ProviderId::selectable().contains(&pid),
                "{pid:?}: a linha do registro e selectable() discordam"
            );
            let slot = defaults.iter().position(|&default| default == pid);
            assert_eq!(pid.default_slot(), slot, "{pid:?}");
        }
    }

    #[test]
    fn is_ai_provider_host_table() {
        let allowed = [
            ("https://chatgpt.com/", ProviderId::ChatGpt),
            ("http://chatgpt.com/", ProviderId::ChatGpt),
            ("https://CHATGPT.COM:443/c/1", ProviderId::ChatGpt),
            ("https://sub.chatgpt.com/c/123", ProviderId::ChatGpt),
            ("https://chat.openai.com/", ProviderId::ChatGpt),
            ("https://claude.ai/new", ProviderId::Claude),
            ("https://sub.claude.ai/", ProviderId::Claude),
            ("https://gemini.google.com/", ProviderId::Gemini),
            ("https://gemini.google.com/app", ProviderId::Gemini),
            (
                "https://www.google.com/search?q=foo&udm=50",
                ProviderId::GoogleAi,
            ),
            ("https://google.com/search?udm=50", ProviderId::GoogleAi),
            (
                "https://www.google.com/search?udm=50&udm=14",
                ProviderId::GoogleAi,
            ),
            ("https://perplexity.ai/", ProviderId::Perplexity),
            (
                "https://www.perplexity.ai/search?q=test",
                ProviderId::Perplexity,
            ),
            ("https://chat.deepseek.com/", ProviderId::DeepSeek),
            ("https://copilot.microsoft.com/", ProviderId::Copilot),
            ("https://grok.com/", ProviderId::Grok),
            ("https://chat.mistral.ai/", ProviderId::Mistral),
        ];

        for (raw, provider) in allowed {
            let u = url(raw);
            assert_eq!(ProviderId::from_url(&u), Some(provider), "{raw}");
            assert!(
                is_ai_provider_host(&u),
                "Deveria aceitar como host de IA: {raw}"
            );
        }

        let refused = [
            // Google só em google.com e www.google.com, e só com o PRIMEIRO
            // udm igual a 50 (o `searchParams.get` do script).
            "https://www.google.com/search?q=sem+udm",
            "https://google.com/",
            "https://google.com/search?udm=14",
            "https://www.google.com/search?q=x&udm=14&udm=50",
            "https://sites.google.com/view/evil?udm=50",
            "https://mail.google.com/?udm=50",
            "https://notgemini.google.com/",
            "https://notgemini.google.com/?udm=50",
            "https://sub.gemini.google.com/",
            "https://google.com.attacker.com?udm=50",
            // Hosts fora do brief.
            "https://sub.chat.openai.com/",
            "https://openai.com/",
            "https://deepseek.com/",
            "https://platform.deepseek.com/",
            "https://docs.perplexity.ai/",
            "https://x.ai/",
            "https://mistral.ai/",
            "https://docs.mistral.ai/",
            "https://microsoft.com/",
            // Lookalikes.
            "https://chatgpt.com.evil.io/",
            "https://chatgpt.com.evil.io/login",
            "https://evilchatgpt.com/",
            "https://evilclaude.ai/",
            "https://evilclaude.ai/new",
            "https://perplexity.ai.fake/",
            "https://deepseek.com.phish.org/",
            "https://copilot.microsoft.com.hack/",
            "https://grok.com.attacker.com/",
            "https://mistral.ai.scam.net/",
            "https://chatgpt.com@evil.io/",
            "https://evil.io/chatgpt.com/",
            "https://evil.io/?next=https://chatgpt.com/",
            // Só http(s).
            "javascript://claude.ai/%0Aalert(1)",
            "file://chatgpt.com/share/x",
            "ftp://chatgpt.com/",
            "ws://chatgpt.com/",
            "data:text/html,chatgpt.com",
            "https://example.com/",
            "http://localhost:8080/",
        ];

        for raw in refused {
            let u = url(raw);
            assert!(
                !is_ai_provider_host(&u),
                "Deveria RECUSAR como host de IA: {raw}"
            );
        }
    }

    /// Para cada regra do registro, gerado a partir das próprias linhas: o
    /// host aceito em http e https, os lookalikes recusados (o sufixo sem o
    /// ponto, o host como prefixo de outro domínio), só os subdomínios que a
    /// regra aceita, e o `udm=50` exigido quando a regra o pede.
    #[test]
    fn every_registry_host_refuses_its_lookalikes() {
        for &pid in ProviderId::all() {
            for rule in pid.hosts() {
                let host = rule.host;
                let query = if rule.require_udm50 { "?udm=50" } else { "" };
                for scheme in ["https", "http"] {
                    let own = url(&format!("{scheme}://{host}/{query}"));
                    assert_eq!(ProviderId::from_url(&own), Some(pid), "{own}");
                }
                for lookalike in [
                    format!("https://evil{host}/{query}"),
                    format!("https://{host}.evil.io/{query}"),
                    format!("https://{host}-evil.io/{query}"),
                    format!("https://evil.io/{host}/{query}"),
                    format!("ftp://{host}/{query}"),
                    format!("file://{host}/x{query}"),
                    format!("javascript://{host}/%0Aalert(1){query}"),
                ] {
                    assert!(
                        !is_ai_provider_host(&url(&lookalike)),
                        "lookalike de {host} aceito: {lookalike}"
                    );
                }
                let sub = url(&format!("https://sub.{host}/{query}"));
                let deep = url(&format!("https://a.b.{host}/{query}"));
                if rule.subdomains {
                    assert_eq!(ProviderId::from_url(&sub), Some(pid), "{sub}");
                    assert_eq!(ProviderId::from_url(&deep), Some(pid), "{deep}");
                } else {
                    assert!(!is_ai_provider_host(&sub), "subdomínio aceito: {sub}");
                    assert!(!is_ai_provider_host(&deep), "subdomínio aceito: {deep}");
                }
                if rule.require_udm50 {
                    for without in [
                        format!("https://{host}/"),
                        format!("https://{host}/search?q=x"),
                        format!("https://{host}/search?udm=14"),
                        format!("https://{host}/search?udm=14&udm=50"),
                        format!("https://{host}/search?udm=500"),
                    ] {
                        assert!(
                            !is_ai_provider_host(&url(&without)),
                            "Google sem o primeiro udm=50 aceito: {without}"
                        );
                    }
                }
            }
        }
    }

    /// Fixtures mínimas com a forma de uma conversa: a pergunta de quem
    /// escreve (PERGUNTA) e a resposta (RESPOSTA). São sintéticas (sem uma
    /// sessão com login não há captura do DOM real), por isso provam que o
    /// seletor é CSS válido e escolhe a resposta e nunca a pergunta, não que
    /// o site ao vivo não mudou de estrutura.
    const ANSWER_FIXTURES: &[(ProviderId, &str)] = &[
        (
            ProviderId::GoogleAi,
            r#"<html><body>
                <form role="search"><textarea name="q">PERGUNTA sobre Raft</textarea></form>
                <div id="rhs"><a href="/">Fontes</a></div>
                <main><div><p>RESPOSTA do Modo IA sobre Raft</p></div></main>
            </body></html>"#,
        ),
        (
            ProviderId::ChatGpt,
            r#"<html><body><main>
                <div data-message-author-role="user"><p>PERGUNTA sobre Raft</p></div>
                <div data-message-author-role="assistant"><div class="markdown"><p>RESPOSTA do ChatGPT</p></div></div>
                <form><textarea>PERGUNTA seguinte</textarea></form>
            </main></body></html>"#,
        ),
        (
            ProviderId::Claude,
            r#"<html><body><main>
                <div data-message-author-role="user"><p>PERGUNTA sobre Raft</p></div>
                <div data-message-author-role="assistant"><p>RESPOSTA do Claude</p></div>
                <fieldset><div contenteditable="true">PERGUNTA seguinte</div></fieldset>
            </main></body></html>"#,
        ),
    ];

    #[test]
    fn every_provider_has_answer_fixture_or_is_marked_unreadable() {
        for &pid in ProviderId::all() {
            let fixture = ANSWER_FIXTURES.iter().find(|(id, _)| *id == pid);
            match (pid.answer_selector(), fixture) {
                (Some(raw), Some((_, html))) => {
                    let selector = scraper::Selector::parse(raw)
                        .unwrap_or_else(|error| panic!("{pid:?}: seletor inválido: {error:?}"));
                    let document = scraper::Html::parse_document(html);
                    let text: String = document
                        .select(&selector)
                        .flat_map(|element| element.text())
                        .collect();
                    assert!(
                        text.contains("RESPOSTA"),
                        "{pid:?}: o seletor {raw:?} não chega à resposta da fixture"
                    );
                    assert!(
                        !text.contains("PERGUNTA"),
                        "{pid:?}: o seletor {raw:?} também lê a pergunta"
                    );
                    assert!(!pid.is_unreadable());
                }
                (None, None) => assert!(pid.is_unreadable()),
                (Some(_), None) => panic!("{pid:?} lê respostas sem fixture"),
                (None, Some(_)) => panic!("{pid:?} tem fixture mas está marcado como não lido"),
            }
        }
        let readable: Vec<ProviderId> = ProviderId::all()
            .iter()
            .copied()
            .filter(|pid| !pid.is_unreadable())
            .collect();
        assert_eq!(
            readable,
            [
                ProviderId::GoogleAi,
                ProviderId::ChatGpt,
                ProviderId::Claude
            ]
        );
    }

    #[test]
    fn login_hosts_table_and_detection() {
        let valid_logins = [
            "accounts.google.com",
            "sub.accounts.google.com",
            "login.live.com",
            "login.microsoftonline.com",
            "appleid.apple.com",
            "auth.openai.com",
            "auth0.openai.com",
        ];
        for host in valid_logins {
            assert!(is_login_host(host), "Deveria aceitar login host: {host}");
        }

        let invalid_logins = [
            "evilaccounts.google.com",
            "login.live.com.attacker.com",
            "fakeauth.openai.com",
            "google.com",
            "chatgpt.com",
            "example.com",
        ];
        for host in invalid_logins {
            assert!(
                !is_login_host(host),
                "Deveria RECUSAR login host inválido: {host}"
            );
        }
    }

    #[test]
    fn all_self_names_covers_judge_anonymiser() {
        let names = all_self_names();
        let expected = [
            "ChatGPT",
            "OpenAI",
            "GPT-n",
            "Claude",
            "Anthropic",
            "Gemini",
            "Bard",
            "Google IA",
            "AI Mode",
            "Modo IA",
            "Perplexity",
            "DeepSeek",
            "Copilot",
            "Microsoft",
            "Grok",
            "xAI",
            "Mistral",
            "Le Chat",
        ];
        for exp in expected {
            assert!(
                names.contains(&exp),
                "Nome '{exp}' ausente de all_self_names"
            );
        }
        // Exatamente os nomes das linhas do registro, cada um uma vez.
        let rows: HashSet<&str> = ProviderId::all()
            .iter()
            .flat_map(|pid| pid.self_names().iter().copied())
            .collect();
        let listed: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(listed, rows);
        assert_eq!(listed.len(), names.len(), "nome repetido em all_self_names");
    }

    #[test]
    fn router_priors_match_categories() {
        assert_eq!(
            ProviderId::router_prior_for_category("code"),
            &[ProviderId::ChatGpt, ProviderId::Claude]
        );
        assert_eq!(
            ProviderId::router_prior_for_category("codigo"),
            &[ProviderId::ChatGpt, ProviderId::Claude]
        );
        assert_eq!(
            ProviderId::router_prior_for_category("news"),
            &[ProviderId::GoogleAi]
        );
        assert_eq!(
            ProviderId::router_prior_for_category("atualidades"),
            &[ProviderId::GoogleAi]
        );
        assert_eq!(
            ProviderId::router_prior_for_category("sensitive"),
            &[
                ProviderId::GoogleAi,
                ProviderId::ChatGpt,
                ProviderId::Claude
            ]
        );
        assert_eq!(
            ProviderId::router_prior_for_category("geral"),
            &[] as &[ProviderId]
        );
    }
}
