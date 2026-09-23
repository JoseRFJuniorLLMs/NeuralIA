//! Leitor de EPUB e biblioteca no estilo do Calibre, sobre o núcleo em
//! `neural_core::epub` e `neural_core::library`.
//!
//! Tudo o que é da página vive numa origem própria,
//! `http://neuralia-epub.localhost` (o esquema `neuralia-epub` que o wry
//! expõe assim no Windows, como o `neuralia-pdf`):
//!
//! - `/library.html`, `/reader.html`, `/common.js`, `/library.js`,
//!   `/reader.js`, `/epub.css`: ficheiros do próprio NeuralIA, embutidos no
//!   binário e servidos com uma CSP estrita (script só de `'self'`, nada
//!   inline, `connect-src 'self'`).
//! - `/api/library` e `/api/book/<id>`: JSON só de leitura.
//! - `/cover/<id>`: a capa que a biblioteca extraiu.
//! - `/book/<id>/<caminho>`: uma entrada do EPUB pelo caminho normalizado,
//!   sempre com a CSP dos livros ([`BOOK_CSP`]): nenhum script, nenhuma
//!   rede, formulários desligados. Nada fora disto: 404.
//!
//! O livro é mostrado num `<iframe sandbox="allow-same-origin">` sem
//! `allow-scripts`; a página do leitor (mesma origem) pagina, trata os
//! cliques e pesquisa. As mensagens da página para o lado nativo passam pelo
//! `with_ipc_handler` do próprio WebView, com um parser fechado
//! ([`parse_epub_ipc`]) que confere a origem, a página e o tamanho; nunca
//! pelo `ipc.rs` nem pela capability das páginas remotas.
//!
//! A biblioteca vive numa thread própria ([`EpubWorker`]): copiar e ler um
//! livro grande nunca corre no event loop. Os recursos são servidos por outra
//! thread ([`spawn_epub_server`]), que lê um retrato imutável da biblioteca
//! ([`LibrarySnapshot`]) e por isso nunca espera por uma cópia em curso.

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use neural_core::{
    BookEntry, EpubArchive, EpubBook, EpubError, Library, LibraryError, PageProgression, Position,
    TocEntry,
    epub::normalize_entry_name,
    is_local_network_target,
    library::{BOOKS_DIR, COVERS_DIR, MAX_COVER_BYTES, MAX_LABEL_CHARS, is_valid_book_id},
    validate_web_url,
};
use serde_json::{Map, Value, json};
use url::Url;

/// Nome do esquema registado no WebView.
pub(crate) const EPUB_SCHEME: &str = "neuralia-epub";
/// Como o WebView2 mostra o esquema às páginas.
pub(crate) const EPUB_ORIGIN: &str = "http://neuralia-epub.localhost";
const EPUB_HOST: &str = "neuralia-epub.localhost";
pub(crate) const LIBRARY_PATH: &str = "/library.html";
pub(crate) const READER_PATH: &str = "/reader.html";

/// CSP das páginas do NeuralIA (biblioteca e leitor): script só dos nossos
/// ficheiros, nada inline, rede só para a própria origem.
pub(crate) const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-src 'self'; media-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// CSP de TODO o recurso de um livro. `script-src 'none'` além do sandbox
/// do iframe; imagens, estilos, fontes e media só da nossa origem ou `data:`;
/// uma URL `http(s)` remota nunca carrega.
pub(crate) const BOOK_CSP: &str = "default-src 'none'; img-src http://neuralia-epub.localhost data:; style-src http://neuralia-epub.localhost data: 'unsafe-inline'; font-src http://neuralia-epub.localhost data:; media-src http://neuralia-epub.localhost data:; script-src 'none'; form-action 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors http://neuralia-epub.localhost";

/// JSON e capas: não são documentos, mas nada neles deve correr.
const DATA_CSP: &str = "default-src 'none'; frame-ancestors 'none'";

/// Corpo máximo de uma mensagem da página.
pub(crate) const EPUB_IPC_MAX_BYTES: usize = 4 * 1024;
const MAX_URL_BYTES: usize = 2048;
/// Nenhum EPUB real tem tantos documentos; o núcleo recusa o que passar do fim.
const MAX_SPINE_INDEX: u64 = 100_000;
/// Um recurso servido de uma vez (o mesmo teto por entrada do núcleo).
const MAX_SERVED_BYTES: u64 = neural_core::epub::MAX_ENTRY_SIZE;
/// Pedidos à espera do servidor; acima disto responde-se 503 na hora.
const SERVER_QUEUE: usize = 256;
const WORKER_QUEUE: usize = 256;

const LIBRARY_HTML: &[u8] = include_bytes!("../../../assets/epub/library.html");
const READER_HTML: &[u8] = include_bytes!("../../../assets/epub/reader.html");
const COMMON_JS: &[u8] = include_bytes!("../../../assets/epub/common.js");
const LIBRARY_JS: &[u8] = include_bytes!("../../../assets/epub/library.js");
const READER_JS: &[u8] = include_bytes!("../../../assets/epub/reader.js");
const EPUB_CSS: &[u8] = include_bytes!("../../../assets/epub/epub.css");

