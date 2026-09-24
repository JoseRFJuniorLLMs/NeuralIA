//! Gates do JavaScript QUE EMBARCA (`epub_asset`: common.js, reader.js,
//! library.js, reader.html, library.html), corrido no Node com `node:vm` e um
//! DOM falso (`harness.js`). Os cenários (`scenarios.js`) recebem o livro
//! como o `EpubServer` o serve (a mesma API e os mesmos capítulos) e devolvem
//! as mensagens que a página mandou ao nativo; aqui essas mensagens passam
//! pelo mesmo `handle_epub_ipc` do `with_ipc_handler` e o efeito é conferido
//! na biblioteca. Um cenário que falha diz a asserção que falhou.

use super::*;

use std::process::{Command, Stdio};

const HARNESS: &str = include_str!("harness.js");
const SCENARIOS: &str = include_str!("scenarios.js");

/// O harness não cabe numa linha de comando do Windows: vai pelo stdin.
const BOOTSTRAP: &str = "const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8')); \
new Function('require', 'input', input.harness)(require, input);";

fn asset_text(path: &str) -> String {
    let (_, bytes) = epub_asset(path).expect("asset embarcado");
    String::from_utf8(bytes.to_vec()).expect("asset em UTF-8")
}

/// Corre o cenário `name` com `data`; devolve o JSON que ele devolver.
fn scenario(name: &str, data: Value) -> Value {
    use std::io::Write as _;
    let input = json!({
        "harness": HARNESS,
        "scenarios": SCENARIOS,
        "name": name,
        "data": data,
        "assets": {
            "common.js": asset_text("/common.js"),
            "reader.js": asset_text("/reader.js"),
            "library.js": asset_text("/library.js"),
            "reader.html": asset_text(READER_PATH),
            "library.html": asset_text(LIBRARY_PATH),
        },
    });
    let mut child = Command::new("node")
        .arg("-e")
        .arg(BOOTSTRAP)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("os gates do leitor de EPUB precisam do `node` no PATH (o CI já o usa)");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.to_string().as_bytes())
        .expect("escrever o cenário");
    let output = child.wait_with_output().expect("node terminou");
    assert!(
        output.status.success(),
        "cenário {name} falhou: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON do cenário")
}

// ------------------------------------------------------------ o livro

const JS_OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">urn:uuid:neuralia-epub-js-test</dc:identifier>
    <dc:title>O Livro de Teste</dc:title>
    <dc:creator>Ana Autora</dc:creator>
    <dc:language>pt-BR</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="Text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="Text/ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="c3" href="Text/ch3.xhtml" media-type="application/xhtml+xml"/>
    <item id="cover" href="Images/cover.png" media-type="image/png" properties="cover-image"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/></spine>
</package>"#;

const JS_NAV: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Sumário</title></head>
<body><nav epub:type="toc"><ol>
  <li><a href="Text/ch1.xhtml">Capítulo Um</a><ol><li><a href="Text/ch1.xhtml#s2">Seção 1.2</a></li></ol></li>
  <li><a href="Text/ch2.xhtml">Capítulo Dois</a></li>
  <li><a href="Text/ch3.xhtml">Capítulo Três</a></li>
</ol></nav></body></html>"#;

const JS_CH1: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="pt-BR"><head><title>Um</title></head>
<body><h1 id="c1">Capítulo Um</h1><p>A ação começa aqui.</p>
<p id="s2">Seção dois do primeiro capítulo, onde bate um coração.</p>
<p>Um link <a href="https://example.com/pagina"><em>externo</em></a>, um <a href="ch2.xhtml#meio">interno</a>, um <a href="javascript:alert(1)">perigoso</a> e um <a href="data:text/html,oi">dado</a>.</p>
</body></html>"#;

const JS_CH2: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Dois</title></head>
<body><h1>Capítulo Dois</h1><p id="meio">Meio do livro, o coração bate forte. Outra frase aqui!</p></body></html>"#;

const JS_CH3: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Três</title></head>
<body><h1>Capítulo Três</h1><p>Fim da ação, coração tranquilo.</p></body></html>"#;

fn js_epub() -> Vec<u8> {
    Zip::default()
        .add("mimetype", b"application/epub+zip", false)
        .add("META-INF/container.xml", CONTAINER.as_bytes(), true)
        .add("OEBPS/content.opf", JS_OPF.as_bytes(), true)
        .add("OEBPS/nav.xhtml", JS_NAV.as_bytes(), true)
        .add("OEBPS/Text/ch1.xhtml", JS_CH1.as_bytes(), true)
        .add("OEBPS/Text/ch2.xhtml", JS_CH2.as_bytes(), false)
        .add("OEBPS/Text/ch3.xhtml", JS_CH3.as_bytes(), true)
        .add("OEBPS/Images/cover.png", PNG_1X1, false)
        .finish()
}

