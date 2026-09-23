//! Biblioteca de livros EPUB, no estilo do Calibre: uma pasta com o índice
//! `library.json`, as cópias dos livros e as capas extraídas.
//!
//! ```text
//! <dir>/library.json          índice (versão, livros, posições, marcadores)
//! <dir>/books/<id>.epub       cópia do livro; <id> = prefixo do SHA-256
//! <dir>/covers/<id>.<ext>     capa extraída (jpg, png, gif ou webp)
//! <dir>/.trash/               livros e capas removidos (recuperáveis)
//! ```
//!
//! - O índice é gravado inteiro a cada mudança, num temporário na mesma pasta
//!   seguido de `rename`: um corte a meio deixa a versão anterior intacta.
//!   Se a gravação falha, o estado em memória também não muda.
//! - Índice ilegível: é movido para `library.json.bak` (sem apagar um `.bak`
//!   anterior) e a biblioteca abre vazia, em vez de recusar abrir. Entradas
//!   individuais inválidas (id que não é hex, cover com extensão estranha)
//!   são descartadas e o arquivo original é copiado para o `.bak`.
//! - `add` copia o arquivo (com teto de tamanho) calculando o SHA-256 no
//!   caminho, lê o EPUB a partir da cópia (o que se guarda é o que se leu) e
//!   só então recebe o nome definitivo. O mesmo livro duas vezes é o mesmo
//!   registro. DRM, ZIP hostil ou EPUB inválido: nada fica para trás.
//! - Ids são validados antes de virarem caminho: só hex minúsculo com 16 a
//!   64 caracteres. A capa também não guarda caminho, só a extensão (de uma
//!   lista fechada), para um índice adulterado não apontar para fora da pasta.
//! - O tempo entra por argumento (`add_at`, `Position::updated_unix`...);
//!   só `add` lê o relógio, por conveniência.
//!
//! Uma instância por pasta: o índice fica em memória e cada mutação reescreve
//! o arquivo inteiro.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::epub::{EpubArchive, EpubBook, EpubError, MAX_TOTAL_SIZE};

pub const INDEX_FILE: &str = "library.json";
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

const INDEX_VERSION: u32 = 1;
const ID_MIN_LEN: usize = 16;
const SHA256_HEX_LEN: usize = 64;
const COVER_EXTENSIONS: [&str; 4] = ["jpg", "png", "gif", "webp"];

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
    #[error("falha de E/S na biblioteca: {0}")]
    Io(#[from] io::Error),
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

/// Um livro da biblioteca.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookEntry {
    /// Prefixo (16+ caracteres) do SHA-256 do arquivo; nome dos arquivos.
    pub id: String,
    pub sha256: String,
    pub title: String,
    pub authors: Vec<String>,
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
    pub last_opened_unix: Option<u64>,
    pub position: Option<Position>,
    pub bookmarks: Vec<Bookmark>,
}

/// O que a abertura encontrou de estranho no índice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Cópia do índice ilegível ou parcialmente inválido.
    pub backup: Option<PathBuf>,
    /// Entradas descartadas por serem inválidas.
    pub skipped_entries: usize,
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

#[derive(Debug)]
pub struct Library {
    dir: PathBuf,
    /// Mais recente primeiro.
    books: Vec<BookEntry>,
    report: LoadReport,
}

