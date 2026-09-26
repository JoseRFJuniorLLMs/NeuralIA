//! As conversas com cada agente, em `<data_dir>/agents/<agente>.jsonl`.
//!
//! Um ficheiro por agente, uma linha JSON por registo, so acrescentado. Tem
//! tecto: passando de `MAX_LINES` linhas ou `MAX_BYTES` bytes, o ficheiro e
//! reescrito de forma atomica (temporario + `rename`) so com os registos mais
//! recentes (`KEEP_LINES` / `KEEP_BYTES`). Uma linha estragada (o processo
//! morreu a meio de uma escrita) e descartada no arranque e o ficheiro e
//! reescrito limpo, para o registo seguinte nao ficar colado a ela.
//!
//! Nada daqui toca no historico nem na memoria da NeuralIA: e o unico sitio
//! onde as conversas com agentes vivem, e o Ctrl+Shift+Delete apaga-o.
//!
//! A pasta abre-se com o `StoreGrant` da loja `agents` (`StoreKind::Automatic`,
//! `StoreShape::Dir`: efeito lateral do uso), pedido ao `StoreRegistry` do
//! `App`. Enquanto o registo disser `StoreMode::Private`, nada daqui escreve:
//! as conversas ficam no ecra e na memoria (`Conversation::unsaved`), o
//! agente le-as pelo `get_user_messages`, e nem uma reescrita posterior do
//! ficheiro as leva ao disco. Apagar (`clear`) vale sempre.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use neural_core::json_store::{StoreError, StoreGrant, StoreShape};
use serde_json::{Value, json};

use super::tools::{
    ANSWER_MAX_CHARS, MESSAGE_MAX_CHARS, OPTION_MAX_CHARS, OPTIONS_MAX, QUESTION_MAX_CHARS,
    TITLE_MAX_CHARS, clean_text, sanitize_agent_name,
};

pub(crate) const MAX_LINES: usize = 2_000;
pub(crate) const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const KEEP_LINES: usize = 1_000;
pub(crate) const KEEP_BYTES: u64 = 1024 * 1024;
/// Quantos agentes diferentes o NeuralIA guarda.
pub(crate) const MAX_AGENTS: usize = 32;
/// O que o painel partilha com os botoes «esta página» / «estas abas».
pub(crate) const SHARE_TITLE_MAX_CHARS: usize = 300;
pub(crate) const SHARE_URL_MAX_CHARS: usize = 2_048;
pub(crate) const SHARE_EXCERPT_MAX_CHARS: usize = 20_000;
pub(crate) const SHARE_TABS_MAX: usize = 50;

const STATE_FILE: &str = "state.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnswerVia {
    Button,
    Text,
}