/// Um EPUB mínimo com título e autor dados, sem capa.
fn plain_epub(title: &str, author: &str) -> Vec<u8> {
    let opf = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">urn:neuralia:{title}</dc:identifier>
    <dc:title>{title}</dc:title><dc:creator>{author}</dc:creator><dc:language>pt-BR</dc:language>
  </metadata>
  <manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#
    );
    let chapter = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>{title}</title></head><body><p>{title}</p></body></html>"#
    );
    Zip::default()
        .add("mimetype", b"application/epub+zip", false)
        .add("META-INF/container.xml", CONTAINER.as_bytes(), true)
        .add("OEBPS/content.opf", opf.as_bytes(), true)
        .add("OEBPS/c1.xhtml", chapter.as_bytes(), true)
        .finish()
}

/// A biblioteca numa pasta temporária, com o livro dos cenários dentro.
struct JsBook {
    _temp: TempDir,
    library: PathBuf,
    worker: EpubWorker,
    id: String,
}

fn js_book() -> JsBook {
    let temp = TempDir::new("js");
    let library = temp.path().join("library");
    let source = write_file(temp.path(), "livro.epub", &js_epub());
    let id = Library::open(&library)
        .and_then(|mut library| library.add_at(&source, 1_000))
        .expect("livro dos cenários")
        .id;
    let (worker, _notices) = worker_in(&library);
    JsBook {
        _temp: temp,
        library,
        worker,
        id,
    }
}

impl JsBook {
    /// O que a página do leitor vê: a API do livro e cada capítulo, tal como
    /// o `EpubServer` os serve.
    fn reader_data(&self) -> Value {
        flush(&self.worker);
        let mut server = EpubServer::new(self.worker.shared());
        let api = format!("/api/book/{}", self.id);
        let response = get(&mut server, &api);
        assert_eq!(response.status, 200);
        let book = json_body(&response);
        let mut routes = Map::new();
        routes.insert(api, json!({ "body": book.clone() }));
        let mut hrefs = Vec::new();
        for item in book["spine"].as_array().expect("spine") {
            let href = item["href"].as_str().expect("href").to_string();
            let chapter = get(&mut server, &href);
            assert_eq!(chapter.status, 200, "{href}");
            assert_eq!(
                header(&chapter.headers(), "Content-Security-Policy"),
                Some(BOOK_CSP)
            );
            routes.insert(
                href.clone(),
                json!({
                    "body": String::from_utf8(chapter.body.to_vec()).expect("capítulo em UTF-8"),
                    "type": chapter.content_type,
                }),
            );
            hrefs.push(href);
        }
        json!({ "id": self.id, "book": book, "routes": routes, "hrefs": hrefs })
    }

    fn reader_source(&self) -> String {
        format!("{EPUB_ORIGIN}{READER_PATH}?book={}", self.id)
    }

    /// Passa as mensagens que a página mandou pelo corpo do
    /// `with_ipc_handler`; devolve o que ficou para a interface.
    fn feed(&self, source: &str, posts: &Value) -> Vec<Option<EpubUiRequest>> {
        let answers = posts
            .as_array()
            .expect("mensagens")
            .iter()
            .map(|post| {
                let body = post.to_string();
                assert!(
                    parse_epub_ipc(source, &body).is_some(),
                    "o parser recusou uma mensagem da página: {body}"
                );
                handle_epub_ipc(source, &body, &self.worker)
            })
            .collect();
        flush(&self.worker);
        answers
    }

    fn entry(&self) -> BookEntry {
        Library::open(&self.library)
            .expect("biblioteca")
            .get(&self.id)
            .cloned()
            .expect("livro")
    }
}

// ------------------------------------------------------------ gates

#[test]
fn shipped_reader_paginates_with_css_columns_and_crosses_chapters() {
    let book = js_book();
    let out = scenario("pagination", book.reader_data());
    assert_eq!(out["moves"], json!([[1200, 0], [2400, 0]]));
    assert_eq!(out["pages"], "Página 2 de 5 do capítulo");
}

#[test]
fn shipped_reader_highlights_the_toc_entry_of_the_current_page() {
    let book = js_book();
    let out = scenario("toc", book.reader_data());
    assert_eq!(
        out["seen"],
        json!([
            ["Capítulo Um", 0],
            ["Seção 1.2", 1],
            ["Seção 1.2", 1],
            ["Capítulo Dois", 2]
        ])
    );
}

