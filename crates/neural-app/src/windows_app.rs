#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread,
};

use image::RgbaImage;

use neural_core::{
    CoreConfig, HistoryEntry, HistoryKind, HistoryStore, Intent, ReaderArticle, ReaderClient,
    chatgpt_search_url, claude_search_url, google_ai_url, parse_intent, reader_html,
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CLEARTYPE_QUALITY, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
        DIB_RGB_COLORS, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
        DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FW_BOLD, FW_NORMAL, FillRect, GetDC,
        OUT_DEFAULT_PRECIS, ReleaseDC, SRCCOPY, SelectObject, SetBkColor, SetBkMode, SetTextColor,
        StretchDIBits, TRANSPARENT,
    },
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, SetFocus, VK_CONTROL, VK_SHIFT},
        WindowsAndMessaging::{
            CreateWindowExW, ES_AUTOHSCROLL, GetClientRect, GetWindowTextLengthW, GetWindowTextW,
            MB_ICONINFORMATION, MB_OK, MessageBoxW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
            SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow, WM_KEYDOWN, WS_CHILD,
            WS_TABSTOP, WS_VISIBLE,
        },
    },
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Icon, Window, WindowId},
};
use wry::{NewWindowResponse, PermissionResponse, WebView, WebViewBuilder};

enum UserEvent {
    HomeRequested,
    ShowHistory,
    ClearHistory,
    HistoryCleared(Result<(), String>),
    SubmitText(String),
    OpenExternal(String),
    ExpandComparator(usize),
    RestoreComparator,
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
    Comparator,
}

const TOP_BAR_HEIGHT: f64 = 42.0;
const COMPARATOR_COLUMNS: usize = 3;

/// O que esta debaixo do rato na barra de topo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarHit {
    Home,
    Column(usize),
}

/// Geometria da barra de topo — fonte unica para desenho E para o clique.
#[derive(Debug, Clone, Copy)]
struct BarLayout {
    /// Falso em tela cheia: nao se desenha nada e nada responde ao rato.
    visible: bool,
    height: f64,
    home: UiRect,
    columns: [UiRect; COMPARATOR_COLUMNS],
    columns_len: usize,
    hint: UiRect,
}

impl BarLayout {
    /// Em tela cheia devolve uma barra escondida: a coluna expandida fica com a
    /// janela inteira, sem faixa nativa por cima do site.
    fn new(client_width: f64, scale: f64, expanded: bool, columns: usize) -> Self {
        let scale = scale.max(1.0);
        if expanded {
            let empty = UiRect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            };
            return Self {
                visible: false,
                height: 0.0,
                home: empty,
                columns: [empty; COMPARATOR_COLUMNS],
                columns_len: 0,
                hint: empty,
            };
        }

        let height = TOP_BAR_HEIGHT * scale;
        let pad = 10.0 * scale;
        let gap = 8.0 * scale;
        let pill_h = 30.0 * scale;
        let pill_y = ((height - pill_h) / 2.0).round();

        let home = UiRect {
            x: pad,
            y: pill_y,
            width: 104.0 * scale,
            height: pill_h,
        };
        let hint_w = 104.0 * scale;
        let hint = UiRect {
            x: (client_width - hint_w - pad).max(home.x + home.width),
            y: 0.0,
            width: hint_w,
            height,
        };

        let content_x = home.x + home.width + gap * 1.5;
        let content_right = hint.x - gap;

        let empty = UiRect {
            x: 0.0,
            y: pill_y,
            width: 0.0,
            height: pill_h,
        };
        let mut rects = [empty; COMPARATOR_COLUMNS];
        let columns_len = columns.min(COMPARATOR_COLUMNS);

        if columns_len > 0 {
            // Cada pilula fica centrada sobre a coluna que representa: e o que
            // liga o botao ao painel por baixo dele. So encolhe/desliza quando
            // a janela e estreita de mais e ela bateria no Home ou no "Esc".
            let column_width = client_width / columns_len as f64;
            let width = (column_width - 16.0 * scale).clamp(44.0 * scale, 200.0 * scale);
            let mut left_bound = content_x;
            for (i, rect) in rects.iter_mut().enumerate().take(columns_len) {
                let center = column_width * (i as f64 + 0.5);
                let highest = (content_right - width).max(left_bound);
                let x = (center - width / 2.0).clamp(left_bound, highest);
                *rect = UiRect {
                    x,
                    y: pill_y,
                    width,
                    height: pill_h,
                };
                left_bound = x + width + gap;
            }
        }

        Self {
            visible: true,
            height,
            home,
            columns: rects,
            columns_len,
            hint,
        }
    }

    fn hit(&self, x: f64, y: f64) -> Option<BarHit> {
        if !self.visible || y > self.height {
            return None;
        }
        if self.home.contains(x, y) {
            return Some(BarHit::Home);
        }
        for (i, rect) in self.columns.iter().enumerate().take(self.columns_len) {
            if rect.contains(x, y) {
                return Some(BarHit::Column(i));
            }
        }
        None
    }
}

struct ComparatorView {
    webview: WebView,
    name: &'static str,
}

struct ComparatorState {
    views: Vec<ComparatorView>,
    expanded: Option<usize>,
}

const EM_SETSEL: u32 = 0x00B1;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_SETCUEBANNER: u32 = 0x1501;
const EM_SETMARGINS: u32 = 0x00D3;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const WINDOW_SUBCLASS_ID: usize = 0x4E4A;
const EC_LEFTMARGIN: usize = 0x0001;
const EC_RIGHTMARGIN: usize = 0x0002;
const WM_SETFONT: u32 = 0x0030;
const OMNIBOX_SUBCLASS_ID: usize = 0x4E49;

/// Consulta disparada automaticamente quando o app abre. `NEURALIA_STARTUP_INPUT`
/// substitui-a e `NEURALIA_NO_STARTUP` desliga-a, que e como o teste de
/// desempenho consegue medir a Home mesmo em repouso.
const DEFAULT_STARTUP_INPUT: &str = "jose r f junior";

fn startup_input() -> String {
    // Variavel propria em vez de string vazia: no Windows pôr uma variavel a ""
    // e o mesmo que apaga-la, e o teste de desempenho ficaria sem forma de a
    // desligar.
    if std::env::var_os("NEURALIA_NO_STARTUP").is_some() {
        return String::new();
    }
    std::env::var("NEURALIA_STARTUP_INPUT")
        .unwrap_or_else(|_| DEFAULT_STARTUP_INPUT.to_string())
        .trim()
        .to_string()
}

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

/// Pincel de fundo da omnibox, um por cor. Criar um a cada WM_CTLCOLOREDIT
/// vazaria objetos GDI a cada repintura.
static OMNIBOX_BRUSH: Mutex<Option<(Rgb, usize)>> = Mutex::new(None);

fn omnibox_brush(color: Rgb) -> *mut core::ffi::c_void {
    let mut slot = OMNIBOX_BRUSH.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((cached, handle)) = *slot
        && cached == color
    {
        return handle as *mut core::ffi::c_void;
    }
    unsafe {
        let brush = CreateSolidBrush(rgb3(color));
        if let Some((_, previous)) = slot.replace((color, brush as usize)) {
            DeleteObject(previous as _);
        }
        brush
    }
}

