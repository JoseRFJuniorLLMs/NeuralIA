//! Leitor de ZIP com política EPUB: diretório central, entradas *stored* e
//! *deflate*, ZIP64 quando os campos de 32 bits saturam.
//!
//! O arquivo é tratado como hostil. Tudo o que um EPUB legítimo nunca tem é
//! recusado ao abrir, antes de descompactar qualquer coisa, e o que sobra é
//! lido em fluxo com o tamanho declarado como teto rígido:
//!
//! - no máximo [`MAX_ENTRIES`] entradas, [`MAX_ENTRY_SIZE`] bytes por entrada
//!   e [`MAX_TOTAL_SIZE`] no total (tamanhos descompactados declarados);
//! - razão de compressão até [`MAX_COMPRESSION_RATIO`]:1 para entradas acima
//!   de [`RATIO_CHECK_MIN_SIZE`] (abaixo disso o overhead fixo do deflate torna
//!   a razão ruído, e o estrago possível é pequeno);
//! - nomes: nada de `..`, `/` inicial, letra de unidade, `\`, NUL ou outros
//!   caracteres de controle; nome, campo extra e comentário com tamanho limitado;
//! - criptografia do ZIP (tradicional, forte, AES, cabeçalhos mascarados) e
//!   métodos além de 0/8 recusados;
//! - cada cabeçalho local tem de bater com o diretório central (assinatura,
//!   nome, método) e os dados das entradas não podem se sobrepor nem invadir o
//!   diretório central (a "bomba de sobreposição" reusa os mesmos bytes
//!   comprimidos em várias entradas);
//! - nomes repetidos: vale a primeira entrada do diretório central, sempre.
//!
//! A busca por nome tenta o nome exato e depois o mesmo nome sem distinguir
//! maiúsculas (EPUBs feitos no Windows costumam errar a caixa nos `href`).

use std::{
    collections::HashMap,
    fmt,
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
    sync::Mutex,
};

use flate2::{Crc, read::DeflateDecoder};

use crate::epub::{EpubError, EpubResult, LimitKind};

/// Limites de leitura para um tipo de arquivo ZIP. Novas políticas entram
/// junto com o primeiro consumidor; por enquanto só EPUB está habilitado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZipPolicy {
    pub max_entries: u64,
    pub max_entry_size: u64,
    pub max_total_size: u64,
    pub max_compression_ratio: u64,
    pub ratio_check_min_size: u64,
    pub max_name_len: usize,
    pub max_extra_len: usize,
    pub max_comment_len: usize,
}

impl ZipPolicy {
    /// Os mesmos limites que o leitor EPUB usava antes da mudança de módulo.
    pub const EPUB: Self = Self {
        max_entries: 20_000,
        max_entry_size: 64 * 1024 * 1024,
        max_total_size: 1024 * 1024 * 1024,
        max_compression_ratio: 200,
        ratio_check_min_size: 64 * 1024,
        max_name_len: 1024,
        max_extra_len: 4096,
        max_comment_len: 4096,
    };
}

/// Entradas no diretório central.
pub const MAX_ENTRIES: u64 = ZipPolicy::EPUB.max_entries;
/// Tamanho descompactado de uma entrada.
pub const MAX_ENTRY_SIZE: u64 = ZipPolicy::EPUB.max_entry_size;
/// Soma dos tamanhos descompactados de todas as entradas.
pub const MAX_TOTAL_SIZE: u64 = ZipPolicy::EPUB.max_total_size;
/// Razão máxima entre tamanho descompactado e comprimido.
pub const MAX_COMPRESSION_RATIO: u64 = ZipPolicy::EPUB.max_compression_ratio;
/// Entradas até este tamanho não têm a razão medida.
pub const RATIO_CHECK_MIN_SIZE: u64 = ZipPolicy::EPUB.ratio_check_min_size;
/// Bytes do nome de uma entrada.
pub const MAX_NAME_LEN: usize = ZipPolicy::EPUB.max_name_len;
/// Bytes do campo extra de uma entrada.
pub const MAX_EXTRA_LEN: usize = ZipPolicy::EPUB.max_extra_len;
/// Bytes do comentário de uma entrada.
pub const MAX_COMMENT_LEN: usize = ZipPolicy::EPUB.max_comment_len;

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD_LEN: usize = 22;
const MAX_ARCHIVE_COMMENT: usize = 0xFFFF;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const ZIP64_LOCATOR_LEN: usize = 20;
const ZIP64_EOCD_SIG: u32 = 0x0606_4b50;
const ZIP64_EOCD_LEN: usize = 56;
const ZIP64_EXTRA_ID: u16 = 0x0001;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const CENTRAL_LEN: usize = 46;
const LOCAL_SIG: u32 = 0x0403_4b50;
const LOCAL_LEN: usize = 30;

