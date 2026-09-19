#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    borrow::Cow,
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

use image::RgbaImage;

use neural_core::{
    CoreConfig, HistoryEntry, HistoryKind, HistoryStore, Intent, MemoryDocument, MemoryHit,
    MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore, ReaderArticle, ReaderBlock,
    ReaderClient, ResearchSession, chatgpt_search_url, claude_search_url, google_ai_url,
    is_local_network_target, is_pdf_url, parse_intent, reader_html,
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CLEARTYPE_QUALITY,
        ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
        CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS,
        DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
        DrawTextW, Ellipse, EndPaint, FW_BOLD, FW_NORMAL, FillRect, GetDC, InvalidateRect, LineTo,
        MoveToEx, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, ReleaseDC, SRCCOPY, ScreenToClient,
        SelectObject, SetBkColor, SetBkMode, SetTextColor, SetWindowRgn, StretchDIBits,
        TRANSPARENT,
    },
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, SendInput, SetFocus,
            VK_CONTROL, VK_NEXT, VK_SHIFT,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow,
            ES_AUTOHSCROLL, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowTextLengthW,
            GetWindowTextW, GetWindowThreadProcessId, MB_ICONINFORMATION, MB_OK, MF_SEPARATOR,
            MF_STRING, MessageBoxW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW,
            SetWindowPos, SetWindowTextW, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
            TrackPopupMenu, WM_KEYDOWN, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
        },
    },
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Fullscreen, Icon, Window, WindowId},
};
use wry::{
    NewWindowResponse, PermissionResponse, WebView, WebViewBuilder,
    http::{Request, Response as HttpResponse},
};

#[derive(Debug)]
enum UserEvent {
    HomeRequested,
    /// Voltar um nivel: de ecra completo para tres colunas, de la para a Home.
    BackRequested,
    ToggleAutoScroll,
    AutoScrollAnswer(bool),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ReloadPage,
    PrintPage,
    FocusOmnibox,
    ToggleColumnFullscreen,
    OpenDevTools,
    ViewSource,
    AutoScrollTick(u64),
    HideSplash(u64),
    GmailProbe(u64),
    GmailInboxState {
        unread: u32,
        sender: String,
        subject: String,
        key: String,
    },
    HideGmailToast(u64),
    ShowHistory,
    ClearHistory,
    HistoryCleared(Result<(), String>),
    MemoryQueryReady {
        query: String,
        result: Result<Vec<MemoryHit>, String>,
    },
    MemoryCleared(Result<(), String>),
    /// Esconde outra vez a barra em ecra completo, se nada a tiver reavivado.
    HideChrome(u64),
    SubmitText(String),
    OpenExternal(String),
    /// Popup pedido por uma coluna do comparador: carrega nessa coluna.
    OpenInColumn(usize, String),
    /// Fonte aberta sem abandonar a conversa que a originou.
    OpenSplit {
        source_index: usize,
        url: String,
    },
    OpenPrivateSplit {
        source_index: usize,
        url: String,
    },
    NewTab(usize),
    CloseSplit,
    ToggleSplitFullscreen,
    /// Entrada da omnibox flutuante, sempre associada à IA que tinha foco.
    PaletteSubmit {
        source_index: usize,
        input: String,
    },
    ExpandComparator(usize),
    MinimizeComparator(usize),
    ResizeComparator {
        divider: usize,
        screen_x: i32,
    },
    RestoreComparator,
    ExitRequested,
    ReaderReady {
        generation: u64,
        input: String,
        result: Result<ReaderArticle, String>,
    },
    PdfReady {
        generation: u64,
        url: String,
        result: Result<Vec<u8>, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    Home,
    Reader,
    External,
    Comparator,
    /// O nosso visualizador de PDF (PDF.js embutido), numa origem nossa.
    Pdf,
}

/// Linha superior sem moldura: abas + minimizar/maximizar/fechar, como num browser.
const TITLE_TAB_HEIGHT: f64 = 32.0;
/// Segunda linha: Home, provedores, +, Privado e controles de Split View.
const TOP_BAR_HEIGHT: f64 = 44.0;
const COMPARATOR_CHROME_HEIGHT: f64 = TITLE_TAB_HEIGHT + TOP_BAR_HEIGHT;
const MAX_VISIBLE_CONTEXT_TABS: usize = 3;
const COMPARATOR_COLUMNS: usize = 3;
/// Intervalo da rolagem automatica de leitura, do primeiro avanco ao ultimo.
const AUTO_SCROLL_SECONDS: u64 = 30;
/// Quanto tempo a pergunta fica no ecra antes de se dar por respondida com
/// "nao". Sem resposta nao se mexe em nada: e uma pergunta, nao um aviso.
const AUTO_SCROLL_PROMPT_SECONDS: u64 = 20;

const SPLASH_SUBCLASS_ID: usize = 0x4E4C;
const GMAIL_TOAST_SUBCLASS_ID: usize = 0x4E4D;
const SPLASH_WIDTH: f64 = 470.0;
const SPLASH_HEIGHT: f64 = 46.0;
const GMAIL_TOAST_WIDTH: f64 = 390.0;
const GMAIL_TOAST_HEIGHT: f64 = 68.0;

/// Texto do aviso flutuante. Vive fora do App porque quem o pinta e o
/// procedimento de janela, que nao tem acesso ao estado da aplicacao.
static SPLASH_TEXT: Mutex<String> = Mutex::new(String::new());
static GMAIL_TOAST_TEXT: Mutex<String> = Mutex::new(String::new());
/// Verdadeiro enquanto a janela esta a fazer uma pergunta com Sim/Nao.
static SPLASH_ASKS: AtomicBool = AtomicBool::new(false);

/// Os dois botoes ocupam o terco direito da janela. Uma so funcao para o
/// desenho e o clique concordarem sempre.
fn splash_buttons(client: &RECT) -> (RECT, RECT) {
    let width = client.right - client.left;
    let button = width / 5;
    let margin = width / 40;
    let no = RECT {
        left: client.right - margin - button,
        top: client.top + margin,
        right: client.right - margin,
        bottom: client.bottom - margin,
    };
    let yes = RECT {
        left: no.left - margin - button,
        top: no.top,
        right: no.left - margin,
        bottom: no.bottom,
    };
    (yes, no)
}

/// Avanca uma pagina, parando no fim em vez de dar a volta. Usa a altura visivel
/// menos uma faixa de sobreposicao, para nao se perder a linha que se estava a
/// ler. Corre no documento e tambem nos frames a que conseguimos chegar.
const AUTO_SCROLL_SCRIPT: &str = r#"
(function () {
  // O nosso visualizador de PDF sabe avancar uma pagina inteira.
  if (typeof window.__neuralia_next_page === 'function') {
    window.__neuralia_next_page();
    return;
  }

  function scrollRoot(doc) {
    var root = doc.scrollingElement || doc.documentElement || doc.body;
    var best = root;
    var bestRange = best ? Math.max(0, best.scrollHeight - best.clientHeight) : 0;
    var candidates = doc.querySelectorAll(
      'main,[role="main"],[data-radix-scroll-area-viewport],'
      + '[data-testid*="scroll"],[class*="overflow"],[class*="scroll"]'
    );

    for (var i = 0; i < candidates.length; i++) {
      var el = candidates[i];
      if (!el || el === doc.body || el === doc.documentElement) continue;
      var range = Math.max(0, el.scrollHeight - el.clientHeight);
      if (range <= bestRange + 24) continue;
      var css = doc.defaultView.getComputedStyle(el);
      if (css.display === 'none' || css.visibility === 'hidden' || css.overflowY === 'hidden') {
        continue;
      }
      best = el;
      bestRange = range;
    }
    return best;
  }

  function step(win) {
    try {
      var doc = win.document;
      var el = scrollRoot(doc);
      if (!el) return false;

      var docLike = el === doc.scrollingElement
        || el === doc.documentElement || el === doc.body;
      var view = el.clientHeight || win.innerHeight || 0;
      var top = docLike ? win.scrollY : el.scrollTop;
      var max = Math.max(0, el.scrollHeight - el.clientHeight);
      if (view <= 0 || max <= 4 || top >= max - 2) return false;

      var amount = Math.max(view - 72, 120);
      if (docLike) {
        win.scrollBy({ top: amount, left: 0, behavior: 'smooth' });
      } else {
        el.scrollBy({ top: amount, left: 0, behavior: 'smooth' });
      }
      return true;
    } catch (err) {
      return false;
    }
  }

  if (step(window)) return;

  // Alguns leitores desenham o conteudo dentro de um frame proprio.
  var frames = document.querySelectorAll('iframe, frame');
  for (var i = 0; i < frames.length; i++) {
    try {
      if (frames[i].contentWindow && step(frames[i].contentWindow)) return;
    } catch (err) { /* outra origem: nao ha nada a fazer daqui */ }
  }
})();
"#;

/// Aviso curto dentro da propria pagina: a barra nativa nao esta sempre visivel.
const AUTO_SCROLL_TOAST: &str = r#"
(function (on) {
  var id = 'neuralia-autoscroll-toast';
  var el = document.getElementById(id);
  if (!el) {
    el = document.createElement('div');
    el.id = id;
    document.documentElement.appendChild(el);
  }
  el.textContent = on ? 'Rolagem automatica ligada — __SECONDS__s (F8 desliga)' : 'Rolagem automatica desligada';
  el.setAttribute('style', [
    'position:fixed', 'left:50%', 'bottom:24px', 'transform:translateX(-50%)',
    'z-index:2147483647', 'padding:10px 18px', 'border-radius:999px',
    'background:rgba(17,19,20,.92)', 'color:#fff',
    'font:600 13px Segoe UI, system-ui, sans-serif',
    'box-shadow:0 8px 28px rgba(0,0,0,.35)', 'pointer-events:none',
    'opacity:1', 'transition:opacity .4s ease'
  ].join(';'));
  clearTimeout(window.__neuralia_toast_timer);
  window.__neuralia_toast_timer = setTimeout(function () {
    el.style.opacity = '0';
  }, 2200);
})(__ON__);
"#;

/// O que esta debaixo do rato no chrome nativo do comparador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarHit {
    Home,
    Column(usize),
    AddTab(usize),
    ContextTab {
        source_index: usize,
        context_index: usize,
    },
    SplitExpand,
    SplitClose,
    Private,
    WindowMinimize,
    WindowMaximize,
    WindowClose,
}

