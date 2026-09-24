//! Leitura de livros EPUB 2 e 3 (sem MOBI/AZW e sem DRM).
//!
//! Duas camadas, ambas sem UI:
//!
//! - [`EpubArchive`]: um leitor de ZIP escrito para conteúdo hostil. Lê só o
//!   diretório central, recusa o que um EPUB legítimo nunca tem (nomes com
//!   `..`, caminhos absolutos, entradas criptografadas, métodos que não são
//!   *stored*/*deflate*) e impõe limites antes de descompactar um byte:
//!   [`MAX_ENTRIES`], [`MAX_ENTRY_SIZE`], [`MAX_TOTAL_SIZE`] e
//!   [`MAX_COMPRESSION_RATIO`]. A leitura é em fluxo e nunca produz mais do que
//!   o tamanho declarado da entrada; o CRC-32 é conferido no fim.
//! - [`EpubBook`]: `META-INF/container.xml` → OPF (metadados, manifest, spine),
//!   capa, sumário (nav do EPUB 3 ou NCX do EPUB 2) e deteção de DRM. O XML
//!   passa por `quick-xml` sem expansão de entidades: um `<!DOCTYPE>` que
//!   declara entidades é recusado ([`EpubError::UnsafeXml`]), e referências
//!   desconhecidas ficam como texto literal.
//!
//! Nada aqui escreve no disco nem acessa a rede; a biblioteca em
//! [`crate::library`] é quem copia livros e capas para a pasta do usuário.

mod archive;
mod book;
mod toc;
mod xml;

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::{fmt, io};

use thiserror::Error;

pub use archive::{
    Compression, EntryReader, EpubArchive, MAX_COMMENT_LEN, MAX_COMPRESSION_RATIO, MAX_ENTRIES,
    MAX_ENTRY_SIZE, MAX_EXTRA_LEN, MAX_NAME_LEN, MAX_TOTAL_SIZE, RATIO_CHECK_MIN_SIZE, ZipEntry,
    normalize_entry_name, resolve_href,
};
pub use book::{
    ADOBE_FONT_OBFUSCATION, CoverImage, Creator, EpubBook, EpubMetadata, IDPF_FONT_OBFUSCATION,
    MAX_ATTR_CHARS, MAX_MEDIA_TYPE_CHARS, MAX_SPINE_ITEMS, MAX_WARNINGS, ManifestItem,
    PageProgression, SpineItem,
};
pub use toc::{MAX_TOC_DEPTH, MAX_TOC_ENTRIES, TocEntry};
pub use xml::{MAX_XML_BYTES, MAX_XML_DEPTH, MAX_XML_NODES};

/// Qual limite de segurança um arquivo ultrapassou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Mais de [`MAX_ENTRIES`] entradas no diretório central.
    Entries,
    /// Uma entrada declara mais de [`MAX_ENTRY_SIZE`] bytes (ou mais do que o
    /// teto pedido a [`EpubArchive::read_capped`]).
    EntrySize,
    /// A soma dos tamanhos descompactados passa de [`MAX_TOTAL_SIZE`].
    TotalSize,
    /// Razão de compressão acima de [`MAX_COMPRESSION_RATIO`]:1.
    CompressionRatio,
    /// Nome de entrada com mais de [`MAX_NAME_LEN`] bytes.
    NameLength,
    /// Campo *extra* com mais de [`MAX_EXTRA_LEN`] bytes.
    ExtraLength,
    /// Comentário de entrada com mais de [`MAX_COMMENT_LEN`] bytes.
    CommentLength,
    /// Documento XML com mais de [`MAX_XML_BYTES`] bytes.
    XmlSize,
    /// Documento XML com mais de [`MAX_XML_NODES`] elementos.
    XmlNodes,
}

impl fmt::Display for LimitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LimitKind::Entries => "número de entradas",
            LimitKind::EntrySize => "tamanho de uma entrada",
            LimitKind::TotalSize => "tamanho total descompactado",
            LimitKind::CompressionRatio => "razão de compressão",
            LimitKind::NameLength => "tamanho do nome",
            LimitKind::ExtraLength => "tamanho do campo extra",
            LimitKind::CommentLength => "tamanho do comentário",
            LimitKind::XmlSize => "tamanho do XML",
            LimitKind::XmlNodes => "elementos no XML",
        })
    }
}

#[derive(Debug, Error)]
pub enum EpubError {
    #[error("falha de E/S: {0}")]
    Io(#[from] io::Error),
    /// Não há um diretório central ZIP legível.
    #[error("o arquivo não é um ZIP válido: {0}")]
    NotZip(String),
    /// ZIP multivolume ou outra estrutura que um EPUB nunca usa.
    #[error("ZIP não suportado: {0}")]
    Unsupported(String),
    #[error("limite excedido ({kind}): {value} > {limit}")]
    Limit {
        kind: LimitKind,
        value: u64,
        limit: u64,
    },
    /// Nome com `..`, absoluto, com letra de unidade, `\`, NUL ou caractere de controle.
    #[error("nome de entrada inseguro no ZIP: {0:?}")]
    UnsafeName(String),
    /// Entrada com criptografia do próprio ZIP (não confundir com DRM).
    #[error("entrada criptografada no ZIP: {0:?}")]
    Encrypted(String),
    #[error("método de compressão {method} não suportado em {name:?}")]
    UnsupportedCompression { name: String, method: u16 },
    /// Dados que não batem com o diretório central: CRC, tamanho, cabeçalho
    /// local, entradas sobrepostas.
    #[error("entrada {name:?} corrompida: {reason}")]
    Corrupt { name: String, reason: String },
    #[error("entrada não encontrada no EPUB: {0}")]
    NotFound(String),
    /// O ZIP abriu mas não tem a estrutura de um EPUB.
    #[error("não é um EPUB válido: {0}")]
    NotEpub(String),
    #[error("XML inválido em {path}: {message}")]
    Xml { path: String, message: String },
    /// DOCTYPE com declarações de entidade (bomba de entidades, XXE).
    #[error("XML recusado em {path}: {reason}")]
    UnsafeXml { path: String, reason: String },
    /// `META-INF/rights.xml`, ou `encryption.xml` cifrando algo que não é
    /// uma fonte ofuscada.
    #[error("livro protegido por DRM ({0}); o NeuralIA não abre livros com DRM")]
    Drm(String),
}

pub type EpubResult<T> = std::result::Result<T, EpubError>;
