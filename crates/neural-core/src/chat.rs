//! Camada de Domínio para a Chat Surface Multi-Provedor (SPEC-0115).
//!
//! Estruturas normativas de conversa, turnos e mensagens unificadas
//! entre provedores de IA sem dependência da DOM externa.

use serde::{Deserialize, Serialize};

use crate::search::ProviderId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChatThreadId(pub String);

impl ChatThreadId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub String);

impl TurnId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl MessageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadPrivacy {
    Persistent,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageStatus {
    Streaming,
    Settled,
    Error,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtractionQuality {
    Clean,
    Partial,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ContentPart {
    Text(String),
    Citation {
        text: String,
        url: String,
    },
    Code {
        code: String,
        language: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub message_id: MessageId,
    pub thread_id: ChatThreadId,
    pub turn_id: TurnId,
    pub ordinal: u64,
    pub attempt: u32,
    pub role: MessageRole,
    pub provider_id: Option<ProviderId>,
    pub status: MessageStatus,
    pub extraction_quality: ExtractionQuality,
    pub parts: Vec<ContentPart>,
    pub is_partial: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl ChatMessage {
    pub fn user_text(
        message_id: MessageId,
        thread_id: ChatThreadId,
        turn_id: TurnId,
        ordinal: u64,
        text: String,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            message_id,
            thread_id,
            turn_id,
            ordinal,
            attempt: 1,
            role: MessageRole::User,
            provider_id: None,
            status: MessageStatus::Settled,
            extraction_quality: ExtractionQuality::Clean,
            parts: vec![ContentPart::Text(text)],
            is_partial: false,
            created_at_ms: timestamp_ms,
            updated_at_ms: timestamp_ms,
        }
    }

    pub fn assistant_reply(
        message_id: MessageId,
        thread_id: ChatThreadId,
        turn_id: TurnId,
        ordinal: u64,
        provider_id: ProviderId,
        parts: Vec<ContentPart>,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            message_id,
            thread_id,
            turn_id,
            ordinal,
            attempt: 1,
            role: MessageRole::Assistant,
            provider_id: Some(provider_id),
            status: MessageStatus::Settled,
            extraction_quality: ExtractionQuality::Clean,
            parts,
            is_partial: false,
            created_at_ms: timestamp_ms,
            updated_at_ms: timestamp_ms,
        }
    }

    pub fn full_text(&self) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                ContentPart::Text(txt) => out.push_str(txt),
                ContentPart::Citation { text, url } => {
                    out.push_str(&format!("[{text}]({url})"));
                }
                ContentPart::Code { code, language } => {
                    let lang = language.as_deref().unwrap_or("");
                    out.push_str(&format!("\n```{lang}\n{code}\n```\n"));
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTurn {
    pub turn_id: TurnId,
    pub thread_id: ChatThreadId,
    pub ordinal: u64,
    pub user_message_id: MessageId,
    pub created_at_ms: u64,
    pub responses: Vec<ChatMessage>,
}

impl ChatTurn {
    pub fn new(
        turn_id: TurnId,
        thread_id: ChatThreadId,
        ordinal: u64,
        user_message_id: MessageId,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            turn_id,
            thread_id,
            ordinal,
            user_message_id,
            created_at_ms: timestamp_ms,
            responses: Vec::new(),
        }
    }

    pub fn add_response(&mut self, message: ChatMessage) {
        self.responses.push(message);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatThread {
    pub thread_id: ChatThreadId,
    pub research_session_id: Option<String>,
    pub title: String,
    pub intent: Option<String>,
    pub privacy: ThreadPrivacy,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub next_turn_ordinal: u64,
    pub providers: Vec<ProviderId>,
    pub turns: Vec<ChatTurn>,
}

impl ChatThread {
    pub fn new(
        thread_id: ChatThreadId,
        title: String,
        privacy: ThreadPrivacy,
        providers: Vec<ProviderId>,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            thread_id,
            research_session_id: None,
            title,
            intent: None,
            privacy,
            created_at_ms: timestamp_ms,
            updated_at_ms: timestamp_ms,
            next_turn_ordinal: 1,
            providers,
            turns: Vec::new(),
        }
    }

    pub fn begin_turn(
        &mut self,
        turn_id: TurnId,
        user_message_id: MessageId,
        timestamp_ms: u64,
    ) -> &mut ChatTurn {
        let ordinal = self.next_turn_ordinal;
        self.next_turn_ordinal += 1;
        self.updated_at_ms = timestamp_ms;
        let turn = ChatTurn::new(
            turn_id,
            self.thread_id.clone(),
            ordinal,
            user_message_id,
            timestamp_ms,
        );
        self.turns.push(turn);
        self.turns.last_mut().expect("just pushed")
    }

    pub fn export_markdown(&self) -> String {
        let mut md = format!("# {}\n\n", self.title);
        for turn in &self.turns {
            md.push_str(&format!("## Turno {}\n\n", turn.ordinal));
            for resp in &turn.responses {
                let provider_name = resp
                    .provider_id
                    .as_ref()
                    .map(|p| p.info().display_name)
                    .unwrap_or("Assistente");
                md.push_str(&format!("### {provider_name}\n\n{}\n\n", resp.full_text()));
            }
        }
        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_thread_lifecycle() {
        let thread_id = ChatThreadId::new("thread_1");
        let mut thread = ChatThread::new(
            thread_id.clone(),
            "Pesquisa sobre Rust".to_string(),
            ThreadPrivacy::Persistent,
            vec![ProviderId::ChatGpt, ProviderId::Claude],
            1000,
        );

        assert_eq!(thread.next_turn_ordinal, 1);
        assert_eq!(thread.turns.len(), 0);

        let turn = thread.begin_turn(TurnId::new("turn_1"), MessageId::new("msg_user_1"), 1050);
        assert_eq!(turn.ordinal, 1);

        let chatgpt_reply = ChatMessage::assistant_reply(
            MessageId::new("msg_chatgpt_1"),
            thread_id.clone(),
            turn.turn_id.clone(),
            1,
            ProviderId::ChatGpt,
            vec![
                ContentPart::Text("Rust garante memória segura através de ownership.".to_string()),
                ContentPart::Citation {
                    text: "Documentação do Rust".to_string(),
                    url: "https://doc.rust-lang.org".to_string(),
                },
            ],
            1100,
        );
        turn.add_response(chatgpt_reply);

        assert_eq!(thread.turns[0].responses.len(), 1);
        let exported = thread.export_markdown();
        assert!(exported.contains("# Pesquisa sobre Rust"));
        assert!(exported.contains("## Turno 1"));
        assert!(exported.contains("### ChatGPT"));
        assert!(exported.contains("Rust garante memória segura"));
        assert!(exported.contains("[Documentação do Rust](https://doc.rust-lang.org)"));
    }

    #[test]
    fn test_chat_serialization() {
        let thread = ChatThread::new(
            ChatThreadId::new("t1"),
            "Teste".to_string(),
            ThreadPrivacy::Private,
            vec![ProviderId::Perplexity],
            500,
        );
        let serialized = serde_json::to_string(&thread).expect("serialize");
        let deserialized: ChatThread = serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(thread, deserialized);
    }
}
