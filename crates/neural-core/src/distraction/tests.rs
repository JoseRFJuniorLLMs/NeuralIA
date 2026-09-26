//! Gates da anti-distracao no nucleo (plano 2.4): as regras dos CMPs, as
//! listas, a politica por site e a decisao `distraction_config`. O script
//! que embarca corre-se nos gates do app (`node:vm`); aqui fica o que e
//! puro. Nenhum teste toca na rede nem no disco.

use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::search::{HostRule, ProviderId, login_hosts};

fn url(text: &str) -> Url {
    Url::parse(text).expect("url")
}

fn web(text: &str, policy: &DistractionPolicy) -> Option<String> {
    distraction_config(&url(text), policy, DistractionSurface::Web).map(|config| config.site)
}

/// Um endereco por regra do registo dos provedores: o host, um
/// subdominio quando a regra os aceita, e o `udm=50` quando ela o pede.
fn provider_urls() -> Vec<String> {
    let mut urls = Vec::new();
    let with_udm = |rule: &HostRule, host: &str| {
        if rule.require_udm50 {
            format!("https://{host}/search?q=x&udm=50")
        } else {
            format!("https://{host}/")
        }
    };
    for provider in ProviderId::all() {
        for rule in provider.hosts() {
            urls.push(with_udm(rule, rule.host));
            if rule.subdomains {
                urls.push(with_udm(rule, &format!("chat.{}", rule.host)));
            }
        }
    }
    for host in login_hosts() {
        urls.push(format!("https://{host}/signin"));
    }
    urls
}

/// Gate (critico, script injetado): nenhum seletor de recusa nomeia um
/// aceitar. Os pedacos de cada seletor (`#onetrust-reject-all-handler` da
/// `onetrust`, `reject`, `all`, `handler`; o camelCase parte) nao comecam
/// por nenhuma palavra de `ACCEPT_WORDS` (`disagree` nao comeca por
/// `agree`). As molduras de outra origem nunca tem recusa, os ids sao
/// unicos e ha uma regra por CMP do plano.
///
/// Sabotagem: a recusa do OneTrust a apontar para
/// `#onetrust-accept-btn-handler` -> vermelho.
#[test]
fn cmp_rules_reject_selectors_never_name_accept() {
    let mut ids = Vec::new();
    for rule in CMP_RULES {
        assert!(!ids.contains(&rule.id), "{} repetido", rule.id);
        ids.push(rule.id);
        assert!(!rule.detect.trim().is_empty(), "{}: sem deteccao", rule.id);
        if rule.frame_only {
            assert_eq!(rule.reject, None, "{}: moldura com clique", rule.id);
            assert!(
                rule.banner.is_some(),
                "{}: moldura sem nada a esconder",
                rule.id
            );
        }
        if let Some(reject) = rule.reject {
            let tokens = selector_tokens(reject);
            assert!(!tokens.is_empty(), "{}", rule.id);
            assert_eq!(
                names_accept(&tokens),
                None,
                "{}: o seletor de recusa {reject:?} nomeia um aceitar ({tokens:?})",
                rule.id
            );
        }
    }
    for expected in [
        "onetrust",
        "cookiebot",
        "didomi",
        "usercentrics",
        "quantcast",
        "cookieyes",
        "complianz",
        "iubenda",
        "osano",
        "axeptio",
        "termly",
        "google-consent",
        "sourcepoint",
        "trustarc",
    ] {
        assert!(ids.contains(&expected), "falta a regra {expected}");
    }
    // O detetor apanha o que um seletor de aceitar teria: o que o gate
    // guarda nao passa com qualquer lista.
    for accept in [
        "#onetrust-accept-btn-handler",
        "#CybotCookiebotDialogBodyLevelButtonLevelOptinAllowAll",
        "#didomi-notice-agree-button",
        r#"[data-testid="uc-accept-all-button"]"#,
        ".cky-btn-accept",
        "#axeptio_btn_acceptAll",
        r#"[data-tid="banner-accept"]"#,
    ] {
        assert!(
            names_accept(&selector_tokens(accept)).is_some(),
            "{accept} passava pelo gate"
        );
    }
}

