use crate::{NeuralError, Result};
use serde::{Deserialize, Serialize};
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

/// Metadados estáticos de um provedor de IA.
#[derive(Debug, Clone, Copy)]
pub struct ProviderInfo {
    pub id: ProviderId,
    pub key: &'static str,
    pub display_name: &'static str,
    pub is_selectable: bool,
    pub default_slot: Option<usize>,
    pub hosts: &'static [&'static str],
    pub self_names: &'static [&'static str],
    pub answer_selector: Option<&'static str>,
}

static PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: ProviderId::GoogleAi,
        key: "google",
        display_name: "Google IA",
        is_selectable: true,
        default_slot: Some(0),
        hosts: &["google.com", "www.google.com"],
        self_names: &["Google IA", "AI Mode", "Modo IA"],
        answer_selector: Some(r#"main, [data-message-author-role="assistant"], [role="article"]"#),
    },
    ProviderInfo {
        id: ProviderId::ChatGpt,
        key: "chatgpt",
        display_name: "ChatGPT",
        is_selectable: true,
        default_slot: Some(1),
        hosts: &["chatgpt.com", "chat.openai.com"],
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
        hosts: &["claude.ai"],
        self_names: &["Claude", "Anthropic"],
        answer_selector: Some(r#"[data-message-author-role="assistant"]"#),
    },
    ProviderInfo {
        id: ProviderId::Perplexity,
        key: "perplexity",
        display_name: "Perplexity",
        is_selectable: true,
        default_slot: None,
        hosts: &["perplexity.ai", "www.perplexity.ai"],
        self_names: &["Perplexity"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Gemini,
        key: "gemini",
        display_name: "Gemini",
        is_selectable: true,
        default_slot: None,
        hosts: &["gemini.google.com"],
        self_names: &["Gemini", "Bard"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::DeepSeek,
        key: "deepseek",
        display_name: "DeepSeek",
        is_selectable: true,
        default_slot: None,
        hosts: &["chat.deepseek.com", "deepseek.com"],
        self_names: &["DeepSeek"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Copilot,
        key: "copilot",
        display_name: "Copilot",
        is_selectable: true,
        default_slot: None,
        hosts: &["copilot.microsoft.com"],
        self_names: &["Copilot", "Microsoft Copilot", "Microsoft"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Grok,
        key: "grok",
        display_name: "Grok",
        is_selectable: true,
        default_slot: None,
        hosts: &["grok.com", "x.ai"],
        self_names: &["Grok", "xAI"],
        answer_selector: None,
    },
    ProviderInfo {
        id: ProviderId::Mistral,
        key: "mistral",
        display_name: "Mistral",
        is_selectable: false,
        default_slot: None,
        hosts: &["chat.mistral.ai", "mistral.ai"],
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
    /// Todos os provedores cadastrados no registro (8 selecionáveis + Mistral).
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

    /// Os 8 provedores selecionáveis do design v1.
    pub const fn selectable() -> &'static [ProviderId] {
        &[
            ProviderId::GoogleAi,
            ProviderId::ChatGpt,
            ProviderId::Claude,
            ProviderId::Perplexity,
            ProviderId::Gemini,
            ProviderId::DeepSeek,
            ProviderId::Copilot,
            ProviderId::Grok,
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

    pub fn hosts(self) -> &'static [&'static str] {
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

    /// Identifica o provedor de IA a partir de um URL.
    pub fn from_url(url: &Url) -> Option<ProviderId> {
        let host = url.host_str()?.to_ascii_lowercase();

        if host == "gemini.google.com" || host.ends_with(".gemini.google.com") {
            return Some(ProviderId::Gemini);
        }

        // Google: requer parâmetro udm=50 (Modo IA)
        if (host == "google.com" || host.ends_with(".google.com"))
            && url.query_pairs().any(|(k, v)| k == "udm" && v == "50")
        {
            return Some(ProviderId::GoogleAi);
        }
        if host == "chatgpt.com"
            || host.ends_with(".chatgpt.com")
            || host == "chat.openai.com"
            || host.ends_with(".chat.openai.com")
        {
            return Some(ProviderId::ChatGpt);
        }
        if host == "claude.ai" || host.ends_with(".claude.ai") {
            return Some(ProviderId::Claude);
        }
        if host == "perplexity.ai" || host.ends_with(".perplexity.ai") {
            return Some(ProviderId::Perplexity);
        }
        if host == "deepseek.com" || host.ends_with(".deepseek.com") {
            return Some(ProviderId::DeepSeek);
        }
        if host == "copilot.microsoft.com" || host.ends_with(".copilot.microsoft.com") {
            return Some(ProviderId::Copilot);
        }
        if host == "grok.com"
            || host.ends_with(".grok.com")
            || host == "x.ai"
            || host.ends_with(".x.ai")
        {
            return Some(ProviderId::Grok);
        }
        if host == "mistral.ai" || host.ends_with(".mistral.ai") {
            return Some(ProviderId::Mistral);
        }

        None
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

/// Todos os nomes próprios de provedores para uso pelo anonimizador do juiz.
pub fn all_self_names() -> &'static [&'static str] {
    &[
        "ChatGPT",
        "OpenAI",
        "GPT-4",
        "GPT-3",
        "GPT-5",
        "GPT-o1",
        "GPT-4o",
        "GPT-n",
        "GPT",
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
        "Microsoft Copilot",
        "Microsoft",
        "Grok",
        "xAI",
        "Mistral",
        "Le Chat",
    ]
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

    #[test]
    fn ai_mode_url() {
        let u = google_ai_url("raft consensus", "pt-BR").unwrap();
        assert_eq!(u.host_str(), Some("www.google.com"));
        assert!(u.as_str().contains("udm=50"));
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
    fn registry_is_unique() {
        let mut seen = HashSet::new();
        for &pid in ProviderId::all() {
            let info = pid.info();
            for &host in info.hosts {
                assert!(
                    seen.insert(host),
                    "Host '{host}' cadastrado em mais de um provedor!"
                );
            }
        }
    }

    #[test]
    fn is_ai_provider_host_table() {
        let allowed = [
            "https://chatgpt.com/",
            "https://sub.chatgpt.com/c/123",
            "https://chat.openai.com/",
            "https://claude.ai/new",
            "https://sub.claude.ai/",
            "https://gemini.google.com/",
            "https://gemini.google.com/app",
            "https://www.google.com/search?q=foo&udm=50",
            "https://google.com/search?udm=50",
            "https://perplexity.ai/",
            "https://www.perplexity.ai/search?q=test",
            "https://chat.deepseek.com/",
            "https://deepseek.com/",
            "https://copilot.microsoft.com/",
            "https://grok.com/",
            "https://x.ai/",
            "https://chat.mistral.ai/",
            "https://mistral.ai/",
        ];

        for raw in allowed {
            let u = Url::parse(raw).expect(raw);
            assert!(
                is_ai_provider_host(&u),
                "Deveria aceitar como host de IA: {raw}"
            );
        }

        let refused = [
            "https://www.google.com/search?q=sem+udm",
            "https://google.com/",
            "https://google.com/search?udm=14",
            "https://chatgpt.com.evil.io/",
            "https://chatgpt.com.evil.io/login",
            "https://evilclaude.ai/",
            "https://evilclaude.ai/new",
            "https://notgemini.google.com/",
            "https://google.com.attacker.com?udm=50",
            "https://perplexity.ai.fake/",
            "https://deepseek.com.phish.org/",
            "https://copilot.microsoft.com.hack/",
            "https://grok.com.attacker.com/",
            "https://mistral.ai.scam.net/",
            "https://example.com/",
            "http://localhost:8080/",
        ];

        for raw in refused {
            let u = Url::parse(raw).expect(raw);
            assert!(
                !is_ai_provider_host(&u),
                "Deveria RECUSAR como host de IA: {raw}"
            );
        }
    }

    #[test]
    fn script_hosts_subset_of_registry() {
        // Os hosts conferidos por onProviderPage() em COMPARATOR_INJECT_SCRIPT:
        let script_urls = [
            "https://chatgpt.com/",
            "https://chat.openai.com/",
            "https://claude.ai/",
            "https://gemini.google.com/",
            "https://google.com/search?udm=50",
            "https://www.google.com/search?udm=50",
        ];

        for raw in script_urls {
            let u = Url::parse(raw).expect(raw);
            assert!(
                is_ai_provider_host(&u),
                "Host do script não reconhecido pelo registro: {raw}"
            );
        }
    }

    #[test]
    fn every_provider_has_answer_fixture_or_is_marked_unreadable() {
        for &pid in ProviderId::all() {
            match pid {
                ProviderId::GoogleAi | ProviderId::ChatGpt | ProviderId::Claude => {
                    assert!(
                        pid.answer_selector().is_some(),
                        "{:?} deve ter selector de resposta",
                        pid
                    );
                    assert!(!pid.is_unreadable());
                }
                ProviderId::Perplexity
                | ProviderId::Gemini
                | ProviderId::DeepSeek
                | ProviderId::Copilot
                | ProviderId::Grok
                | ProviderId::Mistral => {
                    assert!(
                        pid.answer_selector().is_none(),
                        "{:?} deve ser unreadable (não lida)",
                        pid
                    );
                    assert!(pid.is_unreadable());
                }
            }
        }
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
