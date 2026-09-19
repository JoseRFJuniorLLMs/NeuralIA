use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::Result;

const DEFAULT_HISTORY_LIMIT: usize = 250;

/// Tecto de cada campo de texto de uma entrada. A omnibox ja corta o que o
/// utilizador escreve, mas os alvos vindos das paginas (redireccoes, `data:`
/// enormes, titulos gerados) nunca passam por la: sem este tecto uma unica
/// entrada de megabytes fica no ficheiro e e relida para memoria a cada
/// `append`, que reescreve o historico inteiro.
pub const MAX_FIELD_CHARS: usize = 2048;

/// Corta na fronteira de CHAR e nunca na de byte: `String::truncate` num
/// offset a meio de um UTF-8 entra em panico, e cortar por bytes partiria
/// acentos e emojis ao meio.
fn truncate_field(mut value: String) -> String {
    // Caminho rapido: cada char ocupa pelo menos um byte, por isso um
    // comprimento em bytes dentro do tecto garante que tambem esta em chars,
    // sem percorrer a string.
    if value.len() <= MAX_FIELD_CHARS {
        return value;
    }

    // `nth(MAX)` da o offset do char seguinte ao ultimo que se mantem, ou seja
    // o comprimento exacto dos primeiros MAX chars. `None` = ja cabia.
    if let Some((offset, _)) = value.char_indices().nth(MAX_FIELD_CHARS) {
        value.truncate(offset);
    }
    value
}

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
            // Cortar na origem: o que nao entra aqui nunca chega ao ficheiro.
            input: truncate_field(input.into()),
            target: truncate_field(target.into()),
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

    /// Sem `path.exists()` antes do lock: entre o teste e a leitura o ficheiro
    /// pode nascer ou ser substituido pelo `rename` do `append` (TOCTOU), e o
    /// teste ficava de fora do lock que devia protege-lo. `read_entries` ja
    /// trata o `NotFound` como historico vazio, por isso basta ler sempre.
    pub fn recent(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
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

/// Corta tambem na leitura: um ficheiro escrito por uma versao anterior (ou
/// a mao) traz campos sem tecto, e o `append` reescreve o que leu. Sem este
/// corte o gigante sobrevivia a todas as gravacoes seguintes.
fn parse_lines(content: &str) -> Vec<HistoryEntry> {
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<HistoryEntry>(line).ok())
        .map(|mut entry| {
            entry.input = truncate_field(entry.input);
            entry.target = truncate_field(entry.target);
            entry
        })
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
    fn oversized_fields_are_truncated_on_creation() {
        let entry = HistoryEntry::now(HistoryKind::Read, "a".repeat(10_000), "b".repeat(10_000));
        assert_eq!(entry.input.chars().count(), MAX_FIELD_CHARS);
        assert_eq!(entry.target.chars().count(), MAX_FIELD_CHARS);

        // O que ja cabia nao pode ser tocado.
        let small = HistoryEntry::now(HistoryKind::Ask, "curto", "google-ai");
        assert_eq!(small.input, "curto");
        assert_eq!(small.target, "google-ai");
    }

    #[test]
    fn truncation_cuts_on_char_boundary() {
        // Cada 'ç' sao dois bytes: cortar aos 2048 BYTES cairia a meio de um
        // char e o `String::truncate` entraria em panico.
        let entry = HistoryEntry::now(HistoryKind::Web, "ç".repeat(10_000), "https://example.com");
        assert_eq!(entry.input.chars().count(), MAX_FIELD_CHARS);
        assert!(entry.input.chars().all(|c| c == 'ç'));
        assert_eq!(entry.input.len(), MAX_FIELD_CHARS * 2);
    }

    #[test]
    fn oversized_fields_from_disk_are_truncated_on_read() {
        let path = temp_history("oversized");
        let line = format!(
            "{{\"timestamp_unix\":1,\"kind\":\"Read\",\"input\":\"{}\",\"target\":\"{}\"}}\n",
            "a".repeat(10_000),
            "https://example.com/".to_string() + &"b".repeat(10_000)
        );
        fs::write(&path, line.as_bytes()).unwrap();

        let store = HistoryStore::new(&path);
        let got = store.recent(10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].input.chars().count(), MAX_FIELD_CHARS);
        assert_eq!(got[0].target.chars().count(), MAX_FIELD_CHARS);

        // E o corte tem de sobreviver ao ciclo ler-escrever do `append`.
        store
            .append(&HistoryEntry::now(HistoryKind::Ask, "depois", "alvo"))
            .unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.lines().all(|line| line.len() < 6_000));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(sibling(&path, "lock"));
    }

    #[test]
    fn recent_on_missing_file_is_empty() {
        let path = temp_history("missing");
        let store = HistoryStore::new(&path);
        assert!(store.recent(10).unwrap().is_empty());
        let _ = fs::remove_file(sibling(&path, "lock"));
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