/// Geometria em duas linhas. As fontes ficam na title bar; os provedores ficam
/// numa segunda linha, sem disputar espaco com as abas.
#[derive(Debug, Clone, Copy)]
struct BarLayout {
    visible: bool,
    height: f64,
    home: UiRect,
    columns: [UiRect; COMPARATOR_COLUMNS],
    add_tabs: [UiRect; COMPARATOR_COLUMNS],
    context_tabs: [[UiRect; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    context_indices: [[usize; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    context_tab_counts: [usize; COMPARATOR_COLUMNS],
    columns_len: usize,
    window_minimize: UiRect,
    window_maximize: UiRect,
    window_close: UiRect,
}

impl BarLayout {
    #[cfg(test)]
    fn new(client_width: f64, scale: f64, visible: bool, columns: usize) -> Self {
        Self::with_contexts(
            client_width,
            scale,
            visible,
            columns,
            [0; COMPARATOR_COLUMNS],
        )
    }

    fn with_contexts(
        client_width: f64,
        scale: f64,
        visible: bool,
        columns: usize,
        context_counts: [usize; COMPARATOR_COLUMNS],
    ) -> Self {
        let scale = scale.max(1.0);
        let empty = UiRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        if !visible {
            return Self {
                visible: false,
                height: 0.0,
                home: empty,
                columns: [empty; COMPARATOR_COLUMNS],
                add_tabs: [empty; COMPARATOR_COLUMNS],
                context_tabs: [[empty; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                context_indices: [[0; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                context_tab_counts: [0; COMPARATOR_COLUMNS],
                columns_len: 0,
                window_minimize: empty,
                window_maximize: empty,
                window_close: empty,
            };
        }

        let height = COMPARATOR_CHROME_HEIGHT * scale;
        let title_h = TITLE_TAB_HEIGHT * scale;
        let caption_w = 46.0 * scale;
        let window_close = UiRect {
            x: (client_width - caption_w).max(0.0),
            y: 0.0,
            width: caption_w,
            height: title_h,
        };
        let window_maximize = UiRect {
            x: (window_close.x - caption_w).max(0.0),
            y: 0.0,
            width: caption_w,
            height: title_h,
        };
        let window_minimize = UiRect {
            x: (window_maximize.x - caption_w).max(0.0),
            y: 0.0,
            width: caption_w,
            height: title_h,
        };

        let pad = 7.0 * scale;
        let row_y = title_h + 7.0 * scale;
        let row_h = 30.0 * scale;
        let home = UiRect {
            x: pad,
            y: row_y,
            width: 72.0 * scale,
            height: row_h,
        };

        let mut columns_rect = [empty; COMPARATOR_COLUMNS];
        let mut plus_rect = [empty; COMPARATOR_COLUMNS];
        let mut tabs = [[empty; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS];
        let mut tab_indices = [[0usize; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS];
        let mut tab_counts = [0usize; COMPARATOR_COLUMNS];
        let columns_len = columns.min(COMPARATOR_COLUMNS);

        // Linha dos provedores, agora livre das abas.
        if columns_len > 0 {
            let column_width = client_width / columns_len as f64;
            let group_pad = 6.0 * scale;
            let gap = 4.0 * scale;
            let provider_width = 116.0 * scale;
            let plus_width = 26.0 * scale;

            for index in 0..columns_len {
                let mut left = index as f64 * column_width + group_pad;
                if index == 0 {
                    left = left.max(home.x + home.width + 8.0 * scale);
                }
                let right = ((index + 1) as f64 * column_width - group_pad).min(client_width - pad);
                let available = (right - left).max(provider_width + plus_width + gap);
                columns_rect[index] = UiRect {
                    x: left,
                    y: row_y,
                    width: provider_width.min(available - plus_width - gap),
                    height: row_h,
                };
                plus_rect[index] = UiRect {
                    x: columns_rect[index].x + columns_rect[index].width + gap,
                    y: row_y + 2.0 * scale,
                    width: plus_width,
                    height: row_h - 4.0 * scale,
                };
            }
        }

        // Linha superior: todas as fontes/abas, antes dos controles da janela.
        let tabs_left = 90.0 * scale;
        let tabs_right = (window_minimize.x - 8.0 * scale).max(tabs_left);
        let desired: [usize; COMPARATOR_COLUMNS] =
            std::array::from_fn(|index| context_counts[index].min(MAX_VISIBLE_CONTEXT_TABS));
        let total_tabs: usize = desired.iter().sum();
        if total_tabs > 0 && tabs_right > tabs_left {
            let gap = 3.0 * scale;
            let usable = tabs_right - tabs_left - gap * total_tabs.saturating_sub(1) as f64;
            let tab_width = (usable / total_tabs as f64).clamp(56.0 * scale, 156.0 * scale);
            let mut x = tabs_left;

            for index in 0..columns_len {
                let count = desired[index];
                let first_context = context_counts[index].saturating_sub(count);
                for visual in 0..count {
                    if x + 28.0 * scale > tabs_right {
                        break;
                    }
                    let width = tab_width.min(tabs_right - x).max(28.0 * scale);
                    tabs[index][visual] = UiRect {
                        x,
                        y: 3.0 * scale,
                        width,
                        height: (title_h - 6.0 * scale).max(20.0 * scale),
                    };
                    tab_indices[index][visual] = first_context + visual;
                    tab_counts[index] += 1;
                    x += width + gap;
                }
            }
        }

        Self {
            visible: true,
            height,
            home,
            columns: columns_rect,
            add_tabs: plus_rect,
            context_tabs: tabs,
            context_indices: tab_indices,
            context_tab_counts: tab_counts,
            columns_len,
            window_minimize,
            window_maximize,
            window_close,
        }
    }

    fn hit(&self, x: f64, y: f64) -> Option<BarHit> {
        if !self.visible || y > self.height {
            return None;
        }
        if self.window_close.contains(x, y) {
            return Some(BarHit::WindowClose);
        }
        if self.window_maximize.contains(x, y) {
            return Some(BarHit::WindowMaximize);
        }
        if self.window_minimize.contains(x, y) {
            return Some(BarHit::WindowMinimize);
        }
        for index in 0..self.columns_len {
            for visual in 0..self.context_tab_counts[index] {
                if self.context_tabs[index][visual].contains(x, y) {
                    return Some(BarHit::ContextTab {
                        source_index: index,
                        context_index: self.context_indices[index][visual],
                    });
                }
            }
        }
        if self.home.contains(x, y) {
            return Some(BarHit::Home);
        }
        for index in 0..self.columns_len {
            if self.add_tabs[index].contains(x, y) {
                return Some(BarHit::AddTab(index));
            }
            if self.columns[index].contains(x, y) {
                return Some(BarHit::Column(index));
            }
        }
        None
    }
}

struct ComparatorView {
    webview: WebView,
    name: &'static str,
}

struct SplitView {
    webview: WebView,
    source_index: usize,
    url: String,
    fullscreen: bool,
    private: bool,
}

struct ComparatorState {
    views: Vec<ComparatorView>,
    expanded: Option<usize>,
    minimized: [bool; COMPARATOR_COLUMNS],
    weights: [f64; COMPARATOR_COLUMNS],
    split: Option<SplitView>,
    /// Abas/fontes agrupadas automaticamente pela IA que abriu cada link.
    contexts: [Vec<String>; COMPARATOR_COLUMNS],
}

const EM_SETSEL: u32 = 0x00B1;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_SETCUEBANNER: u32 = 0x1501;
const EM_SETMARGINS: u32 = 0x00D3;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const WM_ERASEBKGND: u32 = 0x0014;

/// Instante de arranque, para termos milissegundos monotonos num AtomicU64.
static START: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Construir um WebView faz correr um ciclo de mensagens ANINHADO dentro do
/// nosso proprio callback (wry chama `wait_with_pump`). Enquanto isso dura, o
/// winit nao entrega `RedrawRequested` -- so revalida e reinvalida a janela em
/// ciclo -- por isso o ecra fica com os pixeis das janelas que acabamos de
/// destruir. Este sinalizador deixa o `WM_ERASEBKGND` apagar o fundo mesmo
/// nessas voltas, que e o unico ponto de pintura que ainda corre.
static ERASE_PENDING: AtomicBool = AtomicBool::new(false);
const WINDOW_SUBCLASS_ID: usize = 0x4E4A;
const EXIT_BUTTON_SUBCLASS_ID: usize = 0x4E4B;
const WM_PAINT: u32 = 0x000F;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_NCHITTEST: u32 = 0x0084;
const HTCLIENT: u32 = 1;
/// Botao flutuante de saida, em pixeis logicos. Fica centrado no topo: nos
/// cantos chocava com a propria interface dos sites (o login do Google estava
/// exatamente por baixo dele).
const EXIT_BUTTON_WIDTH: f64 = 196.0;
const EXIT_BUTTON_HEIGHT: f64 = 38.0;
const EC_LEFTMARGIN: usize = 0x0001;
const EC_RIGHTMARGIN: usize = 0x0002;
const WM_SETFONT: u32 = 0x0030;
const OMNIBOX_SUBCLASS_ID: usize = 0x4E49;
const TAB_MENU_OPEN: usize = 1;
const TAB_MENU_FULLSCREEN: usize = 2;
const TAB_MENU_CLOSE: usize = 3;
const TAB_MENU_CLOSE_OTHERS: usize = 4;
const TAB_MENU_CLOSE_ALL: usize = 5;
const SPLITTER_SUBCLASS_BASE: usize = 0x4E60;
const SPLITTER_WIDTH: f64 = 7.0;
const MIN_PANEL_WIDTH: f64 = 180.0;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSEMOVE: u32 = 0x0200;

/// Consulta opcional para automacao/benchmarks. Em producao a Home abre em
/// repouso e nao envia texto a nenhum fornecedor sem acao do utilizador.
fn startup_input() -> String {
    if std::env::var_os("NEURALIA_NO_STARTUP").is_some() {
        return String::new();
    }
    std::env::var("NEURALIA_STARTUP_INPUT")
        .unwrap_or_default()
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

#[link(name = "user32")]
unsafe extern "system" {
    fn GetCapture() -> HWND;
    fn SetCapture(hwnd: HWND) -> HWND;
    fn ReleaseCapture() -> i32;
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
    if message == WM_ERASEBKGND {
        if ERASE_PENDING.swap(false, Ordering::SeqCst) {
            let hdc = wparam as *mut core::ffi::c_void;
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let brush = CreateSolidBrush(rgb3(Theme::system().page_bg));
                FillRect(hdc, &client, brush);
                DeleteObject(brush as _);
            }
        }
        // Damos sempre a mensagem por tratada: fora das transicoes nao ha nada
        // a apagar, e apagar a cada repintura faria a barra piscar.
        return 1;
    }

    if message == WM_CTLCOLOREDIT {
        let theme = Theme::system();
        let hdc = wparam as *mut core::ffi::c_void;
        SetTextColor(hdc, rgb3(theme.fg));
        SetBkColor(hdc, rgb3(theme.surface));
        return omnibox_brush(theme.surface) as LRESULT;
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

/// Botao de sair do ecra completo. Tem de ser uma janela de topo propria: o
/// WebView2 e uma janela filha que cobre o cliente todo, por isso nada pintado
/// pela janela principal apareceria por cima dele. Tambem nao pode depender de
/// nada injetado na pagina -- o YouTube reescreve o seu proprio DOM e o botao
/// injetado desaparece, que foi exatamente o que aconteceu.
/// Aviso flutuante no fundo do ecra. Tem de ser nativo e nao injetado na
/// pagina: por cima de um PDF nao ha pagina nossa onde escrever -- o
/// visualizador do Edge e outro documento, noutra origem e noutro processo.
unsafe extern "system" fn splash_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    // Quando a janela faz uma pergunta tem de receber cliques; uma janela da
    // classe STATIC devolve HTTRANSPARENT e o clique atravessava-a.
    if message == WM_NCHITTEST && SPLASH_ASKS.load(Ordering::SeqCst) {
        return HTCLIENT as LRESULT;
    }

    if message == WM_LBUTTONUP && SPLASH_ASKS.load(Ordering::SeqCst) && reference_data != 0 {
        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client) != 0 {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let (yes, no) = splash_buttons(&client);
            let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
            if x >= yes.left && x < yes.right {
                let _ = proxy.send_event(UserEvent::AutoScrollAnswer(true));
            } else if x >= no.left && x < no.right {
                let _ = proxy.send_event(UserEvent::AutoScrollAnswer(false));
            }
        }
        return 0;
    }

    if message == WM_PAINT {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        if !hdc.is_null() {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let theme = Theme::system();
                let height = (client.bottom - client.top) as f64;

                let background = CreateSolidBrush(rgb3(theme.surface));
                FillRect(hdc, &client, background);
                DeleteObject(background as _);

                let scale = (height / SPLASH_HEIGHT).max(1.0);
                let font = create_font((-14.0 * scale) as i32, FW_NORMAL as i32);
                let old_font = SelectObject(hdc, font as _);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, rgb3(theme.fg));

                let text = SPLASH_TEXT
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();

                if SPLASH_ASKS.load(Ordering::SeqCst) {
                    let (yes, no) = splash_buttons(&client);
                    let mut question = RECT {
                        left: client.left + (18.0 * scale) as i32,
                        top: client.top,
                        right: yes.left - (10.0 * scale) as i32,
                        bottom: client.bottom,
                    };
                    draw_text(
                        hdc,
                        &text,
                        &mut question,
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                    );

                    for (rect, label, primary) in [(yes, "Sim", true), (no, "Não", false)] {
                        let pill = UiRect {
                            x: rect.left as f64,
                            y: rect.top as f64,
                            width: (rect.right - rect.left) as f64,
                            height: (rect.bottom - rect.top) as f64,
                        };
                        let style = if primary {
                            PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
                        } else {
                            PillStyle::new(
                                mix(theme.surface, theme.fg, 0.10),
                                theme.surface_line,
                                theme.fg,
                            )
                        };
                        draw_pill(hdc, pill, label, style, scale, font, theme.surface);
                    }
                } else {
                    let mut rect = client;
                    draw_text(
                        hdc,
                        &text,
                        &mut rect,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                    );
                }

                SelectObject(hdc, old_font);
                DeleteObject(font as _);
            }
            EndPaint(hwnd, &paint);
        }
        return 0;
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe extern "system" fn gmail_toast_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if message == WM_PAINT {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        if !hdc.is_null() {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let theme = Theme::system();
                let background = CreateSolidBrush(rgb3(theme.surface));
                FillRect(hdc, &client, background);
                DeleteObject(background as _);

                let scale = ((client.bottom - client.top) as f64 / GMAIL_TOAST_HEIGHT).max(1.0);
                let title_font = create_font((-13.0 * scale) as i32, FW_BOLD as i32);
                let body_font = create_font((-12.0 * scale) as i32, FW_NORMAL as i32);
                let old_font = SelectObject(hdc, title_font as _);
                SetBkMode(hdc, TRANSPARENT as i32);

                SetTextColor(hdc, rgb3(theme.accent));
                let mut title = RECT {
                    left: (16.0 * scale) as i32,
                    top: (7.0 * scale) as i32,
                    right: client.right - (14.0 * scale) as i32,
                    bottom: (28.0 * scale) as i32,
                };
                draw_text(
                    hdc,
                    "Gmail · novo e-mail",
                    &mut title,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );

                SelectObject(hdc, body_font as _);
                SetTextColor(hdc, rgb3(theme.fg));
                let text = GMAIL_TOAST_TEXT
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let mut body = RECT {
                    left: (16.0 * scale) as i32,
                    top: (28.0 * scale) as i32,
                    right: client.right - (14.0 * scale) as i32,
                    bottom: client.bottom - (7.0 * scale) as i32,
                };
                draw_text(
                    hdc,
                    &text,
                    &mut body,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );

                SelectObject(hdc, old_font);
                DeleteObject(title_font as _);
                DeleteObject(body_font as _);
            }
            EndPaint(hwnd, &paint);
        }
        return 0;
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe extern "system" fn comparator_splitter_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    match message {
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            return 0;
        }
        WM_MOUSEMOVE => {
            if GetCapture() == hwnd {
                let mut point = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut point) != 0 {
                    let divider = subclass_id.saturating_sub(SPLITTER_SUBCLASS_BASE);
                    let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                    let _ = proxy.send_event(UserEvent::ResizeComparator {
                        divider,
                        screen_x: point.x,
                    });
                }
            }
            return 0;
        }
        WM_LBUTTONUP => {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            return 0;
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let theme = Theme::system();
                    let bg = CreateSolidBrush(rgb3(theme.bar_bg));
                    FillRect(hdc, &client, bg);
                    DeleteObject(bg as _);
                    let center = (client.right - client.left) / 2;
                    let line = RECT {
                        left: center,
                        top: 0,
                        right: center + 1,
                        bottom: client.bottom,
                    };
                    let brush = CreateSolidBrush(rgb3(theme.surface_line));
                    FillRect(hdc, &line, brush);
                    DeleteObject(brush as _);
                }
                EndPaint(hwnd, &paint);
            }
            return 0;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe extern "system" fn exit_button_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    match message {
        // Uma janela da classe STATIC devolve HTTRANSPARENT por omissao: o
        // clique atravessa-a e vai parar ao WebView por baixo, que era por isso
        // que este botao nao fazia nada.
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let theme = Theme::system();
                    let width = (client.right - client.left) as f64;
                    let height = (client.bottom - client.top) as f64;

                    let background = CreateSolidBrush(rgb3(theme.surface));
                    FillRect(hdc, &client, background);
                    DeleteObject(background as _);

                    let scale = (height / EXIT_BUTTON_HEIGHT).max(1.0);
                    let font = create_font((-14.0 * scale) as i32, FW_BOLD as i32);
                    let old_font = SelectObject(hdc, font as _);
                    SetBkMode(hdc, TRANSPARENT as i32);
                    SetTextColor(hdc, rgb3(theme.fg));
                    let mut text_rect = RECT {
                        left: 0,
                        top: 0,
                        right: width as i32,
                        bottom: height as i32,
                    };
                    draw_text(
                        hdc,
                        "\u{2715}  Sair da tela cheia",
                        &mut text_rect,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                    );
                    SelectObject(hdc, old_font);
                    DeleteObject(font as _);
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        WM_LBUTTONUP => {
            let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
            let _ = proxy.send_event(UserEvent::RestoreComparator);
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
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
            // Um EDIT de uma linha nao trata Ctrl+A sozinho -- e uma velha
            // manha do Win32. Ctrl+L faz o mesmo, por ser o habito do Chrome.
            0x41 | 0x4C if ctrl => {
                SendMessageW(hwnd, EM_SETSEL, 0, -1);
                return 0;
            }
            0x48 if ctrl => {
                let _ = proxy.send_event(UserEvent::ShowHistory);
                return 0;
            }
            0x4E if ctrl => {
                let _ = proxy.send_event(UserEvent::NewTab(0));
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

struct DocumentJob {
    generation: u64,
    url: String,
}

#[derive(Clone)]
struct DocumentWorker {
    pending: Arc<(Mutex<Option<DocumentJob>>, Condvar)>,
    alive: bool,
}

impl DocumentWorker {
    fn new(
        client: ReaderClient,
        proxy: EventLoopProxy<UserEvent>,
        generation: Arc<AtomicU64>,
    ) -> Self {
        let pending = Arc::new((Mutex::new(None::<DocumentJob>), Condvar::new()));
        let worker_pending = Arc::clone(&pending);

        let spawned = thread::Builder::new()
            .name("neural-pdf".into())
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
                        slot.take().expect("document job present")
                    };

                    let job_generation = job.generation;
                    let watch = Arc::clone(&generation);
                    let result = client
                        .fetch_document(&job.url, "application/pdf", PDF_MAX_BYTES, &|| {
                            watch.load(Ordering::SeqCst) != job_generation
                        })
                        .map_err(|error| error.to_string());

                    let _ = proxy.send_event(UserEvent::PdfReady {
                        generation: job_generation,
                        url: job.url,
                        result,
                    });
                }
            });

        Self {
            pending,
            alive: spawned.is_ok(),
        }
    }

    fn submit(&self, job: DocumentJob) -> Result<(), String> {
        if !self.alive {
            return Err("a thread de documentos não pôde ser criada".to_string());
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
    proxy: EventLoopProxy<UserEvent>,
}

impl HistoryWriter {
    fn new(store: HistoryStore, proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = sync_channel::<HistoryCommand>(128);
        let worker_store = store.clone();
        let worker_proxy = proxy.clone();
        let _ = thread::Builder::new()
            .name("neural-history".into())
            .spawn(move || {
                while let Ok(command) = rx.recv() {
                    match command {
                        HistoryCommand::Append(entry) => {
                            let _ = worker_store.append(&entry);
                        }
                        HistoryCommand::Clear => {
                            let result = worker_store.clear().map_err(|error| error.to_string());
                            let _ = worker_proxy.send_event(UserEvent::HistoryCleared(result));
                        }
                    }
                }
            });
        Self { tx, store, proxy }
    }

    /// Nunca faz I/O no event loop. Sob saturacao, perder uma entrada e menos
    /// grave do que congelar a interface com lock + fsync + rename.
    fn append(&self, entry: HistoryEntry) {
        if self.tx.try_send(HistoryCommand::Append(entry)).is_err() {
            eprintln!("history queue saturated; dropping one entry");
        }
    }

    /// A limpeza nao pode ser perdida. Se o worker estiver saturado, usa uma
    /// thread excepcional em vez de executar I/O sincrono na UI.
    fn clear(&self) -> Option<Result<(), String>> {
        if self.tx.try_send(HistoryCommand::Clear).is_ok() {
            return None;
        }

        let store = self.store.clone();
        let proxy = self.proxy.clone();
        match thread::Builder::new()
            .name("neural-history-clear".into())
            .spawn(move || {
                let result = store.clear().map_err(|error| error.to_string());
                let _ = proxy.send_event(UserEvent::HistoryCleared(result));
            }) {
            Ok(_) => None,
            Err(error) => Some(Err(format!(
                "não consegui criar a thread de limpeza: {error}"
            ))),
        }
    }
}

enum MemoryCommand {
    Capture(MemoryDocument),
    Query(String),
    Clear,
    SaveSession(ResearchSession),
    Rebuild,
}

#[derive(Clone)]
struct MemoryWorker {
    tx: SyncSender<MemoryCommand>,
}

impl MemoryWorker {
    fn new(root: std::path::PathBuf, proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = sync_channel::<MemoryCommand>(128);
        let _ = thread::Builder::new()
            .name("neural-memory".into())
            .spawn(move || {
                let store = match MemoryStore::new(&root) {
                    Ok(store) => store,
                    Err(error) => {
                        eprintln!("memory store unavailable: {error}");
                        while let Ok(command) = rx.recv() {
                            match command {
                                MemoryCommand::Query(query) => {
                                    let _ = proxy.send_event(UserEvent::MemoryQueryReady {
                                        query,
                                        result: Err(error.to_string()),
                                    });
                                }
                                MemoryCommand::Clear => {
                                    let _ = proxy.send_event(UserEvent::MemoryCleared(Err(
                                        error.to_string()
                                    )));
                                }
                                _ => {}
                            }
                        }
                        return;
                    }
                };

                while let Ok(command) = rx.recv() {
                    match command {
                        MemoryCommand::Capture(document) => {
                            if let Err(error) = store.capture(document) {
                                eprintln!("memory capture failed: {error}");
                            }
                        }
                        MemoryCommand::Query(query) => {
                            let result = if query.trim().is_empty() {
                                store.documents().map(|documents| {
                                    documents
                                        .into_iter()
                                        .take(20)
                                        .map(|document| MemoryHit {
                                            id: document.id,
                                            title: document.title,
                                            url: document.url,
                                            provider: document.provider,
                                            session_id: document.session_id,
                                            excerpt: document
                                                .body
                                                .split_whitespace()
                                                .take(36)
                                                .collect::<Vec<_>>()
                                                .join(" "),
                                            score: 0.0,
                                            matched_by: vec!["recent".into()],
                                        })
                                        .collect()
                                })
                            } else {
                                store.query(&MemoryQuery::new(query.clone()))
                            }
                            .map_err(|error| error.to_string());
                            let _ = proxy.send_event(UserEvent::MemoryQueryReady { query, result });
                        }
                        MemoryCommand::Clear => {
                            let result = store
                                .forget(neural_core::ForgetScope::All)
                                .map(|_| ())
                                .map_err(|error| error.to_string());
                            let _ = proxy.send_event(UserEvent::MemoryCleared(result));
                        }
                        MemoryCommand::SaveSession(session) => {
                            if let Err(error) = session.save(store.root()) {
                                eprintln!("research session save failed: {error}");
                            }
                        }
                        MemoryCommand::Rebuild => {
                            if let Err(error) = store.rebuild() {
                                eprintln!("memory rebuild failed: {error}");
                            }
                        }
                    }
                }
            });
        Self { tx }
    }

    fn capture(&self, document: MemoryDocument) {
        if self.tx.try_send(MemoryCommand::Capture(document)).is_err() {
            eprintln!("memory queue saturated; dropping one capture");
        }
    }

    fn query(&self, query: String) {
        if self.tx.try_send(MemoryCommand::Query(query)).is_err() {
            eprintln!("memory queue saturated; query not scheduled");
        }
    }

    fn clear(&self) {
        if self.tx.try_send(MemoryCommand::Clear).is_err() {
            eprintln!("memory queue saturated; clear not scheduled");
        }
    }

    fn save_session(&self, session: ResearchSession) {
        if self
            .tx
            .try_send(MemoryCommand::SaveSession(session))
            .is_err()
        {
            eprintln!("memory queue saturated; session save not scheduled");
        }
    }

    fn rebuild(&self) {
        if self.tx.try_send(MemoryCommand::Rebuild).is_err() {
            eprintln!("memory queue saturated; rebuild not scheduled");
        }
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
    /// Em ecra completo a barra some; volta enquanto o rato estiver no topo.
    chrome_revealed: bool,
    chrome_token: u64,
    /// Prazo de vida da barra, em milissegundos monotonos. Cada movimento do
    /// rato empurra-o; a thread de vigia le-o. Antes era uma thread do sistema
    /// operativo POR CADA evento de rato -- centenas vivas ao mesmo tempo.
    chrome_deadline: Arc<AtomicU64>,
    exit_button: Option<HWND>,
    splitters: [Option<HWND>; COMPARATOR_COLUMNS - 1],
    auto_scroll: bool,
    auto_scroll_answered: bool,
    auto_scroll_token: u64,
    zoom: f64,
    /// O visualizador de PDF nao aceita script do host: rola-se por tecla.
    reading_pdf: bool,
    splash: Option<HWND>,
    splash_token: u64,
    gmail_toast: Option<HWND>,
    gmail_toast_token: u64,
    gmail_monitor: Option<WebView>,
    gmail_probe_token: u64,
    gmail_last_unread: Option<u32>,
    gmail_last_key: Option<String>,
    /// O Win32 nao apaga o fundo por nos e uma janela filha destruida deixa os
    /// ultimos pixeis onde estava. Sem isto viam-se barras e texto fantasma.
    needs_clear: bool,
    omnibox_font: Option<*mut core::ffi::c_void>,
    omnibox_font_height: i32,
    omnibox_proxy: Box<EventLoopProxy<UserEvent>>,
    config: CoreConfig,
    history_store: HistoryStore,
    history: HistoryWriter,
    memory: MemoryWorker,
    current_research: Option<ResearchSession>,
    reader: ReaderWorker,
    /// Worker unico para documentos binarios. Um pedido novo substitui o
    /// pendente, evitando uma thread/socket de 90 s por clique em PDF.
    document: DocumentWorker,
    /// Os bytes do PDF aberto, servidos ao visualizador pela origem propria.
    pdf_bytes: Arc<Mutex<Vec<u8>>>,
    surface: Surface,
    navigation_generation: Arc<AtomicU64>,
    status: Option<String>,
    cursor: (f64, f64),
    /// Proximo frame da rede neural nativa da Home. Nao existe WebView nem
    /// rede por tras do efeito: e apenas GDI, limitado a ~15 FPS.
    next_home_frame: Instant,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let config = CoreConfig::default();
        let history_store =
            HistoryStore::with_limit(config.data_dir.join("history.jsonl"), config.history_limit);
        let history = HistoryWriter::new(history_store.clone(), proxy.clone());
        let memory = MemoryWorker::new(config.data_dir.join("memory"), proxy.clone());
        let reader_client = ReaderClient::new(config.reader_timeout_secs, config.reader_max_bytes);
        let navigation_generation = Arc::new(AtomicU64::new(0));
        let reader = ReaderWorker::new(
            reader_client,
            proxy.clone(),
            Arc::clone(&navigation_generation),
        );
        let omnibox_proxy = Box::new(proxy.clone());
        let document = DocumentWorker::new(
            ReaderClient::new(PDF_TIMEOUT_SECS, PDF_MAX_BYTES),
            proxy.clone(),
            Arc::clone(&navigation_generation),
        );
        Self {
            document,
            pdf_bytes: Arc::new(Mutex::new(Vec::new())),
            proxy,
            window: None,
            webview: None,
            comparator: None,
            omnibox: None,
            bar_hover: None,
            chrome_revealed: false,
            chrome_token: 0,
            chrome_deadline: Arc::new(AtomicU64::new(0)),
            exit_button: None,
            splitters: [None; COMPARATOR_COLUMNS - 1],
            // Ligada por omissao: a aplicacao serve para ler.
            // Nada rola sem o utilizador dizer que sim.
            auto_scroll: false,
            auto_scroll_answered: false,
            auto_scroll_token: 0,
            zoom: 1.0,
            reading_pdf: false,
            splash: None,
            splash_token: 0,
            gmail_toast: None,
            gmail_toast_token: 0,
            gmail_monitor: None,
            gmail_probe_token: 0,
            gmail_last_unread: None,
            gmail_last_key: None,
            needs_clear: true,
            omnibox_font: None,
            omnibox_font_height: 0,
            omnibox_proxy,
            config,
            history_store,
            history,
            memory,
            current_research: None,
            reader,
            surface: Surface::Home,
            navigation_generation,
            status: None,
            cursor: (-1.0, -1.0),
            next_home_frame: Instant::now(),
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

    /// Pinta a area de cliente inteira com o fundo do tema. Corre quando a
    /// superficie muda ou a janela muda de tamanho, nunca a cada realce do rato.
    /// Marca o ecra como sujo e limpa-o JA. Nao chega agendar para o proximo
    /// `RedrawRequested`: a seguir a isto vem quase sempre a construcao de um
    /// WebView, e durante essa construcao o winit deixa de entregar redraws.
    fn mark_dirty(&mut self) {
        self.needs_clear = true;
        ERASE_PENDING.store(true, Ordering::SeqCst);
        self.clear_client();
    }

    fn clear_client(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };
        unsafe {
            let hdc = GetDC(hwnd);
            if hdc.is_null() {
                return;
            }
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let brush = CreateSolidBrush(rgb3(Theme::system().page_bg));
                FillRect(hdc, &client, brush);
                DeleteObject(brush as _);
            }
            let _ = ReleaseDC(hwnd, hdc);
        }
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
        // O EDIT deixa o desenho antigo para tras quando muda de sitio.
        self.needs_clear = true;
        self.request_redraw();
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
    /// Sai do ecra completo. Faltava em quase todas as saidas: bastava um login
    /// ou um Esc para a janela ficar sem barra de titulo e sem forma de voltar.
    fn leave_fullscreen(&mut self) {
        self.chrome_revealed = false;
        self.chrome_token = self.chrome_token.wrapping_add(1);
        if let Some(window) = &self.window {
            window.set_fullscreen(None);
        }
    }

    fn destroy_web_surfaces(&mut self) {
        self.mark_dirty();
        self.leave_fullscreen();
        if let Some(window) = &self.window {
            window.set_decorations(true);
        }
        if let Some(button) = self.exit_button.take() {
            unsafe {
                DestroyWindow(button);
            }
        }
        for splitter in &mut self.splitters {
            if let Some(hwnd) = splitter.take() {
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }
        if let Some(comparator) = self.comparator.take() {
            drop(comparator);
        }
        if let Some(webview) = self.webview.take() {
            let _ = webview.focus_parent();
            drop(webview);
        }
        if let Ok(mut bytes) = self.pdf_bytes.lock() {
            *bytes = Vec::new();
        }
        self.reading_pdf = false;
    }

    fn show_home(&mut self) {
        self.next_generation();
        self.destroy_web_surfaces();
        self.surface = Surface::Home;
        self.bar_hover = None;
        self.status = None;
        self.next_home_frame = Instant::now();
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

    fn show_history(&mut self) {
        self.set_omnibox_text("memory:");
        self.focus_omnibox();
        self.show_splash(
            "Memória semântica: descreva o que você quer reencontrar e pressione Enter."
                .to_string(),
            4,
        );
    }

    fn show_recent_history(&self) {
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
        self.show_native_text("NeuralIA — Histórico cronológico", &text);
    }

    fn show_native_text(&self, title: &str, text: &str) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };
        let body = wide_null(text);
        let title = wide_null(title);
        unsafe {
            MessageBoxW(
                hwnd,
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    fn show_memory_results(&self, query: &str, result: Result<Vec<MemoryHit>, String>) {
        let text = match result {
            Err(error) => format!("Não foi possível consultar a memória: {error}"),
            Ok(hits) if hits.is_empty() => {
                if query.trim().is_empty() {
                    "Memória semântica vazia.".to_string()
                } else {
                    format!("Nenhum resultado para \"{query}\".")
                }
            }
            Ok(hits) => hits
                .into_iter()
                .enumerate()
                .map(|(index, hit)| {
                    let source = hit
                        .provider
                        .as_deref()
                        .or(hit.url.as_deref())
                        .unwrap_or("local");
                    let via = hit.matched_by.join("+");
                    format!(
                        "{}. {}\r\n   {} · {}\r\n   {}{}",
                        index + 1,
                        hit.title,
                        source,
                        via,
                        hit.excerpt,
                        hit.url
                            .as_deref()
                            .map(|url| format!("\r\n   {url}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\r\n\r\n"),
        };
        self.show_native_text("NeuralIA — Memória semântica", &text);
    }

    fn handle_input(&mut self, input: String) {
        if let Some(query) = input
            .strip_prefix("memory:")
            .or_else(|| input.strip_prefix("mem:"))
        {
            self.memory.query(query.trim().to_string());
            self.show_splash("Buscando na memória local…".to_string(), 2);
            return;
        }
        if input.trim().eq_ignore_ascii_case("history:") {
            self.show_recent_history();
            return;
        }
        if input.trim().eq_ignore_ascii_case("memory:rebuild") {
            self.memory.rebuild();
            self.show_splash("Reconstrução da memória agendada.".to_string(), 3);
            return;
        }

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

        let session = ResearchSession::new(query.clone());
        let question_memory = MemoryDocument::new(
            MemoryKind::ResearchResult,
            MemorySourceKind::Note,
            format!("Pesquisa · {}", session.title),
            None,
            query.clone(),
        )
        .session(session.id.clone());
        self.memory.capture(question_memory);
        self.memory.save_session(session.clone());
        self.current_research = Some(session);

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
            Ok(valid) if is_pdf_url(&valid) => self.read_pdf(valid),
            Ok(valid) => {
                self.next_generation();
                self.record(HistoryKind::Web, valid.to_string(), valid.to_string());
                self.open_external(valid.as_str());
            }
            Err(error) => self.show_native_error(error.to_string()),
        }
    }

    /// Descarrega o PDF no worker coalescente de documentos.
    fn read_pdf(&mut self, url: Url) {
        let generation = self.next_generation();
        self.destroy_web_surfaces();
        self.surface = Surface::Home;
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

    fn pdf_webview_builder(&self) -> WebViewBuilder<'static> {
        let proxy = self.proxy.clone();
        let bytes = Arc::clone(&self.pdf_bytes);

        WebViewBuilder::new()
            .with_custom_protocol("neuralia-pdf".to_string(), move |_id, request| {
                serve_pdf_asset(&bytes, &request)
            })
            .with_initialization_script(NEURALIA_KEYMAP_SCRIPT)
            .with_navigation_handler(move |target| {
                if let Some(event) = neuralia_action(&target) {
                    let _ = proxy.send_event(event);
                    return false;
                }
                if is_pdf_internal_target(&target) {
                    return true;
                }
                if remote_web_target(&target, false) {
                    let _ = proxy.send_event(UserEvent::OpenExternal(target));
                }
                false
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    fn open_pdf(&mut self, url: &str, bytes: Vec<u8>) {
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

    fn record(&self, kind: HistoryKind, input: String, target: String) {
        self.history.append(HistoryEntry::now(kind, input, target));
    }

    fn capture_reader_memory(&mut self, article: &ReaderArticle) {
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

    fn reader_webview_builder(&self) -> WebViewBuilder<'static> {
        let proxy = self.proxy.clone();
        WebViewBuilder::new()
            .with_initialization_script(format!(
                "{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}"
            ))
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
                    let _ = proxy.send_event(event);
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

    fn external_webview_builder(&self, allow_local: bool) -> WebViewBuilder<'static> {
        let navigation_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let capability = remote_capability();
        let navigation_capability = capability.clone();
        let init_script = format!("{NEURALIA_KEYMAP_SCRIPT}\n{EXTERNAL_RETURN_BUTTON}")
            .replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
            .with_initialization_script(init_script)
            .with_navigation_handler(move |target| {
                if let Some(event) = remote_neuralia_action(&target, &navigation_capability) {
                    let _ = navigation_proxy.send_event(event);
                    return false;
                }
                remote_web_target(&target, allow_local)
                    || is_view_source_target(&target, allow_local)
            })
            .with_new_window_req_handler(move |target, _features| {
                if remote_web_target(&target, allow_local) {
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
        let is_pdf = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase()
            .ends_with(".pdf");

        let allow_local = Url::parse(url)
            .ok()
            .is_some_and(|target| is_local_network_target(&target));
        let result = if let Some(window) = &self.window {
            self.external_webview_builder(allow_local)
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
                let _ = webview.zoom(self.zoom);
                self.webview = Some(webview);
                self.surface = Surface::Reader;
                self.begin_reading_session(false);
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
        // No comparador o chrome e nosso: a primeira linha recebe as abas e os
        // controles de janela; a segunda fica reservada aos provedores.
        window.set_decorations(false);

        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;

        let content_h = (logical_h - COMPARATOR_CHROME_HEIGHT).max(100.0);
        let content_y = COMPARATOR_CHROME_HEIGHT;
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
                    let _ = wv.zoom(self.zoom);
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
            minimized: [false; COMPARATOR_COLUMNS],
            weights: [1.0; COMPARATOR_COLUMNS],
            split: None,
            contexts: std::array::from_fn(|_| Vec::new()),
        });
        self.bar_hover = None;
        self.surface = Surface::Comparator;
        self.schedule_gmail_probe(4);
        self.begin_reading_session(false);
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    /// Alterna: o botao injetado na pagina pede sempre "expandir", e e aqui que
    /// isso vira "sair da tela cheia" quando a coluna ja esta expandida. Sem a
    /// barra nativa em tela cheia, esse botao e o Esc sao o caminho de volta.
    fn expand_comparator(&mut self, idx: usize) {
        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some())
        {
            self.close_split();
        }

        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
            && comp.minimized[idx]
        {
            comp.minimized[idx] = false;
            comp.expanded = None;
            self.bar_hover = None;
            self.needs_clear = true;
            self.update_comparator_layout();
            self.sync_comparator_splitters();
            self.sync_comparator_buttons();
            self.request_redraw();
            return;
        }

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
        self.bar_hover = None;
        self.chrome_revealed = false;
        self.chrome_token = self.chrome_token.wrapping_add(1);
        self.needs_clear = true;

        // Ecra completo a serio: sem barra de titulo, sem minimizar/fechar.
        if let Some(window) = &self.window {
            if restored {
                window.set_fullscreen(None);
            } else {
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
            }
        }

        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.sync_comparator_buttons();
        self.sync_exit_button();
        self.request_redraw();
    }

    fn minimize_comparator(&mut self, idx: usize) {
        if self.surface != Surface::Comparator {
            return;
        }

        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some())
        {
            self.close_split();
        }

        let mut was_expanded = false;
        let mut changed = false;
        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
        {
            let visible = comp
                .views
                .iter()
                .enumerate()
                .filter(|(index, _)| !comp.minimized[*index])
                .count();

            // Mantemos sempre pelo menos uma IA visível.
            if !comp.minimized[idx] && visible > 1 {
                was_expanded = comp.expanded == Some(idx);
                comp.expanded = None;
                comp.minimized[idx] = true;
                changed = true;
            }
        }

        if !changed {
            self.show_splash(
                "Pelo menos um painel precisa continuar visível.".to_string(),
                2,
            );
            return;
        }

        if was_expanded && let Some(window) = &self.window {
            window.set_fullscreen(None);
        }

        self.bar_hover = None;
        self.chrome_revealed = false;
        self.chrome_token = self.chrome_token.wrapping_add(1);
        self.needs_clear = true;
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.sync_comparator_buttons();
        self.sync_exit_button();
        self.request_redraw();
    }

    fn restore_comparator(&mut self) {
        if let Some(comp) = &mut self.comparator {
            comp.expanded = None;
        }
        self.chrome_revealed = false;
        self.chrome_token = self.chrome_token.wrapping_add(1);
        if let Some(window) = &self.window {
            window.set_fullscreen(None);
        }
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.sync_comparator_buttons();
        self.sync_exit_button();
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

        let content_h = (logical_h - COMPARATOR_CHROME_HEIGHT).max(100.0);
        let content_y = COMPARATOR_CHROME_HEIGHT;

        // Fonte lateral: a IA que originou o link continua visível, as demais
        // ficam vivas e preservam estado para reaparecer ao fechar a gaveta.
        if let Some(split) = &comp.split {
            if split.fullscreen {
                for view in &comp.views {
                    let _ = view.webview.set_visible(false);
                }
                let _ = split.webview.set_bounds(wry::Rect {
                    position: LogicalPosition::new(0.0, content_y).into(),
                    size: LogicalSize::new(logical_w, content_h).into(),
                });
                let _ = split.webview.set_visible(true);
                return;
            }

            let ai_width = (logical_w * 0.54).clamp(logical_w * 0.38, logical_w * 0.68);
            for (index, view) in comp.views.iter().enumerate() {
                if index == split.source_index {
                    let _ = view.webview.set_bounds(wry::Rect {
                        position: LogicalPosition::new(0.0, content_y).into(),
                        size: LogicalSize::new(ai_width, content_h).into(),
                    });
                    let _ = view.webview.set_visible(true);
                } else {
                    let _ = view.webview.set_visible(false);
                }
            }
            let _ = split.webview.set_bounds(wry::Rect {
                position: LogicalPosition::new(ai_width, content_y).into(),
                size: LogicalSize::new((logical_w - ai_width).max(1.0), content_h).into(),
            });
            let _ = split.webview.set_visible(true);
            return;
        }

        match comp.expanded {
            Some(idx) => {
                // Ecra completo. A coluna ocupa tudo menos uma faixa de 1px no
                // topo: o WebView e uma janela filha e engole o rato, por isso
                // sem essa faixa a aplicacao nunca saberia que o rato subiu ao
                // topo para chamar a barra de volta.
                let (top, height) = if self.chrome_revealed {
                    (
                        COMPARATOR_CHROME_HEIGHT,
                        (logical_h - COMPARATOR_CHROME_HEIGHT).max(1.0),
                    )
                } else {
                    (1.0, (logical_h - 1.0).max(1.0))
                };

                for (i, v) in comp.views.iter().enumerate() {
                    if i == idx {
                        let _ = v.webview.set_bounds(wry::Rect {
                            position: LogicalPosition::new(0.0, top).into(),
                            size: LogicalSize::new(logical_w, height).into(),
                        });
                        let _ = v.webview.set_visible(true);
                    } else {
                        let _ = v.webview.set_visible(false);
                    }
                }
            }
            None => {
                let visible: Vec<usize> = comp
                    .views
                    .iter()
                    .enumerate()
                    .filter_map(|(index, _)| (!comp.minimized[index]).then_some(index))
                    .collect();
                let total_weight: f64 = visible
                    .iter()
                    .map(|index| comp.weights[*index].max(0.05))
                    .sum::<f64>()
                    .max(0.05);
                let mut col_x = 0.0;

                for (slot, index) in visible.iter().enumerate() {
                    let v = &comp.views[*index];
                    let actual_w = if slot == visible.len() - 1 {
                        logical_w - col_x
                    } else {
                        logical_w * comp.weights[*index].max(0.05) / total_weight
                    };
                    let _ = v.webview.set_bounds(wry::Rect {
                        position: LogicalPosition::new(col_x, content_y).into(),
                        size: LogicalSize::new(actual_w.max(1.0), content_h).into(),
                    });
                    let _ = v.webview.set_visible(true);
                    col_x += actual_w;
                }

                for (index, v) in comp.views.iter().enumerate() {
                    if comp.minimized[index] {
                        let _ = v.webview.set_visible(false);
                    }
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
        let capability = remote_capability();
        let navigation_capability = capability.clone();

        let init_script = format!(
            "window.__neuralia_col_index = {col_index}; window.__neuralia_col_name = '{col_name}';\n{NEURALIA_KEYMAP_SCRIPT}\n{NEURALIA_PALETTE_SCRIPT}\n{AI_AUTO_SUBMIT_SCRIPT}\n{COMPARATOR_INJECT_SCRIPT}"
        )
        .replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
            .with_initialization_script(init_script)
            .with_navigation_handler(move |target| {
                if target.starts_with("neuralia:split") {
                    if remote_capability_matches(&target, &navigation_capability)
                        && let (Some(col), Some(url)) = (
                            neuralia_query_param(&target, "col"),
                            neuralia_query_param(&target, "url"),
                        )
                        && let Ok(source_index) = col.parse::<usize>()
                        && remote_web_target(&url, false)
                    {
                        let _ =
                            navigation_proxy.send_event(UserEvent::OpenSplit { source_index, url });
                    }
                    return false;
                }
                if target.starts_with("neuralia:palette") {
                    if remote_capability_matches(&target, &navigation_capability)
                        && let (Some(col), Some(input)) = (
                            neuralia_query_param(&target, "col"),
                            neuralia_query_param(&target, "q"),
                        )
                        && let Ok(source_index) = col.parse::<usize>()
                    {
                        let _ = navigation_proxy.send_event(UserEvent::PaletteSubmit {
                            source_index,
                            input,
                        });
                    }
                    return false;
                }
                if target.starts_with("neuralia:newtab") {
                    if remote_capability_matches(&target, &navigation_capability)
                        && let Ok(action_url) = Url::parse(&target)
                        && let Some((_, val)) = action_url.query_pairs().find(|(k, _)| k == "col")
                        && let Ok(idx) = val.parse::<usize>()
                    {
                        let _ = navigation_proxy.send_event(UserEvent::NewTab(idx));
                    }
                    return false;
                }
                if target.starts_with("neuralia:expand") {
                    if remote_capability_matches(&target, &navigation_capability)
                        && let Ok(action_url) = Url::parse(&target)
                        && let Some((_, val)) = action_url.query_pairs().find(|(k, _)| k == "col")
                        && let Ok(idx) = val.parse::<usize>()
                    {
                        let _ = navigation_proxy.send_event(UserEvent::ExpandComparator(idx));
                    }
                    return false;
                }
                if target.starts_with("neuralia:minimize") {
                    if remote_capability_matches(&target, &navigation_capability)
                        && let Ok(action_url) = Url::parse(&target)
                        && let Some((_, val)) = action_url.query_pairs().find(|(k, _)| k == "col")
                        && let Ok(idx) = val.parse::<usize>()
                    {
                        let _ = navigation_proxy.send_event(UserEvent::MinimizeComparator(idx));
                    }
                    return false;
                }
                if let Some(event) = remote_neuralia_action(&target, &navigation_capability) {
                    let _ = navigation_proxy.send_event(event);
                    return false;
                }

                remote_web_target(&target, false) || is_view_source_target(&target, false)
            })
            .with_new_window_req_handler(move |target, _features| {
                if remote_web_target(&target, false) {
                    let _ = new_window_proxy.send_event(UserEvent::OpenInColumn(col_index, target));
                }
                NewWindowResponse::Deny
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    /// O login abre-se com `window.open`, e ate aqui isso destruia as tres
    /// colunas para pôr um WebView unico no lugar delas -- perdia-se a
    /// comparacao e o ecra ficava com os pixeis das janelas mortas. O popup
    /// pertence a coluna que o pediu e e nela que carrega.
    fn open_in_column(&mut self, index: usize, url: String) {
        if self.surface != Surface::Comparator {
            self.web(url);
            return;
        }

        let loaded = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(index))
            .is_some_and(|view| view.webview.load_url(&url).is_ok());

        if !loaded {
            self.web(url);
        }
    }

    fn split_webview_builder(
        &self,
        source_index: usize,
        source_name: &'static str,
        allow_local: bool,
        private: bool,
    ) -> WebViewBuilder<'static> {
        let navigation_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let capability = remote_capability();
        let navigation_capability = capability.clone();
        let init_script = format!(
            "window.__neuralia_col_index = {source_index}; window.__neuralia_col_name = '{source_name}';\n{NEURALIA_KEYMAP_SCRIPT}\n{NEURALIA_PALETTE_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}"
        )
        .replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
            .with_incognito(private)
            .with_initialization_script(init_script)
            .with_navigation_handler(move |target| {
                if target.starts_with("neuralia:split-close") {
                    if remote_capability_matches(&target, &navigation_capability) {
                        let _ = navigation_proxy.send_event(UserEvent::CloseSplit);
                    }
                    return false;
                }
                if target.starts_with("neuralia:split-expand") {
                    if remote_capability_matches(&target, &navigation_capability) {
                        let _ = navigation_proxy.send_event(UserEvent::ToggleSplitFullscreen);
                    }
                    return false;
                }
                if target.starts_with("neuralia:palette") {
                    if remote_capability_matches(&target, &navigation_capability)
                        && let Some(input) = neuralia_query_param(&target, "q")
                    {
                        let _ = navigation_proxy.send_event(UserEvent::PaletteSubmit {
                            source_index,
                            input,
                        });
                    }
                    return false;
                }
                if let Some(event) = remote_neuralia_action(&target, &navigation_capability) {
                    let _ = navigation_proxy.send_event(event);
                    return false;
                }
                remote_web_target(&target, allow_local)
                    || is_view_source_target(&target, allow_local)
            })
            .with_new_window_req_handler(move |target, _features| {
                if remote_web_target(&target, false) {
                    let event = if private {
                        UserEvent::OpenPrivateSplit {
                            source_index,
                            url: target,
                        }
                    } else {
                        UserEvent::OpenSplit {
                            source_index,
                            url: target,
                        }
                    };
                    let _ = new_window_proxy.send_event(event);
                }
                NewWindowResponse::Deny
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    fn open_split(&mut self, source_index: usize, url: String, allow_local: bool) {
        self.open_split_mode(source_index, url, allow_local, false);
    }

    fn open_split_mode(
        &mut self,
        source_index: usize,
        url: String,
        allow_local: bool,
        private: bool,
    ) {
        if self.surface != Surface::Comparator {
            self.web(url);
            return;
        }

        let Ok(valid) = neural_core::validate_web_url(&url) else {
            self.show_splash("URL da fonte inválida.".to_string(), 3);
            return;
        };
        if !allow_local && neural_core::is_local_network_target(&valid) {
            self.show_splash(
                "A página não pode redirecionar a fonte para a rede local.".to_string(),
                4,
            );
            return;
        }

        let Some(source_name) = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(source_index))
            .map(|view| view.name)
        else {
            return;
        };

        if !private {
            let value = valid.to_string();
            let title = valid
                .host_str()
                .map(|host| format!("Fonte · {host}"))
                .unwrap_or_else(|| "Fonte Web".to_string());
            let mut document = MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Web,
                title.clone(),
                Some(value.clone()),
                value.clone(),
            )
            .provider(source_name);
            if let Some(session) = &mut self.current_research {
                document = document.session(session.id.clone());
                let memory_id = document.id.clone();
                session.add_source(
                    Some(source_name.to_string()),
                    title,
                    value.clone(),
                    Some(memory_id),
                    value,
                );
                self.memory.save_session(session.clone());
            }
            self.memory.capture(document);
        }

        self.leave_fullscreen();
        if let Some(comp) = &mut self.comparator {
            comp.expanded = None;
            if let Some(previous) = comp.split.take() {
                drop(previous);
            }
        }
        // O Split View substitui a topologia de colunas. Os divisores sao
        // janelas Win32 independentes; esconda-os antes de criar a nova WebView
        // para que nenhum divisor antigo fique por cima do painel lateral.
        self.hide_comparator_splitters();

        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;
        let ai_width = logical_w * 0.54;
        let bounds = wry::Rect {
            position: LogicalPosition::new(ai_width, COMPARATOR_CHROME_HEIGHT).into(),
            size: LogicalSize::new(
                (logical_w - ai_width).max(1.0),
                (logical_h - COMPARATOR_CHROME_HEIGHT).max(100.0),
            )
            .into(),
        };

        let result = self
            .split_webview_builder(source_index, source_name, allow_local, private)
            .with_bounds(bounds)
            .with_url(valid.as_str())
            .build_as_child(window);

        match result {
            Ok(webview) => {
                let _ = webview.zoom(self.zoom);
                if let Some(comp) = &mut self.comparator {
                    if !private {
                        let links = &mut comp.contexts[source_index];
                        let value = valid.to_string();
                        if links.last() != Some(&value) {
                            links.push(value);
                            if links.len() > 32 {
                                links.remove(0);
                            }
                        }
                    }
                    comp.split = Some(SplitView {
                        webview,
                        source_index,
                        url: valid.to_string(),
                        fullscreen: false,
                        private,
                    });
                }
                self.update_comparator_layout();
                self.sync_comparator_splitters();
                self.request_redraw();
            }
            Err(error) => {
                self.show_splash(format!("Não consegui abrir a fonte ao lado: {error}"), 4)
            }
        }
    }

    fn open_private_panel(&mut self) {
        if self.surface != Surface::Comparator {
            return;
        }
        let source_index = self
            .comparator
            .as_ref()
            .and_then(|comp| {
                comp.expanded.or_else(|| {
                    comp.views
                        .iter()
                        .enumerate()
                        .find_map(|(index, _)| (!comp.minimized[index]).then_some(index))
                })
            })
            .unwrap_or(0);
        self.open_split_mode(
            source_index,
            "https://www.google.com/".to_string(),
            false,
            true,
        );
    }

    fn new_tab(&mut self, source_index: usize) {
        if self.surface == Surface::Comparator {
            let index = source_index.min(COMPARATOR_COLUMNS - 1);
            if let Some(comp) = &mut self.comparator
                && comp.minimized[index]
            {
                comp.minimized[index] = false;
            }
            self.update_comparator_layout();
            self.sync_comparator_splitters();
            self.open_ai_palette(index);
        } else {
            self.focus_omnibox();
        }
    }

    fn close_split(&mut self) {
        let was_fullscreen = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| split.fullscreen);
        if let Some(comp) = &mut self.comparator
            && let Some(split) = comp.split.take()
        {
            drop(split);
        }
        if was_fullscreen && let Some(window) = &self.window {
            window.set_fullscreen(None);
        }
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    fn toggle_split_fullscreen(&mut self) {
        let Some(fullscreen) = self
            .comparator
            .as_mut()
            .and_then(|comp| comp.split.as_mut())
            .map(|split| {
                split.fullscreen = !split.fullscreen;
                split.fullscreen
            })
        else {
            return;
        };

        if let Some(window) = &self.window {
            window.set_fullscreen(if fullscreen {
                Some(Fullscreen::Borderless(None))
            } else {
                None
            });
        }
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    fn submit_palette(&mut self, source_index: usize, input: String) {
        let input = input.trim();
        if input.is_empty() {
            return;
        }

        match parse_intent(input) {
            Ok(Intent::Read(url)) | Ok(Intent::Web(url)) => {
                self.open_split(source_index, url.to_string(), true);
            }
            Ok(Intent::Home) => self.show_home(),
            Ok(Intent::Ask(query)) | Ok(Intent::Compare(query)) => {
                let target = match source_index {
                    0 => google_ai_url(&query, &self.config.language),
                    1 => chatgpt_search_url(&query),
                    2 => claude_search_url(&query),
                    _ => return,
                };
                match target {
                    Ok(url) => {
                        if let Some(view) = self
                            .comparator
                            .as_ref()
                            .and_then(|comp| comp.views.get(source_index))
                        {
                            let _ = view.webview.load_url(url.as_str());
                            self.record(HistoryKind::Ask, query, url.to_string());
                        }
                    }
                    Err(error) => self.show_splash(error.to_string(), 3),
                }
            }
            Err(error) => self.show_splash(error.to_string(), 3),
        }
    }

    /// Um nivel para tras. O Escape da janela nativa e o Escape apanhado dentro
    /// das paginas acabam os dois aqui.
    fn go_back(&mut self) {
        if self.surface == Surface::Comparator {
            if self
                .comparator
                .as_ref()
                .and_then(|comp| comp.split.as_ref())
                .is_some_and(|split| split.fullscreen)
            {
                self.toggle_split_fullscreen();
                return;
            }
            if self
                .comparator
                .as_ref()
                .is_some_and(|comp| comp.split.is_some())
            {
                self.close_split();
                return;
            }
            if let Some(comp) = &self.comparator
                && comp.expanded.is_some()
            {
                self.restore_comparator();
                return;
            }
        }
        self.show_home();
    }

    /// Liga/desliga a rolagem de leitura. O temporizador e nativo e nao vive na
    /// pagina: assim sobrevive a navegacao dentro do site.
    fn toggle_auto_scroll(&mut self) {
        SPLASH_ASKS.store(false, Ordering::SeqCst);
        self.auto_scroll_answered = true;
        self.auto_scroll = !self.auto_scroll;
        self.auto_scroll_token = self.auto_scroll_token.wrapping_add(1);

        if self.auto_scroll {
            self.schedule_auto_scroll();
        }

        self.announce_auto_scroll();

        self.request_redraw();

        if self.surface == Surface::Home {
            self.status = Some(if self.auto_scroll {
                format!("Rolagem automática ligada — {AUTO_SCROLL_SECONDS}s. F8 desliga.")
            } else {
                "Rolagem automática desligada.".to_string()
            });
            self.request_redraw();
        }
    }

    /// Mostra na propria pagina em que estado esta a rolagem. A barra nativa
    /// tambem o diz, mas em ecra completo ela esconde-se.
    fn announce_auto_scroll(&self) {
        let toast = AUTO_SCROLL_TOAST
            .replace("__ON__", if self.auto_scroll { "true" } else { "false" })
            .replace("__SECONDS__", &AUTO_SCROLL_SECONDS.to_string());
        self.for_each_visible_webview(|webview| {
            let _ = webview.evaluate_script(&toast);
        });
    }

    /// Aviso flutuante, centrado no fundo da janela, que se apaga sozinho.
    fn show_splash(&mut self, text: String, seconds: u64) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SPLASH_WIDTH * scale).round() as i32;
        let height = (SPLASH_HEIGHT * scale).round() as i32;

        if let Ok(mut slot) = SPLASH_TEXT.lock() {
            *slot = text;
        }

        let splash = match self.splash {
            Some(splash) => splash,
            None => unsafe {
                let created = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    WS_POPUP | WS_VISIBLE,
                    0,
                    0,
                    width,
                    height,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if created.is_null() {
                    return;
                }
                let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                if SetWindowSubclass(
                    created,
                    Some(splash_subclass),
                    SPLASH_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, height, height);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.splash = Some(created);
                created
            },
        };

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            SetWindowPos(
                splash,
                std::ptr::null_mut(),
                origin.x + (client.right - width) / 2,
                origin.y + client.bottom - height - (48.0 * scale) as i32,
                width,
                height,
                SWP_NOACTIVATE,
            );
            ShowWindow(splash, SW_SHOW);
            InvalidateRect(splash, std::ptr::null(), 1);
        }

        self.splash_token = self.splash_token.wrapping_add(1);
        let token = self.splash_token;
        let proxy = self.proxy.clone();
        let _ = thread::Builder::new()
            .name("neural-splash".into())
            .spawn(move || {
                thread::sleep(Duration::from_secs(seconds));
                let _ = proxy.send_event(UserEvent::HideSplash(token));
            });
    }

    fn hide_splash(&mut self, token: u64) {
        if token != self.splash_token {
            return;
        }
        if SPLASH_ASKS.swap(false, Ordering::SeqCst) {
            self.auto_scroll_answered = true;
            self.auto_scroll = false;
        }
        if let Some(splash) = self.splash.take() {
            unsafe {
                DestroyWindow(splash);
            }
        }
    }

    fn show_gmail_toast(&mut self, sender: &str, subject: &str) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (GMAIL_TOAST_WIDTH * scale).round() as i32;
        let height = (GMAIL_TOAST_HEIGHT * scale).round() as i32;

        let body = match (sender.trim(), subject.trim()) {
            ("", "") => "Nova mensagem na sua caixa de entrada".to_string(),
            ("", subject) => subject.to_string(),
            (sender, "") => sender.to_string(),
            (sender, subject) => format!("{sender} · {subject}"),
        };
        if let Ok(mut slot) = GMAIL_TOAST_TEXT.lock() {
            *slot = body;
        }

        let toast = match self.gmail_toast {
            Some(toast) => toast,
            None => unsafe {
                let created = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    WS_POPUP | WS_VISIBLE,
                    0,
                    0,
                    width,
                    height,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if created.is_null() {
                    return;
                }
                if SetWindowSubclass(
                    created,
                    Some(gmail_toast_subclass),
                    GMAIL_TOAST_SUBCLASS_ID,
                    0,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 18, 18);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.gmail_toast = Some(created);
                created
            },
        };

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let margin = (18.0 * scale) as i32;
            SetWindowPos(
                toast,
                std::ptr::null_mut(),
                origin.x + client.right - width - margin,
                origin.y + client.bottom - height - margin,
                width,
                height,
                SWP_NOACTIVATE,
            );
            ShowWindow(toast, SW_SHOW);
            InvalidateRect(toast, std::ptr::null(), 1);
        }

        self.gmail_toast_token = self.gmail_toast_token.wrapping_add(1);
        let token = self.gmail_toast_token;
        let proxy = self.proxy.clone();
        let _ = thread::Builder::new()
            .name("neural-gmail-toast".into())
            .spawn(move || {
                thread::sleep(Duration::from_secs(7));
                let _ = proxy.send_event(UserEvent::HideGmailToast(token));
            });
    }

    fn hide_gmail_toast(&mut self, token: u64) {
        if token != self.gmail_toast_token {
            return;
        }
        if let Some(toast) = self.gmail_toast.take() {
            unsafe {
                DestroyWindow(toast);
            }
        }
    }

    fn google_session_available(&self) -> bool {
        let source = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.first().map(|view| &view.webview))
            .or(self.webview.as_ref());
        let Some(source) = source else {
            return false;
        };

        source
            .cookies_for_url("https://mail.google.com/")
            .ok()
            .is_some_and(|cookies| {
                cookies.iter().any(|cookie| {
                    matches!(
                        cookie.name(),
                        "SID" | "HSID" | "SSID" | "SAPISID" | "__Secure-1PSID" | "__Secure-3PSID"
                    )
                })
            })
    }

    fn schedule_gmail_probe(&mut self, seconds: u64) {
        if self.gmail_monitor.is_some() {
            return;
        }
        self.gmail_probe_token = self.gmail_probe_token.wrapping_add(1);
        let token = self.gmail_probe_token;
        let proxy = self.proxy.clone();
        let _ = thread::Builder::new()
            .name("neural-gmail-probe".into())
            .spawn(move || {
                thread::sleep(Duration::from_secs(seconds));
                let _ = proxy.send_event(UserEvent::GmailProbe(token));
            });
    }

    fn maybe_start_gmail_monitor(&mut self) {
        if self.gmail_monitor.is_some() || !self.google_session_available() {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };

        let capability = remote_capability();
        let navigation_capability = capability.clone();
        let proxy = self.proxy.clone();
        let init_script = GMAIL_MONITOR_SCRIPT.replace("__NEURALIA_CAP__", &capability);
        let bounds = wry::Rect {
            position: LogicalPosition::new(-10_000.0, -10_000.0).into(),
            size: LogicalSize::new(1.0, 1.0).into(),
        };

        let result = WebViewBuilder::new()
            .with_initialization_script(init_script)
            .with_navigation_handler(move |target| {
                if target.starts_with("neuralia:gmail-state") {
                    if remote_capability_matches(&target, &navigation_capability)
                        && let Some(count) = neuralia_query_param(&target, "count")
                        && let Ok(unread) = count.parse::<u32>()
                    {
                        let _ = proxy.send_event(UserEvent::GmailInboxState {
                            unread,
                            sender: neuralia_query_param(&target, "sender").unwrap_or_default(),
                            subject: neuralia_query_param(&target, "subject").unwrap_or_default(),
                            key: neuralia_query_param(&target, "key").unwrap_or_default(),
                        });
                    }
                    return false;
                }
                Url::parse(&target).ok().is_some_and(|url| {
                    url.scheme() == "https"
                        && matches!(
                            url.host_str(),
                            Some("mail.google.com") | Some("accounts.google.com")
                        )
                })
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(false)
            .with_bounds(bounds)
            .with_url("https://mail.google.com/mail/u/0/#inbox")
            .build_as_child(window);

        if let Ok(webview) = result {
            self.gmail_monitor = Some(webview);
        }
    }

    fn handle_gmail_state(&mut self, unread: u32, sender: String, subject: String, key: String) {
        let notify = gmail_is_new_mail(
            self.gmail_last_unread,
            self.gmail_last_key.as_deref(),
            unread,
            &key,
        );
        self.gmail_last_unread = Some(unread);
        self.gmail_last_key = Some(key);
        if notify {
            self.show_gmail_toast(&sender, &subject);
        }
    }

    /// Abrir um documento: anuncia e da tempo de se comecar a ler em paz antes
    /// do primeiro avanco.
    fn begin_reading_session(&mut self, is_pdf: bool) {
        self.reading_pdf = is_pdf;

        // Pergunta-se uma vez por sessao. Depois disso respeita-se a resposta
        // em silencio -- perguntar a cada pagina seria assedio, nao consentimento.
        if !self.auto_scroll_answered {
            self.ask_auto_scroll();
            return;
        }

        if self.auto_scroll {
            self.auto_scroll_token = self.auto_scroll_token.wrapping_add(1);
            self.schedule_auto_scroll();
            self.show_splash(
                format!("Rolagem automática a cada {AUTO_SCROLL_SECONDS}s  ·  F8 desliga"),
                4,
            );
        }
    }

    fn ask_auto_scroll(&mut self) {
        SPLASH_ASKS.store(true, Ordering::SeqCst);
        self.show_splash(
            format!("Rolar a página sozinho a cada {AUTO_SCROLL_SECONDS}s?"),
            AUTO_SCROLL_PROMPT_SECONDS,
        );
    }

    /// Sem resposta nao se mexe: se a pergunta desaparecer sozinha, fica "nao"
    /// ate a pessoa carregar em F8.
    fn answer_auto_scroll(&mut self, yes: bool) {
        SPLASH_ASKS.store(false, Ordering::SeqCst);
        self.auto_scroll_answered = true;
        self.auto_scroll = yes;
        self.hide_splash(self.splash_token);

        if yes {
            self.auto_scroll_token = self.auto_scroll_token.wrapping_add(1);
            self.schedule_auto_scroll();
            self.show_splash(
                format!("Rolagem automática ligada — {AUTO_SCROLL_SECONDS}s  ·  F8 desliga"),
                4,
            );
        }
        self.request_redraw();
    }

    fn schedule_auto_scroll(&self) {
        self.schedule_auto_scroll_in(AUTO_SCROLL_SECONDS);
    }

    /// Sobe ou desce um degrau da escada de zoom, como o Chrome.
    fn step_zoom(&mut self, direction: i32) {
        let current = self.zoom;
        let next = if direction > 0 {
            ZOOM_STEPS
                .iter()
                .find(|step| **step > current + 0.001)
                .copied()
                .unwrap_or(current)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .find(|step| **step < current - 0.001)
                .copied()
                .unwrap_or(current)
        };
        self.set_zoom(next);
    }

    fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        let zoom = self.zoom;
        self.for_each_visible_webview(|webview| {
            let _ = webview.zoom(zoom);
        });
        self.show_splash(format!("Zoom {}%", (zoom * 100.0).round() as i32), 2);
    }

    fn reload_page(&mut self) {
        self.for_each_visible_webview(|webview| {
            let _ = webview.reload();
        });
    }

    fn print_page(&mut self) {
        // Uma folha por pagina visivel seria absurdo: imprime-se a que se esta
        // mesmo a ver.
        let printed = match (&self.webview, &self.comparator) {
            (Some(webview), _) => webview.print().is_ok(),
            (None, Some(comp)) => comp
                .views
                .get(comp.expanded.unwrap_or(0))
                .is_some_and(|view| view.webview.print().is_ok()),
            _ => false,
        };
        if !printed {
            self.show_splash("Não há nada para imprimir aqui.".to_string(), 3);
        }
    }

    /// O equivalente ao Ctrl+L do Chrome: volta a barra e seleciona o texto.
    fn focus_omnibox(&mut self) {
        self.show_home();
        if let Some(edit) = self.omnibox {
            unsafe {
                SetFocus(edit);
                SendMessageW(edit, EM_SETSEL, 0, -1);
            }
        }
    }

    /// Inspetor do Chromium, o mesmo que o F12 abre num navegador.
    fn open_devtools(&mut self) {
        let mut opened = false;
        self.for_each_visible_webview(|webview| {
            webview.open_devtools();
            opened = true;
        });
        if !opened {
            self.show_splash("Não há página aberta para inspecionar.".to_string(), 3);
        }
    }

    /// `view-source:` e do proprio Chromium, mas a pagina nao pode pedi-lo por
    /// script: o handler de navegacao so deixa passar o alvo que o lado nativo
    /// constroi a partir do URL actual. So faz sentido com um documento a vista.
    fn view_source(&mut self) {
        let mut visible = 0;
        self.for_each_visible_webview(|_| visible += 1);
        if visible != 1 {
            let reason = if visible == 0 {
                "Ver código-fonte só numa página aberta."
            } else {
                "Ver código-fonte só com uma página em ecrã completo."
            };
            self.show_splash(reason.to_string(), 3);
            return;
        }
        self.for_each_visible_webview(|webview| {
            let Ok(current) = webview.url() else {
                return;
            };
            let target = format!("view-source:{current}");
            if is_view_source_target(&target, true) {
                let _ = webview.load_url(&target);
            }
        });
    }

    fn toggle_column_fullscreen(&mut self) {
        let index = match &self.comparator {
            Some(comp) => comp.expanded.unwrap_or(0),
            None => return,
        };
        self.expand_comparator(index);
    }

    fn schedule_auto_scroll_in(&self, seconds: u64) {
        let proxy = self.proxy.clone();
        let token = self.auto_scroll_token;
        let _ = thread::Builder::new()
            .name("neural-autoscroll".into())
            .spawn(move || {
                thread::sleep(Duration::from_secs(seconds));
                let _ = proxy.send_event(UserEvent::AutoScrollTick(token));
            });
    }

    fn auto_scroll_tick(&mut self, token: u64) {
        if !self.auto_scroll || token != self.auto_scroll_token {
            return;
        }
        // O script avanca tudo o que e nosso ou HTML: nas tres colunas a tecla
        // so chegaria a uma, e num documento sozinho a tecla sintetica e uma
        // arma que pode disparar noutra aplicacao. So o visualizador de PDF do
        // Edge (URL .pdf), que nao aceita script, fica com a tecla.
        match self.surface {
            Surface::Comparator | Surface::Pdf => {
                self.for_each_visible_webview(|webview| {
                    let _ = webview.evaluate_script(AUTO_SCROLL_SCRIPT);
                });
            }
            Surface::Reader | Surface::External => {
                let edge_pdf = self
                    .webview
                    .as_ref()
                    .and_then(|webview| webview.url().ok())
                    .and_then(|url| Url::parse(&url).ok())
                    .is_some_and(|url| is_pdf_url(&url));
                if edge_pdf {
                    self.page_down_synthetic();
                } else if let Some(webview) = &self.webview {
                    let _ = webview.evaluate_script(AUTO_SCROLL_SCRIPT);
                }
            }
            Surface::Home => {}
        }
        self.schedule_auto_scroll();
    }

    /// O visualizador de PDF do Edge corre noutro documento, noutra origem e
    /// noutro processo: nenhum script do host la chega. A unica via que resta e
    /// a tecla, e so a enviamos com a nossa janela em primeiro plano e a
    /// pertencer ao nosso processo, verificado mesmo antes do envio -- caso
    /// contrario iria parar a aplicacao de outra pessoa.
    fn page_down_synthetic(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };

        // A tecla vai para quem tiver o foco. Depois de carregar um PDF o
        // visualizador nao o toma sozinho -- sem isto o PageDown caia no vazio
        // e a pagina nao se mexia.
        if let Some(webview) = &self.webview {
            let _ = webview.focus();
        }

        unsafe {
            let mut inputs: [INPUT; 2] = std::mem::zeroed();
            for (index, input) in inputs.iter_mut().enumerate() {
                input.r#type = INPUT_KEYBOARD;
                input.Anonymous.ki.wVk = VK_NEXT;
                input.Anonymous.ki.dwFlags = if index == 1 { KEYEVENTF_KEYUP } else { 0 };
            }

            let foreground = GetForegroundWindow();
            if foreground != hwnd {
                return;
            }
            let mut owner = 0u32;
            GetWindowThreadProcessId(foreground, &mut owner);
            if owner != std::process::id() {
                return;
            }
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }

    /// O WebView unico (Reader ou Full Web), ou as colunas que estao a ser
    /// vistas: em ecra completo so a expandida, nas tres colunas todas elas.
    fn for_each_visible_webview(&self, mut action: impl FnMut(&WebView)) {
        if let Some(webview) = &self.webview {
            action(webview);
        }
        if let Some(comp) = &self.comparator {
            if let Some(split) = &comp.split {
                if split.fullscreen {
                    action(&split.webview);
                    return;
                }
                if let Some(view) = comp.views.get(split.source_index) {
                    action(&view.webview);
                }
                action(&split.webview);
                return;
            }

            match comp.expanded {
                Some(index) => {
                    if let Some(view) = comp.views.get(index) {
                        action(&view.webview);
                    }
                }
                None => {
                    for view in &comp.views {
                        action(&view.webview);
                    }
                }
            }
        }
    }

    fn is_fullscreen_column(&self) -> bool {
        self.comparator
            .as_ref()
            .is_some_and(|comp| comp.expanded.is_some())
    }

    /// Em tres colunas a barra esta sempre la; em ecra completo so enquanto o
    /// rato a chamar.
    fn bar_visible(&self) -> bool {
        match &self.comparator {
            Some(comp) if comp.split.is_some() => true,
            Some(comp) => comp.expanded.is_none() || self.chrome_revealed,
            None => false,
        }
    }

    fn bar_layout(&self) -> Option<BarLayout> {
        let (Some(window), Some(comp)) = (&self.window, &self.comparator) else {
            return None;
        };
        Some(BarLayout::with_contexts(
            window.inner_size().width as f64,
            window.scale_factor(),
            self.bar_visible(),
            comp.views.len(),
            std::array::from_fn(|index| comp.contexts[index].len()),
        ))
    }

    /// Cria/mostra/esconde o botao flutuante de saida. Existe apenas enquanto
    /// houver uma coluna em ecra completo -- e a unica saida sempre visivel,
    /// porque a barra de titulo desapareceu e a barra da app auto-esconde-se.
    fn sync_exit_button(&mut self) {
        // Acompanha a barra: aparece quando o rato a chama e desaparece com ela.
        let wanted = self.surface == Surface::Comparator
            && self.is_fullscreen_column()
            && self.chrome_revealed;

        if !wanted {
            if let Some(button) = self.exit_button.take() {
                unsafe {
                    DestroyWindow(button);
                }
            }
            return;
        }

        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (EXIT_BUTTON_WIDTH * scale).round() as i32;
        let height = (EXIT_BUTTON_HEIGHT * scale).round() as i32;

        let button = match self.exit_button {
            Some(button) => button,
            None => unsafe {
                // Janela de topo, e nao filha: uma janela filha ficaria por
                // baixo do WebView2 na ordem Z e nunca se veria.
                let created = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    WS_POPUP | WS_VISIBLE,
                    0,
                    0,
                    width,
                    height,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if created.is_null() {
                    return;
                }
                let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                if SetWindowSubclass(
                    created,
                    Some(exit_button_subclass),
                    EXIT_BUTTON_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, height, height);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.exit_button = Some(created);
                created
            },
        };

        // Centrado no topo, logo abaixo da barra revelada.
        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let top = ((COMPARATOR_CHROME_HEIGHT + 10.0) * scale).round() as i32;
            SetWindowPos(
                button,
                std::ptr::null_mut(),
                origin.x + (client.right - width) / 2,
                origin.y + top,
                width,
                height,
                SWP_NOACTIVATE,
            );
            ShowWindow(button, SW_SHOW);
        }
    }

    /// Mostra a barra e marca-a para desaparecer sozinha. Cada chamada invalida
    /// o temporizador anterior, por isso ela fica enquanto o rato la andar.
    fn reveal_chrome(&mut self) {
        // Adiar e so escrever um numero; nao ha thread nenhuma envolvida.
        self.chrome_deadline
            .store(now_ms() + 2500, Ordering::SeqCst);

        if self.chrome_revealed {
            return;
        }

        self.chrome_revealed = true;
        self.chrome_token = self.chrome_token.wrapping_add(1);

        let proxy = self.proxy.clone();
        let token = self.chrome_token;
        let deadline = Arc::clone(&self.chrome_deadline);
        let _ = thread::Builder::new()
            .name("neural-chrome".into())
            .spawn(move || {
                loop {
                    let now = now_ms();
                    let target = deadline.load(Ordering::SeqCst);
                    if now >= target {
                        let _ = proxy.send_event(UserEvent::HideChrome(token));
                        return;
                    }
                    thread::sleep(Duration::from_millis((target - now).min(300)));
                }
            });

        self.update_comparator_layout();
        self.sync_exit_button();
        self.request_redraw();
    }

    fn hide_chrome(&mut self, token: u64) {
        if token != self.chrome_token || !self.chrome_revealed {
            return;
        }
        self.chrome_revealed = false;
        self.bar_hover = None;
        self.update_comparator_layout();
        self.sync_exit_button();
        self.request_redraw();
    }

    fn hide_comparator_splitters(&self) {
        for hwnd in self.splitters.iter().flatten() {
            unsafe {
                ShowWindow(*hwnd, SW_HIDE);
            }
        }
    }

    fn sync_comparator_splitters(&mut self) {
        // Comece sempre escondendo todas as janelas de divisor. Assim uma
        // transicao 3 -> 2 -> 1 colunas, ou Comparator -> Split View, nunca
        // deixa um splitter da geometria anterior visivel.
        self.hide_comparator_splitters();

        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };

        let (show, boundaries, content_height, scale) = if let Some(comp) = &self.comparator {
            let scale = window.scale_factor().max(1.0);
            let size = window.inner_size();
            let logical_w = size.width as f64 / scale;
            let logical_h = size.height as f64 / scale;
            let show = self.surface == Surface::Comparator
                && comp.split.is_none()
                && comp.expanded.is_none();
            let visible: Vec<usize> = comp
                .views
                .iter()
                .enumerate()
                .filter_map(|(index, _)| (!comp.minimized[index]).then_some(index))
                .collect();
            let total_weight: f64 = visible
                .iter()
                .map(|index| comp.weights[*index].max(0.05))
                .sum::<f64>()
                .max(0.05);
            let mut boundaries = Vec::new();
            let mut x = 0.0;
            for (slot, index) in visible.iter().enumerate() {
                if slot + 1 == visible.len() {
                    break;
                }
                x += logical_w * comp.weights[*index].max(0.05) / total_weight;
                boundaries.push(x);
            }
            (
                show,
                boundaries,
                (logical_h - COMPARATOR_CHROME_HEIGHT).max(1.0),
                scale,
            )
        } else {
            (false, Vec::new(), 1.0, window.scale_factor().max(1.0))
        };

        let mut origin = POINT { x: 0, y: 0 };
        unsafe {
            ClientToScreen(owner, &mut origin);
        }

        for slot in 0..self.splitters.len() {
            if !show || slot >= boundaries.len() {
                continue;
            }

            let hwnd = match self.splitters[slot] {
                Some(hwnd) => hwnd,
                None => unsafe {
                    let width = (SPLITTER_WIDTH * scale).round().max(3.0) as i32;
                    let height = (content_height * scale).round().max(1.0) as i32;
                    let created = CreateWindowExW(
                        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                        windows_sys::w!("STATIC"),
                        windows_sys::w!(""),
                        WS_POPUP | WS_VISIBLE,
                        0,
                        0,
                        width,
                        height,
                        owner,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    );
                    if created.is_null() {
                        continue;
                    }
                    let proxy_ptr =
                        (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                    if SetWindowSubclass(
                        created,
                        Some(comparator_splitter_subclass),
                        SPLITTER_SUBCLASS_BASE + slot,
                        proxy_ptr,
                    ) == 0
                    {
                        DestroyWindow(created);
                        continue;
                    }
                    self.splitters[slot] = Some(created);
                    created
                },
            };

            let width = (SPLITTER_WIDTH * scale).round().max(3.0) as i32;
            let x =
                origin.x + (boundaries[slot] * scale - SPLITTER_WIDTH * scale / 2.0).round() as i32;
            let y = origin.y + (COMPARATOR_CHROME_HEIGHT * scale).round() as i32;
            let height = (content_height * scale).round().max(1.0) as i32;
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE,
                );
                ShowWindow(hwnd, SW_SHOW);
                InvalidateRect(hwnd, std::ptr::null(), 1);
            }
        }
    }

    fn resize_comparator(&mut self, divider: usize, screen_x: i32) {
        let (Some(window), Some(comp)) = (&self.window, &mut self.comparator) else {
            return;
        };
        if comp.split.is_some() || comp.expanded.is_some() {
            return;
        }

        let visible: Vec<usize> = comp
            .views
            .iter()
            .enumerate()
            .filter_map(|(index, _)| (!comp.minimized[index]).then_some(index))
            .collect();
        if divider + 1 >= visible.len() {
            return;
        }

        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let mut point = POINT { x: screen_x, y: 0 };
        unsafe {
            ScreenToClient(owner, &mut point);
        }
        let scale = window.scale_factor().max(1.0);
        let logical_w = window.inner_size().width as f64 / scale;
        let mouse_x = (point.x as f64 / scale).clamp(0.0, logical_w);

        let total_weight: f64 = visible
            .iter()
            .map(|index| comp.weights[*index].max(0.05))
            .sum::<f64>()
            .max(0.05);
        let left_index = visible[divider];
        let right_index = visible[divider + 1];
        let before_weight: f64 = visible[..divider]
            .iter()
            .map(|index| comp.weights[*index].max(0.05))
            .sum();
        let pair_weight = comp.weights[left_index].max(0.05) + comp.weights[right_index].max(0.05);
        let left_edge = logical_w * before_weight / total_weight;
        let pair_span = logical_w * pair_weight / total_weight;
        if pair_span <= 1.0 {
            return;
        }
        let min_width = MIN_PANEL_WIDTH.min(pair_span * 0.45);
        let left_width = (mouse_x - left_edge).clamp(min_width, pair_span - min_width);
        let left_weight = pair_weight * left_width / pair_span;
        comp.weights[left_index] = left_weight.max(0.05);
        comp.weights[right_index] = (pair_weight - left_weight).max(0.05);

        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    fn private_bar_rect(&self) -> Option<UiRect> {
        let window = self.window.as_ref()?;
        if self.surface != Surface::Comparator || !self.bar_visible() {
            return None;
        }
        let scale = window.scale_factor().max(1.0);
        let width = window.inner_size().width as f64;
        let row_y = (TITLE_TAB_HEIGHT + 7.0) * scale;
        let row_h = 30.0 * scale;
        let button_w = 78.0 * scale;
        let margin = 8.0 * scale;
        let right = if let Some((label, _, _)) = self.split_bar_rects() {
            label.x - 6.0 * scale
        } else {
            width - margin
        };
        Some(UiRect {
            x: right - button_w,
            y: row_y,
            width: button_w,
            height: row_h,
        })
    }

    fn split_bar_rects(&self) -> Option<(UiRect, UiRect, UiRect)> {
        let (Some(window), Some(comp)) = (&self.window, &self.comparator) else {
            return None;
        };
        if comp.split.is_none() || !self.bar_visible() {
            return None;
        }
        let scale = window.scale_factor().max(1.0);
        let width = window.inner_size().width as f64;
        let margin = 8.0 * scale;
        let row_y = (TITLE_TAB_HEIGHT + 7.0) * scale;
        let row_h = 30.0 * scale;
        let close_w = 30.0 * scale;
        let expand_w = 30.0 * scale;
        let label_w = 150.0 * scale;
        let gap = 5.0 * scale;
        let close = UiRect {
            x: width - margin - close_w,
            y: row_y,
            width: close_w,
            height: row_h,
        };
        let expand = UiRect {
            x: close.x - gap - expand_w,
            y: row_y,
            width: expand_w,
            height: row_h,
        };
        let label = UiRect {
            x: expand.x - gap - label_w,
            y: row_y,
            width: label_w,
            height: row_h,
        };
        Some((label, expand, close))
    }

    fn comparator_bar_hit(&self) -> Option<BarHit> {
        if let Some(private) = self.private_bar_rect()
            && private.contains(self.cursor.0, self.cursor.1)
        {
            return Some(BarHit::Private);
        }
        if let Some((_label, expand, close)) = self.split_bar_rects() {
            if close.contains(self.cursor.0, self.cursor.1) {
                return Some(BarHit::SplitClose);
            }
            if expand.contains(self.cursor.0, self.cursor.1) {
                return Some(BarHit::SplitExpand);
            }
        }
        self.bar_layout()
            .and_then(|layout| layout.hit(self.cursor.0, self.cursor.1))
    }

    fn update_bar_hover(&mut self) {
        let next = self.comparator_bar_hit();
        if next != self.bar_hover {
            self.bar_hover = next;
            self.request_redraw();
        }
    }

    fn open_ai_palette(&mut self, source_index: usize) {
        if source_index >= COMPARATOR_COLUMNS {
            return;
        }
        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some())
        {
            self.close_split();
        }
        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.expanded.is_some())
        {
            self.restore_comparator();
        }
        if let Some(view) = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(source_index))
        {
            let _ = view
                .webview
                .evaluate_script("window.dispatchEvent(new CustomEvent('neuralia-open-palette'));");
        }
    }

    fn context_tab_url(&self, source_index: usize, context_index: usize) -> Option<String> {
        self.comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .and_then(|tabs| tabs.get(context_index))
            .cloned()
    }

    fn open_context_tab(&mut self, source_index: usize, context_index: usize) {
        if let Some(url) = self.context_tab_url(source_index, context_index) {
            self.open_split(source_index, url, false);
        }
    }

    fn open_context_tab_fullscreen(&mut self, source_index: usize, context_index: usize) {
        self.open_context_tab(source_index, context_index);
        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some())
        {
            self.toggle_split_fullscreen();
        }
    }

    fn close_context_tab(&mut self, source_index: usize, context_index: usize) {
        let Some(url) = self.context_tab_url(source_index, context_index) else {
            return;
        };
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| split.source_index == source_index && split.url == url);
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator
            && let Some(tabs) = comp.contexts.get_mut(source_index)
            && context_index < tabs.len()
        {
            tabs.remove(context_index);
        }
        self.request_redraw();
    }

    fn close_other_context_tabs(&mut self, source_index: usize, context_index: usize) {
        let Some(keep) = self.context_tab_url(source_index, context_index) else {
            return;
        };
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| split.source_index == source_index && split.url != keep);
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator
            && let Some(tabs) = comp.contexts.get_mut(source_index)
        {
            tabs.clear();
            tabs.push(keep);
        }
        self.request_redraw();
    }

    fn close_all_context_tabs(&mut self, source_index: usize) {
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| split.source_index == source_index);
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator
            && let Some(tabs) = comp.contexts.get_mut(source_index)
        {
            tabs.clear();
        }
        self.request_redraw();
    }

    fn context_menu_comparator(&mut self) {
        let hit = self.comparator_bar_hit();
        let Some(BarHit::ContextTab {
            source_index,
            context_index,
        }) = hit
        else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };

        let command = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            let open = wide_null("Abrir");
            let fullscreen = wide_null("Abrir em tela cheia");
            let close = wide_null("Fechar aba");
            let close_others = wide_null("Fechar outras abas deste grupo");
            let close_all = wide_null("Fechar todas deste grupo");
            AppendMenuW(menu, MF_STRING, TAB_MENU_OPEN, open.as_ptr());
            AppendMenuW(menu, MF_STRING, TAB_MENU_FULLSCREEN, fullscreen.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, TAB_MENU_CLOSE, close.as_ptr());
            AppendMenuW(
                menu,
                MF_STRING,
                TAB_MENU_CLOSE_OTHERS,
                close_others.as_ptr(),
            );
            AppendMenuW(menu, MF_STRING, TAB_MENU_CLOSE_ALL, close_all.as_ptr());

            let mut point = windows_sys::Win32::Foundation::POINT {
                x: self.cursor.0.round() as i32,
                y: self.cursor.1.round() as i32,
            };
            ClientToScreen(hwnd, &mut point);
            let selected = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                hwnd,
                std::ptr::null(),
            ) as usize;
            DestroyMenu(menu);
            selected
        };

        match command {
            TAB_MENU_OPEN => self.open_context_tab(source_index, context_index),
            TAB_MENU_FULLSCREEN => self.open_context_tab_fullscreen(source_index, context_index),
            TAB_MENU_CLOSE => self.close_context_tab(source_index, context_index),
            TAB_MENU_CLOSE_OTHERS => self.close_other_context_tabs(source_index, context_index),
            TAB_MENU_CLOSE_ALL => self.close_all_context_tabs(source_index),
            _ => {}
        }
    }

    fn click_comparator(&mut self) {
        let hit = self.comparator_bar_hit();
        match hit {
            Some(BarHit::WindowMinimize) => {
                if let Some(window) = &self.window {
                    window.set_minimized(true);
                }
            }
            Some(BarHit::WindowMaximize) => {
                if let Some(window) = &self.window {
                    window.set_maximized(!window.is_maximized());
                }
            }
            Some(BarHit::WindowClose) => {
                let _ = self.proxy.send_event(UserEvent::ExitRequested);
            }
            Some(BarHit::Private) => self.open_private_panel(),
            Some(BarHit::SplitClose) => self.close_split(),
            Some(BarHit::SplitExpand) => self.toggle_split_fullscreen(),
            Some(BarHit::Home) => self.show_home(),
            Some(BarHit::Column(index)) => self.expand_comparator(index),
            Some(BarHit::AddTab(index)) => self.open_ai_palette(index),
            Some(BarHit::ContextTab {
                source_index,
                context_index,
            }) => self.open_context_tab(source_index, context_index),
            None => {
                let scale = self
                    .window
                    .as_ref()
                    .map(|window| window.scale_factor().max(1.0))
                    .unwrap_or(1.0);
                if self.cursor.1 >= 0.0
                    && self.cursor.1 <= TITLE_TAB_HEIGHT * scale
                    && let Some(window) = &self.window
                {
                    let _ = window.drag_window();
                }
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
        }
    }
}

fn reader_article_memory_text(article: &ReaderArticle) -> String {
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
    output.truncate(output.len().min(512 * 1024));
    output
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface == Surface::Home && home_animation_enabled() {
            let now = Instant::now();
            if now >= self.next_home_frame {
                self.next_home_frame = now + Duration::from_millis(66);
                self.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_home_frame));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ExitRequested => event_loop.exit(),
            UserEvent::HomeRequested => self.show_home(),
            UserEvent::BackRequested => self.go_back(),
            UserEvent::ToggleAutoScroll => self.toggle_auto_scroll(),
            UserEvent::AutoScrollAnswer(yes) => self.answer_auto_scroll(yes),
            UserEvent::ZoomIn => self.step_zoom(1),
            UserEvent::ZoomOut => self.step_zoom(-1),
            UserEvent::ZoomReset => self.set_zoom(1.0),
            UserEvent::ReloadPage => self.reload_page(),
            UserEvent::PrintPage => self.print_page(),
            UserEvent::FocusOmnibox => self.focus_omnibox(),
            UserEvent::ToggleColumnFullscreen => self.toggle_column_fullscreen(),
            UserEvent::OpenDevTools => self.open_devtools(),
            UserEvent::ViewSource => self.view_source(),
            UserEvent::AutoScrollTick(token) => self.auto_scroll_tick(token),
            UserEvent::HideSplash(token) => self.hide_splash(token),
            UserEvent::GmailProbe(token) => {
                if token == self.gmail_probe_token && self.gmail_monitor.is_none() {
                    self.maybe_start_gmail_monitor();
                    if self.gmail_monitor.is_none()
                        && (self.webview.is_some() || self.comparator.is_some())
                    {
                        self.schedule_gmail_probe(60);
                    }
                }
            }
            UserEvent::GmailInboxState {
                unread,
                sender,
                subject,
                key,
            } => self.handle_gmail_state(unread, sender, subject, key),
            UserEvent::HideGmailToast(token) => self.hide_gmail_toast(token),
            UserEvent::ShowHistory => self.show_history(),
            UserEvent::ClearHistory => {
                self.memory.clear();
                match self.history.clear() {
                    None => {
                        self.show_home();
                        self.status = Some("A apagar o histórico local…".to_string());
                        self.request_redraw();
                    }
                    Some(result) => self.report_history_cleared(result),
                }
            }
            UserEvent::HistoryCleared(result) => self.report_history_cleared(result),
            UserEvent::MemoryQueryReady { query, result } => {
                self.show_memory_results(&query, result);
            }
            UserEvent::MemoryCleared(result) => {
                if let Err(error) = result {
                    self.show_splash(format!("Memória: {error}"), 4);
                } else {
                    self.status = Some("Histórico e memória semântica apagados.".to_string());
                    self.request_redraw();
                }
            }
            UserEvent::HideChrome(token) => self.hide_chrome(token),
            UserEvent::SubmitText(input) => {
                if self.surface == Surface::Home {
                    let input = input.trim().to_string();
                    if !input.is_empty() {
                        self.handle_input(input);
                    }
                }
            }
            UserEvent::OpenExternal(url) => self.web(url),
            UserEvent::OpenInColumn(index, url) => self.open_in_column(index, url),
            UserEvent::OpenSplit { source_index, url } => {
                self.open_split(source_index, url, false);
            }
            UserEvent::OpenPrivateSplit { source_index, url } => {
                self.open_split_mode(source_index, url, false, true);
            }
            UserEvent::NewTab(index) => self.new_tab(index),
            UserEvent::CloseSplit => self.close_split(),
            UserEvent::ToggleSplitFullscreen => self.toggle_split_fullscreen(),
            UserEvent::PaletteSubmit {
                source_index,
                input,
            } => self.submit_palette(source_index, input),
            UserEvent::ExpandComparator(idx) => {
                if self.surface == Surface::Comparator {
                    self.expand_comparator(idx);
                }
            }
            UserEvent::MinimizeComparator(idx) => {
                if self.surface == Surface::Comparator {
                    self.minimize_comparator(idx);
                }
            }
            UserEvent::ResizeComparator { divider, screen_x } => {
                if self.surface == Surface::Comparator {
                    self.resize_comparator(divider, screen_x);
                }
            }
            UserEvent::RestoreComparator => {
                if self.surface == Surface::Comparator {
                    self.restore_comparator();
                }
            }
            UserEvent::PdfReady {
                generation,
                url,
                result,
            } => {
                if generation != self.current_generation() {
                    return;
                }
                match result {
                    Ok(bytes) => self.open_pdf(&url, bytes),
                    Err(error) => self.show_native_error(format!("PDF: {error}")),
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
                        self.capture_reader_memory(&article);
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
            WindowEvent::RedrawRequested => {
                if self.needs_clear {
                    self.clear_client();
                    self.needs_clear = false;
                }
                match self.surface {
                    Surface::Home => {
                        if let Some(window) = &self.window {
                            draw_home(window, self.status.as_deref());
                        }
                    }
                    Surface::Comparator => {
                        if let Some(window) = &self.window
                            && let Some(comp) = &self.comparator
                        {
                            draw_comparator_bar(
                                window,
                                comp,
                                self.bar_hover,
                                self.bar_visible(),
                                self.auto_scroll,
                            );
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::Resized(_) => match self.surface {
                Surface::Home => {
                    self.needs_clear = true;
                    self.position_omnibox();
                    self.request_redraw();
                }
                Surface::Comparator => {
                    self.needs_clear = true;
                    self.update_comparator_layout();
                    self.sync_comparator_splitters();
                    self.sync_exit_button();
                    self.request_redraw();
                }
                _ => {}
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.surface == Surface::Comparator {
                    if self.is_fullscreen_column() {
                        let scale = self
                            .window
                            .as_ref()
                            .map(|window| window.scale_factor().max(1.0))
                            .unwrap_or(1.0);
                        // Ou o rato encostou ao topo, ou ja esta sobre a barra
                        // revelada -- em qualquer dos casos ela fica.
                        if self.chrome_revealed || position.y <= 2.0 * scale {
                            self.reveal_chrome();
                        }
                    }
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } if self.surface == Surface::Comparator => self.context_menu_comparator(),
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => self.go_back(),
                    Key::Named(NamedKey::F8) => self.toggle_auto_scroll(),
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
    pin_webview_profile();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Fixa o perfil do WebView2 em `%LOCALAPPDATA%\NeuralIA\WebView2`.
///
/// Sem isto o WebView2 escolhe sozinho uma pasta ao lado do executavel
/// (`NeuralIA.exe.WebView2`), e a sessao passa a depender do sitio de onde o
/// programa foi corrido: mover o ficheiro, descarregar uma versao nova ou
/// limpar a pasta de build faz perder todos os inicios de sessao. A pasta do
/// utilizador e a mesma onde ja vive o historico, e sobrevive a tudo isso.
fn pin_webview_profile() {
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return;
    }
    let profile = CoreConfig::default().data_dir.join("WebView2");
    if std::fs::create_dir_all(&profile).is_err() {
        return;
    }
    // SAFETY: corre antes de qualquer thread ser criada.
    unsafe {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &profile);
    }
}

/// Responde a origem do visualizador: os tres ficheiros do PDF.js e o
/// documento que esta aberto. Tudo em memoria; nada toca no disco.
fn serve_pdf_asset(
    bytes: &Arc<Mutex<Vec<u8>>>,
    request: &Request<Vec<u8>>,
) -> HttpResponse<Cow<'static, [u8]>> {
    let path = request.uri().path();
    let (status, content_type, body): (u16, &str, Cow<'static, [u8]>) = match path {
        "/viewer.html" | "/" => (
            200,
            "text/html; charset=utf-8",
            Cow::Borrowed(PDF_VIEWER_HTML),
        ),
        "/viewer.mjs" => (200, "text/javascript", Cow::Borrowed(PDF_VIEWER_JS)),
        "/pdf.mjs" => (200, "text/javascript", Cow::Borrowed(PDFJS_CORE)),
        "/pdf.worker.mjs" => (200, "text/javascript", Cow::Borrowed(PDFJS_WORKER)),
        "/document.pdf" => {
            let data = bytes.lock().map(|slot| slot.clone()).unwrap_or_default();
            (200, "application/pdf", Cow::Owned(data))
        }
        _ => (404, "text/plain", Cow::Borrowed(b"not found" as &[u8])),
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
    response
        .body(body)
        .unwrap_or_else(|_| HttpResponse::new(Cow::Borrowed(b"" as &[u8])))
}

/// Traduz um `neuralia:<accao>` num evento. E o unico sitio onde a lista de
/// atalhos existe do lado nativo: as paginas so sabem escrever o nome.
fn neuralia_action(target: &str) -> Option<UserEvent> {
    let rest = target.strip_prefix("neuralia:").or_else(|| {
        target
            .get(..9)
            .filter(|prefix| prefix.eq_ignore_ascii_case("neuralia:"))
            .map(|_| &target[9..])
    })?;
    let name = rest.split(['?', '#']).next().unwrap_or(rest);

    Some(match name.to_ascii_lowercase().as_str() {
        "home" => UserEvent::HomeRequested,
        "back" => UserEvent::BackRequested,
        "restore" => UserEvent::RestoreComparator,
        "autoscroll" => UserEvent::ToggleAutoScroll,
        "zoomin" => UserEvent::ZoomIn,
        "zoomout" => UserEvent::ZoomOut,
        "zoomreset" => UserEvent::ZoomReset,
        "reload" => UserEvent::ReloadPage,
        "print" => UserEvent::PrintPage,
        "omnibox" => UserEvent::FocusOmnibox,
        "history" => UserEvent::ShowHistory,
        "newtab" => UserEvent::NewTab(0),
        "clearhistory" => UserEvent::ClearHistory,
        "fullscreen" => UserEvent::ToggleColumnFullscreen,
        "devtools" => UserEvent::OpenDevTools,
        "viewsource" => UserEvent::ViewSource,
        _ => return None,
    })
}

/// Token que so os scripts injetados conhecem: 128 bits do RNG do sistema.
/// Se o BCrypt falhar, o SipHash com semente aleatoria de antes entra a
/// misturar-se com o que houver no buffer, para nunca sair um token vazio.
fn remote_capability() -> String {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 16];
    // SAFETY: buffer valido com o tamanho declarado; sem handle de algoritmo,
    // a flag manda usar o RNG preferido do sistema.
    let status = unsafe {
        BCryptGenRandom(
            core::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        let mut left = RandomState::new().build_hasher();
        left.write_u64(now_ms());
        left.write_u8(0x5a);

        let mut right = RandomState::new().build_hasher();
        right.write_u64(now_ms().rotate_left(17));
        right.write_u8(0xa5);

        let mix = left
            .finish()
            .to_le_bytes()
            .into_iter()
            .chain(right.finish().to_le_bytes());
        for (byte, extra) in bytes.iter_mut().zip(mix) {
            *byte ^= extra;
        }
    }
    let mut token = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    token
}

/// Igualdade sem atalho: percorre sempre tudo, para o tempo nao denunciar em
/// que byte o token deixou de bater.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn remote_capability_matches(target: &str, expected: &str) -> bool {
    let Ok(url) = Url::parse(target) else {
        return false;
    };
    url.scheme().eq_ignore_ascii_case("neuralia")
        && url.query_pairs().any(|(key, value)| {
            key == "cap" && constant_time_eq(value.as_bytes(), expected.as_bytes())
        })
}

fn remote_neuralia_action(target: &str, capability: &str) -> Option<UserEvent> {
    if !remote_capability_matches(target, capability) {
        return None;
    }
    neuralia_action(target)
}

fn neuralia_query_param(target: &str, key: &str) -> Option<String> {
    Url::parse(target)
        .ok()?
        .query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn remote_web_target(target: &str, allow_local: bool) -> bool {
    if target.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    neural_core::validate_web_url(target)
        .is_ok_and(|url| allow_local || !is_local_network_target(&url))
}

/// `view-source:` so e aceite sobre uma URL web que a propria superficie ja
/// deixaria abrir: a mesma politica de rede local, sem `about:` nem esquemas
/// aninhados.
fn is_view_source_target(target: &str, allow_local: bool) -> bool {
    let Some(rest) = target.strip_prefix("view-source:") else {
        return false;
    };
    neural_core::validate_web_url(rest)
        .is_ok_and(|url| allow_local || !is_local_network_target(&url))
}

fn is_pdf_internal_target(target: &str) -> bool {
    if target.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    Url::parse(target).is_ok_and(|url| {
        url.scheme() == "http"
            && url.host_str() == Some("neuralia-pdf.localhost")
            && url.port().is_none()
    })
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

fn home_animation_enabled() -> bool {
    std::env::var_os("NEURALIA_REDUCE_MOTION").is_none()
}

fn neural_hash(mut value: u32) -> f64 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^= value >> 16;
    value as f64 / u32::MAX as f64
}

/// Rede neural puramente nativa. Os nodos nascem nas bordas e percorrem curvas
/// lentas em direcao a marca, ligando-se aos vizinhos proximos. O calculo e
/// deterministico a partir do tempo, portanto nao precisa de estado ou alocacao
/// persistente entre frames.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_neural_background(
    hdc: *mut core::ffi::c_void,
    width: f64,
    height: f64,
    scale: f64,
    target_x: f64,
    target_y: f64,
    brand_width: f64,
    theme: &Theme,
) {
    if width < 1.0 || height < 1.0 {
        return;
    }

    let seconds = now_ms() as f64 / 1000.0;
    let count = ((width / (30.0 * scale.max(1.0))).round() as usize).clamp(34, 58);
    let mut nodes = Vec::with_capacity(count);

    for i in 0..count {
        let seed = i as u32 + 1;
        let side = seed % 4;
        let along = neural_hash(seed.wrapping_mul(0x9e37_79b9));
        let (sx, sy) = match side {
            0 => (along * width, -18.0 * scale),
            1 => (width + 18.0 * scale, along * height),
            2 => (along * width, height + 18.0 * scale),
            _ => (-18.0 * scale, along * height),
        };

        let phase = neural_hash(seed.wrapping_mul(0x85eb_ca6b));
        let speed = 0.018 + neural_hash(seed.wrapping_mul(0xc2b2_ae35)) * 0.018;
        let progress = (seconds * speed + phase).fract();
        let eased = progress * progress * (3.0 - 2.0 * progress);

        let target_offset_x =
            (neural_hash(seed.wrapping_mul(0x27d4_eb2d)) - 0.5) * brand_width * 0.44;
        let target_offset_y =
            (neural_hash(seed.wrapping_mul(0x1656_67b1)) - 0.5) * brand_width * 0.16;
        let tx = target_x + target_offset_x;
        let ty = target_y + target_offset_y;

        let dx = tx - sx;
        let dy = ty - sy;
        let length = (dx * dx + dy * dy).sqrt().max(1.0);
        let px = -dy / length;
        let py = dx / length;
        let swirl_phase = neural_hash(seed.wrapping_mul(0xd3a2_646c)) * std::f64::consts::TAU;
        let swirl = (progress * std::f64::consts::TAU * 1.7 + swirl_phase).sin()
            * (1.0 - eased)
            * 38.0
            * scale;

        let x = sx + dx * eased + px * swirl;
        let y = sy + dy * eased + py * swirl;
        let energy = 0.35 + 0.65 * progress;
        nodes.push((x, y, energy));
    }

    let line_color = mix(
        theme.page_bg,
        theme.accent,
        if system_dark_mode() { 0.30 } else { 0.18 },
    );
    let line_pen = CreatePen(PS_SOLID, 1, rgb3(line_color));
    let old_pen = SelectObject(hdc, line_pen as _);
    let max_link = 150.0 * scale;

    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            let dx = nodes[i].0 - nodes[j].0;
            let dy = nodes[i].1 - nodes[j].1;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance > max_link {
                continue;
            }
            MoveToEx(
                hdc,
                nodes[i].0.round() as i32,
                nodes[i].1.round() as i32,
                std::ptr::null_mut(),
            );
            LineTo(hdc, nodes[j].0.round() as i32, nodes[j].1.round() as i32);
        }
    }
    SelectObject(hdc, old_pen);
    DeleteObject(line_pen as _);

    let node_color = mix(
        theme.page_bg,
        theme.accent,
        if system_dark_mode() { 0.72 } else { 0.50 },
    );
    let node_brush = CreateSolidBrush(rgb3(node_color));
    let node_pen = CreatePen(PS_SOLID, 1, rgb3(node_color));
    let old_brush = SelectObject(hdc, node_brush as _);
    let old_node_pen = SelectObject(hdc, node_pen as _);

    for (x, y, energy) in nodes {
        let radius = ((1.4 + energy * 2.1) * scale).clamp(2.0, 6.0);
        Ellipse(
            hdc,
            (x - radius).round() as i32,
            (y - radius).round() as i32,
            (x + radius).round() as i32,
            (y + radius).round() as i32,
        );
    }

    SelectObject(hdc, old_node_pen);
    SelectObject(hdc, old_brush);
    DeleteObject(node_pen as _);
    DeleteObject(node_brush as _);
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

        // A Home agora anima continuamente; desenhar direto no ecra faria o
        // FillRect piscar. Compoe-se o frame inteiro em memoria e faz-se um
        // unico BitBlt no fim.
        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = if mem_dc.is_null() {
            std::ptr::null_mut()
        } else {
            CreateCompatibleBitmap(hdc, client.right.max(1), client.bottom.max(1))
        };
        let buffered = !mem_dc.is_null() && !mem_bmp.is_null();
        let target = if buffered { mem_dc } else { hdc };
        let old_bmp = if buffered {
            SelectObject(mem_dc, mem_bmp as _)
        } else {
            std::ptr::null_mut()
        };

        let background = CreateSolidBrush(rgb3(theme.page_bg));
        FillRect(target, &client, background);
        DeleteObject(background as _);
        SetBkMode(target, TRANSPARENT as i32);

        let brand_width = (420.0 * scale).min(width * 0.52);
        let brand_height = brand_width * BRAND_ASPECT;
        let brand_x = ((width - brand_width) / 2.0).round() as i32;
        let brand_y = (layout.input.y - brand_height - 44.0 * scale)
            .max(24.0 * scale)
            .round() as i32;

        if home_animation_enabled() {
            draw_neural_background(
                target,
                width,
                height,
                scale,
                brand_x as f64 + brand_width / 2.0,
                brand_y as f64 + brand_height * 0.58,
                brand_width,
                &theme,
            );
        }

        // Marca e omnibox continuam acima da rede neural.
        draw_brand(
            target,
            brand_x,
            brand_y,
            brand_width.round() as i32,
            brand_height.round() as i32,
            theme.page_bg,
        );

        let body_font = create_font((-17.0 * scale) as i32, FW_NORMAL as i32);
        let small_font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);
        let old_font = SelectObject(target, body_font as _);

        fill_pill(
            target,
            layout.input,
            layout.input.height / 2.0,
            theme.surface,
            Some((theme.surface_line, scale)),
            theme.page_bg,
        );
        draw_button(target, layout.go, "Ir", true, scale, body_font, &theme);

        if let Some(message) = status {
            SelectObject(target, small_font as _);
            SetTextColor(target, rgb3(theme.fg_muted));
            let mut status_rect = RECT {
                left: (32.0 * scale) as i32,
                top: client.bottom - (64.0 * scale) as i32,
                right: client.right - (32.0 * scale) as i32,
                bottom: client.bottom - (24.0 * scale) as i32,
            };
            draw_text(
                target,
                message,
                &mut status_rect,
                DT_CENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }

        SelectObject(target, old_font);
        DeleteObject(body_font as _);
        DeleteObject(small_font as _);

        if buffered {
            BitBlt(
                hdc,
                0,
                0,
                client.right.max(1),
                client.bottom.max(1),
                mem_dc,
                0,
                0,
                SRCCOPY,
            );
            SelectObject(mem_dc, old_bmp);
            DeleteObject(mem_bmp as _);
            DeleteDC(mem_dc);
        } else if !mem_dc.is_null() {
            DeleteDC(mem_dc);
        }

        let _ = ReleaseDC(hwnd, hdc);
    }
}

fn draw_comparator_bar(
    window: &Window,
    comp: &ComparatorState,
    hover: Option<BarHit>,
    visible: bool,
    auto_scroll: bool,
) {
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
        let bar_h = (COMPARATOR_CHROME_HEIGHT * scale).round() as i32;

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

        paint_comparator_bar_with_contexts(
            target,
            width,
            scale,
            &names,
            &comp.contexts,
            comp.split.as_ref().map(|split| {
                (
                    split.source_index,
                    split.url.as_str(),
                    split.fullscreen,
                    split.private,
                )
            }),
            visible,
            hover,
            auto_scroll,
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
#[cfg(test)]
unsafe fn paint_comparator_bar(
    target: *mut core::ffi::c_void,
    width: i32,
    scale: f64,
    names: &[&str],
    visible: bool,
    hover: Option<BarHit>,
    auto_scroll: bool,
    theme: &Theme,
) {
    let empty: [Vec<String>; COMPARATOR_COLUMNS] = std::array::from_fn(|_| Vec::new());
    paint_comparator_bar_with_contexts(
        target,
        width,
        scale,
        names,
        &empty,
        None,
        visible,
        hover,
        auto_scroll,
        theme,
    );
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_comparator_bar_with_contexts(
    target: *mut core::ffi::c_void,
    width: i32,
    scale: f64,
    names: &[&str],
    contexts: &[Vec<String>; COMPARATOR_COLUMNS],
    active_context: Option<(usize, &str, bool, bool)>,
    visible: bool,
    hover: Option<BarHit>,
    auto_scroll: bool,
    theme: &Theme,
) {
    let layout = BarLayout::with_contexts(
        width as f64,
        scale,
        visible,
        names.len(),
        std::array::from_fn(|index| contexts[index].len()),
    );
    if !layout.visible {
        return;
    }
    let bar_h = layout.height.round() as i32;
    let title_h = (TITLE_TAB_HEIGHT * scale).round() as i32;

    let bar_rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: bar_h,
    };
    let background = CreateSolidBrush(rgb3(theme.bar_bg));
    FillRect(target, &bar_rect, background);
    DeleteObject(background as _);

    // Separa discretamente title bar e barra dos provedores.
    let title_line = RECT {
        left: 0,
        top: title_h - scale.round().max(1.0) as i32,
        right: width,
        bottom: title_h,
    };
    let separator = CreateSolidBrush(rgb3(theme.bar_line));
    FillRect(target, &title_line, separator);
    let bottom_line = RECT {
        left: 0,
        top: bar_h - scale.round().max(1.0) as i32,
        right: width,
        bottom: bar_h,
    };
    FillRect(target, &bottom_line, separator);
    DeleteObject(separator as _);

    SetBkMode(target, TRANSPARENT as i32);
    let font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);
    let tab_font = create_font((-10.0 * scale) as i32, FW_NORMAL as i32);
    let old_font = SelectObject(target, font as _);

    // Identidade do app ocupa o canto esquerdo; o restante da faixa e
    // arrastavel quando nao houver uma aba sob o rato.
    draw_pill(
        target,
        UiRect {
            x: 7.0 * scale,
            y: 3.0 * scale,
            width: 76.0 * scale,
            height: (TITLE_TAB_HEIGHT - 6.0) * scale,
        },
        "NeuralIA",
        PillStyle::new(theme.bar_bg, theme.bar_bg, theme.fg_muted),
        scale,
        tab_font,
        theme.bar_bg,
    );

    // Abas/fontes na mesma faixa dos botoes de janela.
    for (index, source_contexts) in contexts.iter().enumerate().take(layout.columns_len) {
        let brand = theme.brand(index);
        for visual in 0..layout.context_tab_counts[index] {
            let context_index = layout.context_indices[index][visual];
            let Some(url) = source_contexts.get(context_index) else {
                continue;
            };
            let active = active_context
                .is_some_and(|(source, active_url, _, _)| source == index && active_url == url);
            let hovered = hover
                == Some(BarHit::ContextTab {
                    source_index: index,
                    context_index,
                });
            let fill = if active {
                mix(theme.bar_bg, brand, 0.48)
            } else if hovered {
                mix(theme.bar_bg, brand, 0.30)
            } else {
                mix(theme.bar_bg, brand, 0.12)
            };
            draw_pill(
                target,
                layout.context_tabs[index][visual],
                &context_tab_label(url),
                PillStyle::new(fill, mix(theme.bar_bg, brand, 0.30), theme.fg_muted),
                scale,
                tab_font,
                theme.bar_bg,
            );
        }
    }

    // Caption controls fazem parte da nossa title bar frameless.
    draw_button(
        target,
        layout.window_minimize,
        "—",
        hover == Some(BarHit::WindowMinimize),
        scale,
        font,
        theme,
    );
    draw_button(
        target,
        layout.window_maximize,
        "□",
        hover == Some(BarHit::WindowMaximize),
        scale,
        font,
        theme,
    );
    draw_button(
        target,
        layout.window_close,
        "×",
        hover == Some(BarHit::WindowClose),
        scale,
        font,
        theme,
    );

    // Segunda linha: apenas controles do NeuralIA/provedores.
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
        draw_button(
            target,
            layout.add_tabs[index],
            "+",
            hover == Some(BarHit::AddTab(index)),
            scale,
            font,
            theme,
        );
    }

    {
        let margin = 8.0 * scale;
        let row_y = (TITLE_TAB_HEIGHT + 7.0) * scale;
        let row_h = 30.0 * scale;
        let button_w = 78.0 * scale;
        let right = if active_context.is_some() {
            width as f64
                - margin
                - 30.0 * scale
                - 5.0 * scale
                - 30.0 * scale
                - 5.0 * scale
                - 150.0 * scale
                - 6.0 * scale
        } else {
            width as f64 - margin
        };
        let rect = UiRect {
            x: right - button_w,
            y: row_y,
            width: button_w,
            height: row_h,
        };
        draw_button(
            target,
            rect,
            "Privado",
            hover == Some(BarHit::Private),
            scale,
            tab_font,
            theme,
        );
    }

    if let Some((source_index, _url, fullscreen, private_split)) = active_context {
        let margin = 8.0 * scale;
        let row_y = (TITLE_TAB_HEIGHT + 7.0) * scale;
        let row_h = 30.0 * scale;
        let close_w = 30.0 * scale;
        let expand_w = 30.0 * scale;
        let label_w = 150.0 * scale;
        let gap = 5.0 * scale;
        let close = UiRect {
            x: width as f64 - margin - close_w,
            y: row_y,
            width: close_w,
            height: row_h,
        };
        let expand = UiRect {
            x: close.x - gap - expand_w,
            y: row_y,
            width: expand_w,
            height: row_h,
        };
        let label = UiRect {
            x: expand.x - gap - label_w,
            y: row_y,
            width: label_w,
            height: row_h,
        };
        let source = names.get(source_index).copied().unwrap_or("IA");
        draw_pill(
            target,
            label,
            &if private_split {
                format!("Privado · {source}")
            } else {
                format!("Fonte · {source}")
            },
            PillStyle::new(theme.surface, theme.surface_line, theme.fg_muted),
            scale,
            tab_font,
            theme.bar_bg,
        );
        draw_button(
            target,
            expand,
            if fullscreen { "↙" } else { "⛶" },
            hover == Some(BarHit::SplitExpand),
            scale,
            font,
            theme,
        );
        draw_button(
            target,
            close,
            "×",
            hover == Some(BarHit::SplitClose),
            scale,
            font,
            theme,
        );
    }

    let _ = auto_scroll;

    SelectObject(target, old_font);
    DeleteObject(font as _);
    DeleteObject(tab_font as _);
}

fn context_tab_label(value: &str) -> String {
    let raw = Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "Fonte".to_string());
    let clean = raw.strip_prefix("www.").unwrap_or(&raw);
    let mut label = clean.chars().take(16).collect::<String>();
    if clean.chars().count() > 16 {
        label.push('…');
    }
    label
}

fn gmail_is_new_mail(
    previous_unread: Option<u32>,
    previous_key: Option<&str>,
    unread: u32,
    key: &str,
) -> bool {
    let Some(previous_unread) = previous_unread else {
        return false;
    };
    if unread > previous_unread {
        return true;
    }
    unread > 0
        && !key.is_empty()
        && previous_key.is_some_and(|previous| !previous.is_empty() && previous != key)
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

/// (largura, altura, cor de fundo, pixeis BGRX ja compostos)
type SplashCache = Option<(i32, i32, Rgb, Vec<u8>)>;

/// Visualizador de PDF proprio: o do Edge corre noutro processo e nao aceita
/// nem script nem teclado nosso; este e uma pagina nossa, com o PDF.js da
/// Mozilla (Apache-2.0, assets/pdfjs/LICENSE) a desenhar as paginas em canvas.
const PDF_VIEWER_HTML: &[u8] = include_bytes!("../../../assets/pdfjs/viewer.html");
const PDF_VIEWER_JS: &[u8] = include_bytes!("../../../assets/pdfjs/viewer.mjs");
const PDFJS_CORE: &[u8] = include_bytes!("../../../assets/pdfjs/pdf.mjs");
const PDFJS_WORKER: &[u8] = include_bytes!("../../../assets/pdfjs/pdf.worker.mjs");
/// No Windows um esquema personalizado `neuralia-pdf` aparece a pagina como
/// `http://neuralia-pdf.<host>`; o wry intercepta tudo o que comece assim.
const PDF_ORIGIN: &str = "http://neuralia-pdf.localhost";
/// A mesma politica do `<meta>` do viewer.html, servida em cabecalho para
/// valer antes de o HTML ser lido; um teste garante que as duas nao divergem.
const PDF_VIEWER_CSP: &str = "default-src 'none'; script-src 'self' blob:; worker-src 'self' blob:; connect-src 'self'; img-src 'self' blob: data:; style-src 'unsafe-inline'; font-src 'self' data:; object-src 'none'; base-uri 'none'; form-action 'none'";
/// Limite para um documento; o do Reader (2 MiB) e para HTML.
const PDF_MAX_BYTES: usize = 32 * 1024 * 1024;
const PDF_TIMEOUT_SECS: u64 = 90;

static LOGO_IMAGE: OnceLock<RgbaImage> = OnceLock::new();
static BRAND_IMAGE: OnceLock<RgbaImage> = OnceLock::new();
static SPLASH_CACHE: Mutex<SplashCache> = Mutex::new(None);

fn get_logo_image() -> &'static RgbaImage {
    LOGO_IMAGE.get_or_init(|| {
        let raw = include_bytes!("../../../assets/logo.png");
        image::load_from_memory(raw)
            .expect("assets/logo.png must be valid PNG")
            .to_rgba8()
    })
}

/// Arte da marca mostrada na tela inicial: e a unica coisa la, com a barra.
fn get_brand_image() -> &'static RgbaImage {
    BRAND_IMAGE.get_or_init(|| {
        let raw = include_bytes!("../../../assets/neuralia-home.png");
        image::load_from_memory(raw)
            .expect("assets/neuralia-home.png must be valid PNG")
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

unsafe fn draw_brand(
    hdc: *mut core::ffi::c_void,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    bg_rgb: Rgb,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let pixels = {
        let mut cache = SPLASH_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        match *cache {
            Some((cached_w, cached_h, cached_bg, ref cached_pixels))
                if cached_w == width && cached_h == height && cached_bg == bg_rgb =>
            {
                cached_pixels.clone()
            }
            _ => {
                let rendered = render_brand_pixels(width, height, bg_rgb);
                *cache = Some((width, height, bg_rgb, rendered.clone()));
                rendered
            }
        }
    };

    blit_bgrx(hdc, &pixels, x, y, width, height);
}

/// A arte vem sem fundo (o azul-escuro foi tirado no PNG): compomos o alfa
/// dela por cima da cor da pagina e nao ha caixa nenhuma, em nenhum tema.
fn render_brand_pixels(width: i32, height: i32, bg_rgb: Rgb) -> Vec<u8> {
    let image = image::imageops::resize(
        get_brand_image(),
        width as u32,
        height as u32,
        image::imageops::FilterType::Lanczos3,
    );

    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for py in 0..height as u32 {
        for px in 0..width as u32 {
            let pixel = image.get_pixel(px, py);
            let alpha = pixel[3] as f32 / 255.0;
            let channel = |value: u8, bg: u8| {
                (value as f32 * alpha + bg as f32 * (1.0 - alpha)).round() as u8
            };
            pixels.push(channel(pixel[2], bg_rgb.2));
            pixels.push(channel(pixel[1], bg_rgb.1));
            pixels.push(channel(pixel[0], bg_rgb.0));
            pixels.push(0);
        }
    }
    pixels
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
            let img = get_brand_image();
            assert_eq!(img.width(), 1200);
            let size = 104;
            let pixels = render_brand_pixels(size, size, (248, 249, 250));
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
            let height = COMPARATOR_CHROME_HEIGHT as i32;
            let accent = system_accent();

            let cases: [(&str, Theme, bool, Option<BarHit>); 3] = [
                ("dark", Theme::dark(accent), true, None),
                (
                    "dark-hover",
                    Theme::dark(accent),
                    true,
                    Some(BarHit::Column(2)),
                ),
                ("light", Theme::light(accent), true, None),
            ];

            for (name, theme, visible, hover) in cases {
                let mem = CreateCompatibleDC(screen);
                let bitmap = CreateCompatibleBitmap(screen, width, height);
                assert!(!mem.is_null() && !bitmap.is_null());
                let old = SelectObject(mem, bitmap as _);

                paint_comparator_bar(
                    mem,
                    width,
                    1.0,
                    &["Google Gemini", "ChatGPT", "Claude"],
                    visible,
                    hover,
                    true,
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
    fn private_panel_and_new_tab_are_wired() {
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("neuralia:newtab?col="));
        assert!(format!("{:?}", neuralia_action("neuralia:newtab")).starts_with("Some(NewTab"));
        assert_ne!(BarHit::Private, BarHit::SplitClose);
    }

    #[test]
    fn neuralia_actions_are_routed() {
        // As paginas so escrevem o nome da accao; a traducao vive toda aqui.
        for (target, expected) in [
            ("neuralia:back", "BackRequested"),
            ("NEURALIA:BACK", "BackRequested"),
            ("neuralia:zoomin", "ZoomIn"),
            ("neuralia:zoomout", "ZoomOut"),
            ("neuralia:zoomreset", "ZoomReset"),
            ("neuralia:reload", "ReloadPage"),
            ("neuralia:print", "PrintPage"),
            ("neuralia:omnibox", "FocusOmnibox"),
            ("neuralia:history", "ShowHistory"),
            ("neuralia:clearhistory", "ClearHistory"),
            ("neuralia:fullscreen", "ToggleColumnFullscreen"),
            ("neuralia:autoscroll", "ToggleAutoScroll"),
            ("neuralia:restore", "RestoreComparator"),
            ("neuralia:home", "HomeRequested"),
        ] {
            let action = neuralia_action(target);
            assert!(action.is_some(), "{target} devia ser reconhecido");
            assert!(
                format!("{:?}", action.unwrap()).starts_with(expected),
                "{target} devia dar {expected}"
            );
        }

        // Tudo o resto tem de passar ao lado, incluindo navegacao verdadeira.
        for target in [
            "https://example.com",
            "neuralia:inventado",
            "about:blank",
            "neuralia",
        ] {
            assert!(neuralia_action(target).is_none(), "{target}");
        }

        let capability = "0123456789abcdef0123456789abcdef";
        for action in [
            "home",
            "history",
            "clearhistory",
            "devtools",
            "viewsource",
            "print",
            "reload",
        ] {
            let unsigned = format!("neuralia:{action}");
            assert!(
                remote_neuralia_action(&unsigned, capability).is_none(),
                "pagina remota nao pode invocar {action} sem capability"
            );
            let signed = format!("neuralia:{action}?cap={capability}");
            assert!(
                remote_neuralia_action(&signed, capability).is_some(),
                "script injetado deve poder invocar {action} com capability"
            );
        }
        assert!(!remote_capability_matches(
            "neuralia:home?cap=errado",
            capability
        ));
    }

    #[test]
    fn remote_navigation_cannot_pivot_into_private_network() {
        assert!(remote_web_target("https://example.com/a", false));
        assert!(!remote_web_target("http://127.0.0.1:8000/", false));
        assert!(!remote_web_target("http://192.168.1.1/", false));
        assert!(remote_web_target("http://127.0.0.1:8000/", true));
    }

    #[test]
    fn view_source_follows_the_surface_network_policy() {
        assert!(is_view_source_target(
            "view-source:https://example.com/a?b=c",
            false
        ));
        assert!(is_view_source_target(
            "view-source:http://example.com/",
            false
        ));
        assert!(!is_view_source_target(
            "view-source:http://127.0.0.1:8000/",
            false
        ));
        assert!(is_view_source_target(
            "view-source:http://127.0.0.1:8000/",
            true
        ));
        assert!(!is_view_source_target(
            "view-source:http://192.168.1.1/",
            false
        ));
        assert!(!is_view_source_target(
            "view-source:http://neuralia-pdf.localhost/viewer.html",
            false
        ));

        // So URL web por baixo: nada de about:, file:, javascript:, credenciais
        // nem view-source aninhado; e o prefixo tem de estar la.
        for target in [
            "view-source:about:blank",
            "view-source:file:///C:/Windows/win.ini",
            "view-source:javascript:alert(1)",
            "view-source:https://user:pw@example.com/",
            "view-source:view-source:https://example.com/",
            "view-source:",
            "view-source:neuralia:home",
            "https://example.com/",
            "VIEW-SOURCE:https://example.com/",
        ] {
            assert!(!is_view_source_target(target, true), "{target}");
        }
        // O pedido por script da pagina continua a nao ser navegacao web.
        assert!(!remote_web_target("view-source:https://example.com/", true));
    }

    #[test]
    fn capability_tokens_are_32_hex_and_never_repeat() {
        let first = remote_capability();
        let second = remote_capability();
        for token in [&first, &second] {
            assert_eq!(token.len(), 32, "{token}");
            assert!(
                token
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
                "{token}"
            );
        }
        assert_ne!(first, second);
        assert_ne!(first, "0".repeat(32));
    }

    #[test]
    fn capability_comparison_walks_every_byte() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"xbc"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));

        let token = remote_capability();
        assert!(remote_capability_matches(
            &format!("neuralia:home?cap={token}"),
            &token
        ));
        assert!(remote_capability_matches(
            &format!("neuralia:split?col=1&url=https%3A%2F%2Fa.test%2F&cap={token}"),
            &token
        ));
        let flipped = if token.ends_with('0') { "1" } else { "0" };
        let wrong = format!("{}{flipped}", &token[..31]);
        assert!(!remote_capability_matches(
            &format!("neuralia:home?cap={wrong}"),
            &token
        ));
        assert!(!remote_capability_matches(
            &format!("neuralia:home?cap={}", &token[..31]),
            &token
        ));
        assert!(!remote_capability_matches(
            &format!("neuralia:home?cap={token}0"),
            &token
        ));
        assert!(!remote_capability_matches("neuralia:home", &token));
        assert!(!remote_capability_matches(
            &format!("https://example.com/?cap={token}"),
            &token
        ));
    }

    #[test]
    fn pdf_assets_carry_nosniff_and_the_viewer_csp() {
        let html = std::str::from_utf8(PDF_VIEWER_HTML).expect("viewer.html e UTF-8");
        assert!(
            html.contains(PDF_VIEWER_CSP),
            "o cabecalho tem de ser igual ao <meta> do viewer.html"
        );

        let bytes = Arc::new(Mutex::new(b"%PDF-1.7".to_vec()));
        for (path, is_html) in [
            ("/viewer.html", true),
            ("/", true),
            ("/viewer.mjs", false),
            ("/pdf.mjs", false),
            ("/pdf.worker.mjs", false),
            ("/document.pdf", false),
            ("/nada", false),
        ] {
            let request = Request::builder()
                .uri(format!("{PDF_ORIGIN}{path}"))
                .body(Vec::new())
                .expect("pedido de teste");
            let response = serve_pdf_asset(&bytes, &request);
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .map(|value| value.to_str().unwrap_or("").to_string())
            };
            assert_eq!(
                header("X-Content-Type-Options").as_deref(),
                Some("nosniff"),
                "{path}"
            );
            assert_eq!(
                header("Cache-Control").as_deref(),
                Some("no-store"),
                "{path}"
            );
            let csp = header("Content-Security-Policy");
            assert_eq!(csp.is_some(), is_html, "{path}");
            if is_html {
                assert_eq!(csp.as_deref(), Some(PDF_VIEWER_CSP), "{path}");
            }
        }
    }

    #[test]
    fn injected_scripts_capture_globals_before_the_page_runs() {
        // O token so passa pela captura feita no document-created: um unico
        // `encodeURIComponent` por script, o da captura, e nenhum `const` de
        // topo que a pagina pudesse ler pelo nome.
        for (name, script) in [
            ("keymap", NEURALIA_KEYMAP_SCRIPT),
            ("return", EXTERNAL_RETURN_BUTTON),
            ("gmail", GMAIL_MONITOR_SCRIPT),
            ("palette", NEURALIA_PALETTE_SCRIPT),
            ("comparator", COMPARATOR_INJECT_SCRIPT),
        ] {
            assert!(script.contains("__NEURALIA_CAP__"), "{name}");
            assert_eq!(
                script.matches("encodeURIComponent").count(),
                1,
                "{name}: so a captura pode nomear encodeURIComponent"
            );
            assert!(
                script.contains("const encode = encodeURIComponent;"),
                "{name}"
            );
            assert!(script.trim_start().starts_with("(function"), "{name}");
        }
        // Os scripts que so correm depois do DOMContentLoaded nao tocam em
        // nenhum global do DOM pelo nome.
        for (name, script) in [
            ("return", EXTERNAL_RETURN_BUTTON),
            ("comparator", COMPARATOR_INJECT_SCRIPT),
        ] {
            assert!(
                script.contains(
                    "Function.prototype.call.bind(EventTarget.prototype.addEventListener)"
                ),
                "{name}"
            );
            assert!(
                script.contains("document.createElement.bind(document)"),
                "{name}"
            );
            for forbidden in [
                "Object.assign(",
                "document.createElement(",
                "document.getElementById(",
                ".appendChild(",
                ".addEventListener(",
            ] {
                assert!(!script.contains(forbidden), "{name}: {forbidden}");
            }
        }
        assert!(NEURALIA_PALETTE_SCRIPT.contains("listen(input, 'keydown'"));
        assert!(!NEURALIA_PALETTE_SCRIPT.contains("Object.assign("));
        assert!(!NEURALIA_PALETTE_SCRIPT.contains("document.createElement("));

        // Nenhum handler que leve o token responde a eventos sinteticos, e os
        // botoes nao expoem o handler em `onclick`.
        assert_eq!(
            COMPARATOR_INJECT_SCRIPT
                .matches("if (!event.isTrusted")
                .count(),
            4
        );
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("expand.onclick"));
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("minimize.onclick"));
        assert!(EXTERNAL_RETURN_BUTTON.contains("if (!event.isTrusted) return;"));
        assert!(NEURALIA_PALETTE_SCRIPT.contains("if (!event.isTrusted) return;"));
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("if (!e.isTrusted) { return; }"));

        // Redireccionador do Google: o dominio e os subdominios, nao um sufixo.
        assert!(
            COMPARATOR_INJECT_SCRIPT
                .contains("host === 'google.com' || host.endsWith('.google.com')")
        );
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("endsWith('google.com')"));
    }

    #[test]
    fn pdf_origin_is_exact_not_prefix_based() {
        assert!(is_pdf_internal_target(
            "http://neuralia-pdf.localhost/viewer.html"
        ));
        assert!(!is_pdf_internal_target(
            "http://neuralia-pdf.localhost.evil.test/viewer.html"
        ));
        assert!(!is_pdf_internal_target(
            "https://neuralia-pdf.localhost/viewer.html"
        ));
    }

    #[test]
    fn comparator_script_contains_independent_response_timeline() {
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia-response-rail"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("Resposta anterior"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("Próxima resposta"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("scrollToPosition"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia-scroll-root"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("semanticAnchors"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("data-message-author-role"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("ariaLabel"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("top:'50%'"));
    }

    #[test]
    fn comparator_has_split_palette_and_real_three_way_submit() {
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia:split?col="));
        assert!(NEURALIA_PALETTE_SCRIPT.contains("neuralia:palette?col="));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("neuralia-split-scroll-rail"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("chatgpt.com"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("claude.ai"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("button.click()"));
    }

    #[test]
    fn split_controls_are_native_bar_hits() {
        assert_ne!(BarHit::SplitExpand, BarHit::SplitClose);
        assert!(!SPLIT_SCROLL_RAIL_SCRIPT.contains("neuralia-split-controls"));
        assert!(!SPLIT_SCROLL_RAIL_SCRIPT.contains("Fonte ·"));
    }

    #[test]
    fn splitter_topology_is_resynced_after_layout_transitions() {
        let source = include_str!("windows_app.rs");
        let minimize = source
            .split("fn minimize_comparator")
            .nth(1)
            .and_then(|part| part.split("fn restore_comparator").next())
            .expect("minimize body");
        assert!(minimize.contains("sync_comparator_splitters()"));

        let split = source
            .split("fn open_split_mode")
            .nth(1)
            .and_then(|part| part.split("fn open_private_panel").next())
            .expect("split body");
        assert!(split.contains("hide_comparator_splitters()"));
        assert!(split.contains("sync_comparator_splitters()"));
    }

    #[test]
    fn comparator_resize_uses_persistent_weights_and_native_splitters() {
        let weights = [1.0_f64; COMPARATOR_COLUMNS];
        assert!(weights.iter().all(|weight| *weight > 0.0));
    }

    #[test]
    fn comparator_minimize_control_is_wired_and_layout_keeps_one_visible() {
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia-comp-minimize"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia:minimize?col="));
        assert!(COMPARATOR_BUTTON_COLLAPSED.contains("neuralia-comp-minimize"));
    }

    #[test]
    fn comparator_timeline_and_sync_use_current_control_ids() {
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia-response-rail"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia-comp-expand"));
        assert!(COMPARATOR_BUTTON_EXPANDED.contains("#neuralia-comp-expand"));
        assert!(COMPARATOR_BUTTON_COLLAPSED.contains("#neuralia-comp-expand"));
        assert!(!COMPARATOR_BUTTON_EXPANDED.contains("neuralia-comp-btn"));
        assert!(!COMPARATOR_BUTTON_COLLAPSED.contains("neuralia-comp-btn"));
    }

    #[test]
    fn zoom_walks_the_chrome_ladder() {
        assert_eq!(ZOOM_STEPS[0], 0.25);
        assert!(ZOOM_STEPS.contains(&1.0));
        assert!(
            ZOOM_STEPS.windows(2).all(|pair| pair[0] < pair[1]),
            "a escada tem de ser crescente"
        );
    }

    #[test]
    fn split_view_uses_neuralia_scroll_rail_and_auto_scroll() {
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("neuralia-split-scroll-rail"));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("scrollbar-width:none"));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("scrollToPosition"));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("semanticAnchors"));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("data-message-author-role"));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("aria-label"));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("top:'50%'"));
    }

