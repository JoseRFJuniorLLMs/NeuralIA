use url::Url;
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
        return Err(NeuralError::InvalidUrl("credenciais embutidas na URL não são aceitas".into()));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn accepts_https(){ assert!(validate_web_url("https://example.com/a").is_ok()); }
    #[test] fn rejects_unsafe_schemes(){
        assert!(validate_web_url("javascript:alert(1)").is_err());
        assert!(validate_web_url("file:///etc/passwd").is_err());
    }
    #[test] fn rejects_credentials(){ assert!(validate_web_url("https://u:p@example.com").is_err()); }
}
