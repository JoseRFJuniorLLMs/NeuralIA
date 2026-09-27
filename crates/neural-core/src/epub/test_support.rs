//! Só para testes: um escritor de ZIP mínimo (stored + deflate, ZIP64
//! opcional, campos falsificáveis) e EPUBs de brinquedo montados na hora.
//! Nenhum livro real entra no repositório.

use std::io::Write;

use flate2::{Compression as Level, Crc, write::DeflateEncoder};

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Level::best());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Uma entrada com todos os campos à mão, para fabricar ZIPs inválidos.
#[derive(Clone, Debug)]
pub(crate) struct RawEntry {
    pub name: Vec<u8>,
    /// Nome no cabeçalho local, se diferente do central.
    pub local_name: Option<Vec<u8>>,
    /// Bytes gravados depois do cabeçalho local (já comprimidos, se for o caso).
    pub payload: Vec<u8>,
    pub method: u16,
    pub local_method: Option<u16>,
    pub flags: u16,
    pub crc: u32,
    pub size: u64,
    pub compressed: u64,
    pub extra: Vec<u8>,
    /// Campo extra do cabeçalho local, antes do ZIP64 dele.
    pub local_extra: Vec<u8>,
    pub comment: Vec<u8>,
    /// Offset do cabeçalho local escrito no diretório central (falsificado).
    pub offset: Option<u64>,
    /// Só no diretório central (o cabeçalho local está noutro lugar).
    pub central_only: bool,
}