/// Gate (critico, script injetado; SPEC-0114 «as definicoes de cookies do
/// rodape ficam»): nenhum `banner` de `CMP_RULES` nomeia um anfitriao que o
/// CMP deixa na pagina e reusa para as definicoes que o utilizador abre
/// depois (`PERSISTENT_CMP_HOSTS`). Escondido no carregamento, ele deixava
/// essas definicoes invisiveis para sempre.
///
/// Sabotagem: o `#didomi-host` de volta ao `banner` do Didomi -> vermelho.
#[test]
fn cmp_banners_never_name_a_persistent_host() {
    let mut named = 0;
    for rule in CMP_RULES {
        let detect_and_reject = format!("{} {}", rule.detect, rule.reject.unwrap_or_default());
        named += PERSISTENT_CMP_HOSTS
            .iter()
            .filter(|host| detect_and_reject.contains(*host))
            .count();
        let Some(banner) = rule.banner else {
            continue;
        };
        for part in banner.split(',').map(str::trim) {
            assert!(!part.is_empty(), "{}: seletor vazio", rule.id);
            for host in PERSISTENT_CMP_HOSTS {
                assert!(
                    !part.contains(host),
                    "{}: o banner {part:?} esconde o anfitriao persistente {host}",
                    rule.id
                );
            }
        }
        // A recusa tambem nunca vive so dentro do centro de preferencias.
        if let Some(reject) = rule.reject {
            assert!(
                !reject.contains("#onetrust-pc-sdk"),
                "{}: {reject}",
                rule.id
            );
        }
    }
    // O controlo: os anfitrioes da lista sao os destas regras.
    assert!(named >= 3, "{named}");
    let usercentrics = CMP_RULES
        .iter()
        .find(|rule| rule.id == "usercentrics")
        .expect("usercentrics");
    assert_eq!(usercentrics.banner, None);
    assert!(usercentrics.reject.is_some());
}

/// Gate: as listas estao na forma em que o script compara (normalizadas),
/// nenhuma frase de recusa traz uma palavra de aceitar (nunca seria
/// clicada) e cada lingua do plano tem recusa e aceitar.
#[test]
fn word_lists_are_normalized() {
    for list in [
        REJECT_WORDS,
        ACCEPT_WORDS,
        PAYWALL_MARKERS,
        NEWSLETTER_MARKERS,
    ] {
        for word in list {
            assert_eq!(
                &normalize_label(word),
                word,
                "{word:?} nao esta normalizada"
            );
            assert!(!word.is_empty());
        }
    }
    for phrase in REJECT_WORDS {
        let tokens: Vec<String> = phrase.split(' ').map(str::to_string).collect();
        assert_eq!(names_accept(&tokens), None, "{phrase:?} tem um aceitar");
    }
    for reject in [
        "rejeitar", "reject", "rechazar", "refuser", "ablehnen", "rifiuta",
    ] {
        assert!(REJECT_WORDS.contains(&reject), "{reject}");
    }
    for accept in ["aceit", "accept", "acept", "accord", "akzept", "accett"] {
        assert!(ACCEPT_WORDS.contains(&accept), "{accept}");
    }
    assert_eq!(normalize_label("  Só   Necessários! "), "só necessários");
    assert_eq!(normalize_label("D'ACCORD"), "d accord");
    assert_eq!(normalize_label("Reject\u{200b}all"), "reject all");
}

/// Gate (critico): nunca nas paginas das IAs nem dos logins. Cada regra do
/// registo dos provedores (e cada login) da `None`; os parecidos
/// (`chatgpt.com.evil.io`, `evilclaude.ai`, o Google sem `udm=50` ou com
/// o primeiro `udm` diferente) nao sao IAs e recebem o script.
///
/// Sabotagem: tirar o `page_is_exempt` de `distraction_site` -> vermelho.
#[test]
fn distraction_leaves_ai_provider_pages_untouched() {
    let policy = DistractionPolicy::default();
    let urls = provider_urls();
    assert!(urls.len() >= 15, "{urls:?}");
    for page in &urls {
        assert_eq!(web(page, &policy), None, "{page}");
    }
    assert_eq!(web("https://chatgpt.com./c/1", &policy), None);
    for (page, site) in [
        ("https://chatgpt.com.evil.io/", "chatgpt.com.evil.io"),
        ("https://evilclaude.ai/", "evilclaude.ai"),
        ("https://www.google.com/search?q=x", "google.com"),
        ("https://www.google.com/search?udm=14&udm=50", "google.com"),
        (
            "https://consent.google.com/ml?continue=x",
            "consent.google.com",
        ),
        ("https://news.example.com/a", "news.example.com"),
    ] {
        assert_eq!(web(page, &policy).as_deref(), Some(site), "{page}");
    }
}