const FLAG_ENCRYPTED: u16 = 1;
const FLAG_STRONG_ENCRYPTION: u16 = 1 << 6;
const FLAG_MASKED_HEADERS: u16 = 1 << 13;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
const METHOD_AES: u16 = 99;
const SATURATED_32: u64 = 0xFFFF_FFFF;
const SATURATED_16: u64 = 0xFFFF;

/// Como os bytes de uma entrada estão guardados.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Stored,
    Deflate,
}

/// Uma entrada (arquivo, não pasta) do ZIP, já validada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    name: String,
    size: u64,
    compressed_size: u64,
    crc32: u32,
    compression: Compression,
    data_offset: u64,
}

impl ZipEntry {
    /// Nome normalizado (ver [`normalize_entry_name`]).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tamanho descompactado declarado; a leitura nunca passa dele.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    pub fn compression(&self) -> Compression {
        self.compression
    }
}

/// De onde vêm os bytes: memória ou um arquivo aberto (lido por partes, sem
/// carregar o livro inteiro).
enum Source {
    Memory(Vec<u8>),
    File { file: Mutex<File>, len: u64 },
}

impl Source {
    fn len(&self) -> u64 {
        match self {
            Source::Memory(bytes) => bytes.len() as u64,
            Source::File { len, .. } => *len,
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= self.len())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "leitura além do fim do arquivo",
                )
            })?;
        match self {
            Source::Memory(bytes) => {
                // `end <= len`, e `len` veio de um `Vec`: cabe em usize.
                buf.copy_from_slice(&bytes[offset as usize..end as usize]);
                Ok(())
            }
            Source::File { file, .. } => {
                let mut file = file.lock().unwrap_or_else(|poison| poison.into_inner());
                file.seek(SeekFrom::Start(offset))?;
                file.read_exact(buf)
            }
        }
    }
}

/// Uma fatia `[pos, end)` da fonte, lida sob demanda.
struct Section<'a> {
    source: &'a Source,
    pos: u64,
    end: u64,
}

