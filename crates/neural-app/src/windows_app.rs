#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{SyncSender, sync_channel},
    },
    thread,
};

use neural_core::{
    CoreConfig, HistoryEntry, HistoryKind, HistoryStore, Intent, ReaderArticle, ReaderClient,
    google_ai_url, parse_intent, reader_html,
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        CLEARTYPE_QUALITY, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET,
        DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
        DeleteObject, DrawTextW, Ellipse, FW_BOLD, FW_NORMAL, FillRect, GetDC, GetStockObject,
        NULL_PEN, OUT_DEFAULT_PRECIS, PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkMode,
        SetTextColor, TRANSPARENT,
    },
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, SetFocus, VK_CONTROL, VK_SHIFT},
        WindowsAndMessaging::{
            CreateWindowExW, ES_AUTOHSCROLL, GetClientRect, GetWindowTextLengthW, GetWindowTextW,
            SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos,
            SetWindowTextW, ShowWindow, WM_KEYDOWN, WS_CHILD, WS_EX_CLIENTEDGE, WS_TABSTOP,
            WS_VISIBLE,
        },
    },
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};
use wry::{NewWindowResponse, PermissionResponse, WebView, WebViewBuilder};

enum UserEvent {
    HomeRequested,
    ClearHistory,
    SubmitText(String),
    OpenExternal(String),
    ReaderReady {
        generation: u64,
        input: String,
        result: Result<ReaderArticle, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    Home,
    Reader,
    External,
}

const EM_SETSEL: u32 = 0x00B1;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_SETCUEBANNER: u32 = 0x1501;
const OMNIBOX_SUBCLASS_ID: usize = 0x4E49;

#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(
        hwnd: HWND,
        callback: Option<
            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT,
        >,
        subclass_id: usize,
        reference_data: usize,
    ) -> i32;
    fn DefSubclassProc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
}

unsafe extern "system" fn omnibox_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    if message == WM_KEYDOWN {
        let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
        let ctrl = (GetAsyncKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0;
        let shift = (GetAsyncKeyState(VK_SHIFT as i32) as u16 & 0x8000) != 0;

        match wparam as u32 {
            13 => {
                let text = window_text(hwnd);
                let _ = proxy.send_event(UserEvent::SubmitText(text));
                return 0;
            }
            27 => {
                SetWindowTextW(hwnd, windows_sys::w!(""));
                let _ = proxy.send_event(UserEvent::HomeRequested);
                return 0;
            }
            0x4C if ctrl => {
                SendMessageW(hwnd, EM_SETSEL, 0, -1);
                return 0;
            }
            0x2E if ctrl && shift => {
                let _ = proxy.send_event(UserEvent::ClearHistory);
                return 0;
            }
            _ => {}
        }
    }

    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe fn window_text(hwnd: HWND) -> String {
    let length = GetWindowTextLengthW(hwnd);
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    if copied <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..copied as usize])
    }
}

struct ReaderJob {
    generation: u64,
    input: String,
    url: String,
}

#[derive(Clone)]
struct ReaderWorker {
    pending: Arc<(Mutex<Option<ReaderJob>>, Condvar)>,
}

impl ReaderWorker {
    fn new(client: ReaderClient, proxy: EventLoopProxy<UserEvent>) -> Self {
        let pending = Arc::new((Mutex::new(None::<ReaderJob>), Condvar::new()));
        let worker_pending = Arc::clone(&pending);

        let _ = thread::Builder::new()
            .name("neural-reader".into())
            .spawn(move || {
                loop {
                    let job = {
                        let (lock, wake) = &*worker_pending;
                        let mut slot = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        while slot.is_none() {
                            slot = wake
                                .wait(slot)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                        slot.take().expect("reader job present")
                    };

                    let result = client.fetch(&job.url).map_err(|error| error.to_string());
                    let _ = proxy.send_event(UserEvent::ReaderReady {
                        generation: job.generation,
                        input: job.input,
                        result,
                    });
                }
            });

        Self { pending }
    }

    fn submit(&self, job: ReaderJob) {
        let (lock, wake) = &*self.pending;
        let mut slot = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(job);
        wake.notify_one();
    }
}

enum HistoryCommand {
    Append(HistoryEntry),
    Clear,
}

#[derive(Clone)]
struct HistoryWriter {
    tx: SyncSender<HistoryCommand>,
}

impl HistoryWriter {
    fn new(store: HistoryStore) -> Self {
        let (tx, rx) = sync_channel::<HistoryCommand>(64);
        let _ = thread::Builder::new()
            .name("neural-history".into())
            .spawn(move || {
                while let Ok(command) = rx.recv() {
                    match command {
                        HistoryCommand::Append(entry) => {
                            let _ = store.append(&entry);
                        }
                        HistoryCommand::Clear => {
                            let _ = store.clear();
                        }
                    }
                }
            });
        Self { tx }
    }

