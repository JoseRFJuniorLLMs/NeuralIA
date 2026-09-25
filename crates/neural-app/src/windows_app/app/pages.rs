use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use url::Url;
use windows_sys::Win32::Foundation::HWND;
use wry::http::{Request, Response as HttpResponse};
use wry::{NewWindowResponse, PermissionResponse, WebViewBuilder};

use crate::epub_app::{
    EpubJob, EpubNotice, EpubResponse, EpubRuntime, EpubUiRequest, ServeJob, dispatch_epub_request,
    epub_dialog_filter, epub_drop_job, epub_navigation_allowed, epub_request_target,
    handle_epub_ipc, is_epub_path, library_url, notice_script, parse_dialog_selection, reader_url,
};
use neural_core::{
    MemoryDocument, MemoryKind, MemorySourceKind, ReaderArticle, ReaderBlock, reader_html,
};

use crate::windows_app::{
    AGENT_OBSERVER_SCRIPT, App, COMPARATOR_COLUMNS, DocumentJob, EPUB_SCHEME,
    EXTERNAL_RETURN_BUTTON, HistoryKind, IpcAction, NEURALIA_KEYMAP_SCRIPT, PDF_ORIGIN,
    PDF_VIEWER_CSP, PDF_VIEWER_HTML, PDF_VIEWER_JS, PDFJS_CORE, PDFJS_WORKER, PanelExit,
    READ_ALOUD_SCRIPT, SPLIT_SCROLL_RAIL_SCRIPT, Surface, UserEvent, bind_page_script,
    common_ipc_event, is_view_source_target, local_origin_of, neuralia_action,
    parse_agent_observation, parse_ipc_message, remote_capability, remote_web_target,
    themed_webview_builder, web_media_permission, wide_null, window_hwnd,
};

impl App {
    /// Descarrega o PDF no worker coalescente de documentos.
    pub(in crate::windows_app) fn read_pdf(&mut self, url: Url) {
        let generation = self.next_generation();
        self.destroy_web_surfaces();
        self.surface = Surface::Home;
        self.schedule_home_restoration();
        self.status = Some(format!(
            "A descarregar PDF de {} …",
            url.host_str().unwrap_or("?")
        ));
        self.request_redraw();

        if let Err(error) = self.document.submit(DocumentJob {
            generation,
            url: url.to_string(),
        }) {
            self.show_native_error(format!("PDF: {error}"));
        }
    }

    pub(in crate::windows_app) fn pdf_webview_builder(&self) -> WebViewBuilder<'static> {
        let ipc_proxy = self.proxy.clone();
        let navigation_proxy = self.proxy.clone();
        let bytes = Arc::clone(&self.pdf_bytes);
        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let init_script = bind_page_script(NEURALIA_KEYMAP_SCRIPT, &capability, false);