impl Read for Section<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.end.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.source.read_at(self.pos, &mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

/// Um EPUB aberto como ZIP. `Send + Sync`: a UI pode servir recursos de outra
/// thread.
pub struct EpubArchive {
    source: Source,
    entries: Vec<ZipEntry>,
    exact: HashMap<String, usize>,
    folded: HashMap<String, usize>,
}

impl fmt::Debug for EpubArchive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EpubArchive")
            .field("len", &self.source.len())
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl EpubArchive {
    /// Abre o arquivo e valida o diretório central inteiro. Os dados das
    /// entradas só são lidos quando pedidos.
    pub fn open(path: impl AsRef<Path>) -> EpubResult<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Self::from_source(Source::File {
            file: Mutex::new(file),
            len,
        })
    }

    /// Como [`EpubArchive::open`], sobre bytes em memória.
    pub fn from_bytes(bytes: Vec<u8>) -> EpubResult<Self> {
        Self::from_source(Source::Memory(bytes))
    }

    /// Entradas de arquivo na ordem do diretório central (sem pastas e sem
    /// os nomes repetidos que perderam para a primeira ocorrência).
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Procura pelo nome exato (depois de normalizado) e, se não houver, pelo
    /// mesmo nome sem distinguir maiúsculas.
    pub fn entry(&self, name: &str) -> Option<&ZipEntry> {
        let key = normalize_entry_name(name)?;
        let index = self
            .exact
            .get(&key)
            .or_else(|| self.folded.get(&key.to_lowercase()))?;
        self.entries.get(*index)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entry(name).is_some()
    }

    /// Resolve um `href` escrito dentro de `base` (caminho do documento que o
    /// contém) para o nome canônico de uma entrada que existe, mais o
    /// fragmento. Ver [`resolve_href`]. Se o nome descodificado não existir,
    /// tenta o `href` literal (há EPUBs com `%20` no próprio nome da entrada).
    pub fn locate(&self, base: &str, href: &str) -> Option<(String, Option<String>)> {
        let found = |href: &str| {
            let (path, fragment) = resolve_href(base, href)?;
            let entry = self.entry(&path)?;
            Some((entry.name.clone(), fragment))
        };
        found(href).or_else(|| {
            href.contains('%')
                .then(|| found(&href.replace('%', "%25")))
                .flatten()
        })
    }

    /// Lê a entrada inteira (até [`MAX_ENTRY_SIZE`]).
    pub fn read(&self, name: &str) -> EpubResult<Vec<u8>> {
        self.read_capped(name, MAX_ENTRY_SIZE)
    }

    /// Lê a entrada inteira se o tamanho declarado couber em `max`; senão
    /// [`EpubError::Limit`] sem ler nada.
    pub fn read_capped(&self, name: &str, max: u64) -> EpubResult<Vec<u8>> {
        let entry = self
            .entry(name)
            .ok_or_else(|| EpubError::NotFound(name.to_string()))?;
        if entry.size > max {
            return Err(EpubError::Limit {
                kind: LimitKind::EntrySize,
                value: entry.size,
                limit: max,
            });
        }
        // A capacidade inicial não confia no tamanho declarado além de 1 MiB.
        let mut out = Vec::with_capacity(entry.size.min(1024 * 1024) as usize);
        self.reader_for(entry)
            .read_to_end(&mut out)
            .map_err(|error| read_error(&entry.name, error))?;
        Ok(out)
    }

    /// Leitura em fluxo: nunca devolve mais do que o tamanho declarado e
    /// falha (em vez de terminar em silêncio) se o fluxo for mais curto, mais
    /// longo ou se o CRC-32 não conferir.
    pub fn open_entry(&self, name: &str) -> EpubResult<EntryReader<'_>> {
        let entry = self
            .entry(name)
            .ok_or_else(|| EpubError::NotFound(name.to_string()))?;
        Ok(self.reader_for(entry))
    }

    fn reader_for<'a>(&'a self, entry: &ZipEntry) -> EntryReader<'a> {
        let section = Section {
            source: &self.source,
            pos: entry.data_offset,
            end: entry.data_offset + entry.compressed_size,
        };
        let inner = match entry.compression {
            Compression::Stored => EntryStream::Stored(section),
            Compression::Deflate => EntryStream::Deflate(Box::new(DeflateDecoder::new(section))),
        };
        EntryReader {
            inner,
            remaining: entry.size,
            expected_crc: entry.crc32,
            crc: Crc::new(),
            done: false,
        }
    }

    fn from_source(source: Source) -> EpubResult<Self> {
        let directory = find_directory(&source)?;
        let mut records = read_central_directory(&source, &directory)?;
        check_local_headers(&source, &directory, &mut records)?;

        let mut entries = Vec::with_capacity(records.len());
        let mut exact = HashMap::new();
        let mut folded = HashMap::new();
        for record in records {
            if record.is_dir || exact.contains_key(&record.name) {
                continue;
            }
            let index = entries.len();
            exact.insert(record.name.clone(), index);
            folded.entry(record.name.to_lowercase()).or_insert(index);
            entries.push(ZipEntry {
                name: record.name,
                size: record.size,
                compressed_size: record.compressed_size,
                crc32: record.crc32,
                compression: record.compression,
                data_offset: record.data_offset,
            });
        }
        Ok(Self {
            source,
            entries,
            exact,
            folded,
        })
    }
}

