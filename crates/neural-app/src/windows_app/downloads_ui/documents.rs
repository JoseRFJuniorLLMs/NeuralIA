use super::*;
use crate::epub_app::{EpubJob, EpubNotice, EpubRuntime, handle_epub_ipc, reader_url};
use crate::windows_app::app::pages::{epub_http_response, epub_serve_job, serve_pdf_asset};
use std::io::Read;
use std::sync::{Arc, Mutex};

pub(in crate::windows_app) enum PreparedDocument {
    Pdf(Vec<u8>),
    Text(String),
    Epub {
        runtime: EpubRuntime,
        id: String,
        /// True only when this open created the library entry. Cancelling must
        /// not delete a book the user already owned (same bytes, same id).
        imported: bool,
    },
}

impl std::fmt::Debug for PreparedDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pdf(_) => "PDF",
            Self::Text(_) => "TXT",
            Self::Epub { .. } => "EPUB",
        })
    }
}

pub(in crate::windows_app) fn is_internal_document(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| {
            ["pdf", "epub", "txt"]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("O arquivo ultrapassa o limite do leitor interno.".into());
    }
    // Recheck the bytes we actually read, as the file may have changed since click.
    if bytes.starts_with(b"MZ")
        || matches!(
            neural_core::file_risk::sniff_download(&bytes[..bytes.len().min(4096)]),
            neural_core::file_risk::SniffRisk::Dangerous
        )
    {
        return Err("O conteúdo do arquivo não é um documento seguro.".into());
    }
    Ok(bytes)
}

fn prepare_document(
    path: &Path,
    library: PathBuf,
    notify: Box<dyn Fn(EpubNotice) + Send>,
) -> Result<PreparedDocument, String> {
    if default_app_target(path).is_none() || !is_internal_document(path) {
        return Err("Este arquivo não pode ser aberto pelo leitor.".into());
    }
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if ext.eq_ignore_ascii_case("pdf") {
        let bytes = read_bounded(path, PDF_MAX_BYTES)?;
        if !bytes.starts_with(b"%PDF-") {
            return Err("O arquivo não contém um PDF válido.".into());
        }
        return Ok(PreparedDocument::Pdf(bytes));
    }
    if ext.eq_ignore_ascii_case("txt") {
        let bytes = read_bounded(path, 8 * 1024 * 1024)?;
        let text = decode_text(&bytes)?;
        let article = neural_core::ReaderArticle {
            source_url: String::new(),
            title: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            byline: None,
            excerpt: None,
            blocks: vec![neural_core::ReaderBlock::Code(text)],
        };
        let html = neural_core::reader_html(&article).replace("</head>",
            "<style>pre{white-space:pre-wrap;overflow-wrap:anywhere}.top a{display:none}</style></head>");
        return Ok(PreparedDocument::Text(html));
    }
    // Import uses the existing library worker/parser and file locks. This
    // temporary worker's channels stay alive in the viewer's protocol closures.
    // Snapshot ids first and drop that handle: the worker locks the same dir.
    // If the snapshot fails, `imported` stays false so a cancel cannot delete
    // a book we failed to see.
    let known = neural_core::Library::open(&library).ok().map(|library| {
        library
            .list()
            .iter()
            .map(|book| book.id.clone())
            .collect::<std::collections::HashSet<_>>()
    });
    let (send, receive) = std::sync::mpsc::channel();
    let runtime = EpubRuntime::start(
        library,
        Box::new(move |notice| {
            let _ = send.send(notice.clone());
            notify(notice);
        }),
    )
    .map_err(|error| error.to_string())?;
    if !runtime.worker.submit(EpubJob::Add {
        paths: vec![path.to_path_buf()],
        open: false,
    }) {
        return Err("A biblioteca está ocupada; tente novamente.".into());
    }
    match receive
        .recv_timeout(Duration::from_secs(60))
        .map_err(|error| error.to_string())?
    {
        EpubNotice::Added {
            books, failures, ..
        } => {
            let id = books.first().map(|book| book.id.clone()).ok_or_else(|| {
                failures
                    .first()
                    .map(|failure| failure.message.clone())
                    .unwrap_or_else(|| "Não foi possível ler o EPUB.".into())
            })?;
            let imported = known.as_ref().is_some_and(|ids| !ids.contains(&id));
            Ok(PreparedDocument::Epub {
                runtime,
                id,
                imported,
            })
        }
        EpubNotice::Failed { message } => Err(message),
        _ => Err("Resposta inesperada da biblioteca.".into()),
    }
}