/// O EDIT nativo nao tem cantos redondos nem cor de fundo propria. Pintamos a
/// pilula suavizada por tras dele e respondemos aqui com a mesma cor, para o
/// retangulo do controlo desaparecer dentro dela.
unsafe extern "system" fn window_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if message == WM_CTLCOLOREDIT {
        let theme = Theme::system();
        let hdc = wparam as *mut core::ffi::c_void;
        SetTextColor(hdc, rgb3(theme.fg));
        SetBkColor(hdc, rgb3(theme.surface));
        return omnibox_brush(theme.surface) as LRESULT;
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
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
            0x48 if ctrl => {
                let _ = proxy.send_event(UserEvent::ShowHistory);
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
    alive: bool,
}

impl ReaderWorker {
    fn new(
        client: ReaderClient,
        proxy: EventLoopProxy<UserEvent>,
        generation: Arc<AtomicU64>,
    ) -> Self {
        let pending = Arc::new((Mutex::new(None::<ReaderJob>), Condvar::new()));
        let worker_pending = Arc::clone(&pending);

        let spawned = thread::Builder::new()
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

                    // Desistir assim que a navegacao mudar: sem isto a ligacao
                    // continua a receber dados depois de o utilizador voltar a
                    // Home, o que contraria o orcamento de rede da SPEC-0008.
                    let job_generation = job.generation;
                    let watch = Arc::clone(&generation);
                    let result = client
                        .fetch_cancellable(&job.url, &|| {
                            watch.load(Ordering::SeqCst) != job_generation
                        })
                        .map_err(|error| error.to_string());

                    let _ = proxy.send_event(UserEvent::ReaderReady {
                        generation: job_generation,
                        input: job.input,
                        result,
                    });
                }
            });

        Self {
            pending,
            alive: spawned.is_ok(),
        }
    }

    fn submit(&self, job: ReaderJob) -> Result<(), String> {
        if !self.alive {
            return Err("a thread do Reader não pôde ser criada".to_string());
        }
        let (lock, wake) = &*self.pending;
        let mut slot = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(job);
        wake.notify_one();
        Ok(())
    }
}

enum HistoryCommand {
    Append(HistoryEntry),
    Clear,
}

#[derive(Clone)]
struct HistoryWriter {
    tx: SyncSender<HistoryCommand>,
    store: HistoryStore,
}

impl HistoryWriter {
    fn new(store: HistoryStore, proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = sync_channel::<HistoryCommand>(64);
        let worker_store = store.clone();
        let _ = thread::Builder::new()
            .name("neural-history".into())
            .spawn(move || {
                while let Ok(command) = rx.recv() {
                    match command {
                        HistoryCommand::Append(entry) => {
                            let _ = worker_store.append(&entry);
                        }
                        HistoryCommand::Clear => {
                            // O utilizador so pode ver "apagado" depois de o
                            // disco confirmar; ate aqui isto era fire-and-forget.
                            let result = worker_store.clear().map_err(|error| error.to_string());
                            let _ = proxy.send_event(UserEvent::HistoryCleared(result));
                        }
                    }
                }
            });
        Self { tx, store }
    }

    /// Fila cheia ou worker em falta: escreve aqui mesmo, em vez de perder a
    /// entrada em silencio.
    fn append(&self, entry: HistoryEntry) {
        if self
            .tx
            .try_send(HistoryCommand::Append(entry.clone()))
            .is_err()
        {
            let _ = self.store.append(&entry);
        }
    }

