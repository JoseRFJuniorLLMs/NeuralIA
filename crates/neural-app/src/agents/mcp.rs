//! A ponte: servidor MCP em stdio para Claude Code, Codex CLI e Gemini CLI.
//!
//! Transporte stdio do MCP: uma mensagem JSON-RPC 2.0 por linha, em UTF-8,
//! sem quebras de linha dentro; no stdout so sai protocolo (os registos vao
//! para o stderr); fecha-se quando o stdin fecha.
//!
//! Serve as duas eras do protocolo (spec 2026-07-28, «Versioning»):
//!
//! - **legada** (2025-11-25 e anteriores): `initialize` negocia a versao,
//!   `notifications/initialized`, depois `tools/list` e `tools/call`;
//! - **moderna** (2026-07-28): cada pedido traz no `_meta` a versao
//!   (`io.modelcontextprotocol/protocolVersion`) e as capacidades do cliente;
//!   `server/discover` descreve o servidor; os resultados levam
//!   `resultType: "complete"`; versao desconhecida da -32022 com a lista.
//!
//! As ferramentas (`tools`) validam os argumentos aqui e outra vez no hub.
//! Argumentos invalidos voltam como resultado com `isError: true` (o modelo
//! pode corrigir); ferramenta desconhecida e erro de protocolo (-32602).
//! `ask_user` espera em fatias curtas (`wait`), para a ligacao ao hub ficar
//! livre para as outras chamadas, manda `notifications/progress` quando o
//! cliente deu um `progressToken`, e desiste com `notifications/cancelled`.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::tools::{ToolCall, ToolError, parse_tool_call, tool_definitions};
use super::{Line, read_line_capped};

/// A versao moderna (sem `initialize`).
pub(crate) const MODERN_VERSION: &str = "2026-07-28";
/// As versoes legadas (com `initialize`), da mais recente para a mais antiga.
pub(crate) const LEGACY_VERSIONS: [&str; 4] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// Tecto de uma mensagem do cliente.
pub(crate) const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// `tools/call` em curso ao mesmo tempo.
pub(crate) const MAX_INFLIGHT_CALLS: usize = 8;

pub(crate) const PARSE_ERROR: i64 = -32_700;
pub(crate) const INVALID_REQUEST: i64 = -32_600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32_601;
pub(crate) const INVALID_PARAMS: i64 = -32_602;
pub(crate) const INTERNAL_ERROR: i64 = -32_603;
pub(crate) const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32_022;

const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// O erro que o modelo ve quando o NeuralIA nao esta aberto.
pub(crate) const NOT_RUNNING: &str = "Abra o NeuralIA para falar com o usuário. (O NeuralIA não está aberto neste computador.) / Open NeuralIA to talk to the user: it is not running.";

pub(crate) const INSTRUCTIONS: &str = "O NeuralIA liga você ao usuário que está no navegador NeuralIA. send_message mostra uma mensagem no painel «Agentes»; ask_user faz uma pergunta com botões e espera a resposta; get_user_messages lê o que o usuário escreveu para você no painel (guarde o last_id e passe-o como since_id); set_status mostra o que você está fazendo. Você nunca vê o que o usuário digita nas páginas web: só o que ele escreve para você ou partilha de propósito. / NeuralIA connects you to the user of the NeuralIA browser. send_message shows a message in the «Agentes» panel; ask_user asks a question with buttons and waits for the answer; get_user_messages reads what the user wrote to you (keep last_id and pass it as since_id); set_status shows what you are doing. You never see what the user types in web pages, only what they write to you or explicitly share.";

/// Uma falha a falar com o hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkError {
    /// Nao ha NeuralIA aberto (sem ficheiros, sem canal).
    NotRunning(String),
    /// O canal existe mas recusou (token, dono do canal, limite de agentes).
    Rejected(String),
    /// A ligacao caiu a meio.
    Broken(String),
}

/// A ligacao da ponte ao hub. O `pipe::PipeLink` e a que embarca.
pub(crate) trait HubLink: Send {
    /// Um pedido, uma resposta. Liga-se primeiro, se preciso.
    fn exchange(&mut self, request: &Value) -> Result<Value, LinkError>;
    fn ensure_connected(&mut self) -> Result<(), LinkError>;
    fn is_connected(&self) -> bool;
    fn close(&mut self);
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ServeOptions {
    /// Liga-se ao hub logo no arranque e mantem a ligacao viva (o painel
    /// mostra o agente como ligado enquanto o CLI esta aberto).
    pub(crate) keepalive: bool,
    /// Intervalo das `notifications/progress` durante um `ask_user`.
    pub(crate) progress_every: Duration,
    /// Quanto cada `wait` segura a ligacao ao hub.
    pub(crate) wait_slice: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            keepalive: true,
            progress_every: Duration::from_secs(10),
            wait_slice: Duration::from_millis(1_000),
        }
    }
}

const KEEPALIVE_EVERY: Duration = Duration::from_secs(5);
const PING_WHEN_IDLE: Duration = Duration::from_secs(15);
/// A ponte da ao hub esta folga sobre o prazo da pergunta antes de desistir
/// por conta propria.
const ASK_GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
struct Ctx {
    modern: bool,
    /// `structuredContent` existe a partir de 2025-06-18.
    structured: bool,
}

enum CallError {
    Message(String),
    /// Cancelado pelo cliente ou pelo fim do stdin: nao se responde.
    Cancelled,
}

