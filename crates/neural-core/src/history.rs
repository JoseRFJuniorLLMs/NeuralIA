use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HistoryKind { Ask, Read, Web }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub timestamp_unix: u64,
    pub kind: HistoryKind,
    pub input: String,
    pub target: String,
}

impl HistoryEntry {
    pub fn now(kind: HistoryKind, input: impl Into<String>, target: impl Into<String>) -> Self {
        let timestamp_unix=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        Self{timestamp_unix,kind,input:input.into(),target:target.into()}
    }
}

#[derive(Debug, Clone)]
pub struct HistoryStore { path: PathBuf }

impl HistoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self { Self{path:path.into()} }

    pub fn append(&self, entry:&HistoryEntry)->Result<()>{
        if let Some(parent)=self.path.parent(){ fs::create_dir_all(parent)?; }
        let mut file=OpenOptions::new().create(true).append(true).open(&self.path)?;
        serde_json::to_writer(&mut file,entry)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    pub fn recent(&self, limit:usize)->Result<Vec<HistoryEntry>>{
        if !self.path.exists(){ return Ok(Vec::new()); }
        let file=File::open(&self.path)?;
        let mut entries:Vec<_>=BufReader::new(file).lines()
            .map_while(std::result::Result::ok)
            .filter_map(|line|serde_json::from_str::<HistoryEntry>(&line).ok())
            .collect();
        entries.reverse();
        entries.truncate(limit);
        Ok(entries)
    }

    pub fn path(&self)->&Path{ &self.path }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip(){
        let path=std::env::temp_dir().join(format!(
            "neuralia-history-{}-{}.jsonl",std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let store=HistoryStore::new(&path);
        store.append(&HistoryEntry::now(HistoryKind::Ask,"teste","https://example.com")).unwrap();
        let got=store.recent(10).unwrap();
        assert_eq!(got.len(),1);
        assert_eq!(got[0].input,"teste");
        let _=fs::remove_file(path);
    }
}
