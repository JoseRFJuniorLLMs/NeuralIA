use thiserror::Error;

#[derive(Debug, Error)]
pub enum NeuralError {
    #[error("entrada vazia")]
    EmptyInput,
    #[error("URL inválida: {0}")]
    InvalidUrl(String),
    #[error("esquema não permitido: {0}")]
    DisallowedScheme(String),
    #[error("resposta não é HTML: {0}")]
    UnsupportedContentType(String),
    #[error("falha de rede: {0}")]
    Network(#[from] ureq::Error),
    #[error("falha de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("falha de serialização: {0}")]
    Json(#[from] serde_json::Error),
    #[error("não foi possível extrair conteúdo legível")]
    ReaderExtraction,
}
pub type Result<T> = std::result::Result<T, NeuralError>;