struct Bridge {
    out: Mutex<Box<dyn Write + Send>>,
    link: Mutex<Box<dyn HubLink>>,
    last_hub_use: Mutex<Instant>,
    /// Versao legada negociada no `initialize`.
    negotiated: Mutex<Option<&'static str>>,
    inflight: Mutex<HashMap<String, Arc<AtomicBool>>>,
    shutdown: AtomicBool,
    output_closed: AtomicBool,
    options: ServeOptions,
}

fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Corre o servidor MCP ate o `input` acabar. `agent` so serve para os
/// registos: quem o apresenta ao hub e o `link`.
pub(crate) fn serve(
    agent: &str,
    mut input: impl BufRead,
    output: Box<dyn Write + Send>,
    link: Box<dyn HubLink>,
    options: ServeOptions,
) -> io::Result<()> {
    let bridge = Arc::new(Bridge {
        out: Mutex::new(output),
        link: Mutex::new(link),
        last_hub_use: Mutex::new(Instant::now()),
        negotiated: Mutex::new(None),
        inflight: Mutex::new(HashMap::new()),
        shutdown: AtomicBool::new(false),
        output_closed: AtomicBool::new(false),
        options,
    });
    let keepalive = if options.keepalive {
        let bridge = Arc::clone(&bridge);
        thread::Builder::new()
            .name("neuralia-mcp-keepalive".into())
            .spawn(move || bridge.keepalive())
            .ok()
    } else {
        None
    };
    eprintln!("[neuralia-mcp] ponte do agente «{agent}» pronta");

    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    let result = loop {
        match read_line_capped(&mut input, MAX_MESSAGE_BYTES) {
            Ok(None) => break Ok(()),
            Ok(Some(Line::TooLong)) => bridge.send_error(
                Value::Null,
                PARSE_ERROR,
                "Parse error: message larger than 1 MiB",
            ),
            Ok(Some(Line::Text(bytes))) => bridge.handle(&bytes, &mut workers),
            Err(error) => break Err(error),
        }
        workers.retain(|worker| !worker.is_finished());
        if bridge.output_closed.load(Ordering::SeqCst) {
            break Ok(());
        }
    };

    // Fim do stdin: as perguntas pendentes sao canceladas no hub (o cartao
    // sai do ecra) e a ponte sai depressa, como o transporte pede.
    bridge.shutdown.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(5);
    while workers.iter().any(|worker| !worker.is_finished()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    for worker in workers {
        if worker.is_finished() {
            let _ = worker.join();
        }
    }
    if let Some(keepalive) = keepalive {
        let _ = keepalive.join();
    }
    lock(&bridge.link).close();
    result
}

impl Bridge {
    fn handle(self: &Arc<Self>, bytes: &[u8], workers: &mut Vec<JoinHandle<()>>) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return self.send_error(Value::Null, PARSE_ERROR, "Parse error: not UTF-8");
        };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return self.send_error(Value::Null, PARSE_ERROR, "Parse error");
        };
        let Value::Object(message) = message else {
            return self.send_error(
                Value::Null,
                INVALID_REQUEST,
                "Invalid Request: one JSON-RPC object per line (batches are not supported)",
            );
        };
        let id = message.get("id");
        let valid_id = id.filter(|id| id.is_string() || id.is_i64() || id.is_u64());
        let reply_id = valid_id.cloned().unwrap_or(Value::Null);
        let Some(method) = message.get("method") else {
            // Uma resposta do cliente: o servidor nunca faz pedidos, ignora-se.
            if message.contains_key("result") || message.contains_key("error") {
                return;
            }
            return self.send_error(reply_id, INVALID_REQUEST, "Invalid Request");
        };
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            if id.is_none() {
                return;
            }
            return self.send_error(
                reply_id,
                INVALID_REQUEST,
                "Invalid Request: jsonrpc must be \"2.0\"",
            );
        }
        let Some(method) = method.as_str() else {
            if id.is_none() {
                return;
            }
            return self.send_error(reply_id, INVALID_REQUEST, "Invalid Request: bad method");
        };
        let params = match message.get("params") {
            None => None,
            Some(Value::Object(params)) => Some(params),
            Some(_) => {
                if id.is_none() {
                    return;
                }
                return self.send_error(reply_id, INVALID_PARAMS, "params must be an object");
            }
        };
        match (id, valid_id) {
            (None, _) => self.notification(method, params),
            (Some(_), None) => self.send_error(
                Value::Null,
                INVALID_REQUEST,
                "Invalid Request: id must be a string or an integer",
            ),
            (Some(_), Some(id)) => self.request(id.clone(), method, params, workers),
        }
    }

    fn notification(&self, method: &str, params: Option<&Map<String, Value>>) {
        if method == "notifications/cancelled"
            && let Some(request) = params.and_then(|p| p.get("requestId"))
            && let Some(flag) = lock(&self.inflight).get(&request.to_string())
        {
            flag.store(true, Ordering::SeqCst);
        }
        // `notifications/initialized` e o resto: nada a fazer.
    }

    fn request(
        self: &Arc<Self>,
        id: Value,
        method: &str,
        params: Option<&Map<String, Value>>,
        workers: &mut Vec<JoinHandle<()>>,
    ) {
        let meta = params
            .and_then(|p| p.get("_meta"))
            .and_then(Value::as_object);
        let ctx = if let Some(version) = meta.and_then(|m| m.get(META_PROTOCOL_VERSION)) {
            let Some(version) = version.as_str() else {
                return self.send_error(
                    id,
                    INVALID_PARAMS,
                    "_meta protocolVersion must be a string",
                );
            };
            if version != MODERN_VERSION {
                return self.send_error_data(
                    id,
                    UNSUPPORTED_PROTOCOL_VERSION,
                    "Unsupported protocol version",
                    json!({"supported": supported_versions(), "requested": version}),
                );
            }
            if !meta
                .and_then(|m| m.get(META_CLIENT_CAPABILITIES))
                .is_some_and(Value::is_object)
            {
                return self.send_error(
                    id,
                    INVALID_PARAMS,
                    "Missing required _meta field io.modelcontextprotocol/clientCapabilities",
                );
            }
            Ctx {
                modern: true,
                structured: true,
            }
        } else {
            match method {
                "initialize" => return self.initialize(id, params),
                "ping" => {
                    return self.send_result(
                        id,
                        json!({}),
                        Ctx {
                            modern: false,
                            structured: false,
                        },
                    );
                }
                "server/discover" => {
                    return self.send_error(
                        id,
                        INVALID_PARAMS,
                        "Missing required _meta field io.modelcontextprotocol/protocolVersion",
                    );
                }
                _ => {}
            }
            let Some(version) = *lock(&self.negotiated) else {
                return self.send_error(
                    id,
                    INVALID_REQUEST,
                    "Servidor não inicializado: envie initialize primeiro. / Server not initialized: send initialize first.",
                );
            };
            Ctx {
                modern: false,
                structured: version >= "2025-06-18",
            }
        };
        match method {
            "ping" => self.send_result(id, json!({}), ctx),
            "server/discover" => self.send_result(
                id,
                json!({
                    "supportedVersions": supported_versions(),
                    "capabilities": {"tools": {"listChanged": false}},
                    "instructions": INSTRUCTIONS,
                }),
                ctx,
            ),
            "tools/list" => self.send_result(id, json!({"tools": tool_definitions()}), ctx),
            "tools/call" => self.tools_call(id, params, ctx, workers),
            "initialize" => self.send_error(
                id,
                INVALID_REQUEST,
                "initialize is not used with protocol 2026-07-28",
            ),
            _ => self.send_error(id, METHOD_NOT_FOUND, &format!("Method not found: {method}")),
        }
    }

    fn initialize(&self, id: Value, params: Option<&Map<String, Value>>) {
        let Some(requested) = params
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str)
        else {
            return self.send_error(
                id,
                INVALID_PARAMS,
                "initialize: protocolVersion is required",
            );
        };
        let version = {
            let mut negotiated = lock(&self.negotiated);
            if negotiated.is_some() {
                drop(negotiated);
                return self.send_error(id, INVALID_REQUEST, "Already initialized");
            }
            // A versao pedida, se a conhecemos; senao a mais recente das nossas.
            let version = LEGACY_VERSIONS
                .iter()
                .copied()
                .find(|known| *known == requested)
                .unwrap_or(LEGACY_VERSIONS[0]);
            *negotiated = Some(version);
            version
        };
        self.send_result(
            id,
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": server_info(),
                "instructions": INSTRUCTIONS,
            }),
            Ctx {
                modern: false,
                structured: false,
            },
        );
    }

    fn tools_call(
        self: &Arc<Self>,
        id: Value,
        params: Option<&Map<String, Value>>,
        ctx: Ctx,
        workers: &mut Vec<JoinHandle<()>>,
    ) {
        let Some(params) = params else {
            return self.send_error(id, INVALID_PARAMS, "tools/call: params are required");
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return self.send_error(id, INVALID_PARAMS, "tools/call: name is required");
        };
        let call = match parse_tool_call(name, params.get("arguments")) {
            Ok(call) => call,
            Err(ToolError::UnknownTool(name)) => {
                let shown: String = name.chars().take(64).collect();
                return self.send_error(id, INVALID_PARAMS, &format!("Unknown tool: {shown}"));
            }
            Err(ToolError::Invalid(message)) => {
                return self.send_result(id, tool_error(&message), ctx);
            }
        };
        let progress = params
            .get("_meta")
            .and_then(|meta| meta.get("progressToken"))
            .filter(|token| token.is_string() || token.is_i64() || token.is_u64())
            .cloned();
        let key = id.to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut inflight = lock(&self.inflight);
            if inflight.contains_key(&key) {
                drop(inflight);
                return self.send_error(id, INVALID_REQUEST, "Duplicate request id");
            }
            if inflight.len() >= MAX_INFLIGHT_CALLS {
                drop(inflight);
                return self.send_result(
                    id,
                    tool_error(
                        "Demasiadas chamadas ao mesmo tempo; espere que uma acabe. / Too many concurrent calls.",
                    ),
                    ctx,
                );
            }
            inflight.insert(key.clone(), Arc::clone(&cancel));
        }
        let bridge = Arc::clone(self);
        let worker_id = id.clone();
        let worker_key = key.clone();
        let spawned = thread::Builder::new()
            .name("neuralia-mcp-call".into())
            .spawn(move || bridge.run_call(worker_id, worker_key, call, progress, cancel, ctx));
        match spawned {
            Ok(worker) => workers.push(worker),
            Err(error) => {
                lock(&self.inflight).remove(&key);
                self.send_error(id, INTERNAL_ERROR, &format!("Internal error: {error}"));
            }
        }
    }

    fn run_call(
        &self,
        id: Value,
        key: String,
        call: ToolCall,
        progress: Option<Value>,
        cancel: Arc<AtomicBool>,
        ctx: Ctx,
    ) {
        let outcome = match &call {
            ToolCall::AskUser {
                timeout_seconds, ..
            } => self.ask(&call, *timeout_seconds, progress.as_ref(), &cancel),
            _ => self.hub_call(&call),
        };
        lock(&self.inflight).remove(&key);
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let result = match outcome {
            Ok(value) => tool_ok(value, ctx),
            Err(CallError::Message(message)) => tool_error(&message),
            Err(CallError::Cancelled) => return,
        };
        self.send_result(id, result, ctx);
    }

    fn hub_call(&self, call: &ToolCall) -> Result<Value, CallError> {
        self.hub_request(&json!({"t": "call", "tool": call.name(), "args": call.to_args()}))
    }

    fn hub_request(&self, request: &Value) -> Result<Value, CallError> {
        let mut link = lock(&self.link);
        *lock(&self.last_hub_use) = Instant::now();
        match link.exchange(request) {
            Ok(reply) => {
                if reply.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(reply.get("value").cloned().unwrap_or_else(|| json!({})))
                } else {
                    Err(CallError::Message(
                        reply
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("O NeuralIA recusou o pedido.")
                            .to_string(),
                    ))
                }
            }
            Err(LinkError::NotRunning(detail)) => {
                eprintln!("[neuralia-mcp] NeuralIA fechado: {detail}");
                Err(CallError::Message(NOT_RUNNING.into()))
            }
            Err(LinkError::Rejected(reason)) => Err(CallError::Message(format!(
                "O NeuralIA recusou a ligação: {reason} / NeuralIA refused the connection."
            ))),
            Err(LinkError::Broken(reason)) => Err(CallError::Message(format!(
                "A ligação ao NeuralIA caiu ({reason}). Tente de novo. / The connection to NeuralIA dropped; try again."
            ))),
        }
    }

    fn ask(
        &self,
        call: &ToolCall,
        timeout_seconds: u32,
        progress: Option<&Value>,
        cancel: &AtomicBool,
    ) -> Result<Value, CallError> {
        let registered = self.hub_call(call)?;
        let Some(question_id) = registered.get("question_id").and_then(Value::as_u64) else {
            return Err(CallError::Message("Resposta inválida do NeuralIA.".into()));
        };
        let started = Instant::now();
        let deadline = started + Duration::from_secs(u64::from(timeout_seconds)) + ASK_GRACE;
        let mut next_progress = started + self.options.progress_every;
        let wait_ms = u64::try_from(self.options.wait_slice.as_millis()).unwrap_or(1_000);
        loop {
            if cancel.load(Ordering::SeqCst) || self.shutdown.load(Ordering::SeqCst) {
                let _ = self.hub_request(&json!({"t": "cancel", "question_id": question_id}));
                return Err(CallError::Cancelled);
            }
            if Instant::now() >= deadline {
                let _ = self.hub_request(&json!({"t": "cancel", "question_id": question_id}));
                return Ok(json!({"timed_out": true}));
            }
            let state = self.hub_request(
                &json!({"t": "wait", "question_id": question_id, "wait_ms": wait_ms}),
            )?;
            match state.get("state").and_then(Value::as_str) {
                Some("pending") => {}
                Some("answered") => {
                    return Ok(json!({
                        "answer": state.get("answer").cloned().unwrap_or(Value::Null),
                        "via": state.get("via").cloned().unwrap_or(Value::Null),
                    }));
                }
                Some("timed_out") => return Ok(json!({"timed_out": true})),
                Some("dismissed") => return Ok(json!({"timed_out": false, "dismissed": true})),
                Some("cancelled") => {
                    return Err(CallError::Message("A pergunta foi cancelada.".into()));
                }
                _ => return Err(CallError::Message("Resposta inválida do NeuralIA.".into())),
            }
            if let Some(token) = progress
                && Instant::now() >= next_progress
                && !cancel.load(Ordering::SeqCst)
            {
                next_progress += self.options.progress_every;
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": {
                        "progressToken": token,
                        "progress": started.elapsed().as_millis() as u64,
                        "total": u64::from(timeout_seconds) * 1_000,
                        "message": "Aguardando a resposta do usuário… / Waiting for the user's answer…",
                    },
                }));
            }
        }
    }

    fn keepalive(&self) {
        let mut next = Instant::now();
        while !self.shutdown.load(Ordering::SeqCst) {
            if Instant::now() >= next {
                next = Instant::now() + KEEPALIVE_EVERY;
                let idle = lock(&self.last_hub_use).elapsed();
                let mut link = lock(&self.link);
                if !link.is_connected() {
                    let _ = link.ensure_connected();
                } else if idle >= PING_WHEN_IDLE && link.exchange(&json!({"t": "ping"})).is_ok() {
                    *lock(&self.last_hub_use) = Instant::now();
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn send(&self, message: &Value) {
        // serde_json escapa as quebras de linha: uma mensagem e uma linha.
        let mut line = message.to_string();
        line.push('\n');
        let mut out = lock(&self.out);
        if out
            .write_all(line.as_bytes())
            .and_then(|()| out.flush())
            .is_err()
        {
            self.output_closed.store(true, Ordering::SeqCst);
        }
    }

    fn send_result(&self, id: Value, mut result: Value, ctx: Ctx) {
        if ctx.modern {
            result["resultType"] = json!("complete");
            result["_meta"] = json!({ META_SERVER_INFO: server_info() });
        }
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn send_error(&self, id: Value, code: i64, message: &str) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }));
    }

    fn send_error_data(&self, id: Value, code: i64, message: &str, data: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message, "data": data},
        }));
    }
}