impl Library {
    /// Abre (e cria, se preciso) a biblioteca em `dir`. Um índice ilegível não
    /// impede a abertura: ver [`Library::load_report`].
    pub fn open(dir: impl Into<PathBuf>) -> LibraryResult<Self> {
        let dir = dir.into();
        fs::create_dir_all(dir.join(BOOKS_DIR))?;
        fs::create_dir_all(dir.join(COVERS_DIR))?;
        let (books, report) = load_index(&dir)?;
        Ok(Self { dir, books, report })
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
        let (sha256, file_size) = copy_and_hash(source, temp)?;
        if let Some(existing) = self.books.iter().find(|book| book.sha256 == sha256) {
            // Já está na biblioteca. Se a cópia guardada sumiu (apagada à mão),
            // esta é idêntica, byte a byte: volta para o lugar.
            let stored = self.book_file(&existing.id);
            if !stored.exists() {
                fs::rename(temp, &stored)?;
            }
            return Ok(existing.clone());
        }
        // Lê a cópia, não a origem: o que fica guardado é exatamente o que se leu.
        let archive = EpubArchive::open(temp)?;
        let book = EpubBook::parse(&archive)?;
        let id = choose_id(&sha256, &self.books);
        let cover_ext = extract_cover(&archive, &book, &self.dir.join(COVERS_DIR), &id);
        drop(archive);

        let metadata = &book.metadata;
        let title = metadata
            .title
            .clone()
            .or_else(|| {
                source
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Sem título".to_string());
        let entry = BookEntry {
            id: id.clone(),
            sha256,
            title,
            authors: metadata.authors(),
            language: metadata.language.clone(),
            identifier: metadata.identifier.clone(),
            publisher: metadata.publisher.clone(),
            description: metadata.description.clone(),
            date: metadata.date.clone(),
            subjects: metadata.subjects.clone(),
            series: metadata.series.clone(),
            series_index: metadata.series_index,
            source_name: source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            file_size,
            spine_len: book.spine.len(),
            cover_ext: cover_ext.clone(),
            added_unix: now_unix,
            last_opened_unix: None,
            position: None,
            bookmarks: Vec::new(),
        };

        let target = self.book_file(&id);
        let rollback = |this: &Self| {
            let _ = fs::remove_file(&target);
            if let Some(ext) = &cover_ext {
                let _ = fs::remove_file(this.cover_file(&id, ext));
            }
        };
        if let Err(error) = fs::rename(temp, &target) {
            rollback(self);
            return Err(error.into());
        }
        self.books.insert(0, entry.clone());
        if let Err(error) = self.save() {
            self.books.remove(0);
            rollback(self);
            return Err(error);
        }
        Ok(entry)
    }

    /// Move o livro e a capa para `.trash/` e o tira do índice. Devolve o
    /// registro removido.
    pub fn remove(&mut self, id: &str) -> LibraryResult<BookEntry> {
        let index = self.index_of(id)?;
        let trash = self.dir.join(TRASH_DIR);
        fs::create_dir_all(&trash)?;
        let book_file = self.book_file(id);
        let moved_book = move_to_trash(&book_file, &trash, id, "epub")?;
        let cover_file = self.books[index]
            .cover_ext
            .clone()
            .map(|ext| (self.cover_file(id, &ext), ext));
        let moved_cover = cover_file
            .as_ref()
            .and_then(|(file, ext)| move_to_trash(file, &trash, id, ext).ok().flatten());

        let entry = self.books.remove(index);
        if let Err(error) = self.save() {
            self.books.insert(index, entry);
            if let Some(moved) = moved_book {
                let _ = fs::rename(moved, &book_file);
            }
            if let (Some(moved), Some((file, _))) = (moved_cover, cover_file) {
                let _ = fs::rename(moved, file);
            }
            return Err(error);
        }
        Ok(entry)
    }

    /// Guarda onde a leitura parou. `fraction` fora de 0..=1 é cortada;
    /// NaN/infinito e `spine_index` além do fim do spine são recusados.
    pub fn set_position(&mut self, id: &str, position: Position) -> LibraryResult<()> {
        let index = self.index_of(id)?;
        let fraction = checked_place(&self.books[index], position.spine_index, position.fraction)?;
        let position = Position {
            fraction,
            ..position
        };
        self.update(index, |book| book.position = Some(position))
    }

    pub fn set_last_opened(&mut self, id: &str, unix: u64) -> LibraryResult<()> {
        let index = self.index_of(id)?;
        self.update(index, |book| book.last_opened_unix = Some(unix))
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
        let index = self.index_of(id)?;
        let book = &self.books[index];
        let fraction = checked_place(book, spine_index, fraction)?;
        if book.bookmarks.len() >= MAX_BOOKMARKS {
            return Err(LibraryError::TooManyBookmarks(MAX_BOOKMARKS));
        }
        let bookmark = Bookmark {
            id: book.bookmarks.iter().map(|mark| mark.id).max().unwrap_or(0) + 1,
            spine_index,
            fraction,
            label: clean_label(label),
            created_unix,
        };
        let added = bookmark.clone();
        self.update(index, move |book| {
            book.bookmarks.push(bookmark);
            sort_bookmarks(&mut book.bookmarks);
        })?;
        Ok(added)
    }

    pub fn remove_bookmark(&mut self, id: &str, bookmark: u64) -> LibraryResult<Bookmark> {
        let index = self.index_of(id)?;
        let Some(at) = self.books[index]
            .bookmarks
            .iter()
            .position(|mark| mark.id == bookmark)
        else {
            return Err(LibraryError::BookmarkNotFound {
                book: id.to_string(),
                bookmark,
            });
        };
        let removed = self.books[index].bookmarks[at].clone();
        self.update(index, |book| {
            book.bookmarks.remove(at);
        })?;
        Ok(removed)
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

    /// Muda um registro e grava; se a gravação falha, o registro volta a ser
    /// o que era.
    fn update(&mut self, index: usize, change: impl FnOnce(&mut BookEntry)) -> LibraryResult<()> {
        let previous = self.books[index].clone();
        change(&mut self.books[index]);
        if let Err(error) = self.save() {
            self.books[index] = previous;
            return Err(error);
        }
        Ok(())
    }

    fn save(&self) -> LibraryResult<()> {
        let json = serde_json::to_vec_pretty(&IndexOut {
            version: INDEX_VERSION,
            books: &self.books,
        })?;
        write_atomically(&self.dir, &self.dir.join(INDEX_FILE), &json)?;
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
/// passar de [`MAX_BOOK_BYTES`].
fn copy_and_hash(source: &Path, temp: &Path) -> LibraryResult<(String, u64)> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(temp)?;
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
        output.write_all(&buffer[..read])?;
    }
    output.sync_all()?;
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

/// Primeiro nome livre para a cópia de um índice estragado.
fn backup_path(dir: &Path) -> PathBuf {
    let first = dir.join(format!("{INDEX_FILE}.bak"));
    if !first.exists() {
        return first;
    }
    let mut copy = 2;
    loop {
        let candidate = dir.join(format!("{INDEX_FILE}.{copy}.bak"));
        if !candidate.exists() {
            return candidate;
        }
        copy += 1;
    }
}

fn load_index(dir: &Path) -> LibraryResult<(Vec<BookEntry>, LoadReport)> {
    let path = dir.join(INDEX_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), LoadReport::default()));
        }
        Err(error) => return Err(error.into()),
    };
    let Ok(index) = serde_json::from_slice::<IndexIn>(&bytes) else {
        let backup = backup_path(dir);
        fs::rename(&path, &backup)?;
        return Ok((
            Vec::new(),
            LoadReport {
                backup: Some(backup),
                skipped_entries: 0,
            },
        ));
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
    let mut report = LoadReport {
        backup: None,
        skipped_entries: skipped,
    };
    if skipped > 0 {
        // A próxima gravação reescreve o índice sem essas entradas: guarda-se o
        // original antes.
        let backup = backup_path(dir);
        fs::copy(&path, &backup)?;
        report.backup = Some(backup);
    }
    Ok((books, report))
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
    if let Some(position) = &mut book.position {
        position.fraction = position.fraction.clamp(0.0, 1.0);
    }
    for mark in &mut book.bookmarks {
        mark.fraction = mark.fraction.clamp(0.0, 1.0);
        mark.label = clean_label(&mark.label);
    }
    sort_bookmarks(&mut book.bookmarks);
    book
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epub::test_support::{
        CONTAINER_XML, PNG_1X1, ZipBuilder, chapter, epub_with, epub2_sample, epub3_sample, opf,
    };

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
            [BOOKS_DIR, COVERS_DIR, INDEX_FILE]
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
        assert_eq!(file_names(&temp.lib_dir()), [BOOKS_DIR, COVERS_DIR]);
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
        assert_eq!(
            file_names(&trash),
            [format!("{id}.epub"), format!("{id}.png")]
        );
        assert_eq!(fs::read(trash.join(format!("{id}.epub"))).unwrap(), bytes);
        assert!(matches!(
            library.remove(&id),
            Err(LibraryError::NotFound(_))
        ));

        library.add_at(&source, T0 + 1).unwrap();
        library.remove(&id).unwrap();
        assert_eq!(
            file_names(&trash),
            [
                format!("{id}.epub"),
                format!("{id}.png"),
                format!("{id}~2.epub"),
                format!("{id}~2.png"),
            ]
        );
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

        // Uma pasta no lugar do índice: o rename final falha em qualquer sistema.
        let index = temp.lib_dir().join(INDEX_FILE);
        fs::remove_file(&index).unwrap();
        fs::create_dir(&index).unwrap();
        fs::write(index.join("ocupado"), b"x").unwrap();

        assert!(library.set_position(&id, at(2, 0.9, T0 + 2)).is_err());
        assert_eq!(library.get(&id).unwrap().position, Some(at(1, 0.5, T0 + 1)));
        assert!(library.set_last_opened(&id, T0 + 3).is_err());
        assert_eq!(library.get(&id).unwrap().last_opened_unix, None);
        assert!(library.add_bookmark(&id, 0, 0.1, "x", T0 + 4).is_err());
        assert!(library.bookmarks(&id).unwrap().is_empty());

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
        assert!(
            library.book_path(&id).unwrap().exists(),
            "o livro volta do lixo"
        );
        assert!(library.cover_path(&id).unwrap().unwrap().exists());

        let leftovers: Vec<String> = file_names(&temp.lib_dir())
            .into_iter()
            .chain(file_names(&temp.lib_dir().join(BOOKS_DIR)))
            .chain(file_names(&temp.lib_dir().join(COVERS_DIR)))
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