        themed_webview_builder()
            .with_custom_protocol("neuralia-pdf".to_string(), move |_id, request| {
                serve_pdf_asset(&bytes, &request)
            })
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                if let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                    && let Some(event) = common_ipc_event(action)
                {
                    let _ = ipc_proxy.send_event(event);
                }
            })
            .with_navigation_handler(move |target| {
                if target
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("neuralia:"))
                {
                    return false;
                }
                if is_pdf_internal_target(&target) {
                    return true;
                }
                if remote_web_target(&target, None) {
                    let _ = navigation_proxy.send_event(UserEvent::OpenExternal(target));
                }
                false
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    pub(in crate::windows_app) fn open_pdf(&mut self, url: &str, bytes: Vec<u8>) {
        self.destroy_web_surfaces();
        self.show_omnibox(false);

        if let Ok(mut slot) = self.pdf_bytes.lock() {
            *slot = bytes;
        }

        let result = if let Some(window) = &self.window {
            self.pdf_webview_builder()
                .with_url(format!("{PDF_ORIGIN}/viewer.html"))
                .build(window)
        } else {
            return;
        };

        match result {
            Ok(webview) => {
                let _ = webview.zoom(self.zoom);
                self.webview = Some(webview);
                self.surface = Surface::Pdf;
                self.page_source = Some(url.to_string());
                self.record(HistoryKind::Read, url.to_string(), url.to_string());
                let mut document = MemoryDocument::new(
                    MemoryKind::Source,
                    MemorySourceKind::Pdf,
                    Url::parse(url)
                        .ok()
                        .and_then(|parsed| parsed.path_segments()?.next_back().map(str::to_string))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Documento PDF".to_string()),
                    Some(url.to_string()),
                    format!("Documento PDF aberto no NeuralIA: {url}"),
                );
                if let Some(session) = &self.current_research {
                    document = document.session(session.id.clone());
                }
                self.memory.capture(document);
                self.begin_reading_session(true);
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde abrir o PDF: {error}"));
            }
        }
    }

    /// Arranca (uma vez) o worker da biblioteca de livros e o servidor da
    /// origem `neuralia-epub`. Nada disto corre na thread da interface.
    pub(in crate::windows_app) fn ensure_epub(&mut self) -> bool {
        if self.epub.is_some() {
            return true;
        }
        let proxy = self.proxy.clone();
        let notify = Box::new(move |notice| {
            let _ = proxy.send_event(UserEvent::EpubNotice(notice));
        });
        match EpubRuntime::start(self.config.data_dir.join("library"), notify) {
            Ok(runtime) => {
                self.epub = Some(runtime);
                true
            }
            Err(error) => {
                self.show_native_error(format!(
                    "A biblioteca de livros não pôde ser iniciada: {error}"
                ));
                false
            }
        }
    }

    /// A biblioteca de livros (estilo Calibre). `livros:` na omnibox; o
    /// botão da Home vem depois, pela mão de quem integra.
    pub(in crate::windows_app) fn open_library(&mut self) {
        self.open_epub_page(library_url());
    }

    /// Acrescenta o EPUB à biblioteca (numa thread própria: um livro grande
    /// pode demorar) e abre-o no leitor quando estiver lá.
    pub(in crate::windows_app) fn open_epub(&mut self, path: PathBuf) {
        self.submit_epub_job(EpubJob::Add {
            paths: vec![path],
            open: true,
        });
    }

    pub(in crate::windows_app) fn submit_epub_job(&mut self, job: EpubJob) {
        if !self.ensure_epub() {
            return;
        }
        let adding = match &job {
            EpubJob::Add { paths, .. } => paths.len(),
            _ => 0,
        };
        let submitted = self
            .epub
            .as_ref()
            .is_some_and(|epub| epub.worker.submit(job));
        if submitted && adding > 0 && self.surface == Surface::Home {
            self.status = Some(if adding == 1 {
                "Adicionando o livro à biblioteca…".to_string()
            } else {
                format!("Adicionando {adding} livros à biblioteca…")
            });
            self.request_redraw();
        }
    }

    /// Arquivos largados na janela (ou no WebView dos livros): os `.epub`
    /// entram e abrem, o resto é ignorado.
    pub(in crate::windows_app) fn route_dropped_files(&mut self, paths: Vec<PathBuf>) {
        if let Some(job) = epub_drop_job(paths) {
            self.submit_epub_job(job);
        }
    }

    /// Ctrl+O e `epub:`: o diálogo "Abrir" do Windows, só `*.epub`.
    pub(in crate::windows_app) fn open_epub_dialog(&mut self, open: bool) {
        let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        let paths = pick_epub_files(owner);
        if !paths.is_empty() {
            self.submit_epub_job(EpubJob::Add { paths, open });
        }
    }

    pub(in crate::windows_app) fn open_epub_reader(&mut self, id: &str) {
        if let Some(url) = reader_url(id) {
            self.open_epub_page(url);
        }
    }

    pub(in crate::windows_app) fn open_epub_page(&mut self, url: String) {
        if !self.ensure_epub() {
            return;
        }
        if self.surface == Surface::Epub
            && let Some(webview) = &self.webview
        {
            let _ = webview.load_url(&url);
            return;
        }
        self.close_side_panel(PanelExit::SurfaceChange);
        self.close_service_panel();
        self.next_generation();
        self.destroy_web_surfaces();
        self.show_omnibox(false);
        self.status = None;
        let result = match (&self.window, &self.epub) {
            (Some(window), Some(runtime)) => self
                .epub_webview_builder(runtime)
                .with_url(url)
                .build(window),
            _ => return,
        };
        match result {
            Ok(webview) => {
                let _ = webview.zoom(self.zoom);
                let _ = webview.focus();
                self.webview = Some(webview);
                self.surface = Surface::Epub;
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde abrir os livros: {error}"));
            }
        }
    }

    /// O WebView da biblioteca e do leitor: a origem `neuralia-epub` servida
    /// fora da thread da interface, o IPC fechado das páginas EPUB (nunca o
    /// `ipc.rs` nem a capability das páginas remotas), navegação de topo só
    /// para as duas páginas, sem popups, downloads nem permissões.
    pub(in crate::windows_app) fn epub_webview_builder(
        &self,
        runtime: &EpubRuntime,
    ) -> WebViewBuilder<'static> {
        let server = runtime.server.clone();
        let worker = runtime.worker.clone();
        let ipc_proxy = self.proxy.clone();
        let drop_proxy = self.proxy.clone();
        themed_webview_builder()
            .with_asynchronous_custom_protocol(
                EPUB_SCHEME.to_string(),
                move |_id, request, responder| {
                    let job = epub_serve_job(
                        &request,
                        Box::new(move |response| {
                            responder.respond(epub_http_response(response));
                        }),
                    );
                    dispatch_epub_request(&server, job);
                },
            )
            .with_ipc_handler(move |request| {
                let source = request.uri().to_string();
                if let Some(ui) = handle_epub_ipc(&source, request.body(), &worker) {
                    let _ = ipc_proxy.send_event(UserEvent::EpubUi(ui));
                }
            })
            .with_navigation_handler(|target| epub_navigation_allowed(&target))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_drag_drop_handler(move |event| {
                if let wry::DragDropEvent::Drop { paths, .. } = event {
                    let _ = drop_proxy.send_event(UserEvent::EpubDropped(paths));
                }
                true
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    pub(in crate::windows_app) fn handle_epub_notice(&mut self, notice: EpubNotice) {
        // A página de livros aberta recebe sempre o aviso (lista, marcadores,
        // erros); o resto decide-o `plan_epub_notice`.
        if self.surface == Surface::Epub
            && let Some(webview) = &self.webview
        {
            let _ = webview.evaluate_script(&notice_script(&notice));
        }
        match plan_epub_notice(&notice, self.surface) {
            EpubNoticePlan::OpenReader(id) => self.open_epub_reader(&id),
            EpubNoticePlan::OpenLibrary(line) => {
                self.open_library();
                if let Some(line) = line {
                    self.show_splash(line, 8);
                }
            }
            EpubNoticePlan::HomeStatus(line) => {
                self.status = Some(line);
                self.request_redraw();
            }
            EpubNoticePlan::Splash(line) => self.show_splash(line, 8),
            EpubNoticePlan::ClearHomeStatus => {
                self.status = None;
                self.request_redraw();
            }
            EpubNoticePlan::PageOnly | EpubNoticePlan::Nothing => {}
        }
    }

    pub(in crate::windows_app) fn handle_epub_ui(&mut self, request: EpubUiRequest) {
        // Um pedido que chega depois de a página ter saído já não vale.
        if self.surface != Surface::Epub {
            return;
        }
        match request {
            EpubUiRequest::AddBooks => self.open_epub_dialog(false),
            EpubUiRequest::OpenExternal(url) => self.web(url),
            EpubUiRequest::Close => self.show_home(),
        }
    }

    pub(in crate::windows_app) fn capture_reader_memory(&mut self, article: &ReaderArticle) {
        let body = reader_article_memory_text(article);
        let mut document = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Reader,
            article.title.clone(),
            Some(article.source_url.clone()),
            body.clone(),
        );

        if let Some(session) = &mut self.current_research {
            document = document.session(session.id.clone());
            let memory_id = document.id.clone();
            session.add_source(
                None,
                article.title.clone(),
                article.source_url.clone(),
                Some(memory_id),
                body,
            );
            self.memory.save_session(session.clone());
        }
        self.memory.capture(document);
    }

    pub(in crate::windows_app) fn reader_webview_builder(&self) -> WebViewBuilder<'static> {
        let navigation_proxy = self.proxy.clone();
        let ipc_proxy = self.proxy.clone();
        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let init_script = Self::reader_init_script(&capability);

        themed_webview_builder()
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                if let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                    && let Some(event) = common_ipc_event(action)
                {
                    let _ = ipc_proxy.send_event(event);
                }
            })
            .with_navigation_handler(move |target| {
                if target.starts_with("about:blank") {
                    return true;
                }

                let Ok(action_url) = Url::parse(&target) else {
                    return false;
                };
                if action_url.scheme() != "neuralia" {
                    return false;
                }

                if let Some(event) = neuralia_action(&target) {
                    let _ = navigation_proxy.send_event(event);
                    return false;
                }

                match action_url.path().trim_matches('/') {
                    "home" => {
                        let _ = navigation_proxy.send_event(UserEvent::HomeRequested);
                    }
                    "web" => {
                        if let Some((_, value)) =
                            action_url.query_pairs().find(|(key, _)| key == "url")
                            && neural_core::validate_web_url(value.as_ref()).is_ok()
                        {
                            let _ = navigation_proxy
                                .send_event(UserEvent::OpenExternal(value.into_owned()));
                        }
                    }
                    _ => {}
                }

                false
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    /// O que o Modo Leitura injeta no document-created: o mapa de teclas (com
    /// a barra de selecao), o rail de secoes e a leitura em voz alta
    /// (SPEC-0110), que se liga sozinha ao artigo. O HTML do Reader tem
    /// `script-src 'none'`; os initialization scripts do WebView2 correm na
    /// mesma.
    pub(in crate::windows_app) fn reader_init_script(capability: &str) -> String {
        bind_page_script(
            &format!("{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}\n{READ_ALOUD_SCRIPT}"),
            capability,
            false,
        )
    }

    pub(in crate::windows_app) fn external_webview_builder(
        &self,
        local_origin: Option<String>,
        agent_enabled: bool,
    ) -> WebViewBuilder<'static> {
        let ipc_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let nav_origin = local_origin.clone();
        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let init_script = external_init_script(&capability, agent_enabled);

        themed_webview_builder()
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                if let Some(event) = external_ipc_event(action, agent_enabled) {
                    let _ = ipc_proxy.send_event(event);
                }
            })
            .with_navigation_handler(move |target| {
                if target
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("neuralia:"))
                {
                    return false;
                }
                remote_web_target(&target, nav_origin.as_deref())
                    || is_view_source_target(&target, nav_origin.as_deref())
            })
            .with_new_window_req_handler(move |target, _features| {
                if let Some(event) = external_new_window_event(target, local_origin.as_deref()) {
                    let _ = new_window_proxy.send_event(event);
                }
                NewWindowResponse::Deny
            })
            .with_permission_handler(move |kind| web_media_permission(kind, !agent_enabled))
            .with_focused(true)
    }

    pub(in crate::windows_app) fn open_external(&mut self, url: &str) {
        self.destroy_web_surfaces();
        self.show_omnibox(false);
        let is_pdf = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase()
            .ends_with(".pdf");

        // A autorizacao vale para a origem escrita, nao para a rede local.
        let local_origin = Url::parse(url).ok().as_ref().and_then(local_origin_of);
        let allow_local = local_origin.is_some();
        let result = if let Some(window) = &self.window {
            self.external_webview_builder(local_origin, false)
                .with_url(url)
                .build(window)
        } else {
            return;
        };

        match result {
            Ok(webview) => {
                let _ = webview.zoom(self.zoom);
                self.webview = Some(webview);
                self.surface = Surface::External;
                if !allow_local {
                    let title = Url::parse(url)
                        .ok()
                        .and_then(|parsed| parsed.host_str().map(str::to_string))
                        .unwrap_or_else(|| "Página Web".to_string());
                    let mut document = MemoryDocument::new(
                        MemoryKind::Source,
                        MemorySourceKind::Web,
                        title,
                        Some(url.to_string()),
                        url.to_string(),
                    );
                    if let Some(session) = &self.current_research {
                        document = document.session(session.id.clone());
                    }
                    self.memory.capture(document);
                }
                self.schedule_gmail_probe(4);
                self.begin_reading_session(is_pdf);
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde abrir a página: {error}"));
            }
        }
    }

    pub(in crate::windows_app) fn open_reader(&mut self, article: &ReaderArticle) {
        self.destroy_web_surfaces();
        self.show_omnibox(false);
        let html = reader_html(article);

        let result = if let Some(window) = &self.window {
            self.reader_webview_builder().with_html(html).build(window)
        } else {
            return;
        };

        match result {
            Ok(webview) => {
                let _ = webview.zoom(self.zoom);
                self.webview = Some(webview);
                self.surface = Surface::Reader;
                self.page_source = Some(article.source_url.clone());
                self.begin_reading_session(false);
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde exibir o Reader: {error}"));
            }
        }
    }
}

