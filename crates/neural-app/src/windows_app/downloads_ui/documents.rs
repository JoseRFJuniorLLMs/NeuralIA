use super::*;
use crate::epub_app::{EpubJob, EpubNotice, EpubRuntime, handle_epub_ipc, reader_url};
use crate::windows_app::app::pages::{epub_http_response, epub_serve_job, serve_pdf_asset};
use std::io::Read;
use std::sync::{Arc, Mutex};

pub(in crate::windows_app) enum PreparedDocument {
    Pdf(Vec<u8>),
    Text(String),
    Html(String),
    Image {
        html: String,
        bytes: Vec<u8>,
        mime: &'static str,
    },
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
            Self::Html(_) => "HTML",
            Self::Image { .. } => "Imagem",
            Self::Epub { .. } => "EPUB",
        })
    }
}

pub(in crate::windows_app) fn is_internal_document(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| {
            [
                "pdf", "epub", "txt", "html", "htm", "xhtml", "png", "jpg", "jpeg", "gif", "webp",
                "bmp",
            ]
            .iter()
            .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
}

/// HTML is accepted only by the read-only preview, never by ShellExecute.
/// Keep the same filename checks as ordinary downloads, including ADS/bidi.
pub(in crate::windows_app) fn internal_document_name_allowed(path: &Path) -> bool {
    is_internal_document(path)
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                neural_core::file_risk::classify_download_name(name)
                    == neural_core::file_risk::RiskClass::Safe
            })
}

