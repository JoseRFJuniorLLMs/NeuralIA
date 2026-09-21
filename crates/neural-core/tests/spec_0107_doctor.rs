use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use neural_core::{MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore};

fn temp_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "neuralia-spec-0107-doctor-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

#[test]
fn spec_0107_memory_doctor_reports_corruption_and_rebuilds_derived_index() {
    let root = temp_root();
    let store = MemoryStore::new(&root).unwrap();

    for (title, body) in [
        (
            "WebView2 security",
            "NeuralIA pesquisa prompt injection e WebView2 com proveniencia.",
        ),
        (
            "SQLite recovery",
            "O indice SQLite e derivado e pode ser reconstruido das fontes validas.",
        ),
    ] {
        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                title,
                Some("https://example.test/source".into()),
                body,
            ))
            .unwrap();
    }

    // Fonte inválida deve ser relatada, nunca silenciosamente promovida a
    // conhecimento. O rebuild pode ignorá-la e preservar as fontes válidas.
    let corrupt = root.join("documents").join("corrupt.json");
    fs::write(&corrupt, b"{ this is not json").unwrap();

    let before = store.doctor(false).unwrap();
    assert_eq!(before.documents, 2);
    assert_eq!(before.corrupt_documents, 1);
    assert!(!before.rebuilt);

    // Simula perda/corrupção do índice derivado. As fontes JSON/Markdown são
    // a evidência durável; o Doctor tem de conseguir recriar a busca.
    let sqlite = root.join("db").join("neural-memory.sqlite");
    let _ = fs::remove_file(&sqlite);
    let _ = fs::remove_file(format!("{}-wal", sqlite.to_string_lossy()));
    let _ = fs::remove_file(format!("{}-shm", sqlite.to_string_lossy()));
    assert!(!sqlite.exists());

    let report = store.doctor(true).unwrap();
    assert!(report.rebuilt);
    assert_eq!(report.documents, 2);
    assert_eq!(report.corrupt_documents, 1);
    assert!(report.sqlite_present);
    assert!(sqlite.exists());

    let hits = store
        .query(&MemoryQuery::new("WebView2 prompt injection"))
        .unwrap();
    assert!(
        hits.iter().any(|hit| hit.title == "WebView2 security"),
        "rebuild deve devolver pesquisa funcional sobre as fontes validas"
    );

    let _ = fs::remove_dir_all(root);
}
