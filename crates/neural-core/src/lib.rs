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
pub use intent::{parse_intent, Intent};
pub use reader::{ReaderArticle, ReaderBlock, ReaderClient};
pub use render::{home_html, reader_html};
pub use search::google_ai_url;
pub use security::validate_web_url;
