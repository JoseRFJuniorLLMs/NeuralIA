use super::*;

use crate::agents::store::ConversationStore;
use crate::agents::{self, AgentEvent, AgentHub, QuestionView, RecordBody};
use crate::stores::AGENTS_STORE;

// ===================== os agentes externos no app (agents-hub) =====================
//
// O nucleo (`crate::agents`: a ponte `--mcp`, o hub, o canal e as conversas)
// decide e guarda; aqui vive so o que o liga ao produto:
//
// - o hub nasce no `App::new` sobre a loja `agents` (`<data_dir>/agents`,
//   `Automatic`), aberta SO pelo grant do registo das lojas -- numa sessao
//   privada as conversas ficam fora do disco; sem registo nao ha hub -- e o
//   canal (named pipe) abre numa thread propria (`agents::pipe::start`),
//   nunca na thread da janela;
// - cada `AgentEvent` que o hub anuncia vira UM evento da janela,
//   `UserEvent::AgentsHub(AgentsHubEvent::Hub(..))`, entregue pela thread do
//   canal atraves do `EventLoopProxy`;
// - uma mensagem ou uma pergunta de um agente mostra um aviso curto
//   (`agent_toast_text`); o painel «Agentes» (cartoes, estado, badge)
//   desenha-se a partir de `AgentHub::snapshot` e chega no int-agents-finish;
// - o Ctrl+Shift+Delete apaga as conversas (`ClearTarget::Agents`); o canal
//   e as credenciais dele ficam, o hub continua a correr.

/// A variante `UserEvent::AgentsHub`: o que o hub anuncia. As accoes do
/// painel «Agentes» (responder, escrever, partilhar) entram aqui quando o
/// painel existir, sem tocar na raiz.
#[derive(Debug)]
pub(in crate::windows_app) enum AgentsHubEvent {
    /// Um `agents::AgentEvent`, tal como o hub o entregou.
    Hub(AgentEvent),
}

/// O estado dos agentes externos no `App`: o hub (clonavel; os clones sao o
/// mesmo hub) que as threads do canal tambem seguram. `None` so sem registo
/// das lojas, que nao acontece (ha um `App` por processo): sem grant nao ha
/// pasta, nem hub, nem canal.
pub(in crate::windows_app) struct AgentsHubState {
    hub: Option<AgentHub>,
}

impl AgentsHubState {
    /// Pede o grant da loja `agents` ao registo, cria o hub sobre ela e abre
    /// o canal fora desta thread (`agents::pipe::start`: le as conversas e
    /// faz o bind do named pipe).
    pub(in crate::windows_app) fn open(
        stores: Option<&StoreRegistry>,
        proxy: &EventLoopProxy<UserEvent>,
    ) -> Self {
        let hub = stores
            .and_then(|stores| stores.grant(AGENTS_STORE).ok())
            .and_then(|grant| ConversationStore::open(grant).ok())
            .map(|store| {
                let proxy = proxy.clone();
                AgentHub::new(store, move |event| {
                    let _ = proxy.send_event(UserEvent::AgentsHub(AgentsHubEvent::Hub(event)));
                })
            });
        if let Some(hub) = &hub {
            agents::pipe::start(hub.clone());
        }
        Self { hub }
    }

    pub(in crate::windows_app) fn hub(&self) -> Option<&AgentHub> {
        self.hub.as_ref()
    }
}

/// O texto do aviso de uma mensagem ou pergunta de um agente, numa linha,
/// com tecto (140 chars + «…»). `None` para o resto da conversa.
pub(in crate::windows_app) fn agent_toast_text(
    display_name: &str,
    body: &RecordBody,
) -> Option<String> {
    let (prefix, text) = match body {
        RecordBody::AgentMessage { title, text } => (
            title
                .as_deref()
                .map_or_else(String::new, |title| format!("{title} — ")),
            text.as_str(),
        ),
        RecordBody::AgentQuestion { text, .. } => ("pergunta: ".to_string(), text.as_str()),
        _ => return None,
    };
    let line: String = format!("{display_name}: {prefix}{text}")
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect();
    let mut shown: String = line.chars().take(140).collect();
    if line.chars().count() > 140 {
        shown.push('…');
    }
    Some(shown)
}