/// Drop a document that never became the visible viewer. A book this open
/// just created is removed; a book that was already in the library stays.
pub(in crate::windows_app) fn release_unshown_document(document: PreparedDocument) {
    if let PreparedDocument::Epub {
        runtime,
        id,
        imported: true,
    } = document
    {
        let _ = runtime.worker.submit(EpubJob::Remove { id });
    }
}

impl App {
    pub(in crate::windows_app) fn open_downloaded_document(&mut self, path: PathBuf) {
        let Some(ticket) = self.side_panel.active_ticket() else {
            return;
        };
        self.downloads_ui.document_generation += 1;
        let generation = self.downloads_ui.document_generation;
        let library = self.config.data_dir.join("library");
        let proxy = self.proxy.clone();
        self.show_splash("Abrindo documento no painel…".into(), 2);
        if let Err(error) = std::thread::Builder::new()
            .name("download-document".into())
            .spawn(move || {
                let notice_proxy = proxy.clone();
                let result = prepare_document(
                    &path,
                    library,
                    Box::new(move |notice| {
                        let _ = notice_proxy
                            .send_event(UserEvent::DownloadDocumentNotice { generation, notice });
                    }),
                );
                let _ = proxy.send_event(UserEvent::DownloadDocumentReady {
                    generation,
                    ticket,
                    result,
                });
            })
        {
            self.show_splash(format!("Não foi possível abrir: {error}"), 4);
        }
    }

    pub(in crate::windows_app) fn downloaded_document_ready(
        &mut self,
        generation: u64,
        ticket: side_panel::PanelTicket,
        result: Result<PreparedDocument, String>,
    ) {
        // Closing/replacing Downloads while a large file imports cancels UI delivery.
        if generation != self.downloads_ui.document_generation
            || self.side_panel.active_ticket() != Some(ticket)
        {
            if let Ok(document) = result {
                release_unshown_document(document);
            }
            return;
        }
        let document = match result {
            Ok(document) => document,
            Err(error) => {
                self.show_splash(error, 5);
                return;
            }
        };
        let Some(bounds) = self.side_panel_rect() else {
            return;
        };
        self.close_side_panel(side_panel::PanelExit::OtherPanel);
        let ticket = self.side_panel.ticket();
        let proxy = self.proxy.clone();
        let builder = themed_webview_builder()
            .with_bounds(document_content_bounds(
                bounds,
                self.window
                    .as_ref()
                    .map_or(1.0, |window| window.scale_factor()),
            ))
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny);
        let (builder, host) = match document {
            PreparedDocument::Pdf(bytes) => {
                let bytes = Arc::new(Mutex::new(bytes));
                (
                    builder
                        .with_custom_protocol("neuralia-pdf".into(), move |_, request| {
                            serve_pdf_asset(&bytes, &request)
                        })
                        .with_url(format!("{PDF_ORIGIN}/viewer.html")),
                    WebViewHost::Pdf,
                )
            }
            PreparedDocument::Text(html) => (
                builder.with_custom_protocol("neuralia-pdf".into(), move |_, request| {
                    let found = request.uri().path() == "/text.html";
                    wry::http::Response::builder().status(if found { 200 } else { 404 })
                        .header("Content-Type", "text/html; charset=utf-8")
                        .header("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; img-src data:; script-src 'none'; base-uri 'none'; object-src 'none'")
                        .body(std::borrow::Cow::Owned(if found { html.as_bytes().to_vec() } else { Vec::new() })).expect("static response")
                }).with_url(format!("{PDF_ORIGIN}/text.html"))
                    .with_initialization_script(SPLIT_SCROLL_RAIL_SCRIPT),
                WebViewHost::Pdf,
            ),
            PreparedDocument::Epub { runtime, id, .. } => {
                let server = runtime.server.clone();
                let worker = runtime.worker.clone();
                let epub_proxy = proxy.clone();
                let builder = builder
                    .with_asynchronous_custom_protocol(
                        EPUB_SCHEME.into(),
                        move |_, request, responder| {
                            let job = epub_serve_job(
                                &request,
                                Box::new(move |response| {
                                    responder.respond(epub_http_response(response))
                                }),
                            );
                            crate::epub_app::dispatch_epub_request(&server, job);
                        },
                    )
                    .with_ipc_handler(move |request| {
                        if let Some(request) = handle_epub_ipc(&request.uri().to_string(), request.body(), &worker) {
                            let _ = epub_proxy.send_event(UserEvent::DownloadDocumentUi { generation, request });
                        }
                        if request.body() == r#"{"action":"close"}"#
                            && let Some(post) = side_panel::PanelPost::parse(ticket, request.body())
                        {
                            let _ = epub_proxy.send_event(UserEvent::Panel(post));
                        }
                    })
                    .with_url(reader_url(&id).expect("library returned valid book id"));
                self.install_document_panel(builder, WebViewHost::Epub, ticket, generation);
                return;
            }
        };
        let builder = builder.with_ipc_handler(move |request| {
            if request.body() == r#"{"action":"close"}"#
                && let Some(post) = side_panel::PanelPost::parse(ticket, request.body())
            {
                let _ = proxy.send_event(UserEvent::Panel(post));
            }
        });
        self.install_document_panel(builder, host, ticket, generation);
    }

    fn install_document_panel(
        &mut self,
        builder: wry::WebViewBuilder<'static>,
        host: WebViewHost,
        ticket: side_panel::PanelTicket,
        generation: u64,
    ) {
        let Some(window) = &self.window else {
            return;
        };
        match self
            .hooked_builder(builder, host, None)
            .build_hooked_as_child(window)
        {
            Ok(view) => {
                let _ = view.focus();
                if let Err(extra) = self.side_panel.open(ticket, view) {
                    drop(extra);
                    return;
                }
                self.downloads_ui.document_view_generation = generation;
                self.downloads_ui.document_ticket = Some(ticket);
                self.fit_comparator_to_panel();
                self.after_panel_change();
                self.request_redraw();
            }
            Err(error) => self.show_splash(format!("Não foi possível abrir o leitor: {error}"), 5),
        }
    }
}

