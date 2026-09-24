//! Biblioteca de livros EPUB, no estilo do Calibre: uma pasta com o índice
//! `library.json`, o estado de leitura de cada livro, as cópias dos livros e
//! as capas extraídas.
//!
//! ```text
//! <dir>/library.json          índice (versão, livros e os seus metadados)
//! <dir>/library.json.lock     serializa quem escreve (outros processos também)
//! <dir>/state/<id>.json       estado de leitura: posição, marcadores, última abertura
//! <dir>/books/<id>.epub       cópia do livro; <id> = prefixo do SHA-256
//! <dir>/covers/<id>.<ext>     capa extraída (jpg, png, gif ou webp)
//! <dir>/.trash/               livros e capas removidos, e `<sha256>.json` com
//!                             o registro e o estado de leitura de cada um
//! ```
//!
//! - Mais de um processo pode abrir a mesma pasta (duas janelas do NeuralIA).
//!   Toda a escrita toma o lock exclusivo de `library.json.lock`, RELÊ do
//!   disco o que vai mudar (o índice, ou o estado daquele livro), aplica a
//!   mudança e grava: nada que o outro processo gravou se perde.
//! - Cada arquivo é gravado inteiro num temporário na mesma pasta seguido de
//!   `rename`: um corte a meio deixa a versão anterior intacta. Se a gravação
//!   falha, o estado em memória também não muda.
//! - Virar a página grava só `state/<id>.json` (pequeno), nunca o índice.
//! - Índice ilegível: o conteúdo vai para `library.json.bak` (sem apagar um
//!   `.bak` anterior diferente; um igual é reaproveitado) e a biblioteca abre
//!   com o que conseguir recuperar: cada `books/<id>.epub` que o índice não
//!   conhece volta a entrar (o SHA-256 é conferido e o livro relido), com a
//!   posição e os marcadores que estão em `state/`. Entradas individuais
//!   inválidas são descartadas, o original vai para o `.bak` e o índice limpo
//!   é gravado logo.
//! - `add` copia o arquivo (com teto de tamanho) calculando o SHA-256 no
//!   caminho, lê o EPUB a partir da cópia (o que se guarda é o que se leu) e
//!   só então recebe o nome definitivo. O mesmo livro duas vezes é o mesmo
//!   registro; um livro removido e adicionado de novo recupera os marcadores e
//!   a posição. DRM, ZIP hostil ou EPUB inválido: nada fica para trás.
//! - Temporários de um processo que morreu a meio (`.incoming-*`, `.write-*`,
//!   de outro processo e com mais de uma hora) são apagados ao abrir.
//! - Ids são validados antes de virarem caminho: só hex minúsculo com 16 a
//!   64 caracteres. A capa também não guarda caminho, só a extensão (de uma
//!   lista fechada), para um índice adulterado não apontar para fora da pasta.
//! - O tempo entra por argumento (`add_at`, `Position::updated_unix`...); só
//!   `add` e a limpeza de temporários leem o relógio.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::epub::{EpubArchive, EpubBook, EpubError, EpubMetadata, MAX_TOTAL_SIZE};

pub const INDEX_FILE: &str = "library.json";
pub const LOCK_FILE: &str = "library.json.lock";
pub const STATE_DIR: &str = "state";
pub const BOOKS_DIR: &str = "books";
pub const COVERS_DIR: &str = "covers";
pub const TRASH_DIR: &str = ".trash";
/// Tamanho máximo do arquivo EPUB aceito (o teto descompactado do ZIP mais
/// folga para cabeçalhos).
pub const MAX_BOOK_BYTES: u64 = MAX_TOTAL_SIZE + 64 * 1024 * 1024;
/// Capas maiores do que isto não são extraídas (o livro entra sem capa).
pub const MAX_COVER_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_BOOKMARKS: usize = 5_000;
pub const MAX_LABEL_CHARS: usize = 200;

const INDEX_VERSION: u32 = 2;
const ID_MIN_LEN: usize = 16;
const SHA256_HEX_LEN: usize = 64;
const COVER_EXTENSIONS: [&str; 4] = ["jpg", "png", "gif", "webp"];
/// Um `state/<id>.json` maior do que isto não foi escrito por nós.
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;
/// Um temporário de outro processo mais velho do que isto é de um processo
/// que morreu a meio (uma cópia em curso reescreve o arquivo sem parar).
const STALE_TEMP: Duration = Duration::from_secs(60 * 60);

static TEMP_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("id de livro inválido: {0:?}")]
    InvalidId(String),
    #[error("livro não encontrado na biblioteca: {0}")]
    NotFound(String),
    #[error("marcador {bookmark} não encontrado no livro {book}")]
    BookmarkNotFound { book: String, bookmark: u64 },
    #[error("posição inválida: {0}")]
    InvalidPosition(String),
    #[error("o livro já tem o máximo de {0} marcadores")]
    TooManyBookmarks(usize),
    #[error("arquivo grande demais para a biblioteca ({size} > {limit} bytes)")]
    TooLarge { size: u64, limit: u64 },
    #[error("{0}")]
    Epub(#[from] EpubError),
    /// Ler: o arquivo de origem, o índice, o estado de um livro.
    #[error("falha de E/S na biblioteca: {0}")]
    Io(#[from] io::Error),
    /// Gravar na pasta da biblioteca (disco cheio, sem permissão...): não é
    /// o arquivo de origem que está mal.
    #[error("falha ao gravar na biblioteca: {0}")]
    Write(#[source] io::Error),
    #[error("falha ao gravar o índice da biblioteca: {0}")]
    Json(#[from] serde_json::Error),
}

pub type LibraryResult<T> = std::result::Result<T, LibraryError>;

/// Onde a leitura parou: documento do spine e fração (0..=1) dentro dele.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub spine_index: usize,
    pub fraction: f64,
    pub updated_unix: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    /// Único entre os marcadores atuais do livro (o maior id + 1).
    pub id: u64,
    pub spine_index: usize,
    pub fraction: f64,
    pub label: String,
    pub created_unix: u64,
}

/// Um livro da biblioteca: o registro do índice mais o estado de leitura
/// (que vive em `state/<id>.json` e nunca é gravado no índice).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookEntry {
    /// Prefixo (16+ caracteres) do SHA-256 do arquivo; nome dos arquivos.
    pub id: String,
    pub sha256: String,
    pub title: String,
    pub authors: Vec<String>,
    /// Como ordenar pelo autor, como no Calibre ("Assis, Machado de"): o
    /// `file-as` do livro ou, na falta, o último nome primeiro.
    pub author_sort: String,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub date: Option<String>,
    pub subjects: Vec<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    /// Nome do arquivo de origem (só para exibir).
    pub source_name: String,
    pub file_size: u64,
    /// Documentos no spine: limite de `Position::spine_index`.
    pub spine_len: usize,
    /// Extensão da capa em `covers/<id>.<ext>`, se foi extraída.
    pub cover_ext: Option<String>,
    pub added_unix: u64,
    #[serde(skip_serializing)]
    pub last_opened_unix: Option<u64>,
    #[serde(skip_serializing)]
    pub position: Option<Position>,
    #[serde(skip_serializing)]
    pub bookmarks: Vec<Bookmark>,
}

/// O que a abertura encontrou de estranho no índice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Cópia do índice ilegível ou parcialmente inválido.
    pub backup: Option<PathBuf>,
    /// Entradas descartadas por serem inválidas.
    pub skipped_entries: usize,
    /// Livros em `books/` que o índice não tinha e voltaram a entrar.
    pub recovered: usize,
}

#[derive(Serialize)]
struct IndexOut<'a> {
    version: u32,
    books: &'a [BookEntry],
}

#[derive(Deserialize)]
struct IndexIn {
    #[serde(default)]
    books: Vec<serde_json::Value>,
}

/// `state/<id>.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct ReadingState {
    last_opened_unix: Option<u64>,
    position: Option<Position>,
    bookmarks: Vec<Bookmark>,
}

impl ReadingState {
    fn of(book: &BookEntry) -> Self {
        Self {
            last_opened_unix: book.last_opened_unix,
            position: book.position,
            bookmarks: book.bookmarks.clone(),
        }
    }

    fn apply_to(self, book: &mut BookEntry) {
        book.last_opened_unix = self.last_opened_unix;
        book.position = self.position;
        book.bookmarks = self.bookmarks;
    }

    fn sanitized(mut self) -> Self {
        if let Some(position) = &mut self.position {
            position.fraction = finite_fraction(position.fraction);
        }
        self.bookmarks.truncate(MAX_BOOKMARKS);
        for mark in &mut self.bookmarks {
            mark.fraction = finite_fraction(mark.fraction);
            mark.label = clean_label(&mark.label);
        }
        sort_bookmarks(&mut self.bookmarks);
        self
    }
}

/// `.trash/<sha256>.json`: o que o livro era quando saiu da biblioteca.
#[derive(Serialize, Deserialize)]
struct TrashRecord {
    entry: BookEntry,
    state: ReadingState,
}

/// O lock exclusivo da pasta; solta-se ao sair de cena.
struct DirLock(File);

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn lock_dir(dir: &Path) -> LibraryResult<DirLock> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
        .map_err(LibraryError::Write)?;
    file.lock().map_err(LibraryError::Write)?;
    Ok(DirLock(file))
}

/// Tamanho e data do índice: mudou desde a última leitura, outro processo
/// gravou.
type IndexStamp = Option<(u64, SystemTime)>;

