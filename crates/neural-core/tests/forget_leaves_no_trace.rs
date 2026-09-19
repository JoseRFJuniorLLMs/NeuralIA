//! "Esquecer" tem de apagar os bytes, nao so a vista da API.
//!
//! `forget_session_removes_source_and_derived_files` (em memory.rs) ja prova
//! que `store.documents()` deixa de os devolver. Isso nao chega: o indice
//! SQLite escreve em WAL, e um `-wal`/`-shm` que sobreviva ao apagamento
//! guarda imagens de pagina com o texto que o utilizador mandou esquecer.
//! Estes testes leem o disco inteiro, ficheiro a ficheiro, e procuram o texto.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use neural_core::{ForgetScope, MemoryDocument, MemoryKind, MemorySourceKind, MemoryStore};

const NEEDLE: &str = "SEGREDOxyz123ABC";

fn temp_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "neuralia-forget-trace-{name}-{}-{nonce}",
        std::process::id()
    ))
}

/// Todos os ficheiros por baixo de `dir` cujos bytes contenham `needle`.
fn files_containing(dir: &Path, needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return hits;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            hits.extend(files_containing(&path, needle));
            continue;
        }
        if let Ok(bytes) = fs::read(&path)
            && bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        {
            hits.push(path.display().to_string());
        }
    }
    hits
}

fn store_with_secrets(name: &str) -> (PathBuf, MemoryStore) {
    let root = temp_root(name);
    let store = MemoryStore::new(&root).expect("store abre");
    for index in 0..40 {
        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Web,
                format!("Titulo {index}"),
                Some(format!("https://example.com/p/{index}")),
                format!("corpo {index} com {NEEDLE} no meio do texto"),
            ))
            .expect("captura guarda");
    }
    assert!(
        !files_containing(&root, NEEDLE).is_empty(),
        "a fixture tinha de deixar o segredo no disco antes do forget"
    );
    (root, store)
}

#[test]
fn forget_all_leaves_no_trace_on_disk() {
    let (root, store) = store_with_secrets("all");
    store.forget(ForgetScope::All).expect("forget corre");

    let remaining = files_containing(&root, NEEDLE);
    assert!(remaining.is_empty(), "segredo sobreviveu em {remaining:?}");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn forget_by_domain_leaves_no_trace_on_disk() {
    // O caso comum -- "esquece este site" -- nao apaga o ficheiro SQLite:
    // reconstroi-o. E o caminho onde o WAL antigo tem mais hipoteses de
    // sobreviver com as paginas la dentro.
    let (root, store) = store_with_secrets("domain");
    store
        .forget(ForgetScope::Domain("example.com".into()))
        .expect("forget corre");

    let remaining = files_containing(&root, NEEDLE);
    assert!(remaining.is_empty(), "segredo sobreviveu em {remaining:?}");

    let _ = fs::remove_dir_all(&root);
}