#[test]
fn shipped_reader_restores_the_saved_position_and_saves_it_debounced() {
    let book = js_book();
    // A posição que o leitor gravou antes (página 2 de 3 do capítulo 2).
    assert!(book.worker.submit(EpubJob::SavePosition {
        id: book.id.clone(),
        spine: 1,
        fraction: 0.3333,
    }));
    let out = scenario("position", book.reader_data());
    let posts = out["posts"].as_array().expect("mensagens").clone();
    assert_eq!(posts[0], json!({"t": "opened", "id": book.id}));
    assert!(
        book.feed(&book.reader_source(), &out["posts"])
            .iter()
            .all(Option::is_none)
    );
    // O que a página mandou chegou ao disco: a última posição e a abertura.
    let entry = book.entry();
    let position = entry.position.expect("posição");
    assert_eq!((position.spine_index, position.fraction), (1, 0.3333));
    assert!(entry.last_opened_unix.is_some());
}

#[test]
fn shipped_reader_searches_the_whole_book_and_jumps_to_the_hit() {
    let book = js_book();
    let out = scenario("search", book.reader_data());
    let rows = out["rows"].as_array().expect("resultados");
    assert_eq!(rows.len(), 3);
    assert!(
        rows[0][2]
            .as_str()
            .is_some_and(|context| context.contains("bate um coração")),
        "{rows:?}"
    );
}

#[test]
fn shipped_reader_adds_lists_and_removes_bookmarks_through_the_library() {
    let book = js_book();
    let added = scenario("bookmark_add", book.reader_data());
    book.feed(&book.reader_source(), &added["posts"]);
    let entry = book.entry();
    assert_eq!(entry.bookmarks.len(), 1);
    let mark = &entry.bookmarks[0];
    assert_eq!((mark.spine_index, mark.fraction), (0, 0.3333));
    assert!(mark.label.starts_with("Seção 1.2 — "), "{}", mark.label);

    let mut data = book.reader_data();
    data["labels"] = json!([mark.label]);
    data["bookmarkId"] = json!(mark.id);
    data["noticeScript"] = json!(notice_script(&EpubNotice::Bookmarks {
        id: book.id.clone()
    }));
    let listed = scenario("bookmark_list", data);
    book.feed(&book.reader_source(), &listed["posts"]);
    assert!(book.entry().bookmarks.is_empty());
}

#[test]
fn shipped_reader_reads_aloud_with_a_local_voice_one_sentence_at_a_time() {
    let book = js_book();
    let out = scenario("read_aloud", book.reader_data());
    assert_eq!(out["first"], "Capítulo Um");
    assert_eq!(out["voice"], "Microsoft Maria");
}

#[test]
fn shipped_reader_routes_book_links_inside_the_book_or_out_through_the_app() {
    let book = js_book();
    let out = scenario("links", book.reader_data());
    let answers = book.feed(&book.reader_source(), &out["posts"]);
    assert_eq!(
        answers,
        vec![
            None,
            Some(EpubUiRequest::OpenExternal(
                "https://example.com/pagina".to_string()
            )),
        ]
    );
}

#[test]
fn shipped_reader_renders_the_book_in_a_scriptless_sandbox() {
    let book = js_book();
    let out = scenario("sandbox", book.reader_data());
    assert_eq!(out["sandbox"], "allow-same-origin");
}

#[test]
fn shipped_reader_retries_an_xhtml_chapter_the_xml_parser_refused_as_html() {
    let book = js_book();
    let data = book.reader_data();
    let out = scenario("html_fallback", data.clone());
    let retry = format!(
        "{EPUB_ORIGIN}{}?{HTML_FALLBACK_QUERY}",
        data["hrefs"][0].as_str().unwrap()
    );
    assert_eq!(out["loads"][1], retry.as_str());
    // E o servidor responde a esse pedido com o mesmo capítulo, como HTML.
    let mut server = EpubServer::new(book.worker.shared());
    let path = retry.strip_prefix(EPUB_ORIGIN).unwrap();
    let response = get(&mut server, path);
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "text/html");
    assert_eq!(response.body.as_ref(), JS_CH1.as_bytes());
    assert_eq!(
        header(&response.headers(), "Content-Security-Policy"),
        Some(BOOK_CSP)
    );
}

#[test]
fn shipped_reader_turns_to_the_true_last_page_and_leftwards_in_rtl_chapters() {
    let book = js_book();
    let out = scenario("last_page_and_rtl", book.reader_data());
    // A 4.ª página começa em 3 x 1200, não onde o conteúdo deixa de rolar.
    assert_eq!(out["last"], json!([3600, 0]));
    let pages = out["pages"].as_u64().expect("páginas");
    assert_eq!(out["narrow"], json!([(pages - 1) * 800, 0]));
    assert_eq!(
        out["rtl"],
        json!([[-1200, 0], [-2400, 0], [-1200, 0], [-2400, 0]])
    );
}