/// Percent-encoding para dentro de um literal JS que a pagina le com
/// `decodeURIComponent`. O serializador de formularios escreve o espaco como
/// `+` e `decodeURIComponent` nao o desfaz: um `+` aqui era um `+` escrito no
/// campo, e um nome com espacos nunca batia com o do DOM -- o guard do script
/// desistia em silencio e a accao do agente nao acontecia. Um `+` literal ja
/// chega como `%2B`, por isso os que sobram sao todos espacos.
pub(in crate::windows_app) fn js_percent(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// Tecto do texto de um artigo que entra na memoria semantica.
pub(in crate::windows_app) const READER_MEMORY_MAX_BYTES: usize = 512 * 1024;

pub(in crate::windows_app) fn reader_article_memory_text(article: &ReaderArticle) -> String {
    let mut output = String::new();
    if let Some(excerpt) = &article.excerpt {
        output.push_str(excerpt);
        output.push_str("\n\n");
    }
    for block in &article.blocks {
        let text = match block {
            ReaderBlock::Heading { text, .. }
            | ReaderBlock::Paragraph(text)
            | ReaderBlock::Quote(text)
            | ReaderBlock::Code(text)
            | ReaderBlock::ListItem(text) => text,
        };
        if !text.trim().is_empty() {
            output.push_str(text.trim());
            output.push_str("\n\n");
        }
    }
    // `String::truncate` num indice que cai a meio de um UTF-8 entra em
    // panico -- e um artigo longo com acentos e o caso normal, nao o raro.
    // Recua-se ate a fronteira de char anterior antes de cortar.
    if output.len() > READER_MEMORY_MAX_BYTES {
        let mut cut = READER_MEMORY_MAX_BYTES;
        while cut > 0 && !output.is_char_boundary(cut) {
            cut -= 1;
        }
        output.truncate(cut);
    }
    output
}

/// O que a interface faz com um aviso do worker de livros, conforme a
/// superfície em que a pessoa está quando ele chega.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum EpubNoticePlan {
    /// Um livro pedido para ler entrou (ou já lá estava): abre no leitor.
    OpenReader(String),
    /// Vários entraram fora da biblioteca: abre-a, com a linha dos que
    /// falharam (se algum falhou).
    OpenLibrary(Option<String>),
    /// A página de livros aberta já recebeu o aviso; nada mais a fazer.
    PageOnly,
    /// Erro para ler na Home.
    HomeStatus(String),
    /// Erro sobre outra superfície.
    Splash(String),
    /// Tudo entrou sem nada para abrir: sai o "Adicionando…" da Home.
    ClearHomeStatus,
    Nothing,
}