impl RawEntry {
    pub fn stored(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            local_name: None,
            payload: data.to_vec(),
            method: 0,
            local_method: None,
            flags: 0,
            crc: crc32(data),
            size: data.len() as u64,
            compressed: data.len() as u64,
            extra: Vec::new(),
            local_extra: Vec::new(),
            comment: Vec::new(),
            offset: None,
            central_only: false,
        }
    }

    pub fn deflated(name: &str, data: &[u8]) -> Self {
        let payload = deflate(data);
        Self {
            compressed: payload.len() as u64,
            payload,
            method: 8,
            ..Self::stored(name, data)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ZipBuilder {
    pub entries: Vec<RawEntry>,
    pub zip64: bool,
}

impl ZipBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stored(mut self, name: &str, data: &[u8]) -> Self {
        self.entries.push(RawEntry::stored(name, data));
        self
    }

    pub fn deflated(mut self, name: &str, data: &[u8]) -> Self {
        self.entries.push(RawEntry::deflated(name, data));
        self
    }

    pub fn entry(mut self, entry: RawEntry) -> Self {
        self.entries.push(entry);
        self
    }

    pub fn zip64(mut self) -> Self {
        self.zip64 = true;
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for entry in &self.entries {
            let offset = out.len() as u64;
            if !entry.central_only {
                out.extend_from_slice(&local_record(entry, self.zip64));
            }

            let header_offset = entry.offset.unwrap_or(offset);
            let mut extra = entry.extra.clone();
            if self.zip64 {
                push16(&mut extra, 1);
                push16(&mut extra, 24);
                push64(&mut extra, entry.size);
                push64(&mut extra, entry.compressed);
                push64(&mut extra, header_offset);
            }
            push32(&mut central, 0x0201_4b50);
            push16(&mut central, 0x031E);
            push16(&mut central, if self.zip64 { 45 } else { 20 });
            push16(&mut central, entry.flags);
            push16(&mut central, entry.method);
            push16(&mut central, 0);
            push16(&mut central, 0x21);
            push32(&mut central, entry.crc);
            push32(&mut central, sized(self.zip64, entry.compressed));
            push32(&mut central, sized(self.zip64, entry.size));
            push16(&mut central, entry.name.len() as u16);
            push16(&mut central, extra.len() as u16);
            push16(&mut central, entry.comment.len() as u16);
            push16(&mut central, 0);
            push16(&mut central, 0);
            push32(&mut central, 0);
            push32(&mut central, sized(self.zip64, header_offset));
            central.extend_from_slice(&entry.name);
            central.extend_from_slice(&extra);
            central.extend_from_slice(&entry.comment);
        }
        let directory_offset = out.len() as u64;
        let directory_size = central.len() as u64;
        out.extend_from_slice(&central);
        let count = self.entries.len() as u64;
        if self.zip64 {
            let record_offset = out.len() as u64;
            push32(&mut out, 0x0606_4b50);
            push64(&mut out, 44);
            push16(&mut out, 45);
            push16(&mut out, 45);
            push32(&mut out, 0);
            push32(&mut out, 0);
            push64(&mut out, count);
            push64(&mut out, count);
            push64(&mut out, directory_size);
            push64(&mut out, directory_offset);
            push32(&mut out, 0x0706_4b50);
            push32(&mut out, 0);
            push64(&mut out, record_offset);
            push32(&mut out, 1);
        }
        push32(&mut out, 0x0605_4b50);
        push16(&mut out, 0);
        push16(&mut out, 0);
        let short_count = if self.zip64 { 0xFFFF } else { count as u16 };
        push16(&mut out, short_count);
        push16(&mut out, short_count);
        push32(&mut out, sized(self.zip64, directory_size));
        push32(&mut out, sized(self.zip64, directory_offset));
        push16(&mut out, 0);
        out
    }
}

/// Cabeçalho local + nome + extra + dados de uma entrada.
fn local_record(entry: &RawEntry, zip64: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let local_name = entry.local_name.as_ref().unwrap_or(&entry.name);
    let mut local_extra = entry.local_extra.clone();
    if zip64 {
        push16(&mut local_extra, 1);
        push16(&mut local_extra, 16);
        push64(&mut local_extra, entry.size);
        push64(&mut local_extra, entry.compressed);
    }
    push32(&mut out, 0x0403_4b50);
    push16(&mut out, if zip64 { 45 } else { 20 });
    push16(&mut out, entry.flags);
    push16(&mut out, entry.local_method.unwrap_or(entry.method));
    push16(&mut out, 0);
    push16(&mut out, 0x21);
    push32(&mut out, entry.crc);
    push32(&mut out, sized(zip64, entry.compressed));
    push32(&mut out, sized(zip64, entry.size));
    push16(&mut out, local_name.len() as u16);
    push16(&mut out, local_extra.len() as u16);
    out.extend_from_slice(local_name);
    out.extend_from_slice(&local_extra);
    out.extend_from_slice(&entry.payload);
    out
}

/// Um registro local completo (sem compressão), para embutir dentro dos
/// dados de outra entrada.
pub(crate) fn local_header(name: &str, data: &[u8]) -> Vec<u8> {
    local_record(&RawEntry::stored(name, data), false)
}

/// Um campo extra Info-ZIP Unicode Path (`0x7075`): versão 1, o CRC32 do
/// nome cru e o nome em UTF-8 -- o nome com que o 7-Zip e o `tar.exe`
/// extraem a entrada.
pub(crate) fn unicode_path_extra(raw: &[u8], name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    push16(&mut out, 0x7075);
    push16(&mut out, (5 + name.len()) as u16);
    out.push(1);
    push32(&mut out, crc32(raw));
    out.extend_from_slice(name.as_bytes());
    out
}

/// Um registro de fim clássico.
fn end_record(entries: u16, size: usize, offset: usize, comment_len: u16) -> Vec<u8> {
    let mut out = Vec::new();
    push32(&mut out, 0x0605_4b50);
    push16(&mut out, 0);
    push16(&mut out, 0);
    push16(&mut out, entries);
    push16(&mut out, entries);
    push32(&mut out, size as u32);
    push32(&mut out, offset as u32);
    push16(&mut out, comment_len);
    out
}

/// Um registro de fim ZIP64 (44 bytes depois dos 12 do começo).
fn zip64_end_record(entries: u64, size: usize, offset: usize) -> Vec<u8> {
    let mut out = Vec::new();
    push32(&mut out, 0x0606_4b50);
    push64(&mut out, 44);
    push16(&mut out, 45);
    push16(&mut out, 45);
    push32(&mut out, 0);
    push32(&mut out, 0);
    push64(&mut out, entries);
    push64(&mut out, entries);
    push64(&mut out, size as u64);
    push64(&mut out, offset as u64);
    out
}

/// O localizador ZIP64, a apontar para `record`.
fn zip64_locator(record: usize) -> Vec<u8> {
    let mut out = Vec::new();
    push32(&mut out, 0x0706_4b50);
    push32(&mut out, 0);
    push64(&mut out, record as u64);
    push32(&mut out, 1);
    out
}

/// A base dos ZIPs de diretório ambíguo (downloads-zip-inspect): o
/// `LEIAME.txt` e um `setup.exe`, e onde está o diretório central inteiro
/// e o tamanho do registro central do `LEIAME.txt` (o primeiro).
fn ambiguous_base() -> (Vec<u8>, usize, usize, usize) {
    let zip = ZipBuilder::new()
        .stored("LEIAME.txt", b"ola")
        .stored("setup.exe", b"MZ\x90\x00")
        .build();
    let eocd = zip.len() - 22;
    let le16 = |at: usize| u16::from_le_bytes([zip[at], zip[at + 1]]) as usize;
    let le32 =
        |at: usize| u32::from_le_bytes([zip[at], zip[at + 1], zip[at + 2], zip[at + 3]]) as usize;
    let (size, offset) = (le32(eocd + 12), le32(eocd + 16));
    let first = 46 + le16(offset + 28) + le16(offset + 30) + le16(offset + 32);
    (zip, offset, size, first)
}

/// Dois registros de fim: o que tem o comentário até ao fim do arquivo
/// aponta para um diretório só com o `LEIAME.txt`; dentro do comentário
/// dele está outro, a última assinatura do arquivo (acaba um byte antes do
/// fim), que aponta para o diretório com o `setup.exe` -- o que o 7-Zip, o
/// .NET e o bsdtar listam.
pub(crate) fn two_end_records_zip() -> Vec<u8> {
    let (zip, offset, size, first) = ambiguous_base();
    let mut out = zip[..offset + size].to_vec();
    let benign = out.len();
    out.extend_from_slice(&zip[offset..offset + first]);
    let hidden = end_record(2, size, offset, 0);
    out.extend_from_slice(&end_record(1, first, benign, (hidden.len() + 1) as u16));
    out.extend_from_slice(&hidden);
    out.push(0);
    out
}

/// O registro de fim declara uma entrada e o tamanho do registro central
/// do `LEIAME.txt`; o do `setup.exe` fica entre o fim declarado do
/// diretório e o registro de fim (o 7-Zip lista-o e extrai-o).
pub(crate) fn directory_gap_zip() -> Vec<u8> {
    let (mut zip, _, _, first) = ambiguous_base();
    let eocd = zip.len() - 22;
    zip[eocd + 8..eocd + 12].copy_from_slice(&[1, 0, 1, 0]);
    zip[eocd + 12..eocd + 16].copy_from_slice(&(first as u32).to_le_bytes());
    zip
}

/// O registro ZIP64 aponta para um diretório só com o `LEIAME.txt`; o
/// registro de fim clássico, sem nenhum campo saturado, aponta para o
/// diretório com o `setup.exe` (o que o .NET usa quando nada satura).
pub(crate) fn classic_disagrees_with_zip64_zip() -> Vec<u8> {
    let (zip, offset, size, first) = ambiguous_base();
    let mut out = zip[..offset + size].to_vec();
    let benign = out.len();
    out.extend_from_slice(&zip[offset..offset + first]);
    let record = out.len();
    out.extend_from_slice(&zip64_end_record(1, first, benign));
    out.extend_from_slice(&zip64_locator(record));
    out.extend_from_slice(&end_record(2, size, offset, 0));
    out
}

/// O localizador aponta para o registro ZIP64 de um diretório só com o
/// `LEIAME.txt`; entre esse registro e o localizador está outro registro
/// ZIP64, o do diretório com o `setup.exe` -- o 7-Zip lê primeiro o
/// registro colado ao localizador.
pub(crate) fn zip64_record_gap_zip() -> Vec<u8> {
    let (zip, offset, size, first) = ambiguous_base();
    let mut out = zip[..offset + size].to_vec();
    let benign = out.len();
    out.extend_from_slice(&zip[offset..offset + first]);
    let record = out.len();
    out.extend_from_slice(&zip64_end_record(1, first, benign));
    out.extend_from_slice(&zip64_end_record(2, size, offset));
    out.extend_from_slice(&zip64_locator(record));
    let mut end = end_record(0xFFFF, 0, 0, 0);
    end[12..20].copy_from_slice(&[0xFF; 8]);
    out.extend_from_slice(&end);
    out
}

fn sized(zip64: bool, value: u64) -> u32 {
    if zip64 { 0xFFFF_FFFF } else { value as u32 }
}

fn push16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

/// Bytes de um PNG 1x1 (para o sniffing da biblioteca só a assinatura conta).
pub(crate) const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// Cabeçalho JPEG mínimo (o suficiente para o sniffing).
pub(crate) const JPEG_STUB: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0xFF, 0xD9,
];