/// Os ficheiros do NeuralIA, por caminho exato. Nenhum caminho vindo do
/// WebView vira caminho de disco.
pub(crate) fn epub_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    Some(match path {
        LIBRARY_PATH => ("text/html; charset=utf-8", LIBRARY_HTML),
        READER_PATH => ("text/html; charset=utf-8", READER_HTML),
        "/common.js" => ("text/javascript; charset=utf-8", COMMON_JS),
        "/library.js" => ("text/javascript; charset=utf-8", LIBRARY_JS),
        "/reader.js" => ("text/javascript; charset=utf-8", READER_JS),
        "/epub.css" => ("text/css; charset=utf-8", EPUB_CSS),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Retrato da biblioteca partilhado entre o worker e o servidor
// ---------------------------------------------------------------------------

/// O que o servidor precisa de saber da biblioteca, imutável depois de
/// publicado. O worker publica um novo a cada mudança.
#[derive(Debug, Clone, Default)]
pub(crate) struct LibrarySnapshot {
    pub dir: PathBuf,
    pub books: Vec<BookEntry>,
    /// Aviso para a página (índice danificado, biblioteca indisponível).
    pub notice: Option<String>,
}

impl LibrarySnapshot {
    fn book(&self, id: &str) -> Option<&BookEntry> {
        if !is_valid_book_id(id) {
            return None;
        }
        self.books.iter().find(|book| book.id == id)
    }

    fn book_file(&self, id: &str) -> Option<PathBuf> {
        self.book(id)
            .map(|book| self.dir.join(BOOKS_DIR).join(format!("{}.epub", book.id)))
    }

    fn cover_file(&self, id: &str) -> Option<(PathBuf, &'static str)> {
        let book = self.book(id)?;
        let (ext, mime) = match book.cover_ext.as_deref()? {
            "jpg" => ("jpg", "image/jpeg"),
            "png" => ("png", "image/png"),
            "gif" => ("gif", "image/gif"),
            "webp" => ("webp", "image/webp"),
            _ => return None,
        };
        Some((
            self.dir.join(COVERS_DIR).join(format!("{}.{ext}", book.id)),
            mime,
        ))
    }
}

/// O retrato em vigor. Antes do primeiro, o servidor espera (numa thread que
/// não é a da interface), para a página nunca ver uma biblioteca vazia só
/// porque perguntou cedo demais.
#[derive(Debug, Clone, Default)]
pub(crate) struct SharedLibrary {
    inner: Arc<(Mutex<Option<Arc<LibrarySnapshot>>>, Condvar)>,
}

impl SharedLibrary {
    fn publish(&self, snapshot: LibrarySnapshot) {
        let (slot, ready) = &*self.inner;
        let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
        *guard = Some(Arc::new(snapshot));
        ready.notify_all();
    }

    fn wait(&self) -> Arc<LibrarySnapshot> {
        let (slot, ready) = &*self.inner;
        let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
        loop {
            if let Some(snapshot) = guard.as_ref() {
                return Arc::clone(snapshot);
            }
            guard = ready
                .wait(guard)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}

// ---------------------------------------------------------------------------
// Mensagens de erro para o utilizador
// ---------------------------------------------------------------------------

/// A frase que a pessoa lê quando um livro não entra ou não abre.
pub(crate) fn library_error_message(error: &LibraryError) -> String {
    match error {
        LibraryError::Epub(error) => epub_error_message(error),
        LibraryError::TooLarge { .. } => {
            "Este livro é grande demais para a biblioteca e não pode ser aberto.".to_string()
        }
        LibraryError::Io(error) => format!("Não foi possível ler o arquivo: {error}."),
        LibraryError::NotFound(_) | LibraryError::InvalidId(_) => {
            "Este livro não está mais na biblioteca.".to_string()
        }
        LibraryError::InvalidPosition(_) => "A posição de leitura é inválida.".to_string(),
        LibraryError::TooManyBookmarks(limit) => {
            format!("Este livro já tem o máximo de {limit} marcadores.")
        }
        LibraryError::BookmarkNotFound { .. } => "Este marcador não existe mais.".to_string(),
        LibraryError::Json(_) => "Não foi possível gravar a biblioteca.".to_string(),
    }
}

pub(crate) fn epub_error_message(error: &EpubError) -> String {
    match error {
        EpubError::Drm(_) => "Este livro tem DRM e não pode ser aberto.".to_string(),
        EpubError::Limit { .. } => {
            "Este livro é grande demais para ser aberto com segurança.".to_string()
        }
        EpubError::UnsafeName(_) | EpubError::UnsafeXml { .. } | EpubError::Encrypted(_) => {
            "Este arquivo tem uma estrutura insegura e foi recusado.".to_string()
        }
        EpubError::Io(error) => format!("Não foi possível ler o arquivo: {error}."),
        EpubError::NotZip(_)
        | EpubError::Unsupported(_)
        | EpubError::UnsupportedCompression { .. }
        | EpubError::Corrupt { .. }
        | EpubError::NotFound(_)
        | EpubError::NotEpub(_)
        | EpubError::Xml { .. } => {
            "Este arquivo está corrompido ou não é um EPUB válido.".to_string()
        }
    }
}

fn load_notice(library: &Library) -> Option<String> {
    let report = library.load_report();
    if let Some(backup) = &report.backup {
        return Some(format!(
            "O índice da biblioteca estava danificado; uma cópia foi guardada em {}.",
            backup.display()
        ));
    }
    (report.skipped_entries > 0).then(|| {
        format!(
            "{} registro(s) inválido(s) do índice foram ignorados.",
            report.skipped_entries
        )
    })
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

// ---------------------------------------------------------------------------
// Worker da biblioteca
// ---------------------------------------------------------------------------

/// Uma operação sobre a biblioteca. Todas correm na thread do worker, por
/// ordem de chegada.
#[derive(Debug)]
pub(crate) enum EpubJob {
    /// Copia os livros para a biblioteca. `open`: quem pediu quer ler já.
    Add {
        paths: Vec<PathBuf>,
        open: bool,
    },
    SavePosition {
        id: String,
        spine: usize,
        fraction: f64,
    },
    Opened {
        id: String,
    },
    AddBookmark {
        id: String,
        spine: usize,
        fraction: f64,
        label: String,
    },
    RemoveBookmark {
        id: String,
        bookmark: u64,
    },
    Remove {
        id: String,
    },
    /// Responde quando tudo o que veio antes já foi feito (testes, sem
    /// relógio).
    #[cfg(test)]
    Flush(SyncSender<()>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddedBook {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddFailure {
    /// Nome do arquivo (sem a pasta).
    pub file: String,
    pub message: String,
}

/// O que o worker conta à interface.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EpubNotice {
    Added {
        books: Vec<AddedBook>,
        failures: Vec<AddFailure>,
        open: bool,
    },
    Removed {
        id: String,
        title: String,
    },
    Bookmarks {
        id: String,
    },
    Failed {
        message: String,
    },
}

impl EpubNotice {
    /// O aviso como a página o recebe (`window.neuraliaEpubNotice(...)`).
    pub(crate) fn page_json(&self) -> Value {
        match self {
            EpubNotice::Added {
                books, failures, ..
            } => json!({
                "kind": "added",
                "ids": books.iter().map(|book| book.id.clone()).collect::<Vec<_>>(),
                "errors": failures
                    .iter()
                    .map(|failure| json!({"file": failure.file, "message": failure.message}))
                    .collect::<Vec<_>>(),
            }),
            EpubNotice::Removed { id, title } => {
                json!({"kind": "removed", "id": id, "title": title})
            }
            EpubNotice::Bookmarks { id } => json!({"kind": "bookmarks", "id": id}),
            EpubNotice::Failed { message } => json!({"kind": "error", "message": message}),
        }
    }

    /// Uma linha para a Home quando a biblioteca não está aberta.
    pub(crate) fn status_line(&self) -> Option<String> {
        match self {
            EpubNotice::Added { failures, .. } if !failures.is_empty() => Some(
                failures
                    .iter()
                    .map(|failure| format!("“{}”: {}", failure.file, failure.message))
                    .collect::<Vec<_>>()
                    .join(" · "),
            ),
            EpubNotice::Failed { message } => Some(message.clone()),
            _ => None,
        }
    }
}

type Notify = Box<dyn Fn(EpubNotice) + Send>;

/// A thread dona da [`Library`]. Nada de disco da biblioteca corre na thread
/// da interface.
#[derive(Debug, Clone)]
pub(crate) struct EpubWorker {
    jobs: SyncSender<EpubJob>,
    shared: SharedLibrary,
}

impl EpubWorker {
    pub(crate) fn spawn(dir: PathBuf, notify: Notify) -> std::io::Result<Self> {
        let (jobs, receiver) = sync_channel::<EpubJob>(WORKER_QUEUE);
        let shared = SharedLibrary::default();
        let worker_shared = shared.clone();
        thread::Builder::new()
            .name("neural-epub-library".into())
            .spawn(move || run_worker(dir, receiver, worker_shared, notify))?;
        Ok(Self { jobs, shared })
    }

    /// Nunca bloqueia a interface: com a fila cheia o pedido cai e diz-se.
    pub(crate) fn submit(&self, job: EpubJob) -> bool {
        match self.jobs.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(job)) => {
                eprintln!("epub: fila da biblioteca cheia; pedido descartado: {job:?}");
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub(crate) fn shared(&self) -> SharedLibrary {
        self.shared.clone()
    }
}

fn publish_library(shared: &SharedLibrary, library: &Library, notice: Option<String>) {
    shared.publish(LibrarySnapshot {
        dir: library.dir().to_path_buf(),
        books: library.list().to_vec(),
        notice,
    });
}

fn run_worker(dir: PathBuf, receiver: Receiver<EpubJob>, shared: SharedLibrary, notify: Notify) {
    let mut library = match Library::open(&dir) {
        Ok(library) => {
            publish_library(&shared, &library, load_notice(&library));
            Some(library)
        }
        Err(error) => {
            shared.publish(LibrarySnapshot {
                dir: dir.clone(),
                books: Vec::new(),
                notice: Some(format!(
                    "A biblioteca não pôde ser aberta: {}",
                    library_error_message(&error)
                )),
            });
            None
        }
    };
    let mut notice = library.as_ref().and_then(load_notice);

    for job in receiver {
        #[cfg(test)]
        if let EpubJob::Flush(done) = job {
            let _ = done.send(());
            continue;
        }
        let Some(library) = library.as_mut() else {
            notify(EpubNotice::Failed {
                message: "A biblioteca de livros está indisponível.".to_string(),
            });
            continue;
        };
        let result = apply_job(library, job);
        publish_library(&shared, library, notice.clone());
        if let Some(outcome) = result {
            notify(outcome);
        }
        // O aviso do índice é dito uma vez; depois de uma gravação bem
        // sucedida o índice é o novo.
        notice = None;
    }
}

fn apply_job(library: &mut Library, job: EpubJob) -> Option<EpubNotice> {
    let failed = |error: LibraryError| {
        Some(EpubNotice::Failed {
            message: library_error_message(&error),
        })
    };
    match job {
        EpubJob::Add { paths, open } => {
            let mut books = Vec::new();
            let mut failures = Vec::new();
            for path in paths {
                match library.add(&path) {
                    Ok(entry) => books.push(AddedBook {
                        id: entry.id,
                        title: entry.title,
                    }),
                    Err(error) => failures.push(AddFailure {
                        file: path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        message: library_error_message(&error),
                    }),
                }
            }
            Some(EpubNotice::Added {
                books,
                failures,
                open,
            })
        }
        EpubJob::SavePosition {
            id,
            spine,
            fraction,
        } => library
            .set_position(
                &id,
                Position {
                    spine_index: spine,
                    fraction,
                    updated_unix: now_unix(),
                },
            )
            .err()
            .and_then(failed),
        EpubJob::Opened { id } => library
            .set_last_opened(&id, now_unix())
            .err()
            .and_then(failed),
        EpubJob::AddBookmark {
            id,
            spine,
            fraction,
            label,
        } => match library.add_bookmark(&id, spine, fraction, &label, now_unix()) {
            Ok(_) => Some(EpubNotice::Bookmarks { id }),
            Err(error) => failed(error),
        },
        EpubJob::RemoveBookmark { id, bookmark } => match library.remove_bookmark(&id, bookmark) {
            Ok(_) => Some(EpubNotice::Bookmarks { id }),
            Err(error) => failed(error),
        },
        EpubJob::Remove { id } => match library.remove(&id) {
            Ok(entry) => Some(EpubNotice::Removed {
                id: entry.id,
                title: entry.title,
            }),
            Err(error) => failed(error),
        },
        #[cfg(test)]
        EpubJob::Flush(done) => {
            let _ = done.send(());
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Mensagens da página (IPC)
// ---------------------------------------------------------------------------

/// Qual das nossas páginas mandou a mensagem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EpubPage {
    Library,
    Reader,
}

/// A lista fechada do que as páginas podem pedir.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EpubIpc {
    SavePosition {
        id: String,
        spine: usize,
        fraction: f64,
    },
    Opened {
        id: String,
    },
    AddBooks,
    RemoveBook {
        id: String,
    },
    AddBookmark {
        id: String,
        spine: usize,
        fraction: f64,
        label: String,
    },
    RemoveBookmark {
        id: String,
        bookmark: u64,
    },
    /// Já validada: `http(s)`, sem credenciais, fora da rede local.
    OpenExternal {
        url: String,
    },
    Close,
}

impl EpubIpc {
    fn allowed_from(&self, page: EpubPage) -> bool {
        match self {
            EpubIpc::AddBooks => true,
            EpubIpc::RemoveBook { .. } | EpubIpc::Close => page == EpubPage::Library,
            EpubIpc::SavePosition { .. }
            | EpubIpc::Opened { .. }
            | EpubIpc::AddBookmark { .. }
            | EpubIpc::RemoveBookmark { .. }
            | EpubIpc::OpenExternal { .. } => page == EpubPage::Reader,
        }
    }
}

/// O que só a thread da interface pode fazer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EpubUiRequest {
    /// Abrir o diálogo de arquivos (`*.epub`).
    AddBooks,
    /// Abrir a URL na vista web normal (já validada).
    OpenExternal(String),
    /// Sair da biblioteca para a Home.
    Close,
}

/// A página que mandou a mensagem: só as duas nossas, na nossa origem, sem
/// porta nem credenciais. O WebView2 dá a URL do documento de topo.
pub(crate) fn epub_page_of(source: &str) -> Option<EpubPage> {
    let url = Url::parse(source).ok()?;
    if url.scheme() != "http"
        || url.host_str() != Some(EPUB_HOST)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.path() {
        LIBRARY_PATH => Some(EpubPage::Library),
        READER_PATH => Some(EpubPage::Reader),
        _ => None,
    }
}

fn exact_keys(map: &Map<String, Value>, keys: &[&str]) -> Option<()> {
    (map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key))).then_some(())
}

fn ipc_book_id(map: &Map<String, Value>) -> Option<String> {
    let id = map.get("id")?.as_str()?;
    is_valid_book_id(id).then(|| id.to_string())
}

fn ipc_spine(map: &Map<String, Value>) -> Option<usize> {
    let spine = map.get("spine")?.as_u64()?;
    if spine > MAX_SPINE_INDEX {
        return None;
    }
    usize::try_from(spine).ok()
}

fn ipc_fraction(map: &Map<String, Value>) -> Option<f64> {
    let fraction = map.get("fraction")?.as_f64()?;
    (fraction.is_finite() && (0.0..=1.0).contains(&fraction)).then_some(fraction)
}

fn ipc_label(map: &Map<String, Value>) -> Option<String> {
    let label = map.get("label")?.as_str()?;
    (label.chars().count() <= MAX_LABEL_CHARS).then(|| label.to_string())
}

/// Lê uma mensagem de uma das nossas páginas. Tudo o que não for
/// exatamente uma das formas conhecidas (chaves a mais, tipos errados,
/// números fora do lugar, corpo grande, outra origem ou outra página) é
/// `None`.
pub(crate) fn parse_epub_ipc(source: &str, body: &str) -> Option<EpubIpc> {
    if body.len() > EPUB_IPC_MAX_BYTES {
        return None;
    }
    let page = epub_page_of(source)?;
    let Value::Object(map) = serde_json::from_str::<Value>(body).ok()? else {
        return None;
    };
    let message = match map.get("t")?.as_str()? {
        "savePosition" => {
            exact_keys(&map, &["t", "id", "spine", "fraction"])?;
            EpubIpc::SavePosition {
                id: ipc_book_id(&map)?,
                spine: ipc_spine(&map)?,
                fraction: ipc_fraction(&map)?,
            }
        }
        "opened" => {
            exact_keys(&map, &["t", "id"])?;
            EpubIpc::Opened {
                id: ipc_book_id(&map)?,
            }
        }
        "addBooks" => {
            exact_keys(&map, &["t"])?;
            EpubIpc::AddBooks
        }
        "removeBook" => {
            exact_keys(&map, &["t", "id"])?;
            EpubIpc::RemoveBook {
                id: ipc_book_id(&map)?,
            }
        }
        "addBookmark" => {
            exact_keys(&map, &["t", "id", "spine", "fraction", "label"])?;
            EpubIpc::AddBookmark {
                id: ipc_book_id(&map)?,
                spine: ipc_spine(&map)?,
                fraction: ipc_fraction(&map)?,
                label: ipc_label(&map)?,
            }
        }
        "removeBookmark" => {
            exact_keys(&map, &["t", "id", "bookmark"])?;
            EpubIpc::RemoveBookmark {
                id: ipc_book_id(&map)?,
                bookmark: map.get("bookmark")?.as_u64()?,
            }
        }
        "openExternal" => {
            exact_keys(&map, &["t", "url"])?;
            let url = map.get("url")?.as_str()?;
            if url.len() > MAX_URL_BYTES {
                return None;
            }
            let valid = validate_web_url(url).ok()?;
            if is_local_network_target(&valid) {
                return None;
            }
            EpubIpc::OpenExternal {
                url: valid.to_string(),
            }
        }
        "close" => {
            exact_keys(&map, &["t"])?;
            EpubIpc::Close
        }
        _ => return None,
    };
    message.allowed_from(page).then_some(message)
}

/// O corpo do `with_ipc_handler` das páginas EPUB: as operações da
/// biblioteca seguem para o worker; o que precisa da interface volta para
/// quem chamou (que o manda para o event loop).
pub(crate) fn handle_epub_ipc(
    source: &str,
    body: &str,
    worker: &EpubWorker,
) -> Option<EpubUiRequest> {
    let job = match parse_epub_ipc(source, body)? {
        EpubIpc::AddBooks => return Some(EpubUiRequest::AddBooks),
        EpubIpc::OpenExternal { url } => return Some(EpubUiRequest::OpenExternal(url)),
        EpubIpc::Close => return Some(EpubUiRequest::Close),
        EpubIpc::SavePosition {
            id,
            spine,
            fraction,
        } => EpubJob::SavePosition {
            id,
            spine,
            fraction,
        },
        EpubIpc::Opened { id } => EpubJob::Opened { id },
        EpubIpc::RemoveBook { id } => EpubJob::Remove { id },
        EpubIpc::AddBookmark {
            id,
            spine,
            fraction,
            label,
        } => EpubJob::AddBookmark {
            id,
            spine,
            fraction,
            label,
        },
        EpubIpc::RemoveBookmark { id, bookmark } => EpubJob::RemoveBookmark { id, bookmark },
    };
    worker.submit(job);
    None
}

// ---------------------------------------------------------------------------
// Arrastar e largar, diálogo de arquivos
// ---------------------------------------------------------------------------

/// `.epub`, sem distinguir maiúsculas.
pub(crate) fn is_epub_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
}

/// O que fazer com um lote de arquivos largados na janela: os `.epub` entram
/// na biblioteca e abrem; o resto é ignorado. `None` quando não há nenhum.
pub(crate) fn epub_drop_job(paths: Vec<PathBuf>) -> Option<EpubJob> {
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|path| is_epub_path(path))
        .collect();
    (!paths.is_empty()).then_some(EpubJob::Add { paths, open: true })
}

/// O filtro do `GetOpenFileNameW`: pares descrição/padrão terminados em NUL
/// e a lista inteira terminada por um NUL a mais.
pub(crate) fn epub_dialog_filter() -> Vec<u16> {
    let mut filter = Vec::new();
    for part in ["Livros EPUB (*.epub)", "*.epub"] {
        filter.extend(part.encode_utf16());
        filter.push(0);
    }
    filter.push(0);
    filter
}

/// Lê o buffer de um `GetOpenFileNameW` com `OFN_ALLOWMULTISELECT |
/// OFN_EXPLORER`: um arquivo só vem como caminho completo; vários vêm como
/// pasta seguida dos nomes, tudo separado por NUL e terminado por NUL duplo.
pub(crate) fn parse_dialog_selection(buffer: &[u16]) -> Vec<PathBuf> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, unit) in buffer.iter().enumerate() {
        if *unit != 0 {
            continue;
        }
        if index == start {
            break;
        }
        parts.push(String::from_utf16_lossy(&buffer[start..index]));
        start = index + 1;
    }
    match parts.as_slice() {
        [] => Vec::new(),
        [single] => vec![PathBuf::from(single)],
        [dir, names @ ..] => names
            .iter()
            .filter(|name| {
                // Um nome com separador ou `..` não é um nome de arquivo.
                !name.contains(['\\', '/']) && name.as_str() != ".." && name.as_str() != "."
            })
            .map(|name| Path::new(dir).join(name))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// O servidor da origem neuralia-epub
// ---------------------------------------------------------------------------

/// Uma resposta da origem, independente do tipo HTTP do wry (assim os testes
/// correm fora do Windows).
#[derive(Debug, Clone)]
pub(crate) struct EpubResponse {
    pub status: u16,
    pub content_type: Cow<'static, str>,
    pub csp: &'static str,
    pub body: Cow<'static, [u8]>,
}

impl EpubResponse {
    fn new(
        status: u16,
        content_type: impl Into<Cow<'static, str>>,
        csp: &'static str,
        body: impl Into<Cow<'static, [u8]>>,
    ) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            csp,
            body: body.into(),
        }
    }

    fn not_found() -> Self {
        Self::new(
            404,
            "text/plain; charset=utf-8",
            DATA_CSP,
            b"not found".as_slice(),
        )
    }

    fn json(status: u16, value: &Value) -> Self {
        Self::new(
            status,
            "application/json; charset=utf-8",
            DATA_CSP,
            serde_json::to_vec(value).unwrap_or_default(),
        )
    }

    /// Os cabeçalhos, pela ordem em que vão na resposta.
    pub(crate) fn headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Content-Type", self.content_type.to_string()),
            ("Content-Security-Policy", self.csp.to_string()),
            ("X-Content-Type-Options", "nosniff".to_string()),
            ("Cache-Control", "no-store".to_string()),
            ("Referrer-Policy", "no-referrer".to_string()),
        ]
    }
}