pub(in crate::windows_app) fn plan_epub_notice(
    notice: &EpubNotice,
    surface: Surface,
) -> EpubNoticePlan {
    let on_epub = surface == Surface::Epub;
    // Abrir o leitor ou a biblioteca destrói a superfície atual. Só por cima
    // da Home ou das páginas de livros: uma importação lenta (livro grande,
    // pen drive, rede) que acaba depois de a pessoa ter ido para uma página
    // web ou para o comparador não lhe tira o que está a fazer.
    let may_replace = matches!(surface, Surface::Home | Surface::Epub);
    if let EpubNotice::Added {
        books,
        failures,
        open: true,
    } = notice
    {
        if let ([book], true) = (books.as_slice(), failures.is_empty()) {
            return if may_replace {
                EpubNoticePlan::OpenReader(book.id.clone())
            } else {
                EpubNoticePlan::Splash(format!(
                    "“{}” entrou na biblioteca. Para ler, abra Livros (livros: na Home).",
                    book.title
                ))
            };
        }
        if !books.is_empty() && !on_epub {
            if may_replace {
                return EpubNoticePlan::OpenLibrary(notice.status_line());
            }
            let count = books.len();
            let added = if count == 1 {
                "1 livro entrou na biblioteca.".to_string()
            } else {
                format!("{count} livros entraram na biblioteca.")
            };
            return EpubNoticePlan::Splash(match notice.status_line() {
                Some(line) => format!("{added} {line}"),
                None => added,
            });
        }
    }
    if on_epub {
        return EpubNoticePlan::PageOnly;
    }
    match notice.status_line() {
        Some(line) if surface == Surface::Home => EpubNoticePlan::HomeStatus(line),
        Some(line) => EpubNoticePlan::Splash(line),
        None if matches!(notice, EpubNotice::Added { .. }) && surface == Surface::Home => {
            EpubNoticePlan::ClearHomeStatus
        }
        None => EpubNoticePlan::Nothing,
    }
}

