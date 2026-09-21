use std::{
    env, fs,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use neural_core::{MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore};
use serde_json::json;

fn root_for(size: usize) -> PathBuf {
    env::temp_dir().join(format!(
        "neuralia-memory-scale-{size}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

fn as_millis(duration: Duration) -> u128 {
    duration.as_millis()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sizes = env::args()
        .skip(1)
        .map(|arg| arg.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()?;
    let sizes = if sizes.is_empty() {
        vec![1_000, 10_000, 100_000]
    } else {
        sizes
    };

    for size in sizes {
        let root = root_for(size);
        let store = MemoryStore::new(&root)?;

        let capture_started = Instant::now();
        for index in 0..size {
            let topic = match index % 8 {
                0 => "WebView2 prompt injection security",
                1 => "AVX-512 SIMD vector optimization",
                2 => "Rust memory ownership borrow checker",
                3 => "PDF reader semantic extraction",
                4 => "NeuralIA semantic memory research",
                5 => "SQLite FTS hybrid retrieval",
                6 => "Claude ChatGPT Gemini comparison",
                _ => "browser navigation accessibility timeline",
            };
            store.capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                format!("Documento {index} {topic}"),
                Some(format!("https://example.test/source/{index}")),
                format!(
                    "{topic}. Corpo deterministico {index}.                      NeuralIA WebView2 SQLite Rust corpus-{index}."
                ),
            ))?;
        }
        let capture = capture_started.elapsed();

        let query_started = Instant::now();
        for _ in 0..100 {
            black_box(store.query(&MemoryQuery::new(
                "WebView2 security semantic memory",
            ))?);
        }
        let query = query_started.elapsed();

        let rebuild_started = Instant::now();
        store.rebuild()?;
        let rebuild = rebuild_started.elapsed();

        let report = json!({
            "documents": size,
            "capture_ms": as_millis(capture),
            "query_100_ms": as_millis(query),
            "rebuild_ms": as_millis(rebuild),
            "root": root,
        });
        println!("{}", serde_json::to_string(&report)?);

        let _ = fs::remove_dir_all(root);
    }

    Ok(())
}
