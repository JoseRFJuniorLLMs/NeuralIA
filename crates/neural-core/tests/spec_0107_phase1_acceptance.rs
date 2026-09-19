use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use neural_core::{
    CaptureOutcome, ForgetScope, MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind,
    MemoryStore,
};

fn temp_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "neuralia-spec-0107-{name}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn spec_0107_phase1_sqlite_retrieval_forget_and_rebuild_are_operational() {
    let root = temp_root("phase1");
    let store = MemoryStore::new(&root).unwrap();

    let target = MemoryDocument::new(
        MemoryKind::Concept,
        MemorySourceKind::Web,
        "Rust ownership",
        Some("https://docs.example.com/rust".into()),
        "Rust ownership borrowing lifetimes compiler memory safety",
    )
    .provider("Reader");
    let target_id = target.id.clone();

    let noise = MemoryDocument::new(
        MemoryKind::Concept,
        MemorySourceKind::Web,
        "Cooking",
        Some("https://food.invalid/pasta".into()),
        "tomato basil pasta recipe kitchen",
    );

    assert!(matches!(
        store.capture(target).unwrap(),
        CaptureOutcome::Stored(_)
    ));
    assert!(matches!(
        store.capture(noise).unwrap(),
        CaptureOutcome::Stored(_)
    ));

    let sqlite = root.join("db").join("neural-memory.sqlite");
    assert!(sqlite.exists());
    let doctor = store.doctor(false).unwrap();
    assert!(doctor.sqlite_present);

    let hits = store
        .query(&MemoryQuery::new("rust ownership compiler"))
        .unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].id, target_id);
    assert!(
        hits[0]
            .matched_by
            .iter()
            .any(|source| source == "lexical" || source == "semantic")
    );

    let forgotten = store
        .forget(ForgetScope::Domain("EXAMPLE.com".into()))
        .unwrap();
    assert_eq!(forgotten.documents, 1);
    assert!(root.join("tombstones.json").exists());

    let recapture = MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Web,
        "Forgotten source",
        Some("https://deep.docs.example.com/again".into()),
        "must never come back",
    );
    assert_eq!(
        store.capture(recapture).unwrap(),
        CaptureOutcome::SkippedForgotten
    );

    let _ = fs::remove_file(&sqlite);
    let _ = fs::remove_file(format!("{}-wal", sqlite.to_string_lossy()));
    let _ = fs::remove_file(format!("{}-shm", sqlite.to_string_lossy()));

    store.rebuild().unwrap();
    assert!(sqlite.exists());

    let after_rebuild = store
        .query(&MemoryQuery::new("ownership compiler"))
        .unwrap();
    assert!(after_rebuild.iter().all(|hit| hit.id != target_id));

    let _ = fs::remove_dir_all(root);
}