/// O livro aberto mais recentemente: servir os recursos de um capítulo não
/// relê o OPF a cada imagem.
struct OpenBook {
    id: String,
    archive: EpubArchive,
    book: EpubBook,
}

pub(crate) struct EpubServer {
    shared: SharedLibrary,
    open: Option<OpenBook>,
    /// Tamanho de cada documento do spine, por livro, para o progresso.
    sizes: HashMap<String, Option<Arc<Vec<u64>>>>,
}

impl EpubServer {
    pub(crate) fn new(shared: SharedLibrary) -> Self {
        Self {
            shared,
            open: None,
            sizes: HashMap::new(),
        }
    }

    /// Responde a um pedido (`path` sem a query, tal como vem no URI).
    pub(crate) fn respond(&mut self, method: &str, path: &str) -> EpubResponse {
        let head = method.eq_ignore_ascii_case("HEAD");
        if !head && !method.eq_ignore_ascii_case("GET") {
            return EpubResponse::new(
                405,
                "text/plain; charset=utf-8",
                DATA_CSP,
                b"method not allowed".as_slice(),
            );
        }
        let mut response = self.route(path);
        if head {
            response.body = Cow::Borrowed(&[]);
        }
        response
    }

    fn route(&mut self, path: &str) -> EpubResponse {
        if let Some((content_type, bytes)) = epub_asset(path) {
            return EpubResponse::new(200, content_type, PAGE_CSP, bytes);
        }
        let snapshot = self.shared.wait();
        if path == "/api/library" {
            let value = self.library_json(&snapshot);
            return EpubResponse::json(200, &value);
        }
        if let Some(id) = path.strip_prefix("/api/book/") {
            return self.book_json(&snapshot, id);
        }
        if let Some(id) = path.strip_prefix("/cover/") {
            return serve_cover(&snapshot, id);
        }
        if let Some(rest) = path.strip_prefix("/book/") {
            let Some((id, entry)) = rest.split_once('/') else {
                return EpubResponse::not_found();
            };
            return self.book_resource(&snapshot, id, entry);
        }
        EpubResponse::not_found()
    }