/// O pedido do WebView para a origem `neuralia-epub` como o servidor o lê:
/// método e caminho COM a query (o `?as=html` dos capítulos).
pub(in crate::windows_app) fn epub_serve_job(
    request: &Request<Vec<u8>>,
    reply: Box<dyn FnOnce(EpubResponse) + Send>,
) -> ServeJob {
    ServeJob {
        method: request.method().as_str().to_string(),
        target: epub_request_target(request.uri().path_and_query().map(|target| target.as_str())),
        reply,
    }
}

/// A resposta da origem `neuralia-epub` no tipo HTTP do wry, com TODOS os
/// cabeçalhos do servidor (a CSP dos livros é o que impede um livro de
/// carregar imagens ou fontes da rede).
pub(in crate::windows_app) fn epub_http_response(
    response: EpubResponse,
) -> HttpResponse<Cow<'static, [u8]>> {
    let mut builder = HttpResponse::builder().status(response.status);
    for (name, value) in response.headers() {
        builder = builder.header(name, value);
    }
    builder
        .body(response.body)
        .unwrap_or_else(|_| HttpResponse::new(Cow::Borrowed(b"" as &[u8])))
}

/// Capacidade do buffer do dialogo (em UTF-16): muitos livros de uma vez,
/// cada um com um caminho longo.
pub(in crate::windows_app) const EPUB_DIALOG_BUFFER: usize = 64 * 1024;

