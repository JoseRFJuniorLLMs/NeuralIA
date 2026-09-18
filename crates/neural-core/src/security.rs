use std::net::IpAddr;

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

/// Resolve and validate an HTTP redirect before the next network request.
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

/// True for obvious local/special URL targets before DNS resolution.
pub fn is_local_network_target(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => is_forbidden_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => is_forbidden_ip(IpAddr::V6(ip)),
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
        }
        None => false,
    }
}

/// Reject addresses that are not globally routable Internet destinations.
///
/// This is deliberately conservative for Reader mode: loopback, private,
/// link-local, carrier-grade NAT, documentation, benchmarking, multicast and
/// reserved address space are not valid targets for an untrusted public page.
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 88 && c == 99)
                || (a == 192 && b == 168)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 224
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_forbidden_ip(IpAddr::V4(mapped));
            }

            let segments = ip.segments();
            ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

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
    fn rejects_special_resolved_addresses() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            "2001:db8::1".parse().unwrap(),
        ] {
            assert!(is_forbidden_ip(ip), "{ip}");
        }
        assert!(!is_forbidden_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_forbidden_ip("2606:4700:4700::1111".parse().unwrap()));
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
