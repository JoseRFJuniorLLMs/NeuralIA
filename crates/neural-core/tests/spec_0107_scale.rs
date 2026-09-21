use std::{
    fs,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use neural_core::{MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore};

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "neuralia-spec-0107-scale-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

fn populate(root: &PathBuf, count: usize) -> MemoryStore {
    let store = MemoryStore::new(root).unwrap();
    for index in 0..count {
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
        let document = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Reader,
            format!("Documento {index} {topic}"),
            Some(format!("https://example.test/source/{index}")),
            format!(
                "{topic}. Corpo deterministico {index}.                  Entidades NeuralIA WebView2 SQLite Rust.                  Marcador de pesquisa corpus-{index}."
            ),
        );
        store.capture(document).unwrap();
    }
    store
}

fn best_elapsed(rounds: usize, mut run: impl FnMut()) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..rounds {
        let started = Instant::now();
        run();
        best = best.min(started.elapsed());
    }
    best
}

fn query_cost(store: &MemoryStore) -> Duration {
    best_elapsed(5, || {
        for _ in 0..16 {
            let hits = store
                .query(&MemoryQuery::new("WebView2 security semantic memory"))
                .unwrap();
            black_box(hits);
        }
    })
}

fn rebuild_cost(store: &MemoryStore) -> Duration {
    best_elapsed(3, || {
        store.rebuild().unwrap();
        black_box(());
    })
}

fn ratio(large: Duration, small: Duration) -> f64 {
    let small = small.as_nanos().max(1) as f64;
    large.as_nanos() as f64 / small
}

#[test]
fn spec_0107_query_and_rebuild_scale_by_ratio_not_absolute_clock() {
    let small_root = temp_root("small");
    let large_root = temp_root("large");

    // 4x corpus. O gate mede relações na mesma máquina, não segundos
    // absolutos de um runner compartilhado.
    let small = populate(&small_root, 128);
    let large = populate(&large_root, 512);

    // Alternar a ordem reduz o viés de cache/aquecimento.
    let query_large = query_cost(&large);
    let query_small = query_cost(&small);
    let rebuild_small = rebuild_cost(&small);
    let rebuild_large = rebuild_cost(&large);

    let query_ratio = ratio(query_large, query_small);
    let rebuild_ratio = ratio(rebuild_large, rebuild_small);

    eprintln!(
        "SPEC-0107 scale: query 128={:?} 512={:?} ratio={query_ratio:.2}x;          rebuild 128={:?} 512={:?} ratio={rebuild_ratio:.2}x",
        query_small, query_large, rebuild_small, rebuild_large
    );

    // Query usa shortlist limitado e não deve crescer nem perto de quadrático.
    // Rebuild percorre corpus inteiro e pode crescer aproximadamente linear.
    // Para 4x entrada, 8x deixa margem generosa para filesystem/SQLite sem
    // aceitar comportamento claramente superlinear.
    assert!(
        query_ratio < 8.0,
        "query 4x corpus custou {query_ratio:.2}x; regressão superlinear"
    );
    assert!(
        rebuild_ratio < 8.0,
        "rebuild 4x corpus custou {rebuild_ratio:.2}x; regressão superlinear"
    );

    let _ = fs::remove_dir_all(small_root);
    let _ = fs::remove_dir_all(large_root);
}
