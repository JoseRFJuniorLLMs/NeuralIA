use url::Url;

use crate::{NeuralError, Result, security::validate_web_url};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Ask(String),
    /// Comparacao lado-a-lado, o destino normal de uma pergunta: a mesma
    /// pergunta segue para os tres fornecedores. `ask:` ou `?` limitam-na ao
    /// Google AI Mode quando o utilizador nao quer esse leque.
    Compare(String),
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

    for prefix in ["compare:", "comparar:", "!compare "] {
        if let Some(rest) = strip_prefix_ascii(raw, prefix) {
            let query = rest.trim();
            if query.is_empty() {
                return Err(NeuralError::EmptyInput);
            }
            return Ok(Intent::Compare(query.to_string()));
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
        let url = validate_web_url(raw)?;
        // O Reader so sabe ler HTML: mandar-lhe um PDF dava um erro seco e o
        // ficheiro nunca chegava a abrir. O visualizador embutido trata disso.
        return Ok(if is_pdf_url(&url) {
            Intent::Web(url)
        } else {
            Intent::Read(url)
        });
    }
    if looks_like_domain(raw) {
        let url = parse_urlish(raw)?;
        return Ok(if is_pdf_url(&url) {
            Intent::Web(url)
        } else {
            Intent::Read(url)
        });
    }

    // Texto normal vai para o comparador. Quem quiser um unico fornecedor usa
    // `ask:` ou `?`, que ficam no Google AI Mode.
    Ok(Intent::Compare(raw.to_string()))
}

/// Verdadeiro quando o caminho da URL aponta para um PDF. So olha para o
/// caminho: a query pode trazer qualquer coisa.
pub fn is_pdf_url(url: &Url) -> bool {
    url.path().to_ascii_lowercase().ends_with(".pdf")
}

fn parse_urlish(input: &str) -> Result<Url> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    if looks_like_local_path(trimmed) {
        return Err(NeuralError::LocalPath(redact_local_path(trimmed)));
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower == "localhost" || lower.starts_with("localhost:") {
        return validate_web_url(&format!("http://{trimmed}"));
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
    fn compare_prefix_is_explicit_fan_out() {
        for input in [
            "compare: mvcc vs occ",
            "comparar:mvcc vs occ",
            "!compare mvcc vs occ",
            "COMPARE: mvcc vs occ",
        ] {
            assert_eq!(
                parse_intent(input).unwrap(),
                Intent::Compare("mvcc vs occ".to_string()),
                "{input}"
            );
        }
    }

    #[test]
    fn plain_question_goes_to_the_comparator() {
        for input in ["como funciona raft", "mvcc vs occ", "jose r f junior"] {
            assert!(
                matches!(parse_intent(input).unwrap(), Intent::Compare(_)),
                "{input} devia abrir o comparador"
            );
        }
    }

    #[test]
    fn ask_prefix_stays_on_a_single_provider() {
        // A saida para quem nao quer a pergunta em tres sitios ao mesmo tempo.
        for input in ["? mvcc vs occ", "ask: mvcc vs occ"] {
            assert_eq!(
                parse_intent(input).unwrap(),
                Intent::Ask("mvcc vs occ".to_string()),
                "{input}"
            );
        }
    }

    #[test]
    fn empty_compare_is_rejected() {
        assert!(parse_intent("compare:").is_err());
        assert!(parse_intent("compare:   ").is_err());
    }

    #[test]
    fn text_is_ai() {
        assert_eq!(
            parse_intent("como funciona raft").unwrap(),
            Intent::Compare("como funciona raft".into())
        );
    }

    #[test]
    fn pdf_goes_to_the_viewer_not_the_reader() {
        // O Reader rejeita tudo o que nao seja HTML; um PDF tem de ir inteiro
        // para o WebView, senao nunca abre.
        for input in [
            "https://example.com/artigo.pdf",
            "https://example.com/a/b/RELATORIO.PDF",
            "example.com/ficheiro.pdf?download=1",
        ] {
            assert!(
                matches!(parse_intent(input).unwrap(), Intent::Web(_)),
                "{input} devia abrir no visualizador"
            );
        }
    }

    #[test]
    fn pdf_in_the_query_is_not_a_pdf() {
        assert!(matches!(
            parse_intent("https://example.com/artigo?ref=guia.pdf").unwrap(),
            Intent::Read(_)
        ));
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
        // Texto com dois pontos nao e esquema nem prefixo: segue o caminho
        // normal, que hoje e o comparador.
        assert!(matches!(
            parse_intent("rust: ownership model").unwrap(),
            Intent::Compare(_)
        ));
    }

    #[test]
    fn prefixed_navigation_rejects_unsafe_scheme() {
        assert!(parse_intent("web:file:///etc/passwd").is_err());
        assert!(parse_intent("reader:javascript:alert(1)").is_err());
    }
}