fn decode_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if !(bytes.len() - 2).is_multiple_of(2) {
            return Err("TXT UTF-16 incompleto.".into());
        }
        let little = bytes[0] == 0xff;
        let (pairs, _) = bytes[2..].as_chunks::<2>();
        let units: Vec<u16> = pairs
            .iter()
            .map(|pair| {
                if little {
                    u16::from_le_bytes(*pair)
                } else {
                    u16::from_be_bytes(*pair)
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| "TXT UTF-16 inválido.".into())
    } else {
        String::from_utf8(
            bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(bytes)
                .to_vec(),
        )
        .map_err(|_| "O TXT precisa estar em UTF-8 ou UTF-16.".into())
    }
}

pub(in crate::windows_app) const DOCUMENT_TITLE_HEIGHT: f64 = 38.0;

/// O clique fecha só na faixa da direita, onde está o «Fechar ×».
/// O resto da barra é título: um clique ali não desmonta o leitor.
pub(in crate::windows_app) fn document_chrome_closes(x: i32, width: i32) -> bool {
    if width <= 0 || x < 0 || x >= width {
        return false;
    }
    let close = (width / 3).clamp(48, 160);
    x >= width - close
}

/// Um aviso ou um pedido do leitor só vale para o documento que ainda está
/// na tela. A geração do carregamento pode já ter subido (outro arquivo foi
/// pedido e falhou) sem aposentar este.
pub(in crate::windows_app) fn viewer_still_showing(
    event_generation: u64,
    view_generation: u64,
    document: Option<side_panel::PanelTicket>,
    active: Option<side_panel::PanelTicket>,
) -> bool {
    event_generation == view_generation
        && view_generation != 0
        && document.is_some()
        && document == active
}

pub(in crate::windows_app) fn document_content_bounds(bounds: wry::Rect, scale: f64) -> wry::Rect {
    let position = bounds.position.to_logical::<f64>(scale);
    let size = bounds.size.to_logical::<f64>(scale);
    wry::Rect {
        position: winit::dpi::LogicalPosition::new(position.x, position.y + DOCUMENT_TITLE_HEIGHT)
            .into(),
        size: winit::dpi::LogicalSize::new(
            size.width,
            (size.height - DOCUMENT_TITLE_HEIGHT).max(1.0),
        )
        .into(),
    }
}

pub(in crate::windows_app) struct DocumentChrome {
    hwnd: HWND,
    proxy: EventLoopProxy<UserEvent>,
    generation: u64,
}