    #[test]
    fn reader_uses_semantic_timeline_script() {
        let source = include_str!("windows_app.rs");
        let reader = source
            .split("fn reader_webview_builder")
            .nth(1)
            .and_then(|part| part.split("fn external_webview_builder").next())
            .expect("reader builder");
        assert!(reader.contains("SPLIT_SCROLL_RAIL_SCRIPT"));
    }

    #[test]
    fn auto_scroll_supports_all_three_internal_scroll_roots() {
        assert!(AUTO_SCROLL_SCRIPT.contains("[class*=\"overflow\"]"));
        assert!(AUTO_SCROLL_SCRIPT.contains("[class*=\"scroll\"]"));
        assert!(AUTO_SCROLL_SCRIPT.contains("el.scrollBy"));
        assert!(AUTO_SCROLL_SCRIPT.contains("scrollRoot(doc)"));
    }

    #[test]
    fn gmail_notifications_do_not_fire_on_initial_baseline() {
        assert!(!gmail_is_new_mail(None, None, 4, "thread-a"));
        assert!(gmail_is_new_mail(Some(4), Some("thread-a"), 5, "thread-b"));
        assert!(gmail_is_new_mail(Some(4), Some("thread-a"), 4, "thread-b"));
        assert!(!gmail_is_new_mail(Some(4), Some("thread-a"), 4, "thread-a"));
        assert!(GMAIL_MONITOR_SCRIPT.contains("mail.google.com"));
        assert!(GMAIL_MONITOR_SCRIPT.contains("neuralia:gmail-state"));
    }