fn index_stamp(dir: &Path) -> IndexStamp {
    let meta = fs::metadata(dir.join(INDEX_FILE)).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

#[derive(Debug)]
pub struct Library {
    dir: PathBuf,
    /// Mais recente primeiro.
    books: Vec<BookEntry>,
    report: LoadReport,
    stamp: IndexStamp,
}

impl Library {
    /// Abre (e cria, se preciso) a biblioteca em `dir`. Um índice ilegível não
    /// impede a abertura: ver [`Library::load_report`].
    pub fn open(dir: impl Into<PathBuf>) -> LibraryResult<Self> {
        let dir = dir.into();
        for sub in [BOOKS_DIR, COVERS_DIR, STATE_DIR] {
            fs::create_dir_all(dir.join(sub)).map_err(LibraryError::Write)?;
        }
        let _lock = lock_dir(&dir)?;
        sweep_stale_temps(&dir);
        let (mut books, mut report) = match load_index(&dir)? {
            Loaded::Missing => (Vec::new(), LoadReport::default()),
            Loaded::Corrupt(backup) => (
                Vec::new(),
                LoadReport {
                    backup: Some(backup),
                    ..LoadReport::default()
                },
            ),
            Loaded::Books {
                books,
                skipped,
                backup,
            } => (
                books,
                LoadReport {
                    backup,
                    skipped_entries: skipped,
                    recovered: 0,
                },
            ),
        };
        report.recovered = recover_orphans(&dir, &mut books);
        if report.skipped_entries > 0 || report.recovered > 0 {
            // O índice limpo (ou recuperado) vai já para o disco: a próxima
            // abertura não descarta nem copia as mesmas coisas outra vez.
            write_index(&dir, &books)?;
        }
        let stamp = index_stamp(&dir);
        let books = with_states(&dir, books);
        Ok(Self {
            dir,
            books,
            report,
            stamp,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn load_report(&self) -> &LoadReport {
        &self.report
    }

    /// Livros, o adicionado mais recentemente primeiro.
    pub fn list(&self) -> &[BookEntry] {
        &self.books
    }

    /// Relê o índice se outro processo o gravou desde a última leitura.
    /// Devolve se releu.
    pub fn refresh(&mut self) -> LibraryResult<bool> {
        if index_stamp(&self.dir) == self.stamp {
            return Ok(false);
        }
        let _lock = lock_dir(&self.dir)?;
        let books = self.reread_index()?;
        self.stamp = index_stamp(&self.dir);
        self.books = with_states(&self.dir, books);
        Ok(true)
    }

    /// `None` também para ids inválidos.
    pub fn get(&self, id: &str) -> Option<&BookEntry> {
        if !is_valid_book_id(id) {
            return None;
        }
        self.books.iter().find(|book| book.id == id)
    }

    /// `<dir>/books/<id>.epub` de um livro que está no índice.
    pub fn book_path(&self, id: &str) -> LibraryResult<PathBuf> {
        self.index_of(id)?;
        Ok(self.book_file(id))
    }

    /// Caminho da capa extraída, se houver.
    pub fn cover_path(&self, id: &str) -> LibraryResult<Option<PathBuf>> {
        let index = self.index_of(id)?;
        Ok(self.books[index]
            .cover_ext
            .as_deref()
            .map(|ext| self.cover_file(id, ext)))
    }

    /// Abre a cópia guardada do livro para leitura.
    pub fn open_book(&self, id: &str) -> LibraryResult<(EpubArchive, EpubBook)> {
        let archive = EpubArchive::open(self.book_path(id)?)?;
        let book = EpubBook::parse(&archive)?;
        Ok((archive, book))
    }

    /// [`Library::add_at`] com o relógio do sistema.
    pub fn add(&mut self, path: impl AsRef<Path>) -> LibraryResult<BookEntry> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        self.add_at(path, now)
    }

    /// Copia o EPUB para a biblioteca e devolve o registro. Se um livro com o
    /// mesmo SHA-256 já existe, devolve esse registro sem mudar nada.
    pub fn add_at(&mut self, path: impl AsRef<Path>, now_unix: u64) -> LibraryResult<BookEntry> {
        let source = path.as_ref();
        let books_dir = self.dir.join(BOOKS_DIR);
        let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let temp = books_dir.join(format!(".incoming-{}-{nonce}.tmp", std::process::id()));
        let result = self.add_from_temp(source, &temp, now_unix);
        // Em qualquer desfecho o temporário já foi renomeado ou tem de sair.
        let _ = fs::remove_file(&temp);
        result
    }

    fn add_from_temp(
        &mut self,
        source: &Path,
        temp: &Path,
        now_unix: u64,
    ) -> LibraryResult<BookEntry> {
        // A cópia (a parte demorada) corre fora do lock.
        let (sha256, file_size) = copy_and_hash(source, temp)?;
        let _lock = lock_dir(&self.dir)?;
        let mut books = self.reread_index()?;
        if let Some(existing) = books.iter().find(|book| book.sha256 == sha256) {
            // Já está na biblioteca. Se a cópia guardada sumiu (apagada à mão),
            // esta é idêntica, byte a byte: volta para o lugar.
            let id = existing.id.clone();
            let stored = self.book_file(&id);
            if !stored.exists() {
                fs::rename(temp, &stored).map_err(LibraryError::Write)?;
            }
            self.stamp = index_stamp(&self.dir);
            self.books = with_states(&self.dir, books);
            return self.get(&id).cloned().ok_or(LibraryError::NotFound(id));
        }
        // Lê a cópia, não a origem: o que fica guardado é exatamente o que se leu.
        let archive = EpubArchive::open(temp)?;
        let book = EpubBook::parse(&archive)?;
        let id = choose_id(&sha256, &books);
        let cover_ext = extract_cover(&archive, &book, &self.dir.join(COVERS_DIR), &id);
        drop(archive);

        let fallback_title = source
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned());
        let source_name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entry = entry_for(
            &book,
            BookFacts {
                id: id.clone(),
                sha256: sha256.clone(),
                file_size,
                source_name,
                fallback_title,
                cover_ext: cover_ext.clone(),
                added_unix: now_unix,
            },
        );

        let target = self.book_file(&id);
        let state_file = self.state_file(&id);
        let rollback = |this: &Self, state_written: bool| {
            let _ = fs::remove_file(&target);
            if let Some(ext) = &cover_ext {
                let _ = fs::remove_file(this.cover_file(&id, ext));
            }
            if state_written {
                let _ = fs::remove_file(&state_file);
            }
        };
        if let Err(error) = fs::rename(temp, &target) {
            rollback(self, false);
            return Err(LibraryError::Write(error));
        }
        // Um livro removido antes volta com os marcadores e a posição.
        let trashed = trashed_state(&self.dir, &sha256, entry.spine_len);
        if let Some((_, state)) = &trashed
            && let Err(error) = write_state(&self.dir, &id, state)
        {
            rollback(self, false);
            return Err(error);
        }
        books.insert(0, entry);
        if let Err(error) = write_index(&self.dir, &books) {
            rollback(self, trashed.is_some());
            return Err(error);
        }
        if let Some((record, _)) = trashed {
            let _ = fs::remove_file(record);
        }
        self.stamp = index_stamp(&self.dir);
        self.books = with_states(&self.dir, books);
        self.get(&id).cloned().ok_or(LibraryError::NotFound(id))
    }

    /// Move o livro e a capa para `.trash/`, guarda lá o registro e o estado
    /// de leitura (`<sha256>.json`, para o mesmo livro os recuperar se voltar)
    /// e o tira do índice. Devolve o registro removido.
    pub fn remove(&mut self, id: &str) -> LibraryResult<BookEntry> {
        self.index_of(id)?;
        let _lock = lock_dir(&self.dir)?;
        let mut books = self.reread_index()?;
        let Some(index) = books.iter().position(|book| book.id == id) else {
            // Outro processo já o tirou.
            self.stamp = index_stamp(&self.dir);
            self.books = with_states(&self.dir, books);
            return Err(LibraryError::NotFound(id.to_string()));
        };
        let memory = self.get(id).cloned();
        let state = match read_state(&self.dir, id)? {
            Some(state) => state,
            None => memory.as_ref().map(ReadingState::of).unwrap_or_default(),
        };
        let mut entry = books[index].clone();
        state.clone().apply_to(&mut entry);

        let trash = self.dir.join(TRASH_DIR);
        fs::create_dir_all(&trash).map_err(LibraryError::Write)?;
        let book_file = self.book_file(id);
        let moved_book =
            move_to_trash(&book_file, &trash, id, "epub").map_err(LibraryError::Write)?;
        let cover_file = entry
            .cover_ext
            .clone()
            .map(|ext| (self.cover_file(id, &ext), ext));
        let moved_cover = cover_file
            .as_ref()
            .and_then(|(file, ext)| move_to_trash(file, &trash, id, ext).ok().flatten());
        let restore_files = || {
            if let Some(moved) = &moved_book {
                let _ = fs::rename(moved, &book_file);
            }
            if let (Some(moved), Some((file, _))) = (&moved_cover, &cover_file) {
                let _ = fs::rename(moved, file);
            }
        };

        let record_path = trash.join(format!("{}.json", entry.sha256));
        let record = TrashRecord {
            entry: books[index].clone(),
            state,
        };
        let written = serde_json::to_vec(&record)
            .map_err(LibraryError::Json)
            .and_then(|json| {
                write_atomically(&trash, &record_path, &json).map_err(LibraryError::Write)
            });
        if let Err(error) = written {
            restore_files();
            return Err(error);
        }
        books.remove(index);
        if let Err(error) = write_index(&self.dir, &books) {
            restore_files();
            let _ = fs::remove_file(&record_path);
            return Err(error);
        }
        let _ = fs::remove_file(self.state_file(id));
        self.stamp = index_stamp(&self.dir);
        self.books = with_states(&self.dir, books);
        Ok(entry)
    }

    /// Guarda onde a leitura parou. `fraction` fora de 0..=1 é cortada;
    /// NaN/infinito e `spine_index` além do fim do spine são recusados.
    pub fn set_position(&mut self, id: &str, position: Position) -> LibraryResult<()> {
        self.update_state(id, |state, book| {
            let fraction = checked_place(book, position.spine_index, position.fraction)?;
            state.position = Some(Position {
                fraction,
                ..position
            });
            Ok(())
        })
    }

    pub fn set_last_opened(&mut self, id: &str, unix: u64) -> LibraryResult<()> {
        self.update_state(id, |state, _| {
            state.last_opened_unix = Some(unix);
            Ok(())
        })
    }

    /// Esquece quando cada livro foi aberto (o "Continuar lendo" e a ordem
    /// dos recentes). Posições e marcadores ficam: são do leitor, como no
    /// Calibre. Devolve quantos livros mudaram.
    pub fn clear_last_opened(&mut self) -> LibraryResult<usize> {
        let _lock = lock_dir(&self.dir)?;
        let mut changed = 0;
        for index in 0..self.books.len() {
            let id = self.books[index].id.clone();
            let mut state = match read_state(&self.dir, &id)? {
                Some(state) => state,
                None => ReadingState::of(&self.books[index]),
            };
            if state.last_opened_unix.take().is_none() {
                continue;
            }
            write_state(&self.dir, &id, &state)?;
            state.apply_to(&mut self.books[index]);
            changed += 1;
        }
        Ok(changed)
    }

    /// Acrescenta um marcador (rótulo sem caracteres de controle, até
    /// [`MAX_LABEL_CHARS`]) e o devolve com o id atribuído.
    pub fn add_bookmark(
        &mut self,
        id: &str,
        spine_index: usize,
        fraction: f64,
        label: &str,
        created_unix: u64,
    ) -> LibraryResult<Bookmark> {
        let mut added = None;
        self.update_state(id, |state, book| {
            let fraction = checked_place(book, spine_index, fraction)?;
            if state.bookmarks.len() >= MAX_BOOKMARKS {
                return Err(LibraryError::TooManyBookmarks(MAX_BOOKMARKS));
            }
            let bookmark = Bookmark {
                id: state
                    .bookmarks
                    .iter()
                    .map(|mark| mark.id)
                    .max()
                    .unwrap_or(0)
                    + 1,
                spine_index,
                fraction,
                label: clean_label(label),
                created_unix,
            };
            added = Some(bookmark.clone());
            state.bookmarks.push(bookmark);
            sort_bookmarks(&mut state.bookmarks);
            Ok(())
        })?;
        added.ok_or_else(|| LibraryError::NotFound(id.to_string()))
    }

    pub fn remove_bookmark(&mut self, id: &str, bookmark: u64) -> LibraryResult<Bookmark> {
        let mut removed = None;
        self.update_state(id, |state, _| {
            let Some(at) = state.bookmarks.iter().position(|mark| mark.id == bookmark) else {
                return Err(LibraryError::BookmarkNotFound {
                    book: id.to_string(),
                    bookmark,
                });
            };
            removed = Some(state.bookmarks.remove(at));
            Ok(())
        })?;
        removed.ok_or_else(|| LibraryError::NotFound(id.to_string()))
    }

    /// Marcadores na ordem de leitura.
    pub fn bookmarks(&self, id: &str) -> LibraryResult<&[Bookmark]> {
        let index = self.index_of(id)?;
        Ok(&self.books[index].bookmarks)
    }

    fn index_of(&self, id: &str) -> LibraryResult<usize> {
        if !is_valid_book_id(id) {
            return Err(LibraryError::InvalidId(id.to_string()));
        }
        self.books
            .iter()
            .position(|book| book.id == id)
            .ok_or_else(|| LibraryError::NotFound(id.to_string()))
    }

    fn book_file(&self, id: &str) -> PathBuf {
        self.dir.join(BOOKS_DIR).join(format!("{id}.epub"))
    }

    fn cover_file(&self, id: &str, ext: &str) -> PathBuf {
        self.dir.join(COVERS_DIR).join(format!("{id}.{ext}"))
    }

    fn state_file(&self, id: &str) -> PathBuf {
        state_path(&self.dir, id)
    }

    /// Sob o lock: o índice como está no disco agora. Sem índice, ou com um
    /// que deixou de se ler, vale o que esta instância conhece (o ilegível é
    /// guardado num `.bak` antes de ser substituído).
    fn reread_index(&self) -> LibraryResult<Vec<BookEntry>> {
        Ok(match load_index(&self.dir)? {
            Loaded::Books { books, .. } => books,
            Loaded::Missing | Loaded::Corrupt(_) => self
                .books
                .iter()
                .cloned()
                .map(|mut book| {
                    ReadingState::default().apply_to(&mut book);
                    book
                })
                .collect(),
        })
    }

    /// Muda o estado de leitura de um livro: sob o lock, relê
    /// `state/<id>.json` (outro processo pode tê-lo mudado), aplica e grava.
    /// Se algo falha, nada muda (nem no disco nem em memória).
    fn update_state(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ReadingState, &BookEntry) -> LibraryResult<()>,
    ) -> LibraryResult<()> {
        let index = self.index_of(id)?;
        let _lock = lock_dir(&self.dir)?;
        let mut state = match read_state(&self.dir, id)? {
            Some(state) => state,
            None => ReadingState::of(&self.books[index]),
        };
        change(&mut state, &self.books[index])?;
        write_state(&self.dir, id, &state)?;
        state.apply_to(&mut self.books[index]);
        Ok(())
    }
}

/// Hex minúsculo, 16 a 64 caracteres: nunca vira `..`, separador ou unidade.
pub fn is_valid_book_id(id: &str) -> bool {
    (ID_MIN_LEN..=SHA256_HEX_LEN).contains(&id.len()) && is_lower_hex(id)
}

fn is_lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn finite_fraction(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn checked_place(book: &BookEntry, spine_index: usize, fraction: f64) -> LibraryResult<f64> {
    if !fraction.is_finite() {
        return Err(LibraryError::InvalidPosition(format!(
            "fração não finita: {fraction}"
        )));
    }
    if book.spine_len > 0 && spine_index >= book.spine_len {
        return Err(LibraryError::InvalidPosition(format!(
            "documento {spine_index} além do fim do spine ({})",
            book.spine_len
        )));
    }
    Ok(fraction.clamp(0.0, 1.0))
}

fn clean_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_LABEL_CHARS)
        .collect()
}

