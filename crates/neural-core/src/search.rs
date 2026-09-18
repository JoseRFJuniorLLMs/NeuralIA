use crate::{NeuralError, Result};
use url::Url;

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
    url.query_pairs_mut()
        .append_pair("q", query);
    Ok(url)
}

pub fn perplexity_search_url(query: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() {
        return Err(NeuralError::EmptyInput);
    }
    let mut url = Url::parse("https://www.perplexity.ai/search")
        .map_err(|_| NeuralError::InvalidUrl("Perplexity search endpoint".into()))?;
    url.query_pairs_mut()
        .append_pair("q", query);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