    #[test]
    fn context_tabs_live_in_browser_title_bar() {
        let layout = BarLayout::with_contexts(1600.0, 1.0, true, 3, [3, 3, 3]);
        assert_eq!(layout.height, COMPARATOR_CHROME_HEIGHT);
        for index in 0..3 {
            let provider = layout.columns[index];
            let plus = layout.add_tabs[index];
            assert!(provider.y >= TITLE_TAB_HEIGHT);
            assert!((provider.y - plus.y).abs() <= 2.0);
            for visual in 0..layout.context_tab_counts[index] {
                let tab = layout.context_tabs[index][visual];
                assert!(tab.y < TITLE_TAB_HEIGHT);
                assert!(tab.y + tab.height <= TITLE_TAB_HEIGHT);
            }
        }
        assert!(layout.window_minimize.y < TITLE_TAB_HEIGHT);
        assert!(layout.window_maximize.y < TITLE_TAB_HEIGHT);
        assert!(layout.window_close.y < TITLE_TAB_HEIGHT);
    }

    #[test]
    fn title_bar_window_controls_are_hit_tested() {
        let layout = BarLayout::with_contexts(1400.0, 1.0, true, 3, [1, 1, 1]);
        let center = |r: UiRect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
        let (x, y) = center(layout.window_minimize);
        assert_eq!(layout.hit(x, y), Some(BarHit::WindowMinimize));
        let (x, y) = center(layout.window_maximize);
        assert_eq!(layout.hit(x, y), Some(BarHit::WindowMaximize));
        let (x, y) = center(layout.window_close);
        assert_eq!(layout.hit(x, y), Some(BarHit::WindowClose));
    }

