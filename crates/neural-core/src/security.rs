use url::{Host, Url};

use crate::{NeuralError, Result};

pub fn validate_web_url(input: &str) -> Result<Url> {
    let url = Url::parse(input).map_err(|_| NeuralError::InvalidUrl(input.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(NeuralError::DisallowedScheme(other.to_string())),
    }
    if url.host_str().is_none() {
        return Err(NeuralError::InvalidUrl(input.to_string()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(NeuralError::InvalidUrl(
            "credenciais embutidas na URL não são aceitas".into(),
        ));
    }
    Ok(url)
}

/// Resolves and validates an HTTP redirect before the next network request.
///
/// A page that starts on the public Internet is not allowed to bounce Reader
/// into obvious loopback/private/link-local targets. Direct user navigation to
/// a local HTTP(S) service remains possible; the protection is specifically
/// against hostile public -> local redirects.
pub fn validate_redirect_target(previous: &Url, location: &str) -> Result<Url> {
    let joined = previous
        .join(location)
        .or_else(|_| Url::parse(location))
        .map_err(|_| NeuralError::InvalidRedirect("Location inválido".into()))?;
    let next = validate_web_url(joined.as_str())?;

    if !is_local_network_target(previous) && is_local_network_target(&next) {
        let host = next.host_str().unwrap_or("<local>");
        return Err(NeuralError::UnsafeRedirect(host.to_string()));
    }
    Ok(next)
}

pub fn is_local_network_target(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => is_local_ipv4(ip.octets()),
        Some(Host::Ipv6(ip)) => {
            if ip.is_loopback() || ip.is_unspecified() {
                return true;
            }
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_local_ipv4(mapped.octets());
            }
            let first = ip.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
        }
        None => false,
    }
}

fn is_local_ipv4([a, b, _, _]: [u8; 4]) -> bool {
    a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https() {
        assert!(validate_web_url("https://example.com/a").is_ok());
    }

    #[test]
    fn rejects_unsafe_schemes() {
        assert!(validate_web_url("javascript:alert(1)").is_err());
        assert!(validate_web_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn rejects_credentials() {
        assert!(validate_web_url("https://u:p@example.com").is_err());
    }

    #[test]
    fn detects_local_targets() {
        for value in [
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://169.254.1.1/",
            "http://localhost/",
            "http://printer.local/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fc00::1]/",
        ] {
            let url = Url::parse(value).unwrap();
            assert!(is_local_network_target(&url), "{value}");
        }
        assert!(!is_local_network_target(
            &Url::parse("https://example.com/").unwrap()
        ));
    }

    #[test]
    fn public_to_local_redirect_is_blocked() {
        let from = Url::parse("https://example.com/start").unwrap();
        assert!(matches!(
            validate_redirect_target(&from, "http://127.0.0.1:8080/admin"),
            Err(NeuralError::UnsafeRedirect(_))
        ));
    }

    #[test]
    fn relative_redirect_is_resolved() {
        let from = Url::parse("https://example.com/a/start").unwrap();
        let to = validate_redirect_target(&from, "../final").unwrap();
        assert_eq!(to.as_str(), "https://example.com/final");
    }
}
