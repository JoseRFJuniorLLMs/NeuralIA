//! Dominios (adblock, plano 2.3; critica C4): o nome de dominio normalizado,
//! o casamento por sufixo de rotulos e a chave canonica de um endereco.
//!
//! Um modulo so para tres consumidores: o bloqueio de anuncios (a lista de
//! dominios e a decisao por pedido), os favoritos (a chave que deteta um
//! favorito repetido) e as fontes do Consenso (que acrescentam aqui o
//! dominio registavel). Nenhum deles tem uma segunda copia.
//!
//! - `normalize_domain`: minusculas, IDN em punycode (pelo `url`, o mesmo
//!   parser do WebView), sem o ponto final; um endereco IP, uma porta, um
//!   caminho ou um curinga nao sao um dominio.
//! - `label_suffix_match`: o host inteiro, depois cada sufixo que comeca
//!   num rotulo, do mais comprido ao mais curto. Uma consulta por rotulo:
//!   O(rotulos do host), nunca O(tamanho da lista). `notexample.com` nao e
//!   um sufixo de rotulos de `example.com` -- o ponto faz parte da regra.
//! - `canonical_url_key`: o endereco sem rastreio (`utm_*`, `fbclid`,
//!   `gclid`, `dclid`, `msclkid`, `srsltid`, `mc_cid`, `mc_eid`), com o
//!   `google.*/url?q=` desembrulhado, a query ordenada, sem `www.` e sem a
//!   barra final.

use url::{Host, Url};

/// Os parametros de rastreio que a chave canonica tira, alem de `utm_*`.
pub const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "dclid", "msclkid", "srsltid", "mc_cid", "mc_eid",
];

/// O maior nome de dominio (RFC 1035) e o maior rotulo.
const MAX_DOMAIN_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

/// Quantos `google.*/url?q=` encaixados se desembrulham.
const MAX_GOOGLE_UNWRAP: usize = 3;

/// Um nome de dominio como o WebView o ve: minusculas, IDN em punycode, sem
/// o ponto final. `None` para um IP, um nome com porta, caminho, curinga,
/// utilizador, espacos, rotulos vazios ou compridos demais.
pub fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if trimmed.is_empty() || trimmed.len() > MAX_DOMAIN_LEN * 4 {
        return None;
    }
    if trimmed.chars().any(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                '/' | '\\' | ':' | '@' | '?' | '#' | '*' | '%' | '[' | ']' | '^' | '|' | '$'
            )
    }) {
        return None;
    }
    let url = Url::parse(&format!("http://{trimmed}/")).ok()?;
    let Host::Domain(domain) = url.host()? else {
        return None;
    };
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    let valid = !domain.is_empty()
        && domain.len() <= MAX_DOMAIN_LEN
        && domain
            .split('.')
            .all(|label| !label.is_empty() && label.len() <= MAX_LABEL_LEN);
    valid.then(|| domain.to_string())
}

/// `host` e `suffix`, ou um subdominio dele: `a.example.com` e
/// `example.com` sim, `notexample.com` nao.
pub fn is_label_suffix(host: &str, suffix: &str) -> bool {
    if suffix.is_empty() {
        return false;
    }
    host == suffix
        || host
            .strip_suffix(suffix)
            .is_some_and(|head| head.len() > 1 && head.ends_with('.'))
}

/// O primeiro sufixo de rotulos de `host` que `listed` aceita, do host
/// inteiro para o ultimo rotulo. `listed` e chamado uma vez por rotulo e
/// nunca com um pedaco que nao comece num rotulo: `ads.example.com` pergunta
/// por `ads.example.com`, `example.com` e `com` -- e so.
pub fn label_suffix_match(host: &str, mut listed: impl FnMut(&str) -> bool) -> Option<&str> {
    let mut rest = host;
    while !rest.is_empty() {
        if listed(rest) {
            return Some(rest);
        }
        let dot = rest.find('.')?;
        rest = &rest[dot + 1..];
    }
    None
}

/// O host sem o `www.` da frente (so quando sobra um nome com ponto:
/// `www.com` fica como esta).
pub fn without_www(host: &str) -> &str {
    match host.strip_prefix("www.") {
        Some(rest) if rest.contains('.') => rest,
        _ => host,
    }
}

