use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub language: String,
    pub reader_max_bytes: usize,
    pub reader_timeout_secs: u64,
    pub history_limit: usize,
    pub data_dir: PathBuf,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            language: "pt-BR".into(),
            reader_max_bytes: 2 * 1024 * 1024,
            reader_timeout_secs: 12,
            history_limit: 250,
            data_dir: default_data_dir(),
        }
    }
}

pub fn default_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("NEURALIA_DATA_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("NeuralIA");
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("neuralia");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/neuralia");
    }
    PathBuf::from(".neuralia")
}