    fn open_book(
        &mut self,
        snapshot: &LibrarySnapshot,
        id: &str,
    ) -> Result<&OpenBook, EpubResponse> {
        let Some(file) = snapshot.book_file(id) else {
            return Err(EpubResponse::not_found());
        };
        if self.open.as_ref().is_none_or(|open| open.id != id) {
            self.open = None;
            let opened = EpubArchive::open(&file).and_then(|archive| {
                let book = EpubBook::parse(&archive)?;
                Ok((archive, book))
            });
            match opened {
                Ok((archive, book)) => {
                    self.open = Some(OpenBook {
                        id: id.to_string(),
                        archive,
                        book,
                    });
                }
                Err(error) => {
                    let status = if matches!(error, EpubError::Drm(_)) {
                        409
                    } else {
                        422
                    };
                    return Err(EpubResponse::json(
                        status,
                        &json!({"error": epub_error_message(&error)}),
                    ));
                }
            }
        }
        self.open.as_ref().ok_or_else(EpubResponse::not_found)
    }

    fn spine_sizes(&mut self, snapshot: &LibrarySnapshot, id: &str) -> Option<Arc<Vec<u64>>> {
        if let Some(sizes) = self.sizes.get(id) {
            return sizes.clone();
        }
        let sizes = snapshot.book_file(id).and_then(|file| {
            let archive = EpubArchive::open(file).ok()?;
            let book = EpubBook::parse(&archive).ok()?;
            Some(Arc::new(spine_entry_sizes(&archive, &book)))
        });
        self.sizes.insert(id.to_string(), sizes.clone());
        sizes
    }