/// Gate (critico): as nossas origens e as superficies que nao sao a web
/// nunca recebem o script.
#[test]
fn our_origins_and_local_surfaces_get_no_script() {
    let policy = DistractionPolicy::default();
    for page in [
        "http://neuralia-pdf.localhost/viewer.html",
        "http://neuralia-epub.localhost/library.html",
        "http://localhost:8080/",
        "http://127.0.0.1/",
        "http://[::1]/",
        "http://192.168.0.10/admin",
        "http://intranet/",
        "neuralia://home",
        "about:blank",
        "file:///C:/x.html",
        "data:text/html,<p>x</p>",
    ] {
        assert_eq!(web(page, &policy), None, "{page}");
    }
    let news = url("https://news.example.com/");
    for surface in [
        DistractionSurface::Home,
        DistractionSurface::Reader,
        DistractionSurface::Pdf,
        DistractionSurface::Books,
        DistractionSurface::ServicePanel,
        DistractionSurface::AppPanel,
    ] {
        assert_eq!(
            distraction_config(&news, &policy, surface),
            None,
            "{surface:?}"
        );
    }
    assert!(distraction_config(&news, &policy, DistractionSurface::Web).is_some());
}

/// Gate (critico, parte pura): `sites[host] = false` da `None` nesse site
/// (com e sem `www.`), e so nesse; ligar de novo tira a escolha propria;
/// com o padrao desligado so os sites ligados recebem o script.
///
/// Sabotagem: `site_on` a ignorar `sites` -> vermelho.
#[test]
fn per_site_toggle_off_injects_nothing() {
    let mut policy = DistractionPolicy::default();
    assert_eq!(
        policy.set_site("www.Example.com", false),
        SiteChange::Changed
    );
    assert_eq!(
        policy.sites,
        BTreeMap::from([("example.com".to_string(), false)])
    );
    assert_eq!(policy.set_site("example.com", false), SiteChange::Unchanged);
    assert_eq!(web("https://example.com/", &policy), None);
    assert_eq!(web("https://www.example.com/x", &policy), None);
    assert_eq!(
        web("https://news.example.com/", &policy).as_deref(),
        Some("news.example.com"),
        "um subdominio e outro site"
    );
    assert_eq!(policy.set_site("example.com", true), SiteChange::Changed);
    assert!(
        policy.sites.is_empty(),
        "igual ao padrao: sem escolha propria"
    );
    assert_eq!(
        web("https://www.example.com/", &policy).as_deref(),
        Some("example.com")
    );

    for refused in [
        "127.0.0.1",
        "localhost",
        "neuralia-pdf.localhost",
        "not a host",
        "",
    ] {
        assert_eq!(
            policy.set_site(refused, false),
            SiteChange::Refused,
            "{refused:?}"
        );
    }

    let mut off = DistractionPolicy {
        default_on: false,
        sites: BTreeMap::new(),
    };
    assert_eq!(web("https://example.com/", &off), None);
    assert_eq!(off.set_site("example.com", true), SiteChange::Changed);
    assert_eq!(
        web("https://example.com/", &off).as_deref(),
        Some("example.com")
    );
    assert_eq!(web("https://other.example.org/", &off), None);

    // O tecto: a lista cheia recusa um site novo e aceita tirar um.
    let mut full = DistractionPolicy::default();
    for index in 0..MAX_DISTRACTION_SITES {
        assert_eq!(
            full.set_site(&format!("s{index}.example.com"), false),
            SiteChange::Changed
        );
    }
    assert_eq!(
        full.set_site("one-more.example.com", false),
        SiteChange::Refused
    );
    assert_eq!(full.set_site("s1.example.com", true), SiteChange::Changed);
}