pub(in crate::windows_app) fn internal_document_target_allowed(path: &Path) -> bool {
    if !internal_document_name_allowed(path) {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = Vec::new();
    file.take(neural_core::file_risk::SNIFF_HEAD_BYTES)
        .read_to_end(&mut head)
        .is_ok()
        && document_bytes_safe(&head)
}

const TEXT_MAX_BYTES: usize = 8 * 1024 * 1024;
const HTML_MAX_BYTES: usize = 2 * 1024 * 1024;
const IMAGE_MAX_BYTES: usize = 32 * 1024 * 1024;
const PREVIEW_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:; script-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; connect-src 'none'";

fn document_title(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn article_preview_html(article: &neural_core::ReaderArticle) -> String {
    neural_core::reader_html(article).replace("</head>",
        "<style>pre{white-space:pre-wrap;overflow-wrap:anywhere}.top a,.source{display:none}</style></head>")
}

fn text_preview_html(path: &Path, text: String) -> String {
    article_preview_html(&neural_core::ReaderArticle {
        source_url: String::new(),
        title: document_title(path),
        byline: None,
        excerpt: None,
        blocks: vec![neural_core::ReaderBlock::Code(text)],
    })
}

fn image_mime(ext: &str, bytes: &[u8]) -> Result<&'static str, String> {
    // This signature parser does not decompress the image. Chromium renders
    // the immutable, bounded snapshot with an explicit MIME and nosniff.
    let format =
        image::guess_format(bytes).map_err(|_| "O arquivo não contém uma imagem válida.")?;
    match (ext.to_ascii_lowercase().as_str(), format) {
        ("png", image::ImageFormat::Png) => Ok("image/png"),
        ("jpg" | "jpeg", image::ImageFormat::Jpeg) => Ok("image/jpeg"),
        ("gif", image::ImageFormat::Gif) => Ok("image/gif"),
        ("webp", image::ImageFormat::WebP) => Ok("image/webp"),
        ("bmp", image::ImageFormat::Bmp) => Ok("image/bmp"),
        _ => Err("A imagem não corresponde à extensão do arquivo.".into()),
    }
}

fn image_preview_html(path: &Path) -> String {
    let title = neural_core::render::escape_html(&document_title(path)).into_owned();
    format!(
        r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title><style>body{{margin:0;padding:20px;font-family:Segoe UI,system-ui,sans-serif;color:#eee;background:#181b20}}h1{{font-size:18px;overflow-wrap:anywhere}}p{{font-size:13px;color:#bbc2cd}}img{{display:block;max-width:100%;height:auto;margin:auto}}</style></head><body><h1>{title}</h1><p>Pré-visualização da imagem · role para ver o arquivo inteiro</p><img src="/preview-image" alt="{title}"></body></html>"#
    )
}

/// Serve only our generated page and its in-memory raster snapshot. There
/// is no filesystem lookup, network fetch, supplied URL, or active HTML.
fn serve_preview_asset(
    html: &str,
    image: Option<(&[u8], &'static str)>,
    request: &wry::http::Request<Vec<u8>>,
) -> wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    let valid_origin = Url::parse(&request.uri().to_string())
        .is_ok_and(|url| url.origin().ascii_serialization() == PDF_ORIGIN);
    let (status, mime, body) = if !valid_origin {
        (403, "text/plain; charset=utf-8", Vec::new())
    } else if !matches!(
        *request.method(),
        wry::http::Method::GET | wry::http::Method::HEAD
    ) {
        (405, "text/plain; charset=utf-8", Vec::new())
    } else {
        match request.uri().path() {
            "/preview.html" => (200, "text/html; charset=utf-8", html.as_bytes().to_vec()),
            "/preview-image" if image.is_some() => {
                let (bytes, mime) = image.expect("checked image");
                (200, mime, bytes.to_vec())
            }
            _ => (404, "text/plain; charset=utf-8", Vec::new()),
        }
    };
    wry::http::Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Content-Security-Policy", PREVIEW_CSP)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-store")
        .header("Content-Length", body.len())
        .body(std::borrow::Cow::Owned(
            if request.method() == wry::http::Method::HEAD {
                Vec::new()
            } else {
                body
            },
        ))
        .expect("static preview response")
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
    if !document_bytes_safe(&bytes) {
        return Err("O conteúdo do arquivo não é um documento seguro.".into());
    }
    Ok(bytes)
}

fn document_bytes_safe(bytes: &[u8]) -> bool {
    !bytes.starts_with(b"MZ")
        && !matches!(
            neural_core::file_risk::sniff_download(&bytes[..bytes.len().min(4096)]),
            neural_core::file_risk::SniffRisk::Dangerous
        )
}

fn prepare_document(
    path: &Path,
    library: PathBuf,
    notify: Box<dyn Fn(EpubNotice) + Send>,
) -> Result<PreparedDocument, String> {
    if !internal_document_name_allowed(path) {
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
        let bytes = read_bounded(path, TEXT_MAX_BYTES)?;
        let text = decode_text(&bytes)?;
        return Ok(PreparedDocument::Text(text_preview_html(path, text)));
    }
    if ["html", "htm", "xhtml"]
        .iter()
        .any(|kind| ext.eq_ignore_ascii_case(kind))
    {
        let bytes = read_bounded(path, HTML_MAX_BYTES)?;
        let source = decode_text(&bytes)?;
        let url = Url::parse(&format!("{PDF_ORIGIN}/preview.html")).expect("static preview URL");
        let html = match neural_core::reader::extract_article_bounded(
            &url,
            &source,
            Some(Instant::now() + Duration::from_secs(5)),
            &|| false,
        ) {
            Ok(mut article) => {
                article.source_url.clear();
                article_preview_html(&article)
            }
            // Tiny/empty HTML has no article: its escaped source is still
            // useful, and never falls back to executing the original file.
            Err(_) => text_preview_html(path, source),
        };
        return Ok(PreparedDocument::Html(html));
    }
    if ["png", "jpg", "jpeg", "gif", "webp", "bmp"]
        .iter()
        .any(|kind| ext.eq_ignore_ascii_case(kind))
    {
        let bytes = read_bounded(path, IMAGE_MAX_BYTES)?;
        let mime = image_mime(ext, &bytes)?;
        return Ok(PreparedDocument::Image {
            html: image_preview_html(path),
            bytes,
            mime,
        });
    }
    // EPUB's library parser owns the archive bounds, path validation and
    // format checks. Recheck the disk head before handing it to that parser.
    if default_app_target(path).is_none() {
        return Err("Este arquivo não pode ser aberto pelo leitor.".into());
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
        self.show_splash("Preparando a pré-visualização…".into(), 2);
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
            PreparedDocument::Text(html) | PreparedDocument::Html(html) => (
                builder
                    .with_custom_protocol("neuralia-pdf".into(), move |_, request| {
                        serve_preview_asset(&html, None, &request)
                    })
                    .with_url(format!("{PDF_ORIGIN}/preview.html"))
                    .with_initialization_script(SPLIT_SCROLL_RAIL_SCRIPT),
                WebViewHost::Pdf,
            ),
            PreparedDocument::Image { html, bytes, mime } => (
                builder
                    .with_custom_protocol("neuralia-pdf".into(), move |_, request| {
                        serve_preview_asset(&html, Some((&bytes, mime)), &request)
                    })
                    .with_url(format!("{PDF_ORIGIN}/preview.html"))
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
                        if let Some(request) =
                            handle_epub_ipc(&request.uri().to_string(), request.body(), &worker)
                        {
                            let _ = epub_proxy.send_event(UserEvent::DownloadDocumentUi {
                                generation,
                                request,
                            });
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

    #[derive(Default)]
    struct PreviewShell(Vec<PathBuf>);
    impl ShellHost for PreviewShell {
        fn shell_execute(&mut self, _verb: &'static str, file: &Path) -> Result<(), String> {
            self.0.push(file.to_path_buf());
            Ok(())
        }
        fn reveal_in_folder(&mut self, _file: &Path) -> Result<(), String> {
            panic!("preview must not reveal a folder")
        }
    }

    #[test]
    fn download_previews_route_only_validated_files_without_shell() {
        let files = Files::new();
        let mut shell = PreviewShell::default();
        for (name, data) in [
            ("note.txt", b"hello".as_slice()),
            ("report.pdf", b"%PDF-1.7\n".as_slice()),
            ("page.HTML", b"<p>hello</p>".as_slice()),
            (
                "photo.PNG",
                include_bytes!("../../../../../assets/neuralia-home-reader.png").as_slice(),
            ),
        ] {
            let path = files.file(name, data);
            assert!(internal_document_target_allowed(&path), "{name}");
            assert_eq!(
                shell_open_checked(&path, &mut shell),
                OpenOutcome::InternalDocument(path.clone())
            );
            std::fs::write(&path, b"MZ replaced executable").unwrap();
            assert!(!internal_document_target_allowed(&path), "changed {name}");
            assert_eq!(shell_open_checked(&path, &mut shell), OpenOutcome::Refused);
        }
        for name in [
            "CON.html",
            "page.html.exe",
            "page.hta",
            "photo.svg",
            "page\u{202e}.html",
        ] {
            let path = files.0.join(name);
            if name != "CON.html" {
                std::fs::write(&path, b"<p>hello</p>").unwrap();
            }
            assert!(!internal_document_name_allowed(&path), "{name}");
            assert_eq!(
                shell_open_checked(&path, &mut shell),
                OpenOutcome::Refused,
                "{name}"
            );
        }
        assert!(shell.0.is_empty(), "a preview reached ShellExecute");
        let outcome = RecordOutcome::Completed {
            warn: false,
            inspection: Inspection::Checked,
        };
        let html = files.file("page.htm", b"<p>hello</p>");
        assert_eq!(file_actions(outcome, Some(&html)), (true, true));
        let rows = [
            PanelRow {
                id: 1,
                name: "page.htm".into(),
                status: "Concluído".into(),
                percent: None,
                open: true,
                show: true,
                cancel: false,
                tone: "done",
            },
            PanelRow {
                id: 2,
                name: "movie.mp4".into(),
                status: "Concluído".into(),
                percent: None,
                open: true,
                show: true,
                cancel: false,
                tone: "done",
            },
        ];
        let script = downloads_render_script(&rows, false);
        let json = script
            .split_once(".render(")
            .unwrap()
            .1
            .strip_suffix(");")
            .unwrap();
        let data: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(data["rows"][0]["preview"], true);
        assert_eq!(data["rows"][1]["preview"], false);
    }

    #[test]
    fn html_download_preview_preserves_content_and_removes_active_markup() {
        let files = Files::new();
        let path = files.file("article.xhtml", br#"<!doctype html><title>A &amp; B</title><article><h1>Local article</h1><p onclick="alert(1)">Visible content &amp; detail.</p><script>window.pwned=1</script><iframe src="https://evil.invalid"></iframe><img src="file:///secret.txt"><form action="https://evil.invalid"><input value="secret"></form><a href="javascript:alert(1)">Visible link</a></article>"#);
        // The shell still refuses HTML. Its internal reader is the only path.
        assert!(default_app_target(&path).is_none());
        let PreparedDocument::Html(html) = files.prepare(&path).unwrap() else {
            panic!("HTML not internal");
        };
        assert!(html.contains("Visible content &amp; detail."));
        assert!(html.contains("A &amp; B"));
        for dangerous in [
            "window.pwned",
            "onclick=",
            "<iframe",
            "file:///secret",
            "<form",
            "javascript:alert",
        ] {
            assert!(!html.contains(dangerous), "retained {dangerous}");
        }
        let tiny = files.file("tiny.html", b"<div>x</div><script>alert(1)</script>");
        let PreparedDocument::Html(html) = files.prepare(&tiny).unwrap() else {
            panic!("tiny HTML not internal");
        };
        assert!(html.contains("&lt;div&gt;x&lt;/div&gt;"));
        assert!(!html.contains("<script>"));
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend(
            "<p>Ação em UTF-16</p>"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        let encoded = files.file("utf16.htm", &utf16);
        let PreparedDocument::Html(html) = files.prepare(&encoded).unwrap() else {
            panic!("UTF-16 HTML not internal");
        };
        assert!(html.contains("Ação em UTF-16"));
    }

    #[test]
    fn image_download_preview_pins_bytes_mime_and_escapes_the_title() {
        let files = Files::new();
        let original = include_bytes!("../../../../../assets/neuralia-home-reader.png");
        let path = files.file("photo & holiday.PNG", original);
        let PreparedDocument::Image { html, bytes, mime } = files.prepare(&path).unwrap() else {
            panic!("image not internal");
        };
        std::fs::write(&path, b"MZ swapped executable").unwrap();
        assert_eq!(bytes, original);
        assert_eq!(mime, "image/png");
        assert!(html.contains("photo &amp; holiday.PNG"));
        assert!(html.contains("src=\"/preview-image\""));
        assert!(files.prepare(&path).is_err());
        let disguised = files.file("fake.jpg", original);
        assert!(files.prepare(&disguised).is_err());
        let invalid = files.file("fake.png", b"<html><script>alert(1)</script></html>");
        assert!(files.prepare(&invalid).is_err());
        for (ext, signature, mime) in [
            ("jpg", b"\xff\xd8\xff".as_slice(), "image/jpeg"),
            ("gif", b"GIF89a".as_slice(), "image/gif"),
            ("webp", b"RIFF\0\0\0\0WEBP".as_slice(), "image/webp"),
            ("bmp", b"BM".as_slice(), "image/bmp"),
        ] {
            assert_eq!(image_mime(ext, signature).unwrap(), mime);
            assert!(image_mime("png", signature).is_err());
        }
    }

    #[test]
    fn download_preview_read_limits_apply_to_the_real_prepare_path() {
        let files = Files::new();
        for (name, limit) in [
            ("large.html", HTML_MAX_BYTES),
            ("large.txt", TEXT_MAX_BYTES),
            ("large.png", IMAGE_MAX_BYTES),
        ] {
            let path = files.file(name, b"plain");
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len((limit + 1) as u64)
                .unwrap();
            let error = files.prepare(&path).unwrap_err();
            assert!(error.contains("ultrapassa o limite"), "{name}: {error}");
        }
        let path = files.file("exact.txt", b"four");
        assert_eq!(read_bounded(&path, 4).unwrap(), b"four");
        assert!(read_bounded(&path, 3).is_err());
    }

    #[test]
    fn preview_protocol_exposes_only_immutable_assets_at_its_own_origin() {
        let files = Files::new();
        let path = files.file("visible.txt", b"Visible local text");
        let PreparedDocument::Text(html) = files.prepare(&path).unwrap() else {
            panic!("TXT not internal");
        };
        let image = include_bytes!("../../../../../assets/neuralia-home-reader.png");
        let request = |url: &str, method| {
            wry::http::Request::builder()
                .method(method)
                .uri(url)
                .body(Vec::new())
                .unwrap()
        };
        let response = serve_preview_asset(
            &html,
            Some((image, "image/png")),
            &request(&format!("{PDF_ORIGIN}/preview.html"), "GET"),
        );
        assert_eq!(response.status(), 200);
        assert_eq!(response.body().as_ref(), html.as_bytes());
        assert_eq!(
            response.headers()["Content-Type"],
            "text/html; charset=utf-8"
        );
        for directive in [
            "default-src 'none'",
            "script-src 'none'",
            "frame-src 'none'",
            "form-action 'none'",
            "connect-src 'none'",
            "img-src 'self' data:",
        ] {
            assert!(
                response.headers()["Content-Security-Policy"]
                    .to_str()
                    .unwrap()
                    .contains(directive),
                "{directive}"
            );
        }
        assert_eq!(response.headers()["X-Content-Type-Options"], "nosniff");
        assert_eq!(response.headers()["Cache-Control"], "no-store");
        let response = serve_preview_asset(
            &html,
            Some((image, "image/png")),
            &request(&format!("{PDF_ORIGIN}/preview-image"), "GET"),
        );
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["Content-Type"], "image/png");
        assert_eq!(response.body().as_ref(), image);
        let head = serve_preview_asset(
            &html,
            None,
            &request(&format!("{PDF_ORIGIN}/preview.html"), "HEAD"),
        );
        assert_eq!(head.status(), 200);
        assert!(head.body().is_empty());
        assert_eq!(
            head.headers()["Content-Length"].to_str().unwrap(),
            html.len().to_string()
        );
        for (url, method, expected) in [
            (format!("{PDF_ORIGIN}/../../secret.txt"), "GET", 404),
            (format!("{PDF_ORIGIN}/preview-image"), "GET", 404),
            (format!("{PDF_ORIGIN}/preview.html"), "POST", 405),
            ("https://evil.invalid/preview.html".into(), "GET", 403),
            (
                "http://neuralia-pdf.localhost.evil.invalid/preview.html".into(),
                "GET",
                403,
            ),
            (
                "http://neuralia-pdf.localhost:81/preview.html".into(),
                "GET",
                403,
            ),
        ] {
            let response = serve_preview_asset(&html, None, &request(&url, method));
            assert_eq!(response.status(), expected, "{url}");
            assert!(response.body().is_empty(), "{url}");
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
