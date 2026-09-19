//! A ponte entre português e inglês tem de sobreviver ao atalho lexical.
//!
//! O `hashed_embedding` constrói uma camada semântica mínima: "navegador" e
//! "browser" partilham uma feature canónica. A partir do momento em que a
//! pesquisa passou a escolher candidatos por FTS antes de pontuar, essa ponte
//! deixou de servir para nada sempre que ALGUM documento batia pela palavra
//! exacta: o documento em inglês não entrava na lista de candidatos, e a fase
//! semântica nem chegava a vê-lo.
//!
//! Medido antes da correcção, com os dois documentos deste teste:
//!   com FTS  -> ["Navegadores leves"]
//!   sem FTS  -> ["Navegadores leves", "Browser engines"]

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use neural_core::{MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore};

fn temp_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "neuralia-semantic-recall-{name}-{}-{nonce}",
        std::process::id()
    ))
}

fn store_with_both_languages(name: &str) -> (PathBuf, MemoryStore) {
    let root = temp_root(name);
    let store = MemoryStore::new(&root).expect("store abre");
    store
        .capture(MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Navegadores leves",
            Some("https://example.com/nav".into()),
            "navegador rapido e leve para uso diario",
        ))
        .expect("captura guarda");
    store
        .capture(MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Browser engines",
            Some("https://example.com/eng".into()),
            "browser engines and rendering pipelines explained in depth",
        ))
        .expect("captura guarda");
    (root, store)
}

#[test]
fn portuguese_query_finds_the_english_document() {
    let (root, store) = store_with_both_languages("pt-en");

    let titles = store
        .query(&MemoryQuery::new("navegador"))
        .expect("consulta corre")
        .into_iter()
        .map(|hit| hit.title)
        .collect::<Vec<_>>();

    assert!(
        titles.iter().any(|title| title == "Navegadores leves"),
        "{titles:?}"
    );
    assert!(
        titles.iter().any(|title| title == "Browser engines"),
        "o documento em inglês tem de entrar nos candidatos: {titles:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn english_query_finds_the_portuguese_document() {
    let (root, store) = store_with_both_languages("en-pt");

    let titles = store
        .query(&MemoryQuery::new("browser"))
        .expect("consulta corre")
        .into_iter()
        .map(|hit| hit.title)
        .collect::<Vec<_>>();

    assert!(
        titles.iter().any(|title| title == "Browser engines"),
        "{titles:?}"
    );
    assert!(
        titles.iter().any(|title| title == "Navegadores leves"),
        "o documento em português tem de entrar nos candidatos: {titles:?}"
    );

    let _ = fs::remove_dir_all(&root);
}