    /// `None` = pedido entregue ao worker, a resposta chega em `HistoryCleared`.
    /// `Some(..)` = nao houve worker, foi apagado aqui e o resultado e este.
    fn clear(&self) -> Option<Result<(), String>> {
        if self.tx.try_send(HistoryCommand::Clear).is_ok() {
            return None;
        }
        Some(self.store.clear().map_err(|error| error.to_string()))
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

        Self { input, go }
    }
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Window>,
    webview: Option<WebView>,
    comparator: Option<ComparatorState>,
    omnibox: Option<HWND>,
    bar_hover: Option<BarHit>,
    omnibox_font: Option<*mut core::ffi::c_void>,
    omnibox_font_height: i32,
    omnibox_proxy: Box<EventLoopProxy<UserEvent>>,
    config: CoreConfig,
    history_store: HistoryStore,
    history: HistoryWriter,
    reader: ReaderWorker,
    surface: Surface,
    navigation_generation: Arc<AtomicU64>,
    status: Option<String>,
    cursor: (f64, f64),
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let config = CoreConfig::default();
        let history_store =
            HistoryStore::with_limit(config.data_dir.join("history.jsonl"), config.history_limit);
        let history = HistoryWriter::new(history_store.clone(), proxy.clone());
        let reader_client = ReaderClient::new(config.reader_timeout_secs, config.reader_max_bytes);
        let navigation_generation = Arc::new(AtomicU64::new(0));
        let reader = ReaderWorker::new(
            reader_client,
            proxy.clone(),
            Arc::clone(&navigation_generation),
        );
        let omnibox_proxy = Box::new(proxy.clone());
        Self {
            proxy,
            window: None,
            webview: None,
            comparator: None,
            omnibox: None,
            bar_hover: None,
            omnibox_font: None,
            omnibox_font_height: 0,
            omnibox_proxy,
            config,
            history_store,
            history,
            reader,
            surface: Surface::Home,
            navigation_generation,
            status: None,
            cursor: (-1.0, -1.0),
        }
    }

    /// Toda a navegacao passa por aqui: a thread do Reader observa este contador
    /// para saber que o resultado que esta a buscar ja nao interessa a ninguem.
    fn next_generation(&mut self) -> u64 {
        self.navigation_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn current_generation(&self) -> u64 {
        self.navigation_generation.load(Ordering::SeqCst)
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
            // Sem WS_EX_CLIENTEDGE: a moldura afundada e quadrada e nao ha
            // forma de a arredondar. A borda visivel passa a ser a pilula.
            let edit = CreateWindowExW(
                0,
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

            SetWindowSubclass(parent, Some(window_subclass), WINDOW_SUBCLASS_ID, 0);

            self.omnibox = Some(edit);
            self.position_omnibox();
            SetFocus(edit);
        }
    }

    fn position_omnibox(&mut self) {
        let (Some(window), Some(edit)) = (&self.window, self.omnibox) else {
            return;
        };
        let size = window.inner_size();
        let layout = HomeLayout::new(size.width as f64, size.height as f64, window.scale_factor());
        let scale = window.scale_factor().max(1.0);

        // O controlo vive encaixado dentro da pilula desenhada: os cantos retos
        // ficam por baixo da curva e nunca se veem.
        let pad_x = 22.0 * scale;
        let pad_y = 5.0 * scale;
        let inner = UiRect {
            x: layout.input.x + pad_x,
            y: layout.input.y + pad_y,
            width: (layout.input.width - pad_x * 2.0).max(1.0),
            height: (layout.input.height - pad_y * 2.0).max(1.0),
        };

        unsafe {
            SetWindowPos(
                edit,
                std::ptr::null_mut(),
                inner.x as i32,
                inner.y as i32,
                inner.width as i32,
                inner.height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.apply_omnibox_font(inner.height);
    }

    /// Fonte proporcional a altura da barra: acompanha o tamanho da caixa e o DPI.
    fn apply_omnibox_font(&mut self, box_height: f64) {
        let Some(edit) = self.omnibox else {
            return;
        };
        let height = -((box_height * 0.58).round() as i32).clamp(18, 80);
        if self.omnibox_font_height == height && self.omnibox_font.is_some() {
            return;
        }

        unsafe {
            let font = create_font(height, FW_NORMAL as i32);
            if font.is_null() {
                return;
            }
            SendMessageW(edit, WM_SETFONT, font as usize, 1);
            if let Some(previous) = self.omnibox_font.replace(font) {
                DeleteObject(previous as _);
            }
            self.omnibox_font_height = height;

            // O espacamento ja vem do encaixe dentro da pilula.
            let margin = 0usize;
            SendMessageW(
                edit,
                EM_SETMARGINS,
                EC_LEFTMARGIN | EC_RIGHTMARGIN,
                ((margin << 16) | margin) as isize,
            );
        }
    }

    fn set_omnibox_text(&self, text: &str) {
        let Some(edit) = self.omnibox else {
            return;
        };
        let value = wide_null(text);
        unsafe {
            SetWindowTextW(edit, value.as_ptr());
            SendMessageW(edit, EM_SETSEL, 0, -1);
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

    /// Unica saida de qualquer superficie web. Leva o comparador junto: era
    /// aqui que os tres WebViews sobreviviam ao regresso a Home e o contador
    /// podia chegar a quatro somando o Full Web.
    fn destroy_web_surfaces(&mut self) {
        if let Some(comparator) = self.comparator.take() {
            drop(comparator);
        }
        if let Some(webview) = self.webview.take() {
            let _ = webview.focus_parent();
            drop(webview);
        }
    }

    fn show_home(&mut self) {
        self.next_generation();
        self.destroy_web_surfaces();
        self.surface = Surface::Home;
        self.bar_hover = None;
        self.status = None;
        self.show_omnibox(true);
        self.position_omnibox();
        self.request_redraw();
    }

    fn show_native_error(&mut self, message: impl Into<String>) {
        self.next_generation();
        self.destroy_web_surfaces();
        self.surface = Surface::Home;
        self.status = Some(message.into());
        self.show_omnibox(true);
        self.position_omnibox();
        self.request_redraw();
    }

    /// `show_home` reescreve o estado, por isso a mensagem tem de vir depois
    /// dele — antes desta correcao o aviso de apagado nunca chegava a aparecer.
    fn report_history_cleared(&mut self, result: Result<(), String>) {
        self.show_home();
        self.status = Some(match result {
            Ok(()) => "Histórico local apagado.".to_string(),
            Err(error) => format!("Não foi possível apagar o histórico: {error}"),
        });
        self.request_redraw();
    }

    fn show_history(&self) {
        let text = match self.history_store.recent(20) {
            Ok(entries) if entries.is_empty() => "Histórico local vazio.".to_string(),
            Ok(entries) => entries
                .into_iter()
                .map(|entry| {
                    let kind = match entry.kind {
                        HistoryKind::Ask => "IA",
                        HistoryKind::Read => "Reader",
                        HistoryKind::Web => "Web",
                    };
                    format!("[{kind}] {}", entry.input)
                })
                .collect::<Vec<_>>()
                .join("\r\n"),
            Err(error) => format!("Não foi possível ler o histórico: {error}"),
        };

        let Some(window) = &self.window else {
            return;
        };
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };

        let body = wide_null(&text);
        let title = wide_null("NeuralIA — Histórico local");
        unsafe {
            MessageBoxW(
                hwnd,
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    fn handle_input(&mut self, input: String) {
        match parse_intent(&input) {
            Ok(Intent::Home) => self.show_home(),
            Ok(Intent::Ask(query)) => self.ask(query),
            Ok(Intent::Compare(query)) => self.compare(query),
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

    /// Um unico fornecedor: o Google AI Mode, num so WebView. E a saida para
    /// quem nao quer a pergunta em tres sitios ao mesmo tempo (`ask:` ou `?`).
    fn ask(&mut self, query: String) {
        let url = match google_ai_url(&query, &self.config.language) {
            Ok(url) => url,
            Err(error) => {
                self.show_native_error(error.to_string());
                return;
            }
        };
        self.next_generation();
        self.record(HistoryKind::Ask, query, url.to_string());
        self.open_external(url.as_str());
    }

    /// Destino normal de uma pergunta: a mesma consulta segue em simultaneo
    /// para o Google AI Mode, o ChatGPT e o Claude, lado a lado.
    fn compare(&mut self, query: String) {
        self.next_generation();
        self.record(
            HistoryKind::Ask,
            format!("compare:{query}"),
            "comparator-3col".to_string(),
        );
        self.open_comparator(&query);
    }

    fn read(&mut self, url: String) {
        let generation = self.next_generation();
        self.destroy_web_surfaces();
        self.surface = Surface::Home;
        self.status = Some(format!("Lendo {url} …"));
        self.request_redraw();

        if let Err(error) = self.reader.submit(ReaderJob {
            generation,
            input: url.clone(),
            url,
        }) {
            self.show_native_error(format!("Reader indisponível: {error}"));
        }
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
        self.destroy_web_surfaces();
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
                self.webview = Some(webview);
                self.surface = Surface::Reader;
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde exibir o Reader: {error}"));
            }
        }
    }

    fn open_comparator(&mut self, query: &str) {
        self.destroy_web_surfaces();
        self.show_omnibox(false);

        let google_url = match google_ai_url(query, &self.config.language) {
            Ok(u) => u,
            Err(e) => {
                self.show_native_error(e.to_string());
                return;
            }
        };
        let chatgpt_url = match chatgpt_search_url(query) {
            Ok(u) => u,
            Err(e) => {
                self.show_native_error(e.to_string());
                return;
            }
        };
        let claude_url = match claude_search_url(query) {
            Ok(u) => u,
            Err(e) => {
                self.show_native_error(e.to_string());
                return;
            }
        };

        let Some(window) = &self.window else {
            return;
        };

        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;

        let content_h = (logical_h - TOP_BAR_HEIGHT).max(100.0);
        let content_y = TOP_BAR_HEIGHT;
        let n = 3.0;
        let col_w = logical_w / n;

        let targets = [
            ("Google Gemini", google_url),
            ("ChatGPT", chatgpt_url),
            ("Claude", claude_url),
        ];

        let mut views = Vec::new();
        for (i, (name, url)) in targets.into_iter().enumerate() {
            let col_x = i as f64 * col_w;
            let actual_w = if i == 2 { logical_w - col_x } else { col_w };

            let bounds = wry::Rect {
                position: LogicalPosition::new(col_x, content_y).into(),
                size: LogicalSize::new(actual_w, content_h).into(),
            };

            let builder = self
                .comparator_webview_builder(i, name)
                .with_bounds(bounds)
                .with_url(url.as_str());

            match builder.build_as_child(window) {
                Ok(wv) => {
                    views.push(ComparatorView { webview: wv, name });
                }
                Err(error) => {
                    self.show_native_error(format!("WebView2 não pôde abrir {name}: {error}"));
                    return;
                }
            }
        }

        self.comparator = Some(ComparatorState {
            views,
            expanded: None,
        });
        self.bar_hover = None;
        self.surface = Surface::Comparator;
        self.request_redraw();
    }

    /// Alterna: o botao injetado na pagina pede sempre "expandir", e e aqui que
    /// isso vira "sair da tela cheia" quando a coluna ja esta expandida. Sem a
    /// barra nativa em tela cheia, esse botao e o Esc sao o caminho de volta.
    fn expand_comparator(&mut self, idx: usize) {
        let mut restored = false;
        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
        {
            if comp.expanded == Some(idx) {
                comp.expanded = None;
                restored = true;
            } else {
                comp.expanded = Some(idx);
            }
        }
        if restored {
            self.bar_hover = None;
        }
        self.update_comparator_layout();
        self.sync_comparator_buttons();
        self.request_redraw();
    }

    fn restore_comparator(&mut self) {
        if let Some(comp) = &mut self.comparator {
            comp.expanded = None;
        }
        self.update_comparator_layout();
        self.sync_comparator_buttons();
        self.request_redraw();
    }

    /// O botao vive dentro da pagina e nao sabe o estado; o app diz-lho.
    fn sync_comparator_buttons(&self) {
        let Some(comp) = &self.comparator else {
            return;
        };
        for (index, view) in comp.views.iter().enumerate() {
            let script = if comp.expanded == Some(index) {
                COMPARATOR_BUTTON_EXPANDED
            } else {
                COMPARATOR_BUTTON_COLLAPSED
            };
            let _ = view.webview.evaluate_script(script);
        }
    }

    fn update_comparator_layout(&self) {
        let (Some(window), Some(comp)) = (&self.window, &self.comparator) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;

        let content_h = (logical_h - TOP_BAR_HEIGHT).max(100.0);
        let content_y = TOP_BAR_HEIGHT;

        match comp.expanded {
            Some(idx) => {
                // Tela cheia e tela cheia: a barra sai da frente e o site fica
                // com a janela inteira. O regresso e o Esc ou o botao que a
                // propria pagina recebe injetado.
                for (i, v) in comp.views.iter().enumerate() {
                    if i == idx {
                        let _ = v.webview.set_bounds(wry::Rect {
                            position: LogicalPosition::new(0.0, 0.0).into(),
                            size: LogicalSize::new(logical_w, logical_h).into(),
                        });
                        let _ = v.webview.set_visible(true);
                    } else {
                        let _ = v.webview.set_visible(false);
                    }
                }
            }
            None => {
                let n = comp.views.len() as f64;
                let col_w = logical_w / n;
                for (i, v) in comp.views.iter().enumerate() {
                    let col_x = i as f64 * col_w;
                    let actual_w = if i == comp.views.len() - 1 {
                        logical_w - col_x
                    } else {
                        col_w
                    };
                    let _ = v.webview.set_bounds(wry::Rect {
                        position: LogicalPosition::new(col_x, content_y).into(),
                        size: LogicalSize::new(actual_w, content_h).into(),
                    });
                    let _ = v.webview.set_visible(true);
                }
            }
        }
    }

    fn comparator_webview_builder(
        &self,
        col_index: usize,
        col_name: &'static str,
    ) -> WebViewBuilder<'static> {
        let navigation_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();

        let init_script = format!(
            "window.__neuralia_col_index = {col_index}; window.__neuralia_col_name = '{col_name}';\n{COMPARATOR_INJECT_SCRIPT}"
        );

        WebViewBuilder::new()
            .with_initialization_script(init_script)
            .with_navigation_handler(move |target| {
                if target.eq_ignore_ascii_case("neuralia:home") {
                    let _ = navigation_proxy.send_event(UserEvent::HomeRequested);
                    return false;
                }
                if target.eq_ignore_ascii_case("neuralia:restore") {
                    let _ = navigation_proxy.send_event(UserEvent::RestoreComparator);
                    return false;
                }
                if target.starts_with("neuralia:expand") {
                    if let Ok(action_url) = Url::parse(&target)
                        && let Some((_, val)) = action_url.query_pairs().find(|(k, _)| k == "col")
                        && let Ok(idx) = val.parse::<usize>()
                    {
                        let _ = navigation_proxy.send_event(UserEvent::ExpandComparator(idx));
                    }
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

    fn bar_layout(&self) -> Option<BarLayout> {
        let (Some(window), Some(comp)) = (&self.window, &self.comparator) else {
            return None;
        };
        Some(BarLayout::new(
            window.inner_size().width as f64,
            window.scale_factor(),
            comp.expanded.is_some(),
            comp.views.len(),
        ))
    }

    fn update_bar_hover(&mut self) {
        let next = self
            .bar_layout()
            .and_then(|layout| layout.hit(self.cursor.0, self.cursor.1));
        if next != self.bar_hover {
            self.bar_hover = next;
            self.request_redraw();
        }
    }

    fn click_comparator(&mut self) {
        let hit = self
            .bar_layout()
            .and_then(|layout| layout.hit(self.cursor.0, self.cursor.1));
        match hit {
            Some(BarHit::Home) => self.show_home(),
            Some(BarHit::Column(index)) => self.expand_comparator(index),
            None => {}
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
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attributes = Window::default_attributes()
            .with_title("NeuralIA")
            .with_inner_size(LogicalSize::new(1120.0, 760.0))
            .with_min_inner_size(LogicalSize::new(700.0, 500.0));

        if let Some(icon) = get_app_icon() {
            attributes = attributes.with_window_icon(Some(icon));
        }

        match event_loop.create_window(attributes) {
            Ok(window) => {
                window.set_ime_allowed(true);
                self.window = Some(window);
                self.create_omnibox();
                self.request_redraw();

                // Abertura: a consulta padrao ja entra na omnibox e vai direto
                // para a tela de resultados, sem esperar Enter do utilizador.
                let startup = startup_input();
                if !startup.is_empty() {
                    self.set_omnibox_text(&startup);
                    let _ = self.proxy.send_event(UserEvent::SubmitText(startup));
                }
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
            UserEvent::ShowHistory => self.show_history(),
            UserEvent::ClearHistory => match self.history.clear() {
                None => {
                    self.show_home();
                    self.status = Some("A apagar o histórico local…".to_string());
                    self.request_redraw();
                }
                Some(result) => self.report_history_cleared(result),
            },
            UserEvent::HistoryCleared(result) => self.report_history_cleared(result),
            UserEvent::SubmitText(input) => {
                if self.surface == Surface::Home {
                    let input = input.trim().to_string();
                    if !input.is_empty() {
                        self.handle_input(input);
                    }
                }
            }
            UserEvent::OpenExternal(url) => self.web(url),
            UserEvent::ExpandComparator(idx) => {
                if self.surface == Surface::Comparator {
                    self.expand_comparator(idx);
                }
            }
            UserEvent::RestoreComparator => {
                if self.surface == Surface::Comparator {
                    self.restore_comparator();
                }
            }
            UserEvent::ReaderReady {
                generation,
                input,
                result,
            } => {
                if generation != self.current_generation() {
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
            WindowEvent::RedrawRequested => match self.surface {
                Surface::Home => {
                    if let Some(window) = &self.window {
                        draw_home(window, self.status.as_deref());
                    }
                }
                Surface::Comparator => {
                    if let Some(window) = &self.window
                        && let Some(comp) = &self.comparator
                    {
                        draw_comparator_bar(window, comp, self.bar_hover);
                    }
                }
                _ => {}
            },
            WindowEvent::Resized(_) => match self.surface {
                Surface::Home => {
                    self.position_omnibox();
                    self.request_redraw();
                }
                Surface::Comparator => {
                    self.update_comparator_layout();
                    self.request_redraw();
                }
                _ => {}
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.surface == Surface::Comparator {
                    self.update_bar_hover();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = (-1.0, -1.0);
                if self.surface == Surface::Comparator {
                    self.update_bar_hover();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => match self.surface {
                Surface::Home => self.click_home(),
                Surface::Comparator => self.click_comparator(),
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        if self.surface == Surface::Comparator
                            && let Some(comp) = &self.comparator
                            && comp.expanded.is_some()
                        {
                            self.restore_comparator();
                            return;
                        }
                        self.show_home();
                    }
                    Key::Character(ref c) if self.surface == Surface::Comparator => {
                        match c.as_str() {
                            "1" => self.expand_comparator(0),
                            "2" => self.expand_comparator(1),
                            "3" => self.expand_comparator(2),
                            "0" => self.restore_comparator(),
                            _ => {}
                        }
                    }
                    _ => {}
                }
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

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn window_hwnd(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}

fn draw_home(window: &Window, status: Option<&str>) {
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

        let theme = Theme::system();

        let background = CreateSolidBrush(rgb3(theme.page_bg));
        FillRect(hdc, &client, background);
        DeleteObject(background as _);

        SetBkMode(hdc, TRANSPARENT as i32);

        let logo_size = (104.0 * scale) as i32;
        let logo_x = ((width - logo_size as f64) / 2.0) as i32;
        let logo_y = (height * 0.20).clamp(70.0 * scale, 150.0 * scale) as i32;
        draw_logo_to_dc(hdc, logo_x, logo_y, logo_size, theme.page_bg);

        let title_font = create_font((-38.0 * scale) as i32, FW_BOLD as i32);
        let body_font = create_font((-17.0 * scale) as i32, FW_NORMAL as i32);
        let small_font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);

        let old_font = SelectObject(hdc, title_font as _);
        SetTextColor(hdc, rgb3(theme.fg));
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
        SetTextColor(hdc, rgb3(theme.fg_muted));
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

        // A barra de pesquisa: pilula suavizada por tras do EDIT nativo.
        fill_pill(
            hdc,
            layout.input,
            layout.input.height / 2.0,
            theme.surface,
            Some((theme.surface_line, scale)),
            theme.page_bg,
        );

        draw_button(hdc, layout.go, "Ir", true, scale, body_font, &theme);

        // A Home fica so com a marca e a barra. O estado aparece apenas quando
        // ha mesmo algo a dizer -- um erro ou uma leitura em curso -- em vez de
        // ocupar o ecra com um aviso permanente.
        if let Some(message) = status {
            SelectObject(hdc, small_font as _);
            SetTextColor(hdc, rgb3(theme.fg_muted));
            let mut status_rect = RECT {
                left: (32.0 * scale) as i32,
                top: client.bottom - (64.0 * scale) as i32,
                right: client.right - (32.0 * scale) as i32,
                bottom: client.bottom - (24.0 * scale) as i32,
            };
            draw_text(
                hdc,
                message,
                &mut status_rect,
                DT_CENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }

        SelectObject(hdc, old_font);
        DeleteObject(title_font as _);
        DeleteObject(body_font as _);
        DeleteObject(small_font as _);
        let _ = ReleaseDC(hwnd, hdc);
    }
}

fn draw_comparator_bar(window: &Window, comp: &ComparatorState, hover: Option<BarHit>) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };

    let hwnd = handle.hwnd.get() as HWND;
    let scale = window.scale_factor().max(1.0);
    let names: Vec<&str> = comp.views.iter().map(|view| view.name).collect();

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

        let width = client.right.max(1);
        let bar_h = (TOP_BAR_HEIGHT * scale).round() as i32;

        // Desenhar fora do ecra e fazer um BitBlt so no fim: sem isto a barra
        // pisca a cada movimento do rato, porque o hover obriga a redesenhar.
        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = if mem_dc.is_null() {
            std::ptr::null_mut()
        } else {
            CreateCompatibleBitmap(hdc, width, bar_h)
        };
        let buffered = !mem_dc.is_null() && !mem_bmp.is_null();
        let target = if buffered { mem_dc } else { hdc };
        let old_bmp = if buffered {
            SelectObject(mem_dc, mem_bmp as _)
        } else {
            std::ptr::null_mut()
        };

        paint_comparator_bar(
            target,
            width,
            scale,
            &names,
            comp.expanded,
            hover,
            &Theme::system(),
        );

        if buffered {
            BitBlt(hdc, 0, 0, width, bar_h, mem_dc, 0, 0, SRCCOPY);
            SelectObject(mem_dc, old_bmp);
            DeleteObject(mem_bmp as _);
            DeleteDC(mem_dc);
        } else if !mem_dc.is_null() {
            DeleteDC(mem_dc);
        }

        let _ = ReleaseDC(hwnd, hdc);
    }
}

/// Todo o desenho da barra de topo, num DC qualquer — o ecra em producao, um
/// bitmap em memoria nos testes, que e como este visual se inspeciona sem ecra.
#[allow(clippy::too_many_arguments)]
unsafe fn paint_comparator_bar(
    target: *mut core::ffi::c_void,
    width: i32,
    scale: f64,
    names: &[&str],
    expanded: Option<usize>,
    hover: Option<BarHit>,
    theme: &Theme,
) {
    let layout = BarLayout::new(width as f64, scale, expanded.is_some(), names.len());
    if !layout.visible {
        return;
    }
    let bar_h = layout.height.round() as i32;

    let bar_rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: bar_h,
    };
    let background = CreateSolidBrush(rgb3(theme.bar_bg));
    FillRect(target, &bar_rect, background);
    DeleteObject(background as _);

    let hairline = RECT {
        left: 0,
        top: bar_h - scale.round().max(1.0) as i32,
        right: width,
        bottom: bar_h,
    };
    let line = CreateSolidBrush(rgb3(theme.bar_line));
    FillRect(target, &hairline, line);
    DeleteObject(line as _);

    SetBkMode(target, TRANSPARENT as i32);
    let font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);
    let old_font = SelectObject(target, font as _);

    let home_fill = if hover == Some(BarHit::Home) {
        mix(theme.surface, theme.fg, 0.10)
    } else {
        theme.surface
    };
    draw_pill(
        target,
        layout.home,
        "Home",
        PillStyle::new(home_fill, theme.surface_line, theme.fg)
            .with_icon(ICON_SLOT_HOME, Some(theme.fg)),
        scale,
        font,
        theme.bar_bg,
    );

    for (index, name) in names.iter().enumerate().take(layout.columns_len) {
        let brand = theme.brand(index);
        let tint = if hover == Some(BarHit::Column(index)) {
            0.30
        } else {
            0.16
        };
        draw_pill(
            target,
            layout.columns[index],
            name,
            PillStyle::new(
                mix(theme.bar_bg, brand, tint),
                mix(theme.bar_bg, brand, 0.45),
                theme.fg,
            )
            .with_icon(index, None),
            scale,
            font,
            theme.bar_bg,
        );
    }

    SelectObject(target, font as _);
    SetTextColor(target, rgb3(theme.fg_muted));
    let mut hint_rect = RECT {
        left: layout.hint.x as i32,
        top: 0,
        right: (layout.hint.x + layout.hint.width - 10.0 * scale) as i32,
        bottom: bar_h,
    };
    draw_text(
        target,
        "Esc: voltar",
        &mut hint_rect,
        DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
    );

    SelectObject(target, old_font);
    DeleteObject(font as _);
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

/// (tamanho, cor de fundo, pixeis BGRX ja compostos)
type SplashCache = Option<(i32, Rgb, Vec<u8>)>;

static LOGO_IMAGE: OnceLock<RgbaImage> = OnceLock::new();
static SPLASH_CACHE: Mutex<SplashCache> = Mutex::new(None);

fn get_logo_image() -> &'static RgbaImage {
    LOGO_IMAGE.get_or_init(|| {
        let raw = include_bytes!("../../../assets/logo.png");
        image::load_from_memory(raw)
            .expect("assets/logo.png must be valid PNG")
            .to_rgba8()
    })
}

fn get_app_icon() -> Option<Icon> {
    let img = get_logo_image();
    let size = 64u32;
    let resized = image::imageops::resize(img, size, size, image::imageops::FilterType::Lanczos3);
    let mut rgba = resized.into_raw();
    let radius = (size as f32) * 0.20;
    for y in 0..size {
        for x in 0..size {
            let dx = if (x as f32) < radius {
                radius - (x as f32)
            } else if (x as f32) > (size as f32) - 1.0 - radius {
                (x as f32) - ((size as f32) - 1.0 - radius)
            } else {
                0.0
            };
            let dy = if (y as f32) < radius {
                radius - (y as f32)
            } else if (y as f32) > (size as f32) - 1.0 - radius {
                (y as f32) - ((size as f32) - 1.0 - radius)
            } else {
                0.0
            };
            if dx > 0.0 && dy > 0.0 {
                let dist = (dx * dx + dy * dy).sqrt();
                let idx = ((y * size + x) * 4) as usize;
                if dist > radius {
                    rgba[idx + 3] = 0;
                } else if dist > radius - 1.0 {
                    let coverage = (radius - dist).clamp(0.0, 1.0);
                    rgba[idx + 3] = ((rgba[idx + 3] as f32) * coverage) as u8;
                }
            }
        }
    }
    Icon::from_rgba(rgba, size, size).ok()
}

unsafe fn draw_logo_to_dc(
    hdc: *mut core::ffi::c_void,
    x: i32,
    y: i32,
    size: i32,
    bg_rgb: (u8, u8, u8),
) {
    if size <= 0 {
        return;
    }

    let pixels = {
        let mut cache = SPLASH_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached_sz, cached_bg, ref cached_pixels)) = *cache {
            if cached_sz == size && cached_bg == bg_rgb {
                cached_pixels.clone()
            } else {
                let p = render_logo_pixels(size, bg_rgb);
                *cache = Some((size, bg_rgb, p.clone()));
                p
            }
        } else {
            let p = render_logo_pixels(size, bg_rgb);
            *cache = Some((size, bg_rgb, p.clone()));
            p
        }
    };

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: (size * size * 4) as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }; 1],
    };

    StretchDIBits(
        hdc as _,
        x,
        y,
        size,
        size,
        0,
        0,
        size,
        size,
        pixels.as_ptr() as *const _,
        &bmi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}

fn render_logo_pixels(size: i32, bg_rgb: (u8, u8, u8)) -> Vec<u8> {
    let u_size = size as u32;
    let img = get_logo_image();
    let resized =
        image::imageops::resize(img, u_size, u_size, image::imageops::FilterType::Lanczos3);

    let bg_r = bg_rgb.0 as f32;
    let bg_g = bg_rgb.1 as f32;
    let bg_b = bg_rgb.2 as f32;
    let radius = (size as f32) * 0.20;

    let mut bgr_pixels = Vec::with_capacity((size * size * 4) as usize);

    for py in 0..size {
        for px in 0..size {
            let dx = if (px as f32) < radius {
                radius - (px as f32)
            } else if (px as f32) > (size as f32) - 1.0 - radius {
                (px as f32) - ((size as f32) - 1.0 - radius)
            } else {
                0.0
            };
            let dy = if (py as f32) < radius {
                radius - (py as f32)
            } else if (py as f32) > (size as f32) - 1.0 - radius {
                (py as f32) - ((size as f32) - 1.0 - radius)
            } else {
                0.0
            };

            let mask_alpha = if dx > 0.0 && dy > 0.0 {
                let dist = (dx * dx + dy * dy).sqrt();
                if dist > radius {
                    0.0
                } else if dist > radius - 1.0 {
                    (radius - dist).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            } else {
                1.0
            };

            let pixel = resized.get_pixel(px as u32, py as u32);
            let src_a = (pixel[3] as f32 / 255.0) * mask_alpha;
            let final_r = ((pixel[0] as f32) * src_a + bg_r * (1.0 - src_a)).round() as u8;
            let final_g = ((pixel[1] as f32) * src_a + bg_g * (1.0 - src_a)).round() as u8;
            let final_b = ((pixel[2] as f32) * src_a + bg_b * (1.0 - src_a)).round() as u8;

            bgr_pixels.push(final_b);
            bgr_pixels.push(final_g);
            bgr_pixels.push(final_r);
            bgr_pixels.push(0);
        }
    }
    bgr_pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::GetDIBits;

    #[test]
    fn test_stretch_dibits_on_screen_dc() {
        unsafe {
            let hdc = GetDC(core::ptr::null_mut());
            assert!(!hdc.is_null());
            let img = get_logo_image();
            assert_eq!(img.width(), 1254);
            let size = 104;
            let pixels = render_logo_pixels(size, (248, 249, 250));
            assert_eq!(pixels.len(), (size * size * 4) as usize);

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: (size * size * 4) as u32,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
                    rgbBlue: 0,
                    rgbGreen: 0,
                    rgbRed: 0,
                    rgbReserved: 0,
                }; 1],
            };

            let ret = StretchDIBits(
                hdc as _,
                0,
                0,
                size,
                size,
                0,
                0,
                size,
                size,
                pixels.as_ptr() as *const _,
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            ReleaseDC(core::ptr::null_mut(), hdc);
            assert!(ret > 0, "StretchDIBits failed with ret={ret}");
        }
    }
    /// Desenha a barra de topo para PNG num bitmap em memoria. E a unica forma
    /// de inspecionar o visual sem ter o ecra a frente; `NEURALIA_PREVIEW_DIR`
    /// escolhe onde ficam os ficheiros.
    #[test]
    fn render_comparator_bar_preview() {
        unsafe {
            let screen = GetDC(core::ptr::null_mut());
            assert!(!screen.is_null());

            let width = 1600i32;
            let height = TOP_BAR_HEIGHT as i32;
            let accent = system_accent();

            let cases: [(&str, Theme, Option<usize>, Option<BarHit>); 3] = [
                ("dark", Theme::dark(accent), None, None),
                (
                    "dark-hover",
                    Theme::dark(accent),
                    None,
                    Some(BarHit::Column(2)),
                ),
                ("light-expanded", Theme::light(accent), Some(1), None),
            ];

            for (name, theme, expanded, hover) in cases {
                let mem = CreateCompatibleDC(screen);
                let bitmap = CreateCompatibleBitmap(screen, width, height);
                assert!(!mem.is_null() && !bitmap.is_null());
                let old = SelectObject(mem, bitmap as _);

                paint_comparator_bar(
                    mem,
                    width,
                    1.0,
                    &["Google Gemini", "ChatGPT", "Claude"],
                    expanded,
                    hover,
                    &theme,
                );

                let mut info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        biSizeImage: (width * height * 4) as u32,
                        biXPelsPerMeter: 0,
                        biYPelsPerMeter: 0,
                        biClrUsed: 0,
                        biClrImportant: 0,
                    },
                    bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
                        rgbBlue: 0,
                        rgbGreen: 0,
                        rgbRed: 0,
                        rgbReserved: 0,
                    }; 1],
                };

                let mut pixels = vec![0u8; (width * height * 4) as usize];
                let copied = GetDIBits(
                    mem,
                    bitmap,
                    0,
                    height as u32,
                    pixels.as_mut_ptr() as *mut _,
                    &mut info,
                    DIB_RGB_COLORS,
                );
                assert!(copied > 0, "GetDIBits falhou");

                let mut rgba = Vec::with_capacity(pixels.len());
                for bgrx in pixels.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[bgrx[2], bgrx[1], bgrx[0], 255]);
                }

                let dir = std::env::var_os("NEURALIA_PREVIEW_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(std::env::temp_dir);
                let path = dir.join(format!("neuralia-bar-{name}.png"));
                image::save_buffer(
                    &path,
                    &rgba,
                    width as u32,
                    height as u32,
                    image::ExtendedColorType::Rgba8,
                )
                .expect("gravar o PNG de pre-visualizacao");
                println!("preview: {}", path.display());

                SelectObject(mem, old);
                DeleteObject(bitmap as _);
                DeleteDC(mem);
            }

            ReleaseDC(core::ptr::null_mut(), screen);
        }
    }

    #[test]
    fn bar_layout_hit_matches_drawing() {
        let layout = BarLayout::new(1600.0, 1.0, false, 3);

        // Cada pilula centrada sobre a sua coluna, com 2px de tolerancia.
        for (index, rect) in layout.columns.iter().enumerate().take(3) {
            let column_center = 1600.0 / 3.0 * (index as f64 + 0.5);
            let pill_center = rect.x + rect.width / 2.0;
            assert!(
                (pill_center - column_center).abs() < 2.0,
                "pilula {index} centrada em {pill_center}, coluna em {column_center}"
            );
        }

        for index in 0..3 {
            let rect = layout.columns[index];
            let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            assert_eq!(layout.hit(center.0, center.1), Some(BarHit::Column(index)));
        }
        let home = layout.home;
        assert_eq!(
            layout.hit(home.x + 2.0, home.y + 2.0),
            Some(BarHit::Home),
            "o canto da pilula Home tem de responder ao clique"
        );
        assert_eq!(layout.hit(800.0, layout.height + 5.0), None);

        // Em tela cheia nao ha barra nenhuma: nem se desenha, nem se clica.
        let expanded = BarLayout::new(1600.0, 1.0, true, 3);
        assert!(!expanded.visible);
        assert_eq!(expanded.height, 0.0);
        assert_eq!(expanded.columns_len, 0);
        for y in [0.0, 1.0, 20.0, 41.0] {
            for x in [0.0, 30.0, 400.0, 1599.0] {
                assert_eq!(
                    expanded.hit(x, y),
                    None,
                    "({x}, {y}) nao devia acertar nada"
                );
            }
        }
    }
}

// ===================== tema do sistema (cor de destaque + claro/escuro) =====================

type Rgb = (u8, u8, u8);

/// Le um DWORD do HKEY_CURRENT_USER; None se a chave nao existir.
fn registry_dword(subkey: &str, value: &str) -> Option<u32> {
    let subkey = wide_null(subkey);
    let value = wide_null(value);
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut core::ffi::c_void,
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

/// Cor de destaque escolhida em Definicoes > Personalizacao > Cores.
/// O Windows guarda-a como 0xAABBGGRR.
fn system_accent() -> Rgb {
    match registry_dword("Software\\Microsoft\\Windows\\DWM", "AccentColor") {
        Some(value) => (
            (value & 0xFF) as u8,
            ((value >> 8) & 0xFF) as u8,
            ((value >> 16) & 0xFF) as u8,
        ),
        None => (0, 120, 212),
    }
}

fn system_dark_mode() -> bool {
    registry_dword(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    )
    .map(|value| value == 0)
    .unwrap_or(false)
}

fn mix(base: Rgb, tint: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    let blend = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    (
        blend(base.0, tint.0),
        blend(base.1, tint.1),
        blend(base.2, tint.2),
    )
}

fn channel_luminance(channel: u8) -> f32 {
    let c = channel as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(color: Rgb) -> f32 {
    0.2126 * channel_luminance(color.0)
        + 0.7152 * channel_luminance(color.1)
        + 0.0722 * channel_luminance(color.2)
}

fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Preto ou branco — o que for legivel por cima de `background`.
fn on_color(background: Rgb) -> Rgb {
    if contrast((255, 255, 255), background) >= contrast((17, 19, 20), background) {
        (255, 255, 255)
    } else {
        (17, 19, 20)
    }
}

/// Clareia/escurece `color` ate ter contraste suficiente com o fundo: a cor de
/// destaque do utilizador pode ser preta num tema escuro.
fn readable(color: Rgb, background: Rgb, minimum: f32) -> Rgb {
    let target = if luminance(background) > 0.35 {
        (0, 0, 0)
    } else {
        (255, 255, 255)
    };
    let mut amount = 0.0;
    let mut out = color;
    while contrast(out, background) < minimum && amount < 1.0 {
        amount += 0.05;
        out = mix(color, target, amount);
    }
    out
}

/// Cores de marca das tres IAs comparadas.
const BRAND_COLORS: [Rgb; COMPARATOR_COLUMNS] = [(66, 133, 244), (16, 163, 127), (217, 119, 87)];

#[derive(Debug, Clone, Copy)]
struct Theme {
    accent: Rgb,
    page_bg: Rgb,
    bar_bg: Rgb,
    bar_line: Rgb,
    surface: Rgb,
    surface_line: Rgb,
    fg: Rgb,
    fg_muted: Rgb,
}

impl Theme {
    fn system() -> Self {
        let accent = system_accent();
        if system_dark_mode() {
            Self::dark(accent)
        } else {
            Self::light(accent)
        }
    }

    fn dark(accent: Rgb) -> Self {
        let bar_bg = (27, 30, 32);
        Self {
            accent: readable(accent, bar_bg, 3.2),
            page_bg: (22, 24, 26),
            bar_bg,
            bar_line: (44, 48, 51),
            surface: (37, 41, 44),
            surface_line: (54, 59, 64),
            fg: (233, 236, 239),
            fg_muted: (152, 159, 166),
        }
    }

    fn light(accent: Rgb) -> Self {
        let bar_bg = (255, 255, 255);
        Self {
            accent: readable(accent, bar_bg, 3.2),
            page_bg: (248, 249, 250),
            bar_bg,
            bar_line: (226, 229, 233),
            surface: (242, 244, 246),
            surface_line: (219, 223, 228),
            fg: (26, 29, 32),
            fg_muted: (106, 112, 119),
        }
    }

    fn brand(&self, index: usize) -> Rgb {
        BRAND_COLORS[index.min(COMPARATOR_COLUMNS - 1)]
    }
}

// ===================== desenho com anti-aliasing =====================

/// Distancia com sinal ate um retangulo de cantos redondos — negativa por dentro.
fn round_rect_sdf(px: f32, py: f32, width: f32, height: f32, radius: f32) -> f32 {
    let qx = (px - width * 0.5).abs() - (width * 0.5 - radius);
    let qy = (py - height * 0.5).abs() - (height * 0.5 - radius);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - radius
}

unsafe fn blit_bgrx(
    hdc: *mut core::ffi::c_void,
    pixels: &[u8],
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    if width <= 0 || height <= 0 || pixels.len() < (width * height * 4) as usize {
        return;
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: (width * height * 4) as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }; 1],
    };

    StretchDIBits(
        hdc as _,
        x,
        y,
        width,
        height,
        0,
        0,
        width,
        height,
        pixels.as_ptr() as *const _,
        &bmi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}

/// Pilula de cantos suaves. O GDI nao tem anti-aliasing nem alfa, por isso
/// compomos os pixeis a mao por cima da cor de fundo conhecida e fazemos blit.
unsafe fn fill_pill(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    radius: f64,
    fill: Rgb,
    border: Option<(Rgb, f64)>,
    background: Rgb,
) {
    let width = rect.width.round() as i32;
    let height = rect.height.round() as i32;
    if width <= 0 || height <= 0 {
        return;
    }

    let (border_color, border_width) = border.unwrap_or((fill, 0.0));
    let border_width = border_width as f32;
    let radius = radius.min(rect.width.min(rect.height) / 2.0).max(0.0) as f32;

    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for py in 0..height {
        for px in 0..width {
            let distance = round_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                width as f32,
                height as f32,
                radius,
            );
            let outer = (0.5 - distance).clamp(0.0, 1.0);
            let inner = (0.5 - (distance + border_width)).clamp(0.0, 1.0);
            let edge = (outer - inner).max(0.0);
            let rest = 1.0 - outer;

            let channel = |bg: u8, line: u8, body: u8| {
                (bg as f32 * rest + line as f32 * edge + body as f32 * inner).round() as u8
            };

            pixels.push(channel(background.2, border_color.2, fill.2));
            pixels.push(channel(background.1, border_color.1, fill.1));
            pixels.push(channel(background.0, border_color.0, fill.0));
            pixels.push(0);
        }
    }

    blit_bgrx(
        hdc,
        &pixels,
        rect.x.round() as i32,
        rect.y.round() as i32,
        width,
        height,
    );
}

/// Slot 0..2 = icones das IAs, slot 3 = glifo da casa (pintado com a cor do tema).
const ICON_SLOT_HOME: usize = COMPARATOR_COLUMNS;

static AI_ICON_IMAGES: [OnceLock<RgbaImage>; COMPARATOR_COLUMNS] =
    [OnceLock::new(), OnceLock::new(), OnceLock::new()];
static HOME_ICON_IMAGE: OnceLock<RgbaImage> = OnceLock::new();
static ICON_SCALE_CACHE: Mutex<Vec<(usize, u32, RgbaImage)>> = Mutex::new(Vec::new());

fn ai_icon(index: usize) -> &'static RgbaImage {
    AI_ICON_IMAGES[index.min(COMPARATOR_COLUMNS - 1)].get_or_init(|| {
        let raw: &[u8] = match index {
            0 => include_bytes!("../../../assets/ai/gemini.png"),
            1 => include_bytes!("../../../assets/ai/chatgpt.png"),
            _ => include_bytes!("../../../assets/ai/claude.png"),
        };
        image::load_from_memory(raw)
            .expect("assets/ai/*.png must be valid PNG")
            .to_rgba8()
    })
}

/// Redimensiona uma vez por (icone, tamanho): o Lanczos3 e caro de mais para
/// correr a cada WM_PAINT, e a barra redesenha-se a cada movimento do rato.
fn home_icon() -> &'static RgbaImage {
    HOME_ICON_IMAGE.get_or_init(|| {
        image::load_from_memory(include_bytes!("../../../assets/ai/home.png"))
            .expect("assets/ai/home.png must be valid PNG")
            .to_rgba8()
    })
}