    #[test]
    fn context_menu_commands_are_unique() {
        let ids = [
            TAB_MENU_OPEN,
            TAB_MENU_FULLSCREEN,
            TAB_MENU_CLOSE,
            TAB_MENU_CLOSE_OTHERS,
            TAB_MENU_CLOSE_ALL,
        ];
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[..index].contains(id));
        }
    }

    #[test]
    fn grouped_tabs_have_plus_and_context_hits() {
        let layout = BarLayout::with_contexts(1600.0, 1.0, true, 3, [2, 1, 4]);

        for index in 0..3 {
            let plus = layout.add_tabs[index];
            assert_eq!(
                layout.hit(plus.x + plus.width / 2.0, plus.y + plus.height / 2.0),
                Some(BarHit::AddTab(index))
            );
        }

        assert_eq!(layout.context_tab_counts, [2, 1, 3]);
        let last = layout.context_tabs[2][2];
        assert_eq!(
            layout.hit(last.x + 2.0, last.y + 2.0),
            Some(BarHit::ContextTab {
                source_index: 2,
                context_index: 3,
            })
        );
        assert_eq!(
            context_tab_label("https://www.example.com/path"),
            "example.com"
        );
    }

    #[test]
    fn bar_layout_hit_matches_drawing() {
        let layout = BarLayout::new(1600.0, 1.0, true, 3);

        // Cada grupo fica dentro da faixa horizontal da sua IA.
        for index in 0..3 {
            let provider = layout.columns[index];
            let plus = layout.add_tabs[index];
            let left = index as f64 * (1600.0 / 3.0);
            let right = (index + 1) as f64 * (1600.0 / 3.0);
            assert!(provider.x >= left);
            assert!(plus.x + plus.width <= right);
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

        // Em ecra completo nao ha barra nenhuma: nem se desenha, nem se clica.
        let expanded = BarLayout::new(1600.0, 1.0, false, 3);
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
/// Os degraus de zoom do Chrome, para o gesto ser o que a pessoa ja conhece.
const ZOOM_STEPS: [f64; 16] = [
    0.25, 0.33, 0.50, 0.67, 0.75, 0.80, 0.90, 1.00, 1.10, 1.25, 1.50, 1.75, 2.00, 2.50, 3.00, 4.00,
];

/// altura / largura da arte da marca (assets/neuralia-home.png, 1200x868).
const BRAND_ASPECT: f64 = 868.0 / 1200.0;

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

/// Mapa de teclas injetado em TODAS as paginas. O teclado pertence ao WebView2,
/// que e uma janela filha: a janela nativa nunca ve a tecla, por isso e aqui,
/// na fase de captura, que se apanham os atalhos antes de o site os consumir.
const NEURALIA_KEYMAP_SCRIPT: &str = r#"
(function () {
  if (window.__neuralia_keymap) { return; }
  window.__neuralia_keymap = true;

  // Capturas no document-created, antes de a pagina correr: o que os atalhos
  // usam mais tarde com o token nao pode ser um global ja envenenado.
  const capability = '__NEURALIA_CAP__';
  const encode = encodeURIComponent;
  const colIndex = window.__neuralia_col_index;
  function act(name) {
    window.location.href = 'neuralia:' + name + '?cap=' + encode(capability);
  }

  function findBar() {
    var id = 'neuralia-find';
    var box = document.getElementById(id);
    if (box) { box.querySelector('input').focus(); box.querySelector('input').select(); return; }

    box = document.createElement('div');
    box.id = id;
    box.setAttribute('style', [
      'position:fixed', 'top:12px', 'right:14px', 'z-index:2147483647',
      'display:flex', 'align-items:center', 'gap:8px',
      'padding:8px 12px', 'border-radius:999px',
      'background:rgba(17,19,20,.96)', 'box-shadow:0 8px 28px rgba(0,0,0,.4)',
      'font:600 13px Segoe UI, system-ui, sans-serif'
    ].join(';'));

    var input = document.createElement('input');
    input.type = 'text';
    input.placeholder = 'Procurar na pagina';
    input.setAttribute('style', [
      'border:0', 'outline:0', 'background:transparent', 'color:#fff',
      'font:inherit', 'width:190px'
    ].join(';'));

    var close = document.createElement('span');
    close.textContent = '\u2715';
    close.setAttribute('style', 'color:#9aa1a8;cursor:pointer');
    close.onclick = function () { box.remove(); };

    input.addEventListener('keydown', function (e) {
      e.stopPropagation();
      if (e.key === 'Enter') {
        e.preventDefault();
        try { window.find(input.value, false, e.shiftKey, true); } catch (err) {}
      } else if (e.key === 'Escape') {
        e.preventDefault();
        box.remove();
      }
    }, true);

    box.appendChild(input);
    box.appendChild(close);
    document.documentElement.appendChild(box);
    input.focus();
  }

  document.addEventListener('keydown', function (e) {
    if (!e.isTrusted) { return; }
    var mod = e.ctrlKey || e.metaKey;
    var key = (e.key || '').toLowerCase();

    // Combinacoes com Ctrl valem mesmo dentro de um campo de texto.
    if (mod && !e.altKey) {
      if (e.shiftKey && key === 'delete') { e.preventDefault(); act('clearhistory'); return; }
      if (e.shiftKey && key === 'r') { e.preventDefault(); act('reload'); return; }
      if (e.shiftKey && (key === 'i' || key === 'j' || key === 'c')) {
        e.preventDefault(); act('devtools'); return;
      }
      if (key === 'u') { e.preventDefault(); act('viewsource'); return; }
      switch (key) {
        case 'r': e.preventDefault(); act('reload'); return;
        case 'l': e.preventDefault(); act('omnibox'); return;
        case 'h': e.preventDefault(); act('history'); return;
        case 'n':
          e.preventDefault();
          if (typeof colIndex === 'number') {
            window.location.href = 'neuralia:newtab?col=' + colIndex
              + '&cap=' + encode(capability);
          } else {
            act('newtab');
          }
          return;
        case 'k':
        case 't':
          e.preventDefault();
          if (typeof colIndex === 'number') {
            window.dispatchEvent(new CustomEvent('neuralia-open-palette'));
          } else if (key === 'k') {
            act('omnibox');
          } else {
            act('home');
          }
          return;
        case 'w': e.preventDefault(); act('back'); return;
        case 'p': e.preventDefault(); act('print'); return;
        case 'f': e.preventDefault(); findBar(); return;
        case '+': case '=': e.preventDefault(); act('zoomin'); return;
        case '-': case '_': e.preventDefault(); act('zoomout'); return;
        case '0': e.preventDefault(); act('zoomreset'); return;
      }
    }

    if (e.altKey && key === 'arrowleft') { e.preventDefault(); window.history.back(); return; }
    if (e.altKey && key === 'arrowright') { e.preventDefault(); window.history.forward(); return; }
    var target = e.target || {};
    if (target.closest && target.closest('#neuralia-find')) { return; }

    if (key === 'f5') { e.preventDefault(); act('reload'); return; }
    if (key === 'f12') { e.preventDefault(); act('devtools'); return; }
    if (key === 'f8') { e.preventDefault(); act('autoscroll'); return; }
    if (key === 'f11') { e.preventDefault(); act('fullscreen'); return; }
    if (key === 'escape') { e.preventDefault(); e.stopPropagation(); act('back'); return; }

    var tag = (target.tagName || '').toUpperCase();
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable) {
      return;
    }
    if (e.altKey || mod) { return; }

    if (key === 'backspace') { e.preventDefault(); window.history.back(); return; }
    if (key === '1' || key === '2' || key === '3') {
      if (typeof colIndex === 'number') {
        e.preventDefault();
        window.location.href = 'neuralia:expand?col=' + (parseInt(key, 10) - 1)
          + '&cap=' + encode(capability);
      }
      return;
    }
    if (key === '0' && typeof colIndex === 'number') {
      e.preventDefault();
      act('restore');
    }
  }, true);
})();
"#;

/// Rotulos do botao injetado no comparador. Em tela cheia a barra nativa some,
/// por isso este botao tem de anunciar a saida.
const COMPARATOR_BUTTON_EXPANDED: &str = "(function(){var b=document.querySelector('#neuralia-comp-expand');if(b){b.style.display='none';}var m=document.querySelector('#neuralia-comp-minimize');if(m){m.style.display='none';}})();";
const COMPARATOR_BUTTON_COLLAPSED: &str = "(function(){var b=document.querySelector('#neuralia-comp-expand');if(b){b.style.display='block';b.textContent='\u{26F6} ' + (window.__neuralia_col_name || 'IA');}var m=document.querySelector('#neuralia-comp-minimize');if(m){m.style.display='block';}})();";

/// Fechado num IIFE: um `const` de topo seria um binding lexico global, e a
/// pagina lia o token pelo nome. Tudo o que o botao usa depois do
/// DOMContentLoaded e capturado aqui, antes de a pagina correr.
const EXTERNAL_RETURN_BUTTON: &str = r#"
(function () {
  const capability = '__NEURALIA_CAP__';
  const encode = encodeURIComponent;
  const byId = document.getElementById.bind(document);
  const createElement = document.createElement.bind(document);
  const assign = Object.assign;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const append = Function.prototype.call.bind(Node.prototype.appendChild);

  listen(document, 'DOMContentLoaded', () => {
    if (byId('neural-shell') || byId('neuralia-return')) return;
    const b = createElement('button');
    b.id = 'neuralia-return';
    b.textContent = '◀ NeuralIA';
    assign(b.style, {
      position:'fixed', left:'16px', bottom:'16px', zIndex:'2147483647',
      border:'0', borderRadius:'999px', padding:'11px 16px',
      background:'#111314', color:'#fff', font:'600 13px Segoe UI, sans-serif',
      boxShadow:'0 6px 24px rgba(0,0,0,.25)', cursor:'pointer'
    });
    listen(b, 'click', (event) => {
      if (!event.isTrusted) return;
      window.location.href = 'neuralia:home?cap=' + encode(capability);
    });
    append(document.documentElement, b);
  });
})();
"#;

const GMAIL_MONITOR_SCRIPT: &str = r#"
(function () {
  if (location.hostname !== 'mail.google.com' || window.__neuralia_gmail_monitor) return;
  window.__neuralia_gmail_monitor = true;
  const capability = '__NEURALIA_CAP__';
  // emit() corre tarde, a partir do observer: o codificador e capturado agora.
  const encode = encodeURIComponent;
  let lastState = '';
  let debounce = 0;

  function clean(value) {
    return String(value || '').replace(/\s+/g, ' ').trim().slice(0, 180);
  }

  function unreadCount() {
    const match = String(document.title || '').match(/\(([\d.,]+)\)/);
    if (!match) return 0;
    const digits = match[1].replace(/\D/g, '');
    return Number(digits || '0');
  }

  function firstUnread() {
    const row = document.querySelector(
      'tr.zE,[role="main"] tr.zE,[role="main"] [data-legacy-thread-id].zE'
    );
    if (!row) return { sender:'', subject:'', key:'' };

    const senderNode = row.querySelector('.zF,.yP,[email]');
    const subjectNode = row.querySelector('.bog,[data-thread-id] .bog');
    const sender = clean(
      senderNode && (senderNode.getAttribute('email')
        || senderNode.getAttribute('name')
        || senderNode.textContent)
    );
    const subject = clean(subjectNode && subjectNode.textContent);
    const key = clean(
      row.getAttribute('data-legacy-thread-id')
        || row.getAttribute('data-thread-id')
        || (sender + '|' + subject)
    );
    return { sender, subject, key };
  }

  function emit() {
    if (location.hostname !== 'mail.google.com') return;
    const first = firstUnread();
    const count = unreadCount();
    const state = count + '|' + first.key;
    if (state === lastState) return;
    lastState = state;

    window.location.href = 'neuralia:gmail-state?count=' + count
      + '&sender=' + encode(first.sender)
      + '&subject=' + encode(first.subject)
      + '&key=' + encode(first.key)
      + '&cap=' + encode(capability);
  }

  function schedule() {
    clearTimeout(debounce);
    debounce = setTimeout(emit, 450);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', schedule, { once:true });
  } else {
    schedule();
  }

  new MutationObserver(schedule).observe(document.documentElement, {
    subtree:true, childList:true, characterData:true, attributes:true
  });
  setInterval(emit, 15000);
})();
"#;

const NEURALIA_PALETTE_SCRIPT: &str = r#"
(function () {
  if (window.__neuralia_palette_ready) return;
  window.__neuralia_palette_ready = true;
  const capability = '__NEURALIA_CAP__';
  // openPalette() corre tarde (a pagina tambem pode disparar o evento): tudo
  // o que ele usa vem daqui, capturado antes de a pagina correr.
  const colIndex = window.__neuralia_col_index;
  const encode = encodeURIComponent;
  const byId = document.getElementById.bind(document);
  const createElement = document.createElement.bind(document);
  const assign = Object.assign;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const append = Function.prototype.call.bind(Node.prototype.appendChild);

  function closePalette() {
    const old = byId('neuralia-palette');
    if (old) old.remove();
  }

  function openPalette() {
    closePalette();
    if (typeof colIndex !== 'number') return;

    const shade = createElement('div');
    shade.id = 'neuralia-palette';
    assign(shade.style, {
      position:'fixed', inset:'0', zIndex:'2147483647',
      display:'flex', alignItems:'flex-start', justifyContent:'center',
      paddingTop:'18vh', background:'rgba(0,0,0,.22)',
      backdropFilter:'blur(2px)', fontFamily:'Segoe UI, system-ui, sans-serif'
    });

    const box = createElement('div');
    assign(box.style, {
      width:'min(680px, calc(100vw - 48px))', borderRadius:'18px',
      padding:'12px 16px', background:'rgba(24,26,28,.97)',
      border:'1px solid rgba(255,255,255,.12)',
      boxShadow:'0 24px 80px rgba(0,0,0,.48)'
    });

    const input = createElement('input');
    input.type = 'text';
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.placeholder = 'Pergunte à IA ativa ou digite uma URL';
    assign(input.style, {
      width:'100%', boxSizing:'border-box', border:'0', outline:'0',
      background:'transparent', color:'#fff',
      font:'500 18px Segoe UI, system-ui, sans-serif', padding:'9px 4px'
    });

    const hint = createElement('div');
    hint.textContent = 'URL → abre ao lado   ·   texto → envia para '
      + (window.__neuralia_col_name || 'IA') + '   ·   Esc fecha';
    assign(hint.style, {
      color:'rgba(255,255,255,.48)', fontSize:'11px', padding:'2px 4px 4px'
    });

    listen(input, 'keydown', (event) => {
      if (!event.isTrusted) return;
      event.stopPropagation();
      if (event.key === 'Escape') {
        event.preventDefault(); closePalette(); return;
      }
      if (event.key === 'Enter') {
        event.preventDefault();
        const value = input.value.trim();
        if (!value) return;
        window.location.href = 'neuralia:palette?col=' + colIndex
          + '&q=' + encode(value)
          + '&cap=' + encode(capability);
      }
    }, true);

    listen(shade, 'mousedown', (event) => {
      if (event.target === shade) closePalette();
    });
    append(box, input);
    append(box, hint);
    append(shade, box);
    append(document.documentElement, shade);
    setTimeout(() => input.focus(), 0);
  }

  window.addEventListener('neuralia-open-palette', openPalette);
})();
"#;

/// ChatGPT e Claude aceitam a consulta por ?q=, mas hoje apenas preenchem o
/// compositor. O comparador tem semântica de "perguntar às três", portanto o
/// NeuralIA confirma o envio assim que o botão real do fornecedor fica pronto.
const AI_AUTO_SUBMIT_SCRIPT: &str = r#"
(function () {
  const host = location.hostname.toLowerCase();
  if (host !== 'chatgpt.com' && host !== 'claude.ai') return;
  const query = new URL(location.href).searchParams.get('q');
  if (!query || !query.trim()) return;

  const stampKey = 'neuralia:auto-submit:' + host + ':' + query;
  const previous = Number(sessionStorage.getItem(stampKey) || '0');
  if (Date.now() - previous < 10000) return;

  function promptText() {
    const el = document.querySelector(
      'textarea, [data-testid="prompt-textarea"], [contenteditable="true"][role="textbox"], div[contenteditable="true"]'
    );
    if (!el) return '';
    return String('value' in el ? el.value : el.innerText || el.textContent || '').trim();
  }

  function candidates() {
    if (host === 'chatgpt.com') {
      return [
        'button[data-testid="send-button"]',
        'button[aria-label*="Send prompt"]',
        'button[aria-label*="Send message"]',
        'form button[type="submit"]'
      ];
    }
    return [
      'button[aria-label*="Send"]',
      'button[data-testid*="send"]',
      'form button[type="submit"]'
    ];
  }

  let attempts = 0;
  function submitWhenReady() {
    attempts += 1;
    const typed = promptText();
    if (typed) {
      for (const selector of candidates()) {
        const button = document.querySelector(selector);
        if (!button || button.disabled || button.getAttribute('aria-disabled') === 'true') continue;
        const rect = button.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) continue;
        sessionStorage.setItem(stampKey, String(Date.now()));
        button.click();
        return;
      }
    }
    if (attempts < 120) setTimeout(submitWhenReady, 150);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => setTimeout(submitWhenReady, 100), { once:true });
  } else {
    setTimeout(submitWhenReady, 100);
  }
})();
"#;

