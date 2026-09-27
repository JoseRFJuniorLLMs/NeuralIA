//! As quatro ferramentas que um agente externo ve, e a validacao delas.
//!
//! A mesma funcao (`parse_tool_call`) corre duas vezes: na ponte
//! (`NeuralIA.exe --mcp`), para o modelo receber o erro sem esperar pelo
//! NeuralIA, e no hub, dentro do NeuralIA, que nunca confia no que a ponte
//! diz ter validado. Tudo o que chega a um ecra passa por `clean_text`.

use serde_json::{Map, Value, json};

pub(crate) const TOOL_SEND_MESSAGE: &str = "send_message";
pub(crate) const TOOL_ASK_USER: &str = "ask_user";
pub(crate) const TOOL_GET_USER_MESSAGES: &str = "get_user_messages";
pub(crate) const TOOL_SET_STATUS: &str = "set_status";

/// Os nomes publicados, pela ordem de `tools/list`.
pub(crate) const TOOL_NAMES: [&str; 4] = [
    TOOL_SEND_MESSAGE,
    TOOL_ASK_USER,
    TOOL_GET_USER_MESSAGES,
    TOOL_SET_STATUS,
];

pub(crate) const MESSAGE_MAX_CHARS: usize = 4_000;
pub(crate) const TITLE_MAX_CHARS: usize = 80;
pub(crate) const QUESTION_MAX_CHARS: usize = 1_000;
pub(crate) const OPTION_MAX_CHARS: usize = 40;
pub(crate) const OPTIONS_MAX: usize = 4;
pub(crate) const STATUS_MAX_CHARS: usize = 200;
pub(crate) const ASK_TIMEOUT_MIN_SECS: u32 = 10;
pub(crate) const ASK_TIMEOUT_MAX_SECS: u32 = 3_600;
pub(crate) const ASK_TIMEOUT_DEFAULT_SECS: u32 = 600;
/// Resposta escrita pelo utilizador a uma pergunta (botao «Responder»).
pub(crate) const ANSWER_MAX_CHARS: usize = 4_000;
/// Maior id que o JSON de um cliente em JavaScript representa sem perder
/// precisao (2^53 - 1).
pub(crate) const MAX_SAFE_ID: u64 = (1 << 53) - 1;
pub(crate) const AGENT_NAME_MAX: usize = 32;

/// Os campos que cada ferramenta aceita; o resto e recusado.
const SEND_MESSAGE_KEYS: &[&str] = &["text", "title"];
const ASK_USER_KEYS: &[&str] = &["question", "options", "allow_text", "timeout_seconds"];
const GET_USER_MESSAGES_KEYS: &[&str] = &["since_id"];
const SET_STATUS_KEYS: &[&str] = &["text", "progress"];

fn accepted_keys(name: &str) -> &'static [&'static str] {
    match name {
        TOOL_SEND_MESSAGE => SEND_MESSAGE_KEYS,
        TOOL_ASK_USER => ASK_USER_KEYS,
        TOOL_GET_USER_MESSAGES => GET_USER_MESSAGES_KEYS,
        TOOL_SET_STATUS => SET_STATUS_KEYS,
        _ => &[],
    }
}

