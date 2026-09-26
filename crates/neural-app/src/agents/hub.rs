//! O hub: o estado dos agentes dentro do NeuralIA que esta aberto.
//!
//! Duas portas dao para aqui:
//!
//! - a das pontes (`NeuralIA.exe --mcp`), sempre atraves de uma
//!   [`HubSession`], que so abre depois de o token certo chegar; cada
//!   pedido e validado de novo, com os mesmos tectos da ponte, e contado
//!   contra os limites por agente (`MESSAGES_PER_MINUTE`,
//!   `MAX_PENDING_QUESTIONS`...);
//! - a da interface (o painel «Agentes»), pelos metodos publicos de
//!   [`AgentHub`] — `answer_question`, `user_message`, `share_context`,
//!   `mark_read`, `snapshot`...
//!
//! O que muda e anunciado por [`AgentEvent`]s, entregues fora do lock.
//! Regra de privacidade: um agente so recebe o que o utilizador escreveu no
//! painel para ELE (`user_message`) ou partilhou com um botao
//! (`share_context`). Nada do que se escreve nas paginas passa por aqui.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::store::{
    AgentMarks, AgentRecord, AnswerVia, CloseReason, Conversation, ConversationStore, MAX_AGENTS,
    RecordBody, SharedContext, iso_utc, now_ms,
};
use super::tools::{
    ANSWER_MAX_CHARS, MAX_SAFE_ID, MESSAGE_MAX_CHARS, ToolCall, ToolError, agent_display_name,
    bounded_text, parse_tool_call, sanitize_agent_name,
};
use crate::ipc::constant_time_eq;

/// `send_message` + `ask_user`, por agente, em qualquer janela de 60 s.
pub(crate) const MESSAGES_PER_MINUTE: usize = 30;
pub(crate) const STATUS_PER_MINUTE: usize = 60;
pub(crate) const POLLS_PER_MINUTE: usize = 120;
/// Perguntas a espera de resposta, por agente.
pub(crate) const MAX_PENDING_QUESTIONS: usize = 5;
/// `get_user_messages` com `since_id`: no maximo isto por chamada.
pub(crate) const USER_MESSAGES_PAGE: usize = 50;
/// `get_user_messages` sem `since_id`: as mais recentes.
pub(crate) const USER_MESSAGES_FIRST_PAGE: usize = 20;
/// Tecto do JSON das mensagens numa resposta (a primeira passa sempre).
pub(crate) const USER_MESSAGES_BUDGET_BYTES: usize = 256 * 1024;
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Versao do protocolo entre a ponte e o hub.
pub(crate) const HUB_PROTOCOL_VERSION: u64 = 1;
/// Tecto de uma linha da ponte para o hub.
pub(crate) const HUB_MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Tecto de uma linha do hub para a ponte.
pub(crate) const HUB_MAX_REPLY_BYTES: usize = 2 * 1024 * 1024;
/// Quanto um `wait` pode segurar a ligacao.
pub(crate) const WAIT_MAX_MS: u64 = 2_000;
/// Tamanho do token em hexadecimal (256 bits).
pub(crate) const TOKEN_HEX_LEN: usize = 64;

pub(crate) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
pub(crate) type Notifier = Arc<dyn Fn(AgentEvent) + Send + Sync>;
pub(crate) type ConnectionId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentStatusLine {
    pub(crate) text: String,
    pub(crate) progress: Option<u8>,
}

/// Uma pergunta por responder, como o cartao a mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionView {
    pub(crate) id: u64,
    pub(crate) agent: String,
    pub(crate) display_name: String,
    pub(crate) text: String,
    /// Os botoes; nunca vazio.
    pub(crate) options: Vec<String>,
    /// Mostra o «Responder» (texto livre).
    pub(crate) allow_text: bool,
    /// O registo da pergunta na conversa.
    pub(crate) record_id: u64,
    pub(crate) deadline: Instant,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuestionOutcome {
    Answered { answer: String, via: AnswerVia },
    Closed(CloseReason),
}

