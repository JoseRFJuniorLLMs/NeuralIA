use url::Url;
use crate::{NeuralError, Result};

pub fn google_ai_url(query: &str, language: &str) -> Result<Url> {
    let query = query.trim();
    if query.is_empty() { return Err(NeuralError::EmptyInput); }
    let mut url = Url::parse("https://www.google.com/search")
        .map_err(|_| NeuralError::InvalidUrl("Google search endpoint".into()))?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("udm", "50")
        .append_pair("hl", language);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ai_mode_url(){
        let u=google_ai_url("raft consensus","pt-BR").unwrap();
        assert_eq!(u.host_str(),Some("www.google.com"));
        assert!(u.as_str().contains("udm=50"));
    }
}