fn icon_scaled(slot: usize, size: u32) -> RgbaImage {
    let mut cache = ICON_SCALE_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((_, _, image)) = cache
        .iter()
        .find(|(cached_slot, cached_size, _)| *cached_slot == slot && *cached_size == size)
    {
        return image.clone();
    }

    let source = if slot == ICON_SLOT_HOME {
        home_icon()
    } else {
        ai_icon(slot)
    };
    let scaled = image::imageops::resize(source, size, size, image::imageops::FilterType::Lanczos3);
    cache.push((slot, size, scaled.clone()));
    scaled
}

/// `tint` substitui a cor do icone mantendo o alfa — e assim que o glifo da
/// casa segue o tema sem existirem dois PNGs.
unsafe fn draw_icon(
    hdc: *mut core::ffi::c_void,
    slot: usize,
    x: i32,
    y: i32,
    size: i32,
    background: Rgb,
    tint: Option<Rgb>,
) {
    if size <= 0 {
        return;
    }

    let image = icon_scaled(slot, size as u32);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size as u32 {
        for px in 0..size as u32 {
            let pixel = image.get_pixel(px, py);
            let alpha = pixel[3] as f32 / 255.0;
            let source = tint.unwrap_or((pixel[0], pixel[1], pixel[2]));
            let channel = |value: u8, bg: u8| {
                (value as f32 * alpha + bg as f32 * (1.0 - alpha)).round() as u8
            };
            pixels.push(channel(source.2, background.2));
            pixels.push(channel(source.1, background.1));
            pixels.push(channel(source.0, background.0));
            pixels.push(0);
        }
    }
    blit_bgrx(hdc, &pixels, x, y, size, size);
}