    fn library_json(&mut self, snapshot: &LibrarySnapshot) -> Value {
        let mut books = Vec::with_capacity(snapshot.books.len());
        for book in &snapshot.books {
            let progress = match book.position {
                Some(position) => {
                    let sizes = self.spine_sizes(snapshot, &book.id);
                    Some(book_progress(
                        sizes.as_deref().map(Vec::as_slice),
                        book.spine_len,
                        position,
                    ))
                }
                None => None,
            };
            books.push(json!({
                "id": book.id,
                "title": book.title,
                "authors": book.authors,
                "language": book.language,
                "publisher": book.publisher,
                "series": book.series,
                "seriesIndex": book.series_index,
                "cover": book.cover_ext.as_ref().map(|_| format!("/cover/{}", book.id)),
                "added": book.added_unix,
                "lastOpened": book.last_opened_unix,
                "progress": progress,
                "fileSize": book.file_size,
                "sourceName": book.source_name,
            }));
        }
        // Livros que já não existem deixam de ocupar o cache.
        self.sizes
            .retain(|id, _| snapshot.books.iter().any(|book| &book.id == id));
        json!({"books": books, "notice": snapshot.notice})
    }

    fn book_json(&mut self, snapshot: &LibrarySnapshot, id: &str) -> EpubResponse {
        let Some(entry) = snapshot.book(id).cloned() else {
            return EpubResponse::not_found();
        };
        let open = match self.open_book(snapshot, id) {
            Ok(open) => open,
            Err(response) => return response,
        };
        let sizes = spine_entry_sizes(&open.archive, &open.book);
        let spine: Vec<Value> = open
            .book
            .spine
            .iter()
            .zip(&sizes)
            .map(|(item, size)| {
                json!({
                    "path": item.path,
                    "href": book_href(id, &item.path),
                    "linear": item.linear,
                    "size": size,
                })
            })
            .collect();
        let direction = match open.book.page_progression {
            PageProgression::Rtl => "rtl",
            PageProgression::Ltr => "ltr",
            PageProgression::Default => "default",
        };
        let value = json!({
            "id": entry.id,
            "title": entry.title,
            "authors": entry.authors,
            "language": entry.language,
            "direction": direction,
            "spine": spine,
            "toc": toc_json(&open.book.toc),
            "position": entry.position.map(|position| json!({
                "spine": position.spine_index,
                "fraction": position.fraction,
            })),
            "bookmarks": entry
                .bookmarks
                .iter()
                .map(|mark| json!({
                    "id": mark.id,
                    "spine": mark.spine_index,
                    "fraction": mark.fraction,
                    "label": mark.label,
                    "created": mark.created_unix,
                }))
                .collect::<Vec<_>>(),
        });
        self.sizes.insert(id.to_string(), Some(Arc::new(sizes)));
        EpubResponse::json(200, &value)
    }