/// O que o utilizador fez no cartao.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuestionAnswer {
    /// Carregou no botao com este indice de `QuestionView::options`.
    Button(usize),
    /// Escreveu no «Responder».
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HubServerState {
    /// O canal ainda nao existe (a ler o disco).
    Starting,
    /// A aceitar pontes.
    Running,
    /// Sem canal: outra janela do NeuralIA ja o tem, ou o Windows recusou.
    Unavailable(String),
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentSummary {
    pub(crate) name: String,
    pub(crate) display_name: String,
    pub(crate) connected: bool,
    pub(crate) status: Option<AgentStatusLine>,
    pub(crate) unread: u32,
    pub(crate) last_id: u64,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HubSnapshot {
    /// Por nome.
    pub(crate) agents: Vec<AgentSummary>,
    /// Por ordem de chegada.
    pub(crate) questions: Vec<QuestionView>,
    pub(crate) server: HubServerState,
    /// O disco ja foi lido.
    pub(crate) loaded: bool,
    /// A ultima falha a gravar uma conversa, se houve.
    pub(crate) store_error: Option<String>,
}

/// O que o hub anuncia a interface. Entregue fora do lock, pela ordem em que
/// aconteceu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentEvent {
    /// A primeira ponte deste agente ligou-se.
    Connected { agent: String },
    /// A ultima ponte deste agente desligou-se.
    Disconnected { agent: String },
    /// `send_message`: mostrar no painel e num aviso.
    Message { agent: String, record: AgentRecord },
    /// `ask_user`: mostrar o cartao.
    Question(QuestionView),
    /// A pergunta deixou de estar pendente (respondida, expirada,
    /// dispensada ou abandonada pelo agente): tirar o cartao.
    QuestionClosed {
        agent: String,
        question_id: u64,
        outcome: QuestionOutcome,
    },
    /// Nova linha de estado (`None` limpa).
    Status {
        agent: String,
        status: Option<AgentStatusLine>,
    },
    /// Mudou muita coisa de uma vez (disco lido, conversas apagadas, estado
    /// do canal): redesenhar a partir de `snapshot()`.
    Changed,
}

struct RateWindow {
    hits: VecDeque<Instant>,
    limit: usize,
}

impl RateWindow {
    fn new(limit: usize) -> Self {
        Self {
            hits: VecDeque::new(),
            limit,
        }
    }

    /// Conta um pedido, ou diz daqui a quanto tempo ha lugar.
    fn try_hit(&mut self, now: Instant) -> Result<(), Duration> {
        while self
            .hits
            .front()
            .is_some_and(|&hit| now.saturating_duration_since(hit) >= RATE_WINDOW)
        {
            self.hits.pop_front();
        }
        if self.hits.len() >= self.limit {
            let oldest = self.hits.front().copied().unwrap_or(now);
            return Err(RATE_WINDOW.saturating_sub(now.saturating_duration_since(oldest)));
        }
        self.hits.push_back(now);
        Ok(())
    }
}

struct AgentEntry {
    conversation: Conversation,
    marks: AgentMarks,
    connections: BTreeSet<ConnectionId>,
    status: Option<AgentStatusLine>,
    messages: RateWindow,
    statuses: RateWindow,
    polls: RateWindow,
}

impl AgentEntry {
    fn new(conversation: Conversation, marks: AgentMarks) -> Self {
        Self {
            conversation,
            marks,
            connections: BTreeSet::new(),
            status: None,
            messages: RateWindow::new(MESSAGES_PER_MINUTE),
            statuses: RateWindow::new(STATUS_PER_MINUTE),
            polls: RateWindow::new(POLLS_PER_MINUTE),
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self
            .marks
            .next_id
            .max(self.conversation.last_id() + 1)
            .max(1);
        self.marks.next_id = id + 1;
        id
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn unread(&self) -> u32 {
        let count = self
            .conversation
            .records
            .iter()
            .filter(|record| record.id > self.marks.read_up_to && record.is_from_agent())
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }
}

enum QuestionState {
    Pending,
    Done(QuestionOutcome),
}

struct Question {
    view: QuestionView,
    connection: ConnectionId,
    state: QuestionState,
}

struct HubState {
    agents: BTreeMap<String, AgentEntry>,
    questions: BTreeMap<u64, Question>,
    connections: BTreeMap<ConnectionId, String>,
    next_question: u64,
    next_connection: ConnectionId,
    server: HubServerState,
    loaded: bool,
    store_error: Option<String>,
}

struct HubInner {
    state: Mutex<HubState>,
    /// Acorda quem espera por uma resposta.
    changed: Condvar,
    store: ConversationStore,
    notify: Notifier,
    clock: Clock,
}

/// O hub. Barato de clonar; todos os clones sao o mesmo hub.
#[derive(Clone)]
pub(crate) struct AgentHub {
    inner: Arc<HubInner>,
}

impl std::fmt::Debug for AgentHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHub")
            .field("dir", &self.inner.store.dir())
            .finish_non_exhaustive()
    }
}

impl AgentHub {
    /// Um hub vazio sobre a loja `agents` (`<data_dir>/agents`, aberta pelo
    /// grant do registo). Nao le o disco: `load()` faz isso, numa thread que
    /// nao e a da janela.
    pub(crate) fn new(
        store: ConversationStore,
        notify: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) -> Self {
        Self::with_clock(store, Arc::new(notify), Arc::new(Instant::now))
    }

    pub(crate) fn with_clock(store: ConversationStore, notify: Notifier, clock: Clock) -> Self {
        Self {
            inner: Arc::new(HubInner {
                state: Mutex::new(HubState {
                    agents: BTreeMap::new(),
                    questions: BTreeMap::new(),
                    connections: BTreeMap::new(),
                    next_question: 1,
                    next_connection: 1,
                    server: HubServerState::Starting,
                    loaded: false,
                    store_error: None,
                }),
                changed: Condvar::new(),
                store,
                notify,
                clock,
            }),
        }
    }

    pub(crate) fn dir(&self) -> PathBuf {
        self.inner.store.dir().to_path_buf()
    }

    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn now(&self) -> Instant {
        (self.inner.clock)()
    }

    /// Corre `f` com o estado trancado e entrega os eventos depois de o
    /// soltar: quem os recebe pode voltar a chamar o hub.
    fn with_state<R>(&self, f: impl FnOnce(&mut HubState, &mut Vec<AgentEvent>) -> R) -> R {
        let mut events = Vec::new();
        let result = {
            let mut state = self.lock();
            f(&mut state, &mut events)
        };
        if !events.is_empty() {
            self.inner.changed.notify_all();
        }
        for event in events {
            (self.inner.notify)(event);
        }
        result
    }

    /// Le as conversas do disco. Corre uma vez, antes de o canal abrir.
    pub(crate) fn load(&self) {
        let loaded = self.inner.store.load();
        self.with_state(|state, events| {
            let mut marks = loaded.marks;
            for (agent, conversation) in loaded.conversations {
                let mark = marks.remove(&agent).unwrap_or_default();
                state
                    .agents
                    .insert(agent, AgentEntry::new(conversation, mark));
            }
            // Agentes sem conversa (apagada) mas com ids ja dados.
            for (agent, mark) in marks {
                if state.agents.len() >= MAX_AGENTS {
                    break;
                }
                state
                    .agents
                    .entry(agent)
                    .or_insert_with(|| AgentEntry::new(Conversation::default(), mark));
            }
            state.loaded = true;
            events.push(AgentEvent::Changed);
        });
    }

    pub(crate) fn set_server_state(&self, server: HubServerState) {
        self.with_state(|state, events| {
            state.server = server;
            events.push(AgentEvent::Changed);
        });
    }

    // ---- lado das pontes -------------------------------------------------

    /// Uma ponte autenticada pede para falar como `agent`.
    pub(crate) fn connect(&self, agent: &str) -> Result<ConnectionId, String> {
        if sanitize_agent_name(agent).as_deref() != Some(agent) {
            return Err("Nome de agente inválido.".into());
        }
        self.with_state(|state, events| {
            ensure_agent(state, agent)?;
            let id = state.next_connection;
            state.next_connection += 1;
            state.connections.insert(id, agent.to_string());
            let entry = state.agents.get_mut(agent).expect("ensured");
            let first = entry.connections.is_empty();
            entry.connections.insert(id);
            if first {
                events.push(AgentEvent::Connected {
                    agent: agent.to_string(),
                });
            }
            Ok(id)
        })
    }

    /// A ponte foi-se. As perguntas dela ficam sem ninguem a quem responder:
    /// fecham-se.
    pub(crate) fn disconnect(&self, connection: ConnectionId) {
        let store = self.inner.store.clone();
        self.with_state(|state, events| {
            let Some(agent) = state.connections.remove(&connection) else {
                return;
            };
            let orphaned: Vec<u64> = state
                .questions
                .iter()
                .filter(|(_, q)| q.connection == connection)
                .map(|(&id, _)| id)
                .collect();
            for id in orphaned {
                if let Some(question) = state.questions.remove(&id)
                    && matches!(question.state, QuestionState::Pending)
                {
                    close_question(
                        state,
                        &store,
                        events,
                        &question.view,
                        QuestionOutcome::Closed(CloseReason::Cancelled),
                    );
                }
            }
            let Some(entry) = state.agents.get_mut(&agent) else {
                return;
            };
            entry.connections.remove(&connection);
            if entry.connections.is_empty() {
                if entry.status.take().is_some() {
                    events.push(AgentEvent::Status {
                        agent: agent.clone(),
                        status: None,
                    });
                }
                events.push(AgentEvent::Disconnected { agent });
            }
        });
    }

    /// Um pedido de ferramenta de uma ponte. `ask_user` so regista a
    /// pergunta e devolve `{question_id}`; a espera e `wait_question`.
    pub(crate) fn call(&self, connection: ConnectionId, call: ToolCall) -> Result<Value, String> {
        let now = self.now();
        let store = self.inner.store.clone();
        self.with_state(|state, events| {
            let agent = state
                .connections
                .get(&connection)
                .cloned()
                .ok_or_else(|| "Ligação desconhecida.".to_string())?;
            match call {
                ToolCall::SendMessage { text, title } => {
                    let entry = state.agents.get_mut(&agent).expect("connected agent");
                    entry.messages.try_hit(now).map_err(rate_error)?;
                    let record = AgentRecord {
                        id: entry.next_id(),
                        ts_ms: now_ms(),
                        body: RecordBody::AgentMessage { title, text },
                    };
                    let id = record.id;
                    append(state, &store, &agent, record.clone());
                    events.push(AgentEvent::Message { agent, record });
                    Ok(json!({ "delivered": true, "id": id }))
                }
                ToolCall::AskUser {
                    question,
                    options,
                    allow_text,
                    timeout_seconds,
                } => {
                    let pending = state
                        .questions
                        .values()
                        .filter(|q| q.view.agent == agent && matches!(q.state, QuestionState::Pending))
                        .count();
                    if pending >= MAX_PENDING_QUESTIONS {
                        return Err(format!(
                            "Já há {MAX_PENDING_QUESTIONS} perguntas à espera de resposta; espere por uma delas. / {MAX_PENDING_QUESTIONS} questions are already pending."
                        ));
                    }
                    let entry = state.agents.get_mut(&agent).expect("connected agent");
                    entry.messages.try_hit(now).map_err(rate_error)?;
                    let question_id = state.next_question;
                    state.next_question += 1;
                    let record = AgentRecord {
                        id: entry.next_id(),
                        ts_ms: now_ms(),
                        body: RecordBody::AgentQuestion {
                            question_id,
                            text: question.clone(),
                            options: options.clone(),
                            allow_text,
                        },
                    };
                    let view = QuestionView {
                        id: question_id,
                        agent: agent.clone(),
                        display_name: agent_display_name(&agent),
                        text: question,
                        options,
                        allow_text,
                        record_id: record.id,
                        deadline: now + Duration::from_secs(u64::from(timeout_seconds)),
                    };
                    append(state, &store, &agent, record);
                    state.questions.insert(
                        question_id,
                        Question {
                            view: view.clone(),
                            connection,
                            state: QuestionState::Pending,
                        },
                    );
                    events.push(AgentEvent::Question(view));
                    Ok(json!({ "question_id": question_id }))
                }
                ToolCall::GetUserMessages { since_id } => {
                    let entry = state.agents.get_mut(&agent).expect("connected agent");
                    entry.polls.try_hit(now).map_err(rate_error)?;
                    Ok(user_messages_page(&entry.conversation, since_id))
                }
                ToolCall::SetStatus { text, progress } => {
                    let entry = state.agents.get_mut(&agent).expect("connected agent");
                    entry.statuses.try_hit(now).map_err(rate_error)?;
                    let status = (!text.is_empty() || progress.is_some())
                        .then_some(AgentStatusLine { text, progress });
                    if entry.status != status {
                        entry.status = status.clone();
                        events.push(AgentEvent::Status { agent, status });
                    }
                    Ok(json!({ "ok": true }))
                }
            }
        })
    }

    /// Espera ate `max_wait` pela resposta a uma pergunta desta ligacao.
    /// Devolve `{"state":"pending"}` ou o desfecho, que so se entrega uma vez.
    pub(crate) fn wait_question(
        &self,
        connection: ConnectionId,
        question_id: u64,
        max_wait: Duration,
    ) -> Result<Value, String> {
        let started = Instant::now();
        let store = self.inner.store.clone();
        let mut events = Vec::new();
        let mut state = self.lock();
        let result = loop {
            let now = self.now();
            let Some(question) = state.questions.get(&question_id) else {
                break Err("Pergunta desconhecida.".to_string());
            };
            if question.connection != connection {
                break Err("Pergunta desconhecida.".to_string());
            }
            if matches!(question.state, QuestionState::Pending) && now >= question.view.deadline {
                let view = question.view.clone();
                close_question(
                    &mut state,
                    &store,
                    &mut events,
                    &view,
                    QuestionOutcome::Closed(CloseReason::TimedOut),
                );
                if let Some(question) = state.questions.get_mut(&question_id) {
                    question.state =
                        QuestionState::Done(QuestionOutcome::Closed(CloseReason::TimedOut));
                }
                continue;
            }
            if let QuestionState::Done(outcome) = &question.state {
                let value = outcome_json(outcome);
                state.questions.remove(&question_id);
                break Ok(value);
            }
            let budget = max_wait.saturating_sub(started.elapsed());
            let until_deadline = question.view.deadline.saturating_duration_since(now);
            let wait = budget.min(until_deadline);
            if budget.is_zero() {
                break Ok(json!({ "state": "pending" }));
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, wait.max(Duration::from_millis(1)))
                .map(|(guard, _)| guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        };
        drop(state);
        for event in events {
            (self.inner.notify)(event);
        }
        result
    }

    /// O agente desistiu da pergunta (`notifications/cancelled`, EOF).
    pub(crate) fn cancel_question(&self, connection: ConnectionId, question_id: u64) {
        let store = self.inner.store.clone();
        self.with_state(|state, events| {
            let Some(question) = state.questions.get(&question_id) else {
                return;
            };
            if question.connection != connection {
                return;
            }
            let question = state.questions.remove(&question_id).expect("present");
            if matches!(question.state, QuestionState::Pending) {
                close_question(
                    state,
                    &store,
                    events,
                    &question.view,
                    QuestionOutcome::Closed(CloseReason::Cancelled),
                );
            }
        });
    }
}

/// A API do painel «Agentes». Ate a interface existir, so os testes a usam;
/// o `allow` sai quando o painel a chamar.
#[cfg_attr(not(test), allow(dead_code))]
impl AgentHub {
    /// O utilizador respondeu no cartao. `Button(i)` so vale para um botao
    /// que existe; `Text` so quando a pergunta o permite.
    pub(crate) fn answer_question(
        &self,
        question_id: u64,
        answer: QuestionAnswer,
    ) -> Result<(), String> {
        let store = self.inner.store.clone();
        self.with_state(|state, events| {
            let question = state
                .questions
                .get(&question_id)
                .filter(|q| matches!(q.state, QuestionState::Pending))
                .ok_or_else(|| "Esta pergunta já não está à espera de resposta.".to_string())?;
            let (text, via) = match answer {
                QuestionAnswer::Button(index) => (
                    question
                        .view
                        .options
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "Botão inexistente.".to_string())?,
                    AnswerVia::Button,
                ),
                QuestionAnswer::Text(raw) => {
                    if !question.view.allow_text {
                        return Err("Esta pergunta só aceita os botões.".into());
                    }
                    (
                        bounded_text(&raw, "resposta", 1, ANSWER_MAX_CHARS)?,
                        AnswerVia::Text,
                    )
                }
            };
            let view = question.view.clone();
            let outcome = QuestionOutcome::Answered { answer: text, via };
            close_question(state, &store, events, &view, outcome.clone());
            if let Some(question) = state.questions.get_mut(&question_id) {
                question.state = QuestionState::Done(outcome);
            }
            Ok(())
        })
    }

    /// O utilizador fechou o cartao sem responder.
    pub(crate) fn dismiss_question(&self, question_id: u64) -> Result<(), String> {
        let store = self.inner.store.clone();
        self.with_state(|state, events| {
            let view = state
                .questions
                .get(&question_id)
                .filter(|q| matches!(q.state, QuestionState::Pending))
                .map(|q| q.view.clone())
                .ok_or_else(|| "Esta pergunta já não está à espera de resposta.".to_string())?;
            let outcome = QuestionOutcome::Closed(CloseReason::Dismissed);
            close_question(state, &store, events, &view, outcome.clone());
            if let Some(question) = state.questions.get_mut(&question_id) {
                question.state = QuestionState::Done(outcome);
            }
            Ok(())
        })
    }

    /// O utilizador escreveu `text` ao agente, na caixa do painel. E so isto
    /// (e `share_context`) que `get_user_messages` devolve.
    pub(crate) fn user_message(&self, agent: &str, text: &str) -> Result<AgentRecord, String> {
        let text = bounded_text(text, "mensagem", 1, MESSAGE_MAX_CHARS)?;
        self.user_record(agent, RecordBody::UserMessage { text })
    }

    /// O utilizador carregou em «esta página» ou «estas abas». Nada e
    /// partilhado sem este clique.
    pub(crate) fn share_context(
        &self,
        agent: &str,
        context: SharedContext,
    ) -> Result<AgentRecord, String> {
        let context = context
            .sanitized()
            .ok_or_else(|| "Nada para partilhar.".to_string())?;
        self.user_record(agent, RecordBody::UserShare(context))
    }

    fn user_record(&self, agent: &str, body: RecordBody) -> Result<AgentRecord, String> {
        if sanitize_agent_name(agent).as_deref() != Some(agent) {
            return Err("Nome de agente inválido.".into());
        }
        let store = self.inner.store.clone();
        let (record, marks) = self.with_state(|state, _| {
            ensure_agent(state, agent)?;
            let entry = state.agents.get_mut(agent).expect("ensured");
            let record = AgentRecord {
                id: entry.next_id(),
                ts_ms: now_ms(),
                body,
            };
            // Quem escreve ja leu o que estava acima.
            entry.marks.read_up_to = record.id;
            append(state, &store, agent, record.clone());
            Ok::<_, String>((record, all_marks(state)))
        })?;
        self.save_marks(&marks);
        Ok(record)
    }

    /// O painel mostrou a conversa: o badge deste agente volta a zero.
    pub(crate) fn mark_read(&self, agent: &str) {
        let marks = self.with_state(|state, _| {
            let entry = state.agents.get_mut(agent)?;
            let last = entry.conversation.last_id();
            if entry.marks.read_up_to == last {
                return None;
            }
            entry.marks.read_up_to = last;
            Some(all_marks(state))
        });
        if let Some(marks) = marks {
            self.save_marks(&marks);
        }
    }

    fn save_marks(&self, marks: &BTreeMap<String, AgentMarks>) {
        if let Err(error) = self.inner.store.save_marks(marks) {
            let message = format!("Não foi possível gravar o estado dos agentes: {error}");
            eprintln!("[agents] {message}");
            self.lock().store_error = Some(message);
        }
    }

    pub(crate) fn conversation(&self, agent: &str) -> Vec<AgentRecord> {
        self.lock()
            .agents
            .get(agent)
            .map(|entry| entry.conversation.records.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn pending_questions(&self) -> Vec<QuestionView> {
        self.lock()
            .questions
            .values()
            .filter(|q| matches!(q.state, QuestionState::Pending))
            .map(|q| q.view.clone())
            .collect()
    }

    pub(crate) fn unread(&self, agent: &str) -> u32 {
        self.lock().agents.get(agent).map_or(0, AgentEntry::unread)
    }

    /// Soma de todos os «não lidos»: o numero do badge do botao «Agentes».
    pub(crate) fn total_unread(&self) -> u32 {
        self.lock()
            .agents
            .values()
            .map(AgentEntry::unread)
            .fold(0u32, u32::saturating_add)
    }

    pub(crate) fn snapshot(&self) -> HubSnapshot {
        let state = self.lock();
        HubSnapshot {
            agents: state
                .agents
                .iter()
                .map(|(name, entry)| AgentSummary {
                    name: name.clone(),
                    display_name: agent_display_name(name),
                    connected: !entry.connections.is_empty(),
                    status: entry.status.clone(),
                    unread: entry.unread(),
                    last_id: entry.conversation.last_id(),
                })
                .collect(),
            questions: state
                .questions
                .values()
                .filter(|q| matches!(q.state, QuestionState::Pending))
                .map(|q| q.view.clone())
                .collect(),
            server: state.server.clone(),
            loaded: state.loaded,
            store_error: state.store_error.clone(),
        }
    }

    /// Fecha as perguntas cujo prazo passou (a ponte que as espera tambem o
    /// faz; isto e para a interface nao mostrar um cartao ja morto).
    pub(crate) fn sweep(&self) {
        let now = self.now();
        let store = self.inner.store.clone();
        self.with_state(|state, events| {
            let expired: Vec<QuestionView> = state
                .questions
                .values()
                .filter(|q| matches!(q.state, QuestionState::Pending) && now >= q.view.deadline)
                .map(|q| q.view.clone())
                .collect();
            for view in expired {
                let outcome = QuestionOutcome::Closed(CloseReason::TimedOut);
                close_question(state, &store, events, &view, outcome.clone());
                if let Some(question) = state.questions.get_mut(&view.id) {
                    question.state = QuestionState::Done(outcome);
                }
            }
        });
    }

    /// Ctrl+Shift+Delete: apaga as conversas do disco e da memoria, debaixo
    /// do lock (uma ponte que escreva entretanto nao ressuscita o ficheiro
    /// a meio). As perguntas pendentes continuam (o agente esta a espera
    /// delas).
    pub(crate) fn clear_conversations(&self) -> Result<(), String> {
        let store = self.inner.store.clone();
        let mut result = Ok(());
        let marks = self.with_state(|state, events| {
            result = store.clear().map_err(|error| error.to_string());
            for entry in state.agents.values_mut() {
                let last = entry.conversation.last_id();
                entry.marks.next_id = entry.marks.next_id.max(last + 1);
                entry.marks.read_up_to = entry.marks.next_id.saturating_sub(1);
                entry.conversation = Conversation::default();
            }
            if result.is_ok() {
                state.store_error = None;
            }
            events.push(AgentEvent::Changed);
            all_marks(state)
        });
        self.save_marks(&marks);
        result
    }
}

fn ensure_agent(state: &mut HubState, agent: &str) -> Result<(), String> {
    if state.agents.contains_key(agent) {
        return Ok(());
    }
    if state.agents.len() >= MAX_AGENTS {
        return Err(format!(
            "O NeuralIA já conhece {MAX_AGENTS} agentes; use um dos nomes existentes."
        ));
    }
    state.agents.insert(
        agent.to_string(),
        AgentEntry::new(Conversation::default(), AgentMarks::default()),
    );
    Ok(())
}

fn all_marks(state: &HubState) -> BTreeMap<String, AgentMarks> {
    state
        .agents
        .iter()
        .map(|(agent, entry)| (agent.clone(), entry.marks))
        .collect()
}

/// Grava o registo. Uma falha de disco nao impede a entrega no ecra: fica
/// registada em `store_error` para o painel a mostrar.
fn append(state: &mut HubState, store: &ConversationStore, agent: &str, record: AgentRecord) {
    let Some(entry) = state.agents.get_mut(agent) else {
        return;
    };
    if let Err(error) = store.append(agent, &mut entry.conversation, record) {
        let message = format!("Não foi possível gravar a conversa com {agent}: {error}");
        eprintln!("[agents] {message}");
        state.store_error = Some(message);
    }
}

fn close_question(
    state: &mut HubState,
    store: &ConversationStore,
    events: &mut Vec<AgentEvent>,
    view: &QuestionView,
    outcome: QuestionOutcome,
) {
    let body = match &outcome {
        QuestionOutcome::Answered { answer, via } => RecordBody::UserAnswer {
            question_id: view.id,
            text: answer.clone(),
            via: *via,
        },
        QuestionOutcome::Closed(reason) => RecordBody::QuestionClosed {
            question_id: view.id,
            reason: *reason,
        },
    };
    if let Some(entry) = state.agents.get_mut(&view.agent) {
        let record = AgentRecord {
            id: entry.next_id(),
            ts_ms: now_ms(),
            body,
        };
        append(state, store, &view.agent, record);
    }
    events.push(AgentEvent::QuestionClosed {
        agent: view.agent.clone(),
        question_id: view.id,
        outcome,
    });
}

fn outcome_json(outcome: &QuestionOutcome) -> Value {
    match outcome {
        QuestionOutcome::Answered { answer, via } => {
            json!({"state": "answered", "answer": answer, "via": via.wire()})
        }
        QuestionOutcome::Closed(reason) => json!({"state": reason.wire()}),
    }
}

fn rate_error(retry: Duration) -> String {
    let secs = retry.as_secs().max(1);
    format!(
        "Limite de pedidos deste agente atingido; tente de novo daqui a {secs} s. / Rate limit reached; retry in {secs} s."
    )
}

fn user_messages_page(conversation: &Conversation, since_id: Option<u64>) -> Value {
    let candidates: Vec<&AgentRecord> = conversation
        .records
        .iter()
        .filter(|record| record.is_for_agent() && since_id.is_none_or(|since| record.id > since))
        .collect();
    let (window, limit_more) = match since_id {
        // Sem `since_id`: as mais recentes, para contexto.
        None => {
            let start = candidates.len().saturating_sub(USER_MESSAGES_FIRST_PAGE);
            (&candidates[start..], false)
        }
        Some(_) => {
            let end = candidates.len().min(USER_MESSAGES_PAGE);
            (&candidates[..end], candidates.len() > end)
        }
    };
    let mut messages = Vec::new();
    let mut used = 0usize;
    let mut cut = false;
    for record in window {
        let value = user_message_json(record);
        let size = value.to_string().len();
        if !messages.is_empty() && used + size > USER_MESSAGES_BUDGET_BYTES {
            cut = true;
            break;
        }
        used += size;
        messages.push((record.id, value));
    }
    let last_id = match messages.last() {
        Some((id, _)) => *id,
        None => since_id.unwrap_or(0).max(conversation.last_id()),
    };
    json!({
        "messages": messages.into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
        "last_id": last_id.min(MAX_SAFE_ID),
        "has_more": limit_more || cut,
    })
}

fn user_message_json(record: &AgentRecord) -> Value {
    let mut value = match &record.body {
        RecordBody::UserMessage { text } => json!({"kind": "message", "text": text}),
        RecordBody::UserShare(SharedContext::Page {
            title,
            url,
            excerpt,
        }) => {
            let mut value = json!({"kind": "page", "title": title, "url": url});
            if let Some(excerpt) = excerpt {
                value["excerpt"] = json!(excerpt);
            }
            value
        }
        RecordBody::UserShare(SharedContext::Tabs(tabs)) => json!({
            "kind": "tabs",
            "tabs": tabs.iter().map(|t| json!({"title": t.title, "url": t.url})).collect::<Vec<_>>(),
        }),
        _ => json!({"kind": "other"}),
    };
    value["id"] = json!(record.id);
    value["at"] = json!(iso_utc(record.ts_ms));
    value
}

// ---- protocolo ponte <-> hub ---------------------------------------------

/// Uma resposta do hub a uma linha da ponte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionReply {
    pub(crate) line: String,
    /// Fechar a ligacao depois de responder.
    pub(crate) close: bool,
}

/// Uma ligacao de uma ponte ao hub. A primeira linha tem de ser o `hello`
/// com o token certo; qualquer outra coisa fecha a ligacao. Desligar (ou
/// largar a sessao) fecha as perguntas pendentes dela.
///
/// Linhas (JSON, uma por linha, uma resposta por pedido):
///
/// - `{"t":"hello","v":1,"token":"<64 hex>","agent":"claude"}`
/// - `{"t":"call","tool":"send_message","args":{...}}`
/// - `{"t":"wait","question_id":7,"wait_ms":1000}`
/// - `{"t":"cancel","question_id":7}`
/// - `{"t":"ping"}`
///
/// Respostas: `{"ok":true,"value":{...}}` ou `{"ok":false,"error":"..."}`.
pub(crate) struct HubSession {
    hub: AgentHub,
    token: String,
    connection: Option<ConnectionId>,
}

impl HubSession {
    pub(crate) fn new(hub: AgentHub, token: String) -> Self {
        Self {
            hub,
            token,
            connection: None,
        }
    }

    pub(crate) fn handle_line(&mut self, line: &str) -> SessionReply {
        let authenticated = self.connection.is_some();
        if line.len() > HUB_MAX_REQUEST_BYTES {
            return fail("Pedido grande demais.", true);
        }
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(line) else {
            return fail("Pedido inválido.", !authenticated);
        };
        let kind = message.get("t").and_then(Value::as_str).unwrap_or("");
        match (self.connection, kind) {
            (None, "hello") => self.hello(&message),
            (None, _) => fail("Autenticação obrigatória.", true),
            (Some(_), "hello") => fail("Já autenticado.", true),
            (Some(connection), "call") => {
                if !exact_keys(&message, &["t", "tool", "args"]) {
                    return fail("Pedido inválido.", false);
                }
                let Some(tool) = message.get("tool").and_then(Value::as_str) else {
                    return fail("Pedido inválido.", false);
                };
                match parse_tool_call(tool, message.get("args")) {
                    Ok(call) => reply(self.hub.call(connection, call)),
                    Err(ToolError::Invalid(error)) => fail(&error, false),
                    Err(ToolError::UnknownTool(name)) => {
                        let shown: String = name.chars().take(64).collect();
                        fail(&format!("Ferramenta desconhecida: {shown}"), false)
                    }
                }
            }
            (Some(connection), "wait") => {
                if !exact_keys(&message, &["t", "question_id", "wait_ms"]) {
                    return fail("Pedido inválido.", false);
                }
                let (Some(question_id), Some(wait_ms)) = (
                    message.get("question_id").and_then(Value::as_u64),
                    message.get("wait_ms").and_then(Value::as_u64),
                ) else {
                    return fail("Pedido inválido.", false);
                };
                let wait = Duration::from_millis(wait_ms.min(WAIT_MAX_MS));
                reply(self.hub.wait_question(connection, question_id, wait))
            }
            (Some(connection), "cancel") => {
                if !exact_keys(&message, &["t", "question_id"]) {
                    return fail("Pedido inválido.", false);
                }
                let Some(question_id) = message.get("question_id").and_then(Value::as_u64) else {
                    return fail("Pedido inválido.", false);
                };
                self.hub.cancel_question(connection, question_id);
                reply(Ok(json!({})))
            }
            (Some(_), "ping") if exact_keys(&message, &["t"]) => reply(Ok(json!({}))),
            (Some(_), _) => fail("Pedido desconhecido.", false),
        }
    }

    fn hello(&mut self, message: &Map<String, Value>) -> SessionReply {
        if !exact_keys(message, &["t", "v", "token", "agent"])
            || message.get("v").and_then(Value::as_u64) != Some(HUB_PROTOCOL_VERSION)
        {
            return fail("Versão do protocolo não suportada.", true);
        }
        let token = message.get("token").and_then(Value::as_str).unwrap_or("");
        if token.len() != TOKEN_HEX_LEN
            || !token.bytes().all(|b| b.is_ascii_hexdigit())
            || self.token.len() != TOKEN_HEX_LEN
            || !constant_time_eq(token.as_bytes(), self.token.as_bytes())
        {
            return fail("Token inválido.", true);
        }
        let agent = message.get("agent").and_then(Value::as_str).unwrap_or("");
        match self.hub.connect(agent) {
            Ok(connection) => {
                self.connection = Some(connection);
                reply(Ok(json!({
                    "v": HUB_PROTOCOL_VERSION,
                    "agent": agent,
                    "display_name": agent_display_name(agent),
                })))
            }
            Err(error) => fail(&error, true),
        }
    }
}

impl Drop for HubSession {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.hub.disconnect(connection);
        }
    }
}