enum EntryStream<'a> {
    Stored(Section<'a>),
    Deflate(Box<DeflateDecoder<Section<'a>>>),
}

impl Read for EntryStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            EntryStream::Stored(section) => section.read(buf),
            EntryStream::Deflate(decoder) => decoder.read(buf),
        }
    }
}

/// Leitor de uma entrada; ver [`EpubArchive::open_entry`].
pub struct EntryReader<'a> {
    inner: EntryStream<'a>,
    remaining: u64,
    expected_crc: u32,
    crc: Crc,
    done: bool,
}

impl Read for EntryReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            // O fluxo tem de acabar exatamente aqui.
            let mut probe = [0u8; 1];
            if self.inner.read(&mut probe)? != 0 {
                return Err(corrupt("o fluxo passa do tamanho declarado"));
            }
            if self.crc.sum() != self.expected_crc {
                return Err(corrupt("CRC-32 não confere"));
            }
            self.done = true;
            return Ok(0);
        }
        let want = buf
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fluxo mais curto do que o tamanho declarado",
            ));
        }
        self.crc.update(&buf[..n]);
        self.remaining -= n as u64;
        Ok(n)
    }
}

fn corrupt(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason.to_string())
}

fn read_error(name: &str, error: io::Error) -> EpubError {
    match error.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::UnexpectedEof => {
            EpubError::Corrupt {
                name: name.to_string(),
                reason: error.to_string(),
            }
        }
        _ => EpubError::Io(error),
    }
}

/// Onde está o diretório central e onde ele tem de acabar.
struct Directory {
    entries: u64,
    offset: u64,
    size: u64,
    /// Início do registro de fim (ZIP64 ou clássico): o diretório acaba antes.
    limit: u64,
}

fn find_directory(source: &Source) -> EpubResult<Directory> {
    let len = source.len();
    if len < EOCD_LEN as u64 {
        return Err(EpubError::NotZip("arquivo pequeno demais".into()));
    }
    let tail_len = len.min((EOCD_LEN + MAX_ARCHIVE_COMMENT) as u64) as usize;
    let tail_start = len - tail_len as u64;
    let mut tail = vec![0u8; tail_len];
    source.read_at(tail_start, &mut tail)?;

    // De trás para a frente: o registro cujo comentário acaba no fim do
    // arquivo; na falta dele, o último que cabe (lixo depois do ZIP).
    let mut exact = None;
    let mut loose = None;
    for at in (0..=tail_len - EOCD_LEN).rev() {
        if le32(&tail, at) != EOCD_SIG {
            continue;
        }
        let end = at + EOCD_LEN + le16(&tail, at + 20) as usize;
        if end == tail_len {
            exact = Some(at);
            break;
        }
        if end < tail_len && loose.is_none() {
            loose = Some(at);
        }
    }
    let at = exact
        .or(loose)
        .ok_or_else(|| EpubError::NotZip("fim do diretório central não encontrado".into()))?;
    let record = &tail[at..at + EOCD_LEN];
    let eocd_pos = tail_start + at as u64;
    let disk = le16(record, 4);
    let directory_disk = le16(record, 6);
    let disk_entries = u64::from(le16(record, 8));
    let entries = u64::from(le16(record, 10));
    let size = u64::from(le32(record, 12));
    let offset = u64::from(le32(record, 16));
    let saturated = disk_entries == SATURATED_16
        || entries == SATURATED_16
        || size == SATURATED_32
        || offset == SATURATED_32;

    let directory = match read_zip64_directory(source, eocd_pos)? {
        Some(directory) => directory,
        None if saturated => {
            return Err(EpubError::NotZip(
                "campos ZIP64 sem o registro de fim ZIP64".into(),
            ));
        }
        None => {
            if disk != 0 || directory_disk != 0 || disk_entries != entries {
                return Err(EpubError::Unsupported("ZIP dividido em volumes".into()));
            }
            Directory {
                entries,
                offset,
                size,
                limit: eocd_pos,
            }
        }
    };
    if directory.entries > MAX_ENTRIES {
        return Err(EpubError::Limit {
            kind: LimitKind::Entries,
            value: directory.entries,
            limit: MAX_ENTRIES,
        });
    }
    let fits = directory
        .offset
        .checked_add(directory.size)
        .is_some_and(|end| end <= directory.limit);
    if !fits {
        return Err(EpubError::NotZip(
            "diretório central fora do arquivo".into(),
        ));
    }
    Ok(directory)
}