    fn book_resource(&mut self, snapshot: &LibrarySnapshot, id: &str, raw: &str) -> EpubResponse {
        // O caminho tem de chegar já normalizado: `..`, `.`, segmentos
        // vazios, `\`, NUL, letra de unidade ou um `%` mal formado são 404.
        let Some(name) = percent_decode_strict(raw) else {
            return EpubResponse::not_found();
        };
        if normalize_entry_name(&name).as_deref() != Some(name.as_str()) {
            return EpubResponse::not_found();
        }
        let open = match self.open_book(snapshot, id) {
            Ok(open) => open,
            Err(response) => {
                // Um livro que não abre não tem recursos.
                return if response.status == 404 {
                    response
                } else {
                    EpubResponse::not_found()
                };
            }
        };
        let Some(entry) = open.archive.entry(&name) else {
            return EpubResponse::not_found();
        };
        let entry_name = entry.name().to_string();
        let declared = open
            .book
            .item_for_path(&entry_name)
            .map(|item| item.media_type.as_str());
        let content_type = served_content_type(declared, &entry_name);
        match open.archive.read_capped(&entry_name, MAX_SERVED_BYTES) {
            Ok(bytes) => EpubResponse::new(200, content_type, BOOK_CSP, bytes),
            Err(_) => EpubResponse::new(
                500,
                "text/plain; charset=utf-8",
                BOOK_CSP,
                b"unreadable entry".as_slice(),
            ),
        }
    }
}

