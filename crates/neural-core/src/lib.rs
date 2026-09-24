// Efeito colateral dentro de debug_assert! some no release (embarca sem
// debug-assertions): o pop do Reader ja duplicou texto por isso.
#![deny(clippy::debug_assert_with_mut_call)]

pub mod agent_protocol;
pub mod agent_security;
pub mod config;
pub mod epub;
pub mod error;
pub mod file_risk;
pub mod history;
pub mod intent;
pub mod library;
pub mod local_intelligence;
pub mod memory;
pub mod pomodoro;
pub mod reader;
pub mod render;
pub mod research;
pub mod search;
pub mod security;
pub mod semantic_timeline;
pub mod tissue;
pub mod zettel;

#[cfg(test)]
mod test_alloc;

// Os gates de memória (livro hostil que multiplica o que aloca) medem o pico
// com este alocador; só nos testes do neural-core.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOC: test_alloc::CountingAlloc = test_alloc::CountingAlloc;

pub use config::CoreConfig;
pub use epub::{
    Creator, EpubArchive, EpubBook, EpubError, EpubMetadata, LimitKind, ManifestItem,
    PageProgression, SpineItem, TocEntry,
};
pub use error::{NeuralError, Result};
pub use history::{HistoryEntry, HistoryKind, HistoryStore};
pub use intent::{Intent, is_pdf_url, parse_intent};
pub use library::{BookEntry, Bookmark, Library, LibraryError, Position};
pub use reader::{ReaderArticle, ReaderBlock, ReaderClient};
pub use render::reader_html;
pub use search::{chatgpt_search_url, claude_search_url, google_ai_url, perplexity_search_url};
pub use security::{
    is_forbidden_ip, is_local_network_target, validate_redirect_target, validate_web_url,
};

pub use agent_security::{
    ActionRisk, AgentPermissionPolicy, AgentSecurityAction, CapabilityClass, FieldKind,
    PolicyDecision, redact_sensitive_text,
};
pub use local_intelligence::{
    ActiveModelPack, EMBEDDING_DIM, HashingLocalIntelligence, IntentClass, LocalBenchmark,
    LocalIntelligence, ModelPackActivation, ModelPackManager, ModelPackManifest,
    ModelPackSelection, benchmark_local_intelligence, cosine_similarity, hashed_embedding,
};
pub use memory::{
    CaptureOutcome, ForgetReport, ForgetScope, MemoryDoctorReport, MemoryDocument, MemoryHit,
    MemoryKind, MemoryQuery, MemoryRelation, MemorySourceKind, MemoryStore,
};
pub use pomodoro::{Phase, Pomodoro, PomodoroEvent, PomodoroSettings, PomodoroSettingsError};
pub use research::{
    ComparisonFact, ResearchItem, ResearchItemKind, ResearchSession, SynthesisSnapshot,
};

pub use agent_protocol::{AgentAction, AgentElement, AgentRuntimeConfig, ObservedPage};
pub use file_risk::{
    DefaultAppTarget, RiskClass, SniffRisk, classify_download_name, default_app_target,
    display_label, sniff_download,
};
pub use semantic_timeline::{SemanticAnchor, SemanticAnchorKind, semantic_anchors_html};
pub use zettel::{Note, ZettelError, ZettelStore};
