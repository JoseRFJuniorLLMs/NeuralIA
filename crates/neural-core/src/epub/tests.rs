//! Gates do leitor de EPUB. Todos os livros são montados aqui mesmo.

use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::test_support::*;
use super::*;

static DIR_NONCE: AtomicU64 = AtomicU64::new(1);

struct TempFile(PathBuf);

impl TempFile {
    fn with(bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "neuralia-epub-{}-{}.epub",
            std::process::id(),
            DIR_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn archive(bytes: Vec<u8>) -> EpubArchive {
    EpubArchive::from_bytes(bytes).expect("o ZIP de teste abre")
}

fn parse(bytes: Vec<u8>) -> EpubResult<EpubBook> {
    EpubBook::parse(&EpubArchive::from_bytes(bytes)?)
}

fn limit_kind<T: std::fmt::Debug>(result: EpubResult<T>) -> LimitKind {
    match result {
        Err(EpubError::Limit { kind, .. }) => kind,
        other => panic!("esperava EpubError::Limit, veio {other:?}"),
    }
}

fn names(archive: &EpubArchive) -> Vec<&str> {
    archive.entries().iter().map(ZipEntry::name).collect()
}

/// Um EPUB 3 mínimo com o OPF dado (container padrão, `p1.xhtml` no spine).
fn book_with_opf(opf: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let page = chapter("P1", "<p>x</p>");
    let mut files: Vec<(&str, &[u8])> = vec![
        ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
        ("OEBPS/content.opf", opf.as_bytes()),
        ("OEBPS/p1.xhtml", page.as_bytes()),
    ];
    files.extend_from_slice(extra);
    epub_with(&files)
}

const P1_ITEM: &str = r#"<item id="p1" href="p1.xhtml" media-type="application/xhtml+xml"/>"#;
const P1_SPINE: &str = r#"<itemref idref="p1"/>"#;

// ---------------------------------------------------------------------------
// ZIP
// ---------------------------------------------------------------------------

#[test]
fn stored_and_deflated_entries_round_trip_from_memory_and_from_disk() {
    let text = "Olá, EPUB! ".repeat(500);
    let binary: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
    let zip = ZipBuilder::new()
        .stored("mimetype", b"application/epub+zip")
        .stored("OEBPS/", b"")
        .deflated("OEBPS/texto.xhtml", text.as_bytes())
        .stored("OEBPS/img.bin", &binary)
        .deflated("vazio.txt", b"")
        .build();
    let file = TempFile::with(&zip);
    for archive in [archive(zip), EpubArchive::open(&file.0).unwrap()] {
        assert_eq!(
            names(&archive),
            [
                "mimetype",
                "OEBPS/texto.xhtml",
                "OEBPS/img.bin",
                "vazio.txt"
            ]
        );
        assert_eq!(archive.read("OEBPS/texto.xhtml").unwrap(), text.as_bytes());
        assert_eq!(archive.read("OEBPS/img.bin").unwrap(), binary);
        assert_eq!(archive.read("vazio.txt").unwrap(), b"");
        let entry = archive.entry("OEBPS/texto.xhtml").unwrap();
        assert_eq!(entry.size(), text.len() as u64);
        assert_eq!(entry.compression(), Compression::Deflate);
        assert_eq!(
            archive.entry("OEBPS/img.bin").unwrap().compression(),
            Compression::Stored
        );

        // Em fluxo, de 7 em 7 bytes, dá o mesmo.
        let mut reader = archive.open_entry("OEBPS/texto.xhtml").unwrap();
        let mut streamed = Vec::new();
        let mut chunk = [0u8; 7];
        loop {
            let n = reader.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            streamed.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(streamed, text.as_bytes());
        assert!(matches!(
            archive.read("nao-existe.txt"),
            Err(EpubError::NotFound(_))
        ));
    }
}

#[test]
fn zip64_records_are_read() {
    let body = "zip64 ".repeat(300);
    let zip = ZipBuilder::new()
        .zip64()
        .stored("mimetype", b"application/epub+zip")
        .deflated("OEBPS/a.xhtml", body.as_bytes())
        .build();
    let archive = archive(zip);
    assert_eq!(names(&archive), ["mimetype", "OEBPS/a.xhtml"]);
    assert_eq!(archive.read("OEBPS/a.xhtml").unwrap(), body.as_bytes());
    assert_eq!(archive.read("mimetype").unwrap(), b"application/epub+zip");
}

#[test]
fn unsafe_entry_names_are_rejected() {
    for name in [
        "../evil.txt",
        "OEBPS/../../evil.txt",
        "..",
        "/etc/passwd",
        "C:/Windows/win.ini",
        "c:evil",
        "OEBPS\\Text\\ch1.xhtml",
        "a\0b",
        "a\u{1}b",
        "",
    ] {
        let zip = ZipBuilder::new()
            .stored("mimetype", b"application/epub+zip")
            .stored(name, b"x")
            .build();
        match EpubArchive::from_bytes(zip) {
            Err(EpubError::UnsafeName(_)) => {}
            other => panic!("{name:?} devia ser recusado, veio {other:?}"),
        }
    }
    assert_eq!(
        normalize_entry_name("./OEBPS//Text/./a.xhtml").as_deref(),
        Some("OEBPS/Text/a.xhtml")
    );
    assert_eq!(normalize_entry_name("OEBPS/x/").as_deref(), Some("OEBPS/x"));
    assert_eq!(
        normalize_entry_name("OEBPS/a..b.xhtml").as_deref(),
        Some("OEBPS/a..b.xhtml")
    );
}

/// `(base, href, (caminho, fragmento) esperados)`.
type HrefCase<'a> = (&'a str, &'a str, Option<(&'a str, Option<&'a str>)>);

#[test]
fn hrefs_resolve_relative_to_their_document_and_never_escape_the_root() {
    let cases: [HrefCase; 14] = [
        (
            "OEBPS/content.opf",
            "Text/ch1.xhtml",
            Some(("OEBPS/Text/ch1.xhtml", None)),
        ),
        (
            "OEBPS/Text/ch1.xhtml",
            "../Images/a%20b.png#frag",
            Some(("OEBPS/Images/a b.png", Some("frag"))),
        ),
        (
            "OEBPS/Text/ch1.xhtml",
            "#nota-3",
            Some(("OEBPS/Text/ch1.xhtml", Some("nota-3"))),
        ),
        ("content.opf", "ch1.xhtml", Some(("ch1.xhtml", None))),
        (
            "OEBPS/content.opf",
            "cap%C3%ADtulo.xhtml",
            Some(("OEBPS/capítulo.xhtml", None)),
        ),
        (
            "OEBPS/content.opf",
            "Text\\ch1.xhtml",
            Some(("OEBPS/Text/ch1.xhtml", None)),
        ),
        (
            "OEBPS/content.opf",
            "x.xhtml?v=2#f",
            Some(("OEBPS/x.xhtml", Some("f"))),
        ),
        (
            "OEBPS/content.opf",
            "/OEBPS/x.xhtml",
            Some(("OEBPS/x.xhtml", None)),
        ),
        (
            "OEBPS/content.opf",
            "./a/../b.xhtml#",
            Some(("OEBPS/b.xhtml", None)),
        ),
        ("OEBPS/content.opf", "../../etc/passwd", None),
        ("OEBPS/content.opf", "%2e%2e/%2e%2e/x", None),
        ("OEBPS/content.opf", "https://example.com/x.css", None),
        ("OEBPS/content.opf", "data:image/png;base64,AAAA", None),
        ("OEBPS/content.opf", "a%00b.xhtml", None),
    ];
    for (base, href, expected) in cases {
        let got = resolve_href(base, href);
        let got = got
            .as_ref()
            .map(|(path, fragment)| (path.as_str(), fragment.as_deref()));
        assert_eq!(got, expected, "resolve_href({base:?}, {href:?})");
    }
}

#[test]
fn lookup_prefers_exact_then_ignores_case_and_duplicates_keep_the_first() {
    let zip = ZipBuilder::new()
        .stored("OEBPS/Text/Ch1.xhtml", b"primeiro")
        .stored("OEBPS/Text/Ch1.xhtml", b"repetido")
        .stored("oebps/text/ch1.XHTML", b"outra caixa")
        .build();
    let archive = archive(zip);
    assert_eq!(
        names(&archive),
        ["OEBPS/Text/Ch1.xhtml", "oebps/text/ch1.XHTML"]
    );
    assert_eq!(archive.read("OEBPS/Text/Ch1.xhtml").unwrap(), b"primeiro");
    assert_eq!(
        archive.read("oebps/text/ch1.XHTML").unwrap(),
        b"outra caixa"
    );
    assert_eq!(archive.read("OEBPS/TEXT/ch1.xhtml").unwrap(), b"primeiro");
    assert_eq!(
        archive.locate("OEBPS/content.opf", "TEXT/CH1.XHTML#x"),
        Some(("OEBPS/Text/Ch1.xhtml".to_string(), Some("x".to_string())))
    );
}

#[test]
fn a_compression_ratio_bomb_is_refused_before_inflating() {
    let zeros = vec![0u8; 8 * 1024 * 1024];
    let zip = ZipBuilder::new().deflated("bomba.bin", &zeros).build();
    assert!(
        zip.len() < 64 * 1024,
        "a bomba precisa ser pequena: {}",
        zip.len()
    );
    assert_eq!(
        limit_kind(EpubArchive::from_bytes(zip)),
        LimitKind::CompressionRatio
    );
    // Texto comum (razão bem abaixo de 200:1) passa.
    let prose: String = (0..40_000u32)
        .map(|n| format!("palavra{} ", n.wrapping_mul(2_654_435_761) % 9973))
        .collect();
    let zip = ZipBuilder::new()
        .deflated("prosa.xhtml", prose.as_bytes())
        .build();
    assert_eq!(archive(zip).read("prosa.xhtml").unwrap(), prose.as_bytes());
}

#[test]
fn declared_sizes_are_capped_per_entry_in_total_and_in_count() {
    // Total: 20 entradas "honestas" de 60 MiB cada (razão 60:1) passam de 1 GiB.
    let mut zip = ZipBuilder::new();
    for index in 0..20 {
        let mut entry = RawEntry::stored(&format!("parte{index}.bin"), b"");
        entry.method = 8;
        entry.size = 60 * 1024 * 1024;
        entry.compressed = 1024 * 1024;
        zip = zip.entry(entry);
    }
    assert_eq!(
        limit_kind(EpubArchive::from_bytes(zip.build())),
        LimitKind::TotalSize
    );

    let mut entry = RawEntry::stored("grande.bin", b"");
    entry.method = 8;
    entry.size = MAX_ENTRY_SIZE + 1;
    entry.compressed = 8 * 1024 * 1024;
    let zip = ZipBuilder::new().entry(entry).build();
    assert_eq!(
        limit_kind(EpubArchive::from_bytes(zip)),
        LimitKind::EntrySize
    );

    let mut zip = ZipBuilder::new();
    for index in 0..=MAX_ENTRIES {
        zip = zip.stored(&format!("e/{index}"), b"x");
    }
    assert_eq!(
        limit_kind(EpubArchive::from_bytes(zip.build())),
        LimitKind::Entries
    );
}

#[test]
fn names_extras_and_comments_have_a_length_limit() {
    let long_name = "a".repeat(MAX_NAME_LEN + 1);
    let zip = ZipBuilder::new().stored(&long_name, b"x").build();
    assert_eq!(
        limit_kind(EpubArchive::from_bytes(zip)),
        LimitKind::NameLength
    );

    let mut entry = RawEntry::stored("x", b"x");
    entry.extra = vec![0xAB; MAX_EXTRA_LEN + 1];
    let zip = ZipBuilder::new().entry(entry).build();
    assert_eq!(
        limit_kind(EpubArchive::from_bytes(zip)),
        LimitKind::ExtraLength
    );

    let mut entry = RawEntry::stored("x", b"x");
    entry.comment = vec![b'c'; MAX_COMMENT_LEN + 1];
    let zip = ZipBuilder::new().entry(entry).build();
    assert_eq!(
        limit_kind(EpubArchive::from_bytes(zip)),
        LimitKind::CommentLength
    );
}

#[test]
fn reading_never_goes_past_the_declared_size() {
    // O fluxo inflaria 100 000 bytes; o diretório central diz 1000.
    let big = vec![b'a'; 100_000];
    let mut entry = RawEntry::deflated("mentira.txt", &big);
    entry.size = 1000;
    entry.crc = crc32(&big[..1000]);
    let archive = archive(ZipBuilder::new().entry(entry).build());
    assert!(matches!(
        archive.read("mentira.txt"),
        Err(EpubError::Corrupt { .. })
    ));
    let mut reader = archive.open_entry("mentira.txt").unwrap();
    let mut produced = 0usize;
    let mut buffer = [0u8; 4096];
    let error = loop {
        match reader.read(&mut buffer) {
            Ok(0) => panic!("o fluxo longo demais terminou sem erro"),
            Ok(n) => produced += n,
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(produced, 1000, "nunca mais do que o tamanho declarado");
}

#[test]
fn short_streams_and_bad_checksums_are_corruption() {
    let text = vec![b'z'; 500];
    let mut short = RawEntry::deflated("curto.txt", &text);
    short.size = 800;
    let mut bad_crc = RawEntry::deflated("crc.txt", &text);
    bad_crc.crc ^= 1;
    let mut stored_bad_crc = RawEntry::stored("crc-stored.txt", &text);
    stored_bad_crc.crc ^= 0x8000_0000;
    let archive = archive(
        ZipBuilder::new()
            .entry(short)
            .entry(bad_crc)
            .entry(stored_bad_crc)
            .build(),
    );
    for name in ["curto.txt", "crc.txt", "crc-stored.txt"] {
        match archive.read(name) {
            Err(EpubError::Corrupt { name: got, .. }) => assert_eq!(got, name),
            other => panic!("{name}: esperava Corrupt, veio {other:?}"),
        }
    }

    let mut mismatched = RawEntry::stored("tamanhos.txt", b"abc");
    mismatched.compressed = 2;
    mismatched.payload.truncate(2);
    assert!(matches!(
        EpubArchive::from_bytes(ZipBuilder::new().entry(mismatched).build()),
        Err(EpubError::Corrupt { .. })
    ));
}

#[test]
fn encrypted_entries_and_unknown_methods_are_refused() {
    for (flags, method) in [(1u16, 0u16), (1 << 6, 8), (1 << 13, 8), (0, 99)] {
        let mut entry = RawEntry::stored("cifrado.xhtml", b"segredo");
        entry.flags = flags;
        entry.method = method;
        match EpubArchive::from_bytes(ZipBuilder::new().entry(entry).build()) {
            Err(EpubError::Encrypted(name)) => assert_eq!(name, "cifrado.xhtml"),
            other => panic!("flags {flags:#x} método {method}: veio {other:?}"),
        }
    }
    let mut entry = RawEntry::stored("bzip2.xhtml", b"x");
    entry.method = 12;
    assert!(matches!(
        EpubArchive::from_bytes(ZipBuilder::new().entry(entry).build()),
        Err(EpubError::UnsupportedCompression { method: 12, .. })
    ));
}

#[test]
fn local_headers_must_match_and_entries_cannot_overlap() {
    // Sobreposição "citada" (a bomba de sobreposição de Fifield): o registro
    // local completo de b.txt mora dentro dos dados de a.txt. Nome e método
    // batem; só os intervalos denunciam o reuso dos mesmos bytes.
    let inner = local_header("b.txt", b"xyz");
    let mut quoted = RawEntry::stored("b.txt", b"xyz");
    quoted.central_only = true;
    quoted.offset = Some(30 + "a.txt".len() as u64);
    let zip = ZipBuilder::new()
        .stored("a.txt", &inner)
        .entry(quoted)
        .build();
    match EpubArchive::from_bytes(zip) {
        Err(EpubError::Corrupt { name, reason }) => {
            assert_eq!(name, "b.txt");
            assert!(reason.contains("sobrepostas"), "{reason}");
        }
        other => panic!("sobreposição aceita: {other:?}"),
    }

    // Sobreposição total: duas entradas centrais no mesmo cabeçalho local.
    let mut twin = RawEntry::stored("b.txt", b"x");
    twin.central_only = true;
    twin.offset = Some(0);
    let zip = ZipBuilder::new().stored("a.txt", b"x").entry(twin).build();
    assert!(matches!(
        EpubArchive::from_bytes(zip),
        Err(EpubError::Corrupt { .. })
    ));

    let mut renamed = RawEntry::stored("a.txt", b"x");
    renamed.local_name = Some(b"z.txt".to_vec());
    assert!(matches!(
        EpubArchive::from_bytes(ZipBuilder::new().entry(renamed).build()),
        Err(EpubError::Corrupt { .. })
    ));

    let mut remethod = RawEntry::deflated("a.txt", b"xxxxxxxx");
    remethod.local_method = Some(0);
    assert!(matches!(
        EpubArchive::from_bytes(ZipBuilder::new().entry(remethod).build()),
        Err(EpubError::Corrupt { .. })
    ));

    // Tamanho comprimido declarado maior que os bytes: invadiria o diretório central.
    let mut overlong = RawEntry::stored("a.txt", b"0123456789");
    overlong.size = 10_000;
    overlong.compressed = 10_000;
    assert!(matches!(
        EpubArchive::from_bytes(ZipBuilder::new().entry(overlong).build()),
        Err(EpubError::Corrupt { .. })
    ));
}

/// xorshift64*: determinístico, sem dependências.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn truncated_mutated_and_garbage_archives_never_panic() {
    let seeds = [
        epub3_sample(),
        epub2_sample(),
        ZipBuilder::new()
            .zip64()
            .stored("mimetype", b"application/epub+zip")
            .deflated("META-INF/container.xml", CONTAINER_XML.as_bytes())
            .deflated("OEBPS/content.opf", EPUB3_OPF.as_bytes())
            .build(),
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut opened, mut parsed, mut read_ok) = (0usize, 0usize, 0usize);
    for round in 0..4000 {
        let mut bytes = seeds[round % seeds.len()].clone();
        let len = bytes.len();
        match rng.below(6) {
            0 => bytes.truncate(rng.below(len)),
            1 => {
                for _ in 0..1 + rng.below(8) {
                    let at = rng.below(len);
                    bytes[at] ^= 1 << rng.below(8);
                }
            }
            2 => {
                // Um campo de 16/32/64 bits reescrito (tamanhos, offsets, contagens).
                let width = [2, 4, 8][rng.below(3)];
                let at = rng.below(len.saturating_sub(width));
                let value = rng.next().to_le_bytes();
                bytes[at..at + width].copy_from_slice(&value[..width]);
            }
            3 => {
                // Mesmo, mas no diretório central (último terço do arquivo).
                let start = len - len / 3;
                let at = start + rng.below(len - start - 4);
                let value = [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0x7F][rng.below(8)];
                bytes[at] = value;
                bytes[at + 1] = value;
            }
            4 => {
                let at = rng.below(len);
                let insert: Vec<u8> = (0..1 + rng.below(64)).map(|_| rng.next() as u8).collect();
                bytes.splice(at..at, insert);
            }
            _ => {
                bytes = (0..rng.below(4096)).map(|_| rng.next() as u8).collect();
                if rng.below(2) == 0 {
                    bytes.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
                    bytes.extend((0..18).map(|_| rng.next() as u8));
                }
            }
        }
        let Ok(archive) = EpubArchive::from_bytes(bytes) else {
            continue;
        };
        opened += 1;
        for entry in archive.entries() {
            if archive.read(entry.name()).is_ok() {
                read_ok += 1;
            }
        }
        if EpubBook::parse(&archive).is_ok() {
            parsed += 1;
        }
    }
    // A mutação tem de chegar fundo: muitos ainda abrem, leem e passam no parser.
    assert!(opened > 500, "só {opened} abriram");
    assert!(read_ok > 2000, "só {read_ok} leituras");
    assert!(parsed > 200, "só {parsed} passaram no parser");
}

// ---------------------------------------------------------------------------
// Estrutura do livro
// ---------------------------------------------------------------------------

#[test]
fn epub3_metadata_manifest_spine_and_cover() {
    let book = parse(epub3_sample()).unwrap();
    assert_eq!(book.version, "3.0");
    assert_eq!(book.opf_path, "OEBPS/content.opf");
    let meta = &book.metadata;
    assert_eq!(meta.title.as_deref(), Some("O Livro de Teste & Companhia"));
    assert_eq!(
        meta.creators,
        [
            Creator {
                name: "Maria Autora".into(),
                role: Some("aut".into()),
                file_as: Some("Autora, Maria".into()),
            },
            Creator {
                name: "João Tradutor".into(),
                role: Some("trl".into()),
                file_as: None,
            },
        ]
    );
    assert_eq!(meta.authors(), ["Maria Autora"]);
    assert_eq!(meta.contributors[0].name, "Editor Convidado");
    assert_eq!(meta.language.as_deref(), Some("pt-BR"));
    assert_eq!(
        meta.identifier.as_deref(),
        Some("urn:uuid:12345678-1234-1234-1234-123456789abc")
    );
    assert_eq!(meta.publisher.as_deref(), Some("Editora Exemplo"));
    assert_eq!(
        meta.description.as_deref(),
        Some("Uma descrição em várias linhas.")
    );
    assert_eq!(meta.date.as_deref(), Some("2024-05-01"));
    assert_eq!(meta.subjects, ["Ficção", "Testes"]);
    assert_eq!(meta.series.as_deref(), Some("Série Exemplo"));
    assert_eq!(meta.series_index, Some(2.0));

    let chapter_one = book.manifest_item("c1x").unwrap();
    assert_eq!(
        chapter_one.path.as_deref(),
        Some("OEBPS/Text/capítulo 1.xhtml")
    );
    assert_eq!(
        book.manifest_item("c2x").unwrap().properties,
        ["svg", "scripted"]
    );

    let spine: Vec<(&str, bool)> = book
        .spine
        .iter()
        .map(|item| (item.path.as_str(), item.linear))
        .collect();
    assert_eq!(
        spine,
        [
            ("OEBPS/Text/capítulo 1.xhtml", true),
            ("OEBPS/Text/ch2.xhtml", true),
            ("OEBPS/Text/notes.xhtml", false),
        ]
    );
    assert_eq!(book.spine[1].properties, ["page-spread-left"]);
    assert_eq!(book.page_progression, PageProgression::Rtl);
    assert!(
        book.warnings
            .iter()
            .any(|warning| warning.contains("missing")),
        "{:?}",
        book.warnings
    );
    assert_eq!(book.spine_index_of("OEBPS/Text/ch2.xhtml"), Some(1));
    assert_eq!(
        book.cover,
        Some(CoverImage {
            path: "OEBPS/Images/capa.png".into(),
            media_type: "image/png".into(),
            manifest_id: Some("cover-img".into()),
        })
    );
    assert!(book.obfuscated_fonts.is_empty());
}

fn outline(entries: &[TocEntry]) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, &TocEntry)> = entries.iter().rev().map(|entry| (0, entry)).collect();
    while let Some((depth, entry)) = stack.pop() {
        out.push(format!(
            "{}{} -> {}#{} [{}]",
            "  ".repeat(depth),
            entry.label,
            entry.path.as_deref().unwrap_or("-"),
            entry.fragment.as_deref().unwrap_or(""),
            entry
                .spine_index
                .map_or_else(|| "-".to_string(), |index| index.to_string()),
        ));
        stack.extend(entry.children.iter().rev().map(|child| (depth + 1, child)));
    }
    out
}

#[test]
fn epub3_nav_toc_keeps_nesting_fragments_and_spine_positions() {
    let book = parse(epub3_sample()).unwrap();
    assert_eq!(
        outline(&book.toc),
        [
            "Capítulo 1 -> OEBPS/Text/capítulo 1.xhtml# [0]",
            "Parte II -> -# [-]",
            "  Seção 2.1 -> OEBPS/Text/ch2.xhtml#sec1 [1]",
            "    Seção 2.1.1 — fim -> OEBPS/Text/ch2.xhtml#sec1-1 [1]",
            "  Seção 2.2 -> OEBPS/Text/ch2.xhtml#sec2 [1]",
            "Notas -> OEBPS/Text/notes.xhtml# [2]",
        ]
    );
}

#[test]
fn epub2_metadata_ncx_toc_and_meta_cover() {
    let book = parse(epub2_sample()).unwrap();
    assert_eq!(book.version, "2.0");
    let meta = &book.metadata;
    assert_eq!(meta.title.as_deref(), Some("Livro Dois"));
    assert_eq!(
        meta.creators,
        [
            Creator {
                name: "José Silva".into(),
                role: Some("aut".into()),
                file_as: Some("Silva, José".into()),
            },
            Creator {
                name: "Ilustra Dora".into(),
                role: Some("ill".into()),
                file_as: None,
            },
        ]
    );
    assert_eq!(meta.authors(), ["José Silva"]);
    assert_eq!(meta.identifier.as_deref(), Some("urn:uuid:aaaa"));
    assert_eq!(
        meta.date.as_deref(),
        Some("1999"),
        "data de publicação primeiro"
    );
    assert_eq!(meta.series.as_deref(), Some("Coleção Dois"));
    assert_eq!(meta.series_index, Some(3.5));
    assert_eq!(book.page_progression, PageProgression::Default);
    assert_eq!(
        book.cover
            .as_ref()
            .map(|cover| (cover.path.as_str(), cover.media_type.as_str())),
        Some(("OEBPS/images/cover.jpg", "image/jpeg"))
    );
    assert_eq!(
        outline(&book.toc),
        [
            "Parte 1 -> OEBPS/text/part1.html# [0]",
            "  Parte 1 — A -> OEBPS/text/part1.html#a [0]",
            "    Fundo -> OEBPS/text/part1.html#a1 [0]",
            "Parte 2 -> OEBPS/text/part2.html#inicio [1]",
        ]
    );
}

#[test]
fn nav_wins_over_ncx_and_ncx_covers_a_nav_without_toc() {
    let nav = |label: &str| {
        format!(
            r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body>
<nav epub:type="{label}"><ol><li><a href="p1.xhtml">Do nav</a></li></ol></nav></body></html>"#
        )
    };
    let ncx = r#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap>
<navPoint><navLabel><text>Do NCX</text></navLabel><content src="p1.xhtml"/></navPoint></navMap></ncx>"#;
    let opf = opf(
        "",
        &format!(
            r#"{P1_ITEM}<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#
        ),
        P1_SPINE,
        "",
    );
    let with_toc = nav("toc");
    let book = parse(book_with_opf(
        &opf,
        &[
            ("OEBPS/nav.xhtml", with_toc.as_bytes()),
            ("OEBPS/toc.ncx", ncx.as_bytes()),
        ],
    ))
    .unwrap();
    assert_eq!(outline(&book.toc), ["Do nav -> OEBPS/p1.xhtml# [0]"]);

    let landmarks_only = nav("landmarks");
    let book = parse(book_with_opf(
        &opf,
        &[
            ("OEBPS/nav.xhtml", landmarks_only.as_bytes()),
            ("OEBPS/toc.ncx", ncx.as_bytes()),
        ],
    ))
    .unwrap();
    assert_eq!(outline(&book.toc), ["Do NCX -> OEBPS/p1.xhtml# [0]"]);
}

#[test]
fn a_very_deep_toc_is_flattened_at_the_depth_limit() {
    let levels = 100;
    let mut ncx = String::from(r#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap>"#);
    for level in 0..levels {
        ncx.push_str(&format!(
            r#"<navPoint><navLabel><text>N{level}</text></navLabel><content src="p1.xhtml#n{level}"/>"#
        ));
    }
    ncx.push_str(&"</navPoint>".repeat(levels));
    ncx.push_str("</navMap></ncx>");
    let opf = opf(
        "",
        &format!(
            r#"{P1_ITEM}<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#
        ),
        P1_SPINE,
        "",
    );
    let book = parse(book_with_opf(&opf, &[("OEBPS/toc.ncx", ncx.as_bytes())])).unwrap();
    let lines = outline(&book.toc);
    assert_eq!(lines.len(), levels, "nenhuma entrada se perde");
    let deepest = lines
        .iter()
        .map(|line| (line.len() - line.trim_start().len()) / 2 + 1)
        .max()
        .unwrap();
    assert_eq!(deepest, MAX_TOC_DEPTH);
    assert!(
        lines
            .last()
            .unwrap()
            .trim_start()
            .starts_with("N99 -> OEBPS/p1.xhtml#n99")
    );
}

#[test]
fn cover_is_found_through_every_fallback() {
    let jpeg = JPEG_STUB;
    let cover_page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><div><img src="images/front.jpg" alt=""/></div></body></html>"#;
    let svg_page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="../img/pic.jpg"/></svg></body></html>"#;
    let front_item = r#"<item id="img1" href="images/front.jpg" media-type="image/jpeg"/>"#;
    type Case<'a> = (
        &'a str,
        String,
        String,
        String,
        Vec<(&'a str, &'a [u8])>,
        Option<&'a str>,
    );
    let cases: Vec<Case> = vec![
        (
            "meta cover com id em outra caixa",
            r#"<meta name="cover" content="FrontImg"/>"#.into(),
            format!(r#"{P1_ITEM}<item id="frontimg" href="i/f.png" media-type="image/png"/>"#),
            String::new(),
            vec![("OEBPS/i/f.png", PNG_1X1)],
            Some("OEBPS/i/f.png"),
        ),
        (
            "meta cover apontando para a página de capa",
            r#"<meta name="cover" content="cpage"/>"#.into(),
            format!(
                r#"{P1_ITEM}{front_item}<item id="cpage" href="cover.xhtml" media-type="application/xhtml+xml"/>"#
            ),
            String::new(),
            vec![("OEBPS/cover.xhtml", cover_page.as_bytes()), ("OEBPS/images/front.jpg", jpeg)],
            Some("OEBPS/images/front.jpg"),
        ),
        (
            "guide type=cover",
            String::new(),
            format!(
                r#"{P1_ITEM}{front_item}<item id="cp" href="cover.xhtml" media-type="application/xhtml+xml"/>"#
            ),
            r#"<guide><reference type="cover" title="Capa" href="cover.xhtml"/></guide>"#.into(),
            vec![("OEBPS/cover.xhtml", cover_page.as_bytes()), ("OEBPS/images/front.jpg", jpeg)],
            Some("OEBPS/images/front.jpg"),
        ),
        (
            "nome com cover; cover-image num item que não é imagem é ignorado",
            String::new(),
            format!(
                r#"{P1_ITEM}<item id="css" href="s.css" media-type="text/css" properties="cover-image"/><item id="img9" href="Images/Cover-Art.PNG" media-type="image/png"/>"#
            ),
            String::new(),
            vec![("OEBPS/s.css", b"p{}"), ("OEBPS/Images/Cover-Art.PNG", PNG_1X1)],
            Some("OEBPS/Images/Cover-Art.PNG"),
        ),
        (
            "primeira imagem do primeiro documento do spine (SVG)",
            String::new(),
            r#"<item id="p0" href="text/p0.xhtml" media-type="application/xhtml+xml"/><item id="pic" href="img/pic.jpg" media-type="image/jpeg"/>"#.into(),
            String::new(),
            vec![("OEBPS/text/p0.xhtml", svg_page.as_bytes()), ("OEBPS/img/pic.jpg", jpeg)],
            Some("OEBPS/img/pic.jpg"),
        ),
        ("sem imagem nenhuma", String::new(), P1_ITEM.into(), String::new(), vec![], None),
    ];
    for (label, metadata, manifest, extra, files, expected) in cases {
        let spine = if manifest.contains(r#"id="p0""#) {
            r#"<itemref idref="p0"/>"#
        } else {
            P1_SPINE
        };
        let opf = opf(&metadata, &manifest, spine, &extra);
        let book =
            parse(book_with_opf(&opf, &files)).unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(
            book.cover.as_ref().map(|cover| cover.path.as_str()),
            expected,
            "{label}"
        );
    }
}

fn encryption_xml(entries: &[(&str, &str)]) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0"?><encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#">"#,
    );
    for (algorithm, uri) in entries {
        xml.push_str(&format!(
            r#"<enc:EncryptedData><enc:EncryptionMethod Algorithm="{algorithm}"/><enc:CipherData><enc:CipherReference URI="{uri}"/></enc:CipherData></enc:EncryptedData>"#
        ));
    }
    xml.push_str("</encryption>");
    xml
}

#[test]
fn drm_is_detected_but_font_obfuscation_is_not_drm() {
    let fonts = format!(
        r#"{P1_ITEM}<item id="f1" href="Fonts/serif.otf" media-type="font/otf"/><item id="f2" href="Fonts/sans.ttf" media-type="application/x-font-truetype"/>"#
    );
    let opf = opf("", &fonts, P1_SPINE, "");
    let font_files: [(&str, &[u8]); 2] = [
        ("OEBPS/Fonts/serif.otf", b"OTTO"),
        ("OEBPS/Fonts/sans.ttf", b"\0\x01\0\0"),
    ];

    let obfuscated = encryption_xml(&[
        (IDPF_FONT_OBFUSCATION, "OEBPS/Fonts/serif.otf"),
        (ADOBE_FONT_OBFUSCATION, "OEBPS/Fonts/sans.ttf"),
    ]);
    let mut files = font_files.to_vec();
    files.push(("META-INF/encryption.xml", obfuscated.as_bytes()));
    let book = parse(book_with_opf(&opf, &files)).expect("ofuscação de fonte não é DRM");
    assert_eq!(
        book.obfuscated_fonts,
        ["OEBPS/Fonts/serif.otf", "OEBPS/Fonts/sans.ttf"]
    );

    let aes = "http://www.w3.org/2001/04/xmlenc#aes256-cbc";
    for (label, encryption) in [
        (
            "conteúdo cifrado",
            encryption_xml(&[(aes, "OEBPS/p1.xhtml")]),
        ),
        (
            "fonte cifrada com AES",
            encryption_xml(&[(aes, "OEBPS/Fonts/serif.otf")]),
        ),
        (
            "algoritmo de fonte aplicado a um capítulo",
            encryption_xml(&[(IDPF_FONT_OBFUSCATION, "OEBPS/p1.xhtml")]),
        ),
    ] {
        let mut files = font_files.to_vec();
        files.push(("META-INF/encryption.xml", encryption.as_bytes()));
        match parse(book_with_opf(&opf, &files)) {
            Err(EpubError::Drm(detail)) => assert!(!detail.is_empty()),
            other => panic!("{label}: esperava Drm, veio {other:?}"),
        }
    }

    let rights: [(&str, &[u8]); 1] = [("META-INF/rights.xml", b"<rights/>")];
    assert!(matches!(
        parse(book_with_opf(&opf, &rights)),
        Err(EpubError::Drm(_))
    ));

    let empty = encryption_xml(&[]);
    assert!(
        parse(book_with_opf(
            &opf,
            &[("META-INF/encryption.xml", empty.as_bytes())]
        ))
        .is_ok()
    );
}

#[test]
fn entity_bombs_and_external_entities_are_refused() {
    let laughs = r#"<?xml version="1.0"?>
<!DOCTYPE package [
 <!ENTITY lol "lol">
 <!ENTITY lol1 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
 <!ENTITY lol2 "&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;">
 <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
]>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>&lol3;</dc:title></metadata>
<manifest><item id="p1" href="p1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="p1"/></spine></package>"#;
    match parse(book_with_opf(laughs, &[])) {
        Err(EpubError::UnsafeXml { path, .. }) => assert_eq!(path, "OEBPS/content.opf"),
        other => panic!("bilhão de risadas aceito: {other:?}"),
    }

    let xxe = r#"<?xml version="1.0"?><!DOCTYPE container [<!ENTITY xxe SYSTEM "file:///C:/Windows/win.ini">]>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf&xxe;"/></rootfiles></container>"#;
    let opf_text = opf("", P1_ITEM, P1_SPINE, "");
    let page = chapter("P1", "");
    let zip = epub_with(&[
        ("META-INF/container.xml", xxe.as_bytes()),
        ("OEBPS/content.opf", opf_text.as_bytes()),
        ("OEBPS/p1.xhtml", page.as_bytes()),
    ]);
    match parse(zip) {
        Err(EpubError::UnsafeXml { path, .. }) => assert_eq!(path, "META-INF/container.xml"),
        other => panic!("XXE aceito: {other:?}"),
    }

    let lower_case_ncx = r#"<!doctype ncx [<!entity % p SYSTEM "http://example.com/x.dtd"> %p;]><ncx><navMap/></ncx>"#;
    let opf_ncx = opf(
        "",
        &format!(
            r#"{P1_ITEM}<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#
        ),
        P1_SPINE,
        "",
    );
    assert!(matches!(
        parse(book_with_opf(
            &opf_ncx,
            &[("OEBPS/toc.ncx", lower_case_ncx.as_bytes())]
        )),
        Err(EpubError::UnsafeXml { .. })
    ));

    // Sem DOCTYPE, uma entidade desconhecida fica literal; nada se expande.
    let literal = opf(
        r#"<dc:creator>A &lol9; B &amp; C&#233;</dc:creator><dc:publisher>Tom & Jerry</dc:publisher>"#,
        r#"<item id="p1" href="p1.xhtml?x=&foo;" media-type="application/xhtml+xml"/>"#,
        P1_SPINE,
        "",
    );
    let book = parse(book_with_opf(&literal, &[])).unwrap();
    assert_eq!(book.metadata.creators[0].name, "A &lol9; B & Cé");
    assert_eq!(book.manifest[0].href, "p1.xhtml?x=&foo;");
    // `&` solto (OPF feito à mão) não derruba o documento.
    assert_eq!(book.metadata.publisher.as_deref(), Some("Tom & Jerry"));
}

#[test]
fn package_structure_is_tolerated_where_readers_tolerate_it() {
    let opf_text = opf("", P1_ITEM, P1_SPINE, "");
    let page = chapter("P1", "");

    // Sem mimetype: abre, com aviso.
    let zip = ZipBuilder::new()
        .deflated("META-INF/container.xml", CONTAINER_XML.as_bytes())
        .deflated("OEBPS/content.opf", opf_text.as_bytes())
        .deflated("OEBPS/p1.xhtml", page.as_bytes())
        .build();
    let book = parse(zip).unwrap();
    assert!(
        book.warnings.iter().any(|w| w.contains("mimetype")),
        "{:?}",
        book.warnings
    );

    // mimetype fora de ordem e com outro conteúdo: abre, com avisos.
    let zip = ZipBuilder::new()
        .deflated("META-INF/container.xml", CONTAINER_XML.as_bytes())
        .stored("mimetype", b"application/zip\n")
        .deflated("OEBPS/content.opf", opf_text.as_bytes())
        .deflated("OEBPS/p1.xhtml", page.as_bytes())
        .build();
    let book = parse(zip).unwrap();
    assert_eq!(
        book.warnings
            .iter()
            .filter(|w| w.contains("mimetype"))
            .count(),
        2
    );

    // Sem container.xml: o primeiro .opf serve.
    let zip = epub_with(&[
        ("OEBPS/content.opf", opf_text.as_bytes()),
        ("OEBPS/p1.xhtml", page.as_bytes()),
    ]);
    assert_eq!(parse(zip).unwrap().opf_path, "OEBPS/content.opf");

    // Sem OPF nenhum, ou sem nada legível no spine: não é EPUB.
    let zip = epub_with(&[("OEBPS/p1.xhtml", page.as_bytes())]);
    assert!(matches!(parse(zip), Err(EpubError::NotEpub(_))));
    let empty_spine = opf("", P1_ITEM, r#"<itemref idref="nada"/>"#, "");
    assert!(matches!(
        parse(book_with_opf(&empty_spine, &[])),
        Err(EpubError::NotEpub(_))
    ));
}

#[test]
fn opf_in_utf16_or_latin1_is_decoded() {
    let text = opf("", P1_ITEM, P1_SPINE, "")
        .replace("<dc:title>T</dc:title>", "<dc:title>Café Ω</dc:title>");
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in text.replace("UTF-8", "UTF-16").encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    let page = chapter("P1", "");
    let zip = epub_with(&[
        ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
        ("OEBPS/content.opf", &utf16),
        ("OEBPS/p1.xhtml", page.as_bytes()),
    ]);
    assert_eq!(
        parse(zip).unwrap().metadata.title.as_deref(),
        Some("Café Ω")
    );

    let latin1: Vec<u8> = opf("", P1_ITEM, P1_SPINE, "")
        .replace("<dc:title>T</dc:title>", "<dc:title>Caf\u{e9}</dc:title>")
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap())
        .collect();
    let zip = epub_with(&[
        ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
        ("OEBPS/content.opf", &latin1),
        ("OEBPS/p1.xhtml", page.as_bytes()),
    ]);
    assert_eq!(parse(zip).unwrap().metadata.title.as_deref(), Some("Café"));
}

#[test]
fn xml_documents_have_node_and_size_limits() {
    let many = opf(&"<x/>".repeat(MAX_XML_NODES + 1), P1_ITEM, P1_SPINE, "");
    let page = chapter("P1", "");
    let stored = |opf: &str| {
        ZipBuilder::new()
            .stored("mimetype", b"application/epub+zip")
            .stored("META-INF/container.xml", CONTAINER_XML.as_bytes())
            .stored("OEBPS/content.opf", opf.as_bytes())
            .stored("OEBPS/p1.xhtml", page.as_bytes())
            .build()
    };
    assert_eq!(limit_kind(parse(stored(&many))), LimitKind::XmlNodes);

    let padding = " ".repeat(MAX_XML_BYTES as usize);
    let huge = opf("", P1_ITEM, P1_SPINE, &padding);
    assert_eq!(limit_kind(parse(stored(&huge))), LimitKind::XmlSize);
}

#[test]
fn an_open_archive_can_be_shared_across_threads() {
    // Falha na compilação se `EpubArchive` deixar de ser `Send + Sync`.
    fn shareable<T: Send + Sync>(_: &T) {}
    let archive = archive(epub3_sample());
    shareable(&archive);
    let archive = std::sync::Arc::new(archive);
    let worker = {
        let archive = std::sync::Arc::clone(&archive);
        std::thread::spawn(move || archive.read("OEBPS/Images/capa.png").unwrap())
    };
    assert_eq!(worker.join().unwrap(), PNG_1X1);
}