/// O dialogo "Abrir" do Windows, so com `*.epub` e selecao multipla. Modal
/// sobre a janela principal; devolve os caminhos escolhidos (nenhum se a
/// pessoa cancelou).
pub(in crate::windows_app) fn pick_epub_files(owner: HWND) -> Vec<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
        OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let filter = epub_dialog_filter();
    let title = wide_null("Adicionar livros EPUB");
    let mut buffer = vec![0u16; EPUB_DIALOG_BUFFER];
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: title.as_ptr(),
        Flags: OFN_ALLOWMULTISELECT
            | OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_HIDEREADONLY,
        ..Default::default()
    };
    // SAFETY: `dialog` aponta para buffers vivos ate ao fim da chamada; o
    // Windows escreve no maximo `nMaxFile` unidades em `buffer`.
    let chosen = unsafe { GetOpenFileNameW(&mut dialog) };
    if chosen == 0 {
        return Vec::new();
    }
    parse_dialog_selection(&buffer)
        .into_iter()
        .filter(|path| is_epub_path(path))
        .collect()
}

/// Responde a origem do visualizador: os tres ficheiros do PDF.js e o
/// documento que esta aberto. Tudo em memoria; nada toca no disco.
///
/// O documento e servido por faixas (RFC 9110, `Range`): o visualizador pede
/// `getDocument({url, rangeChunkSize})` e, mal ve `Accept-Ranges: bytes` e o
/// `Content-Length`, aborta o pedido inicial e passa a pedir so os bytes de
/// que precisa. Cada pedido copia apenas a sua fatia -- antes, cada GET
/// clonava o documento inteiro (ate 32 MiB) por baixo do Mutex.
pub(in crate::windows_app) fn serve_pdf_asset(
    bytes: &Arc<Mutex<Vec<u8>>>,
    request: &Request<Vec<u8>>,
) -> HttpResponse<Cow<'static, [u8]>> {
    let path = request.uri().path();
    // HEAD e um GET sem corpo. O WebView2 entrega o metodo tal como a pagina
    // o pediu (wry 0.57, webview2/mod.rs, `prepare_request`), e os cabecalhos
    // -- em especial o Content-Length do documento -- tem de ser os do GET,
    // sem se copiar um unico byte.
    let head = request.method() == wry::http::Method::HEAD;
    // Cabecalhos que so o documento leva: o Content-Length do corpo que um GET
    // traria e, num 206/416, o Content-Range. Os ficheiros do visualizador
    // sao estaticos e nao precisam de nenhum dos dois.
    let mut document: Option<(usize, Option<String>)> = None;
    let (status, content_type, body): (u16, &str, Cow<'static, [u8]>) = match path {
        "/viewer.html" | "/" => (
            200,
            "text/html; charset=utf-8",
            Cow::Borrowed(PDF_VIEWER_HTML),
        ),
        "/viewer.mjs" => (200, "text/javascript", Cow::Borrowed(PDF_VIEWER_JS)),
        "/read-aloud.js" => (
            200,
            "text/javascript",
            Cow::Borrowed(READ_ALOUD_SCRIPT.as_bytes()),
        ),
        "/pdf.mjs" => (200, "text/javascript", Cow::Borrowed(PDFJS_CORE)),
        "/pdf.worker.mjs" => (200, "text/javascript", Cow::Borrowed(PDFJS_WORKER)),
        "/document.pdf" => {
            // O lock fica seguro ate a fatia estar copiada, para `open_pdf`
            // nao trocar o documento a meio; um Mutex envenenado vale como
            // documento vazio, exactamente como antes.
            let guard = bytes.lock().ok();
            let data: &[u8] = match guard.as_deref() {
                Some(slot) => slot,
                None => &[],
            };
            let total = data.len() as u64;
            let range = request
                .headers()
                .get("range")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            let (status, slice, content_range) = match parse_range(range, total) {
                // `end` e inclusivo e ja esta cortado a `total - 1`; os `as
                // usize` nao truncam porque `total` veio de um `len()`.
                RangeParse::Satisfiable { start, end } => (
                    206,
                    &data[start as usize..=end as usize],
                    Some(format!("bytes {start}-{end}/{total}")),
                ),
                RangeParse::Unsatisfiable => (416, &data[..0], Some(format!("bytes */{total}"))),
                RangeParse::None | RangeParse::Ignored => (200, data, None),
            };
            document = Some((slice.len(), content_range));
            // Copiar e inevitavel: a resposta do wry e `Cow<'static, [u8]>` e
            // os bytes vivem atras de um Mutex que nao se pode emprestar para
            // fora do handler. Mas copia-se SO a fatia pedida: com o
            // visualizador a pedir por Range, o documento inteiro passa uma
            // unica vez (o pedido inicial, que o PDF.js aborta assim que le o
            // Accept-Ranges) em vez de uma vez por cada pedido.
            let body = if head {
                Cow::Borrowed(b"" as &[u8])
            } else {
                Cow::Owned(slice.to_vec())
            };
            (status, "application/pdf", body)
        }
        _ => match crate::pdf_assets::lookup(path) {
            Some((content_type, asset)) => (200, content_type, Cow::Borrowed(asset)),
            None => (404, "text/plain", Cow::Borrowed(b"not found" as &[u8])),
        },
    };

    // nosniff em tudo: o tipo declarado e o tipo, nao se adivinha pelo corpo.
    // A politica do HTML vai tambem em cabecalho, que vale antes do <meta>.
    let mut response = HttpResponse::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff");
    if content_type.starts_with("text/html") {
        response = response.header("Content-Security-Policy", PDF_VIEWER_CSP);
    }
    if let Some((length, content_range)) = document {
        // Accept-Ranges vai em todas as respostas do documento, tambem no 416:
        // e ele que diz ao PDF.js que pode pedir por faixas.
        response = response
            .header("Accept-Ranges", "bytes")
            .header("Content-Length", length);
        if let Some(content_range) = content_range {
            response = response.header("Content-Range", content_range);
        }
    }
    // Os ficheiros estaticos num HEAD: sao `Borrowed`, por isso descartar o
    // corpo aqui nao custa nada (o documento ja chegou vazio de cima, antes
    // de qualquer copia).
    let body = if head {
        Cow::Borrowed(b"" as &[u8])
    } else {
        body
    };
    response
        .body(body)
        .unwrap_or_else(|_| HttpResponse::new(Cow::Borrowed(b"" as &[u8])))
}

