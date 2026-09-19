pub mod semantic_timeline;
pub mod agent_runtime;
pub mod research;
pub mod memory;
pub mod local_intelligence;
pub mod agent_security;
pub mod config;
pub mod error;
pub mod history;
pub mod intent;
pub mod reader;
pub mod render;
pub mod search;
pub mod security;

pub use config::CoreConfig;
pub use error::{NeuralError, Result};
pub use history::{HistoryEntry, HistoryKind, HistoryStore};
pub use intent::{Intent, is_pdf_url, parse_intent};
pub use reader::{ReaderArticle, ReaderBlock, ReaderClient};
pub use render::reader_html;
pub use search::{chatgpt_search_url, claude_search_url, google_ai_url, perplexity_search_url};
pub use security::{
    is_forbidden_ip, is_local_network_target, validate_redirect_target, validate_web_url,
};

pub use agent_security::{
    ActionRisk, AgentPermissionPolicy, AgentSecurityAction, FieldKind, PolicyDecision,
    redact_sensitive_text,
};
pub use local_intelligence::{
    EMBEDDING_DIM, HashingLocalIntelligence, IntentClass, LocalIntelligence, ModelPackManager,
    ModelPackManifest, cosine_similarity, hashed_embedding,
};
pub use memory::{
    CaptureOutcome, ForgetReport, ForgetScope, MemoryDocument, MemoryHit, MemoryKind, MemoryQuery,
    MemoryRelation, MemorySourceKind, MemoryStore,
};
pub use research::{
    ComparisonFact, ResearchItem, ResearchItemKind, ResearchSession, SynthesisSnapshot,
};

pub use agent_runtime::{
    AgentAction, AgentElement, AgentOutcome, AgentPlanner, AgentRuntime, AgentRuntimeConfig,
    AgentToolExecutor, ObservedPage, ToolResult,
};
pub use semantic_timeline::{SemanticAnchor, SemanticAnchorKind, semantic_anchors_html};
