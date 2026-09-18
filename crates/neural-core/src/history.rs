use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
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

    pub fn append(&self, entry: &HistoryEntry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.path)?;
        file.lock()?;

        let write_result = (|| -> Result<()> {
            let mut content = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut content)?;

            let mut entries = parse_lines(&content);
            entries.push(entry.clone());
            if entries.len() > self.max_entries {
                let remove = entries.len() - self.max_entries;
                entries.drain(..remove);
            }

            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            for entry in entries {
                serde_json::to_writer(&mut file, &entry)?;
                file.write_all(b"\n")?;
            }
            file.flush()?;
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

        let mut file = File::open(&self.path)?;
        file.lock_shared()?;

        let read_result = (|| -> Result<Vec<HistoryEntry>> {
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            let mut entries = parse_lines(&content);
            entries.reverse();
            entries.truncate(limit.min(self.max_entries));
            Ok(entries)
        })();

        let unlock_result = file.unlock();
        let entries = read_result?;
        unlock_result?;
        Ok(entries)
    }

    pub fn clear(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.path)?;
        file.lock()?;
        let clear_result = (|| -> Result<()> {
            file.set_len(0)?;
            file.sync_data()?;
            Ok(())
        })();
        let unlock_result = file.unlock();
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
                "google-ai",
            ))
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
    fn clear_removes_entries() {
        let path = temp_history("clear");
        let store = HistoryStore::new(&path);
        store
            .append(&HistoryEntry::now(HistoryKind::Web, "x", "https://example.com"))
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
