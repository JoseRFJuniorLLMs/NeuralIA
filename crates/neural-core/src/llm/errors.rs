//! Os erros das chamadas de IA, cada um com a mensagem pt-BR que o
//! utilizador ve. Um `ApiError` so leva numeros que nao identificam nada (o
//! estado HTTP, os segundos de espera, o tecto): nunca a chave, nunca o corpo
//! da resposta nem um pedaco dele. O corpo de um erro e lido aqui, com
//! tecto, so para distinguir "chave recusada", "sem creditos" e "muitos
//! pedidos" (com a espera) -- e morre aqui.

use std::fmt;

/// O maior "tente em N s" que se mostra: um `Retry-After` hostil ou absurdo
/// nao vira um numero de horas no cartao.
pub const MAX_RETRY_AFTER_SECS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 401/403, ou a razao de chave invalida no corpo.
    KeyRejected { status: u16 },
    /// 402; num 400/401/403, a razao de creditos ou de faturacao no corpo;
    /// num 429, so uma razao explicita de creditos ou a quota diaria da
    /// Gemini esgotada.
    NoCredits { status: u16 },
    /// Qualquer outro 429, a quota por minuto da Gemini incluida (o texto
    /// dela fala de faturacao). `retry_after_secs` vem do `Retry-After` ou do
    /// `retryDelay` da Gemini.
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

/// As razoes que so querem dizer "sem creditos", em qualquer estado.
const CREDIT_REASONS: &[&str] = &["insufficient_quota", "credit balance"];

/// A faturacao: diz "sem creditos" num 400/401/403, nunca num 429. A Gemini
/// (e a OpenAI) escrevem "check your plan and billing details" em toda a
/// quota esgotada, tambem na de por minuto, que passa sozinha.
const BILLING_REASONS: &[&str] = &["billing"];

/// Classifica um estado HTTP que nao e 2xx. O corpo so e procurado por
/// razoes conhecidas; nada dele entra no erro.
pub(crate) fn classify_status(status: u16, retry_after: Option<&str>, body: &[u8]) -> ApiError {
    let credits = has_any(body, CREDIT_REASONS) || has_any(body, BILLING_REASONS);
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
        429 => classify_too_many_requests(retry_after, body),
        400..=499 => ApiError::Rejected { status },
        _ => ApiError::ServiceUnavailable {
            status: Some(status),
        },
    }
}

/// Um 429 e "sem creditos" so com uma razao explicita de creditos
/// (`insufficient_quota`, `credit balance`) ou com uma quota diaria da Gemini
/// esgotada, que so volta no dia seguinte por mais que o `retryDelay` diga
/// segundos. Qualquer outro e "muitos pedidos", e a espera do `Retry-After`
/// ou do `retryDelay` chega ao cartao.
fn classify_too_many_requests(retry_after: Option<&str>, body: &[u8]) -> ApiError {
    let details = gemini_error_details(body);
    if has_any(body, CREDIT_REASONS) || details.iter().any(is_daily_quota_failure) {
        return ApiError::NoCredits { status: 429 };
    }
    ApiError::RateLimited {
        retry_after_secs: retry_after
            .and_then(parse_retry_after)
            .or_else(|| gemini_retry_delay(&details)),
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

/// Os `error.details[]` de um erro da Gemini; vazio em qualquer outro corpo.
fn gemini_error_details(body: &[u8]) -> Vec<serde_json::Value> {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    match value
        .pointer_mut("/error/details")
        .map(serde_json::Value::take)
    {
        Some(serde_json::Value::Array(details)) => details,
        _ => Vec::new(),
    }
}

/// A Gemini diz que quota se esgotou em `QuotaFailure.violations[].quotaId`:
/// `GenerateRequestsPerDayPerProjectPerModel-FreeTier` e diaria,
/// `GenerateRequestsPerMinutePerProjectPerModel-FreeTier` e por minuto.
fn is_daily_quota_failure(detail: &serde_json::Value) -> bool {
    detail
        .get("violations")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|violations| {
            violations.iter().any(|violation| {
                violation
                    .get("quotaId")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|quota| quota.contains("PerDay"))
            })
        })
}

/// A Gemini diz quanto esperar em `error.details[].retryDelay` (`"37s"`).
fn gemini_retry_delay(details: &[serde_json::Value]) -> Option<u64> {
    details
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