fn sort_bookmarks(bookmarks: &mut [Bookmark]) {
    bookmarks.sort_by(|a, b| {
        a.spine_index
            .cmp(&b.spine_index)
            .then_with(|| a.fraction.total_cmp(&b.fraction))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Como o Calibre ordena um autor sem `file-as`: o último nome primeiro
/// ("Machado de Assis" -> "Assis, Machado de").
pub fn author_sort_of(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    match words.as_slice() {
        [] => String::new(),
        [single] => (*single).to_string(),
        [rest @ .., last] => format!("{last}, {}", rest.join(" ")),
    }
}

fn author_sort_for(metadata: &EpubMetadata, authors: &[String]) -> String {
    let Some(first) = authors.first() else {
        return String::new();
    };
    metadata
        .creators
        .iter()
        .find(|creator| &creator.name == first)
        .and_then(|creator| creator.file_as.as_deref())
        .map(str::trim)
        .filter(|file_as| !file_as.is_empty())
        .map_or_else(|| author_sort_of(first), str::to_string)
}

/// O que se sabe de um arquivo além do que o EPUB diz.
struct BookFacts {
    id: String,
    sha256: String,
    file_size: u64,
    source_name: String,
    fallback_title: Option<String>,
    cover_ext: Option<String>,
    added_unix: u64,
}

fn entry_for(book: &EpubBook, facts: BookFacts) -> BookEntry {
    let metadata = &book.metadata;
    let authors = metadata.authors();
    BookEntry {
        id: facts.id,
        sha256: facts.sha256,
        title: metadata
            .title
            .clone()
            .or(facts.fallback_title)
            .unwrap_or_else(|| "Sem título".to_string()),
        author_sort: author_sort_for(metadata, &authors),
        authors,
        language: metadata.language.clone(),
        identifier: metadata.identifier.clone(),
        publisher: metadata.publisher.clone(),
        description: metadata.description.clone(),
        date: metadata.date.clone(),
        subjects: metadata.subjects.clone(),
        series: metadata.series.clone(),
        series_index: metadata.series_index,
        source_name: facts.source_name,
        file_size: facts.file_size,
        spine_len: book.spine.len(),
        cover_ext: facts.cover_ext,
        added_unix: facts.added_unix,
        last_opened_unix: None,
        position: None,
        bookmarks: Vec::new(),
    }
}

/// Primeiro prefixo do SHA-256 (16, 20, 24... caracteres) que nenhum outro
/// livro usa como id.
fn choose_id(sha256: &str, books: &[BookEntry]) -> String {
    let mut len = ID_MIN_LEN;
    while len < sha256.len() && books.iter().any(|book| book.id == sha256[..len]) {
        len += 4;
    }
    sha256[..len.min(sha256.len())].to_string()
}

/// Copia `source` para `temp` (criado de novo) calculando o SHA-256, sem
/// passar de [`MAX_BOOK_BYTES`]. Ler a origem que falha é `Io`; gravar a
/// cópia que falha (disco cheio...) é `Write`.
fn copy_and_hash(source: &Path, temp: &Path) -> LibraryResult<(String, u64)> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)
        .map_err(LibraryError::Write)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        total += read as u64;
        if total > MAX_BOOK_BYTES {
            return Err(LibraryError::TooLarge {
                size: total,
                limit: MAX_BOOK_BYTES,
            });
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(LibraryError::Write)?;
    }
    output.sync_all().map_err(LibraryError::Write)?;
    Ok((hex(&hasher.finalize()), total))
}

/// SHA-256 e tamanho de um arquivo que já está na biblioteca.
fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((hex(&hasher.finalize()), total))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0F)]));
    }
    out
}

