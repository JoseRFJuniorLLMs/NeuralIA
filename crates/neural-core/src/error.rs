use thiserror::Error;

#[derive(Debug, Error)]
pub enum NeuralError {
    #[error("entrada vazia")]
    EmptyInput,
    #[error("URL inválida: {0}")]
    InvalidUrl(String),
    #[error("esquema não permitido: {0}")]
    DisallowedScheme(String),
    #[error("caminho local não pode ser enviado pela omnibox: {0}")]
    LocalPath(String),
    #[error("redirecionamento inválido: {0}")]
    InvalidRedirect(String),
    #[error("redirecionamento bloqueado para rede local: {0}")]
    UnsafeRedirect(String),
    #[error("limite de redirecionamentos excedido")]
    RedirectLimit,
    #[error("tempo total do Reader excedido")]
    ReaderDeadline,
    #[error("leitura cancelada")]
    ReaderCancelled,
    #[error("resposta excede o limite do Reader ({declared} > {limit} bytes)")]
    ResponseTooLarge { declared: u64, limit: u64 },
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