fn read_zip64_directory(source: &Source, eocd_pos: u64) -> EpubResult<Option<Directory>> {
    let Some(locator_pos) = eocd_pos.checked_sub(ZIP64_LOCATOR_LEN as u64) else {
        return Ok(None);
    };
    let mut locator = [0u8; ZIP64_LOCATOR_LEN];
    source.read_at(locator_pos, &mut locator)?;
    if le32(&locator, 0) != ZIP64_LOCATOR_SIG {
        return Ok(None);
    }
    let record_pos = le64(&locator, 8);
    let total_disks = le32(&locator, 16);
    if le32(&locator, 4) != 0 || total_disks > 1 {
        return Err(EpubError::Unsupported("ZIP64 dividido em volumes".into()));
    }
    let fits = record_pos
        .checked_add(ZIP64_EOCD_LEN as u64)
        .is_some_and(|end| end <= locator_pos);
    if !fits {
        return Err(EpubError::NotZip(
            "registro de fim ZIP64 fora do lugar".into(),
        ));
    }
    let mut record = [0u8; ZIP64_EOCD_LEN];
    source.read_at(record_pos, &mut record)?;
    if le32(&record, 0) != ZIP64_EOCD_SIG {
        return Err(EpubError::NotZip("assinatura ZIP64 inválida".into()));
    }
    let disk = le32(&record, 16);
    let directory_disk = le32(&record, 20);
    let disk_entries = le64(&record, 24);
    let entries = le64(&record, 32);
    if disk != 0 || directory_disk != 0 || disk_entries != entries {
        return Err(EpubError::Unsupported("ZIP64 dividido em volumes".into()));
    }
    Ok(Some(Directory {
        entries,
        size: le64(&record, 40),
        offset: le64(&record, 48),
        limit: record_pos,
    }))
}

struct CentralRecord {
    raw_name: Vec<u8>,
    name: String,
    is_dir: bool,
    method: u16,
    compression: Compression,
    size: u64,
    compressed_size: u64,
    crc32: u32,
    header_offset: u64,
    data_offset: u64,
}

