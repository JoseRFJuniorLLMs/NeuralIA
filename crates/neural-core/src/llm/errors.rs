//! Os erros das chamadas de IA, cada um com a mensagem pt-BR que o
//! utilizador ve. Um `ApiError` so leva numeros que nao identificam nada (o
//! estado HTTP, os segundos de espera, o tecto): nunca a chave, nunca o corpo
//! da resposta nem um pedaco dele. O corpo de um erro e lido aqui, com
//! tecto, so para distinguir "chave recusada" de "sem creditos" -- e morre
//! aqui.

use std::fmt;

/// O maior "tente em N s" que se mostra: um `Retry-After` hostil ou absurdo
/// nao vira um numero de horas no cartao.
pub const MAX_RETRY_AFTER_SECS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 401/403, ou a razao de chave invalida no corpo.
    KeyRejected { status: u16 },
    /// 402, ou a razao de quota/creditos/faturacao no corpo.
    NoCredits { status: u16 },
    /// 429 sem razao de creditos. `retry_after_secs` vem do `Retry-After` ou
    /// do `retryDelay` da Gemini.
    RateLimited { retry_after_secs: Option<u64> },
    /// 404: o modelo nao existe (ou deixou de existir) nesta API.
    ModelUnavailable { status: u16 },
    /// 3xx (nao se segue), 5xx, ou a rede (`status: None`).
    ServiceUnavailable { status: Option<u16> },
    /// O prazo da chamada (ou 408/504 do servidor).
    Timeout,
    /// Uma resposta 2xx que nao se consegue ler ou que nao traz o esperado.
    Incomplete,
    /// O corpo passou do tecto (declarado ou descodificado).
    TooLarge { limit: u64 },
    /// O utilizador desistiu.
    Cancelled,
    /// Outro 4xx: o servico recusou este pedido.
    Rejected { status: u16 },
}

impl ApiError {
    /// A mensagem para o utilizador, em pt-BR.
    pub fn pt_br_message(&self) -> String {
        match self {
            Self::KeyRejected { .. } => "Chave recusada".to_string(),
            Self::NoCredits { .. } => "Sem créditos ou limite de gastos".to_string(),
            Self::RateLimited {
                retry_after_secs: Some(secs),
            } => format!("Muitos pedidos, tente em {secs} s"),
            Self::RateLimited {
                retry_after_secs: None,
            } => "Muitos pedidos, tente mais tarde".to_string(),
            Self::ModelUnavailable { .. } => "Modelo indisponível, escolha outro".to_string(),
            Self::ServiceUnavailable { .. } => "Serviço indisponível".to_string(),
            Self::Timeout => "Demorou demais".to_string(),
            Self::Incomplete => "Resposta incompleta".to_string(),
            Self::TooLarge { .. } => "Resposta grande demais".to_string(),
            Self::Cancelled => "Cancelado".to_string(),
            Self::Rejected { status } => format!("Pedido recusado (HTTP {status})"),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.pt_br_message())
    }
}

impl std::error::Error for ApiError {}

/// As razoes de chave recusada que as APIs escrevem no corpo (a Gemini
/// responde 400 `API_KEY_INVALID` a uma chave errada).
const KEY_REASONS: &[&str] = &[
    "api_key_invalid",
    "api key not valid",
    "invalid_api_key",
    "invalid x-api-key",
    "authentication_error",
];

/// As razoes de quota, creditos ou faturacao.
const CREDIT_REASONS: &[&str] = &["insufficient_quota", "credit balance", "billing"];

/// Classifica um estado HTTP que nao e 2xx. O corpo so e procurado por
/// razoes conhecidas; nada dele entra no erro.
pub(crate) fn classify_status(status: u16, retry_after: Option<&str>, body: &[u8]) -> ApiError {
    let credits = has_any(body, CREDIT_REASONS);
    let key = has_any(body, KEY_REASONS);
    match status {
        300..=399 => ApiError::ServiceUnavailable {
            status: Some(status),
        },
        402 => ApiError::NoCredits { status },
        401 | 403 if credits => ApiError::NoCredits { status },
        401 | 403 => ApiError::KeyRejected { status },
        400 if key => ApiError::KeyRejected { status },
        400 if credits => ApiError::NoCredits { status },
        404 => ApiError::ModelUnavailable { status },
        408 | 504 => ApiError::Timeout,
        429 if credits => ApiError::NoCredits { status },
        429 => ApiError::RateLimited {
            retry_after_secs: retry_after
                .and_then(parse_retry_after)
                .or_else(|| gemini_retry_delay(body)),
        },
        400..=499 => ApiError::Rejected { status },
        _ => ApiError::ServiceUnavailable {
            status: Some(status),
        },
    }
}

fn has_any(body: &[u8], needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        let needle = needle.as_bytes();
        body.windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
    })
}

/// `Retry-After` em segundos (a forma de data HTTP nao se usa aqui).
fn parse_retry_after(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(
        value
            .parse::<u64>()
            .unwrap_or(u64::MAX)
            .min(MAX_RETRY_AFTER_SECS),
    )
}

/// A Gemini diz quanto esperar em `error.details[].retryDelay` (`"37s"`).
fn gemini_retry_delay(body: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("error")?
        .get("details")?
        .as_array()?
        .iter()
        .filter_map(|detail| detail.get("retryDelay")?.as_str())
        .find_map(parse_duration_secs)
}

fn parse_duration_secs(text: &str) -> Option<u64> {
    let seconds: f64 = text.trim().strip_suffix('s')?.parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some((seconds.ceil() as u64).min(MAX_RETRY_AFTER_SECS))
}