/// O Split privado: as escolhas feitas la ficam por cima da politica
/// gravada, e so em memoria.
#[test]
fn the_private_overlay_wins_over_the_saved_policy() {
    let mut saved = DistractionPolicy::default();
    assert_eq!(
        saved.set_site("keep.example.com", false),
        SiteChange::Changed
    );
    let overlay = BTreeMap::from([
        ("example.com".to_string(), false),
        ("keep.example.com".to_string(), true),
    ]);
    let private = saved.with_overlay(&overlay);
    assert_eq!(web("https://example.com/", &private), None);
    assert!(web("https://keep.example.com/", &private).is_some());
    assert!(
        web("https://example.com/", &saved).is_some(),
        "o gravado nao muda"
    );
    assert_eq!(web("https://keep.example.com/", &saved), None);
}

/// O campo `distraction` tem a forma que a 2.3 reservou (um mapa
/// `site -> bool`): um ficheiro da 2.3 le-se como tudo ligado; o padrao
/// desligado grava-se como `"*": false`; entradas estragadas saem.
#[test]
fn the_policy_keeps_the_reserved_map_shape() {
    let default = DistractionPolicy::default();
    assert_eq!(serde_json::to_value(&default).expect("json"), json!({}));
    let from_23: DistractionPolicy = serde_json::from_value(json!({})).expect("2.3");
    assert_eq!(from_23, default);

    let mut policy = DistractionPolicy {
        default_on: false,
        sites: BTreeMap::new(),
    };
    policy.set_site("example.com", true);
    let value = serde_json::to_value(&policy).expect("json");
    assert_eq!(value, json!({ "*": false, "example.com": true }));
    let back: DistractionPolicy = serde_json::from_value(value.clone()).expect("back");
    assert_eq!(back, policy);
    // A 2.3 le o mesmo campo como o mapa que reservou.
    let as_23: BTreeMap<String, bool> = serde_json::from_value(value).expect("mapa");
    assert_eq!(as_23.len(), 2);

    let dirty: DistractionPolicy = serde_json::from_value(json!({
        "ok.example.com": false,
        "Bad Site": false,
        "localhost": false,
        "www.example.com": false,
        "same.example.com": true
    }))
    .expect("dirty");
    let clean = dirty.sanitized();
    assert!(clean.default_on);
    assert_eq!(
        clean.sites,
        BTreeMap::from([("ok.example.com".to_string(), false)])
    );
}

/// O que o script recebe: cada regra do registo dos provedores e cada
/// login (a decisao que ele repete no inicio do documento), as regras dos
/// CMPs e as listas.
#[test]
fn the_script_config_carries_the_registry_and_the_lists() {
    let mut policy = DistractionPolicy::default();
    policy.set_site("example.com", false);
    let config = script_config(&policy);
    assert_eq!(config["on"], true);
    assert_eq!(config["sites"], json!({ "example.com": false }));
    let never = config["never"].as_array().expect("never");
    for provider in ProviderId::all() {
        for rule in provider.hosts() {
            assert!(
                never.contains(&json!([rule.host, rule.subdomains, rule.require_udm50])),
                "{rule:?}"
            );
        }
    }
    assert_eq!(config["login"], json!(login_hosts()));
    assert_eq!(
        config["cmp"].as_array().expect("cmp").len(),
        CMP_RULES.len()
    );
    assert_eq!(config["reject"], json!(REJECT_WORDS));
    assert_eq!(config["accept"], json!(ACCEPT_WORDS));
    assert_eq!(config["stickyMin"], json!(STICKY_MIN_RATIO));
    assert_eq!(config["budgetMs"], json!(FRAME_BUDGET_MS));
    assert_eq!(config["cmpMs"], json!(CMP_PASS_EVERY_MS));
    assert_eq!(config["frameWall"], json!(FRAME_WALL_RATIO));
    assert_eq!(config["idleMs"], json!(IDLE_STOP_MS));
    assert_eq!(config["userMs"], json!(USER_OPENED_MS));
    let selector = config["paywallSel"].as_str().expect("paywallSel");
    // Sem olhar a maiusculas: `PaywallModal` tambem e um paywall.
    assert!(selector.contains(r#"[class*="tp-modal" i]"#), "{selector}");
    assert!(selector.contains(r#"[id*="paywall" i]"#), "{selector}");
    assert!(!selector.contains(r#"paywall"]"#), "{selector}");
    assert!(!NEWSLETTER_MARKERS.contains(&"boletim"));
}