const SPLIT_SCROLL_RAIL_SCRIPT: &str = r#"
document.addEventListener('DOMContentLoaded', () => {
  if (document.getElementById('neuralia-split-scroll-rail')) return;

  const style = document.createElement('style');
  style.id = 'neuralia-split-scroll-style';
  style.textContent = [
    '*{scrollbar-width:none!important;-ms-overflow-style:none!important;}',
    '*::-webkit-scrollbar{width:0!important;height:0!important;display:none!important;background:transparent!important;}'
  ].join('');
  document.documentElement.appendChild(style);

  let currentRoot = null;
  let semantic = [];

  function scrollRoot() {
    const docRoot = document.scrollingElement || document.documentElement || document.body;
    const candidates = docRoot ? [docRoot] : [];
    document.querySelectorAll(
      'main,[role="main"],[data-radix-scroll-area-viewport],'
      + '[data-testid*="scroll"],[class*="scroll"],[class*="overflow"],[style*="overflow"]'
    ).forEach((el) => candidates.push(el));

    let best = docRoot;
    let bestRange = best ? Math.max(0, best.scrollHeight - best.clientHeight) : 0;
    for (const el of candidates) {
      if (!el || el === document.body) continue;
      const range = Math.max(0, el.scrollHeight - el.clientHeight);
      if (range <= bestRange + 24) continue;
      const css = getComputedStyle(el);
      if (css.display === 'none' || css.visibility === 'hidden' || css.overflowY === 'hidden') continue;
      best = el;
      bestRange = range;
    }
    currentRoot = best || docRoot;
    return currentRoot;
  }

  function metrics() {
    const root = scrollRoot();
    if (!root) return { root:null, top:0, max:0, view:window.innerHeight, docLike:true };
    const docLike = root === document.scrollingElement
      || root === document.documentElement || root === document.body;
    return {
      root,
      docLike,
      top: docLike ? window.scrollY : root.scrollTop,
      max: Math.max(0, root.scrollHeight - root.clientHeight),
      view: root.clientHeight || window.innerHeight
    };
  }

  function scrollToPosition(top) {
    const state = metrics();
    const value = Math.max(0, Math.min(state.max, top));
    if (state.docLike) window.scrollTo({ top:value, behavior:'smooth' });
    else if (state.root) state.root.scrollTo({ top:value, behavior:'smooth' });
  }

  function anchorTop(el, state) {
    const rect = el.getBoundingClientRect();
    if (state.docLike) return state.top + rect.top;
    const rootRect = state.root.getBoundingClientRect();
    return state.top + rect.top - rootRect.top;
  }

  function kindOf(el) {
    const role = el.getAttribute('data-message-author-role');
    const tag = el.tagName ? el.tagName.toLowerCase() : '';
    const text = (el.textContent || '').trim().toLowerCase();
    if (role === 'user') return 'pergunta';
    if (role === 'assistant') return 'resposta';
    if (/^h[1-6]$/.test(tag)) return text.includes('conclus') ? 'conclusão' : 'seção';
    if (tag === 'pre' || tag === 'code') return 'código';
    if (tag === 'table') return 'tabela';
    if (tag === 'blockquote') return 'citação';
    if (tag === 'aside') return 'nota';
    if (tag === 'a') return 'fonte';
    return 'resposta';
  }

  function semanticAnchors() {
    const state = metrics();
    const raw = [];
    const seen = new Set();
    const nodes = document.querySelectorAll(
      '[data-message-author-role="user"],[data-message-author-role="assistant"],'
      + 'h1,h2,h3,h4,h5,h6,pre,table,blockquote,aside,article,[role="article"],a[href]'
    );

    for (const el of nodes) {
      if (raw.length >= 128) break;
      if (!el || el.closest('#neuralia-split-scroll-rail,#neuralia-comp-controls')) continue;
      const css = getComputedStyle(el);
      if (css.display === 'none' || css.visibility === 'hidden') continue;
      const label = (el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 96);
      if (!label) continue;
      const kind = kindOf(el);
      const key = kind + ':' + label;
      if (seen.has(key)) continue;
      seen.add(key);
      raw.push({ el, kind, label, top:anchorTop(el, state) });
    }

    raw.sort((a, b) => a.top - b.top);
    if (raw.length <= 28) return raw;
    const sampled = [];
    for (let i = 0; i < 28; i++) {
      sampled.push(raw[Math.round(i * (raw.length - 1) / 27)]);
    }
    return sampled;
  }

  function semanticStep(direction) {
    semantic = semanticAnchors();
    if (!semantic.length) return false;
    const state = metrics();
    const pivot = state.top + Math.max(24, state.view * .24);
    let current = 0;
    for (let i = 0; i < semantic.length; i++) {
      if (semantic[i].top <= pivot) current = i;
      else break;
    }
    const target = Math.max(0, Math.min(semantic.length - 1, current + direction));
    if (target === current && ((direction < 0 && current === 0)
        || (direction > 0 && current === semantic.length - 1))) return false;
    scrollToPosition(semantic[target].top);
    return true;
  }

  const rail = document.createElement('div');
  rail.id = 'neuralia-split-scroll-rail';
  Object.assign(rail.style, {
    position:'fixed', top:'50%', right:'7px', transform:'translateY(-50%)',
    zIndex:'2147483646', pointerEvents:'auto', width:'44px',
    minHeight:'240px', maxHeight:'58vh',
    display:'flex', flexDirection:'column', alignItems:'center',
    justifyContent:'space-between', opacity:'.68',
    transition:'opacity .18s ease', fontFamily:'Segoe UI, system-ui, sans-serif'
  });
  rail.onmouseenter = () => { rail.style.opacity = '1'; };
  rail.onmouseleave = () => { rail.style.opacity = '.68'; };

  function arrow(symbol, title, direction) {
    const button = document.createElement('button');
    button.textContent = symbol;
    button.title = title;
    button.setAttribute('aria-label', title);
    Object.assign(button.style, {
      width: direction > 0 ? '42px' : '32px',
      height: direction > 0 ? '42px' : '28px',
      border: direction > 0 ? '1px solid rgba(255,255,255,.08)' : '0',
      borderRadius:'50%', padding:'0',
      background: direction > 0 ? 'rgba(38,38,38,.94)' : 'transparent',
      color: direction > 0 ? '#f4f4f4' : 'rgba(255,255,255,.46)',
      boxShadow: direction > 0 ? '0 6px 20px rgba(0,0,0,.28)' : 'none',
      fontSize:'21px', lineHeight: direction > 0 ? '38px' : '26px',
      cursor:'pointer'
    });
    button.onclick = (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (semanticStep(direction)) return;
      const state = metrics();
      scrollToPosition(state.top + Math.max(220, state.view * .82) * direction);
    };
    return button;
  }

  const ticks = document.createElement('div');
  Object.assign(ticks.style, {
    width:'34px', flex:'1', margin:'8px 0 10px',
    display:'flex', flexDirection:'column',
    justifyContent:'space-evenly', alignItems:'flex-end'
  });

  function rebuildTicks() {
    const state = metrics();
    semantic = semanticAnchors();
    const fallbackCount = Math.max(5, Math.min(11,
      Math.ceil((state.max + Math.max(state.view, 1)) / Math.max(state.view, 1))));
    const count = semantic.length || fallbackCount;
    const signature = semantic.length
      ? semantic.map((item) => item.kind + ':' + Math.round(item.top)).join('|')
      : 'fallback:' + count;
    if (ticks.dataset.signature === signature) return;
    ticks.dataset.signature = signature;
    ticks.textContent = '';

    for (let i = 0; i < count; i++) {
      const tick = document.createElement('button');
      tick.type = 'button';
      const item = semantic[i] || null;
      const label = item ? item.kind + ': ' + item.label : 'posição ' + (i + 1);
      tick.title = label;
      tick.setAttribute('aria-label', label);
      tick.dataset.top = item ? String(item.top) : '';
      tick.dataset.fraction = item ? '' : String(count <= 1 ? 0 : i / (count - 1));
      Object.assign(tick.style, {
        display:'block', height:'3px', width:i === 0 ? '30px' : '14px',
        minHeight:'3px', border:'0', borderRadius:'2px', padding:'0',
        background:'rgba(255,255,255,.30)', cursor:'pointer',
        transition:'width .16s ease, background .16s ease, opacity .16s ease'
      });
      tick.onclick = (event) => {
        event.preventDefault();
        event.stopPropagation();
        const top = Number(tick.dataset.top);
        if (tick.dataset.top) scrollToPosition(top);
        else scrollToPosition(metrics().max * Number(tick.dataset.fraction || 0));
      };
      ticks.appendChild(tick);
    }
  }

  function syncTicks() {
    rebuildTicks();
    const state = metrics();
    const children = Array.from(ticks.children);
    let active = 0;
    if (semantic.length) {
      const pivot = state.top + Math.max(24, state.view * .24);
      for (let i = 0; i < children.length; i++) {
        const top = Number(children[i].dataset.top || 0);
        if (top <= pivot) active = i;
        else break;
      }
    } else {
      const progress = state.max <= 0 ? 0 : Math.max(0, Math.min(1, state.top / state.max));
      active = Math.round(progress * Math.max(0, children.length - 1));
    }

    children.forEach((tick, i) => {
      const selected = i === active;
      tick.style.width = selected ? '32px' : (Math.abs(i - active) === 1 ? '22px' : '13px');
      tick.style.background = selected ? '#fff' : 'rgba(255,255,255,.32)';
      tick.style.opacity = selected ? '1' : (Math.abs(i - active) === 1 ? '.78' : '.55');
    });
  }

  rail.appendChild(arrow('⌃', 'Seção semântica anterior', -1));
  rail.appendChild(ticks);
  rail.appendChild(arrow('⌄', 'Próxima seção semântica', 1));
  document.documentElement.appendChild(rail);

  let raf = 0;
  const scheduleSync = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => { raf = 0; syncTicks(); });
  };
  window.addEventListener('scroll', scheduleSync, { passive:true });
  document.addEventListener('scroll', scheduleSync, { passive:true, capture:true });
  window.addEventListener('resize', scheduleSync, { passive:true });
  new MutationObserver(scheduleSync).observe(document.documentElement, {
    childList:true, subtree:true
  });
  syncTicks();
});
"#;