fn serve_cover(snapshot: &LibrarySnapshot, id: &str) -> EpubResponse {
    let Some((file, mime)) = snapshot.cover_file(id) else {
        return EpubResponse::not_found();
    };
    match std::fs::metadata(&file) {
        Ok(meta) if meta.is_file() && meta.len() <= MAX_COVER_BYTES => {}
        _ => return EpubResponse::not_found(),
    }
    match std::fs::read(&file) {
        Ok(bytes) => EpubResponse::new(200, mime, DATA_CSP, bytes),
        Err(_) => EpubResponse::not_found(),
    }
}

/// Tamanho descompactado de cada documento do spine (0 se faltar).
fn spine_entry_sizes(archive: &EpubArchive, book: &EpubBook) -> Vec<u64> {
    book.spine
        .iter()
        .map(|item| archive.entry(&item.path).map_or(0, |entry| entry.size()))
        .collect()
}

/// Quanto do livro já foi lido (0..=1), pesando cada documento pelo seu
/// tamanho; sem tamanhos, cada documento vale o mesmo.
pub(crate) fn book_progress(sizes: Option<&[u64]>, spine_len: usize, position: Position) -> f64 {
    let fraction = if position.fraction.is_finite() {
        position.fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if let Some(sizes) = sizes {
        let total: u64 = sizes.iter().sum();
        if total > 0 && position.spine_index < sizes.len() {
            let before: u64 = sizes[..position.spine_index].iter().sum();
            let here = sizes[position.spine_index] as f64 * fraction;
            return ((before as f64 + here) / total as f64).clamp(0.0, 1.0);
        }
    }
    if spine_len == 0 {
        return 0.0;
    }
    ((position.spine_index as f64 + fraction) / spine_len as f64).clamp(0.0, 1.0)
}

fn toc_json(entries: &[TocEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| {
            json!({
                "label": entry.label,
                "path": entry.path,
                "fragment": entry.fragment,
                "spine": entry.spine_index,
                "children": toc_json(&entry.children),
            })
        })
        .collect()
}

/// `/book/<id>/<caminho codificado>`.
pub(crate) fn book_href(id: &str, path: &str) -> String {
    let mut href = format!("/book/{id}/");
    for (index, segment) in path.split('/').enumerate() {
        if index > 0 {
            href.push('/');
        }
        for byte in segment.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                href.push(byte as char);
            } else {
                href.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    href
}

/// `%XX` → byte; um `%` sem dois dígitos hex ou bytes que não formam UTF-8
/// recusam o caminho inteiro.
fn percent_decode_strict(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let high = (*bytes.get(at + 1)? as char).to_digit(16)?;
            let low = (*bytes.get(at + 2)? as char).to_digit(16)?;
            out.push((high * 16 + low) as u8);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// O tipo com que um recurso do livro é servido. O declarado no manifest
/// vale só se estiver na lista do que um livro legítimo usa; o resto vai pela
/// extensão, e o que não for conhecido (JavaScript incluído) vai como
/// `application/octet-stream`.
pub(crate) fn served_content_type(declared: Option<&str>, path: &str) -> &'static str {
    if let Some(known) = declared.and_then(known_media_type) {
        return known;
    }
    let ext = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "xhtml" | "xht" => "application/xhtml+xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn known_media_type(declared: &str) -> Option<&'static str> {
    let essence = declared
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match essence.as_str() {
        "application/xhtml+xml" => "application/xhtml+xml",
        "text/html" => "text/html",
        "text/css" => "text/css",
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/svg+xml" => "image/svg+xml",
        "image/bmp" => "image/bmp",
        "image/avif" => "image/avif",
        "font/ttf"
        | "application/x-font-ttf"
        | "application/x-font-truetype"
        | "application/font-sfnt" => "font/ttf",
        "font/otf" | "application/vnd.ms-opentype" | "application/x-font-opentype" => "font/otf",
        "font/woff" | "application/font-woff" => "font/woff",
        "font/woff2" => "font/woff2",
        "audio/mpeg" | "audio/mp3" => "audio/mpeg",
        "audio/mp4" => "audio/mp4",
        "audio/ogg" => "audio/ogg",
        "audio/webm" => "audio/webm",
        "video/mp4" => "video/mp4",
        "video/webm" => "video/webm",
        "text/plain" => "text/plain",
        _ => return None,
    })
}

/// Um pedido para o servidor: método, caminho e a quem responder.
pub(crate) struct ServeJob {
    pub method: String,
    pub path: String,
    pub reply: Box<dyn FnOnce(EpubResponse) + Send>,
}

/// A thread que serve a origem. Cada pedido recebido é respondido: o
/// WebView2 fica à espera do adiamento até lá.
pub(crate) fn spawn_epub_server(shared: SharedLibrary) -> std::io::Result<SyncSender<ServeJob>> {
    let (sender, receiver) = sync_channel::<ServeJob>(SERVER_QUEUE);
    thread::Builder::new()
        .name("neural-epub-server".into())
        .spawn(move || {
            let mut server = EpubServer::new(shared);
            for job in receiver {
                let response = server.respond(&job.method, &job.path);
                (job.reply)(response);
            }
        })?;
    Ok(sender)
}

/// Entrega um pedido ao servidor; com a fila cheia (ou o servidor morto)
/// responde já 503, para o pedido nunca ficar pendurado.
pub(crate) fn dispatch_epub_request(server: &SyncSender<ServeJob>, job: ServeJob) {
    match server.try_send(job) {
        Ok(()) => {}
        Err(TrySendError::Full(job)) | Err(TrySendError::Disconnected(job)) => {
            (job.reply)(EpubResponse::new(
                503,
                "text/plain; charset=utf-8",
                DATA_CSP,
                b"busy".as_slice(),
            ));
        }
    }
}

/// Os dois pedaços que vivem enquanto a app vive: o worker da biblioteca e
/// o servidor da origem. Criados na primeira vez que se abre um livro.
pub(crate) struct EpubRuntime {
    pub worker: EpubWorker,
    pub server: SyncSender<ServeJob>,
}

impl EpubRuntime {
    pub(crate) fn start(dir: PathBuf, notify: Notify) -> std::io::Result<Self> {
        let worker = EpubWorker::spawn(dir, notify)?;
        let server = spawn_epub_server(worker.shared())?;
        Ok(Self { worker, server })
    }
}

/// A navegação de topo do WebView das páginas EPUB: só a nossa origem (e
/// `about:blank`). Tudo o resto é recusado; os links externos chegam pelo
/// IPC, validados, e abrem na vista web normal.
pub(crate) fn epub_navigation_allowed(target: &str) -> bool {
    if target.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    let Ok(url) = Url::parse(target) else {
        return false;
    };
    url.scheme() == "http"
        && url.host_str() == Some(EPUB_HOST)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && (url.path() == LIBRARY_PATH || url.path() == READER_PATH)
}

/// A URL do leitor para um livro (id já validado pela biblioteca).
pub(crate) fn reader_url(id: &str) -> Option<String> {
    is_valid_book_id(id).then(|| format!("{EPUB_ORIGIN}{READER_PATH}?book={id}"))
}

pub(crate) fn library_url() -> String {
    format!("{EPUB_ORIGIN}{LIBRARY_PATH}")
}

/// A chamada que entrega um aviso à página aberta. O JSON é uma expressão
/// JavaScript válida; nada do aviso é interpretado como código.
pub(crate) fn notice_script(notice: &EpubNotice) -> String {
    format!(
        "window.neuraliaEpubNotice && window.neuraliaEpubNotice({});",
        notice.page_json()
    )
}

#[cfg(test)]
mod tests;
