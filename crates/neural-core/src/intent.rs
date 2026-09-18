use url::Url;

use crate::{security::validate_web_url, NeuralError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Ask(String),
    Read(Url),
    Web(Url),
    Home,
}

pub fn parse_intent(input: &str) -> Result<Intent> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    if raw.eq_ignore_ascii_case("home:") || raw.eq_ignore_ascii_case("neural:home") {
        return Ok(Intent::Home);
    }
    for prefix in ["web:", "!web "] {
        if let Some(rest) = strip_prefix_ascii(raw, prefix) {
            return Ok(Intent::Web(parse_urlish(rest)?));
        }
    }
    for prefix in ["reader:", "read:", "!read "] {
        if let Some(rest) = strip_prefix_ascii(raw, prefix) {
            return Ok(Intent::Read(parse_urlish(rest)?));
        }
    }
    if let Some(query) = raw.strip_prefix('?') {
        let query = query.trim();
        if query.is_empty() { return Err(NeuralError::EmptyInput); }
        return Ok(Intent::Ask(query.to_string()));
    }
    if let Some(query) = strip_prefix_ascii(raw, "ask:") {
        let query = query.trim();
        if query.is_empty() { return Err(NeuralError::EmptyInput); }
        return Ok(Intent::Ask(query.to_string()));
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Ok(Intent::Read(validate_web_url(raw)?));
    }
    if looks_like_domain(raw) {
        return Ok(Intent::Read(parse_urlish(raw)?));
    }
    Ok(Intent::Ask(raw.to_string()))
}

fn parse_urlish(input: &str) -> Result<Url> {
    let trimmed = input.trim();
    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    validate_web_url(&candidate)
}

fn looks_like_domain(input: &str) -> bool {
    !input.contains(char::is_whitespace)
        && input.contains('.')
        && !input.starts_with('.')
        && !input.ends_with('.')
}

fn strip_prefix_ascii<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() < prefix.len() { return None; }
    let (head, tail) = value.split_at(prefix.len());
    head.eq_ignore_ascii_case(prefix).then_some(tail.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn text_is_ai(){ assert_eq!(parse_intent("como funciona raft").unwrap(),Intent::Ask("como funciona raft".into())); }
    #[test] fn url_is_reader(){ assert!(matches!(parse_intent("https://example.com").unwrap(),Intent::Read(_))); }
    #[test] fn domain_is_reader(){ assert!(matches!(parse_intent("example.com/docs").unwrap(),Intent::Read(_))); }
    #[test] fn web_prefix(){ assert!(matches!(parse_intent("web:example.com").unwrap(),Intent::Web(_))); }
}
