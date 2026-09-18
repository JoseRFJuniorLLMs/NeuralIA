use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::Result;

const DEFAULT_HISTORY_LIMIT: usize = 250;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HistoryKind {
    Ask,
    Read,
    Web,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub timestamp_unix: u64,
    pub kind: HistoryKind,
    pub input: String,
    pub target: String,
}

impl HistoryEntry {
    pub fn now(kind: HistoryKind, input: impl Into<String>, target: impl Into<String>) -> Self {
        let timestamp_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            timestamp_unix,
            kind,
            input: input.into(),
            target: target.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryStore {
    path: PathBuf,
    max_entries: usize,
}

impl HistoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_limit(path, DEFAULT_HISTORY_LIMIT)
    }

    pub fn with_limit(path: impl Into<PathBuf>, max_entries: usize) -> Self {
        Self {
            path: path.into(),
            max_entries: max_entries.max(1),
        }
    }

    /// Caminho do ficheiro que serializa escritores. O lock vive fora do
    /// ficheiro de dados porque o ficheiro de dados e substituido por `rename`,
    /// e no Windows nao se renomeia por cima de um handle aberto.
    fn lock_path(&self) -> PathBuf {
        sibling(&self.path, "lock")
    }

    fn temp_path(&self) -> PathBuf {
        sibling(&self.path, "tmp")
    }

    fn acquire_lock(&self, exclusive: bool) -> Result<File> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.lock_path())?;
        if exclusive {
            file.lock()?;
        } else {
            file.lock_shared()?;
        }
        Ok(file)
    }

    fn read_entries(&self) -> Result<Vec<HistoryEntry>> {
        match File::open(&self.path) {
            Ok(mut file) => {
                let mut content = String::new();
                file.read_to_string(&mut content)?;
                Ok(parse_lines(&content))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    /// Escreve para um ficheiro temporario, sincroniza e so entao o move para o
    /// lugar. Um corte de energia a meio deixa o historico anterior intacto, em
    /// vez do ficheiro truncado que o `set_len(0)` + reescrita deixava.
    fn replace_atomically(&self, entries: &[HistoryEntry]) -> Result<()> {
        let temp = self.temp_path();
        let write_result = (|| -> Result<()> {
            let mut file = File::create(&temp)?;
            for entry in entries {
                serde_json::to_writer(&mut file, entry)?;
                file.write_all(b"\n")?;
            }
            file.flush()?;
            file.sync_data()?;
            drop(file);
            fs::rename(&temp, &self.path)?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        write_result
    }

    pub fn append(&self, entry: &HistoryEntry) -> Result<()> {
        let lock = self.acquire_lock(true)?;

        let write_result = (|| -> Result<()> {
            let mut entries = self.read_entries()?;
            entries.push(entry.clone());
            if entries.len() > self.max_entries {
                let remove = entries.len() - self.max_entries;
                entries.drain(..remove);
            }
            self.replace_atomically(&entries)
        })();

        let unlock_result = lock.unlock();
        write_result?;
        unlock_result?;
        Ok(())
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let lock = self.acquire_lock(false)?;
        let read_result = (|| -> Result<Vec<HistoryEntry>> {
            let mut entries = self.read_entries()?;
            entries.reverse();
            entries.truncate(limit.min(self.max_entries));
            Ok(entries)
        })();

        let unlock_result = lock.unlock();
        let entries = read_result?;
        unlock_result?;
        Ok(entries)
    }

    pub fn clear(&self) -> Result<()> {
        let lock = self.acquire_lock(true)?;
        let clear_result = self.replace_atomically(&[]);
        let unlock_result = lock.unlock();
        clear_result?;
        unlock_result?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }
}

fn parse_lines(content: &str) -> Vec<HistoryEntry> {
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<HistoryEntry>(line).ok())
        .collect()
}

/// `history.jsonl` -> `history.jsonl.lock` / `history.jsonl.tmp`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history.jsonl".to_string());
    path.with_file_name(format!("{name}.{suffix}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn temp_history(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neuralia-history-{name}-{}-{}.jsonl",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn round_trip() {
        let path = temp_history("round-trip");
        let store = HistoryStore::new(&path);
        store
            .append(&HistoryEntry::now(HistoryKind::Ask, "teste", "google-ai"))
            .unwrap();
        let got = store.recent(10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].input, "teste");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn enforces_retention_limit() {
        let path = temp_history("bounded");
        let store = HistoryStore::with_limit(&path, 3);
        for i in 0..7 {
            store
                .append(&HistoryEntry::now(
                    HistoryKind::Read,
                    format!("item-{i}"),
                    "https://example.com",
                ))
                .unwrap();
        }

        let got = store.recent(20).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].input, "item-6");
        assert_eq!(got[2].input, "item-4");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn replacement_is_atomic_and_leaves_no_temp_behind() {
        let path = temp_history("atomic");
        let store = HistoryStore::with_limit(&path, 10);
        let temp = sibling(&path, "tmp");

        store
            .append(&HistoryEntry::now(HistoryKind::Ask, "um", "alvo"))
            .expect("append");
        store
            .append(&HistoryEntry::now(HistoryKind::Ask, "dois", "alvo"))
            .expect("append");

        assert!(
            !temp.exists(),
            "o ficheiro temporario devia ter sido movido"
        );
        assert_eq!(store.recent(10).expect("recent").len(), 2);

        // O ficheiro de dados nunca fica aberto por nos: e sempre substituido
        // inteiro, por isso um leitor externo ve a versao velha ou a nova.
        let content = std::fs::read_to_string(&path).expect("ler");
        assert_eq!(content.lines().filter(|l| !l.is_empty()).count(), 2);

        store.clear().expect("clear");
        assert!(!temp.exists());
        assert!(store.recent(10).expect("recent").is_empty());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(sibling(&path, "lock"));
    }

    #[test]
    fn clear_removes_entries() {
        let path = temp_history("clear");
        let store = HistoryStore::new(&path);
        store
            .append(&HistoryEntry::now(
                HistoryKind::Web,
                "x",
                "https://example.com",
            ))
            .unwrap();
        store.clear().unwrap();
        assert!(store.recent(10).unwrap().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn corrupt_line_does_not_destroy_other_history() {
        let path = temp_history("corrupt");
        fs::write(
            &path,
            b"{not-json}\n{\"timestamp_unix\":1,\"kind\":\"Ask\",\"input\":\"ok\",\"target\":\"google-ai\"}\n",
        )
        .unwrap();
        let store = HistoryStore::new(&path);
        let got = store.recent(10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].input, "ok");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn concurrent_writers_remain_bounded_and_valid() {
        let path = temp_history("concurrent");
        let store = Arc::new(HistoryStore::with_limit(&path, 50));
        let mut workers = Vec::new();

        for worker in 0..4 {
            let store = Arc::clone(&store);
            workers.push(std::thread::spawn(move || {
                for item in 0..20 {
                    store
                        .append(&HistoryEntry::now(
                            HistoryKind::Read,
                            format!("{worker}-{item}"),
                            "https://example.com",
                        ))
                        .unwrap();
                }
            }));
        }

        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(store.recent(1000).unwrap().len(), 50);
        let _ = fs::remove_file(path);
    }
}