/// Extensão pelo conteúdo, não pelo que o livro declara.
fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Grava a capa em `covers/<id>.<ext>`. Qualquer falha (capa grande demais,
/// formato desconhecido, SVG, E/S) deixa o livro sem capa, não sem livro.
fn extract_cover(
    archive: &EpubArchive,
    book: &EpubBook,
    covers: &Path,
    id: &str,
) -> Option<String> {
    let cover = book.cover.as_ref()?;
    let bytes = archive.read_capped(&cover.path, MAX_COVER_BYTES).ok()?;
    let ext = sniff_image(&bytes)?;
    write_atomically(covers, &covers.join(format!("{id}.{ext}")), &bytes).ok()?;
    Some(ext.to_string())
}

/// Temporário na mesma pasta (mesmo volume) e `rename` por cima.
fn write_atomically(dir: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".write-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Move `file` para `trash/<stem>.<ext>` sem esmagar o que já está lá
/// (`<stem>~2.<ext>`, `~3`...). `Ok(None)` se não havia arquivo.
fn move_to_trash(file: &Path, trash: &Path, stem: &str, ext: &str) -> io::Result<Option<PathBuf>> {
    if !file.exists() {
        return Ok(None);
    }
    let mut target = trash.join(format!("{stem}.{ext}"));
    let mut copy = 2;
    while target.exists() {
        target = trash.join(format!("{stem}~{copy}.{ext}"));
        copy += 1;
    }
    match fs::rename(file, &target) {
        Ok(()) => Ok(Some(target)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Guarda um índice que não se pode usar como está num `.bak`: o primeiro
/// nome livre, ou um `.bak` que já tem exatamente estes bytes (abrir três
/// vezes o mesmo índice estragado não faz três cópias).
fn backup_index(dir: &Path, bytes: &[u8]) -> LibraryResult<PathBuf> {
    let mut copy = 1u32;
    loop {
        let candidate = if copy == 1 {
            dir.join(format!("{INDEX_FILE}.bak"))
        } else {
            dir.join(format!("{INDEX_FILE}.{copy}.bak"))
        };
        match fs::read(&candidate) {
            Ok(existing) if existing == bytes => return Ok(candidate),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                write_atomically(dir, &candidate, bytes).map_err(LibraryError::Write)?;
                return Ok(candidate);
            }
            // Um `.bak` que não se lê não é este: o nome seguinte.
            Err(_) => {}
        }
        copy += 1;
    }
}

enum Loaded {
    Missing,
    /// Ilegível: o conteúdo foi para este `.bak` e o índice saiu do lugar.
    Corrupt(PathBuf),
    Books {
        books: Vec<BookEntry>,
        skipped: usize,
        backup: Option<PathBuf>,
    },
}

/// Lê o índice (sob o lock). Sem o estado de leitura: ver [`with_states`].
fn load_index(dir: &Path) -> LibraryResult<Loaded> {
    let path = dir.join(INDEX_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(error) => return Err(error.into()),
    };
    let Ok(index) = serde_json::from_slice::<IndexIn>(&bytes) else {
        let backup = backup_index(dir, &bytes)?;
        fs::remove_file(&path).map_err(LibraryError::Write)?;
        return Ok(Loaded::Corrupt(backup));
    };
    let mut books: Vec<BookEntry> = Vec::new();
    let mut skipped = 0;
    for value in index.books {
        match serde_json::from_value::<BookEntry>(value) {
            Ok(book) if is_sound(&book) && !books.iter().any(|seen| seen.id == book.id) => {
                books.push(sanitized(book));
            }
            _ => skipped += 1,
        }
    }
    // A próxima gravação reescreve o índice sem essas entradas: guarda-se o
    // original antes.
    let backup = if skipped > 0 {
        Some(backup_index(dir, &bytes)?)
    } else {
        None
    };
    Ok(Loaded::Books {
        books,
        skipped,
        backup,
    })
}

fn write_index(dir: &Path, books: &[BookEntry]) -> LibraryResult<()> {
    let json = serde_json::to_vec_pretty(&IndexOut {
        version: INDEX_VERSION,
        books,
    })?;
    write_atomically(dir, &dir.join(INDEX_FILE), &json).map_err(LibraryError::Write)
}

fn state_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(STATE_DIR).join(format!("{id}.json"))
}

/// `state/<id>.json`, se existe. Um arquivo que não é nosso (grande demais,
/// JSON estragado) é posto de lado em `<id>.json.bak` e conta como ausente.
fn read_state(dir: &Path, id: &str) -> LibraryResult<Option<ReadingState>> {
    let path = state_path(dir, id);
    let bytes = match fs::metadata(&path) {
        Ok(meta) if meta.is_file() && meta.len() <= MAX_STATE_BYTES => fs::read(&path)?,
        Ok(meta) if meta.is_file() => Vec::new(),
        Ok(_) => {
            return Err(LibraryError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} não é um arquivo", path.display()),
            )));
        }
        // Sem o arquivo (ou sem a pasta `state/`): ainda não há estado; gravar
        // é que dirá se a pasta serve.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    match serde_json::from_slice::<ReadingState>(&bytes) {
        Ok(state) => Ok(Some(state.sanitized())),
        Err(_) => {
            let _ = fs::rename(&path, path.with_extension("json.bak"));
            Ok(None)
        }
    }
}

fn write_state(dir: &Path, id: &str, state: &ReadingState) -> LibraryResult<()> {
    let json = serde_json::to_vec(state)?;
    let folder = dir.join(STATE_DIR);
    fs::create_dir_all(&folder).map_err(LibraryError::Write)?;
    write_atomically(&folder, &state_path(dir, id), &json).map_err(LibraryError::Write)
}

/// Os registros do índice com o estado de leitura de cada um. Um estado que
/// não se lê fica vazio (o livro abre do início) em vez de esconder o livro.
fn with_states(dir: &Path, books: Vec<BookEntry>) -> Vec<BookEntry> {
    books
        .into_iter()
        .map(|mut book| {
            if let Ok(Some(state)) = read_state(dir, &book.id) {
                state.apply_to(&mut book);
            }
            book
        })
        .collect()
}

/// O estado guardado na lixeira quando este mesmo arquivo (SHA-256) foi
/// removido, com o caminho do registro. Posições além do spine saem.
fn trashed_state(dir: &Path, sha256: &str, spine_len: usize) -> Option<(PathBuf, ReadingState)> {
    let path = dir.join(TRASH_DIR).join(format!("{sha256}.json"));
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_STATE_BYTES {
        return None;
    }
    let record: TrashRecord = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    let mut state = record.state.sanitized();
    let fits = |spine: usize| spine_len == 0 || spine < spine_len;
    if state
        .position
        .is_some_and(|position| !fits(position.spine_index))
    {
        state.position = None;
    }
    state.bookmarks.retain(|mark| fits(mark.spine_index));
    Some((path, state))
}

/// Livros em `books/<id>.epub` que o índice não conhece (índice estragado,
/// ou um corte entre copiar o livro e gravar o índice) voltam a entrar: o
/// SHA-256 tem de começar pelo id e o EPUB tem de abrir. Devolve quantos.
fn recover_orphans(dir: &Path, books: &mut Vec<BookEntry>) -> usize {
    let Ok(entries) = fs::read_dir(dir.join(BOOKS_DIR)) else {
        return 0;
    };
    let mut found: Vec<BookEntry> = Vec::new();
    for item in entries.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".epub") else {
            continue;
        };
        let known = |list: &[BookEntry]| list.iter().any(|book| book.id == id);
        if !is_valid_book_id(id) || known(books) || known(&found) {
            continue;
        }
        if let Some(entry) = rebuild_entry(dir, id, &item.path()) {
            found.push(entry);
        }
    }
    let count = found.len();
    if count > 0 {
        books.extend(found);
        // Mais recente primeiro (estável: a ordem do índice fica).
        books.sort_by_key(|book| std::cmp::Reverse(book.added_unix));
    }
    count
}

fn rebuild_entry(dir: &Path, id: &str, path: &Path) -> Option<BookEntry> {
    let (sha256, file_size) = hash_file(path).ok()?;
    if !sha256.starts_with(id) {
        return None;
    }
    let archive = EpubArchive::open(path).ok()?;
    let book = EpubBook::parse(&archive).ok()?;
    let covers = dir.join(COVERS_DIR);
    let cover_ext = COVER_EXTENSIONS
        .iter()
        .find(|ext| covers.join(format!("{id}.{ext}")).is_file())
        .map(|ext| (*ext).to_string())
        .or_else(|| extract_cover(&archive, &book, &covers, id));
    // Quando entrou: quando a cópia foi feita.
    let added_unix = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs());
    Some(entry_for(
        &book,
        BookFacts {
            id: id.to_string(),
            sha256,
            file_size,
            source_name: String::new(),
            fallback_title: None,
            cover_ext,
            added_unix,
        },
    ))
}

/// Apaga `.incoming-<pid>-*.tmp` e `.write-<pid>-*.tmp` de OUTRO processo
/// com mais de [`STALE_TEMP`] (um processo que morreu a meio de uma cópia).
/// Os deste processo e os recentes (outra janela a copiar agora) ficam.
fn sweep_stale_temps(dir: &Path) {
    let own = std::process::id().to_string();
    let now = SystemTime::now();
    for folder in [
        dir.to_path_buf(),
        dir.join(BOOKS_DIR),
        dir.join(COVERS_DIR),
        dir.join(STATE_DIR),
        dir.join(TRASH_DIR),
    ] {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for item in entries.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let Some(rest) = name
                .strip_prefix(".incoming-")
                .or_else(|| name.strip_prefix(".write-"))
            else {
                continue;
            };
            let Some(pid) = rest
                .strip_suffix(".tmp")
                .and_then(|rest| rest.split('-').next())
            else {
                continue;
            };
            if pid == own {
                continue;
            }
            let stale = item
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STALE_TEMP);
            if stale {
                let _ = fs::remove_file(item.path());
            }
        }
    }
}

fn is_sound(book: &BookEntry) -> bool {
    is_valid_book_id(&book.id)
        && book.sha256.len() == SHA256_HEX_LEN
        && is_lower_hex(&book.sha256)
        && book.sha256.starts_with(&book.id)
        && book
            .cover_ext
            .as_deref()
            .is_none_or(|ext| COVER_EXTENSIONS.contains(&ext))
}