/// O que o cabecalho `Range` de um pedido pede a um documento de `total`
/// bytes. Puro, para os casos limite se testarem sem WebView.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::windows_app) enum RangeParse {
    /// Nao veio cabecalho: resposta completa, 200.
    None,
    /// Uma unica faixa que cabe no documento. `end` e inclusivo e ja esta
    /// cortado ao ultimo byte, por isso `start..=end` indexa sem verificar.
    Satisfiable { start: u64, end: u64 },
    /// Faixa bem formada mas sem um unico byte a devolver (inicio para la do
    /// fim, sufixo de zero bytes, documento vazio): 416 com `bytes */total`.
    Unsatisfiable,
    /// Outra unidade, sintaxe estranha, inicio maior que o fim ou varias
    /// faixas: a RFC 9110 permite ignorar o cabecalho e responder 200 com o
    /// documento inteiro, e e o que o PDF.js tambem aceita. Servir
    /// `multipart/byteranges` nao vale o codigo que custaria.
    Ignored,
}

/// Le o cabecalho `Range` (RFC 9110, 14.2) para um documento com `total`
/// bytes. `header` vazio significa que o pedido nao trouxe cabecalho.
///
/// Aceita `bytes=A-B`, `bytes=A-` e `bytes=-N`; a unidade nao distingue
/// maiusculas e ha tolerancia a espacos, porque nada se ganha em recusar
/// `bytes= 0-9`. Um fim para la do documento corta-se ao ultimo byte; um
/// sufixo maior do que o documento e o documento inteiro. O que decide entre
/// `Unsatisfiable` e `Ignored` e a RFC: uma faixa valida que nao apanha
/// nenhum byte merece 416, porque um 200 mascararia o erro do cliente; uma
/// faixa invalida nao e um pedido de faixa e serve-se tudo.
pub(in crate::windows_app) fn parse_range(header: &str, total: u64) -> RangeParse {
    let header = header.trim();
    if header.is_empty() {
        return RangeParse::None;
    }
    let Some((unit, set)) = header.split_once('=') else {
        return RangeParse::Ignored;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return RangeParse::Ignored;
    }
    // A lista pode trazer elementos vazios (`bytes=0-9,`), que a gramatica de
    // listas do HTTP manda ignorar; contam-se so os que existem, e mais do
    // que um seria multipart.
    let mut specs = set
        .split(',')
        .map(str::trim)
        .filter(|spec| !spec.is_empty());
    let (Some(spec), None) = (specs.next(), specs.next()) else {
        return RangeParse::Ignored;
    };
    let Some((first, last)) = spec.split_once('-') else {
        return RangeParse::Ignored;
    };
    let (first, last) = (first.trim(), last.trim());

    if first.is_empty() {
        // `bytes=-N`: os ultimos N bytes. N = 0 e insatisfazivel por definicao
        // (nao apanha byte nenhum), e um documento vazio nao tem ultimos bytes.
        let Some(suffix) = parse_range_pos(last) else {
            return RangeParse::Ignored;
        };
        if suffix == 0 || total == 0 {
            return RangeParse::Unsatisfiable;
        }
        return RangeParse::Satisfiable {
            start: total.saturating_sub(suffix),
            end: total - 1,
        };
    }

    let Some(start) = parse_range_pos(first) else {
        return RangeParse::Ignored;
    };
    let end = if last.is_empty() {
        u64::MAX
    } else {
        match parse_range_pos(last) {
            Some(end) => end,
            None => return RangeParse::Ignored,
        }
    };
    // Fim antes do inicio e sintaxe invalida pela RFC, nao uma faixa vazia.
    if end < start {
        return RangeParse::Ignored;
    }
    if start >= total {
        return RangeParse::Unsatisfiable;
    }
    RangeParse::Satisfiable {
        start,
        end: end.min(total - 1),
    }
}