    fn append(&self, entry: HistoryEntry) {
        let _ = self.tx.try_send(HistoryCommand::Append(entry));
    }

    fn clear(&self) {
        let _ = self.tx.try_send(HistoryCommand::Clear);
    }
}

#[derive(Debug, Clone, Copy)]
struct UiRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl UiRect {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

#[derive(Debug, Clone, Copy)]
struct HomeLayout {
    input: UiRect,
    go: UiRect,
    ask: UiRect,
    reader: UiRect,
    web: UiRect,
}

impl HomeLayout {
    fn new(width: f64, height: f64, scale: f64) -> Self {
        let scale = scale.max(1.0);
        let row_width = (720.0 * scale).min((width - 48.0 * scale).max(320.0 * scale));
        let row_height = 54.0 * scale;
        let go_width = 84.0 * scale;
        let gap = 10.0 * scale;
        let row_x = (width - row_width) / 2.0;
        let row_y = (height * 0.54).clamp(310.0 * scale, height - 180.0 * scale);

        let input = UiRect {
            x: row_x,
            y: row_y,
            width: row_width - go_width - gap,
            height: row_height,
        };
        let go = UiRect {
            x: input.x + input.width + gap,
            y: row_y,
            width: go_width,
            height: row_height,
        };

        let mode_width = 104.0 * scale;
        let mode_height = 38.0 * scale;
        let modes_y = row_y + row_height + 16.0 * scale;
        let modes_total = mode_width * 3.0 + gap * 2.0;
        let modes_x = (width - modes_total) / 2.0;

        Self {
            input,
            go,
            ask: UiRect {
                x: modes_x,
                y: modes_y,
                width: mode_width,
                height: mode_height,
            },
            reader: UiRect {
                x: modes_x + mode_width + gap,
                y: modes_y,
                width: mode_width,
                height: mode_height,
            },
            web: UiRect {
                x: modes_x + (mode_width + gap) * 2.0,
                y: modes_y,
                width: mode_width,
                height: mode_height,
            },
        }
    }
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Window>,
    webview: Option<WebView>,
    omnibox: Option<HWND>,
    omnibox_proxy: Box<EventLoopProxy<UserEvent>>,
    config: CoreConfig,
    history: HistoryWriter,
    reader: ReaderWorker,
    surface: Surface,
    navigation_generation: u64,
    status: String,
    cursor: (f64, f64),
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let config = CoreConfig::default();
        let history_store =
            HistoryStore::with_limit(config.data_dir.join("history.jsonl"), config.history_limit);
        let history = HistoryWriter::new(history_store);
        let reader_client = ReaderClient::new(config.reader_timeout_secs, config.reader_max_bytes);
        let reader = ReaderWorker::new(reader_client, proxy.clone());
        let omnibox_proxy = Box::new(proxy.clone());
        Self {
            proxy,
            window: None,
            webview: None,
            omnibox: None,
            omnibox_proxy,
            config,
            history,
            reader,
            surface: Surface::Home,
            navigation_generation: 0,
            status: "WebView2 desligado enquanto você está aqui.".to_string(),
            cursor: (-1.0, -1.0),
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.navigation_generation = self.navigation_generation.wrapping_add(1);
        self.navigation_generation
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn create_omnibox(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(parent) = window_hwnd(window) else {
            return;
        };

        unsafe {
            let edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                windows_sys::w!("EDIT"),
                windows_sys::w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                0,
                0,
                100,
                32,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if edit.is_null() {
                return;
            }

            let cue: Vec<u16> = "Pergunte algo ou cole uma URL"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            SendMessageW(edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
            SendMessageW(edit, EM_SETLIMITTEXT, 2048, 0);

            let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
            if SetWindowSubclass(edit, Some(omnibox_subclass), OMNIBOX_SUBCLASS_ID, proxy_ptr) == 0
            {
                return;
            }

            self.omnibox = Some(edit);
            self.position_omnibox();
            SetFocus(edit);
        }
    }

    fn position_omnibox(&self) {
        let (Some(window), Some(edit)) = (&self.window, self.omnibox) else {
            return;
        };
        let size = window.inner_size();
        let layout = HomeLayout::new(size.width as f64, size.height as f64, window.scale_factor());
        unsafe {
            SetWindowPos(
                edit,
                std::ptr::null_mut(),
                layout.input.x as i32,
                layout.input.y as i32,
                layout.input.width as i32,
                layout.input.height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn show_omnibox(&self, visible: bool) {
        let Some(edit) = self.omnibox else {
            return;
        };
        unsafe {
            ShowWindow(edit, if visible { SW_SHOW } else { SW_HIDE });
            if visible {
                SetFocus(edit);
            }
        }
    }

    fn omnibox_text(&self) -> String {
        self.omnibox
            .map(|edit| unsafe { window_text(edit) })
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn destroy_webview(&mut self) {
        if let Some(webview) = self.webview.take() {
            let _ = webview.focus_parent();
            drop(webview);
        }
    }

    fn show_home(&mut self) {
        self.next_generation();
        self.destroy_webview();
        self.surface = Surface::Home;
        self.status = "WebView2 desligado enquanto você está aqui.".to_string();
        self.show_omnibox(true);
        self.position_omnibox();
        self.request_redraw();
    }

    fn show_native_error(&mut self, message: impl Into<String>) {
        self.next_generation();
        self.destroy_webview();
        self.surface = Surface::Home;
        self.status = message.into();
        self.show_omnibox(true);
        self.position_omnibox();
        self.request_redraw();
    }

    fn handle_input(&mut self, input: String) {
        match parse_intent(&input) {
            Ok(Intent::Home) => self.show_home(),
            Ok(Intent::Ask(query)) => self.ask(query),
            Ok(Intent::Read(url)) => self.read(url.to_string()),
            Ok(Intent::Web(url)) => self.web(url.to_string()),
            Err(error) => self.show_native_error(error.to_string()),
        }
    }

    fn submit_current(&mut self) {
        let input = self.omnibox_text();
        if !input.is_empty() {
            self.handle_input(input);
        }
    }

    fn ask_current(&mut self) {
        let query = self.omnibox_text();
        if !query.is_empty() {
            self.ask(query);
        }
    }

    fn reader_current(&mut self) {
        let input = self.omnibox_text();
        if !input.is_empty() {
            self.handle_input(format!("reader:{input}"));
        }
    }

    fn web_current(&mut self) {
        let input = self.omnibox_text();
        if !input.is_empty() {
            self.handle_input(format!("web:{input}"));
        }
    }

    fn ask(&mut self, query: String) {
        match google_ai_url(&query, &self.config.language) {
            Ok(url) => {
                self.next_generation();
                self.record(HistoryKind::Ask, query, "google-ai".to_string());
                self.open_external(url.as_str());
            }
            Err(error) => self.show_native_error(error.to_string()),
        }
    }

    fn read(&mut self, url: String) {
        let generation = self.next_generation();
        self.destroy_webview();
        self.surface = Surface::Home;
        self.status = format!("Lendo {url} …");
        self.request_redraw();

        self.reader.submit(ReaderJob {
            generation,
            input: url.clone(),
            url,
        });
    }

    fn web(&mut self, url: String) {
        match neural_core::validate_web_url(&url) {
            Ok(valid) => {
                self.next_generation();
                self.record(HistoryKind::Web, valid.to_string(), valid.to_string());
                self.open_external(valid.as_str());
            }
            Err(error) => self.show_native_error(error.to_string()),
        }
    }

    fn record(&self, kind: HistoryKind, input: String, target: String) {
        self.history.append(HistoryEntry::now(kind, input, target));
    }

    fn reader_webview_builder(&self) -> WebViewBuilder<'static> {
        let proxy = self.proxy.clone();
        WebViewBuilder::new()
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

                match action_url.path().trim_matches('/') {
                    "home" => {
                        let _ = proxy.send_event(UserEvent::HomeRequested);
                    }
                    "web" => {
                        if let Some((_, value)) =
                            action_url.query_pairs().find(|(key, _)| key == "url")
                            && neural_core::validate_web_url(value.as_ref()).is_ok()
                        {
                            let _ = proxy.send_event(UserEvent::OpenExternal(value.into_owned()));
                        }
                    }
                    _ => {}
                }

                false
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    fn external_webview_builder(&self) -> WebViewBuilder<'static> {
        let navigation_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();

        WebViewBuilder::new()
            .with_initialization_script(EXTERNAL_RETURN_BUTTON)
            .with_navigation_handler(move |target| {
                if target.eq_ignore_ascii_case("neuralia:home") {
                    let _ = navigation_proxy.send_event(UserEvent::HomeRequested);
                    return false;
                }

                target.starts_with("about:blank") || neural_core::validate_web_url(&target).is_ok()
            })
            .with_new_window_req_handler(move |target, _features| {
                if neural_core::validate_web_url(&target).is_ok() {
                    let _ = new_window_proxy.send_event(UserEvent::OpenExternal(target));
                }
                NewWindowResponse::Deny
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    fn open_external(&mut self, url: &str) {
        self.destroy_webview();
        self.show_omnibox(false);

        let result = if let Some(window) = &self.window {
            self.external_webview_builder().with_url(url).build(window)
        } else {
            return;
        };

        match result {
            Ok(webview) => {
                self.webview = Some(webview);
                self.surface = Surface::External;
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde abrir a página: {error}"));
            }
        }
    }

    fn open_reader(&mut self, article: &ReaderArticle) {
        self.destroy_webview();
        self.show_omnibox(false);
        let html = reader_html(article);

        let result = if let Some(window) = &self.window {
            self.reader_webview_builder().with_html(html).build(window)
        } else {
            return;
        };

        match result {
            Ok(webview) => {
                self.webview = Some(webview);
                self.surface = Surface::Reader;
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde exibir o Reader: {error}"));
            }
        }
    }

    fn click_home(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let layout = HomeLayout::new(size.width as f64, size.height as f64, window.scale_factor());
        let (x, y) = self.cursor;

        if layout.go.contains(x, y) {
            self.submit_current();
        } else if layout.ask.contains(x, y) {
            self.ask_current();
        } else if layout.reader.contains(x, y) {
            self.reader_current();
        } else if layout.web.contains(x, y) {
            self.web_current();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("NeuralIA")
            .with_inner_size(LogicalSize::new(1120.0, 760.0))
            .with_min_inner_size(LogicalSize::new(700.0, 500.0));

        match event_loop.create_window(attributes) {
            Ok(window) => {
                window.set_ime_allowed(true);
                self.window = Some(window);
                self.create_omnibox();
                self.request_redraw();
            }
            Err(error) => {
                eprintln!("window creation failed: {error}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::HomeRequested => self.show_home(),
            UserEvent::ClearHistory => {
                self.history.clear();
                self.status = "Histórico local apagado.".to_string();
                self.show_home();
            }
            UserEvent::SubmitText(input) => {
                if self.surface == Surface::Home {
                    let input = input.trim().to_string();
                    if !input.is_empty() {
                        self.handle_input(input);
                    }
                }
            }
            UserEvent::OpenExternal(url) => self.web(url),
            UserEvent::ReaderReady {
                generation,
                input,
                result,
            } => {
                if generation != self.navigation_generation {
                    return;
                }
                match result {
                    Ok(article) => {
                        self.record(HistoryKind::Read, input, article.source_url.clone());
                        self.open_reader(&article);
                    }
                    Err(error) => self.show_native_error(format!("Reader: {error}")),
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested if self.surface == Surface::Home => {
                if let Some(window) = &self.window {
                    draw_home(window, &self.status);
                }
            }
            WindowEvent::Resized(_) if self.surface == Surface::Home => {
                self.position_omnibox();
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.surface == Surface::Home => self.click_home(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state.is_pressed()
                    && matches!(event.logical_key, Key::Named(NamedKey::Escape)) =>
            {
                self.show_home();
            }
            _ => {}
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn window_hwnd(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}

fn draw_home(window: &Window, status: &str) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };

    let hwnd = handle.hwnd.get() as HWND;
    let scale = window.scale_factor().max(1.0);

    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return;
        }

        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client) == 0 {
            let _ = ReleaseDC(hwnd, hdc);
            return;
        }

        let width = (client.right - client.left) as f64;
        let height = (client.bottom - client.top) as f64;
        let layout = HomeLayout::new(width, height, scale);

        let background = CreateSolidBrush(rgb(248, 249, 250));
        FillRect(hdc, &client, background);
        DeleteObject(background as _);

        SetBkMode(hdc, TRANSPARENT as i32);

        let logo_size = (104.0 * scale) as i32;
        let logo_x = ((width - logo_size as f64) / 2.0) as i32;
        let logo_y = (height * 0.20).clamp(70.0 * scale, 150.0 * scale) as i32;
        draw_logo(hdc, logo_x, logo_y, logo_size);

        let title_font = create_font((-38.0 * scale) as i32, FW_BOLD as i32);
        let body_font = create_font((-17.0 * scale) as i32, FW_NORMAL as i32);
        let small_font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);

        let old_font = SelectObject(hdc, title_font as _);
        SetTextColor(hdc, rgb(23, 25, 27));
        let mut title_rect = RECT {
            left: 0,
            top: logo_y + logo_size + (20.0 * scale) as i32,
            right: client.right,
            bottom: logo_y + logo_size + (72.0 * scale) as i32,
        };
        draw_text(
            hdc,
            "NeuralIA",
            &mut title_rect,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );

        SelectObject(hdc, body_font as _);
        SetTextColor(hdc, rgb(114, 118, 125));
        let mut tag_rect = RECT {
            left: 0,
            top: title_rect.bottom,
            right: client.right,
            bottom: title_rect.bottom + (44.0 * scale) as i32,
        };
        draw_text(
            hdc,
            "Pergunte. Leia. Continue.",
            &mut tag_rect,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );

        draw_button(hdc, layout.go, "Ir", true, scale, body_font);
        draw_button(hdc, layout.ask, "IA", false, scale, small_font);
        draw_button(hdc, layout.reader, "Reader", false, scale, small_font);
        draw_button(hdc, layout.web, "Web", false, scale, small_font);

        SelectObject(hdc, small_font as _);
        SetTextColor(hdc, rgb(126, 130, 137));
        let mut help_rect = RECT {
            left: (24.0 * scale) as i32,
            top: (layout.web.y + layout.web.height + 20.0 * scale) as i32,
            right: client.right - (24.0 * scale) as i32,
            bottom: (layout.web.y + layout.web.height + 50.0 * scale) as i32,
        };
        draw_text(
            hdc,
            "Texto → Google AI · URL → Reader · Ctrl+V cola · Ctrl+Shift+Del limpa histórico",
            &mut help_rect,
            DT_CENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );

        SetTextColor(hdc, rgb(96, 101, 108));
        let mut status_rect = RECT {
            left: (32.0 * scale) as i32,
            top: client.bottom - (64.0 * scale) as i32,
            right: client.right - (32.0 * scale) as i32,
            bottom: client.bottom - (24.0 * scale) as i32,
        };
        draw_text(
            hdc,
            status,
            &mut status_rect,
            DT_CENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );

        SelectObject(hdc, old_font);
        DeleteObject(title_font as _);
        DeleteObject(body_font as _);
        DeleteObject(small_font as _);
        let _ = ReleaseDC(hwnd, hdc);
    }
}

unsafe fn create_font(height: i32, weight: i32) -> *mut core::ffi::c_void {
    CreateFontW(
        height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        0,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        windows_sys::w!("Segoe UI"),
    )
}

unsafe fn draw_logo(hdc: *mut core::ffi::c_void, x: i32, y: i32, size: i32) {
    let black = CreateSolidBrush(rgb(17, 19, 20));
    let white = CreateSolidBrush(rgb(255, 255, 255));
    let null_pen = GetStockObject(NULL_PEN);

    let old_pen = SelectObject(hdc, null_pen);
    let old_brush = SelectObject(hdc, black as _);
    let radius = (size as f64 * 0.18) as i32;
    RoundRect(hdc, x, y, x + size, y + size, radius, radius);

    SelectObject(hdc, white as _);
    let c = size / 2;
    let arm = size / 5;
    let thick = size / 7;
    Ellipse(hdc, x + c - thick, y + c - arm * 2, x + c + thick, y + c);
    Ellipse(hdc, x + c, y + c - thick, x + c + arm * 2, y + c + thick);
    Ellipse(hdc, x + c - thick, y + c, x + c + thick, y + c + arm * 2);
    Ellipse(hdc, x + c - arm * 2, y + c - thick, x + c, y + c + thick);
    let dot = size / 9;
    Ellipse(hdc, x + c - dot, y + c - dot, x + c + dot, y + c + dot);

    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(black as _);
    DeleteObject(white as _);
}

unsafe fn draw_button(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    label: &str,
    primary: bool,
    scale: f64,
    font: *mut core::ffi::c_void,
) {
    let fill = CreateSolidBrush(if primary {
        rgb(17, 19, 20)
    } else {
        rgb(234, 236, 239)
    });
    let pen = CreatePen(
        PS_SOLID,
        1,
        if primary {
            rgb(17, 19, 20)
        } else {
            rgb(225, 227, 231)
        },
    );
    let old_brush = SelectObject(hdc, fill as _);
    let old_pen = SelectObject(hdc, pen as _);

    RoundRect(
        hdc,
        rect.x as i32,
        rect.y as i32,
        (rect.x + rect.width) as i32,
        (rect.y + rect.height) as i32,
        (14.0 * scale) as i32,
        (14.0 * scale) as i32,
    );

    SelectObject(hdc, font as _);
    SetTextColor(
        hdc,
        if primary {
            rgb(255, 255, 255)
        } else {
            rgb(31, 34, 37)
        },
    );
    let mut text_rect = RECT {
        left: rect.x as i32,
        top: rect.y as i32,
        right: (rect.x + rect.width) as i32,
        bottom: (rect.y + rect.height) as i32,
    };
    draw_text(
        hdc,
        label,
        &mut text_rect,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
    );

    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(fill as _);
    DeleteObject(pen as _);
}

unsafe fn draw_text(hdc: *mut core::ffi::c_void, text: &str, rect: &mut RECT, format: u32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if !wide.is_empty() {
        DrawTextW(hdc, wide.as_ptr(), wide.len() as i32, rect, format);
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

const EXTERNAL_RETURN_BUTTON: &str = r#"
document.addEventListener('DOMContentLoaded', () => {
  if (document.getElementById('neural-shell') || document.getElementById('neuralia-return')) return;
  const b = document.createElement('button');
  b.id = 'neuralia-return';
  b.textContent = '◀ NeuralIA';
  Object.assign(b.style, {
    position:'fixed', left:'16px', bottom:'16px', zIndex:'2147483647',
    border:'0', borderRadius:'999px', padding:'11px 16px',
    background:'#111314', color:'#fff', font:'600 13px Segoe UI, sans-serif',
    boxShadow:'0 6px 24px rgba(0,0,0,.25)', cursor:'pointer'
  });
  b.addEventListener('click', () => { window.location.href = 'neuralia:home'; });
  document.documentElement.appendChild(b);
});
"#;