fn supported_versions() -> Vec<&'static str> {
    std::iter::once(MODERN_VERSION)
        .chain(LEGACY_VERSIONS)
        .collect()
}

fn server_info() -> Value {
    json!({
        "name": super::MCP_SERVER_NAME,
        "title": "NeuralIA",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn tool_ok(value: Value, ctx: Ctx) -> Value {
    let mut result = json!({
        "content": [{"type": "text", "text": value.to_string()}],
        "isError": false,
    });
    if ctx.structured {
        result["structuredContent"] = value;
    }
    result
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": message}],
        "isError": true,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::agents::hub::tests::{Fixture, TOKEN, fixture};
    use crate::agents::hub::{AgentEvent, HubSession, QuestionAnswer};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};

    /// A ligacao ao hub dentro do mesmo processo: a mesma `HubSession` que o
    /// canal do Windows usa, sem o canal.
    pub(crate) struct LocalLink {
        pub(crate) hub: crate::agents::AgentHub,
        pub(crate) agent: String,
        pub(crate) session: Option<HubSession>,
    }

    impl HubLink for LocalLink {
        fn exchange(&mut self, request: &Value) -> Result<Value, LinkError> {
            self.ensure_connected()?;
            let session = self.session.as_mut().expect("connected");
            let reply = session.handle_line(&request.to_string());
            if reply.close {
                self.session = None;
            }
            serde_json::from_str(&reply.line).map_err(|e| LinkError::Broken(e.to_string()))
        }

        fn ensure_connected(&mut self) -> Result<(), LinkError> {
            if self.session.is_some() {
                return Ok(());
            }
            let mut session = HubSession::new(self.hub.clone(), TOKEN.into());
            let reply = session.handle_line(
                &json!({"t":"hello","v":1,"token":TOKEN,"agent":self.agent}).to_string(),
            );
            if reply.close {
                return Err(LinkError::Rejected(reply.line));
            }
            self.session = Some(session);
            Ok(())
        }

        fn is_connected(&self) -> bool {
            self.session.is_some()
        }

        fn close(&mut self) {
            self.session = None;
        }
    }

    /// Sem NeuralIA.
    pub(crate) struct NoHub;

    impl HubLink for NoHub {
        fn exchange(&mut self, _: &Value) -> Result<Value, LinkError> {
            Err(LinkError::NotRunning("sem canal".into()))
        }
        fn ensure_connected(&mut self) -> Result<(), LinkError> {
            Err(LinkError::NotRunning("sem canal".into()))
        }
        fn is_connected(&self) -> bool {
            false
        }
        fn close(&mut self) {}
    }

    /// stdin de teste: cada `send` e um bloco; largar o `Sender` e o EOF.
    pub(crate) struct ChannelReader {
        rx: Receiver<Vec<u8>>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl io::Read for ChannelReader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let available = self.fill_buf()?;
            let n = available.len().min(out.len());
            out[..n].copy_from_slice(&available[..n]);
            self.consume(n);
            Ok(n)
        }
    }

    impl BufRead for ChannelReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            while self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(chunk) => {
                        self.buf = chunk;
                        self.pos = 0;
                    }
                    Err(_) => return Ok(&[]),
                }
            }
            Ok(&self.buf[self.pos..])
        }

        fn consume(&mut self, amount: usize) {
            self.pos += amount;
        }
    }

    /// stdout de teste: guarda tudo (para a prova de pureza) e entrega linhas.
    pub(crate) struct ChannelWriter {
        tx: Sender<String>,
        pending: Vec<u8>,
        pub(crate) raw: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for ChannelWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.raw.lock().unwrap().extend_from_slice(data);
            self.pending.extend_from_slice(data);
            while let Some(at) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=at).collect();
                let _ = self
                    .tx
                    .send(String::from_utf8(line[..line.len() - 1].to_vec()).unwrap());
            }
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(crate) struct Client {
        pub(crate) stdin: Option<Sender<Vec<u8>>>,
        pub(crate) stdout: Receiver<String>,
        pub(crate) raw: Arc<Mutex<Vec<u8>>>,
        pub(crate) server: Option<JoinHandle<io::Result<()>>>,
    }

    impl Client {
        pub(crate) fn start(link: Box<dyn HubLink>, options: ServeOptions) -> Self {
            let (in_tx, in_rx) = mpsc::channel();
            let (out_tx, out_rx) = mpsc::channel();
            let raw = Arc::new(Mutex::new(Vec::new()));
            let writer = ChannelWriter {
                tx: out_tx,
                pending: Vec::new(),
                raw: Arc::clone(&raw),
            };
            let reader = ChannelReader {
                rx: in_rx,
                buf: Vec::new(),
                pos: 0,
            };
            let server =
                thread::spawn(move || serve("teste", reader, Box::new(writer), link, options));
            Self {
                stdin: Some(in_tx),
                stdout: out_rx,
                raw,
                server: Some(server),
            }
        }

        pub(crate) fn raw_line(&self, line: &str) {
            let mut bytes = line.as_bytes().to_vec();
            bytes.push(b'\n');
            self.stdin.as_ref().unwrap().send(bytes).unwrap();
        }

        pub(crate) fn send(&self, message: Value) {
            self.raw_line(&message.to_string());
        }

        pub(crate) fn recv(&self) -> Value {
            let line = self
                .stdout
                .recv_timeout(Duration::from_secs(10))
                .expect("the bridge did not answer");
            serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON on stdout: {line}"))
        }

        pub(crate) fn nothing_for(&self, wait: Duration) -> bool {
            matches!(
                self.stdout.recv_timeout(wait),
                Err(RecvTimeoutError::Timeout)
            )
        }

        pub(crate) fn request(&self, id: i64, method: &str, params: Value) -> Value {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
            self.recv()
        }

        pub(crate) fn initialize(&self, version: &str) -> Value {
            let reply = self.request(
                0,
                "initialize",
                json!({"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"teste","version":"1"}}),
            );
            self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
            reply
        }

        pub(crate) fn call(&self, id: i64, name: &str, arguments: Value) -> Value {
            self.request(id, "tools/call", json!({"name":name,"arguments":arguments}))
        }

        pub(crate) fn finish(mut self) -> Vec<u8> {
            self.stdin.take();
            self.server
                .take()
                .unwrap()
                .join()
                .unwrap()
                .expect("serve failed");
            self.raw.lock().unwrap().clone()
        }
    }

    pub(crate) fn test_options() -> ServeOptions {
        ServeOptions {
            keepalive: false,
            progress_every: Duration::from_millis(40),
            wait_slice: Duration::from_millis(20),
        }
    }

    fn local(f: &Fixture, agent: &str) -> Box<dyn HubLink> {
        Box::new(LocalLink {
            hub: f.hub.clone(),
            agent: agent.into(),
            session: None,
        })
    }

    fn modern_meta() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "teste", "version": "1"},
        })
    }

    /// Todo o stdout sao mensagens JSON-RPC 2.0, uma por linha.
    pub(crate) fn assert_pure_protocol(raw: &[u8]) {
        let text = std::str::from_utf8(raw).expect("stdout is UTF-8");
        assert!(text.is_empty() || text.ends_with('\n'), "{text:?}");
        for line in text.lines() {
            let value: Value = serde_json::from_str(line)
                .unwrap_or_else(|_| panic!("stdout line is not JSON: {line:?}"));
            assert_eq!(value["jsonrpc"], "2.0", "{line}");
            let object = value.as_object().unwrap();
            assert!(
                object.contains_key("result")
                    || object.contains_key("error")
                    || object.contains_key("method"),
                "{line}"
            );
        }
    }

    fn wait_event(f: &Fixture, pick: impl Fn(&AgentEvent) -> bool) -> AgentEvent {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(event) = f.events.lock().unwrap().iter().find(|e| pick(e)).cloned() {
                return event;
            }
            assert!(Instant::now() < deadline, "event never arrived");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn mcp_lifecycle_negotiates_legacy_and_modern_protocol_versions() {
        let f = fixture("mcp-lifecycle");
        let client = Client::start(local(&f, "claude"), test_options());
        // Antes do initialize: so ping.
        let early = client.request(1, "tools/list", json!({}));
        assert_eq!(early["error"]["code"], INVALID_REQUEST, "{early}");
        assert_eq!(client.request(2, "ping", json!({}))["result"], json!({}));

        let init = client.initialize("2025-06-18");
        assert_eq!(init["id"], 0);
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            init["result"]["capabilities"],
            json!({"tools": {"listChanged": false}})
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "neuralia");
        assert_eq!(
            init["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("ask_user")
        );
        assert!(init["result"].get("resultType").is_none());
        let again = client.request(3, "initialize", json!({"protocolVersion":"2025-06-18"}));
        assert_eq!(again["error"]["code"], INVALID_REQUEST);
        let tools = client.request(4, "tools/list", json!({}));
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 4);

        // Moderno: sem estado, versao e capacidades em cada pedido.
        let discover = client.request(5, "server/discover", json!({"_meta": modern_meta()}));
        assert_eq!(discover["result"]["resultType"], "complete");
        assert_eq!(discover["result"]["supportedVersions"][0], MODERN_VERSION);
        assert_eq!(
            discover["result"]["_meta"][META_SERVER_INFO]["name"],
            "neuralia"
        );
        let modern_list = client.request(6, "tools/list", json!({"_meta": modern_meta()}));
        assert_eq!(modern_list["result"]["resultType"], "complete");
        assert_eq!(modern_list["result"]["tools"], tools["result"]["tools"]);
        let unsupported = client.request(
            7,
            "tools/list",
            json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2099-01-01", "io.modelcontextprotocol/clientCapabilities": {}}}),
        );
        assert_eq!(unsupported["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(unsupported["error"]["data"]["requested"], "2099-01-01");
        assert!(
            unsupported["error"]["data"]["supported"]
                .as_array()
                .unwrap()
                .contains(&json!(MODERN_VERSION))
        );
        let missing = client.request(
            8,
            "tools/list",
            json!({"_meta": {"io.modelcontextprotocol/protocolVersion": MODERN_VERSION}}),
        );
        assert_eq!(missing["error"]["code"], INVALID_PARAMS);
        let legacy_discover = client.request(9, "server/discover", json!({}));
        assert_eq!(legacy_discover["error"]["code"], INVALID_PARAMS);
        assert_pure_protocol(&client.finish());

        // Versao desconhecida no initialize: responde com a mais recente.
        let client = Client::start(local(&f, "claude"), test_options());
        let init = client.initialize("1999-01-01");
        assert_eq!(init["result"]["protocolVersion"], LEGACY_VERSIONS[0]);
        client.finish();
    }

    #[test]
    fn mcp_framing_errors_use_json_rpc_codes_and_stdout_stays_pure() {
        let f = fixture("mcp-framing");
        let client = Client::start(local(&f, "claude"), test_options());
        client.initialize("2025-11-25");
        client.raw_line("{not json");
        let parse = client.recv();
        assert_eq!(parse["error"]["code"], PARSE_ERROR);
        assert_eq!(parse["id"], Value::Null);
        client.raw_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
        assert_eq!(client.recv()["error"]["code"], INVALID_REQUEST);
        client.raw_line(r#"{"id":2,"method":"ping"}"#);
        let no_version = client.recv();
        assert_eq!(no_version["error"]["code"], INVALID_REQUEST);
        assert_eq!(no_version["id"], 2);
        client.raw_line(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#);
        assert_eq!(client.recv()["error"]["code"], INVALID_REQUEST);
        client.raw_line(r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#);
        assert_eq!(client.recv()["error"]["code"], INVALID_REQUEST);
        client.raw_line(r#"{"jsonrpc":"2.0","id":"s","method":"ping","params":[1]}"#);
        let bad_params = client.recv();
        assert_eq!(bad_params["error"]["code"], INVALID_PARAMS);
        assert_eq!(bad_params["id"], "s");
        let unknown = client.request(3, "resources/list", json!({}));
        assert_eq!(unknown["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(unknown["id"], 3);
        // Notificacoes e respostas do cliente nao tem resposta.
        client.raw_line(r#"{"jsonrpc":"2.0","method":"notifications/whatever"}"#);
        client.raw_line(r#"{"jsonrpc":"2.0","id":99,"result":{}}"#);
        client.raw_line("");
        client.raw_line("   \r");
        assert!(client.nothing_for(Duration::from_millis(100)));
        // Uma mensagem acima do tecto e descartada sem rebentar a ponte.
        client.raw_line(&format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"ping","params":{{"x":"{}"}}}}"#,
            "a".repeat(MAX_MESSAGE_BYTES)
        ));
        assert_eq!(client.recv()["error"]["code"], PARSE_ERROR);
        let mut invalid_utf8 = br#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#.to_vec();
        invalid_utf8.insert(10, 0xFF);
        invalid_utf8.push(b'\n');
        client.stdin.as_ref().unwrap().send(invalid_utf8).unwrap();
        assert_eq!(client.recv()["error"]["code"], PARSE_ERROR);
        // Mensagem com \r\n: aceite.
        client.raw_line("{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"ping\"}\r");
        assert_eq!(client.recv()["id"], 6);
        // String ids voltam iguais.
        let string_id = client.request(7, "ping", json!({}));
        assert_eq!(string_id["id"], 7);
        client.send(json!({"jsonrpc":"2.0","id":"abc","method":"ping"}));
        assert_eq!(client.recv()["id"], "abc");
        assert_pure_protocol(&client.finish());
    }

    #[test]
    fn mcp_tool_calls_reach_the_hub_and_bad_input_never_does() {
        let f = fixture("mcp-tools");
        let client = Client::start(local(&f, "claude"), test_options());
        client.initialize("2025-06-18");

        let sent = client.call(
            1,
            "send_message",
            json!({"text":"Terminei os testes.","title":"Pronto"}),
        );
        assert_eq!(sent["result"]["isError"], false, "{sent}");
        assert_eq!(sent["result"]["structuredContent"]["delivered"], true);
        let text: Value =
            serde_json::from_str(sent["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, sent["result"]["structuredContent"]);
        let event = wait_event(&f, |e| matches!(e, AgentEvent::Message { .. }));
        let AgentEvent::Message { agent, record } = event else {
            unreachable!()
        };
        assert_eq!(agent, "claude");
        assert_eq!(
            record.body,
            crate::agents::RecordBody::AgentMessage {
                title: Some("Pronto".into()),
                text: "Terminei os testes.".into()
            }
        );

        // Invalido: erro de execucao, e o hub nao ve nada.
        for (id, arguments) in [
            (2, json!({"text":"x","color":"red"})),
            (3, json!({"text": "y".repeat(4_001)})),
            (4, json!({})),
        ] {
            let reply = client.call(id, "send_message", arguments);
            assert_eq!(reply["result"]["isError"], true, "{reply}");
            assert!(reply["result"].get("structuredContent").is_none());
        }
        assert_eq!(f.hub.conversation("claude").len(), 1);
        let unknown = client.call(5, "delete_files", json!({}));
        assert_eq!(unknown["error"]["code"], INVALID_PARAMS);
        assert!(
            unknown["error"]["message"]
                .as_str()
                .unwrap()
                .contains("delete_files")
        );
        let no_name = client.request(6, "tools/call", json!({"arguments":{}}));
        assert_eq!(no_name["error"]["code"], INVALID_PARAMS);

        // Status.
        let status = client.call(
            7,
            "set_status",
            json!({"text":"a trabalhar…","progress":60}),
        );
        assert_eq!(status["result"]["isError"], false);
        wait_event(
            &f,
            |e| matches!(e, AgentEvent::Status { status: Some(line), .. } if line.progress == Some(60)),
        );

        // Leitura: so o que o usuario escreveu para este agente.
        f.hub.user_message("claude", "podes seguir").unwrap();
        f.hub.user_message("gemini", "segredo do gemini").unwrap();
        let read = client.call(8, "get_user_messages", json!({}));
        let messages = &read["result"]["structuredContent"]["messages"];
        assert_eq!(messages.as_array().unwrap().len(), 1, "{read}");
        assert_eq!(messages[0]["text"], "podes seguir");
        assert!(!read.to_string().contains("segredo"));
        assert_pure_protocol(&client.finish());

        // 2024-11-05 nao conhece structuredContent.
        let client = Client::start(local(&f, "claude"), test_options());
        client.initialize("2024-11-05");
        let old = client.call(1, "get_user_messages", json!({"since_id": 0}));
        assert_eq!(old["result"]["isError"], false);
        assert!(old["result"].get("structuredContent").is_none());
        // Moderno: resultType em tools/call.
        let modern = client.request(
            2,
            "tools/call",
            json!({"name":"set_status","arguments":{"text":"x"},"_meta": modern_meta()}),
        );
        assert_eq!(modern["result"]["resultType"], "complete");
        assert_eq!(modern["result"]["isError"], false);
        client.finish();
    }

    #[test]
    fn mcp_ask_user_waits_for_the_answer_and_reports_progress() {
        let f = fixture("mcp-ask");
        let client = Client::start(local(&f, "claude"), test_options());
        client.initialize("2025-11-25");
        client.send(json!({
            "jsonrpc":"2.0","id":"q1","method":"tools/call",
            "params":{"name":"ask_user","arguments":{"question":"Apago a pasta build?","options":["Sim","Não"]},
                      "_meta":{"progressToken":"tok-1"}}
        }));
        let AgentEvent::Question(card) = wait_event(&f, |e| matches!(e, AgentEvent::Question(_)))
        else {
            unreachable!()
        };
        assert_eq!(card.text, "Apago a pasta build?");
        // Enquanto espera, a ponte manda progresso (a crescer) e continua a
        // responder aos outros pedidos.
        let mut last_progress = 0;
        let mut progress_seen = 0;
        let mut next = |client: &Client| loop {
            let message = client.recv();
            if message["method"] == "notifications/progress" {
                assert_eq!(message["params"]["progressToken"], "tok-1");
                let value = message["params"]["progress"].as_u64().unwrap();
                assert!(value > last_progress, "progress must increase");
                last_progress = value;
                progress_seen += 1;
                continue;
            }
            break message;
        };
        thread::sleep(Duration::from_millis(150));
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
        assert_eq!(next(&client)["id"], 2);
        f.hub
            .answer_question(card.id, QuestionAnswer::Button(0))
            .unwrap();
        let answer = next(&client);
        assert!(progress_seen >= 1, "no progress notification");
        assert_eq!(answer["id"], "q1");
        assert_eq!(
            answer["result"]["structuredContent"],
            json!({"answer":"Sim","via":"button"})
        );
        // Texto livre.
        client.send(json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"ask_user","arguments":{"question":"Que nome dou ao ficheiro?"}}
        }));
        let AgentEvent::Question(card) = wait_event(
            &f,
            |e| matches!(e, AgentEvent::Question(q) if q.text.starts_with("Que nome")),
        ) else {
            unreachable!()
        };
        f.hub
            .answer_question(card.id, QuestionAnswer::Text("notas.md".into()))
            .unwrap();
        let answer = client.recv();
        assert_eq!(
            answer["result"]["structuredContent"],
            json!({"answer":"notas.md","via":"text"})
        );
        assert_pure_protocol(&client.finish());
    }

    #[test]
    fn mcp_ask_user_times_out_with_timed_out_true() {
        let f = fixture("mcp-timeout");
        let client = Client::start(local(&f, "codex"), test_options());
        client.initialize("2025-11-25");
        client.call_async(
            1,
            "ask_user",
            json!({"question":"Continuo?","timeout_seconds":10}),
        );
        wait_event(&f, |e| matches!(e, AgentEvent::Question(_)));
        f.clock.advance(Duration::from_secs(11));
        let reply = client.recv();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(
            reply["result"]["structuredContent"],
            json!({"timed_out": true})
        );
        assert!(f.hub.pending_questions().is_empty());
        client.finish();
    }

    #[test]
    fn mcp_cancelled_ask_user_closes_the_card_and_is_never_answered() {
        let f = fixture("mcp-cancel");
        let client = Client::start(local(&f, "gemini"), test_options());
        client.initialize("2025-11-25");
        client.call_async(7, "ask_user", json!({"question":"Posso?"}));
        wait_event(&f, |e| matches!(e, AgentEvent::Question(_)));
        client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"user"}}));
        wait_event(&f, |e| {
            matches!(
                e,
                AgentEvent::QuestionClosed {
                    outcome: crate::agents::QuestionOutcome::Closed(
                        crate::agents::CloseReason::Cancelled
                    ),
                    ..
                }
            )
        });
        assert!(f.hub.pending_questions().is_empty());
        // A proxima resposta e a do ping: a pergunta cancelada nao teve resposta.
        let ping = client.request(8, "ping", json!({}));
        assert_eq!(ping["id"], 8);
        assert!(client.nothing_for(Duration::from_millis(100)));

        // Fim do stdin com uma pergunta pendente: cancelada, e a ponte sai.
        client.call_async(9, "ask_user", json!({"question":"E agora?"}));
        wait_event(
            &f,
            |e| matches!(e, AgentEvent::Question(q) if q.text == "E agora?"),
        );
        let raw = client.finish();
        assert!(f.hub.pending_questions().is_empty());
        assert!(!String::from_utf8(raw).unwrap().contains("\"id\":9"));
    }

    #[test]
    fn mcp_without_neuralia_says_to_open_it_instead_of_hanging() {
        let client = Client::start(Box::new(NoHub), test_options());
        client.initialize("2025-11-25");
        // O ciclo de vida nao precisa do NeuralIA.
        assert_eq!(
            client.request(1, "tools/list", json!({}))["result"]["tools"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        let started = Instant::now();
        for (id, name, arguments) in [
            (2, "send_message", json!({"text":"oi"})),
            (3, "ask_user", json!({"question":"?"})),
            (4, "get_user_messages", json!({})),
            (5, "set_status", json!({"text":"x"})),
        ] {
            let reply = client.call(id, name, arguments);
            assert_eq!(reply["result"]["isError"], true, "{reply}");
            assert_eq!(reply["result"]["content"][0]["text"], NOT_RUNNING);
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_pure_protocol(&client.finish());
    }

    impl Client {
        pub(crate) fn call_async(&self, id: i64, name: &str, arguments: Value) {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        }
    }
}