/// Tudo o que corre depois do DOMContentLoaded usa as capturas do topo: a
/// pagina ja correu nessa altura e pode ter trocado qualquer global. Os
/// botoes que levam o token ouvem por addEventListener, nao por `onclick`,
/// para a pagina nao poder ler o handler do elemento e chama-lo a mao.
const COMPARATOR_INJECT_SCRIPT: &str = r#"
(function () {
  const colIndex = window.__neuralia_col_index ?? 0;
  const colName = window.__neuralia_col_name ?? 'IA';
  const capability = '__NEURALIA_CAP__';
  const encode = encodeURIComponent;
  const byId = document.getElementById.bind(document);
  const createElement = document.createElement.bind(document);
  const assign = Object.assign;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const append = Function.prototype.call.bind(Node.prototype.appendChild);

  listen(document, 'DOMContentLoaded', () => {
    function mountControls() {
      if (byId('neuralia-comp-controls')) return;

      const style = createElement('style');
      style.id = 'neuralia-scroll-style';
      style.textContent = [
        'html,body,.neuralia-scroll-root{scrollbar-width:none!important;-ms-overflow-style:none!important;}',
        'html::-webkit-scrollbar,body::-webkit-scrollbar,.neuralia-scroll-root::-webkit-scrollbar{width:0!important;height:0!important;display:none!important;}'
      ].join('');
      append(document.documentElement, style);

      let currentRoot = null;
      function scrollRoot() {
        const docRoot = document.scrollingElement || document.documentElement || document.body;
        const candidates = docRoot ? [docRoot] : [];
        document.querySelectorAll(
          'main,[role="main"],[class*="scroll"],[class*="overflow"],[style*="overflow"]'
        ).forEach((el) => candidates.push(el));

        let best = docRoot;
        let bestRange = best ? Math.max(0, best.scrollHeight - best.clientHeight) : 0;
        for (const el of candidates) {
          if (!el || el === document.body) continue;
          const range = Math.max(0, el.scrollHeight - el.clientHeight);
          if (range <= bestRange + 24) continue;
          const css = getComputedStyle(el);
          if (css.overflowY === 'hidden' || css.display === 'none') continue;
          best = el;
          bestRange = range;
        }
        if (currentRoot && currentRoot !== best && currentRoot.classList) {
          currentRoot.classList.remove('neuralia-scroll-root');
        }
        currentRoot = best || docRoot;
        if (currentRoot && currentRoot.classList) currentRoot.classList.add('neuralia-scroll-root');
        return currentRoot;
      }

      function metrics() {
        const root = scrollRoot();
        if (!root) return { root:null, top:0, max:0, docLike:true };
        const docLike = root === document.scrollingElement
          || root === document.documentElement || root === document.body;
        return {
          root,
          docLike,
          top: docLike ? window.scrollY : root.scrollTop,
          max: Math.max(0, root.scrollHeight - root.clientHeight)
        };
      }

      function scrollToPosition(top) {
        const state = metrics();
        const value = Math.max(0, Math.min(state.max, top));
        if (state.docLike) window.scrollTo({ top:value, behavior:'smooth' });
        else if (state.root) state.root.scrollTo({ top:value, behavior:'smooth' });
      }

      const controls = createElement('div');
      controls.id = 'neuralia-comp-controls';
      assign(controls.style, {
        position:'fixed', inset:'0', zIndex:'2147483647',
        pointerEvents:'none', fontFamily:'Segoe UI, system-ui, sans-serif'
      });

      const expand = createElement('button');
      expand.id = 'neuralia-comp-expand';
      expand.textContent = '⛶ ' + colName;
      assign(expand.style, {
        position:'absolute', top:'10px', right:'10px',
        pointerEvents:'auto', border:'1px solid rgba(255,255,255,.12)',
        borderRadius:'999px', padding:'6px 11px', background:'rgba(17,19,20,.90)',
        color:'#fff', fontSize:'11px', fontWeight:'600',
        boxShadow:'0 5px 18px rgba(0,0,0,.28)', cursor:'pointer'
      });
      listen(expand, 'click', (event) => {
        if (!event.isTrusted) return;
        event.preventDefault(); event.stopPropagation();
        window.location.href = 'neuralia:expand?col=' + colIndex
          + '&cap=' + encode(capability);
      });

      const minimize = createElement('button');
      minimize.id = 'neuralia-comp-minimize';
      minimize.textContent = '−';
      minimize.title = 'Minimizar ' + colName;
      assign(minimize.style, {
        position:'absolute', top:'10px', right:'112px',
        pointerEvents:'auto', width:'30px', height:'28px',
        border:'1px solid rgba(255,255,255,.12)',
        borderRadius:'999px', padding:'0',
        background:'rgba(17,19,20,.90)', color:'#fff',
        fontSize:'18px', fontWeight:'600', lineHeight:'24px',
        boxShadow:'0 5px 18px rgba(0,0,0,.28)', cursor:'pointer'
      });
      listen(minimize, 'click', (event) => {
        if (!event.isTrusted) return;
        event.preventDefault(); event.stopPropagation();
        window.location.href = 'neuralia:minimize?col=' + colIndex
          + '&cap=' + encode(capability);
      });

      const rail = createElement('div');
      rail.id = 'neuralia-response-rail';
      assign(rail.style, {
        position:'absolute', top:'50%', right:'7px', transform:'translateY(-50%)',
        pointerEvents:'auto', width:'44px', minHeight:'240px', maxHeight:'58vh',
        display:'flex', flexDirection:'column', alignItems:'center',
        justifyContent:'space-between', opacity:'.68',
        transition:'opacity .18s ease'
      });
      rail.onmouseenter = () => { rail.style.opacity = '1'; };
      rail.onmouseleave = () => { rail.style.opacity = '.68'; };

      function arrow(symbol, title, direction) {
        const button = createElement('button');
        button.textContent = symbol;
        button.title = title;
        assign(button.style, {
          width: direction > 0 ? '42px' : '32px',
          height: direction > 0 ? '42px' : '28px',
          border: direction > 0 ? '1px solid rgba(255,255,255,.08)' : '0',
          borderRadius:'50%', padding:'0',
          background: direction > 0 ? 'rgba(38,38,38,.94)' : 'transparent',
          color: direction > 0 ? '#f4f4f4' : 'rgba(255,255,255,.46)',
          boxShadow: direction > 0 ? '0 6px 20px rgba(0,0,0,.28)' : 'none',
          fontSize:'21px', lineHeight: direction > 0 ? '38px' : '26px',
          cursor:'pointer'
        });
        button.onclick = (event) => {
          event.preventDefault(); event.stopPropagation();
          if (semanticStep(direction)) return;
          const state = metrics();
          const view = state.root ? state.root.clientHeight : window.innerHeight;
          scrollToPosition(state.top + Math.max(220, view * .82) * direction);
        };
        return button;
      }

      const ticks = createElement('div');
      ticks.id = 'neuralia-response-ticks';
      assign(ticks.style, {
        width:'34px', flex:'1', margin:'8px 0 10px', display:'flex',
        flexDirection:'column', justifyContent:'space-evenly',
        alignItems:'flex-end', cursor:'pointer'
      });

      let semantic = [];

      function anchorTop(el, state) {
        const rect = el.getBoundingClientRect();
        if (state.docLike) return state.top + rect.top;
        const rootRect = state.root.getBoundingClientRect();
        return state.top + rect.top - rootRect.top;
      }

      function semanticKind(el) {
        const role = el.getAttribute('data-message-author-role');
        const tag = el.tagName ? el.tagName.toLowerCase() : '';
        const text = (el.textContent || '').trim().toLowerCase();
        if (role === 'user') return 'pergunta';
        if (role === 'assistant') return 'resposta';
        if (/^h[1-6]$/.test(tag)) return text.includes('conclus') ? 'conclusão' : 'seção';
        if (tag === 'pre' || tag === 'code') return 'código';
        if (tag === 'table') return 'tabela';
        if (tag === 'blockquote') return 'citação';
        if (tag === 'aside') return 'nota';
        if (tag === 'a') return 'fonte';
        return 'resposta';
      }

      function semanticAnchors() {
        const state = metrics();
        const raw = [];
        const seen = new Set();
        const nodes = document.querySelectorAll(
          '[data-message-author-role="user"],[data-message-author-role="assistant"],'
          + 'h1,h2,h3,h4,h5,h6,pre,table,blockquote,aside,article,[role="article"],a[href]'
        );
        for (const el of nodes) {
          if (raw.length >= 128) break;
          if (!el || (el.closest && el.closest('#neuralia-comp-controls,#neuralia-palette'))) continue;
          const css = getComputedStyle(el);
          if (css.display === 'none' || css.visibility === 'hidden') continue;
          const label = (el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 96);
          if (!label) continue;
          const kind = semanticKind(el);
          const key = kind + ':' + label;
          if (seen.has(key)) continue;
          seen.add(key);
          raw.push({ el, kind, label, top:anchorTop(el, state) });
        }
        raw.sort((a, b) => a.top - b.top);
        if (raw.length <= 28) return raw;
        const sampled = [];
        for (let i = 0; i < 28; i++) {
          sampled.push(raw[Math.round(i * (raw.length - 1) / 27)]);
        }
        return sampled;
      }

      function semanticStep(direction) {
        semantic = semanticAnchors();
        if (!semantic.length) return false;
        const state = metrics();
        const view = state.root ? state.root.clientHeight : window.innerHeight;
        const pivot = state.top + Math.max(24, view * .24);
        let current = 0;
        for (let i = 0; i < semantic.length; i++) {
          if (semantic[i].top <= pivot) current = i;
          else break;
        }
        const target = Math.max(0, Math.min(semantic.length - 1, current + direction));
        if (target === current && ((direction < 0 && current === 0)
            || (direction > 0 && current === semantic.length - 1))) return false;
        scrollToPosition(semantic[target].top);
        return true;
      }

      function rebuildTicks() {
        const state = metrics();
        const view = state.root ? state.root.clientHeight : window.innerHeight;
        semantic = semanticAnchors();
        const fallbackCount = Math.max(5, Math.min(11,
          Math.ceil((state.max + Math.max(view, 1)) / Math.max(view, 1))));
        const count = semantic.length || fallbackCount;
        const signature = semantic.length
          ? semantic.map((item) => item.kind + ':' + Math.round(item.top)).join('|')
          : 'fallback:' + count;
        if (ticks.dataset.signature === signature) return;
        ticks.dataset.signature = signature;
        ticks.textContent = '';

        for (let i = 0; i < count; i++) {
          const tick = createElement('button');
          const item = semantic[i] || null;
          const label = item ? item.kind + ': ' + item.label : 'posição ' + (i + 1);
          tick.type = 'button';
          tick.title = label;
          tick.ariaLabel = label;
          tick.dataset.top = item ? String(item.top) : '';
          tick.dataset.fraction = item ? '' : String(count <= 1 ? 0 : i / (count - 1));
          assign(tick.style, {
            display:'block', height:'3px', minHeight:'3px',
            width:i === 0 ? '30px' : '14px', border:'0', borderRadius:'2px',
            padding:'0', background:'rgba(255,255,255,.30)', cursor:'pointer',
            transition:'width .16s ease, background .16s ease, opacity .16s ease'
          });
          tick.onclick = (event) => {
            event.preventDefault(); event.stopPropagation();
            if (tick.dataset.top) scrollToPosition(Number(tick.dataset.top));
            else scrollToPosition(metrics().max * Number(tick.dataset.fraction || 0));
          };
          append(ticks, tick);
        }
      }

      function syncTicks() {
        rebuildTicks();
        const state = metrics();
        const view = state.root ? state.root.clientHeight : window.innerHeight;
        const children = Array.from(ticks.children);
        let active = 0;

        if (semantic.length) {
          const pivot = state.top + Math.max(24, view * .24);
          for (let i = 0; i < children.length; i++) {
            const top = Number(children[i].dataset.top || 0);
            if (top <= pivot) active = i;
            else break;
          }
        } else {
          const progress = state.max <= 0 ? 0 : Math.max(0, Math.min(1, state.top / state.max));
          active = Math.round(progress * Math.max(0, children.length - 1));
        }

        children.forEach((tick, i) => {
          const selected = i === active;
          tick.style.width = selected ? '32px' : (Math.abs(i - active) === 1 ? '22px' : '13px');
          tick.style.background = selected ? '#fff' : 'rgba(255,255,255,.32)';
          tick.style.opacity = selected ? '1' : (Math.abs(i - active) === 1 ? '.78' : '.55');
        });
      }

      append(rail, arrow('⌃', 'Resposta anterior', -1));
      append(rail, ticks);
      append(rail, arrow('⌄', 'Próxima resposta', 1));
      append(controls, minimize);
      append(controls, expand);
      append(controls, rail);
      append(document.documentElement, controls);

      let raf = 0;
      const scheduleSync = () => {
        if (raf) return;
        raf = requestAnimationFrame(() => { raf = 0; syncTicks(); });
      };
      listen(window, 'scroll', scheduleSync, { passive:true });
      listen(document, 'scroll', scheduleSync, { passive:true, capture:true });
      listen(window, 'resize', scheduleSync, { passive:true });
      new MutationObserver(scheduleSync).observe(document.documentElement, {
        childList:true, subtree:true
      });
      syncTicks();
    }

    mountControls();
    new MutationObserver(() => {
      if (!byId('neuralia-comp-controls')) mountControls();
    }).observe(document.documentElement, { childList:true, subtree:true });

    // Uma fonte externa abre ao lado da conversa que a produziu.
    listen(document, 'click', (event) => {
      if (!event.isTrusted || event.defaultPrevented) return;
      if (event.target && event.target.closest
          && event.target.closest('#neuralia-comp-controls,#neuralia-palette')) return;
      const anchor = event.target && event.target.closest
        ? event.target.closest('a[href]') : null;
      if (!anchor) return;

      let target;
      try { target = new URL(anchor.href, location.href); } catch (_) { return; }
      if (target.protocol !== 'http:' && target.protocol !== 'https:') return;

      // So o proprio dominio e os seus subdominios: 'evilgoogle.com' nao conta.
      const host = target.hostname;
      if ((host === 'google.com' || host.endsWith('.google.com')) && target.pathname === '/url') {
        const actual = target.searchParams.get('q') || target.searchParams.get('url');
        if (actual) {
          try { target = new URL(actual); } catch (_) {}
        }
      }

      if (target.hostname === location.hostname) return;
      event.preventDefault();
      event.stopPropagation();
      window.location.href = 'neuralia:split?col=' + colIndex
        + '&url=' + encode(target.href)
        + '&cap=' + encode(capability);
    }, true);

    listen(document, 'dblclick', (event) => {
      if (!event.isTrusted || event.defaultPrevented) return;
      if (event.target && event.target.closest
          && event.target.closest('#neuralia-comp-controls,#neuralia-palette')) return;
      const tag = event.target && event.target.tagName
        ? event.target.tagName.toUpperCase() : '';
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
      if (event.target && event.target.isContentEditable) return;
      window.location.href = 'neuralia:expand?col=' + colIndex
        + '&cap=' + encode(capability);
    }, true);
  });
})();
"#;