impl Drop for DocumentChrome {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn document_chrome_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    reference: usize,
) -> LRESULT {
    match message {
        WM_NCHITTEST => return HTCLIENT as LRESULT,
        WM_LBUTTONUP => {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let x = (lparam & 0xffff) as i16 as i32;
            if document_chrome_closes(x, rect.right) {
                let chrome = &*(reference as *const DocumentChrome);
                let _ = chrome.proxy.send_event(UserEvent::DownloadDocumentUi {
                    generation: chrome.generation,
                    request: crate::epub_app::EpubUiRequest::Close,
                });
            }
            return 0;
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            if !dc.is_null() {
                let mut rect = RECT::default();
                GetClientRect(hwnd, &mut rect);
                let theme = Theme::system();
                let brush = CreateSolidBrush(rgb3(theme.bar_bg));
                FillRect(dc, &rect, brush);
                DeleteObject(brush as _);
                SetBkMode(dc, windows_sys::Win32::Graphics::Gdi::TRANSPARENT as i32);
                SetTextColor(dc, rgb3(theme.fg));
                let text = wide_null("Documento · Fechar ×");
                DrawTextW(
                    dc,
                    text.as_ptr(),
                    -1,
                    &mut rect,
                    windows_sys::Win32::Graphics::Gdi::DT_CENTER
                        | windows_sys::Win32::Graphics::Gdi::DT_VCENTER
                        | windows_sys::Win32::Graphics::Gdi::DT_SINGLELINE,
                );
                EndPaint(hwnd, &paint);
            }
            return 0;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

impl App {
    pub(in crate::windows_app) fn sync_document_chrome(&mut self) {
        if self.downloads_ui.document_ticket.is_none()
            || self.side_panel.active_ticket() != self.downloads_ui.document_ticket
        {
            self.downloads_ui.document_chrome = None;
            return;
        }
        let (Some((_, area)), Some(window)) = (self.docked_right_panel(), &self.window) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        if self
            .downloads_ui
            .document_chrome
            .as_ref()
            .is_some_and(|chrome| {
                chrome.generation != self.downloads_ui.document_view_generation
                    || unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::GetParent(chrome.hwnd)
                    } != owner
            })
        {
            self.downloads_ui.document_chrome = None;
        }
        if self.downloads_ui.document_chrome.is_none() {
            unsafe {
                let hwnd = CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    windows_sys::Win32::UI::WindowsAndMessaging::WS_CHILD,
                    0,
                    0,
                    1,
                    1,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return;
                }
                let chrome = Box::new(DocumentChrome {
                    hwnd,
                    proxy: self.proxy.clone(),
                    generation: self.downloads_ui.document_view_generation,
                });
                if SetWindowSubclass(
                    hwnd,
                    Some(document_chrome_subclass),
                    0x4e44,
                    (&*chrome as *const DocumentChrome) as usize,
                ) == 0
                {
                    return;
                }
                self.downloads_ui.document_chrome = Some(chrome);
            }
        }
        if let Some(chrome) = &self.downloads_ui.document_chrome {
            let scale = window.scale_factor().max(1.0);
            unsafe {
                SetWindowPos(
                    chrome.hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOP,
                    (area.x * scale).round() as i32,
                    (area.y * scale).round() as i32,
                    (area.width * scale).round() as i32,
                    (DOCUMENT_TITLE_HEIGHT * scale).round() as i32,
                    windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
                        | windows_sys::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW,
                );
                InvalidateRect(chrome.hwnd, std::ptr::null(), 0);
            }
        }
    }
}

#[cfg(test)]
mod gates {
    use super::*;
    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "neuralia-doc-panel-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn file(&self, name: &str, content: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, content).unwrap();
            path
        }
        fn prepare(&self, path: &Path) -> Result<PreparedDocument, String> {
            prepare_document(path, self.0.join("library"), Box::new(|_| {}))
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn internal_documents_validate_real_files_and_preserve_text_as_text() {
        let files = Files::new();
        let pdf = files.file("document.PDF", b"%PDF-1.7\noriginal");
        let PreparedDocument::Pdf(bytes) = files.prepare(&pdf).unwrap() else {
            panic!("PDF not internal");
        };
        std::fs::write(&pdf, b"MZ switched executable").unwrap();
        assert_eq!(bytes, b"%PDF-1.7\noriginal");
        assert!(files.prepare(&pdf).is_err());
        let invalid = files.file("invalid.pdf", b"plain text");
        assert!(files.prepare(&invalid).is_err());
        let script = files.file("script.txt", b"MZ executable");
        assert!(files.prepare(&script).is_err());
        let txt = files.file("note.txt", b"<script>window.pwned=1</script>\n[[id]]");
        let PreparedDocument::Text(html) = files.prepare(&txt).unwrap() else {
            panic!("TXT not internal");
        };
        assert!(!html.contains("<script>window.pwned"));
        assert!(html.contains("&lt;script&gt;window.pwned"));
        assert!(read_bounded(&txt, 4).is_err());
        assert_eq!(decode_text(&[0xff, 0xfe, 0x61, 0, 0xe7, 0]).unwrap(), "aç");
        assert!(decode_text(&[0xff, 0xfe, 0x61]).is_err());
    }

    #[test]
    fn epub_import_and_bookmark_notifications_keep_the_internal_viewer_live() {
        let files = Files::new();
        let path = files.file("book.epub", &crate::epub_app::tests::sample_epub());
        let (send, receive) = std::sync::mpsc::channel();
        let document = prepare_document(
            &path,
            files.0.join("library"),
            Box::new(move |notice| {
                let _ = send.send(notice);
            }),
        )
        .unwrap();
        let PreparedDocument::Epub { runtime, id, .. } = document else {
            panic!("EPUB not internal");
        };
        assert!(reader_url(&id).is_some());
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(20)).unwrap(),
            EpubNotice::Added { .. }
        ));
        assert!(runtime.worker.submit(EpubJob::AddBookmark {
            id: id.clone(),
            spine: 0,
            fraction: 0.3,
            label: "Ideia".into()
        }));
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(20)).unwrap(),
            EpubNotice::Bookmarks { id }
        );
    }

    #[test]
    fn a_cancelled_open_removes_only_the_book_it_just_imported() {
        let files = Files::new();
        let path = files.file("book.epub", &crate::epub_app::tests::sample_epub());
        let library = files.0.join("library");
        let (send, receive) = std::sync::mpsc::channel();
        let first = prepare_document(
            &path,
            library.clone(),
            Box::new(move |notice| {
                let _ = send.send(notice);
            }),
        )
        .unwrap();
        let PreparedDocument::Epub {
            id, imported: true, ..
        } = &first
        else {
            panic!("a new EPUB must be marked imported");
        };
        let removed_id = id.clone();
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(20)).unwrap(),
            EpubNotice::Added { .. }
        ));
        release_unshown_document(first);
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(20)).unwrap(),
            EpubNotice::Removed { id, .. } if id == removed_id
        ));
        assert!(
            neural_core::Library::open(&library)
                .unwrap()
                .list()
                .iter()
                .all(|book| book.id != removed_id)
        );

        let kept = prepare_document(&path, library.clone(), Box::new(|_| {})).unwrap();
        let PreparedDocument::Epub {
            id: kept_id,
            imported: true,
            ..
        } = &kept
        else {
            panic!("re-import after removal is a new book");
        };
        let kept_id = kept_id.clone();
        drop(kept);

        let again = prepare_document(&path, library.clone(), Box::new(|_| {})).unwrap();
        let PreparedDocument::Epub {
            id,
            imported: false,
            ..
        } = &again
        else {
            panic!("the same bytes must reuse the library entry");
        };
        assert_eq!(id, &kept_id);
        release_unshown_document(again);
        assert!(
            neural_core::Library::open(&library)
                .unwrap()
                .list()
                .iter()
                .any(|book| book.id == kept_id)
        );
    }

    #[test]
    fn document_toolbar_keeps_the_viewer_below_native_title_at_every_dpi() {
        for scale in [1.0, 1.5, 2.0] {
            let bounds = wry::Rect {
                position: winit::dpi::PhysicalPosition::new(
                    (400.0 * scale) as i32,
                    (80.0 * scale) as i32,
                )
                .into(),
                size: winit::dpi::PhysicalSize::new((360.0 * scale) as u32, (700.0 * scale) as u32)
                    .into(),
            };
            let content = document_content_bounds(bounds, scale);
            let position = content.position.to_logical::<f64>(scale);
            let size = content.size.to_logical::<f64>(scale);
            assert_eq!(position.x, 400.0);
            assert_eq!(position.y, 80.0 + DOCUMENT_TITLE_HEIGHT);
            assert_eq!(size.height, 700.0 - DOCUMENT_TITLE_HEIGHT);
            assert_eq!(size.width, 360.0);
        }
    }

    #[test]
    fn document_title_click_closes_only_the_close_band() {
        assert!(!document_chrome_closes(10, 360));
        assert!(!document_chrome_closes(180, 360));
        assert!(document_chrome_closes(250, 360));
        assert!(document_chrome_closes(359, 360));
        assert!(!document_chrome_closes(360, 360));
        assert!(!document_chrome_closes(-1, 360));
    }

    #[test]
    fn a_newer_load_does_not_retire_the_visible_document() {
        let ticket = side_panel::PanelTicket(7);
        assert!(viewer_still_showing(4, 4, Some(ticket), Some(ticket)));
        assert!(!viewer_still_showing(5, 4, Some(ticket), Some(ticket)));
        assert!(!viewer_still_showing(
            4,
            4,
            Some(ticket),
            Some(side_panel::PanelTicket(8))
        ));
        assert!(!viewer_still_showing(0, 0, None, None));
    }
}