fn exact_keys(message: &Map<String, Value>, keys: &[&str]) -> bool {
    message.len() == keys.len() && keys.iter().all(|key| message.contains_key(*key))
}

fn reply(result: Result<Value, String>) -> SessionReply {
    match result {
        Ok(value) => SessionReply {
            line: json!({"ok": true, "value": value}).to_string(),
            close: false,
        },
        Err(error) => fail(&error, false),
    }
}

fn fail(error: &str, close: bool) -> SessionReply {
    SessionReply {
        line: json!({"ok": false, "error": error}).to_string(),
        close,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::agents::store::tests::{TempDir, open_store};
    use crate::agents::tools::ASK_TIMEOUT_MIN_SECS;
    use crate::stores::AGENTS_STORE;
    use neural_core::json_store::{StoreMode, StoreRegistry};

    pub(crate) const TOKEN: &str =
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// Relogio que so anda quando o teste manda.
    #[derive(Clone)]
    pub(crate) struct ManualClock(pub(crate) Arc<Mutex<Instant>>);

    impl ManualClock {
        pub(crate) fn new() -> Self {
            Self(Arc::new(Mutex::new(Instant::now())))
        }

        pub(crate) fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }

        pub(crate) fn clock(&self) -> Clock {
            let inner = Arc::clone(&self.0);
            Arc::new(move || *inner.lock().unwrap())
        }
    }

    pub(crate) struct Fixture {
        pub(crate) dir: TempDir,
        /// O registo das lojas do teste; `set_mode` liga o modo privado.
        pub(crate) registry: StoreRegistry,
        pub(crate) hub: AgentHub,
        pub(crate) clock: ManualClock,
        pub(crate) events: Arc<Mutex<Vec<AgentEvent>>>,
    }

    pub(crate) fn fixture(tag: &str) -> Fixture {
        let dir = TempDir::new(tag);
        let (registry, store) = open_store(&dir.0);
        let clock = ManualClock::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let hub = AgentHub::with_clock(
            store,
            Arc::new(move |event| sink.lock().unwrap().push(event)),
            clock.clock(),
        );
        hub.load();
        events.lock().unwrap().clear();
        Fixture {
            dir,
            registry,
            hub,
            clock,
            events,
        }
    }

    /// Um segundo hub sobre a mesma loja (o NeuralIA reaberto).
    fn reopen(f: &Fixture) -> AgentHub {
        let store = ConversationStore::open(f.registry.grant(AGENTS_STORE).unwrap()).unwrap();
        AgentHub::new(store, |_| {})
    }

    fn send(hub: &AgentHub, connection: ConnectionId, text: &str) -> Result<Value, String> {
        hub.call(
            connection,
            ToolCall::SendMessage {
                text: text.into(),
                title: None,
            },
        )
    }

    fn ask(hub: &AgentHub, connection: ConnectionId, timeout: u32) -> Result<u64, String> {
        hub.call(
            connection,
            ToolCall::AskUser {
                question: "Posso continuar?".into(),
                options: vec!["Sim".into(), "Não".into()],
                allow_text: true,
                timeout_seconds: timeout,
            },
        )
        .map(|value| value["question_id"].as_u64().unwrap())
    }

    #[test]
    fn hub_session_rejects_a_wrong_or_missing_token_before_anything_else() {
        let f = fixture("token");
        let hello =
            |token: &str| json!({"t":"hello","v":1,"token":token,"agent":"claude"}).to_string();
        let mut wrong = TOKEN.as_bytes().to_vec();
        for index in [0usize, 31, 63] {
            let mut bad = wrong.clone();
            bad[index] = if bad[index] == b'0' { b'1' } else { b'0' };
            let mut session = HubSession::new(f.hub.clone(), TOKEN.into());
            let reply = session.handle_line(&hello(std::str::from_utf8(&bad).unwrap()));
            assert!(reply.close, "{reply:?}");
            assert!(reply.line.contains("\"ok\":false"));
            assert!(session.connection.is_none());
        }
        wrong.truncate(63);
        for line in [
            hello(std::str::from_utf8(&wrong).unwrap()),
            hello(""),
            json!({"t":"hello","v":1,"agent":"claude"}).to_string(),
            json!({"t":"hello","v":2,"token":TOKEN,"agent":"claude"}).to_string(),
            json!({"t":"hello","v":1,"token":TOKEN,"agent":"claude","x":1}).to_string(),
            json!({"t":"call","tool":"send_message","args":{"text":"oi"}}).to_string(),
            json!({"t":"ping"}).to_string(),
            "não é json".into(),
        ] {
            let mut session = HubSession::new(f.hub.clone(), TOKEN.into());
            let reply = session.handle_line(&line);
            assert!(reply.close, "{line} -> {reply:?}");
            assert!(reply.line.contains("\"ok\":false"), "{line}");
        }
        // Nenhuma dessas tentativas chegou a ligar um agente.
        assert!(f.events.lock().unwrap().is_empty());
        assert!(f.hub.conversation("claude").is_empty());

        let mut session = HubSession::new(f.hub.clone(), TOKEN.into());
        let reply = session.handle_line(&hello(TOKEN));
        assert!(!reply.close);
        assert!(reply.line.contains("\"ok\":true"), "{reply:?}");
        let reply = session.handle_line(
            &json!({"t":"call","tool":"send_message","args":{"text":"oi"}}).to_string(),
        );
        assert!(reply.line.contains("\"delivered\":true"), "{reply:?}");
        // Um segundo hello na mesma ligacao fecha-a.
        assert!(session.handle_line(&hello(TOKEN)).close);
        drop(session);
        let events = f.events.lock().unwrap().clone();
        assert!(
            matches!(events.first(), Some(AgentEvent::Connected { agent }) if agent == "claude")
        );
        assert!(
            matches!(events.last(), Some(AgentEvent::Disconnected { agent }) if agent == "claude")
        );
    }

    #[test]
    fn hub_revalidates_tool_arguments_the_bridge_sent() {
        let f = fixture("revalidate");
        let mut session = HubSession::new(f.hub.clone(), TOKEN.into());
        session.handle_line(&json!({"t":"hello","v":1,"token":TOKEN,"agent":"codex"}).to_string());
        for args in [
            json!({"text": "x".repeat(MESSAGE_MAX_CHARS + 1)}),
            json!({"text": "oi", "html": "<script>"}),
            json!({"text": "\u{0000}\u{0007}"}),
        ] {
            let reply = session
                .handle_line(&json!({"t":"call","tool":"send_message","args":args}).to_string());
            assert!(reply.line.contains("\"ok\":false"), "{args} -> {reply:?}");
            assert!(!reply.close);
        }
        let reply = session.handle_line(&json!({"t":"call","tool":"rm_rf","args":{}}).to_string());
        assert!(reply.line.contains("Ferramenta desconhecida"));
        assert!(f.hub.conversation("codex").is_empty());
        // Texto com controlos chega limpo.
        session.handle_line(
            &json!({"t":"call","tool":"send_message","args":{"text":"a\u{0008}b\u{202E}c"}})
                .to_string(),
        );
        assert_eq!(
            f.hub.conversation("codex")[0].body,
            RecordBody::AgentMessage {
                title: None,
                text: "abc".into()
            }
        );
    }

    #[test]
    fn rate_limit_caps_messages_per_agent_and_recovers_after_a_minute() {
        let f = fixture("rate");
        let claude = f.hub.connect("claude").unwrap();
        // Uma segunda ponte do mesmo agente partilha o limite.
        let claude_again = f.hub.connect("claude").unwrap();
        let gemini = f.hub.connect("gemini").unwrap();
        for index in 0..MESSAGES_PER_MINUTE {
            let connection = if index % 2 == 0 { claude } else { claude_again };
            send(&f.hub, connection, &format!("m{index}")).unwrap();
        }
        let error = send(&f.hub, claude, "demais").unwrap_err();
        assert!(error.contains("Limite"), "{error}");
        assert!(
            ask(&f.hub, claude_again, 60)
                .unwrap_err()
                .contains("Limite")
        );
        // O limite e por agente: o Gemini continua.
        send(&f.hub, gemini, "ok").unwrap();
        // Recusado nao chega ao ecra nem ao disco.
        let messages = f
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, AgentEvent::Message { agent, .. } if agent == "claude"))
            .count();
        assert_eq!(messages, MESSAGES_PER_MINUTE);
        assert_eq!(f.hub.conversation("claude").len(), MESSAGES_PER_MINUTE);
        f.clock.advance(Duration::from_secs(61));
        send(&f.hub, claude, "de novo").unwrap();

        // Estado e leitura tem os seus proprios limites.
        for _ in 0..STATUS_PER_MINUTE {
            f.hub
                .call(
                    gemini,
                    ToolCall::SetStatus {
                        text: "a".into(),
                        progress: None,
                    },
                )
                .unwrap();
        }
        assert!(
            f.hub
                .call(
                    gemini,
                    ToolCall::SetStatus {
                        text: "b".into(),
                        progress: None
                    }
                )
                .is_err()
        );
        for _ in 0..POLLS_PER_MINUTE {
            f.hub
                .call(gemini, ToolCall::GetUserMessages { since_id: None })
                .unwrap();
        }
        assert!(
            f.hub
                .call(gemini, ToolCall::GetUserMessages { since_id: None })
                .is_err()
        );
    }

    #[test]
    fn pending_questions_are_capped_per_agent() {
        let f = fixture("pending");
        let claude = f.hub.connect("claude").unwrap();
        let mut ids = Vec::new();
        for _ in 0..MAX_PENDING_QUESTIONS {
            ids.push(ask(&f.hub, claude, 600).unwrap());
        }
        let error = ask(&f.hub, claude, 600).unwrap_err();
        assert!(error.contains("perguntas"), "{error}");
        assert_eq!(f.hub.pending_questions().len(), MAX_PENDING_QUESTIONS);
        f.hub
            .answer_question(ids[0], QuestionAnswer::Button(0))
            .unwrap();
        ask(&f.hub, claude, 600).unwrap();
    }

    #[test]
    fn ask_user_answer_by_button_or_text_reaches_the_waiting_bridge_once() {
        let f = fixture("answer");
        let claude = f.hub.connect("claude").unwrap();
        let question = ask(&f.hub, claude, 600).unwrap();
        let card = f
            .events
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                AgentEvent::Question(view) => Some(view.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(card.id, question);
        assert_eq!(card.display_name, "Claude");
        assert_eq!(card.options, vec!["Sim".to_string(), "Não".to_string()]);
        assert_eq!(
            f.hub
                .wait_question(claude, question, Duration::from_millis(10))
                .unwrap(),
            json!({"state":"pending"})
        );
        // Outra ligacao nao pode esperar (nem cancelar) a pergunta desta.
        let intruder = f.hub.connect("claude").unwrap();
        assert!(
            f.hub
                .wait_question(intruder, question, Duration::from_millis(1))
                .is_err()
        );
        f.hub.cancel_question(intruder, question);
        assert_eq!(f.hub.pending_questions().len(), 1);
        // Um botao que nao existe nao responde.
        assert!(
            f.hub
                .answer_question(question, QuestionAnswer::Button(2))
                .is_err()
        );
        // A resposta chega a quem esta a espera, noutra thread.
        let hub = f.hub.clone();
        let waiter =
            std::thread::spawn(move || hub.wait_question(claude, question, Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        f.hub
            .answer_question(question, QuestionAnswer::Button(1))
            .unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap(),
            json!({"state":"answered","answer":"Não","via":"button"})
        );
        // Entregue uma vez: depois disso a pergunta ja nao existe.
        assert!(
            f.hub
                .wait_question(claude, question, Duration::from_millis(1))
                .is_err()
        );
        assert!(
            f.hub
                .answer_question(question, QuestionAnswer::Button(0))
                .is_err()
        );

        // Texto livre, limpo e com tecto.
        let question = ask(&f.hub, claude, 600).unwrap();
        assert!(
            f.hub
                .answer_question(question, QuestionAnswer::Text("  ".into()))
                .is_err()
        );
        f.hub
            .answer_question(question, QuestionAnswer::Text(" pode\u{0007} sim ".into()))
            .unwrap();
        assert_eq!(
            f.hub
                .wait_question(claude, question, Duration::from_millis(1))
                .unwrap(),
            json!({"state":"answered","answer":"pode sim","via":"text"})
        );
        // Sem «Responder», texto nao passa.
        let only_buttons = f
            .hub
            .call(
                claude,
                ToolCall::AskUser {
                    question: "Qual?".into(),
                    options: vec!["A".into()],
                    allow_text: false,
                    timeout_seconds: 600,
                },
            )
            .unwrap()["question_id"]
            .as_u64()
            .unwrap();
        assert!(
            f.hub
                .answer_question(only_buttons, QuestionAnswer::Text("B".into()))
                .is_err()
        );
        let kinds: Vec<&'static str> = f
            .hub
            .conversation("claude")
            .iter()
            .map(|r| match r.body {
                RecordBody::AgentQuestion { .. } => "question",
                RecordBody::UserAnswer { .. } => "answer",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["question", "answer", "question", "answer", "question"]
        );
    }

    #[test]
    fn ask_user_times_out_and_closes_the_card() {
        let f = fixture("timeout");
        let claude = f.hub.connect("claude").unwrap();
        let question = ask(&f.hub, claude, ASK_TIMEOUT_MIN_SECS).unwrap();
        f.clock
            .advance(Duration::from_secs(u64::from(ASK_TIMEOUT_MIN_SECS) - 1));
        assert_eq!(
            f.hub
                .wait_question(claude, question, Duration::from_millis(5))
                .unwrap(),
            json!({"state":"pending"})
        );
        f.clock.advance(Duration::from_secs(1));
        assert_eq!(
            f.hub
                .wait_question(claude, question, Duration::from_millis(5))
                .unwrap(),
            json!({"state":"timed_out"})
        );
        assert!(f.hub.pending_questions().is_empty());
        // Resposta tardia recusada.
        assert!(
            f.hub
                .answer_question(question, QuestionAnswer::Button(0))
                .is_err()
        );
        let closed = f.events.lock().unwrap().iter().any(|event| {
            matches!(event, AgentEvent::QuestionClosed { question_id, outcome: QuestionOutcome::Closed(CloseReason::TimedOut), .. } if *question_id == question)
        });
        assert!(closed);
        // `sweep` fecha o cartao mesmo sem ninguem a esperar.
        let other = ask(&f.hub, claude, ASK_TIMEOUT_MIN_SECS).unwrap();
        f.clock.advance(Duration::from_secs(3_600));
        f.hub.sweep();
        assert!(f.hub.pending_questions().is_empty());
        assert_eq!(
            f.hub
                .wait_question(claude, other, Duration::from_millis(1))
                .unwrap(),
            json!({"state":"timed_out"})
        );
    }

    #[test]
    fn dismissing_a_card_closes_the_question_once_and_tells_the_bridge() {
        let f = fixture("dismiss");
        let claude = f.hub.connect("claude").unwrap();
        let question = ask(&f.hub, claude, 600).unwrap();
        f.hub.dismiss_question(question).unwrap();
        assert!(f.hub.pending_questions().is_empty());
        // Depois de dispensada nao se responde nem se dispensa outra vez.
        assert!(f.hub.dismiss_question(question).is_err());
        assert!(
            f.hub
                .answer_question(question, QuestionAnswer::Button(0))
                .is_err()
        );
        assert_eq!(
            f.hub
                .wait_question(claude, question, Duration::from_millis(1))
                .unwrap(),
            json!({"state":"dismissed"})
        );
        let closed = f.events.lock().unwrap().iter().any(|event| {
            matches!(event, AgentEvent::QuestionClosed { question_id, outcome: QuestionOutcome::Closed(CloseReason::Dismissed), .. } if *question_id == question)
        });
        assert!(closed);
        assert!(matches!(
            f.hub.conversation("claude").last().map(|r| &r.body),
            Some(RecordBody::QuestionClosed {
                reason: CloseReason::Dismissed,
                ..
            })
        ));
        assert!(f.hub.dismiss_question(99).is_err());
    }

    #[test]
    fn disconnect_cancels_the_bridges_questions_and_clears_its_status() {
        let f = fixture("disconnect");
        let claude = f.hub.connect("claude").unwrap();
        f.hub
            .call(
                claude,
                ToolCall::SetStatus {
                    text: "a trabalhar…".into(),
                    progress: Some(60),
                },
            )
            .unwrap();
        assert_eq!(
            f.hub.snapshot().agents[0].status,
            Some(AgentStatusLine {
                text: "a trabalhar…".into(),
                progress: Some(60)
            })
        );
        ask(&f.hub, claude, 600).unwrap();
        f.hub.disconnect(claude);
        assert!(f.hub.pending_questions().is_empty());
        let snapshot = f.hub.snapshot();
        assert!(!snapshot.agents[0].connected);
        assert_eq!(snapshot.agents[0].status, None);
        let events = f.events.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::QuestionClosed {
                outcome: QuestionOutcome::Closed(CloseReason::Cancelled),
                ..
            }
        )));
        assert!(matches!(
            events.last(),
            Some(AgentEvent::Disconnected { .. })
        ));
    }

    #[test]
    fn agents_read_only_what_the_user_wrote_or_shared_for_them() {
        let f = fixture("privacy");
        let claude = f.hub.connect("claude").unwrap();
        let codex = f.hub.connect("codex").unwrap();
        send(&f.hub, claude, "olá, sou o Claude").unwrap();
        let first = f.hub.user_message("claude", "faz o teste").unwrap();
        f.hub.user_message("codex", "isto é para o Codex").unwrap();
        f.hub
            .share_context(
                "claude",
                SharedContext::Page {
                    title: "Docs".into(),
                    url: "https://example.com/docs".into(),
                    excerpt: None,
                },
            )
            .unwrap();
        let page = f
            .hub
            .call(claude, ToolCall::GetUserMessages { since_id: None })
            .unwrap();
        let messages = page["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "{page}");
        assert_eq!(messages[0]["kind"], "message");
        assert_eq!(messages[0]["text"], "faz o teste");
        assert_eq!(messages[1]["kind"], "page");
        assert_eq!(messages[1]["url"], "https://example.com/docs");
        assert!(!page.to_string().contains("Codex"));
        assert!(!page.to_string().contains("sou o Claude"));
        let last = page["last_id"].as_u64().unwrap();
        // Depois do last_id: so o que e novo.
        let empty = f
            .hub
            .call(
                claude,
                ToolCall::GetUserMessages {
                    since_id: Some(last),
                },
            )
            .unwrap();
        assert_eq!(empty["messages"], json!([]));
        assert_eq!(empty["last_id"].as_u64().unwrap(), last);
        let since_first = f
            .hub
            .call(
                claude,
                ToolCall::GetUserMessages {
                    since_id: Some(first.id),
                },
            )
            .unwrap();
        assert_eq!(since_first["messages"].as_array().unwrap().len(), 1);
        // O Codex so ve a dele.
        let codex_page = f
            .hub
            .call(codex, ToolCall::GetUserMessages { since_id: None })
            .unwrap();
        assert_eq!(codex_page["messages"].as_array().unwrap().len(), 1);
        assert_eq!(codex_page["messages"][0]["text"], "isto é para o Codex");
        // Paginacao: 50 por chamada, has_more.
        for index in 0..(USER_MESSAGES_PAGE + 3) {
            f.hub.user_message("codex", &format!("n{index}")).unwrap();
        }
        let page = f
            .hub
            .call(codex, ToolCall::GetUserMessages { since_id: Some(0) })
            .unwrap();
        assert_eq!(
            page["messages"].as_array().unwrap().len(),
            USER_MESSAGES_PAGE
        );
        assert_eq!(page["has_more"], true);
    }

    #[test]
    fn unread_counts_agent_messages_until_the_panel_marks_them_read() {
        let f = fixture("unread");
        let claude = f.hub.connect("claude").unwrap();
        send(&f.hub, claude, "um").unwrap();
        send(&f.hub, claude, "dois").unwrap();
        ask(&f.hub, claude, 600).unwrap();
        assert_eq!(f.hub.unread("claude"), 3);
        assert_eq!(f.hub.total_unread(), 3);
        f.hub.mark_read("claude");
        assert_eq!(f.hub.total_unread(), 0);
        send(&f.hub, claude, "três").unwrap();
        assert_eq!(f.hub.total_unread(), 1);
        // Sobrevive ao reinicio.
        let reopened = reopen(&f);
        reopened.load();
        assert_eq!(reopened.unread("claude"), 1);
        assert_eq!(reopened.conversation("claude").len(), 4);
        // Ids continuam a crescer depois de reabrir.
        let next = reopened.user_message("claude", "novo").unwrap();
        assert_eq!(next.id, 5);
        assert_eq!(reopened.total_unread(), 0);
    }

    #[test]
    fn clearing_conversations_keeps_ids_growing_and_writes_nothing_else() {
        let f = fixture("clear");
        let claude = f.hub.connect("claude").unwrap();
        send(&f.hub, claude, "um").unwrap();
        f.hub.user_message("claude", "dois").unwrap();
        f.hub.clear_conversations().unwrap();
        assert!(f.hub.conversation("claude").is_empty());
        assert_eq!(f.hub.total_unread(), 0);
        assert!(!f.dir.0.join("agents").join("claude.jsonl").exists());
        let reopened = reopen(&f);
        reopened.load();
        assert!(reopened.conversation("claude").is_empty());
        assert_eq!(reopened.user_message("claude", "três").unwrap().id, 3);
        // O hub so escreve dentro de <data_dir>/agents.
        let top: Vec<String> = std::fs::read_dir(&f.dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(top, vec!["agents".to_string()]);
    }

    /// Gate (critico: modo privado). Com o registo das lojas em `Private` o
    /// hub continua a servir -- o ecra recebe os eventos, o agente le o que
    /// o utilizador lhe escreveu -- mas a pasta `agents` fica byte a byte
    /// igual: nem conversa, nem estado de leitura. Quem reabre ve so o que
    /// ja la estava. O comportamento da reescrita esta em
    /// `store::tests::a_private_session_never_reaches_the_disk_even_through_a_rewrite`.
    #[test]
    fn a_private_session_serves_the_screen_and_the_agent_but_writes_nothing() {
        let f = fixture("private-hub");
        let claude = f.hub.connect("claude").unwrap();
        send(&f.hub, claude, "antes").unwrap();
        f.hub.mark_read("claude");
        let snapshot = |dir: &std::path::Path| -> Vec<(String, Vec<u8>)> {
            let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .map(|p| {
                    (
                        p.file_name().unwrap().to_string_lossy().into_owned(),
                        std::fs::read(&p).unwrap(),
                    )
                })
                .collect();
            files.sort();
            files
        };
        let before = snapshot(&f.hub.dir());
        assert_eq!(before.len(), 2, "claude.jsonl + state.json: {before:?}");

        f.registry.set_mode(StoreMode::Private);
        send(&f.hub, claude, "privado").unwrap();
        f.hub.user_message("claude", "segredo").unwrap();
        let question = ask(&f.hub, claude, 600).unwrap();
        f.hub
            .answer_question(question, QuestionAnswer::Button(0))
            .unwrap();
        f.hub.mark_read("claude");
        // Entregue: o ecra viu a mensagem e o cartao, o agente le o segredo.
        let events = f.events.lock().unwrap().clone();
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::Message { record, .. } if matches!(&record.body, RecordBody::AgentMessage { text, .. } if text == "privado"))
        ));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Question(_))));
        assert_eq!(f.hub.conversation("claude").len(), 5);
        let page = f
            .hub
            .call(claude, ToolCall::GetUserMessages { since_id: None })
            .unwrap();
        assert!(page.to_string().contains("segredo"), "{page}");
        // Gravado: nada.
        assert_eq!(snapshot(&f.hub.dir()), before);
        let reopened = reopen(&f);
        reopened.load();
        assert_eq!(reopened.conversation("claude").len(), 1);

        // De volta ao normal, so o que vier a seguir vai ao disco.
        f.registry.set_mode(StoreMode::Normal);
        send(&f.hub, claude, "depois").unwrap();
        let file = std::fs::read_to_string(f.hub.dir().join("claude.jsonl")).unwrap();
        assert!(file.contains("antes") && file.contains("depois"), "{file}");
        assert!(
            !file.contains("privado") && !file.contains("segredo"),
            "{file}"
        );
    }

    #[test]
    fn too_many_agent_names_are_refused() {
        let f = fixture("many");
        for index in 0..MAX_AGENTS {
            f.hub.connect(&format!("a{index}")).unwrap();
        }
        assert!(f.hub.connect("one-more").is_err());
        assert!(f.hub.user_message("one-more", "x").is_err());
        assert!(f.hub.connect("a0").is_ok());
        assert!(f.hub.connect("Bad Name").is_err());
    }
}