/// Um pedido de ferramenta ja validado e limpo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolCall {
    SendMessage {
        text: String,
        title: Option<String>,
    },
    AskUser {
        question: String,
        /// Nunca vazio: sem opcoes do agente, e `["Sim", "Não"]`.
        options: Vec<String>,
        allow_text: bool,
        timeout_seconds: u32,
    },
    GetUserMessages {
        since_id: Option<u64>,
    },
    SetStatus {
        text: String,
        progress: Option<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolError {
    /// Nome que nao existe: erro de protocolo (-32602), nao de execucao.
    UnknownTool(String),
    /// Argumentos invalidos: resultado com `isError`, para o modelo corrigir.
    Invalid(String),
}

impl ToolCall {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::SendMessage { .. } => TOOL_SEND_MESSAGE,
            Self::AskUser { .. } => TOOL_ASK_USER,
            Self::GetUserMessages { .. } => TOOL_GET_USER_MESSAGES,
            Self::SetStatus { .. } => TOOL_SET_STATUS,
        }
    }

    /// Os argumentos normalizados, tal como a ponte os entrega ao hub. O hub
    /// volta a passa-los por `parse_tool_call`.
    pub(crate) fn to_args(&self) -> Value {
        match self {
            Self::SendMessage { text, title } => {
                let mut args = json!({ "text": text });
                if let Some(title) = title {
                    args["title"] = json!(title);
                }
                args
            }
            Self::AskUser {
                question,
                options,
                allow_text,
                timeout_seconds,
            } => json!({
                "question": question,
                "options": options,
                "allow_text": allow_text,
                "timeout_seconds": timeout_seconds,
            }),
            Self::GetUserMessages { since_id } => match since_id {
                Some(id) => json!({ "since_id": id }),
                None => json!({}),
            },
            Self::SetStatus { text, progress } => json!({
                "text": text,
                "progress": progress,
            }),
        }
    }
}

/// Tira o que nao e texto escrito: caracteres de controlo (fica a quebra de
/// linha e o tab; `\r\n` vira `\n`) e os controlos bidirecionais, que deixam
/// uma mensagem mostrar no ecra uma ordem diferente da que tem. Corta os
/// espacos das pontas.
pub(crate) fn clean_text(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|&c| {
            if c == '\n' || c == '\t' {
                return true;
            }
            !(c.is_control() || is_bidi_control(c))
        })
        .collect();
    cleaned.trim().to_string()
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

/// Texto limpo com o tamanho entre `min` e `max` chars.
pub(crate) fn bounded_text(
    raw: &str,
    field: &str,
    min: usize,
    max: usize,
) -> Result<String, String> {
    let text = clean_text(raw);
    let count = text.chars().count();
    if count < min {
        return Err(if min <= 1 {
            format!("«{field}» não pode ficar vazio. / «{field}» must not be empty.")
        } else {
            format!("«{field}» precisa de pelo menos {min} caracteres.")
        });
    }
    if count > max {
        return Err(format!(
            "«{field}» tem {count} caracteres; o máximo é {max}. / «{field}» has {count} characters; the limit is {max}."
        ));
    }
    Ok(text)
}

/// Valida o pedido de uma ferramenta. `args` ausente vale como `{}`.
pub(crate) fn parse_tool_call(name: &str, args: Option<&Value>) -> Result<ToolCall, ToolError> {
    if !TOOL_NAMES.contains(&name) {
        return Err(ToolError::UnknownTool(name.to_string()));
    }
    let empty = Map::new();
    let args = match args {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(map)) => map,
        Some(_) => {
            return Err(ToolError::Invalid(
                "Os argumentos têm de ser um objeto JSON. / Arguments must be a JSON object."
                    .into(),
            ));
        }
    };
    parse_known(name, args).map_err(ToolError::Invalid)
}