fn read_central_directory(
    source: &Source,
    directory: &Directory,
) -> EpubResult<Vec<CentralRecord>> {
    let section = Section {
        source,
        pos: directory.offset,
        end: directory.offset + directory.size,
    };
    let mut reader = BufReader::with_capacity(64 * 1024, section);
    let truncated = |error: io::Error| match error.kind() {
        io::ErrorKind::UnexpectedEof => EpubError::NotZip("diretório central truncado".into()),
        _ => EpubError::Io(error),
    };
    // `entries <= MAX_ENTRIES`: a reserva é limitada.
    let mut records = Vec::with_capacity(directory.entries as usize);
    let mut total: u64 = 0;
    for _ in 0..directory.entries {
        let mut fixed = [0u8; CENTRAL_LEN];
        reader.read_exact(&mut fixed).map_err(truncated)?;
        if le32(&fixed, 0) != CENTRAL_SIG {
            return Err(EpubError::NotZip(
                "assinatura inválida no diretório central".into(),
            ));
        }
        let flags = le16(&fixed, 8);
        let method = le16(&fixed, 10);
        let crc32 = le32(&fixed, 16);
        let mut compressed_size = u64::from(le32(&fixed, 20));
        let mut size = u64::from(le32(&fixed, 24));
        let name_len = le16(&fixed, 28) as usize;
        let extra_len = le16(&fixed, 30) as usize;
        let comment_len = le16(&fixed, 32) as usize;
        let mut disk_start = u64::from(le16(&fixed, 34));
        let mut header_offset = u64::from(le32(&fixed, 42));

        for (kind, value, limit) in [
            (LimitKind::NameLength, name_len, MAX_NAME_LEN),
            (LimitKind::ExtraLength, extra_len, MAX_EXTRA_LEN),
            (LimitKind::CommentLength, comment_len, MAX_COMMENT_LEN),
        ] {
            if value > limit {
                return Err(EpubError::Limit {
                    kind,
                    value: value as u64,
                    limit: limit as u64,
                });
            }
        }
        let mut raw_name = vec![0u8; name_len];
        reader.read_exact(&mut raw_name).map_err(truncated)?;
        let mut extra = vec![0u8; extra_len];
        reader.read_exact(&mut extra).map_err(truncated)?;
        let mut comment = vec![0u8; comment_len];
        reader.read_exact(&mut comment).map_err(truncated)?;

        let display = String::from_utf8_lossy(&raw_name).into_owned();
        if size == SATURATED_32
            || compressed_size == SATURATED_32
            || header_offset == SATURATED_32
            || disk_start == SATURATED_16
        {
            let mut values = Zip64Values {
                size: (size == SATURATED_32).then_some(&mut size),
                compressed_size: (compressed_size == SATURATED_32).then_some(&mut compressed_size),
                header_offset: (header_offset == SATURATED_32).then_some(&mut header_offset),
                disk_start: (disk_start == SATURATED_16).then_some(&mut disk_start),
            };
            if !values.fill_from(&extra) {
                return Err(EpubError::NotZip(format!(
                    "campo ZIP64 incompleto em {display:?}"
                )));
            }
        }
        if disk_start != 0 {
            return Err(EpubError::Unsupported("ZIP dividido em volumes".into()));
        }

        let name =
            normalize_entry_name(&display).ok_or_else(|| EpubError::UnsafeName(display.clone()))?;
        let is_dir = raw_name.last() == Some(&b'/');
        if flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION | FLAG_MASKED_HEADERS) != 0
            || method == METHOD_AES
        {
            return Err(EpubError::Encrypted(display));
        }
        let compression = match method {
            METHOD_STORED => Compression::Stored,
            METHOD_DEFLATE => Compression::Deflate,
            method => {
                return Err(EpubError::UnsupportedCompression {
                    name: display,
                    method,
                });
            }
        };
        if size > MAX_ENTRY_SIZE {
            return Err(EpubError::Limit {
                kind: LimitKind::EntrySize,
                value: size,
                limit: MAX_ENTRY_SIZE,
            });
        }
        total = total.saturating_add(size);
        if total > MAX_TOTAL_SIZE {
            return Err(EpubError::Limit {
                kind: LimitKind::TotalSize,
                value: total,
                limit: MAX_TOTAL_SIZE,
            });
        }
        if size > RATIO_CHECK_MIN_SIZE
            && size > compressed_size.saturating_mul(MAX_COMPRESSION_RATIO)
        {
            return Err(EpubError::Limit {
                kind: LimitKind::CompressionRatio,
                value: size / compressed_size.max(1),
                limit: MAX_COMPRESSION_RATIO,
            });
        }
        if compression == Compression::Stored && compressed_size != size {
            return Err(EpubError::Corrupt {
                name: display,
                reason: "entrada sem compressão com tamanhos diferentes".into(),
            });
        }
        records.push(CentralRecord {
            raw_name,
            name,
            is_dir,
            method,
            compression,
            size,
            compressed_size,
            crc32,
            header_offset,
            data_offset: 0,
        });
    }
    Ok(records)
}

/// Os campos saturados de uma entrada, preenchidos pelo extra ZIP64 na ordem
/// da especificação (só os que estão saturados aparecem lá).
struct Zip64Values<'a> {
    size: Option<&'a mut u64>,
    compressed_size: Option<&'a mut u64>,
    header_offset: Option<&'a mut u64>,
    disk_start: Option<&'a mut u64>,
}