pub(crate) fn chapter(title: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>{title}</title></head>
<body><h1 id="top">{title}</h1>{body}</body>
</html>"#
    )
}

/// EPUB com `mimetype` (stored, primeiro) e os arquivos dados (deflate).
pub(crate) fn epub_with(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipBuilder::new().stored("mimetype", b"application/epub+zip");
    for (name, data) in files {
        zip = zip.deflated(name, data);
    }
    zip.build()
}

pub(crate) const EPUB3_OPF: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id" dir="ltr">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="isbn">urn:isbn:0000000000</dc:identifier>
    <dc:identifier id="pub-id">urn:uuid:12345678-1234-1234-1234-123456789abc</dc:identifier>
    <dc:title id="sub">Um subtítulo</dc:title>
    <dc:title id="main">O Livro de Teste &amp; Companhia</dc:title>
    <meta refines="#main" property="title-type">main</meta>
    <meta refines="#sub" property="title-type">subtitle</meta>
    <dc:creator id="c1">Maria Autora</dc:creator>
    <meta refines="#c1" property="role" scheme="marc:relators">aut</meta>
    <meta refines="#c1" property="file-as">Autora, Maria</meta>
    <dc:creator id="c2">João Tradutor</dc:creator>
    <meta refines="#c2" property="role" scheme="marc:relators">trl</meta>
    <dc:contributor>Editor Convidado</dc:contributor>
    <dc:language>pt-BR</dc:language>
    <dc:publisher>Editora Exemplo</dc:publisher>
    <dc:description>  Uma descrição
      em várias linhas.  </dc:description>
    <dc:date>2024-05-01</dc:date>
    <dc:subject>Ficção</dc:subject>
    <dc:subject>Testes</dc:subject>
    <meta property="belongs-to-collection" id="col">Série Exemplo</meta>
    <meta refines="#col" property="collection-type">series</meta>
    <meta refines="#col" property="group-position">2</meta>
    <meta property="dcterms:modified">2024-05-01T00:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="Nav/nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="cover-img" href="Images/capa.png" media-type="image/png" properties="cover-image"/>
    <item id="c1x" href="Text/cap%C3%ADtulo%201.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2x" href="Text/ch2.xhtml" media-type="application/xhtml+xml" properties="svg scripted"/>
    <item id="notes" href="Text/notes.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="Styles/style.css" media-type="text/css"/>
  </manifest>
  <spine page-progression-direction="rtl">
    <itemref idref="c1x"/>
    <itemref idref="c2x" properties="page-spread-left"/>
    <itemref idref="notes" linear="no"/>
    <itemref idref="missing"/>
  </spine>