#[test]
fn shipped_reader_keeps_the_reading_point_across_relayouts_and_mode_switches() {
    let book = js_book();
    let out = scenario("relayout", book.reader_data());
    assert!(
        out["start"]
            .as_str()
            .is_some_and(|label| label.starts_with("Página 3 de ")),
        "{out}"
    );
    assert_eq!(out["back"], out["start"]);
}

#[test]
fn shipped_reader_lays_a_chapter_out_before_its_images_and_keeps_history_flat() {
    let book = js_book();
    let out = scenario("early_layout", book.reader_data());
    assert_eq!(out["historyAdded"], 0);
    assert_eq!(out["how"], json!(["src", "replace", "replace", "replace"]));
}

#[test]
fn shipped_reader_applies_typography_themes_and_fixes_the_book_css() {
    let book = js_book();
    let out = scenario("typography", book.reader_data());
    assert_eq!(out["saved"]["fontSize"], 90);
    assert_eq!(out["saved"]["theme"], "light");
}

#[test]
fn shipped_reader_reads_aloud_with_the_voice_the_person_picks() {
    let book = js_book();
    let out = scenario("voice", book.reader_data());
    assert_eq!(out["voice"], "Microsoft Zira");
}

#[test]
fn shipped_library_searches_sorts_continues_and_removes_with_confirmation() {
    let temp = TempDir::new("js-library");
    let dir = temp.path().join("library");
    let (read_id, agata_id, abelha_id) = {
        let mut library = Library::open(&dir).expect("biblioteca");
        let read = write_file(temp.path(), "lido.epub", &js_epub());
        let agata = write_file(
            temp.path(),
            "agata.epub",
            &plain_epub("Ágata e o Mar", "Zé Ninguém"),
        );
        let abelha = write_file(
            temp.path(),
            "abelha.epub",
            &plain_epub("Abelha Rainha", "Bruno Zanetti"),
        );
        let read = library.add_at(&read, 1_000).expect("lido").id;
        let agata = library.add_at(&agata, 2_000).expect("ágata").id;
        let abelha = library.add_at(&abelha, 3_000).expect("abelha").id;
        library.set_last_opened(&read, 9_000).expect("aberto");
        library
            .set_position(
                &read,
                Position {
                    spine_index: 1,
                    fraction: 0.5,
                    updated_unix: 9_000,
                },
            )
            .expect("posição");
        (read, agata, abelha)
    };
    let (worker, _notices) = worker_in(&dir);
    let mut server = EpubServer::new(worker.shared());
    let library = json_body(&get(&mut server, "/api/library"));
    let progress = library["books"]
        .as_array()
        .and_then(|books| books.iter().find(|book| book["id"] == read_id.as_str()))
        .and_then(|book| book["progress"].as_f64())
        .expect("progresso do livro lido");
    let failure = EpubNotice::Added {
        books: Vec::new(),
        failures: vec![AddFailure {
            file: "protegido.epub".into(),
            message: "Este livro tem DRM e não pode ser aberto.".into(),
        }],
        open: false,
    };
    let data = json!({
        "library": library,
        "routes": { "/api/library": { "body": library } },
        "readId": read_id,
        "readPercent": format!("{}%", (progress * 100.0).round()),
        "agataId": agata_id,
        "failureScript": notice_script(&failure),
    });
    let out = scenario("library", data);
    let posts = out["posts"].as_array().expect("mensagens").clone();
    assert_eq!(
        posts,
        vec![
            json!({"t": "removeBook", "id": abelha_id}),
            json!({"t": "addBooks"}),
            json!({"t": "addBooks"}),
            json!({"t": "close"}),
            json!({"t": "close"}),
        ]
    );
    let answers: Vec<_> = posts
        .iter()
        .map(|post| handle_epub_ipc(LIBRARY_SOURCE, &post.to_string(), &worker))
        .collect();
    assert_eq!(
        answers,
        vec![
            None,
            Some(EpubUiRequest::AddBooks),
            Some(EpubUiRequest::AddBooks),
            Some(EpubUiRequest::Close),
            Some(EpubUiRequest::Close),
        ]
    );
    flush(&worker);
    let library = Library::open(&dir).expect("reabrir");
    assert!(library.get(&abelha_id).is_none(), "o livro confirmado saiu");
    assert!(library.get(&agata_id).is_some() && library.get(&read_id).is_some());
}
