//! Custo de guardar UM documento na memoria semantica em funcao do tamanho
//! do corpus. A captura acontece a cada navegacao, numa fila de 128; se o
//! custo crescer com o corpus, a fila satura e as capturas comecam a ser
//! deitadas fora em silencio ("memory queue saturated").

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use neural_core::{MemoryDocument, MemoryKind, MemorySourceKind, MemoryStore};

fn temp_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "neuralia-capture-cost-{name}-{}-{nonce}",
        std::process::id()
    ))
}

fn document(index: usize) -> MemoryDocument {
    MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Web,
        format!("Documento {index}"),
        Some(format!("https://example.com/pagina/{index}")),
        format!(
            "Corpo do documento {index} com texto suficiente para entrar no \
             indice lexical e gerar entidades e embedding como uma pagina real."
        ),
    )
}

/// Tempo medio de captura de `count` documentos, ja com `already` no corpus.
fn capture_batch(store: &MemoryStore, start: usize, count: usize) -> Duration {
    let began = Instant::now();
    for index in start..start + count {
        store.capture(document(index)).expect("captura guarda");
    }
    began.elapsed() / count as u32
}

#[test]
fn capture_cost_does_not_grow_with_the_corpus() {
    let root = temp_root("growth");
    let store = MemoryStore::new(&root).expect("store abre");

    const BATCH: usize = 25;
    // 600 chega para o defeito aparecer com folga (com um `integrity_check`
    // por escrita a captura ja custava ~9x a inicial) sem transformar o gate
    // numa espera: sao ~6 s de escritas reais em disco.
    const CORPUS: usize = 600;

    let first = capture_batch(&store, 0, BATCH).max(Duration::from_micros(1));
    // Enche o corpus entre as duas medicoes: e o tamanho do corpus que se
    // quer no eixo, nao o numero de capturas ja feitas.
    capture_batch(&store, BATCH, CORPUS - 2 * BATCH);
    let last = capture_batch(&store, CORPUS - BATCH, BATCH);
    println!("captura: primeiros {first:?}, ultimos {last:?} (corpus {CORPUS})");

    // Orcamento relativo, medido na mesma maquina e na mesma corrida: guardar
    // o documento 250 nao pode custar multiplos de guardar o documento 1. Um
    // `PRAGMA integrity_check` por escrita, ou reler o corpus inteiro para
    // contar ficheiros, aparece aqui como crescimento.
    assert!(
        last <= first * 4,
        "captura com corpus de {CORPUS}: {last:?} contra {first:?} no inicio"
    );

    let _ = fs::remove_dir_all(&root);
}