#[derive(Debug, Clone, Copy)]
struct PillStyle {
    fill: Rgb,
    border: Rgb,
    text: Rgb,
    icon: Option<usize>,
    icon_tint: Option<Rgb>,
}

impl PillStyle {
    fn new(fill: Rgb, border: Rgb, text: Rgb) -> Self {
        Self {
            fill,
            border,
            text,
            icon: None,
            icon_tint: None,
        }
    }

    fn with_icon(mut self, slot: usize, tint: Option<Rgb>) -> Self {
        self.icon = Some(slot);
        self.icon_tint = tint;
        self
    }
}

/// Pilula com icone a esquerda e legenda; sem icone, a legenda fica centrada.
unsafe fn draw_pill(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    label: &str,
    style: PillStyle,
    scale: f64,
    font: *mut core::ffi::c_void,
    background: Rgb,
) {
    fill_pill(
        hdc,
        rect,
        rect.height / 2.0,
        style.fill,
        Some((style.border, scale)),
        background,
    );

    let padding = 11.0 * scale;
    let mut text_left = rect.x + padding;
    let mut format = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX;

    match style.icon {
        Some(slot) => {
            let size = (18.0 * scale).round() as i32;
            let icon_y = (rect.y + (rect.height - size as f64) / 2.0).round() as i32;
            draw_icon(
                hdc,
                slot,
                text_left.round() as i32,
                icon_y,
                size,
                style.fill,
                style.icon_tint,
            );
            text_left += size as f64 + 8.0 * scale;
        }
        None => format |= DT_CENTER,
    }

    SelectObject(hdc, font as _);
    SetTextColor(hdc, rgb3(style.text));
    let mut text_rect = RECT {
        left: text_left.round() as i32,
        top: rect.y as i32,
        right: (rect.x + rect.width - padding * 0.6) as i32,
        bottom: (rect.y + rect.height) as i32,
    };
    draw_text(hdc, label, &mut text_rect, format);
}