fn parse_known(name: &str, args: &Map<String, Value>) -> Result<ToolCall, String> {
    only_keys(args, accepted_keys(name))?;
    match name {
        TOOL_SEND_MESSAGE => {
            let text = required_string(args, "text")?;
            let text = bounded_text(text, "text", 1, MESSAGE_MAX_CHARS)?;
            let title = match args.get("title") {
                None | Some(Value::Null) => None,
                Some(Value::String(raw)) => {
                    let title = bounded_text(raw, "title", 0, TITLE_MAX_CHARS)?;
                    (!title.is_empty()).then_some(title)
                }
                Some(_) => return Err(type_error("title", "string")),
            };
            Ok(ToolCall::SendMessage { text, title })
        }
        TOOL_ASK_USER => {
            let question = required_string(args, "question")?;
            let question = bounded_text(question, "question", 1, QUESTION_MAX_CHARS)?;
            let options = match args.get("options") {
                None | Some(Value::Null) => vec!["Sim".to_string(), "Não".to_string()],
                Some(Value::Array(items)) => parse_options(items)?,
                Some(_) => return Err(type_error("options", "array of strings")),
            };
            let allow_text = match args.get("allow_text") {
                None | Some(Value::Null) => true,
                Some(Value::Bool(value)) => *value,
                Some(_) => return Err(type_error("allow_text", "boolean")),
            };
            let timeout_seconds = match args.get("timeout_seconds") {
                None | Some(Value::Null) => ASK_TIMEOUT_DEFAULT_SECS,
                Some(value) => {
                    let secs =
                        integer(value).ok_or_else(|| type_error("timeout_seconds", "integer"))?;
                    if !(u64::from(ASK_TIMEOUT_MIN_SECS)..=u64::from(ASK_TIMEOUT_MAX_SECS))
                        .contains(&secs)
                    {
                        return Err(format!(
                            "«timeout_seconds» tem de estar entre {ASK_TIMEOUT_MIN_SECS} e {ASK_TIMEOUT_MAX_SECS}. / must be between {ASK_TIMEOUT_MIN_SECS} and {ASK_TIMEOUT_MAX_SECS}."
                        ));
                    }
                    secs as u32
                }
            };
            Ok(ToolCall::AskUser {
                question,
                options,
                allow_text,
                timeout_seconds,
            })
        }
        TOOL_GET_USER_MESSAGES => {
            let since_id = match args.get("since_id") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let id = integer(value).ok_or_else(|| type_error("since_id", "integer"))?;
                    if id > MAX_SAFE_ID {
                        return Err("«since_id» é grande demais. / «since_id» is too large.".into());
                    }
                    Some(id)
                }
            };
            Ok(ToolCall::GetUserMessages { since_id })
        }
        TOOL_SET_STATUS => {
            let text = required_string(args, "text")?;
            let text = bounded_text(text, "text", 0, STATUS_MAX_CHARS)?;
            // Uma linha de estado e uma linha: quebras e tabs viram espacos.
            let text = text
                .chars()
                .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
                .collect::<String>();
            let progress = match args.get("progress") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let pct =
                        integer(value).ok_or_else(|| type_error("progress", "integer or null"))?;
                    if pct > 100 {
                        return Err(
                            "«progress» tem de estar entre 0 e 100. / «progress» must be 0..100."
                                .into(),
                        );
                    }
                    Some(pct as u8)
                }
            };
            Ok(ToolCall::SetStatus { text, progress })
        }
        _ => Err("ferramenta desconhecida".into()),
    }
}

fn parse_options(items: &[Value]) -> Result<Vec<String>, String> {
    if items.is_empty() || items.len() > OPTIONS_MAX {
        return Err(format!(
            "«options» precisa de 1 a {OPTIONS_MAX} opções. / «options» needs 1 to {OPTIONS_MAX} entries."
        ));
    }
    let mut options: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let raw = item
            .as_str()
            .ok_or_else(|| type_error("options[]", "string"))?;
        // Um botao e uma linha.
        let option = bounded_text(raw, "options[]", 1, OPTION_MAX_CHARS)?
            .chars()
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .collect::<String>();
        if options
            .iter()
            .any(|seen| seen.to_lowercase() == option.to_lowercase())
        {
            return Err(format!(
                "Opção repetida: «{option}». / Duplicate option: «{option}»."
            ));
        }
        options.push(option);
    }
    Ok(options)
}

fn only_keys(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match args.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => {
            let shown: String = key.chars().take(40).collect();
            Err(format!(
                "Campo desconhecido: «{shown}». Aceites: {}. / Unknown field «{shown}».",
                allowed.join(", ")
            ))
        }
        None => Ok(()),
    }
}

