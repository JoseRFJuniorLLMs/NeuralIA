//! A API Gemini (Google AI Studio, `v1beta`): a listagem de modelos, com
//! paginas, e o `generateContent`. Os construtores sao puros e nunca veem a
//! chave -- devolvem um `ApiRequest` (caminho e corpo); o `ApiClient` junta
//! o host fixado e o cabecalho `x-goog-api-key`. Os leitores tratam a
//! resposta como texto nao confiavel: ids validados (`ModelId`), token de
//! pagina validado e codificado, e qualquer forma inesperada vira
//! `ApiError::Incomplete` sem citar o corpo.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::errors::ApiError;
use super::models::{ModelId, ModelInfo};
use super::transport::{
    ApiClient, ApiCredential, ApiRequest, GENERATION_TIMEOUT, LIST_TIMEOUT, Method, Provider,
};

const API_PREFIX: &str = "/v1beta";

/// O prefixo dos nomes de modelo na resposta da listagem.
const MODEL_NAME_PREFIX: &str = "models/";

/// Modelos por pagina (o maximo da API).
pub const LIST_PAGE_SIZE: u32 = 1000;

/// No maximo tantas paginas por listagem: um `nextPageToken` que nunca
/// acaba nao prende a thread.
pub const MAX_LIST_PAGES: usize = 8;

const MAX_PAGE_TOKEN_BYTES: usize = 1024;

/// O `nextPageToken` de uma pagina: 1 a 1024 caracteres ASCII visiveis. Vai
/// codificado para URL no pedido seguinte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageToken(String);

impl PageToken {
    pub fn parse(raw: &str) -> Option<Self> {
        let valid = (1..=MAX_PAGE_TOKEN_BYTES).contains(&raw.len())
            && raw.bytes().all(|byte| byte.is_ascii_graphic());
        valid.then(|| Self(raw.to_string()))
    }
}

/// O pedido de uma pagina da listagem de modelos.
pub fn list_models_request(page: Option<&PageToken>) -> ApiRequest {
    let mut path = format!("{API_PREFIX}/models?pageSize={LIST_PAGE_SIZE}");
    if let Some(token) = page {
        path.push_str("&pageToken=");
        path.extend(url::form_urlencoded::byte_serialize(token.0.as_bytes()));
    }
    ApiRequest {
        provider: Provider::Gemini,
        method: Method::Get,
        path,
        body: Vec::new(),
        timeout: LIST_TIMEOUT,
    }
}