fn is_tracking_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("utm_") || TRACKING_PARAMS.contains(&lower.as_str())
}

/// `google.<tld>` ou `www.google.<tld>` (com um ou dois rotulos de TLD:
/// `google.com`, `google.com.br`, `google.co.uk`).
fn is_google_host(host: &str) -> bool {
    let host = without_www(host);
    let Some(tld) = host.strip_prefix("google.") else {
        return false;
    };
    let labels = tld.split('.').count();
    (1..=2).contains(&labels) && tld.split('.').all(|label| !label.is_empty())
}

/// O destino de um `https://www.google.com/url?q=<destino>` (ou `url=`), se
/// for http(s).
fn google_redirect_target(url: &Url) -> Option<Url> {
    if !matches!(url.scheme(), "http" | "https") || url.path() != "/url" {
        return None;
    }
    if !url.host_str().is_some_and(is_google_host) {
        return None;
    }
    url.query_pairs()
        .find(|(name, _)| name == "q" || name == "url")
        .and_then(|(_, value)| Url::parse(&value).ok())
        .filter(|target| matches!(target.scheme(), "http" | "https"))
}

/// A chave que junta dois enderecos da mesma pagina: sem os parametros de
/// rastreio, com o redirecionamento do Google desembrulhado, a query
/// ordenada, sem `www.`, sem a barra final, sem utilizador nem senha. O
/// esquema, a porta e o fragmento ficam (um `#/rota` pode ser outra pagina).
pub fn canonical_url_key(url: &Url) -> String {
    let mut current = url.clone();
    for _ in 0..MAX_GOOGLE_UNWRAP {
        match google_redirect_target(&current) {
            Some(target) => current = target,
            None => break,
        }
    }
    if current.cannot_be_a_base() {
        return current.as_str().to_string();
    }
    let mut key = String::with_capacity(current.as_str().len());
    key.push_str(current.scheme());
    key.push_str("://");
    if let Some(host) = current.host_str() {
        key.push_str(without_www(host));
    }
    if let Some(port) = current.port() {
        key.push(':');
        key.push_str(&port.to_string());
    }
    key.push_str(current.path().trim_end_matches('/'));
    let mut pairs: Vec<(String, String)> = current
        .query_pairs()
        .filter(|(name, _)| !is_tracking_param(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if !pairs.is_empty() {
        pairs.sort();
        key.push('?');
        key.push_str(
            &url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish(),
        );
    }
    if let Some(fragment) = current.fragment().filter(|fragment| !fragment.is_empty()) {
        key.push('#');
        key.push_str(fragment);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(input: &str) -> String {
        canonical_url_key(&Url::parse(input).expect("test URL"))
    }

    #[test]
    fn domains_are_normalized_like_the_webview_sees_them() {
        assert_eq!(
            normalize_domain("Ads.Example.COM"),
            Some("ads.example.com".into())
        );
        assert_eq!(normalize_domain("example.com."), Some("example.com".into()));
        assert_eq!(
            normalize_domain("  tracker.net\t"),
            Some("tracker.net".into())
        );
        // IDN em punycode, como o WebView pede o host.
        assert_eq!(
            normalize_domain("anúncios.exemplo.br"),
            Some("xn--anncios-71a.exemplo.br".into())
        );
        for refused in [
            "",
            ".",
            "1.2.3.4",
            "[::1]",
            "example.com:8080",
            "example.com/ads",
            "*.example.com",
            "ads.*.com",
            "user@example.com",
            "a..b",
            "ex ample.com",
            "||example.com^",
            "example.com$third-party",
        ] {
            assert_eq!(normalize_domain(refused), None, "{refused:?}");
        }
        let long_label = format!("{}.com", "a".repeat(64));
        assert_eq!(normalize_domain(&long_label), None);
    }

    /// Gate (critico): o casamento e por sufixo de ROTULOS. `notexample.com`
    /// termina com o texto `example.com` mas nao e um subdominio dele.
    #[test]
    fn label_suffix_match_rejects_a_bare_text_suffix() {
        let listed = ["example.com", "doubleclick.net"];
        fn find<'a>(listed: &[&str], host: &'a str) -> Option<&'a str> {
            label_suffix_match(host, |suffix| listed.contains(&suffix))
        }
        let find = |host: &'static str| find(&listed, host);
        assert_eq!(find("example.com"), Some("example.com"));
        assert_eq!(find("ads.example.com"), Some("example.com"));
        assert_eq!(find("a.b.c.example.com"), Some("example.com"));
        assert_eq!(find("notexample.com"), None);
        assert_eq!(find("example.com.evil.io"), None);
        assert_eq!(find("xdoubleclick.net"), None);
        assert_eq!(find("com"), None);
        assert_eq!(find(""), None);
        assert!(is_label_suffix("ads.example.com", "example.com"));
        assert!(is_label_suffix("example.com", "example.com"));
        assert!(!is_label_suffix("notexample.com", "example.com"));
        assert!(!is_label_suffix(".example.com", "example.com"));
        assert!(!is_label_suffix("example.com", ""));
    }

    #[test]
    fn label_suffix_match_asks_once_per_label_and_only_at_label_starts() {
        let mut asked = Vec::new();
        let found = label_suffix_match("a.b.example.com", |suffix| {
            asked.push(suffix.to_string());
            false
        });
        assert_eq!(found, None);
        assert_eq!(
            asked,
            ["a.b.example.com", "b.example.com", "example.com", "com"]
        );
    }

    #[test]
    fn www_goes_only_when_a_name_is_left() {
        assert_eq!(without_www("www.example.com"), "example.com");
        assert_eq!(without_www("example.com"), "example.com");
        assert_eq!(without_www("www.com"), "www.com");
        assert_eq!(without_www("wwwexample.com"), "wwwexample.com");
    }

    #[test]
    fn canonical_key_drops_tracking_sorts_and_trims() {
        assert_eq!(
            key("https://www.example.com/a/b/?utm_source=x&b=2&fbclid=1&a=1&UTM_Medium=y"),
            "https://example.com/a/b?a=1&b=2"
        );
        for param in TRACKING_PARAMS {
            assert_eq!(
                key(&format!("https://example.com/p?{param}=1&k=v")),
                "https://example.com/p?k=v",
                "{param}"
            );
        }
        assert_eq!(key("https://example.com/"), "https://example.com");
        assert_eq!(key("https://example.com"), "https://example.com");
        assert_eq!(
            key("https://user:pw@example.com:8443/x"),
            "https://example.com:8443/x"
        );
        assert_eq!(
            key("https://example.com/#/inbox"),
            "https://example.com#/inbox"
        );
        assert_eq!(key("https://example.com/p#"), "https://example.com/p");
        // Valores repetidos e ordem: a mesma pagina da a mesma chave.
        assert_eq!(key("https://e.com/?b=1&a=2"), key("https://e.com?a=2&b=1"));
        assert_ne!(key("https://e.com/?a=1"), key("https://e.com/?a=2"));
        // Nao hierarquico: fica como esta.
        assert_eq!(key("mailto:a@example.com"), "mailto:a@example.com");
    }

    #[test]
    fn canonical_key_unwraps_google_redirects() {
        assert_eq!(
            key(
                "https://www.google.com/url?sa=t&q=https://www.example.com/a/%3Futm_source%3Dg%26x%3D1&ved=2"
            ),
            "https://example.com/a?x=1"
        );
        assert_eq!(
            key("https://google.com.br/url?url=https%3A%2F%2Fexample.com%2Fb%2F"),
            "https://example.com/b"
        );
        // Nao e o Google, nao e /url ou nao e http(s): fica.
        assert_eq!(
            key("https://evilgoogle.com/url?q=https://example.com/"),
            "https://evilgoogle.com/url?q=https%3A%2F%2Fexample.com%2F"
        );
        assert_eq!(
            key("https://www.google.com/search?q=https://example.com/"),
            "https://google.com/search?q=https%3A%2F%2Fexample.com%2F"
        );
        assert_eq!(
            key("https://www.google.com/url?q=javascript:alert(1)"),
            "https://google.com/url?q=javascript%3Aalert%281%29"
        );
    }
}
