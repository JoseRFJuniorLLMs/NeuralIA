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

#[test]
fn forget_all_removes_unparseable_and_leftover_temp_files() {
    // Um JSON truncado (crash a meio, disco cheio) ainda guarda o corpo da
    // pagina; um `.tmp` do atomic_write tambem. Nenhum deles se deixa ler
    // como MemoryDocument, e ambos tem de desaparecer na mesma.
    let (root, store) = store_with_secrets("unparseable");
    let documents = root.join("documents");
    fs::write(
        documents.join("broken.json"),
        format!("{{\"body\":\"corpo {NEEDLE}"),
    )
    .unwrap();
    fs::write(documents.join(".abc.json.1234.tmp"), NEEDLE).unwrap();
    let wiki = root.join("wiki").join("sources");
    fs::create_dir_all(&wiki).unwrap();
    fs::write(wiki.join(".abc.md.1234.tmp"), NEEDLE).unwrap();

    store.forget(ForgetScope::All).expect("forget corre");

    let remaining = files_containing(&root, NEEDLE);
    assert!(remaining.is_empty(), "segredo sobreviveu em {remaining:?}");

    let _ = fs::remove_dir_all(&root);
}

/// Primeiro `.json` de documents/, e um handle que o prende como um antivirus
/// ou indexador faria: aberto sem FILE_SHARE_DELETE, o DeleteFileW falha.
#[cfg(windows)]
fn lock_one_document(root: &Path) -> (PathBuf, fs::File) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    let path = fs::read_dir(root.join("documents"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .expect("ha documentos");
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .expect("abre sem FILE_SHARE_DELETE");
    (path, handle)
}

#[cfg(windows)]
#[test]
fn forget_reports_a_failed_delete_and_retries_it_next_time() {
    // O primeiro forget nao pode dizer "apagado", e o seguinte tem de voltar
    // a tentar: a tombstone do primeiro nao pode esconder o ficheiro para
    // sempre.
    let (root, store) = store_with_secrets("locked");
    let (locked, handle) = lock_one_document(&root);

    let first = store.forget(ForgetScope::All);
    assert!(locked.exists(), "o ficheiro devia ter resistido");
    drop(handle);
    assert!(
        first.is_err(),
        "um delete falhado nao pode ser reportado como sucesso: {first:?}"
    );

    store
        .forget(ForgetScope::All)
        .expect("segundo forget corre");
    assert!(!locked.exists(), "o forget seguinte nao voltou a tentar");
    let remaining = files_containing(&root, NEEDLE);
    assert!(remaining.is_empty(), "segredo sobreviveu em {remaining:?}");

    let _ = fs::remove_dir_all(&root);
}

#[cfg(windows)]
#[test]
fn forget_by_domain_retries_a_document_an_earlier_forget_failed_to_delete() {
    let (root, store) = store_with_secrets("locked-domain");
    let (locked, handle) = lock_one_document(&root);

    let first = store.forget(ForgetScope::Domain("example.com".into()));
    assert!(locked.exists(), "o ficheiro devia ter resistido");
    drop(handle);
    assert!(
        first.is_err(),
        "um delete falhado nao pode ser reportado como sucesso: {first:?}"
    );

    store
        .forget(ForgetScope::Domain("example.com".into()))
        .expect("segundo forget corre");
    assert!(!locked.exists(), "o forget seguinte nao voltou a tentar");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn forget_all_removes_the_copy_left_by_a_killed_rebuild() {
    // memory:rebuild constroi uma copia completa do corpus num temporario
    // antes de a trocar. Se o processo morre a meio, a copia fica em db/.
    let (root, store) = store_with_secrets("killed-rebuild");
    let db = root.join("db");
    fs::write(db.join(".neural-memory.sqlite.rebuild-1-1"), NEEDLE).unwrap();
    fs::write(db.join(".neural-memory.sqlite.rebuild-1-1-wal"), NEEDLE).unwrap();
    // Um pid reutilizado: o mesmo numero do processo actual.
    let own = std::process::id();
    fs::write(
        db.join(format!(".neural-memory.sqlite.rebuild-{own}-7")),
        NEEDLE,
    )
    .unwrap();

    store.forget(ForgetScope::All).expect("forget corre");

    let remaining = files_containing(&root, NEEDLE);
    assert!(remaining.is_empty(), "segredo sobreviveu em {remaining:?}");

    let _ = fs::remove_dir_all(&root);
}

#[cfg(windows)]
#[test]
fn failed_rebuild_swap_does_not_leave_a_copy_of_the_corpus() {
    use std::os::windows::fs::OpenOptionsExt;
    // O SQLite abre o ficheiro sem FILE_SHARE_DELETE; com outro handle assim
    // o rename do indice antigo falha, e cada tentativa deixava uma copia.
    const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
    let (root, store) = store_with_secrets("held-index");
    let db = root.join("db");
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ_WRITE)
        .open(db.join("neural-memory.sqlite"))
        .expect("indice existe");

    let result = store.rebuild();
    drop(held);
    assert!(result.is_err(), "o rename devia ter falhado: {result:?}");

    let leftovers = fs::read_dir(&db)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".rebuild-"))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "copia do corpus ficou em {leftovers:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rebuild_sweeps_the_copy_left_by_a_dead_process() {
    let (root, store) = store_with_secrets("dead-rebuild");
    let leftover = root.join("db").join(".neural-memory.sqlite.rebuild-1-1");
    fs::write(&leftover, NEEDLE).unwrap();

    store.rebuild().expect("rebuild corre");

    assert!(!leftover.exists(), "copia de um rebuild morto ficou em db/");
    let _ = fs::remove_dir_all(&root);
}
