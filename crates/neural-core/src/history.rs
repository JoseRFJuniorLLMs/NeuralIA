use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::Result;

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
}

impl HistoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn append(&self, entry: &HistoryEntry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)?;

        // std::fs file locking is process-safe on Windows and advisory on Unix.
        // Keeping the lock around the whole JSONL record prevents interleaved
        // lines when two NeuralIA instances write at the same time.
        file.lock()?;

        let write_result = (|| -> Result<()> {
            serde_json::to_writer(&mut file, entry)?;
            file.write_all(b"\n")?;
            file.flush()?;
            // History writes are infrequent; syncing here buys simple
            // crash-resilience without introducing a database.
            file.sync_data()?;
            Ok(())
        })();

        let unlock_result = file.unlock();
        write_result?;
        unlock_result?;
        Ok(())
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path)?;
        file.lock_shared()?;

        let read_result = (|| -> Result<Vec<HistoryEntry>> {
            let mut entries = Vec::new();
            for line in BufReader::new(&file).lines() {
                let line = line?;
                // A single torn/corrupt historical record must not make all
                // remaining history unreadable.
                if let Ok(entry) = serde_json::from_str::<HistoryEntry>(&line) {
                    entries.push(entry);
                }
            }
            entries.reverse();
            entries.truncate(limit);
            Ok(entries)
        })();

        let unlock_result = file.unlock();
        let entries = read_result?;
        unlock_result?;
        Ok(entries)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
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
            .append(&HistoryEntry::now(
                HistoryKind::Ask,
                "teste",
                "https://example.com",
            ))
            .unwrap();
        let got = store.recent(10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].input, "teste");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn corrupt_line_does_not_destroy_other_history() {
        let path = temp_history("corrupt");
        fs::write(
            &path,
            b"{not-json}\n{\"timestamp_unix\":1,\"kind\":\"Ask\",\"input\":\"ok\",\"target\":\"https://example.com\"}\n",
        )
        .unwrap();
        let store = HistoryStore::new(&path);
        let got = store.recent(10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].input, "ok");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn concurrent_writers_do_not_interleave_jsonl() {
        let path = temp_history("concurrent");
        let store = Arc::new(HistoryStore::new(&path));
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

        assert_eq!(store.recent(1000).unwrap().len(), 80);
        let _ = fs::remove_file(path);
    }
}