/// Um inteiro decimal do cabecalho `Range`: so digitos ASCII, pelo menos um.
/// Um valor que nao cabe em u64 satura em vez de falhar, porque para este fim
/// "enorme" e "infinito" sao o mesmo: um inicio assim e insatisfazivel e um
/// fim assim corta-se ao documento -- e um cliente que escreve 30 digitos nao
/// merece um 200 com o ficheiro inteiro por causa disso.
pub(in crate::windows_app) fn parse_range_pos(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(text.bytes().fold(0u64, |value, byte| {
        value
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'))
    }))
}

/// O que um WebView de Web externa pode pedir. Fora do closure do builder
/// para se poder exercitar sem janela.
pub(in crate::windows_app) fn external_ipc_event(
    action: IpcAction,
    agent_enabled: bool,
) -> Option<UserEvent> {
    match action {
        IpcAction::AgentObservation { data } if agent_enabled => {
            parse_agent_observation(&data).map(UserEvent::AgentObservation)
        }
        other => common_ipc_event(other),
    }
}

/// O script da Web externa, tal como `external_webview_builder` o injeta.
pub(in crate::windows_app) fn external_init_script(
    capability: &str,
    agent_enabled: bool,
) -> String {
    let agent_script = if agent_enabled {
        AGENT_OBSERVER_SCRIPT
    } else {
        ""
    };
    bind_page_script(
        &format!("{NEURALIA_KEYMAP_SCRIPT}\n{EXTERNAL_RETURN_BUTTON}\n{agent_script}"),
        capability,
        false,
    )
}

/// Pedido de janela nova vindo da pagina em Web completa. `window.open('',
/// '_blank')` chega como `about:blank`: aceite pelo `remote_web_target` (para
/// a navegacao), mas como destino de OpenExternal falha no validate_web_url e
/// o erro destruia a pagina do utilizador e voltava ao Home.
pub(in crate::windows_app) fn external_new_window_event(
    target: String,
    local_origin: Option<&str>,
) -> Option<UserEvent> {
    (!target.eq_ignore_ascii_case("about:blank") && remote_web_target(&target, local_origin))
        .then_some(UserEvent::OpenExternal(target))
}

pub(in crate::windows_app) fn is_pdf_internal_target(target: &str) -> bool {
    if target.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    Url::parse(target).is_ok_and(|url| {
        url.scheme() == "http"
            && url.host_str() == Some("neuralia-pdf.localhost")
            && url.port().is_none()
    })
}