fn required_string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    match args.get(key) {
        Some(Value::String(value)) => Ok(value),
        None | Some(Value::Null) => Err(format!(
            "Falta o campo obrigatório «{key}». / Missing required field «{key}»."
        )),
        Some(_) => Err(type_error(key, "string")),
    }
}

fn type_error(key: &str, expected: &str) -> String {
    format!("«{key}» tem de ser {expected}. / «{key}» must be {expected}.")
}

/// Inteiro nao negativo. Aceita `50.0` (em JSON Schema, e um inteiro).
fn integer(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    let f = value.as_f64()?;
    (f >= 0.0 && f.fract() == 0.0 && f <= MAX_SAFE_ID as f64).then_some(f as u64)
}

/// O `tools/list`: nomes, titulos, descricoes (pt-BR + en) e os JSON Schema
/// (2020-12, o dialeto por omissao do MCP). Os limites daqui sao os mesmos
/// de `parse_tool_call`; o teste `tool_schemas_match_the_validator` prende-os.
pub(crate) fn tool_definitions() -> Value {
    json!([
        {
            "name": TOOL_SEND_MESSAGE,
            "title": "Mandar mensagem ao usuário",
            "description": "Mostra uma mensagem ao usuário no painel «Agentes» do NeuralIA, com um aviso na tela. Use para contar o que fez ou o que encontrou. / Shows a message to the user in NeuralIA's «Agentes» panel, with an on-screen notice. Use it to report progress or results.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MESSAGE_MAX_CHARS,
                        "description": "A mensagem (texto simples, até 4000 caracteres). / The message (plain text, up to 4000 characters)."
                    },
                    "title": {
                        "type": "string",
                        "maxLength": TITLE_MAX_CHARS,
                        "description": "Título curto opcional. / Optional short title."
                    }
                },
                "required": ["text"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false
            }
        },
        {
            "name": TOOL_ASK_USER,
            "title": "Perguntar ao usuário",
            "description": "Faz uma pergunta ao usuário e ESPERA a resposta: aparece um cartão com botões (por omissão [Sim] [Não] e [Responder] para escrever). Devolve {answer, via: \"button\"|\"text\"} ou {timed_out: true}. / Asks the user a question and WAITS for the answer. Returns {answer, via} or {timed_out: true}. If your client has a short tool timeout, pass a smaller timeout_seconds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": QUESTION_MAX_CHARS,
                        "description": "A pergunta. / The question."
                    },
                    "options": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": OPTIONS_MAX,
                        "items": { "type": "string", "minLength": 1, "maxLength": OPTION_MAX_CHARS },
                        "description": "Os botões (1 a 4). Sem isto: [Sim] [Não]. / Button labels (1 to 4). Default: Yes/No in Portuguese."
                    },
                    "allow_text": {
                        "type": "boolean",
                        "default": true,
                        "description": "Deixa o usuário escrever uma resposta livre. / Lets the user type a free-form answer."
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "minimum": ASK_TIMEOUT_MIN_SECS,
                        "maximum": ASK_TIMEOUT_MAX_SECS,
                        "default": ASK_TIMEOUT_DEFAULT_SECS,
                        "description": "Quanto esperar pela resposta. / How long to wait for the answer."
                    }
                },
                "required": ["question"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false
            }
        },
        {
            "name": TOOL_GET_USER_MESSAGES,
            "title": "Ler mensagens do usuário",
            "description": "Lê o que o usuário escreveu PARA ESTE AGENTE no painel «Agentes» (e o que ele partilhou de propósito com os botões «esta página» / «estas abas»). Nunca inclui o que ele escreve nas páginas web. Passe since_id = last_id da chamada anterior. / Reads what the user wrote to THIS agent in the «Agentes» panel, plus context they explicitly shared. Never includes what they type in web pages. Pass since_id = last_id from the previous call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Só mensagens com id maior do que este. / Only messages with a greater id."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": TOOL_SET_STATUS,
            "title": "Mostrar o que está fazendo",
            "description": "Atualiza a linha de estado deste agente no painel (ex.: «a trabalhar… 60%»). Texto vazio e progress null limpam a linha. / Updates this agent's status line in the panel. Empty text and null progress clear it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "maxLength": STATUS_MAX_CHARS,
                        "description": "O estado, numa linha. / One-line status."
                    },
                    "progress": {
                        "type": ["integer", "null"],
                        "minimum": 0,
                        "maximum": 100,
                        "description": "Percentagem (0 a 100) ou null. / Percent (0 to 100) or null."
                    }
                },
                "required": ["text"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }
    ])
}

