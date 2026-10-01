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
//!
//! A segunda política, [`ZipPolicy::BROWSE_LITE`], é a da inspeção de um ZIP
//! baixado (downloads-zip-inspect, [`list_central_directory`]): o diretório
//! central (um só possível) e o cabeçalho local de cada entrada, com um teto
//! de entradas, de bytes do diretório e de bytes dos cabeçalhos locais, sem
//! os tetos de tamanho do EPUB (um ZIP legítimo de 2 GiB é inspecionado) e
//! sem ler um byte dos dados das entradas.

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
/// junto com o primeiro consumidor: EPUB (o leitor de livros) e
/// BROWSE_LITE (a inspeção dos downloads).
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
    /// Bytes do diretório central (o que a listagem lê). `u64::MAX`: sem
    /// teto próprio (o EPUB já o limita pelas entradas e pelos campos).
    pub max_directory_size: u64,
    /// Nomes e campos extra dos cabeçalhos locais, somados, que a listagem
    /// lê ([`list_central_directory`]). `u64::MAX` no EPUB, que confere os
    /// cabeçalhos locais pelo caminho dele.
    pub max_local_header_size: u64,
    /// Só um diretório central possível: o registro de fim escolhido é a
    /// última assinatura dele no arquivo; o diretório acaba colado ao
    /// registro de fim (ou ao registro ZIP64, e este colado ao
    /// localizador); e os campos clássicos que não estão saturados dizem o
    /// mesmo que os do ZIP64. Um leitor que escolhesse outro registro, que
    /// lesse o diretório pelo fim ou que usasse os campos clássicos
    /// listaria outras entradas. Desligado no EPUB (como antes).
    pub unambiguous_directory: bool,
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
        max_directory_size: u64::MAX,
        max_local_header_size: u64::MAX,
        unambiguous_directory: false,
    };

    /// A inspeção de um ZIP baixado (downloads-zip-inspect): o diretório
    /// central e o cabeçalho local de cada entrada, sem os tetos de tamanho
    /// do EPUB -- os dados das entradas nunca são lidos, por isso o tamanho
    /// deles não custa nada. Os campos de 16 bits (nome, extra, comentário)
    /// ficam no máximo do formato. O que limita o trabalho, que corre na
    /// thread da interface, é o número de entradas, os bytes do diretório e
    /// os dos cabeçalhos locais: acima deles, a inspeção falha e o arquivo
    /// fica «não inspecionado», nunca «seguro». E o diretório tem de ser um
    /// só ([`ZipPolicy::unambiguous_directory`]).
    pub const BROWSE_LITE: Self = Self {
        max_entries: 100_000,
        max_entry_size: u64::MAX,
        max_total_size: u64::MAX,
        max_compression_ratio: u64::MAX,
        ratio_check_min_size: u64::MAX,
        max_name_len: 0xFFFF,
        max_extra_len: 0xFFFF,
        max_comment_len: 0xFFFF,
        max_directory_size: 32 * 1024 * 1024,
        max_local_header_size: 32 * 1024 * 1024,
        unambiguous_directory: true,
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
const UNICODE_PATH_EXTRA_ID: u16 = 0x7075;
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

/// Um fluxo com posição que a [`Source`] pode guardar (o arquivo aberto, ou
/// nos testes um leitor que conta o que lê).
trait ReadSeek: Read + Seek + Send {}

impl<T: Read + Seek + Send> ReadSeek for T {}

/// De onde vêm os bytes: memória ou um fluxo com posição (o arquivo aberto,
/// lido por partes, sem carregar o livro inteiro).
enum Source {
    Memory(Vec<u8>),
    Stream { stream: Mutex<Positioned>, len: u64 },
}

/// O fluxo e onde ele está (`None` depois de um erro): uma leitura que
/// começa onde a anterior acabou não pede um `seek` -- no Windows, cada um
/// é uma chamada ao sistema, e a listagem de um ZIP lê o cabeçalho local de
/// cada entrada em duas leituras seguidas.
struct Positioned {
    stream: Box<dyn ReadSeek>,
    pos: Option<u64>,
}

impl Source {
    /// Um fluxo, com o tamanho dado pelo fim dele.
    fn stream(mut stream: impl Read + Seek + Send + 'static) -> io::Result<Self> {
        let len = stream.seek(SeekFrom::End(0))?;
        Ok(Source::Stream {
            stream: Mutex::new(Positioned {
                stream: Box::new(stream),
                pos: Some(len),
            }),
            len,
        })
    }

    fn len(&self) -> u64 {
        match self {
            Source::Memory(bytes) => bytes.len() as u64,
            Source::Stream { len, .. } => *len,
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
            Source::Stream { stream, .. } => {
                let mut guard = stream.lock().unwrap_or_else(|poison| poison.into_inner());
                let Positioned { stream, pos } = &mut *guard;
                if *pos != Some(offset) {
                    *pos = None;
                    stream.seek(SeekFrom::Start(offset))?;
                }
                *pos = None;
                stream.read_exact(buf)?;
                *pos = Some(end);
                Ok(())
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
        Self::from_source(Source::stream(File::open(path)?)?)
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
        let policy = &ZipPolicy::EPUB;
        let directory = find_directory(&source, policy)?;
        let mut records = read_central_directory(&source, &directory, policy)?;
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

fn find_directory(source: &Source, policy: &ZipPolicy) -> EpubResult<Directory> {
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
    // Os outros leitores (7-Zip, .NET, libarchive, Python) procuram de trás
    // para a frente e ficam com a última assinatura: uma depois da
    // escolhida (dentro do comentário dela, ou no lixo do fim) apontaria
    // para outro diretório.
    if policy.unambiguous_directory
        && tail[at + 1..]
            .windows(4)
            .any(|window| window == EOCD_SIG.to_le_bytes())
    {
        return Err(EpubError::NotZip(
            "mais de um registro de fim do diretório central".into(),
        ));
    }
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

    let directory = match read_zip64_directory(source, eocd_pos, policy)? {
        Some(directory) => {
            // Um leitor que só olha para o ZIP64 quando um campo clássico
            // satura (o .NET) usaria os clássicos: têm de dizer o mesmo.
            let agrees =
                |classic: u64, saturated: u64, zip64: u64| classic == saturated || classic == zip64;
            if policy.unambiguous_directory
                && !(agrees(u64::from(disk), SATURATED_16, 0)
                    && agrees(u64::from(directory_disk), SATURATED_16, 0)
                    && agrees(disk_entries, SATURATED_16, directory.entries)
                    && agrees(entries, SATURATED_16, directory.entries)
                    && agrees(size, SATURATED_32, directory.size)
                    && agrees(offset, SATURATED_32, directory.offset))
            {
                return Err(EpubError::NotZip(
                    "registro de fim clássico difere do ZIP64".into(),
                ));
            }
            directory
        }
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
    if directory.entries > policy.max_entries {
        return Err(EpubError::Limit {
            kind: LimitKind::Entries,
            value: directory.entries,
            limit: policy.max_entries,
        });
    }
    if directory.size > policy.max_directory_size {
        return Err(EpubError::Limit {
            kind: LimitKind::DirectorySize,
            value: directory.size,
            limit: policy.max_directory_size,
        });
    }
    let end = directory.offset.checked_add(directory.size);
    if !end.is_some_and(|end| end <= directory.limit) {
        return Err(EpubError::NotZip(
            "diretório central fora do arquivo".into(),
        ));
    }
    // Bytes entre o fim declarado do diretório e o registro de fim: um
    // leitor que ache o diretório pelo fim (o 7-Zip, com o arquivo
    // deslocado) leria outros registros centrais.
    if policy.unambiguous_directory && end != Some(directory.limit) {
        return Err(EpubError::NotZip(
            "bytes entre o diretório central e o registro de fim".into(),
        ));
    }
    Ok(directory)
}

fn read_zip64_directory(
    source: &Source,
    eocd_pos: u64,
    policy: &ZipPolicy,
) -> EpubResult<Option<Directory>> {
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
    // O registro ZIP64 acaba no localizador: o 7-Zip lê o registro que está
    // colado ao localizador antes de seguir o offset dele.
    let record_end = record_pos
        .checked_add(12)
        .and_then(|end| end.checked_add(le64(&record, 4)));
    if policy.unambiguous_directory && record_end != Some(locator_pos) {
        return Err(EpubError::NotZip(
            "bytes entre o registro de fim ZIP64 e o localizador".into(),
        ));
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

/// Um registro do diretório central com os campos ZIP64 já preenchidos,
/// antes das regras de cada política (nome, criptografia, método,
/// tamanhos).
struct RawRecord {
    raw_name: Vec<u8>,
    /// O campo extra do registro central, como veio.
    extra: Vec<u8>,
    flags: u16,
    method: u16,
    crc32: u32,
    size: u64,
    compressed_size: u64,
    header_offset: u64,
}

/// Percorre as `directory.entries` entradas do diretório central, lendo só
/// os bytes dele, e entrega cada uma a `visit`, pela ordem. Nome, extra e
/// comentário passam pelos tetos de `policy`; os campos saturados vêm do
/// extra ZIP64; uma entrada noutro volume é recusada. Devolve se sobraram
/// bytes no diretório depois da última entrada.
fn walk_central_directory(
    source: &Source,
    directory: &Directory,
    policy: &ZipPolicy,
    mut visit: impl FnMut(RawRecord) -> EpubResult<()>,
) -> EpubResult<bool> {
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
            (LimitKind::NameLength, name_len, policy.max_name_len),
            (LimitKind::ExtraLength, extra_len, policy.max_extra_len),
            (
                LimitKind::CommentLength,
                comment_len,
                policy.max_comment_len,
            ),
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
        if comment_len > 0 {
            let mut remaining = comment_len;
            let mut drain = [0u8; 256];
            while remaining > 0 {
                let chunk = remaining.min(drain.len());
                reader.read_exact(&mut drain[..chunk]).map_err(truncated)?;
                remaining -= chunk;
            }
        }

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
                let display = String::from_utf8_lossy(&raw_name);
                return Err(EpubError::NotZip(format!(
                    "campo ZIP64 incompleto em {display:?}"
                )));
            }
        }
        if disk_start != 0 {
            return Err(EpubError::Unsupported("ZIP dividido em volumes".into()));
        }
        visit(RawRecord {
            raw_name,
            extra,
            flags,
            method,
            crc32,
            size,
            compressed_size,
            header_offset,
        })?;
    }
    let mut probe = [0u8; 1];
    Ok(reader.read(&mut probe).map_err(truncated)? != 0)
}

/// O diretório central pelas regras do EPUB: nomes normalizados, nada de
/// criptografia nem de métodos além de 0/8, e os tetos de tamanho e de
/// razão de `policy`.
fn read_central_directory(
    source: &Source,
    directory: &Directory,
    policy: &ZipPolicy,
) -> EpubResult<Vec<CentralRecord>> {
    // `entries <= policy.max_entries`: a reserva é limitada.
    let mut records = Vec::with_capacity(directory.entries as usize);
    let mut total: u64 = 0;
    walk_central_directory(source, directory, policy, |raw| {
        let RawRecord {
            raw_name,
            extra: _,
            flags,
            method,
            crc32,
            size,
            compressed_size,
            header_offset,
        } = raw;
        let display = String::from_utf8_lossy(&raw_name).into_owned();
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
        if size > policy.max_entry_size {
            return Err(EpubError::Limit {
                kind: LimitKind::EntrySize,
                value: size,
                limit: policy.max_entry_size,
            });
        }
        total = total.saturating_add(size);
        if total > policy.max_total_size {
            return Err(EpubError::Limit {
                kind: LimitKind::TotalSize,
                value: total,
                limit: policy.max_total_size,
            });
        }
        if size > policy.ratio_check_min_size
            && size > compressed_size.saturating_mul(policy.max_compression_ratio)
        {
            return Err(EpubError::Limit {
                kind: LimitKind::CompressionRatio,
                value: size / compressed_size.max(1),
                limit: policy.max_compression_ratio,
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
        Ok(())
    })?;
    Ok(records)
}

/// Os nomes dos campos Info-ZIP Unicode Path (`0x7075`: versão, CRC32 do
/// nome cru e o nome em UTF-8) de um bloco extra, central ou local. O
/// 7-Zip e o `tar.exe` (libarchive) extraem a entrada com este nome no lugar
/// do cru. Sem conferir a versão nem o CRC (um extrator que não os confira
/// usa o nome na mesma); um subcampo que passa do fim do bloco dá o que
/// cabe.
fn unicode_path_names(extra: &[u8]) -> Vec<&[u8]> {
    let mut names = Vec::new();
    let mut at = 0usize;
    while at + 4 <= extra.len() {
        let id = le16(extra, at);
        let len = le16(extra, at + 2) as usize;
        let data = &extra[at + 4..extra.len().min(at + 4 + len)];
        if id == UNICODE_PATH_EXTRA_ID && data.len() > 5 {
            names.push(&data[5..]);
        }
        at += 4 + len;
    }
    names
}

/// Lista as entradas de um ZIP sem ler os dados delas (downloads-zip-inspect,
/// política [`ZipPolicy::BROWSE_LITE`]): lê o fim do arquivo (o registro de
/// fim, até 64 KiB de comentário, e o ZIP64), os bytes do diretório central
/// e o cabeçalho local de cada entrada (30 bytes, o nome e o campo extra),
/// e mais nada -- nunca os dados. `visit` recebe cada nome de cada entrada
/// (pastas incluídas), em UTF-8 com perdas e sem normalizar, à medida que é
/// lido: o nome cru do diretório central e o do campo Unicode Path
/// (`0x7075`) dele, pela ordem do diretório; depois, pela ordem do arquivo,
/// o nome do cabeçalho local quando difere do central e os `0x7075` do
/// extra local (o `tar.exe` extrai pelo cabeçalho local). Quem inspeciona
/// fica com o que viu mesmo quando a listagem falha depois.
///
/// Falha (e o arquivo não conta como listado) com um diretório truncado, com
/// assinaturas erradas, fora do arquivo, com bytes a mais no fim, acima dos
/// tetos da política, noutro volume, com ZIP64 incompleto, com mais de um
/// diretório possível ([`ZipPolicy::unambiguous_directory`]), com entradas
/// que se sobrepõem ou invadem o diretório central -- medidas pelo mínimo
/// que cada uma ocupa (cabeçalho local de 30 bytes, o nome e os dados
/// comprimidos) --, ou com um cabeçalho local sem assinatura, que invade o
/// diretório ou cujo nome difere do central. Devolve o número de entradas.
pub fn list_central_directory(
    reader: impl Read + Seek + Send + 'static,
    policy: &ZipPolicy,
    mut visit: impl FnMut(&str),
) -> EpubResult<u64> {
    let source = Source::stream(reader)?;
    let directory = find_directory(&source, policy)?;
    // `entries <= policy.max_entries`: a reserva é limitada.
    let mut spans: Vec<(u64, u64, Vec<u8>)> = Vec::with_capacity(directory.entries as usize);
    let trailing = walk_central_directory(&source, &directory, policy, |raw| {
        visit(&String::from_utf8_lossy(&raw.raw_name));
        for unicode in unicode_path_names(&raw.extra) {
            if unicode != raw.raw_name.as_slice() {
                visit(&String::from_utf8_lossy(unicode));
            }
        }
        let end = raw
            .header_offset
            .checked_add((LOCAL_LEN + raw.raw_name.len()) as u64)
            .and_then(|end| end.checked_add(raw.compressed_size))
            .ok_or_else(|| EpubError::NotZip("entrada fora do arquivo".into()))?;
        spans.push((raw.header_offset, end, raw.raw_name));
        Ok(())
    })?;
    if trailing {
        return Err(EpubError::NotZip(
            "bytes a mais no fim do diretório central".into(),
        ));
    }
    spans.sort_unstable_by_key(|&(start, end, _)| (start, end));
    let mut previous_end = 0u64;
    for &(start, end, _) in &spans {
        if start < previous_end {
            return Err(EpubError::NotZip("entradas sobrepostas no ZIP".into()));
        }
        if end > directory.offset {
            return Err(EpubError::NotZip(
                "dados da entrada invadem o diretório central".into(),
            ));
        }
        previous_end = end;
    }

    // Os cabeçalhos locais, pela ordem do arquivo: o `tar.exe` (libarchive)
    // extrai cada entrada pelo nome do cabeçalho local, e pelo `0x7075` do
    // extra local. Nunca os dados: uma leitura dos 30 bytes fixos e do nome
    // (do tamanho do central, que cabe no mínimo que a entrada ocupa) e,
    // seguida, outra do extra quando o há; o nome local só se relê quando
    // difere. O total dos nomes e dos extras tem o teto da política.
    let mut local_bytes = 0u64;
    let mut head = Vec::new();
    let mut rest = Vec::new();
    for (start, _, central_name) in &spans {
        head.resize(LOCAL_LEN + central_name.len(), 0);
        source.read_at(*start, &mut head)?;
        if le32(&head, 0) != LOCAL_SIG {
            return Err(EpubError::NotZip("cabeçalho local inválido".into()));
        }
        let name_len = le16(&head, 26) as usize;
        let extra_len = le16(&head, 28) as usize;
        local_bytes += (name_len + extra_len) as u64;
        if local_bytes > policy.max_local_header_size {
            return Err(EpubError::Limit {
                kind: LimitKind::LocalHeaderSize,
                value: local_bytes,
                limit: policy.max_local_header_size,
            });
        }
        let name_at = start + LOCAL_LEN as u64;
        if name_at + (name_len + extra_len) as u64 > directory.offset {
            return Err(EpubError::NotZip(
                "cabeçalho local invade o diretório central".into(),
            ));
        }
        let same = name_len == central_name.len() && head[LOCAL_LEN..] == central_name[..];
        let (rest_at, rest_len) = if same {
            (name_at + name_len as u64, extra_len)
        } else {
            (name_at, name_len + extra_len)
        };
        rest.resize(rest_len, 0);
        if rest_len > 0 {
            source.read_at(rest_at, &mut rest)?;
        }
        let (local_name, local_extra) = if same {
            (central_name.as_slice(), rest.as_slice())
        } else {
            rest.split_at(name_len)
        };
        if !same {
            visit(&String::from_utf8_lossy(local_name));
        }
        for unicode in unicode_path_names(local_extra) {
            if unicode != local_name {
                visit(&String::from_utf8_lossy(unicode));
            }
        }
        if !same {
            return Err(EpubError::NotZip(
                "nome do cabeçalho local difere do diretório central".into(),
            ));
        }
    }
    Ok(directory.entries)
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
    let mut local_name = Vec::new();
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
        local_name.resize(name_len, 0);
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
    use crate::epub::test_support::{self, RawEntry, ZipBuilder};

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
        // Sem teto próprio do diretório central, como antes da BROWSE_LITE.
        assert_eq!(ZipPolicy::EPUB.max_directory_size, u64::MAX);
        // O EPUB escolhe o registro de fim como antes (downloads-zip-inspect
        // so aperta a BROWSE_LITE) e confere os cabecalhos locais pelo
        // caminho dele.
        assert_eq!(
            (
                ZipPolicy::EPUB.unambiguous_directory,
                ZipPolicy::EPUB.max_local_header_size
            ),
            (false, u64::MAX)
        );
        assert_eq!(MAX_ENTRIES, ZipPolicy::EPUB.max_entries);
        assert_eq!(MAX_ENTRY_SIZE, ZipPolicy::EPUB.max_entry_size);
        assert_eq!(MAX_TOTAL_SIZE, ZipPolicy::EPUB.max_total_size);
        assert_eq!(MAX_COMPRESSION_RATIO, ZipPolicy::EPUB.max_compression_ratio);
        assert_eq!(RATIO_CHECK_MIN_SIZE, ZipPolicy::EPUB.ratio_check_min_size);
        assert_eq!(MAX_NAME_LEN, ZipPolicy::EPUB.max_name_len);
        assert_eq!(MAX_EXTRA_LEN, ZipPolicy::EPUB.max_extra_len);
        assert_eq!(MAX_COMMENT_LEN, ZipPolicy::EPUB.max_comment_len);
    }

    /// Um EPUB pequeno cujas entradas têm nome, campo extra e comentário, para
    /// os cortes caírem dentro de cada parte de um registro central.
    fn mutation_seed(zip64: bool) -> Vec<u8> {
        let mut noted = RawEntry::deflated("OEBPS/chapter.xhtml", b"<p>Texto de teste</p>");
        // Um campo extra desconhecido (id 0xFECA) antes do ZIP64, e um comentário.
        noted.extra = vec![0xCA, 0xFE, 4, 0, 1, 2, 3, 4];
        noted.comment = b"comentario".to_vec();
        let builder = ZipBuilder::new()
            .stored("mimetype", b"application/epub+zip")
            .entry(noted)
            .stored("OEBPS/b.xhtml", b"<p>b</p>");
        if zip64 { builder.zip64() } else { builder }.build()
    }

    /// Onde está o diretório central de um ZIP do `ZipBuilder` (sem comentário
    /// no fim): `(entradas, offset, tamanho)`.
    fn directory_of(seed: &[u8], zip64: bool) -> (u64, usize, usize) {
        let eocd = seed.len() - EOCD_LEN;
        assert_eq!(le32(seed, eocd), EOCD_SIG);
        if zip64 {
            let record = eocd - ZIP64_LOCATOR_LEN - ZIP64_EOCD_LEN;
            assert_eq!(le32(seed, record), ZIP64_EOCD_SIG);
            (
                le64(seed, record + 32),
                le64(seed, record + 48) as usize,
                le64(seed, record + 40) as usize,
            )
        } else {
            (
                u64::from(le16(seed, eocd + 10)),
                le32(seed, eocd + 16) as usize,
                le32(seed, eocd + 12) as usize,
            )
        }
    }

    /// O `seed` com o diretório central cortado em `keep` bytes e um fim novo
    /// que declara exatamente esse tamanho (e as mesmas entradas): a busca do
    /// fim e a conferência de que o diretório cabe no arquivo passam, e o
    /// corte chega ao parser do diretório central.
    fn with_directory_cut(seed: &[u8], zip64: bool, keep: usize) -> Vec<u8> {
        let (entries, offset, _) = directory_of(seed, zip64);
        let mut out = seed[..offset + keep].to_vec();
        if zip64 {
            let record = out.len() as u64;
            out.extend_from_slice(&ZIP64_EOCD_SIG.to_le_bytes());
            out.extend_from_slice(&44u64.to_le_bytes());
            out.extend_from_slice(&45u16.to_le_bytes());
            out.extend_from_slice(&45u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&entries.to_le_bytes());
            out.extend_from_slice(&entries.to_le_bytes());
            out.extend_from_slice(&(keep as u64).to_le_bytes());
            out.extend_from_slice(&(offset as u64).to_le_bytes());
            out.extend_from_slice(&ZIP64_LOCATOR_SIG.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&record.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        let (count, size, start) = if zip64 {
            (u16::MAX, u32::MAX, u32::MAX)
        } else {
            (entries as u16, keep as u32, offset as u32)
        };
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn parse_without_panic(bytes: Vec<u8>, what: &str) -> EpubResult<EpubArchive> {
        std::panic::catch_unwind(|| EpubArchive::from_bytes(bytes))
            .unwrap_or_else(|_| panic!("o parser panicou: {what}"))
    }

    /// Cada corte do diretório central (clássico e ZIP64), com um fim que o
    /// declara do tamanho cortado, chega ao parser do diretório central e sai
    /// dele com o erro de truncagem, sem pânico. Sem este fim fabricado os
    /// cortes paravam todos na busca do fim do diretório (ver o teste seguinte)
    /// e o parser nunca era exercido por eles.
    #[test]
    fn central_directory_cuts_reach_the_directory_parser_and_never_panic() {
        for zip64 in [false, true] {
            let seed = mutation_seed(zip64);
            let (entries, _, size) = directory_of(&seed, zip64);
            assert_eq!(entries, 3);
            assert!(size <= 4096, "o diretório central da semente cabe em 4 KiB");
            // Sem corte, o fim fabricado abre o mesmo ZIP: a fabricação está certa.
            let whole = parse_without_panic(with_directory_cut(&seed, zip64, size), "sem corte")
                .expect("o diretório inteiro com o fim fabricado abre");
            assert_eq!(whole.len(), 3);
            for keep in 0..size {
                let cut = with_directory_cut(&seed, zip64, keep);
                let what = format!("zip64={zip64}, diretório cortado em {keep} de {size}");
                match parse_without_panic(cut, &what) {
                    Err(EpubError::NotZip(reason)) if reason == "diretório central truncado" => {}
                    other => {
                        panic!("{what}: esperado o erro do parser do diretório, veio {other:?}")
                    }
                }
            }
        }
    }

    /// Cortes no fim do arquivo param na busca do fim do diretório central
    /// (o registro de fim fica sem os seus 22 bytes): recusados, sem pânico.
    #[test]
    fn tail_cuts_stop_at_the_end_record_search_and_never_panic() {
        let seed = mutation_seed(false);
        assert!(EpubArchive::from_bytes(seed.clone()).is_ok());
        for len in 0..seed.len().min(4096) {
            let what = format!("arquivo cortado em {len}");
            match parse_without_panic(seed[..len].to_vec(), &what) {
                Err(EpubError::NotZip(reason))
                    if reason == "arquivo pequeno demais"
                        || reason == "fim do diretório central não encontrado" => {}
                other => panic!("{what}: veio {other:?}"),
            }
        }
    }

    /// A listagem da BROWSE_LITE (downloads-zip-inspect) recusa pelo motivo
    /// certo: cada falha abaixo é a regra que a nomeia, não outra que
    /// calhou antes. E aceita o que o EPUB recusa (uma entrada cifrada, um
    /// método que não é 0/8, um nome com `..`, tamanhos acima dos tetos do
    /// EPUB), porque não lê dados nem extrai nada.
    #[test]
    fn browse_lite_listing_fails_for_the_named_reason() {
        let list = |bytes: Vec<u8>| {
            let mut names = Vec::new();
            let listed =
                list_central_directory(io::Cursor::new(bytes), &ZipPolicy::BROWSE_LITE, |name| {
                    names.push(name.to_string())
                });
            (listed, names)
        };
        let not_zip = |result: EpubResult<u64>| match result {
            Err(EpubError::NotZip(reason)) => reason,
            other => panic!("esperado NotZip, veio {other:?}"),
        };

        // O que o EPUB recusa e a listagem lê.
        let mut cipher = RawEntry::stored("secreto.txt", b"xx");
        cipher.flags = 1;
        let mut lzma = RawEntry::stored("lzma.bin", b"xx");
        lzma.method = 14;
        lzma.local_method = Some(14);
        let mut huge = RawEntry::stored("grande.bin", b"");
        huge.size = MAX_ENTRY_SIZE + 1;
        huge.compressed = 0;
        let odd = ZipBuilder::new()
            .entry(cipher)
            .entry(lzma)
            .stored("../fora.txt", b"x")
            .entry(huge)
            .build();
        assert!(EpubArchive::from_bytes(odd.clone()).is_err());
        let (listed, names) = list(odd);
        assert_eq!(listed.expect("listado"), 4);
        assert_eq!(
            names,
            ["secreto.txt", "lzma.bin", "../fora.txt", "grande.bin"]
        );

        // Sobrepostas: duas entradas no mesmo cabeçalho local. Os nomes
        // chegam a quem inspeciona antes da falha.
        let mut same = RawEntry::stored("b.txt", b"b");
        same.offset = Some(0);
        same.central_only = true;
        let (listed, names) = list(ZipBuilder::new().stored("a.txt", b"a").entry(same).build());
        assert_eq!(not_zip(listed), "entradas sobrepostas no ZIP");
        assert_eq!(names, ["a.txt", "b.txt"]);
        // Dados que invadem o diretório central.
        let mut invades = RawEntry::stored("a.txt", b"a");
        invades.compressed = 4096;
        let (listed, _) = list(ZipBuilder::new().entry(invades).build());
        assert_eq!(
            not_zip(listed),
            "dados da entrada invadem o diretório central"
        );
        // Bytes a mais no fim do diretório: o fim declara uma entrada a
        // menos do que o diretório tem.
        let mut short = ZipBuilder::new()
            .stored("a.txt", b"a")
            .stored("b.txt", b"b")
            .build();
        let eocd = short.len() - EOCD_LEN;
        short[eocd + 8] = 1;
        short[eocd + 10] = 1;
        let (listed, names) = list(short);
        assert_eq!(not_zip(listed), "bytes a mais no fim do diretório central");
        assert_eq!(names, ["a.txt"]);

        // Um só diretório possível: nenhum nome chega a quem inspeciona.
        for (bytes, reason) in [
            (
                test_support::two_end_records_zip(),
                "mais de um registro de fim do diretório central",
            ),
            (
                test_support::directory_gap_zip(),
                "bytes entre o diretório central e o registro de fim",
            ),
            (
                test_support::classic_disagrees_with_zip64_zip(),
                "registro de fim clássico difere do ZIP64",
            ),
            (
                test_support::zip64_record_gap_zip(),
                "bytes entre o registro de fim ZIP64 e o localizador",
            ),
        ] {
            let (listed, names) = list(bytes.clone());
            assert_eq!(not_zip(listed), reason);
            assert!(names.is_empty(), "{reason}: {names:?}");
            // O EPUB escolhe como antes (só a BROWSE_LITE exige um
            // diretório único): o mesmo arquivo abre, com o LEIAME.txt.
            let epub = EpubArchive::from_bytes(bytes).expect(reason);
            assert_eq!(epub.len(), 1, "{reason}");
        }

        // O cabeçalho local: os nomes do diretório chegam antes; o local
        // chega quando difere; e o 0x7075, central ou local, também.
        let mut renamed = RawEntry::stored("b.txt", b"b");
        renamed.local_name = Some(b"setup.exe".to_vec());
        let (listed, names) = list(
            ZipBuilder::new()
                .stored("a.txt", b"a")
                .entry(renamed)
                .build(),
        );
        assert_eq!(
            not_zip(listed),
            "nome do cabeçalho local difere do diretório central"
        );
        assert_eq!(names, ["a.txt", "b.txt", "setup.exe"]);
        let mut unicode = RawEntry::stored("foto.jpg", b"x");
        unicode.extra = test_support::unicode_path_extra(b"foto.jpg", "setup.exe");
        unicode.local_extra = test_support::unicode_path_extra(b"foto.jpg", "run.bat");
        let (listed, names) = list(ZipBuilder::new().entry(unicode).build());
        assert_eq!(listed.expect("listado"), 1);
        assert_eq!(names, ["foto.jpg", "setup.exe", "run.bat"]);
        let mut no_local = ZipBuilder::new().stored("a.txt", b"a").build();
        no_local[0] ^= 0xFF;
        let (listed, names) = list(no_local);
        assert_eq!(not_zip(listed), "cabeçalho local inválido");
        assert_eq!(names, ["a.txt"]);
        let mut long_extra = ZipBuilder::new().stored("a.txt", b"a").build();
        long_extra[28..30].copy_from_slice(&[0xFF, 0xFF]);
        let (listed, _) = list(long_extra);
        assert_eq!(
            not_zip(listed),
            "cabeçalho local invade o diretório central"
        );
        // O teto dos cabeçalhos locais (nomes e extras somados).
        let tight = ZipPolicy {
            max_local_header_size: 9,
            ..ZipPolicy::BROWSE_LITE
        };
        let two = ZipBuilder::new()
            .stored("a.txt", b"a")
            .stored("b.txt", b"b")
            .build();
        let listed = list_central_directory(io::Cursor::new(two.clone()), &tight, |_| {});
        assert!(
            matches!(
                listed,
                Err(EpubError::Limit {
                    kind: LimitKind::LocalHeaderSize,
                    value: 10,
                    limit: 9
                })
            ),
            "{listed:?}"
        );
        let roomy = ZipPolicy {
            max_local_header_size: 10,
            ..ZipPolicy::BROWSE_LITE
        };
        assert_eq!(
            list_central_directory(io::Cursor::new(two), &roomy, |_| {}).expect("listado"),
            2
        );

        // Os tetos da política, antes de ler o diretório.
        let seed = ZipBuilder::new().stored("a.txt", b"a").zip64().build();
        let record = seed.len() - EOCD_LEN - ZIP64_LOCATOR_LEN - ZIP64_EOCD_LEN;
        let mut many = seed.clone();
        let over = ZipPolicy::BROWSE_LITE.max_entries + 1;
        many[record + 24..record + 32].copy_from_slice(&over.to_le_bytes());
        many[record + 32..record + 40].copy_from_slice(&over.to_le_bytes());
        let (listed, names) = list(many);
        assert!(
            matches!(
                listed,
                Err(EpubError::Limit {
                    kind: LimitKind::Entries,
                    ..
                })
            ),
            "{listed:?}"
        );
        assert!(names.is_empty());
        let mut big = seed.clone();
        let size = ZipPolicy::BROWSE_LITE.max_directory_size + 1;
        big[record + 40..record + 48].copy_from_slice(&size.to_le_bytes());
        let (listed, _) = list(big);
        assert!(
            matches!(
                listed,
                Err(EpubError::Limit {
                    kind: LimitKind::DirectorySize,
                    ..
                })
            ),
            "{listed:?}"
        );
        // O EPUB não tem teto próprio do diretório: o mesmo diretório
        // declarado passa dos tetos e cai na conferência de que cabe.
        let mut declared = seed;
        declared[record + 40..record + 48].copy_from_slice(&size.to_le_bytes());
        assert!(matches!(
            EpubArchive::from_bytes(declared),
            Err(EpubError::NotZip(_))
        ));
    }

    /// 257 bit flips no arquivo inteiro e 257 dentro do diretório central, nas
    /// duas formas (clássica e ZIP64): nenhum pânico.
    #[test]
    fn central_directory_bit_flips_never_panic() {
        for zip64 in [false, true] {
            let seed = mutation_seed(zip64);
            assert!(EpubArchive::from_bytes(seed.clone()).is_ok());
            let (_, offset, size) = directory_of(&seed, zip64);
            for flip in 0..257usize {
                for (region, at) in [
                    ("arquivo", flip.wrapping_mul(97) % seed.len()),
                    ("diretório", offset + flip.wrapping_mul(31) % size),
                ] {
                    let mut bytes = seed.clone();
                    bytes[at] ^= 1 << (flip % 8);
                    let what = format!("zip64={zip64}, bit flip {flip} no {region} (byte {at})");
                    let _ = parse_without_panic(bytes, &what);
                }
            }
        }
    }
}