impl AnswerVia {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Text => "text",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "button" => Some(Self::Button),
            "text" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Porque uma pergunta fechou sem resposta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseReason {
    /// Passou o `timeout_seconds` do agente.
    TimedOut,
    /// O utilizador fechou o cartao sem responder.
    Dismissed,
    /// O agente desistiu (`notifications/cancelled`) ou desligou-se.
    Cancelled,
}

impl CloseReason {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::TimedOut => "timed_out",
            Self::Dismissed => "dismissed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "timed_out" => Some(Self::TimedOut),
            "dismissed" => Some(Self::Dismissed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharedTab {
    pub(crate) title: String,
    pub(crate) url: String,
}

/// Contexto que o utilizador partilhou de proposito com um agente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SharedContext {
    /// «esta página»: titulo, endereco e, se a UI o tiver, um excerto.
    Page {
        title: String,
        url: String,
        excerpt: Option<String>,
    },
    /// «estas abas»: titulo e endereco de cada aba.
    Tabs(Vec<SharedTab>),
}

impl SharedContext {
    /// Limpa e corta ao tecto. `None` quando nao sobra nada para partilhar.
    pub(crate) fn sanitized(self) -> Option<Self> {
        match self {
            Self::Page {
                title,
                url,
                excerpt,
            } => {
                let url = cap(&clean_line(&url), SHARE_URL_MAX_CHARS);
                if url.is_empty() {
                    return None;
                }
                let excerpt = excerpt
                    .map(|text| cap(&clean_text(&text), SHARE_EXCERPT_MAX_CHARS))
                    .filter(|text| !text.is_empty());
                Some(Self::Page {
                    title: cap(&clean_line(&title), SHARE_TITLE_MAX_CHARS),
                    url,
                    excerpt,
                })
            }
            Self::Tabs(tabs) => {
                let tabs: Vec<SharedTab> = tabs
                    .into_iter()
                    .filter_map(|tab| {
                        let url = cap(&clean_line(&tab.url), SHARE_URL_MAX_CHARS);
                        (!url.is_empty()).then(|| SharedTab {
                            title: cap(&clean_line(&tab.title), SHARE_TITLE_MAX_CHARS),
                            url,
                        })
                    })
                    .take(SHARE_TABS_MAX)
                    .collect();
                (!tabs.is_empty()).then_some(Self::Tabs(tabs))
            }
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Page {
                title,
                url,
                excerpt,
            } => {
                let mut value = json!({"kind":"page","title":title,"url":url});
                if let Some(excerpt) = excerpt {
                    value["excerpt"] = json!(excerpt);
                }
                value
            }
            Self::Tabs(tabs) => json!({
                "kind": "tabs",
                "tabs": tabs
                    .iter()
                    .map(|tab| json!({"title": tab.title, "url": tab.url}))
                    .collect::<Vec<_>>(),
            }),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        let context = match value.get("kind")?.as_str()? {
            "page" => Self::Page {
                title: value.get("title")?.as_str()?.to_string(),
                url: value.get("url")?.as_str()?.to_string(),
                excerpt: value
                    .get("excerpt")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            "tabs" => Self::Tabs(
                value
                    .get("tabs")?
                    .as_array()?
                    .iter()
                    .map(|tab| {
                        Some(SharedTab {
                            title: tab.get("title")?.as_str()?.to_string(),
                            url: tab.get("url")?.as_str()?.to_string(),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            _ => return None,
        };
        context.sanitized()
    }
}

/// O que aconteceu, por ordem, na conversa com um agente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordBody {
    /// `send_message`.
    AgentMessage { title: Option<String>, text: String },
    /// `ask_user`: a pergunta, tal como o cartao a mostrou.
    AgentQuestion {
        question_id: u64,
        text: String,
        options: Vec<String>,
        allow_text: bool,
    },
    /// A resposta do utilizador a uma pergunta.
    UserAnswer {
        question_id: u64,
        text: String,
        via: AnswerVia,
    },
    /// A pergunta fechou sem resposta.
    QuestionClosed {
        question_id: u64,
        reason: CloseReason,
    },
    /// O que o utilizador escreveu ao agente no painel.
    UserMessage { text: String },
    /// O que o utilizador partilhou com um botao.
    UserShare(SharedContext),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRecord {
    /// Crescente dentro da conversa; e o `since_id` do `get_user_messages`.
    pub(crate) id: u64,
    /// Milissegundos desde 1970 (UTC).
    pub(crate) ts_ms: u64,
    pub(crate) body: RecordBody,
}

impl AgentRecord {
    /// Escrito pelo agente (conta para o «não lido» do painel).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_from_agent(&self) -> bool {
        matches!(
            self.body,
            RecordBody::AgentMessage { .. } | RecordBody::AgentQuestion { .. }
        )
    }

    /// Escrito ou partilhado pelo utilizador PARA o agente: e so isto que o
    /// `get_user_messages` devolve.
    pub(crate) fn is_for_agent(&self) -> bool {
        matches!(
            self.body,
            RecordBody::UserMessage { .. } | RecordBody::UserShare(_)
        )
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut value = match &self.body {
            RecordBody::AgentMessage { title, text } => {
                let mut value = json!({"kind":"message","from":"agent","text":text});
                if let Some(title) = title {
                    value["title"] = json!(title);
                }
                value
            }
            RecordBody::AgentQuestion {
                question_id,
                text,
                options,
                allow_text,
            } => json!({
                "kind": "question",
                "from": "agent",
                "question_id": question_id,
                "text": text,
                "options": options,
                "allow_text": allow_text,
            }),
            RecordBody::UserAnswer {
                question_id,
                text,
                via,
            } => json!({
                "kind": "answer",
                "from": "user",
                "question_id": question_id,
                "text": text,
                "via": via.wire(),
            }),
            RecordBody::QuestionClosed {
                question_id,
                reason,
            } => json!({
                "kind": "closed",
                "from": "system",
                "question_id": question_id,
                "reason": reason.wire(),
            }),
            RecordBody::UserMessage { text } => json!({"kind":"user","from":"user","text":text}),
            RecordBody::UserShare(context) => {
                json!({"kind":"share","from":"user","share":context.to_json()})
            }
        };
        value["id"] = json!(self.id);
        value["ts"] = json!(self.ts_ms);
        value
    }

    /// Le um registo do disco. O ficheiro e do utilizador, mas os tectos e a
    /// limpeza voltam a valer: o painel so mostra o que passaria pela entrada.
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let id = value.get("id")?.as_u64()?;
        let ts_ms = value.get("ts")?.as_u64()?;
        let text = |max: usize| -> Option<String> {
            let text = clean_text(value.get("text")?.as_str()?);
            let count = text.chars().count();
            (count >= 1 && count <= max).then_some(text)
        };
        let question_id = || value.get("question_id").and_then(Value::as_u64);
        let body = match value.get("kind")?.as_str()? {
            "message" => RecordBody::AgentMessage {
                title: value
                    .get("title")
                    .and_then(Value::as_str)
                    .map(|title| cap(&clean_line(title), TITLE_MAX_CHARS))
                    .filter(|title| !title.is_empty()),
                text: text(MESSAGE_MAX_CHARS)?,
            },
            "question" => {
                let options = value
                    .get("options")?
                    .as_array()?
                    .iter()
                    .map(|option| {
                        let option = clean_line(option.as_str()?);
                        let count = option.chars().count();
                        (1..=OPTION_MAX_CHARS).contains(&count).then_some(option)
                    })
                    .collect::<Option<Vec<_>>>()?;
                if options.is_empty() || options.len() > OPTIONS_MAX {
                    return None;
                }
                RecordBody::AgentQuestion {
                    question_id: question_id()?,
                    text: text(QUESTION_MAX_CHARS)?,
                    options,
                    allow_text: value.get("allow_text")?.as_bool()?,
                }
            }
            "answer" => RecordBody::UserAnswer {
                question_id: question_id()?,
                text: text(ANSWER_MAX_CHARS)?,
                via: AnswerVia::parse(value.get("via")?.as_str()?)?,
            },
            "closed" => RecordBody::QuestionClosed {
                question_id: question_id()?,
                reason: CloseReason::parse(value.get("reason")?.as_str()?)?,
            },
            "user" => RecordBody::UserMessage {
                text: text(MESSAGE_MAX_CHARS)?,
            },
            "share" => RecordBody::UserShare(SharedContext::from_json(value.get("share")?)?),
            _ => return None,
        };
        Some(Self { id, ts_ms, body })
    }

    fn line(&self) -> String {
        // serde_json escapa as quebras de linha: um registo e uma linha.
        let mut line = self.to_json().to_string();
        line.push('\n');
        line
    }
}

/// Texto numa linha so: quebras e tabs viram espacos.
pub(crate) fn clean_line(raw: &str) -> String {
    clean_text(raw)
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect()
}

fn cap(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// `2026-09-23T10:00:00Z` a partir de milissegundos UTC.
pub(crate) fn iso_utc(ms: u64) -> String {
    let secs = ms / 1_000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant, `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// A conversa com um agente: o que esta no ficheiro mais, numa sessao
/// privada, o que so esta na memoria.
#[derive(Debug, Default)]
pub(crate) struct Conversation {
    pub(crate) records: VecDeque<AgentRecord>,
    /// Tamanho do ficheiro em bytes (a soma das linhas que estao no disco).
    bytes: u64,
    /// Registos acrescentados enquanto as escritas estavam vedadas (modo
    /// privado): ficam no ecra e na memoria e nunca entram numa reescrita do
    /// ficheiro, mesmo depois de o modo voltar a normal.
    unsaved: BTreeSet<u64>,
}

impl Conversation {
    pub(crate) fn last_id(&self) -> u64 {
        self.records.back().map_or(0, |record| record.id)
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    fn from_records(records: Vec<AgentRecord>) -> Self {
        let bytes = records.iter().map(|r| r.line().len() as u64).sum();
        Self {
            records: records.into(),
            bytes,
            unsaved: BTreeSet::new(),
        }
    }

    /// Larga os registos mais antigos ate caber em `KEEP_LINES`/`KEEP_BYTES`.
    fn trim_to_keep(&mut self) {
        while self.records.len() > KEEP_LINES || self.bytes > KEEP_BYTES {
            match self.records.pop_front() {
                Some(old) => {
                    if !self.unsaved.remove(&old.id) {
                        self.bytes -= old.line().len() as u64;
                    }
                }
                None => break,
            }
        }
    }

    fn over_cap(&self) -> bool {
        self.records.len() > MAX_LINES || self.bytes > MAX_BYTES
    }
}

/// Os ficheiros das conversas na pasta da loja `agents`, aberta pelo grant.
#[derive(Debug, Clone)]
pub(crate) struct ConversationStore {
    grant: Arc<StoreGrant>,
}

/// O que o arranque leu do disco.
#[derive(Debug, Default)]
pub(crate) struct LoadedStore {
    pub(crate) conversations: BTreeMap<String, Conversation>,
    pub(crate) marks: BTreeMap<String, AgentMarks>,
}

/// O que se guarda de cada agente fora da conversa.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AgentMarks {
    /// Ultimo registo que o utilizador viu no painel.
    pub(crate) read_up_to: u64,
    /// Proximo id a dar. Sobrevive ao Ctrl+Shift+Delete, para um agente que
    /// guardou um `since_id` antigo nao perder as mensagens novas.
    pub(crate) next_id: u64,
}

impl ConversationStore {
    /// Abre a loja pelo grant da pasta `agents` (`stores::AGENTS_STORE`). Um
    /// grant que nao e de uma pasta e recusado.
    pub(crate) fn open(grant: StoreGrant) -> Result<Self, StoreError> {
        if grant.shape() != StoreShape::Dir {
            return Err(StoreError::WrongShape {
                name: grant.name(),
                expected: StoreShape::Dir,
            });
        }
        Ok(Self {
            grant: Arc::new(grant),
        })
    }

    pub(crate) fn dir(&self) -> &Path {
        self.grant.path()
    }

    /// Falso enquanto o registo das lojas disser `StoreMode::Private`: a loja
    /// e `Automatic`, e uma sessao privada nao deixa nada no disco.
    pub(crate) fn writes_allowed(&self) -> bool {
        self.grant.writes_allowed()
    }

    fn conversation_path(&self, agent: &str) -> PathBuf {
        self.dir().join(format!("{agent}.jsonl"))
    }

    /// Le tudo o que esta na pasta. Nunca falha: o que nao se le fica de fora.
    pub(crate) fn load(&self) -> LoadedStore {
        let mut loaded = LoadedStore {
            marks: self.load_marks(),
            ..LoadedStore::default()
        };
        let Ok(entries) = fs::read_dir(self.dir()) else {
            return loaded;
        };
        let mut agents: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let stem = name.strip_suffix(".jsonl")?;
                (sanitize_agent_name(stem).as_deref() == Some(stem)).then(|| stem.to_string())
            })
            .collect();
        agents.sort();
        agents.truncate(MAX_AGENTS);
        for agent in agents {
            match self.load_conversation(&agent) {
                Ok(conversation) => {
                    loaded.conversations.insert(agent, conversation);
                }
                Err(error) => eprintln!("[agents] conversa {agent} ilegivel: {error}"),
            }
        }
        loaded
    }

    fn load_conversation(&self, agent: &str) -> io::Result<Conversation> {
        let path = self.conversation_path(agent);
        let mut file = File::open(&path)?;
        let length = file.metadata()?.len();
        let mut needs_rewrite = false;
        // Um ficheiro muito maior do que o tecto nao foi escrito por nos:
        // le-se so a cauda.
        let mut bytes = Vec::new();
        if length > MAX_BYTES * 2 {
            file.seek(SeekFrom::Start(length - MAX_BYTES))?;
            file.read_to_end(&mut bytes)?;
            match bytes.iter().position(|&b| b == b'\n') {
                Some(first) => {
                    bytes.drain(..=first);
                }
                None => bytes.clear(),
            }
            needs_rewrite = true;
        } else {
            file.read_to_end(&mut bytes)?;
        }
        drop(file);
        if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
            needs_rewrite = true;
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut records: Vec<AgentRecord> = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line)
                .ok()
                .as_ref()
                .and_then(AgentRecord::from_json)
            {
                Some(record) => records.push(record),
                None => needs_rewrite = true,
            }
        }
        let before = records.len();
        records.sort_by_key(|record| record.id);
        records.dedup_by_key(|record| record.id);
        if records.len() != before {
            needs_rewrite = true;
        }
        let mut conversation = Conversation::from_records(records);
        if conversation.over_cap() {
            conversation.trim_to_keep();
            needs_rewrite = true;
        }
        if needs_rewrite {
            self.rewrite(agent, &conversation)?;
        }
        Ok(conversation)
    }

    /// Acrescenta `record` a conversa e ao ficheiro; passando do tecto,
    /// reescreve o ficheiro so com os registos mais recentes. Com as
    /// escritas vedadas (modo privado) o registo fica so na memoria.
    pub(crate) fn append(
        &self,
        agent: &str,
        conversation: &mut Conversation,
        record: AgentRecord,
    ) -> io::Result<()> {
        if !self.writes_allowed() {
            conversation.unsaved.insert(record.id);
            conversation.records.push_back(record);
            if conversation.over_cap() {
                conversation.trim_to_keep();
            }
            return Ok(());
        }
        let line = record.line();
        conversation.bytes += line.len() as u64;
        conversation.records.push_back(record);
        if conversation.over_cap() {
            conversation.trim_to_keep();
            return self.rewrite(agent, conversation);
        }
        fs::create_dir_all(self.dir())?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.conversation_path(agent))?;
        // Um so `write_all`: a linha entra inteira ou o arranque seguinte
        // descarta-a.
        file.write_all(line.as_bytes())
    }

    /// Reescreve o ficheiro com os registos que podem estar no disco: os de
    /// uma sessao privada (`unsaved`) nunca entram.
    fn rewrite(&self, agent: &str, conversation: &Conversation) -> io::Result<()> {
        if !self.writes_allowed() {
            return Ok(());
        }
        let mut body = String::with_capacity(conversation.bytes as usize);
        for record in &conversation.records {
            if conversation.unsaved.contains(&record.id) {
                continue;
            }
            body.push_str(&record.line());
        }
        write_atomic(&self.conversation_path(agent), body.as_bytes())
    }

    fn load_marks(&self) -> BTreeMap<String, AgentMarks> {
        let mut marks = BTreeMap::new();
        let Ok(text) = fs::read_to_string(self.dir().join(STATE_FILE)) else {
            return marks;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return marks;
        };
        let Some(agents) = value.get("agents").and_then(Value::as_object) else {
            return marks;
        };
        for (agent, entry) in agents.iter().take(MAX_AGENTS * 2) {
            if sanitize_agent_name(agent).as_deref() != Some(agent.as_str()) {
                continue;
            }
            marks.insert(
                agent.clone(),
                AgentMarks {
                    read_up_to: entry.get("read").and_then(Value::as_u64).unwrap_or(0),
                    next_id: entry.get("next").and_then(Value::as_u64).unwrap_or(0),
                },
            );
        }
        marks
    }

    /// Grava o estado de leitura e os ids. Com as escritas vedadas (modo
    /// privado) nao toca no ficheiro.
    pub(crate) fn save_marks(&self, marks: &BTreeMap<String, AgentMarks>) -> io::Result<()> {
        if !self.writes_allowed() {
            return Ok(());
        }
        let agents: serde_json::Map<String, Value> = marks
            .iter()
            .map(|(agent, mark)| {
                (
                    agent.clone(),
                    json!({"read": mark.read_up_to, "next": mark.next_id}),
                )
            })
            .collect();
        let body = json!({"v": 1, "agents": agents}).to_string();
        write_atomic(&self.dir().join(STATE_FILE), body.as_bytes())
    }

    /// Apaga as conversas (Ctrl+Shift+Delete), em qualquer modo. O token e o
    /// nome do canal ficam: o hub continua a correr.
    pub(crate) fn clear(&self) -> io::Result<()> {
        let entries = match fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut first_error = None;
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.ends_with(".jsonl") || name.ends_with(".jsonl.tmp"))
                && let Err(error) = fs::remove_file(entry.path())
            {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Escreve `path` de uma vez: temporario ao lado, `sync_all`, `rename`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::stores::AGENTS_STORE;
    use neural_core::json_store::{StoreKind, StoreMode, StoreRegistry, StoreSpec};

    /// Pasta temporaria que se apaga sozinha.
    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "neuralia-agents-{tag}-{}-{}-{}",
                std::process::id(),
                now_ms(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Um registo de teste sobre `root`, e a loja `agents` dele
    /// (`<root>/agents`), como o `App` a abre.
    pub(crate) fn open_store(root: &Path) -> (StoreRegistry, ConversationStore) {
        let registry = StoreRegistry::mint_for_test(root);
        let store = ConversationStore::open(registry.grant(AGENTS_STORE).unwrap()).unwrap();
        (registry, store)
    }

    fn message(id: u64, text: &str) -> AgentRecord {
        AgentRecord {
            id,
            ts_ms: 1_700_000_000_000 + id,
            body: RecordBody::AgentMessage {
                title: None,
                text: text.into(),
            },
        }
    }

    #[test]
    fn the_store_only_opens_on_the_agents_dir_grant() {
        let dir = TempDir::new("grant");
        let registry = StoreRegistry::mint_for_test(&dir.0);
        let file = registry
            .grant(StoreSpec::new(
                "agents.json",
                StoreKind::Automatic,
                StoreShape::File,
            ))
            .unwrap();
        assert!(matches!(
            ConversationStore::open(file),
            Err(StoreError::WrongShape { .. })
        ));
        let store = ConversationStore::open(registry.grant(AGENTS_STORE).unwrap()).unwrap();
        assert_eq!(store.dir(), dir.0.join("agents"));
        assert!(store.writes_allowed());
        // Nada no disco so por abrir.
        assert!(!dir.0.join("agents").exists());
    }

    /// Gate (critico: dados do utilizador, modo privado). Com o registo em
    /// `Private`, a loja `agents` (Automatic) nao escreve: conversa, estado
    /// e reescritas ficam na memoria; a pasta fica byte a byte igual. E os
    /// registos dessa sessao nunca chegam ao disco -- nem quando o modo
    /// volta a `Normal` e o tecto forca uma reescrita do ficheiro.
    #[test]
    fn a_private_session_never_reaches_the_disk_even_through_a_rewrite() {
        let dir = TempDir::new("private");
        let (registry, store) = open_store(&dir.0);
        let mut conversation = Conversation::default();
        store
            .append("claude", &mut conversation, message(1, "antes"))
            .unwrap();
        let path = store.conversation_path("claude");
        let before = fs::read(&path).unwrap();
        assert!(String::from_utf8_lossy(&before).contains("antes"));

        registry.set_mode(StoreMode::Private);
        assert!(!store.writes_allowed());
        for id in 2..=(MAX_LINES as u64) {
            store
                .append("claude", &mut conversation, message(id, "privado"))
                .unwrap();
        }
        let mut marks = BTreeMap::new();
        marks.insert(
            "claude".to_string(),
            AgentMarks {
                read_up_to: 5,
                next_id: 9,
            },
        );
        store.save_marks(&marks).unwrap();
        // No ecra e para o agente: tudo. No disco: nada de novo.
        assert_eq!(conversation.records.len(), MAX_LINES);
        assert_eq!(conversation.last_id(), MAX_LINES as u64);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!store.dir().join(STATE_FILE).exists());
        let names: Vec<String> = fs::read_dir(store.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["claude.jsonl".to_string()]);
        // Quem reabre le so o que ja la estava.
        let reloaded = store.load();
        assert_eq!(reloaded.conversations["claude"].records.len(), 1);
        assert!(reloaded.marks.is_empty());

        // De volta ao normal: o registo seguinte passa do tecto e reescreve o
        // ficheiro -- so com o que podia estar no disco.
        registry.set_mode(StoreMode::Normal);
        store
            .append(
                "claude",
                &mut conversation,
                message(MAX_LINES as u64 + 1, "depois"),
            )
            .unwrap();
        let file = fs::read_to_string(&path).unwrap();
        assert_eq!(file.lines().count(), 1, "{file}");
        assert!(file.contains("depois"), "{file}");
        assert!(!file.contains("privado"), "{file}");
        assert_eq!(conversation.records.len(), KEEP_LINES);
        assert_eq!(conversation.bytes(), fs::metadata(&path).unwrap().len());
        store.save_marks(&marks).unwrap();
        assert!(store.dir().join(STATE_FILE).exists());
        // Apagar vale em qualquer modo.
        registry.set_mode(StoreMode::Private);
        store.clear().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn every_record_kind_round_trips_through_disk() {
        let dir = TempDir::new("roundtrip");
        let (_registry, store) = open_store(&dir.0);
        let bodies = vec![
            RecordBody::AgentMessage {
                title: Some("Pronto".into()),
                text: "linha 1\nlinha 2".into(),
            },
            RecordBody::AgentQuestion {
                question_id: 4,
                text: "Posso?".into(),
                options: vec!["Sim".into(), "Não".into()],
                allow_text: true,
            },
            RecordBody::UserAnswer {
                question_id: 4,
                text: "Sim".into(),
                via: AnswerVia::Button,
            },
            RecordBody::QuestionClosed {
                question_id: 5,
                reason: CloseReason::TimedOut,
            },
            RecordBody::UserMessage {
                text: "olá \"agente\"".into(),
            },
            RecordBody::UserShare(SharedContext::Page {
                title: "Título".into(),
                url: "https://example.com/a".into(),
                excerpt: Some("trecho".into()),
            }),
            RecordBody::UserShare(SharedContext::Tabs(vec![SharedTab {
                title: "A".into(),
                url: "https://a.example".into(),
            }])),
        ];
        let mut conversation = Conversation::default();
        let mut expected = Vec::new();
        for (index, body) in bodies.into_iter().enumerate() {
            let record = AgentRecord {
                id: index as u64 + 1,
                ts_ms: 42,
                body,
            };
            expected.push(record.clone());
            store.append("claude", &mut conversation, record).unwrap();
        }
        let mut marks = BTreeMap::new();
        marks.insert(
            "claude".to_string(),
            AgentMarks {
                read_up_to: 3,
                next_id: 99,
            },
        );
        store.save_marks(&marks).unwrap();

        let loaded = store.load();
        let records: Vec<AgentRecord> = loaded.conversations["claude"]
            .records
            .iter()
            .cloned()
            .collect();
        assert_eq!(records, expected);
        assert_eq!(loaded.marks, marks);
        assert_eq!(
            loaded.conversations["claude"].bytes(),
            fs::metadata(store.conversation_path("claude"))
                .unwrap()
                .len()
        );
        // Nada fora da pasta dos agentes.
        let top: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(top, vec!["agents".to_string()]);
    }

    #[test]
    fn conversation_file_is_capped_by_lines_and_bytes() {
        let dir = TempDir::new("caps");
        let (_registry, store) = open_store(&dir.0);
        let mut conversation = Conversation::default();
        for id in 1..=(MAX_LINES as u64 + 1) {
            store
                .append("codex", &mut conversation, message(id, "curta"))
                .unwrap();
            assert!(conversation.records.len() <= MAX_LINES);
        }
        let file = fs::read_to_string(store.conversation_path("codex")).unwrap();
        assert_eq!(file.lines().count(), KEEP_LINES);
        assert_eq!(conversation.records.len(), KEEP_LINES);
        // Ficam os mais recentes.
        assert_eq!(conversation.last_id(), MAX_LINES as u64 + 1);
        assert_eq!(
            conversation.records.front().unwrap().id,
            MAX_LINES as u64 + 2 - KEEP_LINES as u64
        );

        // Por bytes: mensagens de 4000 chars de 3 bytes cada (~12 KiB).
        let big = "€".repeat(MESSAGE_MAX_CHARS);
        let mut conversation = Conversation::default();
        let mut max_seen = 0;
        for id in 1..=300u64 {
            store
                .append("gemini", &mut conversation, message(id, &big))
                .unwrap();
            let len = fs::metadata(store.conversation_path("gemini"))
                .unwrap()
                .len();
            max_seen = max_seen.max(len);
            assert_eq!(len, conversation.bytes());
        }
        assert!(max_seen <= MAX_BYTES, "{max_seen}");
        assert!(conversation.bytes() <= MAX_BYTES);
        assert_eq!(conversation.last_id(), 300);
        // A reescrita e atomica: nao sobra temporario.
        assert!(!store.dir().join("gemini.jsonl.tmp").exists());
    }

    #[test]
    fn torn_last_line_is_dropped_and_the_file_rewritten_clean() {
        let dir = TempDir::new("torn");
        let (_registry, store) = open_store(&dir.0);
        let mut conversation = Conversation::default();
        store
            .append("claude", &mut conversation, message(1, "um"))
            .unwrap();
        store
            .append("claude", &mut conversation, message(2, "dois"))
            .unwrap();
        // O processo morreu a meio da terceira linha.
        let mut file = OpenOptions::new()
            .append(true)
            .open(store.conversation_path("claude"))
            .unwrap();
        file.write_all(br#"{"id":3,"ts":1,"kind":"mess"#).unwrap();
        drop(file);
        // E alguem meteu lixo e um registo acima dos tectos.
        let loaded = store.load();
        let mut conversation = loaded.conversations.into_values().next().unwrap();
        assert_eq!(conversation.records.len(), 2);
        store
            .append("claude", &mut conversation, message(3, "três"))
            .unwrap();
        let reloaded = store.load();
        let ids: Vec<u64> = reloaded.conversations["claude"]
            .records
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
        // Registos acima dos tectos, ou com nome de ficheiro estranho, ficam
        // de fora.
        fs::write(
            store.dir().join("gemini.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"id":1,"ts":1,"kind":"message","text":"x".repeat(MESSAGE_MAX_CHARS + 1)}),
                json!({"id":2,"ts":1,"kind":"user","text":"ok"}),
            ),
        )
        .unwrap();
        fs::write(store.dir().join("Bad Name.jsonl"), "{}\n").unwrap();
        let loaded = store.load();
        assert_eq!(loaded.conversations["gemini"].records.len(), 1);
        assert!(!loaded.conversations.contains_key("Bad Name"));
    }

    #[test]
    fn clear_removes_conversations_but_keeps_the_hub_credentials() {
        let dir = TempDir::new("clear");
        let (_registry, store) = open_store(&dir.0);
        let mut conversation = Conversation::default();
        store
            .append("claude", &mut conversation, message(1, "um"))
            .unwrap();
        fs::write(store.dir().join("token"), "t").unwrap();
        fs::write(store.dir().join("pipe"), "p").unwrap();
        store.clear().unwrap();
        assert!(!store.conversation_path("claude").exists());
        assert!(store.dir().join("token").exists());
        assert!(store.dir().join("pipe").exists());
        assert!(store.load().conversations.is_empty());
    }

    #[test]
    fn iso_timestamps_are_utc_calendar_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_790_155_845_000), "2026-09-23T09:30:45Z");
    }
}