/// Nome de um agente como vive no disco e no canal: so `[a-z0-9-]`, ate 32
/// caracteres. Maiusculas descem, espacos/`_`/`.` viram `-`, o resto cai.
/// `None` quando nao sobra nada, ou quando sobra um nome de dispositivo do
/// Windows (`con`, `nul`, `com1`...), que nao pode ser nome de ficheiro.
pub(crate) fn sanitize_agent_name(raw: &str) -> Option<String> {
    let mut name = String::with_capacity(AGENT_NAME_MAX);
    for c in raw.trim().chars() {
        let mapped = match c {
            'a'..='z' | '0'..='9' => Some(c),
            'A'..='Z' => Some(c.to_ascii_lowercase()),
            '-' | '_' | ' ' | '.' => Some('-'),
            _ => None,
        };
        if let Some(c) = mapped {
            if c == '-' && (name.is_empty() || name.ends_with('-')) {
                continue;
            }
            name.push(c);
        }
        if name.len() >= AGENT_NAME_MAX {
            break;
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    if name.is_empty() || is_windows_device_name(&name) {
        return None;
    }
    Some(name)
}

fn is_windows_device_name(name: &str) -> bool {
    if matches!(name, "con" | "prn" | "aux" | "nul") {
        return true;
    }
    let bytes = name.as_bytes();
    bytes.len() == 4
        && (name.starts_with("com") || name.starts_with("lpt"))
        && bytes[3].is_ascii_digit()
}

/// O nome que o painel mostra.
pub(crate) fn agent_display_name(name: &str) -> String {
    match name {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "gemini" => "Gemini".into(),
        other => {
            let spaced = other.replace('-', " ");
            let mut chars = spaced.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(name: &str, args: Value) -> ToolCall {
        parse_tool_call(name, Some(&args)).unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    fn invalid(name: &str, args: Value) -> String {
        match parse_tool_call(name, Some(&args)) {
            Err(ToolError::Invalid(message)) => message,
            other => panic!("{name} {args}: expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn tool_inputs_reject_unknown_fields_wrong_types_and_oversize() {
        // Campos a mais: nunca ignorados.
        assert!(invalid("send_message", json!({"text":"oi","html":"<b>"})).contains("html"));
        assert!(invalid("ask_user", json!({"question":"?","extra":1})).contains("extra"));
        assert!(invalid("get_user_messages", json!({"since":1})).contains("since"));
        assert!(invalid("set_status", json!({"text":"x","color":"red"})).contains("color"));
        // Tipos errados.
        invalid("send_message", json!({"text": 5}));
        invalid("ask_user", json!({"question":"?","allow_text":"sim"}));
        invalid("ask_user", json!({"question":"?","options":"Sim"}));
        invalid("set_status", json!({"text":"x","progress":"50"}));
        assert!(matches!(
            parse_tool_call("send_message", Some(&json!(["text"]))),
            Err(ToolError::Invalid(_))
        ));
        // Tamanhos: o limite passa, um a mais nao.
        ok(
            "send_message",
            json!({"text": "a".repeat(MESSAGE_MAX_CHARS)}),
        );
        invalid(
            "send_message",
            json!({"text": "a".repeat(MESSAGE_MAX_CHARS + 1)}),
        );
        // Conta caracteres, nao bytes.
        ok(
            "send_message",
            json!({"text": "é".repeat(MESSAGE_MAX_CHARS)}),
        );
        invalid("send_message", json!({"text": "   \n\t "}));
        invalid(
            "send_message",
            json!({"text":"x","title":"t".repeat(TITLE_MAX_CHARS + 1)}),
        );
        ok(
            "ask_user",
            json!({"question": "q".repeat(QUESTION_MAX_CHARS)}),
        );
        invalid(
            "ask_user",
            json!({"question": "q".repeat(QUESTION_MAX_CHARS + 1)}),
        );
        invalid("ask_user", json!({"question":"?","options":[]}));
        invalid(
            "ask_user",
            json!({"question":"?","options":["a","b","c","d","e"]}),
        );
        invalid(
            "ask_user",
            json!({"question":"?","options":["o".repeat(OPTION_MAX_CHARS + 1)]}),
        );
        invalid("ask_user", json!({"question":"?","options":["Sim","sim"]}));
        invalid("ask_user", json!({"question":"?","options":[" "]}));
        invalid("ask_user", json!({"question":"?","timeout_seconds": 9}));
        invalid("ask_user", json!({"question":"?","timeout_seconds": 3601}));
        invalid("ask_user", json!({"question":"?","timeout_seconds": 10.5}));
        ok("set_status", json!({"text": "s".repeat(STATUS_MAX_CHARS)}));
        invalid(
            "set_status",
            json!({"text": "s".repeat(STATUS_MAX_CHARS + 1)}),
        );
        invalid("set_status", json!({"text":"x","progress": 101}));
        invalid("set_status", json!({"text":"x","progress": -1}));
        invalid("get_user_messages", json!({"since_id": MAX_SAFE_ID + 1}));
        invalid("get_user_messages", json!({"since_id": -3}));
        assert_eq!(
            parse_tool_call("delete_everything", Some(&json!({}))),
            Err(ToolError::UnknownTool("delete_everything".into()))
        );
    }

    #[test]
    fn tool_inputs_are_normalised_and_control_characters_stripped() {
        assert_eq!(
            ok(
                "send_message",
                json!({"text":"  linha 1\r\nlinha\u{0007} 2\t!\u{202E}  ","title":"  "})
            ),
            ToolCall::SendMessage {
                text: "linha 1\nlinha 2\t!".into(),
                title: None
            }
        );
        assert_eq!(
            ok("ask_user", json!({"question":"Posso?"})),
            ToolCall::AskUser {
                question: "Posso?".into(),
                options: vec!["Sim".into(), "Não".into()],
                allow_text: true,
                timeout_seconds: ASK_TIMEOUT_DEFAULT_SECS,
            }
        );
        assert_eq!(
            ok(
                "ask_user",
                json!({"question":"Qual?","options":["A\nB","C"],"allow_text":false,"timeout_seconds":10})
            ),
            ToolCall::AskUser {
                question: "Qual?".into(),
                options: vec!["A B".into(), "C".into()],
                allow_text: false,
                timeout_seconds: 10,
            }
        );
        assert_eq!(
            ok("set_status", json!({"text":"a\nb","progress":60.0})),
            ToolCall::SetStatus {
                text: "a b".into(),
                progress: Some(60)
            }
        );
        assert_eq!(
            ok("set_status", json!({"text":"","progress":null})),
            ToolCall::SetStatus {
                text: String::new(),
                progress: None
            }
        );
        assert_eq!(
            parse_tool_call("get_user_messages", None),
            Ok(ToolCall::GetUserMessages { since_id: None })
        );
        // O que a ponte entrega ao hub volta a dar o mesmo pedido.
        for call in [
            ok("send_message", json!({"text":"x","title":"T"})),
            ok(
                "ask_user",
                json!({"question":"q","options":["1"],"allow_text":false}),
            ),
            ok("get_user_messages", json!({"since_id": 7})),
            ok("set_status", json!({"text":"t","progress":3})),
        ] {
            assert_eq!(
                parse_tool_call(call.name(), Some(&call.to_args())),
                Ok(call.clone())
            );
        }
    }

    #[test]
    fn tool_schemas_match_the_validator() {
        let tools = tool_definitions();
        let tools = tools.as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        for tool in tools {
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], false, "{}", tool["name"]);
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains(" / "), "pt-BR + en: {}", tool["name"]);
            // As propriedades publicadas sao exatamente os campos aceites.
            let name = tool["name"].as_str().unwrap();
            let mut published: Vec<&str> = schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let mut accepted = accepted_keys(name).to_vec();
            published.sort_unstable();
            accepted.sort_unstable();
            assert_eq!(published, accepted, "{name}");
            for required in schema["required"].as_array().into_iter().flatten() {
                let required = required.as_str().unwrap();
                let message = invalid(name, json!({}));
                assert!(message.contains(required), "{message}");
            }
        }
        let limit = |tool: usize, path: &[&str]| {
            let mut value = &tools[tool]["inputSchema"]["properties"];
            for part in path {
                value = &value[*part];
            }
            value.as_u64().unwrap() as usize
        };
        assert_eq!(limit(0, &["text", "maxLength"]), MESSAGE_MAX_CHARS);
        assert_eq!(limit(0, &["title", "maxLength"]), TITLE_MAX_CHARS);
        assert_eq!(limit(1, &["question", "maxLength"]), QUESTION_MAX_CHARS);
        assert_eq!(limit(1, &["options", "maxItems"]), OPTIONS_MAX);
        assert_eq!(
            limit(1, &["options", "items", "maxLength"]),
            OPTION_MAX_CHARS
        );
        assert_eq!(
            limit(1, &["timeout_seconds", "minimum"]),
            ASK_TIMEOUT_MIN_SECS as usize
        );
        assert_eq!(
            limit(1, &["timeout_seconds", "maximum"]),
            ASK_TIMEOUT_MAX_SECS as usize
        );
        assert_eq!(limit(3, &["text", "maxLength"]), STATUS_MAX_CHARS);
        assert_eq!(limit(3, &["progress", "maximum"]), 100);
    }

    #[test]
    fn agent_names_are_sanitised_to_a_safe_file_stem() {
        assert_eq!(sanitize_agent_name("claude").as_deref(), Some("claude"));
        assert_eq!(
            sanitize_agent_name(" Claude Code ").as_deref(),
            Some("claude-code")
        );
        assert_eq!(
            sanitize_agent_name("my_agent.v2").as_deref(),
            Some("my-agent-v2")
        );
        assert_eq!(sanitize_agent_name("--a--b--").as_deref(), Some("a-b"));
        assert_eq!(sanitize_agent_name("../../etc").as_deref(), Some("etc"));
        assert_eq!(sanitize_agent_name("C:\\x\\y").as_deref(), Some("cxy"));
        assert_eq!(sanitize_agent_name("ágent").as_deref(), Some("gent"));
        assert_eq!(
            sanitize_agent_name(&"x".repeat(80)).map(|n| n.len()),
            Some(32)
        );
        assert_eq!(sanitize_agent_name("é€"), None);
        assert_eq!(sanitize_agent_name(""), None);
        for device in ["con", "NUL", "aux", "prn", "com1", "LPT9"] {
            assert_eq!(sanitize_agent_name(device), None, "{device}");
        }
        assert_eq!(sanitize_agent_name("com10").as_deref(), Some("com10"));
        assert_eq!(agent_display_name("claude"), "Claude");
        assert_eq!(agent_display_name("codex"), "Codex");
        assert_eq!(agent_display_name("gemini"), "Gemini");
        assert_eq!(agent_display_name("my-bot"), "My bot");
    }
}