impl Zip64Values<'_> {
    fn fill_from(&mut self, extra: &[u8]) -> bool {
        let mut at = 0usize;
        while at + 4 <= extra.len() {
            let id = le16(extra, at);
            let len = le16(extra, at + 2) as usize;
            let Some(data) = extra.get(at + 4..at + 4 + len) else {
                return false;
            };
            if id == ZIP64_EXTRA_ID {
                let mut cursor = 0usize;
                for slot in [
                    &mut self.size,
                    &mut self.compressed_size,
                    &mut self.header_offset,
                ] {
                    if let Some(value) = slot.as_deref_mut() {
                        let Some(bytes) = data.get(cursor..cursor + 8) else {
                            return false;
                        };
                        *value = le64(bytes, 0);
                        cursor += 8;
                    }
                }
                if let Some(value) = self.disk_start.as_deref_mut() {
                    let Some(bytes) = data.get(cursor..cursor + 4) else {
                        return false;
                    };
                    *value = u64::from(le32(bytes, 0));
                }
                return true;
            }
            at += 4 + len;
        }
        false
    }
}

/// Confere cada cabeçalho local contra o diretório central e calcula onde
/// começam os dados. Entradas sobrepostas ou que invadem o diretório central
/// são recusadas.
fn check_local_headers(
    source: &Source,
    directory: &Directory,
    records: &mut [CentralRecord],
) -> EpubResult<()> {
    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by_key(|&index| records[index].header_offset);
    let mut previous_end = 0u64;
    for index in order {
        let record = &mut records[index];
        let fail = |reason: &str| EpubError::Corrupt {
            name: record.name.clone(),
            reason: reason.to_string(),
        };
        if record.header_offset < previous_end {
            return Err(fail("entradas sobrepostas no ZIP"));
        }
        let mut fixed = [0u8; LOCAL_LEN];
        source
            .read_at(record.header_offset, &mut fixed)
            .map_err(|_| fail("cabeçalho local fora do arquivo"))?;
        if le32(&fixed, 0) != LOCAL_SIG {
            return Err(fail("cabeçalho local inválido"));
        }
        if le16(&fixed, 8) != record.method {
            return Err(fail(
                "método do cabeçalho local difere do diretório central",
            ));
        }
        let name_len = le16(&fixed, 26) as usize;
        let extra_len = u64::from(le16(&fixed, 28));
        if name_len != record.raw_name.len() {
            return Err(fail("nome do cabeçalho local difere do diretório central"));
        }
        let mut local_name = vec![0u8; name_len];
        source
            .read_at(record.header_offset + LOCAL_LEN as u64, &mut local_name)
            .map_err(|_| fail("cabeçalho local fora do arquivo"))?;
        if local_name != record.raw_name {
            return Err(fail("nome do cabeçalho local difere do diretório central"));
        }
        let data_offset = record.header_offset + (LOCAL_LEN + name_len) as u64 + extra_len;
        let data_end = data_offset
            .checked_add(record.compressed_size)
            .filter(|end| *end <= directory.offset)
            .ok_or_else(|| fail("dados da entrada invadem o diretório central"))?;
        record.data_offset = data_offset;
        previous_end = data_end;
    }
    Ok(())
}

/// Normaliza o nome de uma entrada: tira segmentos vazios e `.`; recusa
/// (`None`) `..`, nome absoluto, letra de unidade, `\`, NUL e outros
/// caracteres de controle. `"OEBPS//./a.xhtml"` vira `"OEBPS/a.xhtml"`.
pub fn normalize_entry_name(name: &str) -> Option<String> {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') {
        return None;
    }
    if name.chars().any(char::is_control) {
        return None;
    }
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return None;
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Resolve um `href` relativo ao documento `base` (caminho de entrada, por
/// exemplo `OEBPS/content.opf`) e devolve `(caminho, fragmento)`.
///
/// O caminho é descodificado de `%XX`, `\` vira `/`, `.`/`..` são aplicados e
/// o resultado passa por [`normalize_entry_name`]. `None` para URLs com
/// esquema (`http:`, `data:`...) e para o que sairia da raiz do ZIP. Um `href`
/// só de fragmento (`#nota`) aponta para o próprio `base`.
pub fn resolve_href(base: &str, href: &str) -> Option<(String, Option<String>)> {
    let href = href.trim();
    let (path, fragment) = match href.split_once('#') {
        Some((path, fragment)) => (path, Some(percent_decode(fragment))),
        None => (href, None),
    };
    let fragment = fragment.filter(|fragment| !fragment.is_empty());
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    if has_scheme(path) {
        return None;
    }
    let decoded = percent_decode(path).replace('\\', "/");
    if decoded.is_empty() {
        return normalize_entry_name(base).map(|base| (base, fragment));
    }
    let mut segments: Vec<&str> = Vec::new();
    if !decoded.starts_with('/')
        && let Some((dir, _)) = base.rsplit_once('/')
    {
        segments.extend(
            dir.split('/')
                .filter(|part| !part.is_empty() && *part != "."),
        );
    }
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            part => segments.push(part),
        }
    }
    normalize_entry_name(&segments.join("/")).map(|path| (path, fragment))
}

