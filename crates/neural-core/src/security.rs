use std::net::IpAddr;

use ureq::{
    config::Config,
    http::Uri,
    unversioned::{
        resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver},
        transport::NextTimeout,
    },
};
use url::{Host, Url};

use crate::{NeuralError, Result};

#[derive(Debug, Default)]
pub(crate) struct PublicResolver {
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
            domain == "localhost" || domain.ends_with(".localhost") || domain.ends_with(".local")
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

            // Enderecos que embutem um IPv4 sao reencaminhados para a regra
            // IPv4: 6to4 e NAT64 seriam, de outro modo, um tunel direto para a
            // rede local.
            if segments_embed_ipv4(&ip) {
                if let Some(embedded) = embedded_ipv4(&ip) {
                    return is_forbidden_ip(IpAddr::V4(embedded));
                }
                return true;
            }

            let segments = ip.segments();
            ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                // ::/96 compativel com IPv4 (obsoleto) e ::ffff:0:0/96 ja tratados
                || (segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0)
                // fc00::/7 unique local
                || (segments[0] & 0xfe00) == 0xfc00
                // fe80::/10 link-local
                || (segments[0] & 0xffc0) == 0xfe80
                // fec0::/10 site-local (obsoleto, ainda encaminhavel em redes velhas)
                || (segments[0] & 0xffc0) == 0xfec0
                // 100::/64 discard-only
                || (segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0)
                // 2001:db8::/32 documentacao
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                // 2001::/32 Teredo e 2001:2::/48 benchmarking
                || (segments[0] == 0x2001 && segments[1] == 0x0000)
                || (segments[0] == 0x2001 && segments[1] == 0x0002 && segments[2] == 0)
        }
    }
}

/// 6to4 (2002::/16) e NAT64 (64:ff9b::/96 e 64:ff9b:1::/48) carregam um IPv4 la
/// dentro; sem os desempacotar, `2002:7f00:0001::` seria uma rota para 127.0.0.1.
fn segments_embed_ipv4(ip: &std::net::Ipv6Addr) -> bool {
    let s = ip.segments();
    s[0] == 0x2002
        || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0)
        || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0x0001)
}

fn embedded_ipv4(ip: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let s = ip.segments();
    if s[0] == 0x2002 {
        return Some(std::net::Ipv4Addr::new(
            (s[1] >> 8) as u8,
            (s[1] & 0xff) as u8,
            (s[2] >> 8) as u8,
            (s[2] & 0xff) as u8,
        ));
    }
    if s[0] == 0x0064 && s[1] == 0xff9b {
        // 64:ff9b::/96 mete o IPv4 nos ultimos 32 bits; 64:ff9b:1::/48 tem
        // varios formatos de prefixo, por isso so aceitamos o /96 canonico.
        if s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
            return Some(std::net::Ipv4Addr::new(
                (s[6] >> 8) as u8,
                (s[6] & 0xff) as u8,
                (s[7] >> 8) as u8,
                (s[7] & 0xff) as u8,
            ));
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    #[test]
    fn rejects_ipv6_tunnels_to_private_space() {
        // 6to4 e NAT64 carregam um IPv4 la dentro: se nao os desempacotarmos,
        // sao uma rota limpa para o loopback e para a rede local.
        for address in [
            "2002:7f00:0001::", // 6to4 -> 127.0.0.1
            "2002:c0a8:0101::", // 6to4 -> 192.168.1.1
            "64:ff9b::7f00:1",  // NAT64 -> 127.0.0.1
            "64:ff9b::a00:1",   // NAT64 -> 10.0.0.1
            "64:ff9b:1::1",     // NAT64 com prefixo nao canonico
        ] {
            let ip: Ipv6Addr = address.parse().expect(address);
            assert!(
                is_forbidden_ip(IpAddr::V6(ip)),
                "{address} devia ser negado"
            );
        }
    }

    #[test]
    fn rejects_reserved_ipv6_ranges() {
        for address in [
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "100::1",
            "2001:db8::1",
            "2001::1",
            "2001:2::1",
            "ff02::1",
        ] {
            let ip: Ipv6Addr = address.parse().expect(address);
            assert!(
                is_forbidden_ip(IpAddr::V6(ip)),
                "{address} devia ser negado"
            );
        }
    }

    #[test]
    fn accepts_public_ipv6() {
        for address in ["2606:4700:4700::1111", "2a00:1450:4003:80a::200e"] {
            let ip: Ipv6Addr = address.parse().expect(address);
            assert!(
                !is_forbidden_ip(IpAddr::V6(ip)),
                "{address} devia ser aceite"
            );
        }
    }

    #[test]
    fn accepts_6to4_pointing_at_public_space() {
        // 2002:0808:0808:: -> 8.8.8.8, que e publico e deve passar.
        let ip: Ipv6Addr = "2002:0808:0808::".parse().unwrap();
        assert!(!is_forbidden_ip(IpAddr::V6(ip)));
    }

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