/// O aviso de uma pergunta: o mesmo texto que a conversa guarda.
fn question_toast_text(question: &QuestionView) -> Option<String> {
    let body = RecordBody::AgentQuestion {
        question_id: question.id,
        text: question.text.clone(),
        options: question.options.clone(),
        allow_text: question.allow_text,
    };
    agent_toast_text(&question.display_name, &body)
}

impl App {
    /// O unico braco dos agentes no `user_event`.
    pub(in crate::windows_app) fn agents_hub_event(&mut self, event: AgentsHubEvent) {
        match event {
            AgentsHubEvent::Hub(AgentEvent::Message { agent, record }) => {
                if let Some(text) =
                    agent_toast_text(&agents::agent_display_name(&agent), &record.body)
                {
                    self.show_splash(text, 6);
                }
            }
            AgentsHubEvent::Hub(AgentEvent::Question(question)) => {
                if let Some(text) = question_toast_text(&question) {
                    self.show_splash(text, 8);
                }
            }
            // O painel «Agentes» (cartoes, estado, badge) desenha-se a partir
            // de `self.agents.hub().snapshot()`; ate la, so redesenhar.
            AgentsHubEvent::Hub(
                AgentEvent::QuestionClosed { .. }
                | AgentEvent::Status { .. }
                | AgentEvent::Connected { .. }
                | AgentEvent::Disconnected { .. }
                | AgentEvent::Changed,
            ) => self.request_redraw(),
        }
    }

    /// O braco `ClearTarget::Agents` do "Apagar historico": as conversas com
    /// os agentes vao no mesmo gesto; uma falha do disco e dita, nao engolida.
    pub(in crate::windows_app) fn clear_agent_conversations(&mut self) {
        if let Some(Err(error)) = self.agents.hub().map(AgentHub::clear_conversations) {
            self.show_splash(format!("Conversas dos agentes: {error}"), 4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_toasts_are_one_capped_line_and_only_for_messages_and_questions() {
        let long = RecordBody::AgentMessage {
            title: Some("Pronto".into()),
            text: format!("linha 1\nlinha 2 {}", "x".repeat(300)),
        };
        let toast = agent_toast_text("Claude", &long).unwrap();
        assert!(
            toast.starts_with("Claude: Pronto — linha 1 linha 2"),
            "{toast}"
        );
        assert!(!toast.contains('\n'));
        assert_eq!(toast.chars().count(), 141);
        assert!(toast.ends_with('…'));
        let short = RecordBody::AgentMessage {
            title: None,
            text: "oi".into(),
        };
        assert_eq!(
            agent_toast_text("Claude", &short).as_deref(),
            Some("Claude: oi")
        );
        let question = QuestionView {
            id: 1,
            agent: "codex".into(),
            display_name: "Codex".into(),
            text: "Posso?".into(),
            options: vec!["Sim".into()],
            allow_text: true,
            record_id: 2,
            deadline: std::time::Instant::now(),
        };
        assert_eq!(
            question_toast_text(&question).as_deref(),
            Some("Codex: pergunta: Posso?")
        );
        // O que o utilizador escreveu ao agente nunca vira aviso.
        for body in [
            RecordBody::UserMessage {
                text: "privado".into(),
            },
            RecordBody::UserAnswer {
                question_id: 1,
                text: "Sim".into(),
                via: agents::AnswerVia::Button,
            },
            RecordBody::QuestionClosed {
                question_id: 1,
                reason: agents::CloseReason::TimedOut,
            },
        ] {
            assert!(agent_toast_text("Codex", &body).is_none(), "{body:?}");
        }
    }
}