fn has_scheme(href: &str) -> bool {
    let Some((scheme, _)) = href.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `%XX` → byte; sequências inválidas ficam como estão. Bytes que não formam
/// UTF-8 viram U+FFFD.
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(at + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(at + 2).and_then(|b| (*b as char).to_digit(16)),
            )
        {
            out.push((high * 16 + low) as u8);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epub::test_support::ZipBuilder;

    #[test]
    fn epub_policy_is_unchanged_by_the_safezip_move() {
        assert_eq!(ZipPolicy::EPUB.max_entries, 20_000);
        assert_eq!(ZipPolicy::EPUB.max_entry_size, 64 * 1024 * 1024);
        assert_eq!(ZipPolicy::EPUB.max_total_size, 1024 * 1024 * 1024);
        assert_eq!(ZipPolicy::EPUB.max_compression_ratio, 200);
        assert_eq!(ZipPolicy::EPUB.ratio_check_min_size, 64 * 1024);
        assert_eq!(ZipPolicy::EPUB.max_name_len, 1024);
        assert_eq!(ZipPolicy::EPUB.max_extra_len, 4096);
        assert_eq!(ZipPolicy::EPUB.max_comment_len, 4096);
        assert_eq!(MAX_ENTRIES, ZipPolicy::EPUB.max_entries);
        assert_eq!(MAX_ENTRY_SIZE, ZipPolicy::EPUB.max_entry_size);
        assert_eq!(MAX_TOTAL_SIZE, ZipPolicy::EPUB.max_total_size);
        assert_eq!(MAX_COMPRESSION_RATIO, ZipPolicy::EPUB.max_compression_ratio);
        assert_eq!(RATIO_CHECK_MIN_SIZE, ZipPolicy::EPUB.ratio_check_min_size);
        assert_eq!(MAX_NAME_LEN, ZipPolicy::EPUB.max_name_len);
        assert_eq!(MAX_EXTRA_LEN, ZipPolicy::EPUB.max_extra_len);
        assert_eq!(MAX_COMMENT_LEN, ZipPolicy::EPUB.max_comment_len);
    }

    #[test]
    fn central_directory_truncations_and_bit_flips_never_panic() {
        let seed = ZipBuilder::new()
            .stored("mimetype", b"application/epub+zip")
            .deflated("chapter.xhtml", b"<p>Texto de teste</p>")
            .build();
        assert!(EpubArchive::from_bytes(seed.clone()).is_ok());
        for len in 0..seed.len().min(4096) {
            let bytes = seed[..len].to_vec();
            assert!(
                std::panic::catch_unwind(|| EpubArchive::from_bytes(bytes)).is_ok(),
                "parser panicou após corte em {len}"
            );
        }
        for flip in 0..257usize {
            let mut bytes = seed.clone();
            let offset = flip.wrapping_mul(97) % bytes.len();
            bytes[offset] ^= 1 << (flip % 8);
            assert!(
                std::panic::catch_unwind(|| EpubArchive::from_bytes(bytes)).is_ok(),
                "parser panicou após bit flip {flip}"
            );
        }
    }
}
