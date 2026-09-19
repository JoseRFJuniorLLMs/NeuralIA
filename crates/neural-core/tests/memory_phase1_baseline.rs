#![cfg(windows)]

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    ptr,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use neural_core::{MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore};
use serde::Serialize;

type Sqlite = *mut c_void;
type Statement = *mut c_void;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const REINDEX_TIMEOUT: Duration = Duration::from_secs(180);

#[link(name = "winsqlite3")]
unsafe extern "C" {
    fn sqlite3_open(filename: *const c_char, database: *mut Sqlite) -> c_int;
    fn sqlite3_close(database: Sqlite) -> c_int;
    fn sqlite3_exec(
        database: Sqlite,
        sql: *const c_char,
        callback: *mut c_void,
        context: *mut c_void,
        error: *mut *mut c_char,
    ) -> c_int;
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
    fn sqlite3_free(pointer: *mut c_void);
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

    fn exec(&self, sql: &str) -> io::Result<()> {
        let sql = CString::new(sql).map_err(|_| io::Error::other("SQL contains NUL"))?;
        let mut error: *mut c_char = ptr::null_mut();
        let code = unsafe {
            sqlite3_exec(
                self.0,
                sql.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error,
            )
        };
        if code == SQLITE_OK {
            return Ok(());
        }

        let message = if !error.is_null() {
            let text = unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned();
            unsafe { sqlite3_free(error.cast()) };
            text
        } else {
            self.error_message()
        };
        Err(io::Error::other(message))
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
    fts_build_ms: u128,
    reindex_ms: Option<u128>,
    reindex_timed_out: bool,
    reindex_lower_bound_ms: u128,
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
                "NeuralIA memory baseline corpus item {index}.{marker}\
                 Retrieval, provenance, browser research and local semantic memory."
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

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\0', "").replace('\'', "''"))
}

fn build_fts_baseline(path: &Path, count: usize) -> io::Result<u128> {
    let _ = fs::remove_file(path);
    let started = Instant::now();
    let database = Database::open(path)?;
    database.exec("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    database.exec(
        "CREATE VIRTUAL TABLE memory_fts USING fts5(\
         id UNINDEXED,title,body,entities,tokenize='unicode61 remove_diacritics 2');\
         BEGIN IMMEDIATE;",
    )?;

    let mut rows = Vec::with_capacity(500);
    for index in 0..count {
        let marker = if index == count / 2 {
            " baselinequeryneedle "
        } else {
            " "
        };
        rows.push(format!(
            "({},{},{},{})",
            quote(&format!("baseline-{index}")),
            quote(&format!("Baseline document {index}")),
            quote(&format!(
                "NeuralIA memory baseline corpus item {index}.{marker}\
                 Retrieval, provenance, browser research and local semantic memory."
            )),
            quote("NeuralIA memory research")
        ));

        if rows.len() == 500 {
            database.exec(&format!(
                "INSERT INTO memory_fts(id,title,body,entities) VALUES {};",
                rows.join(",")
            ))?;
            rows.clear();
        }
    }

    if !rows.is_empty() {
        database.exec(&format!(
            "INSERT INTO memory_fts(id,title,body,entities) VALUES {};",
            rows.join(",")
        ))?;
    }
    database.exec("COMMIT;")?;
    Ok(started.elapsed().as_millis())
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
        "$p=Get-Process -Id {}; \
         [pscustomobject]@{{rss_bytes=[uint64]$p.WorkingSet64;threads=[uint64]$p.Threads.Count}} \
         | ConvertTo-Json -Compress",
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

fn measure_reindex_with_timeout(root: &Path) -> (Option<u128>, bool, u128) {
    let root = root.to_path_buf();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let started = Instant::now();
        let result = MemoryStore::new(&root).and_then(|store| store.rebuild());
        let _ = sender.send((started.elapsed().as_millis(), result));
    });

    match receiver.recv_timeout(REINDEX_TIMEOUT) {
        Ok((elapsed, Ok(()))) => (Some(elapsed), false, elapsed),
        Ok((elapsed, Err(error))) => panic!("baseline rebuild failed after {elapsed} ms: {error}"),
        Err(mpsc::RecvTimeoutError::Timeout) => (
            None,
            true,
            REINDEX_TIMEOUT.as_millis(),
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("baseline rebuild worker disconnected")
        }
    }
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

    let repetitions = if size >= 100_000 { 1 } else { 3 };
    let hybrid_query_median_us = median_micros(
        || {
            let hits = store.query(&query).expect("hybrid baseline query");
            assert!(
                hits.iter().any(|hit| hit.id == sentinel_id),
                "sentinel document must remain in hybrid results"
            );
        },
        repetitions,
    );

    let fts_path = root.join("db").join("fts-query-baseline.sqlite");
    let fts_build_ms = build_fts_baseline(&fts_path, size)?;
    let database = Database::open(&fts_path)?;
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
    drop(database);

    let (reindex_ms, reindex_timed_out, reindex_lower_bound_ms) =
        measure_reindex_with_timeout(&root);

    let result = Baseline {
        documents: size,
        hybrid_query_median_us,
        fts_query_median_us,
        fts_build_ms,
        reindex_ms,
        reindex_timed_out,
        reindex_lower_bound_ms,
        rss_bytes_idle: metrics.rss_bytes,
        threads_idle: metrics.threads,
        fts_matches,
    };

    println!(
        "{}",
        serde_json::to_string(&result).map_err(io::Error::other)?
    );

    if !reindex_timed_out {
        let _ = fs::remove_dir_all(root);
    }
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