/// Uma pagina da listagem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsPage {
    pub models: Vec<ModelInfo>,
    pub next: Option<PageToken>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPage {
    #[serde(default)]
    models: Vec<RawModel>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModel {
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
    #[serde(default)]
    input_token_limit: Option<u64>,
    #[serde(default)]
    output_token_limit: Option<u64>,
}

/// Le uma pagina. Modelos com nome fora da forma (`models/<id>` com um id
/// seguro) ficam de fora em silencio; um token de pagina invalido e uma
/// resposta incompleta.
pub fn parse_models_page(body: &[u8]) -> Result<ModelsPage, ApiError> {
    let raw: RawPage = serde_json::from_slice(body).map_err(|_| ApiError::Incomplete)?;
    let models = raw
        .models
        .into_iter()
        .filter_map(|model| {
            let id = ModelId::parse(model.name.strip_prefix(MODEL_NAME_PREFIX)?)?;
            let generates_text = model
                .supported_generation_methods
                .iter()
                .any(|method| method == "generateContent");
            Some(ModelInfo::new(
                id,
                model.display_name.as_deref().unwrap_or(""),
                generates_text,
                model.input_token_limit,
                model.output_token_limit,
            ))
        })
        .collect();
    let next = match raw.next_page_token.as_deref() {
        None | Some("") => None,
        Some(token) => Some(PageToken::parse(token).ok_or(ApiError::Incomplete)?),
    };
    Ok(ModelsPage { models, next })
}

/// Passo 1 da escolha: a listagem inteira, pagina a pagina (15 s cada), ate
/// nao haver `nextPageToken`, o token se repetir ou chegar a
/// `MAX_LIST_PAGES`.
pub fn list_models(
    client: &ApiClient,
    key: Option<&dyn ApiCredential>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<ModelInfo>, ApiError> {
    let mut models = Vec::new();
    let mut page: Option<PageToken> = None;
    for _ in 0..MAX_LIST_PAGES {
        let body = client.send(&list_models_request(page.as_ref()), key, cancelled)?;
        let parsed = parse_models_page(&body)?;
        models.extend(parsed.models);
        match parsed.next {
            Some(next) if page.as_ref() != Some(&next) => page = Some(next),
            _ => break,
        }
    }
    Ok(models)
}

/// O texto a gerar. A cerca do texto nao confiavel (`PromptBuilder`) chega
/// com a parte 2 (`infra-llm-untrusted`); ate la nenhuma feature chama isto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextPrompt<'a> {
    pub system: Option<&'a str>,
    pub user: &'a str,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    /// Pede `application/json` na resposta.
    pub json_output: bool,
}

/// O pedido `generateContent` de um modelo.
pub fn generate_content_request(model: &ModelId, prompt: &TextPrompt<'_>) -> ApiRequest {
    let mut body = Map::new();
    body.insert(
        "contents".to_string(),
        json!([{ "role": "user", "parts": [{ "text": prompt.user }] }]),
    );
    if let Some(system) = prompt.system {
        body.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": system }] }),
        );
    }
    let mut config = Map::new();
    if let Some(temperature) = prompt.temperature {
        config.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(tokens) = prompt.max_output_tokens {
        config.insert("maxOutputTokens".to_string(), json!(tokens));
    }
    if prompt.json_output {
        config.insert("responseMimeType".to_string(), json!("application/json"));
    }
    if !config.is_empty() {
        body.insert("generationConfig".to_string(), Value::Object(config));
    }
    ApiRequest {
        provider: Provider::Gemini,
        method: Method::Post,
        path: format!("{API_PREFIX}/models/{model}:generateContent"),
        body: serde_json::to_vec(&Value::Object(body)).unwrap_or_default(),
        timeout: GENERATION_TIMEOUT,
    }
}

/// Porque o modelo parou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    /// Chegou ao `maxOutputTokens`: o texto esta cortado.
    MaxTokens,
    /// Seguranca, recitacao, lista de bloqueio.
    Blocked,
    Other,
}

/// O texto gerado (sem as partes de raciocinio, `thought: true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub text: String,
    pub finish: FinishReason,
}

#[derive(Deserialize)]
struct RawGenerate {
    #[serde(default)]
    candidates: Vec<RawCandidate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCandidate {
    #[serde(default)]
    content: Option<RawContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct RawContent {
    #[serde(default)]
    parts: Vec<RawPart>,
}

#[derive(Deserialize)]
struct RawPart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thought: bool,
}

/// Le a resposta do `generateContent`: o texto do primeiro candidato. Sem
/// candidato ou sem texto e uma resposta incompleta.
pub fn parse_generate_content(body: &[u8]) -> Result<Generated, ApiError> {
    let raw: RawGenerate = serde_json::from_slice(body).map_err(|_| ApiError::Incomplete)?;
    let candidate = raw
        .candidates
        .into_iter()
        .next()
        .ok_or(ApiError::Incomplete)?;
    let text: String = candidate
        .content
        .map(|content| content.parts)
        .unwrap_or_default()
        .into_iter()
        .filter(|part| !part.thought)
        .filter_map(|part| part.text)
        .collect();
    if text.is_empty() {
        return Err(ApiError::Incomplete);
    }
    let finish = match candidate.finish_reason.as_deref() {
        None | Some("STOP") => FinishReason::Stop,
        Some("MAX_TOKENS") => FinishReason::MaxTokens,
        Some(
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY",
        ) => FinishReason::Blocked,
        Some(_) => FinishReason::Other,
    };
    Ok(Generated { text, finish })
}

/// Gera texto com um modelo (60 s de prazo).
pub fn generate_content(
    client: &ApiClient,
    model: &ModelId,
    prompt: &TextPrompt<'_>,
    key: Option<&dyn ApiCredential>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Generated, ApiError> {
    let body = client.send(&generate_content_request(model, prompt), key, cancelled)?;
    parse_generate_content(&body)
}