</package>"##;

pub(crate) const EPUB3_NAV: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Sumário</title></head>
<body>
  <nav epub:type="landmarks"><ol><li><a href="../Text/ch2.xhtml">Não é o sumário</a></li></ol></nav>
  <nav epub:type="toc" id="toc">
    <h1>Sumário</h1>
    <ol>
      <li><a href="../Text/cap%C3%ADtulo%201.xhtml">Capítulo <em>1</em></a></li>
      <li><span>Parte II</span>
        <ol>
          <li><a href="../Text/ch2.xhtml#sec1">Seção 2.1</a>
            <ol>
              <li><a href="../Text/ch2.xhtml#sec1-1">Seção&nbsp;2.1.1 &mdash; fim</a></li>
            </ol>
          </li>
          <li><a href="../Text/CH2.xhtml#sec2">Seção 2.2</a></li>
        </ol>
      </li>
      <li><a href="../Text/notes.xhtml">Notas</a></li>
    </ol>
  </nav>
</body>
</html>"##;

/// EPUB 3 completo: metadados refinados, nav aninhado, capa por
/// `cover-image`, nomes com acento codificados em `%XX`.
pub(crate) fn epub3_sample() -> Vec<u8> {
    let ch1 = chapter("Capítulo 1", "<p>Olá.</p>");
    let ch2 = chapter(
        "Capítulo 2",
        r#"<h2 id="sec1">2.1</h2><h3 id="sec1-1">2.1.1</h3><h2 id="sec2">2.2</h2>"#,
    );
    let notes = chapter("Notas", "<p>Nota.</p>");
    epub_with(&[
        ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
        ("OEBPS/content.opf", EPUB3_OPF.as_bytes()),
        ("OEBPS/Nav/nav.xhtml", EPUB3_NAV.as_bytes()),
        ("OEBPS/Images/capa.png", PNG_1X1),
        ("OEBPS/Text/capítulo 1.xhtml", ch1.as_bytes()),
        ("OEBPS/Text/ch2.xhtml", ch2.as_bytes()),
        ("OEBPS/Text/notes.xhtml", notes.as_bytes()),
        ("OEBPS/Styles/style.css", b"body { margin: 0 }"),
    ])
}