unsafe fn draw_button(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    label: &str,
    primary: bool,
    scale: f64,
    font: *mut core::ffi::c_void,
    theme: &Theme,
) {
    let style = if primary {
        PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
    } else {
        PillStyle::new(theme.surface, theme.surface_line, theme.fg)
    };
    draw_pill(hdc, rect, label, style, scale, font, theme.page_bg);
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

const fn rgb3(color: Rgb) -> u32 {
    rgb(color.0, color.1, color.2)
}

/// Rotulos do botao injetado no comparador. Em tela cheia a barra nativa some,
/// por isso este botao tem de anunciar a saida.
const COMPARATOR_BUTTON_EXPANDED: &str = "(function(){var b=document.querySelector('#neuralia-comp-btn button');if(b){b.textContent='\u{26F6} Sair da tela cheia';}})();";
const COMPARATOR_BUTTON_COLLAPSED: &str = "(function(){var b=document.querySelector('#neuralia-comp-btn button');if(b){b.textContent='\u{26F6} Expandir ' + (window.__neuralia_col_name || 'IA');}})();";

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

const COMPARATOR_INJECT_SCRIPT: &str = r#"
document.addEventListener('DOMContentLoaded', () => {
  if (document.getElementById('neuralia-comp-btn')) return;
  const colIndex = window.__neuralia_col_index ?? 0;
  const colName = window.__neuralia_col_name ?? 'IA';

  const wrap = document.createElement('div');
  wrap.id = 'neuralia-comp-btn';
  Object.assign(wrap.style, {
    position: 'fixed',
    top: '10px',
    right: '12px',
    zIndex: '2147483647',
    display: 'flex',
    gap: '6px',
    fontFamily: 'Segoe UI, -apple-system, BlinkMacSystemFont, sans-serif'
  });

  const btn = document.createElement('button');
  btn.textContent = '⛶ Expandir ' + colName;
  Object.assign(btn.style, {
    border: '0',
    borderRadius: '6px',
    padding: '6px 12px',
    background: '#111314',
    color: '#ffffff',
    fontSize: '11px',
    fontWeight: '600',
    boxShadow: '0 4px 12px rgba(0,0,0,0.3)',
    cursor: 'pointer',
    opacity: '0.9',
    transition: 'transform 0.15s ease'
  });
  btn.onmouseover = () => { btn.style.transform = 'scale(1.05)'; };
  btn.onmouseout = () => { btn.style.transform = 'scale(1)'; };
  btn.onclick = (e) => {
    e.preventDefault();
    e.stopPropagation();
    window.location.href = 'neuralia:expand?col=' + colIndex;
  };
  wrap.appendChild(btn);

  document.documentElement.appendChild(wrap);
});
"#;
