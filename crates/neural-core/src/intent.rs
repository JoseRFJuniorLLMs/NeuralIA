use url::Url;

use crate::{NeuralError, Result, security::validate_web_url};

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
        if query.is_empty() {
            return Err(NeuralError::EmptyInput);
        }
        return Ok(Intent::Ask(query.to_string()));
    }
    if let Some(query) = strip_prefix_ascii(raw, "ask:") {
        let query = query.trim();
        if query.is_empty() {
            return Err(NeuralError::EmptyInput);
        }
        return Ok(Intent::Ask(query.to_string()));
    }

    reject_implicit_local_or_scheme(raw)?;

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
    if trimmed.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    if looks_like_local_path(trimmed) {
        return Err(NeuralError::LocalPath(redact_local_path(trimmed)));
    }
    if let Some(scheme) = explicit_scheme(trimmed) {
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(NeuralError::DisallowedScheme(scheme.to_ascii_lowercase()));
        }
        return validate_web_url(trimmed);
    }
    validate_web_url(&format!("https://{trimmed}"))
}

fn reject_implicit_local_or_scheme(input: &str) -> Result<()> {
    if looks_like_local_path(input) {
        return Err(NeuralError::LocalPath(redact_local_path(input)));
    }

    let lower = input.to_ascii_lowercase();
    if lower == "localhost" || lower.starts_with("localhost:") {
        return Ok(());
    }

    if let Some(scheme) = explicit_scheme(input) {
        if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            return Ok(());
        }

        let compact = !input.chars().any(char::is_whitespace);
        let dangerous = matches!(
            scheme.to_ascii_lowercase().as_str(),
            "file" | "javascript" | "data" | "blob" | "ftp" | "about" | "chrome" | "edge"
        );
        let authority = input
            .get(scheme.len() + 1..)
            .is_some_and(|rest| rest.starts_with("//"));

        if compact || dangerous || authority {
            return Err(NeuralError::DisallowedScheme(scheme.to_ascii_lowercase()));
        }
    }
    Ok(())
}

fn explicit_scheme(input: &str) -> Option<&str> {
    let colon = input.find(':')?;
    let scheme = &input[..colon];
    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')) {
        return None;
    }
    Some(scheme)
}

fn looks_like_local_path(input: &str) -> bool {
    if input.starts_with("\\")
        || input.starts_with('/')
        || input.starts_with("~/")
        || input.starts_with("./")
        || input.starts_with("../")
    {
        return true;
    }

    let bytes = input.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn redact_local_path(input: &str) -> String {
    let kind = if input.starts_with("\\") {
        "caminho UNC"
    } else if input.starts_with('/') || input.starts_with("~/") {
        "caminho local"
    } else {
        "caminho local Windows"
    };
    kind.to_string()
}

fn looks_like_domain(input: &str) -> bool {
    if input.contains(char::is_whitespace) {
        return false;
    }
    let lower = input.to_ascii_lowercase();
    (input.contains('.') && !input.starts_with('.') && !input.ends_with('.'))
        || lower == "localhost"
        || lower.starts_with("localhost:")
        || (input.starts_with('[') && input.contains(']'))
}

fn strip_prefix_ascii<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() < prefix.len() {
        return None;
    }
    let (head, tail) = value.split_at(prefix.len());
    head.eq_ignore_ascii_case(prefix).then_some(tail.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_ai() {
        assert_eq!(
            parse_intent("como funciona raft").unwrap(),
            Intent::Ask("como funciona raft".into())
        );
    }

    #[test]
    fn url_is_reader() {
        assert!(matches!(
            parse_intent("https://example.com").unwrap(),
            Intent::Read(_)
        ));
    }

    #[test]
    fn domain_is_reader() {
        assert!(matches!(
            parse_intent("example.com/docs").unwrap(),
            Intent::Read(_)
        ));
    }

    #[test]
    fn unicode_domain_is_reader() {
        assert!(matches!(
            parse_intent("münchen.de").unwrap(),
            Intent::Read(_)
        ));
    }

    #[test]
    fn localhost_is_reader_not_remote_search() {
        assert!(matches!(
            parse_intent("localhost:8080").unwrap(),
            Intent::Read(_)
        ));
    }

    #[test]
    fn web_prefix() {
        assert!(matches!(
            parse_intent("web:example.com").unwrap(),
            Intent::Web(_)
        ));
    }

    #[test]
    fn unsafe_schemes_are_not_queries() {
        for input in [
            "file:///C:/Users/Eva/secret.txt",
            "javascript:alert(1)",
            "data:text/plain,secret",
            "blob:https://example.com/id",
            "ftp://example.com/file",
        ] {
            assert!(parse_intent(input).is_err(), "{input}");
        }
    }

    #[test]
    fn local_paths_are_not_queries() {
        for input in [
            r"C:\Users\Eva\secret.txt",
            r"\\server\share\secret.txt",
            "/home/eva/.ssh/id_rsa",
            "../secret.txt",
        ] {
            assert!(matches!(
                parse_intent(input),
                Err(NeuralError::LocalPath(_))
            ));
        }
    }

    #[test]
    fn explicit_ask_can_contain_colon_text() {
        assert!(matches!(
            parse_intent("ask:file format on Windows").unwrap(),
            Intent::Ask(_)
        ));
        assert!(matches!(
            parse_intent("rust: ownership model").unwrap(),
            Intent::Ask(_)
        ));
    }

    #[test]
    fn prefixed_navigation_rejects_unsafe_scheme() {
        assert!(parse_intent("web:file:///etc/passwd").is_err());
        assert!(parse_intent("reader:javascript:alert(1)").is_err());
    }
}
