#![cfg(windows)]

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    ptr, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use neural_core::{MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore};
use serde::Serialize;

type Sqlite = *mut c_void;
type Statement = *mut c_void;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;

#[link(name = "winsqlite3")]
unsafe extern "C" {
    fn sqlite3_open(filename: *const c_char, database: *mut Sqlite) -> c_int;
    fn sqlite3_close(database: Sqlite) -> c_int;
    fn sqlite3_prepare_v2(
        database: Sqlite,
        sql: *const c_char,
        bytes: c_int,
        statement: *mut Statement,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_step(statement: Statement) -> c_int;
    fn sqlite3_column_int64(statement: Statement, column: c_int) -> i64;
    fn sqlite3_finalize(statement: Statement) -> c_int;
    fn sqlite3_errmsg(database: Sqlite) -> *const c_char;
}

struct Database(Sqlite);

impl Database {
    fn open(path: &Path) -> io::Result<Self> {
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| io::Error::other("sqlite path contains NUL"))?;
        let mut database: Sqlite = ptr::null_mut();
        let code = unsafe { sqlite3_open(path.as_ptr(), &mut database) };
        if code != SQLITE_OK || database.is_null() {
            return Err(io::Error::other(
                "winsqlite3 could not open baseline database",
            ));
        }
        Ok(Self(database))
    }

    fn count_fts(&self, query: &str) -> io::Result<i64> {
        let escaped = query.replace(char::from(39), "''");
        let sql = CString::new(format!(
            "SELECT count(*) FROM memory_fts WHERE memory_fts MATCH '{escaped}';"
        ))
        .map_err(|_| io::Error::other("FTS query contains NUL"))?;

        let mut statement: Statement = ptr::null_mut();
        let code = unsafe {
            sqlite3_prepare_v2(self.0, sql.as_ptr(), -1, &mut statement, ptr::null_mut())
        };
        if code != SQLITE_OK || statement.is_null() {
            return Err(io::Error::other(self.error_message()));
        }

        let step = unsafe { sqlite3_step(statement) };
        if step != SQLITE_ROW {
            unsafe {
                sqlite3_finalize(statement);
            }
            return Err(io::Error::other(self.error_message()));
        }

        let count = unsafe { sqlite3_column_int64(statement, 0) };
        unsafe {
            sqlite3_finalize(statement);
        }
        Ok(count)
    }

    fn error_message(&self) -> String {
        let ptr = unsafe { sqlite3_errmsg(self.0) };
        if ptr.is_null() {
            "unknown winsqlite3 error".into()
        } else {
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        unsafe {
            sqlite3_close(self.0);
        }
    }
}

#[derive(Debug, Serialize)]
struct ProcessMetrics {
    rss_bytes: u64,
    threads: u64,
}

#[derive(Debug, Serialize)]
struct Baseline {
    documents: usize,
    hybrid_query_median_us: u128,
    fts_query_median_us: u128,
    reindex_ms: u128,
    rss_bytes_idle: u64,
    threads_idle: u64,
    fts_matches: i64,
}

fn temp_root(size: usize) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "neuralia-memory-phase1-baseline-{size}-{}-{nonce}",
        std::process::id()
    ))
}

fn seed_documents(store: &MemoryStore, count: usize) -> io::Result<String> {
    let documents = store.root().join("documents");
    fs::create_dir_all(&documents)?;
    let mut sentinel_id = None;

    for index in 0..count {
        let marker = if index == count / 2 {
            " baselinequeryneedle "
        } else {
            " "
        };
        let document = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            format!("Baseline document {index}"),
            Some(format!("https://baseline.example/{index}")),
            format!(
                "NeuralIA memory baseline corpus item {index}.{marker}                 Retrieval, provenance, browser research and local semantic memory."
            ),
        );
        if index == count / 2 {
            sentinel_id = Some(document.id.clone());
        }
        fs::write(
            documents.join(format!("{}.json", document.id)),
            serde_json::to_vec(&document).map_err(io::Error::other)?,
        )?;
    }

    sentinel_id.ok_or_else(|| io::Error::other("baseline corpus has no sentinel document"))
}

fn median_micros(mut operation: impl FnMut(), repetitions: usize) -> u128 {
    let mut samples = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        let started = Instant::now();
        operation();
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn process_metrics() -> io::Result<ProcessMetrics> {
    let script = format!(
        "$p=Get-Process -Id {};          [pscustomobject]@{{rss_bytes=[uint64]$p.WorkingSet64;threads=[uint64]$p.Threads.Count}}          | ConvertTo-Json -Compress",
        std::process::id()
    );
    let output = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(io::Error::other)?;
    Ok(ProcessMetrics {
        rss_bytes: value
            .get("rss_bytes")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| io::Error::other("missing rss_bytes"))?,
        threads: value
            .get("threads")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| io::Error::other("missing threads"))?,
    })
}

fn measure(size: usize) -> io::Result<Baseline> {
    let root = temp_root(size);
    let store = MemoryStore::new(&root)?;
    let sentinel_id = seed_documents(&store, size)?;

    let query = MemoryQuery::new("baselinequeryneedle");
    let warm = store.query(&query)?;
    assert!(
        warm.iter().any(|hit| hit.id == sentinel_id),
        "sentinel document must be present in hybrid results"
    );

    let hybrid_query_median_us = median_micros(
        || {
            let hits = store.query(&query).expect("hybrid baseline query");
            assert!(
                hits.iter().any(|hit| hit.id == sentinel_id),
                "sentinel document must remain in hybrid results"
            );
        },
        3,
    );

    let reindex_started = Instant::now();
    store.rebuild()?;
    let reindex_ms = reindex_started.elapsed().as_millis();

    let database = Database::open(&root.join("db").join("neural-memory.sqlite"))?;
    let fts_matches = database.count_fts("baselinequeryneedle")?;
    assert_eq!(fts_matches, 1);

    let fts_query_median_us = median_micros(
        || {
            assert_eq!(
                database
                    .count_fts("baselinequeryneedle")
                    .expect("FTS-only baseline query"),
                1
            );
        },
        21,
    );

    thread::sleep(Duration::from_millis(250));
    let metrics = process_metrics()?;

    let result = Baseline {
        documents: size,
        hybrid_query_median_us,
        fts_query_median_us,
        reindex_ms,
        rss_bytes_idle: metrics.rss_bytes,
        threads_idle: metrics.threads,
        fts_matches,
    };

    println!(
        "{}",
        serde_json::to_string(&result).map_err(io::Error::other)?
    );
    let _ = fs::remove_dir_all(root);
    Ok(result)
}

#[test]
#[ignore = "manual SPEC-0107 phase1 baseline on Windows reference machine"]
fn spec_0107_phase1_baseline_1k() {
    measure(1_000).unwrap();
}

#[test]
#[ignore = "manual SPEC-0107 phase1 baseline on Windows reference machine"]
fn spec_0107_phase1_baseline_10k() {
    measure(10_000).unwrap();
}

#[test]
#[ignore = "manual SPEC-0107 phase1 baseline on Windows reference machine"]
fn spec_0107_phase1_baseline_100k() {
    measure(100_000).unwrap();
}