pub(crate) const EPUB2_OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="BookId">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Livro Dois</dc:title>
    <dc:creator opf:role="aut" opf:file-as="Silva, José">José Silva</dc:creator>
    <dc:creator opf:role="ill">Ilustra Dora</dc:creator>
    <dc:language>pt</dc:language>
    <dc:identifier id="BookId" opf:scheme="UUID">urn:uuid:aaaa</dc:identifier>
    <dc:date opf:event="modification">2020-01-01</dc:date>
    <dc:date opf:event="publication">1999</dc:date>
    <meta name="cover" content="capa"/>
    <meta name="calibre:series" content="Coleção Dois"/>
    <meta name="calibre:series_index" content="3.5"/>
  </metadata>
  <manifest>
    <item id="ncx" href="meta/toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="capa" href="images/cover.jpg" media-type="image/jpeg"/>
    <item id="p1" href="text/part1.html" media-type="application/xhtml+xml"/>
    <item id="p2" href="text/part2.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="p1"/>
    <itemref idref="p2"/>
  </spine>
</package>"#;

pub(crate) const EPUB2_NCX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head><meta name="dtb:uid" content="urn:uuid:aaaa"/></head>
  <docTitle><text>Livro Dois</text></docTitle>
  <navMap>
    <navPoint id="n1" playOrder="1">
      <navLabel><text>Parte 1</text></navLabel>
      <content src="../text/part1.html"/>
      <navPoint id="n1a" playOrder="2">
        <navLabel><text>Parte 1 &#8212; A</text></navLabel>
        <content src="../text/part1.html#a"/>
        <navPoint id="n1a1" playOrder="3">
          <navLabel><text>Fundo</text></navLabel>
          <content src="../text/part1.html#a1"/>
        </navPoint>
      </navPoint>
    </navPoint>
    <navPoint id="n2" playOrder="4">
      <navLabel><text>Parte 2</text></navLabel>
      <content src="../text/part2.html#inicio"/>
    </navPoint>
  </navMap>
</ncx>"#;

/// EPUB 2: `opf:role`, `<meta name="cover">`, NCX aninhado com DOCTYPE público.
pub(crate) fn epub2_sample() -> Vec<u8> {
    let p1 = chapter("Parte 1", r#"<p id="a">A</p><p id="a1">A1</p>"#);
    let p2 = chapter("Parte 2", r#"<p id="inicio">B</p>"#);
    epub_with(&[
        ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
        ("OEBPS/content.opf", EPUB2_OPF.as_bytes()),
        ("OEBPS/meta/toc.ncx", EPUB2_NCX.as_bytes()),
        ("OEBPS/images/cover.jpg", JPEG_STUB),
        ("OEBPS/text/part1.html", p1.as_bytes()),
        ("OEBPS/text/part2.html", p2.as_bytes()),
    ])
}

/// OPF mínimo com um manifest e um spine dados (para variar só uma coisa).
pub(crate) fn opf(metadata: &str, manifest: &str, spine: &str, extra: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title>{metadata}</metadata>
  <manifest>{manifest}</manifest>
  <spine>{spine}</spine>
  {extra}
</package>"#
    )
}