fn sanitized(mut book: BookEntry) -> BookEntry {
    let state = ReadingState::of(&book).sanitized();
    state.apply_to(&mut book);
    if book.author_sort.trim().is_empty() {
        book.author_sort = book
            .authors
            .first()
            .map(|name| author_sort_of(name))
            .unwrap_or_default();
    }
    book
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epub::test_support::{
        CONTAINER_XML, PNG_1X1, ZipBuilder, chapter, epub_with, epub2_sample, epub3_sample, opf,
    };
    use crate::test_alloc::peak_during;

    /// 2026-09-23T04:12:05Z.
    const T0: u64 = 1_790_136_725;

    static DIR_NONCE: AtomicU64 = AtomicU64::new(1);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neuralia-library-{name}-{}-{}",
                std::process::id(),
                DIR_NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(path.join("origem")).unwrap();
            Self(path)
        }

        fn lib_dir(&self) -> PathBuf {
            self.0.join("biblioteca")
        }

        fn library(&self) -> Library {
            Library::open(self.lib_dir()).unwrap()
        }

        /// Grava um EPUB fora da biblioteca, como o usuário o teria no disco.
        fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join("origem").join(name);
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn at(spine_index: usize, fraction: f64, updated_unix: u64) -> Position {
        Position {
            spine_index,
            fraction,
            updated_unix,
        }
    }

    #[test]
    fn add_copies_the_book_parses_metadata_and_extracts_the_cover() {
        let temp = TempDir::new("add");
        let bytes = epub3_sample();
        let source = temp.source("Livro Três.epub", &bytes);
        let mut library = temp.library();
        let entry = library.add_at(&source, T0).unwrap();

        let sha = sha256_hex(&bytes);
        assert_eq!(entry.sha256, sha);
        assert_eq!(entry.id, sha[..16]);
        assert_eq!(entry.title, "O Livro de Teste & Companhia");
        assert_eq!(entry.authors, ["Maria Autora"]);
        assert_eq!(entry.language.as_deref(), Some("pt-BR"));
        assert_eq!(entry.series.as_deref(), Some("Série Exemplo"));
        assert_eq!(entry.series_index, Some(2.0));
        assert_eq!(entry.subjects, ["Ficção", "Testes"]);
        assert_eq!(entry.spine_len, 3);
        assert_eq!(entry.source_name, "Livro Três.epub");
        assert_eq!(entry.file_size, bytes.len() as u64);
        assert_eq!(entry.added_unix, T0);
        assert_eq!(entry.cover_ext.as_deref(), Some("png"));

        assert_eq!(
            fs::read(library.book_path(&entry.id).unwrap()).unwrap(),
            bytes
        );
        let cover = library.cover_path(&entry.id).unwrap().unwrap();
        assert_eq!(
            cover,
            temp.lib_dir()
                .join(COVERS_DIR)
                .join(format!("{}.png", entry.id))
        );
        assert_eq!(fs::read(cover).unwrap(), PNG_1X1);
        assert_eq!(
            file_names(&temp.lib_dir().join(BOOKS_DIR)),
            [format!("{}.epub", entry.id)]
        );
        assert_eq!(
            file_names(&temp.lib_dir()),
            [BOOKS_DIR, COVERS_DIR, INDEX_FILE, LOCK_FILE, STATE_DIR]
        );

        let reopened = temp.library();
        assert_eq!(reopened.list(), std::slice::from_ref(&entry));
        assert_eq!(reopened.load_report(), &LoadReport::default());
        let (_archive, book) = reopened.open_book(&entry.id).unwrap();
        assert_eq!(book.toc.len(), 3);
    }

    #[test]
    fn the_same_book_twice_is_one_entry() {
        let temp = TempDir::new("dedupe");
        let bytes = epub3_sample();
        let mut library = temp.library();
        let first = library.add_at(temp.source("a.epub", &bytes), T0).unwrap();
        let again = library
            .add_at(temp.source("copia com outro nome.epub", &bytes), T0 + 60)
            .unwrap();
        assert_eq!(again, first, "o registro existente volta sem mudar");
        assert_eq!(library.list().len(), 1);
        assert_eq!(
            file_names(&temp.lib_dir().join(BOOKS_DIR)),
            [format!("{}.epub", first.id)]
        );

        // A cópia guardada foi apagada à mão: adicionar de novo repõe o arquivo.
        let stored = library.book_path(&first.id).unwrap();
        fs::remove_file(&stored).unwrap();
        assert_eq!(
            library
                .add_at(temp.source("a.epub", &bytes), T0 + 90)
                .unwrap(),
            first
        );
        assert_eq!(fs::read(&stored).unwrap(), bytes);

        let other = library
            .add_at(temp.source("b.epub", &epub2_sample()), T0 + 120)
            .unwrap();
        let ids: Vec<&str> = library.list().iter().map(|book| book.id.as_str()).collect();
        assert_eq!(
            ids,
            [other.id.as_str(), first.id.as_str()],
            "mais recente primeiro"
        );
        assert_eq!(other.cover_ext.as_deref(), Some("jpg"));
    }

    #[test]
    fn rejected_books_leave_nothing_behind() {
        let temp = TempDir::new("rejected");
        let mut library = temp.library();
        let page = chapter("P", "");
        let opf = opf(
            "",
            r#"<item id="p1" href="p1.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="p1"/>"#,
            "",
        );
        let drm = epub_with(&[
            ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
            ("META-INF/rights.xml", b"<rights/>"),
            ("OEBPS/content.opf", opf.as_bytes()),
            ("OEBPS/p1.xhtml", page.as_bytes()),
        ]);
        let bomb = ZipBuilder::new()
            .deflated("zeros.bin", &vec![0u8; 4 * 1024 * 1024])
            .build();
        let cases: [(&str, Vec<u8>); 3] = [
            ("drm.epub", drm),
            ("lixo.epub", b"isto nao e um zip".repeat(10)),
            ("bomba.epub", bomb),
        ];
        for (name, bytes) in cases {
            let error = library.add_at(temp.source(name, &bytes), T0).unwrap_err();
            match (name, &error) {
                ("drm.epub", LibraryError::Epub(EpubError::Drm(_)))
                | ("lixo.epub", LibraryError::Epub(EpubError::NotZip(_)))
                | ("bomba.epub", LibraryError::Epub(EpubError::Limit { .. })) => {}
                _ => panic!("{name}: erro inesperado {error:?}"),
            }
        }
        assert!(matches!(
            library.add_at(temp.0.join("origem").join("nao-existe.epub"), T0),
            Err(LibraryError::Io(_))
        ));
        assert!(library.list().is_empty());
        assert!(file_names(&temp.lib_dir().join(BOOKS_DIR)).is_empty());
        assert!(file_names(&temp.lib_dir().join(COVERS_DIR)).is_empty());
        assert_eq!(
            file_names(&temp.lib_dir()),
            [BOOKS_DIR, COVERS_DIR, LOCK_FILE, STATE_DIR]
        );
    }

    #[test]
    fn remove_moves_book_and_cover_to_the_trash_without_clobbering() {
        let temp = TempDir::new("remove");
        let bytes = epub3_sample();
        let source = temp.source("a.epub", &bytes);
        let mut library = temp.library();
        let entry = library.add_at(&source, T0).unwrap();
        let id = entry.id.clone();

        assert_eq!(library.remove(&id).unwrap(), entry);
        assert!(library.list().is_empty());
        assert!(file_names(&temp.lib_dir().join(BOOKS_DIR)).is_empty());
        assert!(file_names(&temp.lib_dir().join(COVERS_DIR)).is_empty());
        let trash = temp.lib_dir().join(TRASH_DIR);
        let record = format!("{}.json", entry.sha256);
        assert_eq!(
            file_names(&trash),
            [format!("{id}.epub"), format!("{id}.png"), record.clone()]
        );
        assert_eq!(fs::read(trash.join(format!("{id}.epub"))).unwrap(), bytes);
        assert!(matches!(
            library.remove(&id),
            Err(LibraryError::NotFound(_))
        ));

        library.add_at(&source, T0 + 1).unwrap();
        library.remove(&id).unwrap();
        let mut expected = vec![
            format!("{id}.epub"),
            format!("{id}.png"),
            format!("{id}~2.epub"),
            format!("{id}~2.png"),
            record,
        ];
        expected.sort();
        assert_eq!(file_names(&trash), expected);
        assert!(temp.library().list().is_empty(), "a remoção foi gravada");
    }

    #[test]
    fn positions_and_last_opened_survive_a_reopen() {
        let temp = TempDir::new("position");
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap()
            .id;
        library.set_position(&id, at(1, 0.25, T0 + 5)).unwrap();
        library.set_last_opened(&id, T0 + 6).unwrap();
        let reopened = temp.library();
        let book = reopened.get(&id).unwrap();
        assert_eq!(book.position, Some(at(1, 0.25, T0 + 5)));
        assert_eq!(book.last_opened_unix, Some(T0 + 6));

        library.set_position(&id, at(2, 1.7, T0 + 7)).unwrap();
        assert_eq!(library.get(&id).unwrap().position, Some(at(2, 1.0, T0 + 7)));
        library.set_position(&id, at(0, -3.0, T0 + 8)).unwrap();
        assert_eq!(library.get(&id).unwrap().position, Some(at(0, 0.0, T0 + 8)));

        for bad in [
            at(0, f64::NAN, T0 + 9),
            at(0, f64::INFINITY, T0 + 9),
            at(3, 0.5, T0 + 9),
        ] {
            assert!(
                matches!(
                    library.set_position(&id, bad),
                    Err(LibraryError::InvalidPosition(_))
                ),
                "{bad:?}"
            );
        }
        assert_eq!(library.get(&id).unwrap().position, Some(at(0, 0.0, T0 + 8)));
        assert_eq!(
            temp.library().get(&id).unwrap().position,
            Some(at(0, 0.0, T0 + 8))
        );
    }

    #[test]
    fn bookmarks_are_ordered_cleaned_and_removable() {
        let temp = TempDir::new("bookmarks");
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap()
            .id;
        let b = library.add_bookmark(&id, 2, 0.5, "b", T0 + 1).unwrap();
        let a = library
            .add_bookmark(&id, 0, 0.9, "  a\u{7}\n rótulo \t ", T0 + 2)
            .unwrap();
        let c = library.add_bookmark(&id, 2, 0.1, "c", T0 + 3).unwrap();
        assert_eq!((b.id, a.id, c.id), (1, 2, 3));
        assert_eq!(a.label, "a rótulo");
        let order: Vec<u64> = library
            .bookmarks(&id)
            .unwrap()
            .iter()
            .map(|mark| mark.id)
            .collect();
        assert_eq!(order, [2, 3, 1], "ordem de leitura");

        let long = library
            .add_bookmark(&id, 1, 0.0, &"x".repeat(500), T0 + 4)
            .unwrap();
        assert_eq!(long.label.chars().count(), MAX_LABEL_CHARS);
        assert!(matches!(
            library.add_bookmark(&id, 3, 0.0, "fora", T0),
            Err(LibraryError::InvalidPosition(_))
        ));

        assert_eq!(library.remove_bookmark(&id, 3).unwrap(), c);
        assert!(matches!(
            library.remove_bookmark(&id, 3),
            Err(LibraryError::BookmarkNotFound { bookmark: 3, .. })
        ));
        let reopened = temp.library();
        let marks: Vec<(u64, &str)> = reopened
            .bookmarks(&id)
            .unwrap()
            .iter()
            .map(|mark| (mark.id, mark.label.as_str()))
            .collect();
        assert_eq!(marks, [(2, "a rótulo"), (4, long.label.as_str()), (1, "b")]);
    }

    #[test]
    fn ids_are_validated_before_they_become_paths() {
        let temp = TempDir::new("ids");
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap()
            .id;
        let traversal = format!("{id}/../../x");
        for bad in [
            "",
            "../../etc/passwd",
            "..\\..\\x",
            "C:\\Windows",
            "/abs",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdefg",
            traversal.as_str(),
        ] {
            assert!(library.get(bad).is_none(), "{bad:?}");
            for result in [
                library.book_path(bad).map(|_| ()),
                library.cover_path(bad).map(|_| ()),
                library.bookmarks(bad).map(|_| ()),
                library.set_position(bad, at(0, 0.0, T0)),
                library.set_last_opened(bad, T0),
                library.remove(bad).map(|_| ()),
            ] {
                assert!(
                    matches!(result, Err(LibraryError::InvalidId(_))),
                    "{bad:?}: {result:?}"
                );
            }
        }
        assert!(matches!(
            library.book_path("0123456789abcdef"),
            Err(LibraryError::NotFound(_))
        ));
        assert_eq!(library.list().len(), 1);
    }

    #[test]
    fn a_corrupt_index_is_backed_up_and_the_library_opens_empty() {
        let temp = TempDir::new("corrupt");
        let index = temp.lib_dir().join(INDEX_FILE);
        fs::create_dir_all(temp.lib_dir()).unwrap();
        fs::write(&index, b"{ isto nao e json").unwrap();
        let library = temp.library();
        assert!(library.list().is_empty());
        let first_backup = temp.lib_dir().join("library.json.bak");
        assert_eq!(
            library.load_report().backup.as_deref(),
            Some(first_backup.as_path())
        );
        assert_eq!(fs::read(&first_backup).unwrap(), b"{ isto nao e json");
        assert!(!index.exists());

        fs::write(&index, b"[1, 2").unwrap();
        let mut library = temp.library();
        let second_backup = temp.lib_dir().join("library.json.2.bak");
        assert_eq!(
            library.load_report().backup.as_deref(),
            Some(second_backup.as_path())
        );
        assert_eq!(
            fs::read(&first_backup).unwrap(),
            b"{ isto nao e json",
            "o primeiro .bak fica"
        );
        assert_eq!(fs::read(&second_backup).unwrap(), b"[1, 2");

        let entry = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap();
        assert_eq!(temp.library().list(), [entry]);
    }

    #[test]
    fn invalid_entries_are_dropped_and_the_original_index_is_kept() {
        let temp = TempDir::new("partial");
        let mut library = temp.library();
        let entry = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap();
        let index_path = temp.lib_dir().join(INDEX_FILE);
        let mut index: serde_json::Value =
            serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
        let good = index["books"][0].clone();
        let mut traversal = good.clone();
        traversal["id"] = "../../../x".into();
        let mut wrong_sha = good.clone();
        wrong_sha["id"] = "ffffffffffffffff".into();
        let mut evil_cover = good.clone();
        evil_cover["id"] = entry.sha256[..20].into();
        evil_cover["cover_ext"] = "png/../../../evil".into();
        let mut wrong_type = good.clone();
        wrong_type["id"] = entry.sha256[..24].into();
        wrong_type["title"] = 5.into();
        let books = index["books"].as_array_mut().unwrap();
        books.extend([traversal, wrong_sha, good, evil_cover, wrong_type]);
        let tampered = serde_json::to_vec(&index).unwrap();
        fs::write(&index_path, &tampered).unwrap();

        let reopened = temp.library();
        assert_eq!(reopened.list(), [entry]);
        assert_eq!(reopened.load_report().skipped_entries, 5);
        let backup = reopened.load_report().backup.clone().unwrap();
        assert_eq!(fs::read(backup).unwrap(), tampered);
    }

    #[test]
    fn a_failed_index_write_changes_nothing() {
        let temp = TempDir::new("failed-write");
        let mut library = temp.library();
        let entry = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap();
        let id = entry.id.clone();
        library.set_position(&id, at(1, 0.5, T0 + 1)).unwrap();

        // Uma pasta no lugar do índice: ler e gravar o índice falham.
        let index = temp.lib_dir().join(INDEX_FILE);
        fs::remove_file(&index).unwrap();
        fs::create_dir(&index).unwrap();
        fs::write(index.join("ocupado"), b"x").unwrap();

        assert!(
            library
                .add_at(temp.source("b.epub", &epub2_sample()), T0 + 5)
                .is_err()
        );
        let ids: Vec<&str> = library.list().iter().map(|book| book.id.as_str()).collect();
        assert_eq!(ids, [id.as_str()]);
        assert_eq!(
            file_names(&temp.lib_dir().join(BOOKS_DIR)),
            [format!("{id}.epub")]
        );
        assert_eq!(
            file_names(&temp.lib_dir().join(COVERS_DIR)),
            [format!("{id}.png")]
        );

        assert!(library.remove(&id).is_err());
        assert!(library.get(&id).is_some());
        assert!(library.book_path(&id).unwrap().exists());
        assert!(library.cover_path(&id).unwrap().unwrap().exists());

        // Uma pasta no lugar do estado do livro: posição, abertura e
        // marcadores falham e nada muda em memória.
        let state = temp.lib_dir().join(STATE_DIR).join(format!("{id}.json"));
        fs::remove_file(&state).unwrap();
        fs::create_dir(&state).unwrap();
        fs::write(state.join("ocupado"), b"x").unwrap();
        assert!(library.set_position(&id, at(2, 0.9, T0 + 2)).is_err());
        assert_eq!(library.get(&id).unwrap().position, Some(at(1, 0.5, T0 + 1)));
        assert!(library.set_last_opened(&id, T0 + 3).is_err());
        assert_eq!(library.get(&id).unwrap().last_opened_unix, None);
        assert!(library.add_bookmark(&id, 0, 0.1, "x", T0 + 4).is_err());
        assert!(library.bookmarks(&id).unwrap().is_empty());

        let leftovers: Vec<String> = file_names(&temp.lib_dir())
            .into_iter()
            .chain(file_names(&temp.lib_dir().join(BOOKS_DIR)))
            .chain(file_names(&temp.lib_dir().join(COVERS_DIR)))
            .chain(file_names(&temp.lib_dir().join(STATE_DIR)))
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_full_disk_is_a_write_failure_not_a_bad_source_file() {
        let temp = TempDir::new("write-error");
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap()
            .id;
        // `state/` deixou de ser uma pasta: gravar o estado falha.
        let state = temp.lib_dir().join(STATE_DIR);
        fs::remove_dir_all(&state).unwrap();
        fs::write(&state, b"arquivo no lugar da pasta").unwrap();
        let error = library.set_position(&id, at(1, 0.5, T0 + 1)).unwrap_err();
        assert!(matches!(error, LibraryError::Write(_)), "{error:?}");
        assert_eq!(library.get(&id).unwrap().position, None);
        // Ler a origem que não existe continua a ser leitura.
        assert!(matches!(
            library.add_at(temp.0.join("origem").join("nao-existe.epub"), T0),
            Err(LibraryError::Io(_))
        ));
    }

    /// O outro caminho da gravação: a pasta existe e o estado lê-se, mas o
    /// arquivo está aberto por outro programa sem partilhar o apagar (um
    /// antivírus, um backup) e o `rename` por cima falha.
    #[cfg(windows)]
    #[test]
    fn a_state_file_that_cannot_be_replaced_is_a_write_failure() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        let temp = TempDir::new("write-error-rename");
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap()
            .id;
        library.set_position(&id, at(1, 0.25, T0 + 1)).unwrap();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(state_path(&temp.lib_dir(), &id))
            .unwrap();
        let error = library.set_position(&id, at(2, 0.5, T0 + 2)).unwrap_err();
        assert!(matches!(error, LibraryError::Write(_)), "{error:?}");
        assert_eq!(
            library.get(&id).unwrap().position,
            Some(at(1, 0.25, T0 + 1))
        );
        drop(held);
        library.set_position(&id, at(2, 0.5, T0 + 3)).unwrap();
        assert_eq!(library.get(&id).unwrap().position, Some(at(2, 0.5, T0 + 3)));
    }

    // ------------------------------------------------ duas janelas, uma pasta

    #[test]
    fn two_processes_on_one_folder_never_erase_each_other() {
        let temp = TempDir::new("two-instances");
        let mut first = temp.library();
        let mut second = temp.library();
        let one = first
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap();
        first
            .add_bookmark(&one.id, 1, 0.25, "importante", T0 + 1)
            .unwrap();
        first.set_position(&one.id, at(2, 0.5, T0 + 2)).unwrap();
        // A segunda instância não sabe do primeiro livro e adiciona outro.
        assert!(second.get(&one.id).is_none());
        let two = second
            .add_at(temp.source("b.epub", &epub2_sample()), T0 + 3)
            .unwrap();
        // Ela releu o índice antes de gravar: o livro da primeira ficou.
        assert!(second.get(&one.id).is_some());
        let reopened = temp.library();
        let ids: Vec<&str> = reopened
            .list()
            .iter()
            .map(|book| book.id.as_str())
            .collect();
        assert_eq!(ids, [two.id.as_str(), one.id.as_str()]);
        let book = reopened.get(&one.id).unwrap();
        assert_eq!(book.position, Some(at(2, 0.5, T0 + 2)));
        assert_eq!(book.bookmarks.len(), 1);
        assert_eq!(book.bookmarks[0].label, "importante");

        // Marcadores dos dois lados no mesmo livro: nenhum se perde.
        second
            .add_bookmark(&one.id, 0, 0.1, "da segunda", T0 + 4)
            .unwrap();
        first
            .add_bookmark(&one.id, 2, 0.9, "da primeira", T0 + 5)
            .unwrap();
        let labels: Vec<String> = temp
            .library()
            .get(&one.id)
            .unwrap()
            .bookmarks
            .iter()
            .map(|mark| mark.label.clone())
            .collect();
        assert_eq!(labels, ["da segunda", "importante", "da primeira"]);

        // A primeira vê o livro da segunda quando o índice muda no disco.
        assert!(first.get(&two.id).is_none());
        assert!(first.refresh().unwrap());
        assert!(first.get(&two.id).is_some());
        assert!(!first.refresh().unwrap(), "sem mudança, não relê");
        // Remover na segunda não ressuscita na primeira.
        second.remove(&two.id).unwrap();
        first
            .add_at(temp.source("c.epub", &epub3_sample()), T0 + 6)
            .unwrap();
        assert!(temp.library().get(&two.id).is_none());
    }

    #[test]
    fn turning_a_page_writes_the_book_state_not_the_index() {
        let temp = TempDir::new("state-file");
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap()
            .id;
        let index = temp.lib_dir().join(INDEX_FILE);
        let before = (
            fs::read(&index).unwrap(),
            fs::metadata(&index).unwrap().modified().unwrap(),
        );
        library.set_position(&id, at(1, 0.5, T0 + 1)).unwrap();
        library.set_last_opened(&id, T0 + 2).unwrap();
        library.add_bookmark(&id, 1, 0.5, "m", T0 + 3).unwrap();
        let after = (
            fs::read(&index).unwrap(),
            fs::metadata(&index).unwrap().modified().unwrap(),
        );
        assert_eq!(before, after, "o índice não foi regravado");
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(temp.lib_dir().join(STATE_DIR).join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(state["position"]["spine_index"], 1);
        assert_eq!(state["last_opened_unix"], T0 + 2);
        assert_eq!(state["bookmarks"][0]["label"], "m");
        let text = String::from_utf8(after.0).unwrap();
        assert!(
            !text.contains("bookmarks") && !text.contains("position"),
            "{text}"
        );
    }

    // ------------------------------------------- índice estragado, lixeira

    #[test]
    fn a_lost_index_is_rebuilt_from_the_books_folder_with_their_state() {
        let temp = TempDir::new("recover");
        let mut library = temp.library();
        let one = library
            .add_at(temp.source("um.epub", &epub3_sample()), T0)
            .unwrap();
        let two = library
            .add_at(temp.source("dois.epub", &epub2_sample()), T0 + 10)
            .unwrap();
        library.set_position(&one.id, at(1, 0.5, T0 + 11)).unwrap();
        library
            .add_bookmark(&two.id, 0, 0.2, "marca", T0 + 12)
            .unwrap();
        // Um livro com o nome de outro (SHA-256 errado) nunca entra.
        let books = temp.lib_dir().join(BOOKS_DIR);
        fs::copy(
            books.join(format!("{}.epub", one.id)),
            books.join("ffffffffffffffff.epub"),
        )
        .unwrap();
        fs::write(temp.lib_dir().join(INDEX_FILE), b"{ estragado").unwrap();

        let reopened = temp.library();
        let report = reopened.load_report();
        assert_eq!(report.recovered, 2);
        assert!(report.backup.is_some());
        let mut ids: Vec<&str> = reopened
            .list()
            .iter()
            .map(|book| book.id.as_str())
            .collect();
        ids.sort();
        let mut expected = [one.id.as_str(), two.id.as_str()];
        expected.sort();
        assert_eq!(ids, expected);
        let first = reopened.get(&one.id).unwrap();
        assert_eq!(first.title, one.title);
        assert_eq!(first.cover_ext, one.cover_ext);
        assert_eq!(first.position, Some(at(1, 0.5, T0 + 11)));
        assert_eq!(reopened.get(&two.id).unwrap().bookmarks[0].label, "marca");
        // O índice recuperado foi gravado: abrir de novo não recupera nada.
        assert_eq!(temp.library().load_report(), &LoadReport::default());
    }

    #[test]
    fn a_crash_between_copying_and_indexing_is_recovered() {
        let temp = TempDir::new("crash");
        let bytes = epub3_sample();
        let sha = sha256_hex(&bytes);
        let books = temp.lib_dir().join(BOOKS_DIR);
        fs::create_dir_all(&books).unwrap();
        fs::write(books.join(format!("{}.epub", &sha[..16])), &bytes).unwrap();
        let library = temp.library();
        assert_eq!(library.load_report().recovered, 1);
        assert_eq!(library.list()[0].sha256, sha);
        assert_eq!(library.list()[0].title, "O Livro de Teste & Companhia");
    }

    #[test]
    fn a_removed_book_added_again_gets_its_bookmarks_and_position_back() {
        let temp = TempDir::new("trash-restore");
        let bytes = epub3_sample();
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &bytes), T0)
            .unwrap()
            .id;
        library
            .add_bookmark(&id, 1, 0.3, "importante", T0 + 1)
            .unwrap();
        library.set_position(&id, at(2, 0.75, T0 + 2)).unwrap();
        let removed = library.remove(&id).unwrap();
        assert_eq!(removed.bookmarks.len(), 1);
        assert!(
            !temp
                .lib_dir()
                .join(STATE_DIR)
                .join(format!("{id}.json"))
                .exists()
        );

        // Adicionar de novo (até a própria cópia da lixeira) traz tudo de volta.
        let trashed = temp.lib_dir().join(TRASH_DIR).join(format!("{id}.epub"));
        let back = library.add_at(&trashed, T0 + 3).unwrap();
        assert_eq!(back.position, Some(at(2, 0.75, T0 + 2)));
        assert_eq!(back.bookmarks.len(), 1);
        assert_eq!(back.bookmarks[0].label, "importante");
        let record = temp
            .lib_dir()
            .join(TRASH_DIR)
            .join(format!("{}.json", removed.sha256));
        assert!(!record.exists(), "o registro da lixeira foi consumido");
        assert_eq!(temp.library().get(&id).unwrap().bookmarks.len(), 1);
    }

    #[test]
    fn dead_temporaries_are_swept_and_one_bad_index_makes_one_backup() {
        let temp = TempDir::new("sweep");
        let mut library = temp.library();
        library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap();
        drop(library);
        let dir = temp.lib_dir();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(T0);
        let stale = [
            dir.join(BOOKS_DIR).join(".incoming-4242-1.tmp"),
            dir.join(".write-4242-2.tmp"),
            dir.join(STATE_DIR).join(".write-4242-3.tmp"),
        ];
        for path in &stale {
            fs::write(path, vec![0u8; 1024]).unwrap();
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        // Outra janela a copiar agora: fica.
        let live = dir.join(BOOKS_DIR).join(".incoming-4243-1.tmp");
        fs::write(&live, b"em curso").unwrap();
        // Do próprio processo: é dele, fica.
        let own = dir.join(format!(".write-{}-9.tmp", std::process::id()));
        fs::write(&own, b"meu").unwrap();
        File::options()
            .write(true)
            .open(&own)
            .unwrap()
            .set_modified(old)
            .unwrap();

        // Uma entrada inválida no índice: três aberturas, um só .bak.
        let index = dir.join(INDEX_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        let mut bad = value["books"][0].clone();
        bad["id"] = "../x".into();
        value["books"].as_array_mut().unwrap().push(bad);
        fs::write(&index, serde_json::to_vec(&value).unwrap()).unwrap();
        for _ in 0..3 {
            temp.library();
        }
        for path in &stale {
            assert!(!path.exists(), "{}", path.display());
        }
        assert!(live.exists() && own.exists());
        let backups: Vec<String> = file_names(&dir)
            .into_iter()
            .filter(|name| name.ends_with(".bak"))
            .collect();
        assert_eq!(backups, ["library.json.bak"]);
    }

    #[test]
    fn clearing_the_history_forgets_when_books_were_opened_but_keeps_the_place() {
        let temp = TempDir::new("clear-history");
        let mut library = temp.library();
        let id = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap()
            .id;
        library.set_last_opened(&id, T0 + 1).unwrap();
        library.set_position(&id, at(1, 0.5, T0 + 2)).unwrap();
        library.add_bookmark(&id, 1, 0.5, "fica", T0 + 3).unwrap();
        assert_eq!(library.clear_last_opened().unwrap(), 1);
        let reopened = temp.library();
        let book = reopened.get(&id).unwrap();
        assert_eq!(book.last_opened_unix, None);
        assert_eq!(book.position, Some(at(1, 0.5, T0 + 2)));
        assert_eq!(book.bookmarks.len(), 1);
    }

    #[test]
    fn authors_sort_by_file_as_or_by_the_last_name() {
        assert_eq!(author_sort_of("Machado de Assis"), "Assis, Machado de");
        assert_eq!(author_sort_of("  Homero "), "Homero");
        assert_eq!(author_sort_of(""), "");
        let temp = TempDir::new("author-sort");
        let mut library = temp.library();
        let plain = library
            .add_at(temp.source("a.epub", &epub3_sample()), T0)
            .unwrap();
        assert_eq!(plain.author_sort, "Autora, Maria");
        let with_file_as = opf(
            r##"<dc:creator id="c1">Machado de Assis</dc:creator><meta refines="#c1" property="file-as">Assis, Joaquim Maria Machado de</meta>"##,
            r#"<item id="p1" href="p1.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="p1"/>"#,
            "",
        );
        let page = chapter("P", "");
        let book = epub_with(&[
            ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
            ("OEBPS/content.opf", with_file_as.as_bytes()),
            ("OEBPS/p1.xhtml", page.as_bytes()),
        ]);
        let entry = library
            .add_at(temp.source("m.epub", &book), T0 + 1)
            .unwrap();
        assert_eq!(entry.author_sort, "Assis, Joaquim Maria Machado de");
    }

    // ------------------------------------------------ OPF hostil (memória)

    /// Um EPUB cujo OPF tem um item com `media-type` de 64 KiB e outro com um
    /// `href` de 64 KiB que não existe, e `refs` itemrefs para cada um.
    fn amplifying_spine_book(refs: usize) -> Vec<u8> {
        let huge_type = format!("application/{}", "x".repeat(64 * 1024));
        let huge_href = format!("{}.xhtml", "g".repeat(64 * 1024));
        let manifest = format!(
            r#"<item id="c" href="c.xhtml" media-type="{huge_type}"/><item id="g" href="{huge_href}" media-type="application/xhtml+xml"/>"#
        );
        let spine = r#"<itemref idref="c"/><itemref idref="g"/>"#.repeat(refs);
        let opf = opf("", &manifest, &spine, "");
        let page = chapter("C", "<p>texto</p>");
        ZipBuilder::new()
            .stored("mimetype", b"application/epub+zip")
            .stored("META-INF/container.xml", CONTAINER_XML.as_bytes())
            .stored("OEBPS/content.opf", opf.as_bytes())
            .stored("OEBPS/c.xhtml", page.as_bytes())
            .build()
    }

    /// `levels` `<meta refines>` um dentro do outro, com 1 MiB de texto no
    /// mais fundo: cada nível via o texto de todos os de dentro.
    fn nested_refines_book(levels: usize) -> Vec<u8> {
        let open: String = (0..levels)
            .map(|level| format!(r##"<meta refines="#t" property="p{level}">"##))
            .collect();
        let close = "</meta>".repeat(levels);
        book_with_metadata(&format!("{open}{}{close}", "x".repeat(1024 * 1024)))
    }

    /// Um EPUB com estes metadados a mais (ZIP sem compressão: o tamanho do
    /// arquivo é o do OPF).
    fn book_with_metadata(metadata: &str) -> Vec<u8> {
        let opf = opf(
            metadata,
            r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="c"/>"#,
            "",
        );
        let page = chapter("C", "<p>texto</p>");
        ZipBuilder::new()
            .stored("mimetype", b"application/epub+zip")
            .stored("META-INF/container.xml", CONTAINER_XML.as_bytes())
            .stored("OEBPS/content.opf", opf.as_bytes())
            .stored("OEBPS/c.xhtml", page.as_bytes())
            .build()
    }

    /// U+1F4D6: 4 bytes em UTF-8, o pior caso por caractere.
    const WIDE: char = '\u{1F4D6}';

    /// Os campos de metadados que a leitura usa, abertos e fechados; `{n}`
    /// vira o nível (refinamentos com alvos diferentes não se juntam).
    const NESTABLE: [(&str, &str); 8] = [
        ("<dc:subject>", "</dc:subject>"),
        (r#"<dc:creator id="a{n}">"#, "</dc:creator>"),
        ("<dc:contributor>", "</dc:contributor>"),
        ("<dc:title>", "</dc:title>"),
        ("<dc:identifier>", "</dc:identifier>"),
        ("<dc:date>", "</dc:date>"),
        (r##"<meta refines="#a{n}" property="file-as">"##, "</meta>"),
        (
            r#"<meta property="belongs-to-collection" id="s{n}">"#,
            "</meta>",
        ),
    ];

    /// `chains` cadeias de `depth` campos (os oito tipos em rodízio), com
    /// `MAX_FIELD_CHARS` caracteres de 4 bytes no fim de cada cadeia. Com
    /// `nested`, os campos de uma cadeia estão um dentro do outro; sem, são
    /// irmãos. Os dois livros têm os mesmos bytes, a mesma quantidade de
    /// elementos e o mesmo texto: só o aninhamento muda.
    fn metadata_chains_book(chains: usize, depth: usize, nested: bool) -> Vec<u8> {
        use crate::epub::MAX_FIELD_CHARS;
        let text: String = std::iter::repeat_n(WIDE, MAX_FIELD_CHARS).collect();
        let mut metadata = String::new();
        for chain in 0..chains {
            let (open, close) = NESTABLE[chain % NESTABLE.len()];
            let open = |level: usize| open.replace("{n}", &format!("{chain}-{level}"));
            if nested {
                for level in 0..depth {
                    metadata.push_str(&open(level));
                }
                metadata.push_str(&text);
                metadata.push_str(&close.repeat(depth));
            } else {
                for level in 0..depth {
                    metadata.push_str(&open(level));
                    if level + 1 == depth {
                        metadata.push_str(&text);
                    }
                    metadata.push_str(close);
                }
            }
        }
        book_with_metadata(&metadata)
    }

    /// Importa `bytes` numa biblioteca só dele, pelo caminho que embarca:
    /// o registro, o pico de memória e o tamanho do `library.json`.
    fn import_alone(name: &str, bytes: &[u8]) -> (BookEntry, usize, u64) {
        let temp = TempDir::new(name);
        let source = temp.source("livro.epub", bytes);
        let mut library = temp.library();
        let (added, peak) = peak_during(|| library.add_at(&source, T0));
        let entry = added.expect("o livro hostil entra, com os valores cortados");
        let index = fs::metadata(temp.lib_dir().join(INDEX_FILE)).unwrap().len();
        (entry, peak, index)
    }

    /// Pico de memória de importar `bytes` pelo caminho que embarca
    /// (`Library::add_at`, o que o worker chama).
    fn import_peak(temp: &TempDir, name: &str, bytes: &[u8]) -> usize {
        let source = temp.source(name, bytes);
        let mut library = temp.library();
        let (added, peak) = peak_during(|| library.add_at(&source, T0));
        let entry = added.expect("o livro hostil entra, com os valores cortados");
        assert!(entry.spine_len >= 1);
        peak
    }

    #[test]
    fn a_crafted_spine_cannot_multiply_an_attribute_on_import() {
        let temp = TempDir::new("amplify-spine");
        let small = amplifying_spine_book(100);
        let large = amplifying_spine_book(400);
        let small_peak = import_peak(&temp, "pequeno.epub", &small);
        let large_peak = import_peak(&temp, "grande.epub", &large);
        // 4x os itemrefs: o arquivo cresce uns 17 KB. Sem teto, cada itemref
        // copiava 64 KiB duas vezes (~38 MiB a mais); com teto o pico quase
        // não mexe. A relação, não um absoluto da máquina.
        let grown = large_peak.saturating_sub(small_peak);
        assert!(
            grown < 2 * 1024 * 1024,
            "pico {small_peak} -> {large_peak} bytes (+{grown}) para +{} bytes de arquivo",
            large.len() - small.len()
        );
        assert!(
            large_peak < 16 * large.len() + 4 * 1024 * 1024,
            "pico {large_peak} para um arquivo de {} bytes",
            large.len()
        );
        let book = temp.library();
        let entry = book
            .list()
            .iter()
            .find(|entry| entry.source_name == "grande.epub")
            .unwrap();
        assert_eq!(entry.spine_len, 400, "os itemrefs válidos continuam lá");
    }

    #[test]
    fn a_spine_past_the_cap_is_cut_and_warnings_stay_bounded() {
        use crate::epub::{MAX_SPINE_ITEMS, MAX_WARNINGS};
        let refs = format!(
            "{}{}",
            r#"<itemref idref="nada"/>"#.repeat(500),
            r#"<itemref idref="c"/>"#.repeat(MAX_SPINE_ITEMS + 50)
        );
        let opf = opf(
            "",
            r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
            &refs,
            "",
        );
        let page = chapter("C", "<p>texto</p>");
        let bytes = ZipBuilder::new()
            .stored("mimetype", b"application/epub+zip")
            .stored("META-INF/container.xml", CONTAINER_XML.as_bytes())
            .stored("OEBPS/content.opf", opf.as_bytes())
            .stored("OEBPS/c.xhtml", page.as_bytes())
            .build();
        let archive = EpubArchive::from_bytes(bytes).unwrap();
        let book = EpubBook::parse(&archive).unwrap();
        assert_eq!(book.spine.len(), MAX_SPINE_ITEMS);
        assert_eq!(book.warnings.len(), MAX_WARNINGS);
        assert!(
            book.warnings
                .iter()
                .all(|warning| warning.chars().count() < 300)
        );
    }

    #[test]
    fn nested_refines_cannot_copy_their_text_once_per_level() {
        let temp = TempDir::new("amplify-meta");
        let shallow = nested_refines_book(16);
        let deep = nested_refines_book(64);
        let shallow_peak = import_peak(&temp, "raso.epub", &shallow);
        let deep_peak = import_peak(&temp, "fundo.epub", &deep);
        // 4x a profundidade, o mesmo texto: sem corte, 48 níveis a mais
        // guardavam 48 MiB a mais.
        let grown = deep_peak.saturating_sub(shallow_peak);
        assert!(
            grown < 4 * 1024 * 1024,
            "pico {shallow_peak} -> {deep_peak} bytes (+{grown})"
        );
        assert!(
            deep_peak < 16 * deep.len() + 4 * 1024 * 1024,
            "pico {deep_peak} para um arquivo de {} bytes",
            deep.len()
        );
    }

    #[test]
    fn metadata_nested_in_width_and_depth_costs_what_the_flat_book_costs() {
        // 32 cadeias de 128 campos (todos os tipos lidos), 4 KiB de texto
        // no fim de cada uma. Antes, cada nível copiava o texto de novo:
        // 128 cópias por cadeia, ~16 MiB de pico e ~4 MiB de índice para
        // ~0,4 MB de arquivo. O livro irmão tem os mesmos bytes sem o
        // aninhamento: o custo tem de ser o mesmo.
        let (chains, depth) = (32, 128);
        let nested = metadata_chains_book(chains, depth, true);
        let flat = metadata_chains_book(chains, depth, false);
        assert_eq!(nested.len(), flat.len(), "os mesmos bytes");
        let (nested_entry, nested_peak, nested_index) = import_alone("aninhado", &nested);
        let (flat_entry, flat_peak, flat_index) = import_alone("plano", &flat);
        let grown = nested_peak.saturating_sub(flat_peak);
        assert!(
            grown < 1024 * 1024,
            "pico plano {flat_peak} -> aninhado {nested_peak} bytes (+{grown}) para {} bytes de arquivo",
            nested.len()
        );
        assert!(
            nested_index < flat_index + 16 * 1024,
            "library.json plano {flat_index} -> aninhado {nested_index} bytes"
        );
        // Um campo dentro de outro faz parte do de fora: uma cadeia de
        // assuntos é UM assunto, com o texto do fundo.
        assert_eq!(nested_entry.subjects.len(), chains / NESTABLE.len());
        assert_eq!(nested_entry.subjects, flat_entry.subjects);
        assert_eq!(nested_entry.authors.len(), chains / NESTABLE.len());
    }

    #[test]
    fn metadata_lists_are_capped_so_the_index_entry_has_a_fixed_ceiling() {
        use crate::epub::{MAX_FIELD_CHARS, MAX_METADATA_ITEMS};
        // 200 assuntos, 200 autores e 200 de cada outro campo, cada um com
        // mais do que o máximo de caracteres, de 4 bytes: sem teto, ~3 MiB
        // só de assuntos e autores no library.json, relido a cada abertura.
        let text: String = std::iter::repeat_n(WIDE, MAX_FIELD_CHARS + 100).collect();
        let mut metadata = String::new();
        for item in 0..200 {
            for (open, close) in NESTABLE {
                let open = open.replace("{n}", &item.to_string());
                metadata.push_str(&format!("{open}{text}{close}"));
            }
        }
        let bytes = book_with_metadata(&metadata);
        let (entry, peak, index) = import_alone("largo", &bytes);
        assert_eq!(entry.subjects.len(), MAX_METADATA_ITEMS);
        assert_eq!(entry.authors.len(), MAX_METADATA_ITEMS);
        // Cada campo cortado no teto.
        assert!(
            entry
                .subjects
                .iter()
                .chain(&entry.authors)
                .all(|value| value.chars().count() == MAX_FIELD_CHARS)
        );
        // O teto do registro: autores e assuntos cheios, mais título, data,
        // série e ordenação (um campo cada), a 4 bytes por caractere.
        let ceiling = (2 * MAX_METADATA_ITEMS + 8) * MAX_FIELD_CHARS * 4 + 64 * 1024;
        assert!(
            (index as usize) < ceiling,
            "library.json com {index} bytes (teto {ceiling})"
        );
        assert!(
            peak < 16 * bytes.len() + 4 * 1024 * 1024,
            "pico {peak} para um arquivo de {} bytes",
            bytes.len()
        );
    }
}
