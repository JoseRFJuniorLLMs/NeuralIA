//! Gates do leitor de EPUB sobre o caminho que embarca: o servidor da origem
//! (`EpubServer::respond`, o mesmo que a thread do WebView chama), o parser e
//! o despachante do IPC (`handle_epub_ipc`, o corpo do `with_ipc_handler`),
//! o worker da biblioteca numa pasta temporária, e o JavaScript servido
//! (`epub_asset`) corrido no Node com `node:vm` e DOM falso.
//!
//! Os livros são montados aqui mesmo, num ZIP escrito à mão (stored e
//! deflate via flate2). Nenhum livro real entra no repositório.

use super::*;

use std::{
    io::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

use flate2::{Compression as Level, Crc, write::DeflateEncoder};

// ---------------------------------------------------------------- suporte

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Pasta temporária apagada no fim do teste.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "neuralia-epub-app-{tag}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pasta temporária");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Um escritor de ZIP mínimo: entradas stored ou deflate, diretório central
/// e fim de arquivo. Só o que um EPUB precisa.
#[derive(Default)]
struct Zip {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl Zip {
    fn add(mut self, name: &str, data: &[u8], deflate: bool) -> Self {
        let mut crc = Crc::new();
        crc.update(data);
        let crc = crc.sum();
        let (method, payload) = if deflate {
            let mut encoder = DeflateEncoder::new(Vec::new(), Level::best());
            encoder.write_all(data).expect("deflate");
            (8u16, encoder.finish().expect("deflate"))
        } else {
            (0u16, data.to_vec())
        };
        let offset = self.out.len() as u32;
        let name = name.as_bytes();
        let flags: u16 = 0x0800;
        let out = &mut self.out;
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(flags.to_le_bytes());
        out.extend(method.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0x21u16.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend((payload.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name);
        out.extend(&payload);

        let central = &mut self.central;
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(flags.to_le_bytes());
        central.extend(method.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0x21u16.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend((payload.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u32.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name);
        self.count += 1;
        self
    }

    fn finish(mut self) -> Vec<u8> {
        let offset = self.out.len() as u32;
        let size = self.central.len() as u32;
        self.out.extend_from_slice(&self.central);
        self.out.extend(0x0605_4b50u32.to_le_bytes());
        self.out.extend(0u16.to_le_bytes());
        self.out.extend(0u16.to_le_bytes());
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(size.to_le_bytes());
        self.out.extend(offset.to_le_bytes());
        self.out.extend(0u16.to_le_bytes());
        self.out
    }
}

/// PNG 1x1 válido (a capa do livro de teste).
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];
const JPEG_BYTES: &[u8] = b"\xFF\xD8\xFF\xE0fake-jpeg-body\xFF\xD9";
const EVIL_JS: &[u8] = b"window.ipc.postMessage('{\"t\":\"close\"}');";

const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

const OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">urn:uuid:neuralia-epub-app-test</dc:identifier>
    <dc:title>O Livro de Teste</dc:title>
    <dc:creator>Ana Autora</dc:creator>
    <dc:language>pt-BR</dc:language>
    <meta property="dcterms:modified">2026-01-01T00:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="Text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="Text/ch%202.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="Styles/book.css" media-type="text/css"/>
    <item id="cover" href="Images/cover.png" media-type="image/png" properties="cover-image"/>
    <item id="photo" href="Images/caf%C3%A9.jpg" media-type="image/jpeg"/>
    <item id="js" href="Scripts/evil.js" media-type="text/javascript"/>
    <item id="font" href="Fonts/f.woff2" media-type="application/font-woff2"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;

const NAV: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Sumário</title></head>
<body><nav epub:type="toc"><ol>
  <li><a href="Text/ch1.xhtml">Capítulo 1</a><ol><li><a href="Text/ch1.xhtml#s2">Seção 1.2</a></li></ol></li>
  <li><a href="Text/ch%202.xhtml">Capítulo 2</a></li>
</ol></nav></body></html>"#;

const CHAPTER_1: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Um</title>
<link rel="stylesheet" href="../Styles/book.css"/><script src="../Scripts/evil.js"></script></head>
<body><h1>Capítulo 1</h1><p>A ação começa aqui, com bastante texto para ser lido.</p>
<p id="s2">Outra seção <a href="https://example.com/">link</a>.</p>
<img src="../Images/caf%C3%A9.jpg" alt=""/></body></html>"#;

const CHAPTER_2: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Dois</title></head>
<body><h1>Capítulo 2</h1><p>Fim da ação.</p></body></html>"#;

fn sample_zip() -> Zip {
    Zip::default()
        .add("mimetype", b"application/epub+zip", false)
        .add("META-INF/container.xml", CONTAINER.as_bytes(), true)
        .add("OEBPS/content.opf", OPF.as_bytes(), true)
        .add("OEBPS/nav.xhtml", NAV.as_bytes(), true)
        .add("OEBPS/Text/ch1.xhtml", CHAPTER_1.as_bytes(), true)
        .add("OEBPS/Text/ch 2.xhtml", CHAPTER_2.as_bytes(), false)
        .add("OEBPS/Styles/book.css", b"p { text-indent: 1em; }", true)
        .add("OEBPS/Images/cover.png", PNG_1X1, false)
        .add("OEBPS/Images/café.jpg", JPEG_BYTES, false)
        .add("OEBPS/Scripts/evil.js", EVIL_JS, false)
        .add("OEBPS/Fonts/f.woff2", b"wOF2fake-font", false)
}

fn sample_epub() -> Vec<u8> {
    sample_zip().finish()
}

fn drm_epub() -> Vec<u8> {
    sample_zip()
        .add(
            "META-INF/rights.xml",
            b"<rights xmlns=\"http://ns.adobe.com/adept\"/>",
            false,
        )
        .finish()
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("gravar arquivo de teste");
    path
}

fn worker_in(dir: &Path) -> (EpubWorker, mpsc::Receiver<EpubNotice>) {
    let (sender, receiver) = mpsc::channel();
    let worker = EpubWorker::spawn(
        dir.to_path_buf(),
        Box::new(move |notice| {
            let _ = sender.send(notice);
        }),
    )
    .expect("worker");
    (worker, receiver)
}

/// Espera o worker acabar tudo o que foi pedido antes (sem relógio).
fn flush(worker: &EpubWorker) {
    let (done, wait) = sync_channel(1);
    assert!(worker.submit(EpubJob::Flush(done)));
    wait.recv().expect("o worker respondeu");
}

/// Biblioteca numa pasta temporária com o livro de teste já dentro.
struct Fixture {
    _temp: TempDir,
    library: PathBuf,
    worker: EpubWorker,
    notices: mpsc::Receiver<EpubNotice>,
    id: String,
}

fn fixture() -> Fixture {
    let temp = TempDir::new("fixture");
    let library = temp.path().join("library");
    let source = write_file(temp.path(), "teste.epub", &sample_epub());
    let (worker, notices) = worker_in(&library);
    assert!(worker.submit(EpubJob::Add {
        paths: vec![source],
        open: true,
    }));
    flush(&worker);
    let id = match notices.recv().expect("aviso de livro adicionado") {
        EpubNotice::Added {
            books,
            failures,
            open: true,
        } => {
            assert!(failures.is_empty(), "{failures:?}");
            assert_eq!(books.len(), 1);
            assert_eq!(books[0].title, "O Livro de Teste");
            books[0].id.clone()
        }
        other => panic!("aviso inesperado: {other:?}"),
    };
    Fixture {
        _temp: temp,
        library,
        worker,
        notices,
        id,
    }
}

impl Fixture {
    fn server(&self) -> EpubServer {
        EpubServer::new(self.worker.shared())
    }
}

fn get(server: &mut EpubServer, path: &str) -> EpubResponse {
    server.respond("GET", path)
}

fn header<'a>(headers: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn json_body(response: &EpubResponse) -> Value {
    serde_json::from_slice(&response.body).expect("corpo JSON")
}

const READER_SOURCE: &str = "http://neuralia-epub.localhost/reader.html?book=0123456789abcdef";
const LIBRARY_SOURCE: &str = "http://neuralia-epub.localhost/library.html";
const SOME_ID: &str = "0123456789abcdef";

// ------------------------------------------------------ servidor: páginas

#[test]
fn epub_origin_serves_only_its_own_assets_with_the_page_csp() {
    let temp = TempDir::new("assets");
    let (worker, _notices) = worker_in(&temp.path().join("library"));
    let mut server = EpubServer::new(worker.shared());
    for (path, kind) in [
        ("/library.html", "text/html"),
        ("/reader.html", "text/html"),
        ("/common.js", "text/javascript"),
        ("/library.js", "text/javascript"),
        ("/reader.js", "text/javascript"),
        ("/epub.css", "text/css"),
    ] {
        let response = get(&mut server, path);
        assert_eq!(response.status, 200, "{path}");
        assert!(response.content_type.starts_with(kind), "{path}");
        let headers = response.headers();
        assert_eq!(
            header(&headers, "Content-Security-Policy"),
            Some(PAGE_CSP),
            "{path}"
        );
        assert_eq!(
            header(&headers, "X-Content-Type-Options"),
            Some("nosniff")
        );
        assert_eq!(response.body.as_ref(), epub_asset(path).unwrap().1);
    }
    // A política das páginas: só script nosso, nada inline, rede só para a
    // própria origem.
    for directive in [
        "script-src 'self';",
        "style-src 'self';",
        "connect-src 'self';",
        "frame-src 'self';",
        "object-src 'none';",
    ] {
        assert!(PAGE_CSP.contains(directive), "{directive}");
    }
    assert!(!PAGE_CSP.contains("unsafe-inline") && !PAGE_CSP.contains("unsafe-eval"));
    for path in [
        "/",
        "/library.htm",
        "/LIBRARY.HTML",
        "/library.html/",
        "/library.html/x",
        "/../library.html",
        "/assets/epub/reader.js",
        "/api",
        "/api/",
        "/api/libraryx",
        "/book",
        "/book/",
        "/cover/",
    ] {
        assert_eq!(get(&mut server, path).status, 404, "{path}");
    }
    assert_eq!(server.respond("POST", "/library.html").status, 405);
    let head = server.respond("HEAD", "/reader.js");
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(
        header(&head.headers(), "Content-Security-Policy"),
        Some(PAGE_CSP)
    );
}

// -------------------------------------------------- servidor: o livro

#[test]
fn book_resources_are_served_by_exact_path_with_their_type_and_the_book_csp() {
    let fx = fixture();
    let mut server = fx.server();
    let archive = EpubArchive::from_bytes(sample_epub()).expect("zip de teste");
    let base = format!("/book/{}/", fx.id);
    for (path, served_as, entry) in [
        (
            "OEBPS/Text/ch1.xhtml",
            "application/xhtml+xml",
            "OEBPS/Text/ch1.xhtml",
        ),
        (
            "OEBPS/Text/ch%202.xhtml",
            "application/xhtml+xml",
            "OEBPS/Text/ch 2.xhtml",
        ),
        ("OEBPS/Styles/book.css", "text/css", "OEBPS/Styles/book.css"),
        ("OEBPS/Images/cover.png", "image/png", "OEBPS/Images/cover.png"),
        (
            "OEBPS/Images/caf%C3%A9.jpg",
            "image/jpeg",
            "OEBPS/Images/café.jpg",
        ),
        (
            "OEBPS/Scripts/evil.js",
            "application/octet-stream",
            "OEBPS/Scripts/evil.js",
        ),
        // Tipo do manifest fora da lista: vale a extensão.
        ("OEBPS/Fonts/f.woff2", "font/woff2", "OEBPS/Fonts/f.woff2"),
        ("OEBPS/nav.xhtml", "application/xhtml+xml", "OEBPS/nav.xhtml"),
    ] {
        let response = get(&mut server, &format!("{base}{path}"));
        assert_eq!(response.status, 200, "{path}");
        assert_eq!(response.content_type, served_as, "{path}");
        let headers = response.headers();
        assert_eq!(
            header(&headers, "Content-Security-Policy"),
            Some(BOOK_CSP),
            "{path}: todo o recurso do livro leva a CSP dos livros"
        );
        assert_eq!(header(&headers, "X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(
            response.body.as_ref(),
            archive.read(entry).expect("entrada").as_slice(),
            "{path}"
        );
    }
    // A CSP dos livros: nenhum script, nenhuma origem remota, nada de
    // formulários; só a nossa origem e data: para imagens, estilos, fontes e
    // media.
    for directive in [
        "default-src 'none';",
        "script-src 'none';",
        "form-action 'none';",
        "img-src http://neuralia-epub.localhost data:;",
        "style-src http://neuralia-epub.localhost data: 'unsafe-inline';",
        "font-src http://neuralia-epub.localhost data:;",
        "media-src http://neuralia-epub.localhost data:;",
    ] {
        assert!(BOOK_CSP.contains(directive), "{directive}");
    }
    for remote in ["http:", "https:", "*", "'unsafe-eval'", "blob:"] {
        assert!(
            !BOOK_CSP
                .split(';')
                .flat_map(str::split_whitespace)
                .any(|token| token == remote),
            "{remote}"
        );
    }
    let head = server.respond("HEAD", &format!("{base}OEBPS/Text/ch1.xhtml"));
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(
        header(&head.headers(), "Content-Security-Policy"),
        Some(BOOK_CSP)
    );
}

#[test]
fn traversal_malformed_and_unknown_book_paths_are_404() {
    let fx = fixture();
    // Um arquivo que existe no disco da biblioteca mas não no livro.
    std::fs::write(fx.library.join("secret.txt"), b"segredo").unwrap();
    let mut server = fx.server();
    let id = &fx.id;
    let unknown = "fedcba9876543210";
    for path in [
        format!("/book/{id}/OEBPS/Text/../Text/ch1.xhtml"),
        format!("/book/{id}/OEBPS/./Text/ch1.xhtml"),
        format!("/book/{id}//OEBPS/Text/ch1.xhtml"),
        format!("/book/{id}/OEBPS/Text/ch1.xhtml/"),
        format!("/book/{id}/%2e%2e/%2e%2e/secret.txt"),
        format!("/book/{id}/..%2F..%2Flibrary.json"),
        format!("/book/{id}/OEBPS%2F..%2F..%2Fsecret.txt"),
        format!("/book/{id}/OEBPS%5CText%5Cch1.xhtml"),
        format!("/book/{id}/OEBPS/Text/ch1.xhtml%00"),
        format!("/book/{id}/C:/Windows/win.ini"),
        format!("/book/{id}/%2FOEBPS/Text/ch1.xhtml"),
        format!("/book/{id}/OEBPS/Text/ch%zz.xhtml"),
        format!("/book/{id}/OEBPS/Text/ch%ff.xhtml"),
        format!("/book/{id}/OEBPS/Text/ch1"),
        format!("/book/{id}/OEBPS/Text/nao-existe.xhtml"),
        format!("/book/{id}/"),
        format!("/book/{id}"),
        format!("/book/{unknown}/OEBPS/Text/ch1.xhtml"),
        format!("/book/{}/OEBPS/Text/ch1.xhtml", id.to_uppercase()),
        "/book/../library.json".to_string(),
        "/book/xyz/OEBPS/Text/ch1.xhtml".to_string(),
        format!("/api/book/{unknown}"),
        "/api/book/..%2Flibrary.json".to_string(),
        format!("/cover/{unknown}"),
        "/cover/../library.json".to_string(),
    ] {
        let response = get(&mut server, &path);
        assert_eq!(response.status, 404, "{path}");
        assert!(
            !response.body.windows(7).any(|window| window == b"segredo"),
            "{path}"
        );
    }
}

#[test]
fn library_and_book_api_expose_metadata_spine_toc_positions_and_bookmarks() {
    let fx = fixture();
    let mut server = fx.server();
    let library = json_body(&get(&mut server, "/api/library"));
    let books = library["books"].as_array().expect("livros");
    assert_eq!(books.len(), 1);
    let entry = &books[0];
    assert_eq!(entry["id"], fx.id.as_str());
    assert_eq!(entry["title"], "O Livro de Teste");
    assert_eq!(entry["authors"], json!(["Ana Autora"]));
    assert_eq!(entry["language"], "pt-BR");
    assert_eq!(entry["cover"], format!("/cover/{}", fx.id));
    assert!(entry["progress"].is_null(), "ainda não aberto");
    let cover = get(&mut server, &format!("/cover/{}", fx.id));
    assert_eq!(cover.status, 200);
    assert_eq!(cover.content_type, "image/png");
    assert_eq!(cover.body.as_ref(), PNG_1X1);

    let book = json_body(&get(&mut server, &format!("/api/book/{}", fx.id)));
    assert_eq!(book["title"], "O Livro de Teste");
    assert_eq!(book["language"], "pt-BR");
    let spine = book["spine"].as_array().expect("spine");
    assert_eq!(spine.len(), 2);
    assert_eq!(spine[0]["path"], "OEBPS/Text/ch1.xhtml");
    assert_eq!(
        spine[0]["href"],
        format!("/book/{}/OEBPS/Text/ch1.xhtml", fx.id)
    );
    assert_eq!(spine[1]["path"], "OEBPS/Text/ch 2.xhtml");
    assert_eq!(
        spine[1]["href"],
        format!("/book/{}/OEBPS/Text/ch%202.xhtml", fx.id)
    );
    // O href do spine é servido tal como vem.
    let href = spine[1]["href"].as_str().unwrap().to_string();
    assert_eq!(get(&mut server, &href).status, 200);
    assert_eq!(spine[0]["size"], CHAPTER_1.len());
    let toc = book["toc"].as_array().expect("toc");
    assert_eq!(toc.len(), 2);
    assert_eq!(toc[0]["label"], "Capítulo 1");
    assert_eq!(toc[0]["spine"], 0);
    assert_eq!(toc[0]["children"][0]["label"], "Seção 1.2");
    assert_eq!(toc[0]["children"][0]["fragment"], "s2");
    assert_eq!(toc[1]["spine"], 1);
    assert!(book["position"].is_null());
    assert_eq!(book["bookmarks"], json!([]));

    // Posição e marcador gravados pelo worker aparecem na API.
    assert!(fx.worker.submit(EpubJob::SavePosition {
        id: fx.id.clone(),
        spine: 1,
        fraction: 0.5,
    }));
    assert!(fx.worker.submit(EpubJob::AddBookmark {
        id: fx.id.clone(),
        spine: 0,
        fraction: 0.25,
        label: "Capítulo 1 — 10%".to_string(),
    }));
    flush(&fx.worker);
    let book = json_body(&get(&mut server, &format!("/api/book/{}", fx.id)));
    assert_eq!(book["position"], json!({"spine": 1, "fraction": 0.5}));
    assert_eq!(book["bookmarks"][0]["label"], "Capítulo 1 — 10%");
    assert_eq!(book["bookmarks"][0]["spine"], 0);
    let library = json_body(&get(&mut server, "/api/library"));
    let expected = (CHAPTER_1.len() as f64 + CHAPTER_2.len() as f64 * 0.5)
        / (CHAPTER_1.len() + CHAPTER_2.len()) as f64;
    let progress = library["books"][0]["progress"].as_f64().expect("progresso");
    assert!((progress - expected).abs() < 1e-9, "{progress} != {expected}");
}

#[test]
fn a_stored_book_that_became_unreadable_answers_with_a_pt_br_error() {
    let fx = fixture();
    let stored = fx.library.join("books").join(format!("{}.epub", fx.id));
    std::fs::write(&stored, drm_epub()).unwrap();
    let mut server = fx.server();
    let response = get(&mut server, &format!("/api/book/{}", fx.id));
    assert_eq!(response.status, 409);
    assert_eq!(
        json_body(&response)["error"],
        "Este livro tem DRM e não pode ser aberto."
    );
    assert_eq!(
        get(&mut server, &format!("/book/{}/OEBPS/Text/ch1.xhtml", fx.id)).status,
        404
    );

    std::fs::write(&stored, b"isto nao e um zip").unwrap();
    let mut server = fx.server();
    let response = get(&mut server, &format!("/api/book/{}", fx.id));
    assert_eq!(response.status, 422);
    assert_eq!(
        json_body(&response)["error"],
        "Este arquivo está corrompido ou não é um EPUB válido."
    );
}

// ------------------------------------------------------- worker e erros

#[test]
fn rejected_books_are_reported_in_pt_br_and_never_enter_the_library() {
    let temp = TempDir::new("rejected");
    let (worker, notices) = worker_in(&temp.path().join("library"));
    let drm = write_file(temp.path(), "protegido.epub", &drm_epub());
    let corrupt = write_file(temp.path(), "quebrado.epub", b"PK\x03\x04 lixo");
    let missing = temp.path().join("sumiu.epub");
    assert!(worker.submit(EpubJob::Add {
        paths: vec![drm, corrupt, missing],
        open: true,
    }));
    flush(&worker);
    let EpubNotice::Added {
        books,
        failures,
        open,
    } = notices.recv().unwrap()
    else {
        panic!("aviso inesperado");
    };
    assert!(open);
    assert!(books.is_empty());
    assert_eq!(
        failures,
        vec![
            AddFailure {
                file: "protegido.epub".into(),
                message: "Este livro tem DRM e não pode ser aberto.".into(),
            },
            AddFailure {
                file: "quebrado.epub".into(),
                message: "Este arquivo está corrompido ou não é um EPUB válido.".into(),
            },
            AddFailure {
                file: "sumiu.epub".into(),
                message: failures[2].message.clone(),
            },
        ]
    );
    assert!(failures[2].message.starts_with("Não foi possível ler o arquivo"));
    let mut server = EpubServer::new(worker.shared());
    assert_eq!(json_body(&get(&mut server, "/api/library"))["books"], json!([]));
    let status = EpubNotice::Added {
        books,
        failures,
        open,
    }
    .status_line()
    .expect("linha para a Home");
    assert!(status.contains("“protegido.epub”: Este livro tem DRM e não pode ser aberto."));

    assert_eq!(
        library_error_message(&LibraryError::TooLarge { size: 9, limit: 1 }),
        "Este livro é grande demais para a biblioteca e não pode ser aberto."
    );
    assert_eq!(
        epub_error_message(&EpubError::Limit {
            kind: neural_core::LimitKind::TotalSize,
            value: 2,
            limit: 1,
        }),
        "Este livro é grande demais para ser aberto com segurança."
    );
}

// ------------------------------------------------------------------ IPC

#[test]
fn ipc_parser_accepts_only_the_closed_set_from_our_two_pages() {
    let reader = |body: &str| parse_epub_ipc(READER_SOURCE, body);
    let library = |body: &str| parse_epub_ipc(LIBRARY_SOURCE, body);
    assert_eq!(
        reader(&format!(
            r#"{{"t":"savePosition","id":"{SOME_ID}","spine":3,"fraction":0.25}}"#
        )),
        Some(EpubIpc::SavePosition {
            id: SOME_ID.into(),
            spine: 3,
            fraction: 0.25
        })
    );
    assert_eq!(
        reader(&format!(r#"{{"t":"opened","id":"{SOME_ID}"}}"#)),
        Some(EpubIpc::Opened { id: SOME_ID.into() })
    );
    assert_eq!(reader(r#"{"t":"addBooks"}"#), Some(EpubIpc::AddBooks));
    assert_eq!(library(r#"{"t":"addBooks"}"#), Some(EpubIpc::AddBooks));
    assert_eq!(library(r#"{"t":"close"}"#), Some(EpubIpc::Close));
    assert_eq!(
        library(&format!(r#"{{"t":"removeBook","id":"{SOME_ID}"}}"#)),
        Some(EpubIpc::RemoveBook { id: SOME_ID.into() })
    );
    assert_eq!(
        reader(&format!(
            r#"{{"t":"addBookmark","id":"{SOME_ID}","spine":0,"fraction":1,"label":"Cap. 1 — 5%"}}"#
        )),
        Some(EpubIpc::AddBookmark {
            id: SOME_ID.into(),
            spine: 0,
            fraction: 1.0,
            label: "Cap. 1 — 5%".into()
        })
    );
    assert_eq!(
        reader(&format!(
            r#"{{"t":"removeBookmark","id":"{SOME_ID}","bookmark":7}}"#
        )),
        Some(EpubIpc::RemoveBookmark {
            id: SOME_ID.into(),
            bookmark: 7
        })
    );
    assert_eq!(
        reader(r#"{"t":"openExternal","url":"https://example.com/a?b=1#c"}"#),
        Some(EpubIpc::OpenExternal {
            url: "https://example.com/a?b=1#c".into()
        })
    );

    // Origem e página: só as nossas duas páginas, na nossa origem.
    let save = format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":3,"fraction":0.25}}"#);
    for source in [
        "https://neuralia-epub.localhost/reader.html",
        "http://neuralia-epub.localhost:8080/reader.html",
        "http://neuralia-epub.localhost.evil.com/reader.html",
        "http://evil.com/reader.html",
        "http://user@neuralia-epub.localhost/reader.html",
        "http://neuralia-pdf.localhost/reader.html",
        "http://neuralia-epub.localhost/book/0123456789abcdef/OEBPS/x.xhtml",
        "http://neuralia-epub.localhost/reader.htm",
        "neuralia-epub://localhost/reader.html",
        "about:blank",
        "",
    ] {
        assert_eq!(parse_epub_ipc(source, &save), None, "{source}");
    }
    // Cada mensagem só da página que a pode mandar.
    assert_eq!(library(&save), None);
    assert_eq!(
        reader(&format!(r#"{{"t":"removeBook","id":"{SOME_ID}"}}"#)),
        None
    );
    assert_eq!(reader(r#"{"t":"close"}"#), None);
    assert_eq!(
        library(r#"{"t":"openExternal","url":"https://example.com/"}"#),
        None
    );

    // Forma exata: chaves a mais, tipos errados, números fora do lugar.
    for body in [
        String::new(),
        "null".into(),
        "[]".into(),
        r#""addBooks""#.into(),
        r#"{"t":"addBooks","x":1}"#.into(),
        r#"{"t":"AddBooks"}"#.into(),
        r#"{"t":"exec","code":"1"}"#.into(),
        r#"{"type":"addBooks"}"#.into(),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":3}}"#),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":-1,"fraction":0.1}}"#),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":1.5,"fraction":0.1}}"#),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":100001,"fraction":0.1}}"#),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":"1","fraction":0.1}}"#),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":1,"fraction":1.5}}"#),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":1,"fraction":-0.1}}"#),
        format!(r#"{{"t":"savePosition","id":"{SOME_ID}","spine":1,"fraction":"0.5"}}"#),
        r#"{"t":"savePosition","id":"0123456789ABCDEF","spine":1,"fraction":0.5}"#.into(),
        r#"{"t":"savePosition","id":"0123","spine":1,"fraction":0.5}"#.into(),
        r#"{"t":"savePosition","id":"../../library","spine":1,"fraction":0.5}"#.into(),
        format!(r#"{{"t":"removeBookmark","id":"{SOME_ID}","bookmark":-2}}"#),
        format!(
            r#"{{"t":"addBookmark","id":"{SOME_ID}","spine":0,"fraction":0.5,"label":"{}"}}"#,
            "x".repeat(MAX_LABEL_CHARS + 1)
        ),
        format!(r#"{{"t":"addBookmark","id":"{SOME_ID}","spine":0,"fraction":0.5,"label":7}}"#),
    ] {
        assert_eq!(reader(&body), None, "{body}");
    }
    // Rótulo no limite passa; corpo acima de 4 KiB nunca é lido.
    assert!(
        reader(&format!(
            r#"{{"t":"addBookmark","id":"{SOME_ID}","spine":0,"fraction":0.5,"label":"{}"}}"#,
            "é".repeat(MAX_LABEL_CHARS)
        ))
        .is_some()
    );
    let padded = format!(
        r#"{{"t":"opened","id":"{SOME_ID}"}}{}"#,
        " ".repeat(EPUB_IPC_MAX_BYTES)
    );
    assert!(padded.len() > EPUB_IPC_MAX_BYTES);
    assert_eq!(reader(&padded), None);
    let just_fits = format!(r#"{{"t":"opened","id":"{SOME_ID}"}}"#);
    let just_fits = format!(
        "{just_fits}{}",
        " ".repeat(EPUB_IPC_MAX_BYTES - just_fits.len())
    );
    assert_eq!(just_fits.len(), EPUB_IPC_MAX_BYTES);
    assert!(reader(&just_fits).is_some());

    // Links externos: só http(s) público.
    for url in [
        "javascript:alert(1)",
        "data:text/html,<script>1</script>",
        "file:///C:/Windows/win.ini",
        "mailto:a@b.c",
        "http://localhost:3000/",
        "http://127.0.0.1/",
        "http://192.168.0.10/admin",
        "http://10.0.0.1/",
        "http://impressora.local/",
        "http://neuralia-epub.localhost/library.html",
        "https://user:pass@example.com/",
        "neuralia:home",
    ] {
        assert_eq!(
            reader(&format!(r#"{{"t":"openExternal","url":{}}}"#, json!(url))),
            None,
            "{url}"
        );
    }
    let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
    assert_eq!(
        reader(&format!(r#"{{"t":"openExternal","url":"{long}"}}"#)),
        None
    );
}

#[test]
fn library_operations_run_through_the_same_ipc_handler() {
    let fx = fixture();
    let reader_source = format!("{EPUB_ORIGIN}/reader.html?book={}", fx.id);
    let id = &fx.id;
    let send = |source: &str, body: String| handle_epub_ipc(source, &body, &fx.worker);

    assert_eq!(
        send(
            &reader_source,
            format!(r#"{{"t":"savePosition","id":"{id}","spine":1,"fraction":0.75}}"#)
        ),
        None
    );
    assert_eq!(
        send(&reader_source, format!(r#"{{"t":"opened","id":"{id}"}}"#)),
        None
    );
    assert_eq!(
        send(
            &reader_source,
            format!(
                r#"{{"t":"addBookmark","id":"{id}","spine":0,"fraction":0.5,"label":"Primeiro"}}"#
            )
        ),
        None
    );
    assert_eq!(
        send(
            &reader_source,
            format!(
                r#"{{"t":"addBookmark","id":"{id}","spine":1,"fraction":0.1,"label":"Segundo"}}"#
            )
        ),
        None
    );
    // Forjadas: outra origem, outra página, forma errada. Nada muda.
    assert_eq!(
        send(
            "http://evil.com/reader.html",
            format!(r#"{{"t":"savePosition","id":"{id}","spine":0,"fraction":0.0}}"#)
        ),
        None
    );
    assert_eq!(
        send(
            LIBRARY_SOURCE,
            format!(r#"{{"t":"savePosition","id":"{id}","spine":0,"fraction":0.0}}"#)
        ),
        None
    );
    assert_eq!(
        send(&reader_source, format!(r#"{{"t":"removeBook","id":"{id}"}}"#)),
        None
    );
    flush(&fx.worker);

    let library = Library::open(&fx.library).expect("reabrir do disco");
    let entry = library.get(id).expect("livro continua");
    let position = entry.position.expect("posição gravada");
    assert_eq!((position.spine_index, position.fraction), (1, 0.75));
    assert!(entry.last_opened_unix.is_some());
    let labels: Vec<_> = entry
        .bookmarks
        .iter()
        .map(|mark| mark.label.as_str())
        .collect();
    assert_eq!(labels, ["Primeiro", "Segundo"]);
    let first = entry.bookmarks[0].id;

    assert_eq!(
        send(
            &reader_source,
            format!(r#"{{"t":"removeBookmark","id":"{id}","bookmark":{first}}}"#)
        ),
        None
    );
    // O que só a interface pode fazer volta para quem chamou.
    assert_eq!(
        send(&reader_source, r#"{"t":"addBooks"}"#.to_string()),
        Some(EpubUiRequest::AddBooks)
    );
    assert_eq!(
        send(
            &reader_source,
            r#"{"t":"openExternal","url":"https://example.com/x"}"#.to_string()
        ),
        Some(EpubUiRequest::OpenExternal(
            "https://example.com/x".to_string()
        ))
    );
    assert_eq!(
        send(LIBRARY_SOURCE, r#"{"t":"close"}"#.to_string()),
        Some(EpubUiRequest::Close)
    );
    flush(&fx.worker);
    let library = Library::open(&fx.library).unwrap();
    let marks = &library.get(id).unwrap().bookmarks;
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].label, "Segundo");

    // Remover pela biblioteca: livro e capa vão para a lixeira.
    assert_eq!(
        send(LIBRARY_SOURCE, format!(r#"{{"t":"removeBook","id":"{id}"}}"#)),
        None
    );
    flush(&fx.worker);
    let library = Library::open(&fx.library).unwrap();
    assert!(library.get(id).is_none());
    assert!(fx.library.join(".trash").join(format!("{id}.epub")).exists());
    let mut notices = Vec::new();
    while let Ok(notice) = fx.notices.try_recv() {
        notices.push(notice);
    }
    assert!(notices.contains(&EpubNotice::Removed {
        id: id.clone(),
        title: "O Livro de Teste".into()
    }));
    assert!(notices.contains(&EpubNotice::Bookmarks { id: id.clone() }));
    // O servidor já não o conhece.
    let mut server = fx.server();
    assert_eq!(get(&mut server, &format!("/api/book/{id}")).status, 404);
}

// ------------------------------------------ servidor na thread e 503

#[test]
fn the_server_thread_answers_every_request_even_when_it_cannot_serve() {
    let fx = fixture();
    let server = spawn_epub_server(fx.worker.shared()).expect("servidor");
    let (sender, receiver) = mpsc::channel();
    for path in ["/reader.html".to_string(), format!("/api/book/{}", fx.id)] {
        let sender = sender.clone();
        dispatch_epub_request(
            &server,
            ServeJob {
                method: "GET".into(),
                path,
                reply: Box::new(move |response| {
                    let _ = sender.send(response.status);
                }),
            },
        );
    }
    assert_eq!(receiver.recv().unwrap(), 200);
    assert_eq!(receiver.recv().unwrap(), 200);

    // Servidor morto: a resposta é 503 na hora, nunca um pedido pendurado.
    let (dead, gone) = sync_channel::<ServeJob>(1);
    drop(gone);
    let (sender, receiver) = mpsc::channel();
    dispatch_epub_request(
        &dead,
        ServeJob {
            method: "GET".into(),
            path: "/reader.html".into(),
            reply: Box::new(move |response| {
                let _ = sender.send(response.status);
            }),
        },
    );
    assert_eq!(receiver.try_recv(), Ok(503));
}

// ---------------------------------------------- navegação, drop, diálogo

#[test]
fn the_epub_webview_navigates_only_to_its_two_pages() {
    for allowed in [
        "http://neuralia-epub.localhost/library.html",
        "http://neuralia-epub.localhost/reader.html?book=0123456789abcdef",
        "about:blank",
    ] {
        assert!(epub_navigation_allowed(allowed), "{allowed}");
    }
    for denied in [
        "https://example.com/",
        "http://neuralia-epub.localhost/book/0123456789abcdef/OEBPS/Text/ch1.xhtml",
        "http://neuralia-epub.localhost/api/library",
        "http://neuralia-epub.localhost:81/library.html",
        "http://neuralia-epub.localhost.evil.com/library.html",
        "http://neuralia-pdf.localhost/viewer.html",
        "file:///C:/library.html",
        "javascript:alert(1)",
        "neuralia:home",
    ] {
        assert!(!epub_navigation_allowed(denied), "{denied}");
    }
    assert_eq!(
        reader_url("0123456789abcdef").as_deref(),
        Some("http://neuralia-epub.localhost/reader.html?book=0123456789abcdef")
    );
    assert_eq!(reader_url("../x"), None);
    assert!(epub_navigation_allowed(&library_url()));
}

#[test]
fn dropped_files_add_and_open_only_epubs() {
    let job = epub_drop_job(vec![
        PathBuf::from(r"C:\livros\um.epub"),
        PathBuf::from(r"C:\livros\foto.jpg"),
        PathBuf::from(r"C:\livros\DOIS.EPUB"),
        PathBuf::from(r"C:\livros\tres.epub.exe"),
        PathBuf::from(r"C:\livros\sem-extensao"),
        PathBuf::from(r"C:\livros\pasta"),
    ]);
    match job {
        Some(EpubJob::Add { paths, open }) => {
            assert!(open, "largar um livro abre-o");
            assert_eq!(
                paths,
                vec![
                    PathBuf::from(r"C:\livros\um.epub"),
                    PathBuf::from(r"C:\livros\DOIS.EPUB")
                ]
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        epub_drop_job(vec![
            PathBuf::from("a.pdf"),
            PathBuf::from("b.mobi"),
            PathBuf::from("c.azw3")
        ])
        .is_none()
    );
    assert!(epub_drop_job(Vec::new()).is_none());
}

#[test]
fn the_dialog_filter_and_multiselect_buffer_are_well_formed() {
    let filter = epub_dialog_filter();
    let expected: Vec<u16> = "Livros EPUB (*.epub)\0*.epub\0\0".encode_utf16().collect();
    assert_eq!(filter, expected);
    assert_eq!(filter.iter().filter(|unit| **unit == 0).count(), 3);

    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().collect() };
    assert_eq!(
        parse_dialog_selection(&wide("C:\\livros\\um.epub\0\0")),
        vec![PathBuf::from("C:\\livros\\um.epub")]
    );
    assert_eq!(
        parse_dialog_selection(&wide("C:\\livros\0um.epub\0Dois Livros.epub\0\0lixo")),
        vec![
            PathBuf::from("C:\\livros").join("um.epub"),
            PathBuf::from("C:\\livros").join("Dois Livros.epub"),
        ]
    );
    // Um "nome" com separador ou `..` não sai da pasta escolhida.
    assert_eq!(
        parse_dialog_selection(&wide("C:\\livros\0..\0..\\x.epub\0a/b.epub\0ok.epub\0\0")),
        vec![PathBuf::from("C:\\livros").join("ok.epub")]
    );
    assert!(parse_dialog_selection(&wide("\0\0")).is_empty());
    assert!(parse_dialog_selection(&[]).is_empty());
}

#[test]
fn the_notice_for_the_page_is_json_the_script_only_calls_our_hook() {
    let notice = EpubNotice::Added {
        books: vec![AddedBook {
            id: SOME_ID.into(),
            title: "</script><script>alert(1)</script>".into(),
        }],
        failures: vec![AddFailure {
            file: "a\"b.epub".into(),
            message: "Este livro tem DRM e não pode ser aberto.".into(),
        }],
        open: false,
    };
    let script = notice_script(&notice);
    let prefix = "window.neuraliaEpubNotice && window.neuraliaEpubNotice(";
    assert!(script.starts_with(prefix) && script.ends_with(");"));
    let payload = &script[prefix.len()..script.len() - 2];
    let value: Value = serde_json::from_str(payload).expect("argumento é JSON");
    assert_eq!(value["kind"], "added");
    assert_eq!(value["ids"], json!([SOME_ID]));
    assert_eq!(value["errors"][0]["file"], "a\"b.epub");
}

#[test]
fn progress_weights_each_document_by_its_size() {
    let position = |spine_index, fraction| Position {
        spine_index,
        fraction,
        updated_unix: 0,
    };
    let sizes = [100u64, 300, 600];
    assert_eq!(book_progress(Some(&sizes), 3, position(0, 0.0)), 0.0);
    assert!((book_progress(Some(&sizes), 3, position(1, 0.5)) - 0.25).abs() < 1e-12);
    assert!((book_progress(Some(&sizes), 3, position(2, 1.0)) - 1.0).abs() < 1e-12);
    // Sem tamanhos, cada documento vale o mesmo.
    assert!((book_progress(None, 4, position(1, 0.5)) - 0.375).abs() < 1e-12);
    assert_eq!(book_progress(None, 0, position(0, 0.5)), 0.0);
    assert_eq!(book_progress(Some(&sizes), 3, position(1, f64::NAN)), 0.1);
}

mod js;
