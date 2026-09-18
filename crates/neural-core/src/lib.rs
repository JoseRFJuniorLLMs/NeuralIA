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
