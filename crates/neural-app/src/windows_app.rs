#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    borrow::Cow,
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use image::RgbaImage;

use neural_core::{
    CoreConfig, HistoryEntry, HistoryKind, HistoryStore, Intent, ReaderArticle, ReaderClient,
    chatgpt_search_url, claude_search_url, google_ai_url, is_local_network_target, is_pdf_url,
    parse_intent, reader_html,
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CLEARTYPE_QUALITY,
        ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
        CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS,
        DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteDC,
        DeleteObject, DrawTextW, EndPaint, FW_BOLD, FW_NORMAL, FillRect, GetDC, InvalidateRect,
        OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject, SetBkColor, SetBkMode,
        SetTextColor, SetWindowRgn, StretchDIBits, TRANSPARENT,
    },
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, SendInput, SetFocus,
            VK_CONTROL, VK_NEXT, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, ES_AUTOHSCROLL, GetClientRect, GetForegroundWindow,
            GetWindowTextLengthW, GetWindowTextW, MB_ICONINFORMATION, MB_OK, MessageBoxW, SW_HIDE,
            SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos, SetWindowTextW,
            ShowWindow, WM_KEYDOWN, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            WS_POPUP, WS_TABSTOP, WS_VISIBLE,
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
    ShowHistory,
    ClearHistory,
    HistoryCleared(Result<(), String>),
    /// Esconde outra vez a barra em ecra completo, se nada a tiver reavivado.
    HideChrome(u64),
    SubmitText(String),
    OpenExternal(String),
    /// Popup pedido por uma coluna do comparador: carrega nessa coluna.
    OpenInColumn(usize, String),
    ExpandComparator(usize),
    RestoreComparator,
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

const TOP_BAR_HEIGHT: f64 = 42.0;
const COMPARATOR_COLUMNS: usize = 3;
/// Intervalo da rolagem automatica de leitura, do primeiro avanco ao ultimo.
const AUTO_SCROLL_SECONDS: u64 = 30;
/// Quanto tempo a pergunta fica no ecra antes de se dar por respondida com
/// "nao". Sem resposta nao se mexe em nada: e uma pergunta, nao um aviso.
const AUTO_SCROLL_PROMPT_SECONDS: u64 = 20;

const SPLASH_SUBCLASS_ID: usize = 0x4E4C;
const SPLASH_WIDTH: f64 = 470.0;
const SPLASH_HEIGHT: f64 = 46.0;

/// Texto do aviso flutuante. Vive fora do App porque quem o pinta e o
/// procedimento de janela, que nao tem acesso ao estado da aplicacao.
static SPLASH_TEXT: Mutex<String> = Mutex::new(String::new());
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

  function step(win) {
    try {
      var doc = win.document;
      var el = doc.scrollingElement || doc.documentElement || doc.body;
      if (!el) { return false; }
      var view = el.clientHeight || win.innerHeight || 0;
      if (view <= 0) { return false; }
      if (el.scrollHeight - el.clientHeight <= 4) { return false; }
      if (el.scrollTop + el.clientHeight >= el.scrollHeight - 2) { return false; }
      win.scrollBy({ top: Math.max(view - 72, 120), left: 0, behavior: 'smooth' });
      return true;
    } catch (err) {
      return false;
    }
  }

  if (step(window)) { return; }

  // Alguns leitores desenham o conteudo dentro de um frame proprio.
  var frames = document.querySelectorAll('iframe, frame');
  for (var i = 0; i < frames.length; i++) {
    try {
      if (frames[i].contentWindow && step(frames[i].contentWindow)) { return; }
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
}

impl BarLayout {
    /// `visible` falso devolve uma barra escondida: em ecra completo a coluna
    /// fica com o monitor inteiro, sem faixa nativa por cima do site.
    fn new(client_width: f64, scale: f64, visible: bool, columns: usize) -> Self {
        let scale = scale.max(1.0);
        if !visible {
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

/// A Home abre em repouso. `NEURALIA_STARTUP_INPUT` existe apenas para testes,
/// demos e automacao explícita; producao nunca envia uma consulta sem acao do
/// utilizador.
const DEFAULT_STARTUP_INPUT: &str = "";

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
            .name("neural-document".into())
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
            return Err("a thread de documentos nao pôde ser criada".to_string());
        }
        let (lock, wake) = &*self.pending;
        let mut slot = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // So interessa a navegacao mais recente. A ativa e cancelada pelo
        // generation watch; a pendente anterior e substituida sem criar thread.
        *slot = Some(job);
        wake.notify_one();
        Ok(())
    }
}

const HISTORY_QUEUE_LIMIT: usize = 64;

enum HistoryCommand {
    Append(HistoryEntry),
    Clear,
}

#[derive(Clone)]
struct HistoryWriter {
    pending: Arc<(Mutex<VecDeque<HistoryCommand>>, Condvar)>,
    alive: bool,
}

impl HistoryWriter {
    fn new(store: HistoryStore, proxy: EventLoopProxy<UserEvent>) -> Self {
        let pending = Arc::new((
            Mutex::new(VecDeque::<HistoryCommand>::with_capacity(
                HISTORY_QUEUE_LIMIT,
            )),
            Condvar::new(),
        ));
        let worker_pending = Arc::clone(&pending);
        let spawned = thread::Builder::new()
            .name("neural-history".into())
            .spawn(move || {
                loop {
                    let command = {
                        let (lock, wake) = &*worker_pending;
                        let mut queue =
                            lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        while queue.is_empty() {
                            queue = wake
                                .wait(queue)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                        queue.pop_front().expect("history command present")
                    };

                    match command {
                        HistoryCommand::Append(entry) => {
                            let _ = store.append(&entry);
                        }
                        HistoryCommand::Clear => {
                            let result = store.clear().map_err(|error| error.to_string());
                            let _ = proxy.send_event(UserEvent::HistoryCleared(result));
                        }
                    }
                }
            });

        Self {
            pending,
            alive: spawned.is_ok(),
        }
    }

    fn append(&self, entry: HistoryEntry) {
        if !self.alive {
            return;
        }
        let (lock, wake) = &*self.pending;
        let mut queue = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        if queue.len() >= HISTORY_QUEUE_LIMIT {
            // Historico e bounded: sob rajada descartamos a entrada Append mais
            // antiga ainda nao persistida, nunca um Clear, e jamais fazemos
            // fsync no event loop.
            if let Some(index) = queue
                .iter()
                .position(|command| matches!(command, HistoryCommand::Append(_)))
            {
                queue.remove(index);
            } else {
                return;
            }
        }

        queue.push_back(HistoryCommand::Append(entry));
        wake.notify_one();
    }

    /// `None` = pedido entregue ao worker, resposta chega em HistoryCleared.
    /// `Some` = o worker nao existe e nada e fingido como apagado.
    fn clear(&self) -> Option<Result<(), String>> {
        if !self.alive {
            return Some(Err("worker de historico indisponivel".to_string()));
        }
        let (lock, wake) = &*self.pending;
        let mut queue = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.clear();
        queue.push_back(HistoryCommand::Clear);
        wake.notify_one();
        None
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
    auto_scroll: bool,
    auto_scroll_answered: bool,
    auto_scroll_token: u64,
    zoom: f64,
    reading_pdf: bool,
    splash: Option<HWND>,
    splash_token: u64,
    splash_question_token: Option<u64>,
    /// Um unico timer cooperativo atende todos os avisos, em vez de uma thread
    /// adormecida por splash.
    splash_deadline: Arc<AtomicU64>,
    splash_watch_token: Arc<AtomicU64>,
    /// O Win32 nao apaga o fundo por nos e uma janela filha destruida deixa os
    /// ultimos pixeis onde estava. Sem isto viam-se barras e texto fantasma.
    needs_clear: bool,
    omnibox_font: Option<*mut core::ffi::c_void>,
    omnibox_font_height: i32,
    omnibox_proxy: Box<EventLoopProxy<UserEvent>>,
    config: CoreConfig,
    history_store: HistoryStore,
    history: HistoryWriter,
    reader: ReaderWorker,
    document: DocumentWorker,
    /// Entrega one-shot ao protocolo interno. Depois que o WebView recebe o
    /// PDF, a copia Rust e liberada em vez de ficar duplicada em RAM.
    pdf_bytes: Arc<Mutex<Option<Vec<u8>>>>,
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
        let document = DocumentWorker::new(
            ReaderClient::new(PDF_TIMEOUT_SECS, PDF_MAX_BYTES),
            proxy.clone(),
            Arc::clone(&navigation_generation),
        );
        let omnibox_proxy = Box::new(proxy.clone());

        let splash_deadline = Arc::new(AtomicU64::new(0));
        let splash_watch_token = Arc::new(AtomicU64::new(0));
        {
            let deadline = Arc::clone(&splash_deadline);
            let watch_token = Arc::clone(&splash_watch_token);
            let splash_proxy = proxy.clone();
            let _ = thread::Builder::new()
                .name("neural-splash-timer".into())
                .spawn(move || {
                    loop {
                        let target = deadline.load(Ordering::SeqCst);
                        if target == 0 {
                            thread::sleep(Duration::from_millis(200));
                            continue;
                        }
                        let now = now_ms();
                        if now >= target {
                            if deadline
                                .compare_exchange(target, 0, Ordering::SeqCst, Ordering::SeqCst)
                                .is_ok()
                            {
                                let token = watch_token.load(Ordering::SeqCst);
                                let _ = splash_proxy.send_event(UserEvent::HideSplash(token));
                            }
                            continue;
                        }
                        thread::sleep(Duration::from_millis((target - now).min(250)));
                    }
                });
        }

        Self {
            document,
            pdf_bytes: Arc::new(Mutex::new(None)),
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
            // Ligada por omissao: a aplicacao serve para ler.
            // Nada rola sem o utilizador dizer que sim.
            auto_scroll: false,
            auto_scroll_answered: false,
            auto_scroll_token: 0,
            zoom: 1.0,
            reading_pdf: false,
            splash: None,
            splash_token: 0,
            splash_question_token: None,
            splash_deadline,
            splash_watch_token,
            needs_clear: true,
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
        if let Some(button) = self.exit_button.take() {
            unsafe {
                DestroyWindow(button);
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
            *bytes = None;
        }
        self.emit_lifecycle_probe();
    }

    fn emit_lifecycle_probe(&self) {
        let Some(path) = std::env::var_os("NEURALIA_LIFECYCLE_PROBE") else {
            return;
        };
        let count = self.webview.is_some() as usize
            + self
                .comparator
                .as_ref()
                .map(|state| state.views.len())
                .unwrap_or(0);
        let _ = std::fs::write(std::path::PathBuf::from(path), count.to_string());
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
            Ok(valid) if is_pdf_url(&valid) => self.read_pdf(valid),
            Ok(valid) => {
                self.next_generation();
                self.record(HistoryKind::Web, valid.to_string(), valid.to_string());
                self.open_external(valid.as_str());
            }
            Err(error) => self.show_native_error(error.to_string()),
        }
    }

    /// Descarrega o PDF numa thread, com o filtro de rede do Reader, e abre-o
    /// no visualizador nosso quando chegar. Cancela-se como o Reader: se a
    /// navegacao mudar entretanto, o resultado e descartado.
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
            self.show_native_error(format!("PDF indisponível: {error}"));
        }
    }

    fn pdf_webview_builder(&self) -> WebViewBuilder<'static> {
        let proxy = self.proxy.clone();
        let bytes = Arc::clone(&self.pdf_bytes);
        let token = action_token();
        let navigation_token = token.clone();

        WebViewBuilder::new()
            .with_custom_protocol("neuralia-pdf".to_string(), move |_id, request| {
                serve_pdf_asset(&bytes, &request)
            })
            .with_initialization_script(script_with_token(NEURALIA_KEYMAP_SCRIPT, &token))
            .with_navigation_handler(move |target| {
                if let Some(event) = neuralia_action(&target, &navigation_token) {
                    let _ = proxy.send_event(event);
                    return false;
                }
                if target == "about:blank" || is_pdf_internal_url(&target) {
                    return true;
                }
                // PDF e conteudo nao confiavel: um link nele nao ganha direito
                // de pivotar para localhost/RFC1918.
                if let Ok(url) = neural_core::validate_web_url(&target)
                    && !is_local_network_target(&url)
                {
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
            *slot = Some(bytes);
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
                self.begin_reading_session(true);
                self.emit_lifecycle_probe();
            }
            Err(error) => {
                if let Ok(mut slot) = self.pdf_bytes.lock() {
                    *slot = None;
                }
                self.show_native_error(format!("WebView2 não pôde abrir o PDF: {error}"));
            }
        }
    }

    fn record(&self, kind: HistoryKind, input: String, target: String) {
        self.history.append(HistoryEntry::now(kind, input, target));
    }

    fn reader_webview_builder(&self) -> WebViewBuilder<'static> {
        let proxy = self.proxy.clone();
        WebViewBuilder::new()
            .with_navigation_handler(move |target| {
                if target == "about:blank" {
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
                            && let Ok(url) = neural_core::validate_web_url(value.as_ref())
                            && !is_local_network_target(&url)
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

    fn external_webview_builder(&self, initial_url: &Url) -> WebViewBuilder<'static> {
        let navigation_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let allow_local = is_local_network_target(initial_url);
        let token = action_token();
        let navigation_token = token.clone();

        WebViewBuilder::new()
            .with_initialization_script(format!(
                "{}\n{}",
                script_with_token(NEURALIA_KEYMAP_SCRIPT, &token),
                script_with_token(EXTERNAL_RETURN_BUTTON, &token)
            ))
            .with_navigation_handler(move |target| {
                if let Some(event) = neuralia_action(&target, &navigation_token) {
                    let _ = navigation_proxy.send_event(event);
                    return false;
                }
                if target == "about:blank" {
                    return true;
                }
                let Ok(url) = neural_core::validate_web_url(&target) else {
                    return false;
                };
                allow_local || !is_local_network_target(&url)
            })
            .with_new_window_req_handler(move |target, _features| {
                if let Ok(url) = neural_core::validate_web_url(&target)
                    && (allow_local || !is_local_network_target(&url))
                {
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

        let Ok(initial_url) = neural_core::validate_web_url(url) else {
            self.show_native_error("URL externa inválida.");
            return;
        };

        let result = if let Some(window) = &self.window {
            self.external_webview_builder(&initial_url)
                .with_url(initial_url.as_str())
                .build(window)
        } else {
            return;
        };

        match result {
            Ok(webview) => {
                let _ = webview.zoom(self.zoom);
                self.webview = Some(webview);
                self.surface = Surface::External;
                self.begin_reading_session(is_pdf);
                self.emit_lifecycle_probe();
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
                self.emit_lifecycle_probe();
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
        });
        self.bar_hover = None;
        self.surface = Surface::Comparator;
        self.begin_reading_session(false);
        self.emit_lifecycle_probe();
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

        let content_h = (logical_h - TOP_BAR_HEIGHT).max(100.0);
        let content_y = TOP_BAR_HEIGHT;

        match comp.expanded {
            Some(idx) => {
                // Ecra completo. A coluna ocupa tudo menos uma faixa de 1px no
                // topo: o WebView e uma janela filha e engole o rato, por isso
                // sem essa faixa a aplicacao nunca saberia que o rato subiu ao
                // topo para chamar a barra de volta.
                let (top, height) = if self.chrome_revealed {
                    (TOP_BAR_HEIGHT, (logical_h - TOP_BAR_HEIGHT).max(1.0))
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
        let token = action_token();
        let navigation_token = token.clone();

        let init_script = format!(
            "window.__neuralia_col_index = {col_index}; window.__neuralia_col_name = '{col_name}';\n{}\n{}",
            script_with_token(NEURALIA_KEYMAP_SCRIPT, &token),
            script_with_token(COMPARATOR_INJECT_SCRIPT, &token)
        );

        WebViewBuilder::new()
            .with_initialization_script(init_script)
            .with_navigation_handler(move |target| {
                if let Some(action_url) = trusted_action_url(&target, &navigation_token)
                    && action_url
                        .path()
                        .trim_matches('/')
                        .eq_ignore_ascii_case("expand")
                {
                    if let Some((_, value)) = action_url.query_pairs().find(|(key, _)| key == "col")
                        && let Ok(index) = value.parse::<usize>()
                        && index < COMPARATOR_COLUMNS
                    {
                        let _ = navigation_proxy.send_event(UserEvent::ExpandComparator(index));
                    }
                    return false;
                }

                if let Some(event) = neuralia_action(&target, &navigation_token) {
                    let _ = navigation_proxy.send_event(event);
                    return false;
                }

                if target == "about:blank" {
                    return true;
                }
                neural_core::validate_web_url(&target)
                    .is_ok_and(|url| !is_local_network_target(&url))
            })
            .with_new_window_req_handler(move |target, _features| {
                if let Ok(url) = neural_core::validate_web_url(&target)
                    && !is_local_network_target(&url)
                {
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

    /// Um nivel para tras. O Escape da janela nativa e o Escape apanhado dentro
    /// das paginas acabam os dois aqui.
    fn go_back(&mut self) {
        if self.surface == Surface::Comparator
            && let Some(comp) = &self.comparator
            && comp.expanded.is_some()
        {
            self.restore_comparator();
            return;
        }
        self.show_home();
    }

    /// Liga/desliga a rolagem de leitura. O temporizador e nativo e nao vive na
    /// pagina: assim sobrevive a navegacao dentro do site.
    fn toggle_auto_scroll(&mut self) {
        SPLASH_ASKS.store(false, Ordering::SeqCst);
        self.splash_question_token = None;
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
        self.show_splash_kind(text, seconds, false);
    }

    fn show_splash_kind(&mut self, text: String, seconds: u64, question: bool) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SPLASH_WIDTH * scale).round() as i32;
        let height = (SPLASH_HEIGHT * scale).round() as i32;

        if !question && self.splash_question_token.take().is_some() {
            self.auto_scroll_answered = true;
            self.auto_scroll = false;
        }
        SPLASH_ASKS.store(question, Ordering::SeqCst);
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
        self.splash_question_token = question.then_some(token);
        self.splash_watch_token.store(token, Ordering::SeqCst);
        self.splash_deadline.store(
            now_ms().saturating_add(seconds.saturating_mul(1000)),
            Ordering::SeqCst,
        );
    }

    fn hide_splash(&mut self, token: u64) {
        if token != self.splash_token {
            return;
        }

        self.splash_deadline.store(0, Ordering::SeqCst);
        if self.splash_question_token == Some(token) {
            // Timeout equivale a "Nao": a pergunta foi respondida e nao volta a
            // contaminar splashes informativos com botoes Sim/Nao.
            self.splash_question_token = None;
            self.auto_scroll_answered = true;
            self.auto_scroll = false;
            SPLASH_ASKS.store(false, Ordering::SeqCst);
        }

        if let Some(splash) = self.splash.take() {
            unsafe {
                DestroyWindow(splash);
            }
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
        self.show_splash_kind(
            format!("Rolar a página sozinho a cada {AUTO_SCROLL_SECONDS}s?"),
            AUTO_SCROLL_PROMPT_SECONDS,
            true,
        );
    }

    /// Sem resposta nao se mexe: se a pergunta desaparecer sozinha, fica "nao"
    /// ate a pessoa carregar em F8.
    fn answer_auto_scroll(&mut self, yes: bool) {
        SPLASH_ASKS.store(false, Ordering::SeqCst);
        self.splash_question_token = None;
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
        // O PDF e entregue one-shot ao protocolo interno para nao manter uma
        // segunda copia de ate 64 MiB em Rust. Recarregar a pagina exigiria
        // duplicar de novo o documento; o viewer ja mantem o PDF carregado.
        if self.surface == Surface::Pdf {
            self.show_splash(
                "O PDF já está carregado; a recarga foi ignorada.".to_string(),
                3,
            );
            return;
        }
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

    /// `view-source:` e do proprio Chromium; basta navegar para la.
    fn view_source(&mut self) {
        let Some(webview) = &self.webview else {
            self.show_splash("Ver código-fonte só numa página aberta.".to_string(), 3);
            return;
        };
        let _ = webview
            .evaluate_script("window.location.href = 'view-source:' + window.location.href;");
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
        // Um documento sozinho no ecra -- HTML, texto simples ou PDF -- avanca
        // com a tecla, que e a unica via que funciona em todos eles. Nas tres
        // colunas a tecla so chegaria a uma, por isso ai vai o script.
        match self.surface {
            Surface::Comparator | Surface::Pdf => {
                self.for_each_visible_webview(|webview| {
                    let _ = webview.evaluate_script(AUTO_SCROLL_SCRIPT);
                });
            }
            _ => self.page_down_synthetic(),
        }
        self.schedule_auto_scroll();
    }

    /// O visualizador de PDF do Edge corre noutro documento, noutra origem e
    /// noutro processo: nenhum script do host la chega. A unica via que resta e
    /// a tecla, e so a enviamos com a nossa janela em primeiro plano -- caso
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
            if GetForegroundWindow() != hwnd {
                return;
            }

            let mut inputs: [INPUT; 2] = std::mem::zeroed();
            for (index, input) in inputs.iter_mut().enumerate() {
                input.r#type = INPUT_KEYBOARD;
                input.Anonymous.ki.wVk = VK_NEXT;
                input.Anonymous.ki.dwFlags = if index == 1 { KEYEVENTF_KEYUP } else { 0 };
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
            Some(comp) => comp.expanded.is_none() || self.chrome_revealed,
            None => false,
        }
    }

    fn bar_layout(&self) -> Option<BarLayout> {
        let (Some(window), Some(comp)) = (&self.window, &self.comparator) else {
            return None;
        };
        Some(BarLayout::new(
            window.inner_size().width as f64,
            window.scale_factor(),
            self.bar_visible(),
            comp.views.len(),
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
            let top = ((TOP_BAR_HEIGHT + 10.0) * scale).round() as i32;
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

                // Em CI o lifecycle e exercitado pelo proprio event loop, sem
                // SendKeys/foco. O worker apenas pede abrir/fechar e espera o
                // probe confirmar cada transicao antes de seguir.
                if let (Some(cycles), Some(probe_path)) = (
                    std::env::var("NEURALIA_LIFECYCLE_SELFTEST")
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                        .filter(|cycles| *cycles > 0),
                    std::env::var_os("NEURALIA_LIFECYCLE_PROBE")
                        .map(std::path::PathBuf::from),
                ) {
                    let proxy = self.proxy.clone();
                    let _ = thread::Builder::new()
                        .name("neural-lifecycle-selftest".into())
                        .spawn(move || {
                            thread::sleep(Duration::from_millis(350));
                            for cycle in 0..cycles {
                                let _ = proxy.send_event(UserEvent::SubmitText(format!(
                                    "neuralia lifecycle probe {}",
                                    cycle + 1
                                )));
                                if !wait_for_probe_value(&probe_path, "3", Duration::from_secs(30))
                                {
                                    return;
                                }

                                thread::sleep(Duration::from_millis(400));
                                let _ = proxy.send_event(UserEvent::HomeRequested);
                                if !wait_for_probe_value(&probe_path, "0", Duration::from_secs(30))
                                {
                                    return;
                                }
                                thread::sleep(Duration::from_millis(400));
                            }
                        });
                } else {
                    // Abertura normal: producao fica em Home; a variavel de
                    // startup existe apenas para automacao/demo explicita.
                    let startup = startup_input();
                    if !startup.is_empty() {
                        self.set_omnibox_text(&startup);
                        let _ = self.proxy.send_event(UserEvent::SubmitText(startup));
                    }
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
    bytes: &Arc<Mutex<Option<Vec<u8>>>>,
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
            let data = bytes
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .unwrap_or_default();
            if data.is_empty() {
                (
                    410,
                    "text/plain",
                    Cow::Borrowed(b"document already consumed" as &[u8]),
                )
            } else {
                (200, "application/pdf", Cow::Owned(data))
            }
        }
        _ => (404, "text/plain", Cow::Borrowed(b"not found" as &[u8])),
    };

    HttpResponse::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_else(|_| HttpResponse::new(Cow::Borrowed(b"" as &[u8])))
}

static ACTION_FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(1);

fn action_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_ok() {
        return bytes
            .iter()
            .fold(String::with_capacity(32), |mut out, byte| {
                out.push_str(&format!("{byte:02x}"));
                out
            });
    }

    // O fallback so existe para uma falha extrema do RNG do SO. Continua
    // variando por processo/tempo/contador, mas o caminho normal e getrandom.
    let counter = ACTION_FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:016x}{:016x}",
        now_ms() ^ ((std::process::id() as u64) << 32),
        counter.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    )
}

fn script_with_token(script: &str, token: &str) -> String {
    script.replace("__TOKEN__", token)
}

fn trusted_action_url(target: &str, token: &str) -> Option<Url> {
    let url = Url::parse(target).ok()?;
    if !url.scheme().eq_ignore_ascii_case("neuralia") {
        return None;
    }
    let supplied = url
        .query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned())?;
    (supplied == token).then_some(url)
}

fn neuralia_action(target: &str, token: &str) -> Option<UserEvent> {
    let url = trusted_action_url(target, token)?;
    let name = url.path().trim_matches('/');

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
        "clearhistory" => UserEvent::ClearHistory,
        "fullscreen" => UserEvent::ToggleColumnFullscreen,
        "devtools" => UserEvent::OpenDevTools,
        "viewsource" => UserEvent::ViewSource,
        _ => return None,
    })
}

fn is_pdf_internal_url(target: &str) -> bool {
    let Ok(url) = Url::parse(target) else {
        return false;
    };
    url.scheme() == "http"
        && url.host_str() == Some("neuralia-pdf.localhost")
        && url.port_or_known_default() == Some(80)
}

fn wait_for_probe_value(path: &std::path::Path, expected: &str, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if std::fs::read_to_string(path)
            .is_ok_and(|value| value.trim() == expected)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(80));
    }
    false
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

        // Só a marca e a barra, como a página inicial do Google. A arte já
        // traz o nome lá dentro, por isso não se repete em texto.
        let brand_width = (420.0 * scale).min(width * 0.52);
        let brand_height = brand_width * BRAND_ASPECT;
        let brand_x = ((width - brand_width) / 2.0).round() as i32;
        let brand_y = (layout.input.y - brand_height - 44.0 * scale)
            .max(24.0 * scale)
            .round() as i32;
        draw_brand(
            hdc,
            brand_x,
            brand_y,
            brand_width.round() as i32,
            brand_height.round() as i32,
            theme.page_bg,
        );

        let body_font = create_font((-17.0 * scale) as i32, FW_NORMAL as i32);
        let small_font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);

        let old_font = SelectObject(hdc, body_font as _);

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
        DeleteObject(body_font as _);
        DeleteObject(small_font as _);
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
    let layout = BarLayout::new(width as f64, scale, visible, names.len());
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

    // Alinhado a direita a partir do fim da ultima pilula: fica com todo o
    // espaco livre sem obrigar a estreitar as pilulas, que tem de continuar
    // centradas sobre as colunas.
    let pills_right = layout
        .columns
        .iter()
        .take(layout.columns_len)
        .map(|rect| rect.x + rect.width)
        .fold(layout.home.x + layout.home.width, f64::max);

    let hint = if auto_scroll {
        format!("F8: rolagem {AUTO_SCROLL_SECONDS}s ligada  ·  Esc: voltar")
    } else {
        "F8: rolagem automática  ·  Esc: voltar".to_string()
    };

    SelectObject(target, font as _);
    SetTextColor(target, rgb3(theme.fg_muted));
    let mut hint_rect = RECT {
        left: (pills_right + 12.0 * scale) as i32,
        top: 0,
        right: (width as f64 - 12.0 * scale) as i32,
        bottom: bar_h,
    };
    draw_text(
        target,
        &hint,
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
/// Limite para um documento; o do Reader (2 MiB) e para HTML.
const PDF_MAX_BYTES: usize = 64 * 1024 * 1024;
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
            let height = TOP_BAR_HEIGHT as i32;
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
    fn neuralia_actions_require_the_per_webview_capability() {
        let token = "test-capability";
        for (name, expected) in [
            ("back", "BackRequested"),
            ("zoomin", "ZoomIn"),
            ("zoomout", "ZoomOut"),
            ("zoomreset", "ZoomReset"),
            ("reload", "ReloadPage"),
            ("print", "PrintPage"),
            ("omnibox", "FocusOmnibox"),
            ("history", "ShowHistory"),
            ("clearhistory", "ClearHistory"),
            ("fullscreen", "ToggleColumnFullscreen"),
            ("autoscroll", "ToggleAutoScroll"),
            ("restore", "RestoreComparator"),
            ("home", "HomeRequested"),
        ] {
            let target = format!("neuralia:{name}?token={token}");
            let action = neuralia_action(&target, token);
            assert!(action.is_some(), "{target} devia ser reconhecido");
            assert!(
                format!("{:?}", action.unwrap()).starts_with(expected),
                "{target} devia dar {expected}"
            );
        }

        for target in [
            "neuralia:clearhistory",
            "neuralia:clearhistory?token=wrong",
            "https://example.com",
            "neuralia:inventado?token=test-capability",
            "about:blank",
            "neuralia",
        ] {
            assert!(
                neuralia_action(target, token).is_none(),
                "{target} nao devia ganhar uma acao nativa"
            );
        }
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
    fn bar_layout_hit_matches_drawing() {
        let layout = BarLayout::new(1600.0, 1.0, true, 3);

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
  Object.defineProperty(window, '__neuralia_keymap', { value: true, configurable: false });
  const token = '__TOKEN__';

  function act(name, extra) {
    let url = 'neuralia:' + name + '?token=' + encodeURIComponent(token);
    if (extra) { url += '&' + extra; }
    window.location.href = url;
  }

  function findBar() {
    var id = 'neuralia-find';
    var box = document.getElementById(id);
    if (box) {
      var existing = box.querySelector('input');
      if (existing) { existing.focus(); existing.select(); }
      return;
    }

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
    close.addEventListener('click', function (e) {
      if (!e.isTrusted) { return; }
      box.remove();
    });

    input.addEventListener('keydown', function (e) {
      if (!e.isTrusted) { return; }
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
    var target = e.target || {};

    if (key === 'escape' && target.closest && target.closest('#neuralia-find')) {
      e.preventDefault();
      e.stopPropagation();
      var find = document.getElementById('neuralia-find');
      if (find) { find.remove(); }
      return;
    }

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
        case 't': e.preventDefault(); act('home'); return;
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
      if (typeof window.__neuralia_col_index === 'number') {
        e.preventDefault();
        act('expand', 'col=' + (parseInt(key, 10) - 1));
      }
      return;
    }
    if (key === '0' && typeof window.__neuralia_col_index === 'number') {
      e.preventDefault();
      act('restore');
    }
  }, true);
})();
"#;

/// Rotulos do botao injetado no comparador. Em tela cheia a barra nativa some,
/// por isso este botao tem de anunciar a saida.
const COMPARATOR_BUTTON_EXPANDED: &str = "(function(){var w=document.querySelector('#neuralia-comp-btn');if(w){w.style.display='none';}})();";
const COMPARATOR_BUTTON_COLLAPSED: &str = "(function(){var w=document.querySelector('#neuralia-comp-btn');if(w){w.style.display='flex';}var b=document.querySelector('#neuralia-comp-btn button');if(b){b.textContent='\u{26F6} Expandir ' + (window.__neuralia_col_name || 'IA');}})();";

const EXTERNAL_RETURN_BUTTON: &str = r#"
document.addEventListener('DOMContentLoaded', () => {
  if (document.getElementById('neural-shell') || document.getElementById('neuralia-return')) return;
  const token = '__TOKEN__';
  const b = document.createElement('button');
  b.id = 'neuralia-return';
  b.textContent = '◀ NeuralIA';
  Object.assign(b.style, {
    position:'fixed', left:'16px', bottom:'16px', zIndex:'2147483647',
    border:'0', borderRadius:'999px', padding:'11px 16px',
    background:'#111314', color:'#fff', font:'600 13px Segoe UI, sans-serif',
    boxShadow:'0 6px 24px rgba(0,0,0,.25)', cursor:'pointer'
  });
  b.addEventListener('click', (e) => {
    if (!e.isTrusted) return;
    window.location.href = 'neuralia:home?token=' + encodeURIComponent(token);
  });
  document.documentElement.appendChild(b);
});
"#;

const COMPARATOR_INJECT_SCRIPT: &str = r#"
document.addEventListener('DOMContentLoaded', () => {
  if (document.getElementById('neuralia-comp-btn')) return;
  const token = '__TOKEN__';
  const colIndex = window.__neuralia_col_index ?? 0;
  const colName = window.__neuralia_col_name ?? 'IA';

  function nativeAction(name, extra) {
    let url = 'neuralia:' + name + '?token=' + encodeURIComponent(token);
    if (extra) url += '&' + extra;
    window.location.href = url;
  }

  const wrap = document.createElement('div');
  wrap.id = 'neuralia-comp-btn';
  Object.assign(wrap.style, {
    position:'fixed', top:'10px', right:'46px', zIndex:'2147483647',
    display:'flex', gap:'6px',
    fontFamily:'Segoe UI, -apple-system, BlinkMacSystemFont, sans-serif'
  });

  const btn = document.createElement('button');
  btn.textContent = '⛶ Expandir ' + colName;
  Object.assign(btn.style, {
    border:'0', borderRadius:'6px', padding:'6px 12px',
    background:'#111314', color:'#ffffff', fontSize:'11px',
    fontWeight:'600', boxShadow:'0 4px 12px rgba(0,0,0,0.3)',
    cursor:'pointer', opacity:'0.9', transition:'transform 0.15s ease'
  });
  btn.onmouseover = () => { btn.style.transform = 'scale(1.05)'; };
  btn.onmouseout = () => { btn.style.transform = 'scale(1)'; };
  btn.addEventListener('click', (e) => {
    if (!e.isTrusted) return;
    e.preventDefault();
    e.stopPropagation();
    nativeAction('expand', 'col=' + colIndex);
  });
  wrap.appendChild(btn);
  document.documentElement.appendChild(wrap);

  // Trilha vertical discreta, inspirada em uma linha do tempo. Cada WebView
  // recebe a sua propria instancia e portanto rola sem afetar as outras duas.
  const style = document.createElement('style');
  style.id = 'neuralia-scroll-style';
  style.textContent =
    '.neuralia-scroll-target{scrollbar-width:none!important;-ms-overflow-style:none!important}' +
    '.neuralia-scroll-target::-webkit-scrollbar{width:0!important;height:0!important}';
  document.documentElement.appendChild(style);

  const rail = document.createElement('div');
  rail.id = 'neuralia-scroll-rail';
  Object.assign(rail.style, {
    position:'fixed', right:'6px', top:'50%', transform:'translateY(-50%)',
    width:'34px', height:'132px', zIndex:'2147483646',
    cursor:'ns-resize', touchAction:'none', userSelect:'none'
  });

  const spine = document.createElement('div');
  Object.assign(spine.style, {
    position:'absolute', right:'10px', top:'0', width:'2px', height:'100%',
    background:'rgba(120,120,120,.20)', borderRadius:'2px'
  });
  rail.appendChild(spine);

  [0.16, 0.38, 0.62, 0.84].forEach((fraction) => {
    const tick = document.createElement('div');
    Object.assign(tick.style, {
      position:'absolute', right:'10px', top:(fraction * 100) + '%',
      width:'11px', height:'2px', transform:'translateY(-50%)',
      background:'rgba(150,150,150,.52)', borderRadius:'2px'
    });
    rail.appendChild(tick);
  });

  const cursor = document.createElement('div');
  Object.assign(cursor.style, {
    position:'absolute', right:'10px', top:'0%', width:'24px', height:'2px',
    transform:'translateY(-50%)', background:'rgba(210,210,210,.88)',
    borderRadius:'2px', boxShadow:'0 0 8px rgba(0,0,0,.20)',
    transition:'top 70ms linear'
  });
  rail.appendChild(cursor);
  document.documentElement.appendChild(rail);

  let scroller = null;
  let dragging = false;

  function canScroll(el) {
    if (!el || el === rail || (el.closest && el.closest('#neuralia-scroll-rail'))) return false;
    return (el.scrollHeight - el.clientHeight) > 80 && el.clientHeight > 160;
  }

  function findScroller() {
    const root = document.scrollingElement || document.documentElement;
    if (canScroll(root)) return root;

    let best = null;
    let bestScore = -1;
    const nodes = document.querySelectorAll('main,[role="main"],section,div');
    const limit = Math.min(nodes.length, 700);
    for (let i = 0; i < limit; i++) {
      const el = nodes[i];
      if (!canScroll(el)) continue;
      const rect = el.getBoundingClientRect();
      if (rect.width < 220 || rect.height < 180 || rect.bottom <= 0 || rect.top >= innerHeight) continue;
      const css = getComputedStyle(el);
      const overflow = css.overflowY;
      if (overflow !== 'auto' && overflow !== 'scroll' && el.scrollHeight < innerHeight * 1.4) continue;
      const score = (el.scrollHeight - el.clientHeight) + rect.height * 2 + rect.width;
      if (score > bestScore) { best = el; bestScore = score; }
    }
    return best || root;
  }

  function useScroller(next) {
    if (!next || next === scroller) return;
    if (scroller && scroller.classList) scroller.classList.remove('neuralia-scroll-target');
    scroller = next;
    if (scroller.classList) scroller.classList.add('neuralia-scroll-target');
    updateRail();
  }

  function updateRail() {
    if (!scroller || !canScroll(scroller)) useScroller(findScroller());
    if (!scroller) return;
    const max = Math.max(1, scroller.scrollHeight - scroller.clientHeight);
    const p = Math.max(0, Math.min(1, scroller.scrollTop / max));
    cursor.style.top = (p * 100) + '%';
    rail.style.opacity = max > 1 ? '1' : '.28';
  }

  function scrollToPointer(e) {
    if (!e.isTrusted) return;
    useScroller(scroller || findScroller());
    if (!scroller) return;
    const rect = rail.getBoundingClientRect();
    const p = Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height));
    const max = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    scroller.scrollTop = p * max;
    cursor.style.top = (p * 100) + '%';
  }

  rail.addEventListener('pointerdown', (e) => {
    if (!e.isTrusted) return;
    dragging = true;
    try { rail.setPointerCapture(e.pointerId); } catch (_) {}
    scrollToPointer(e);
    e.preventDefault();
    e.stopPropagation();
  });
  rail.addEventListener('pointermove', (e) => {
    if (dragging) scrollToPointer(e);
  });
  rail.addEventListener('pointerup', (e) => {
    dragging = false;
    try { rail.releasePointerCapture(e.pointerId); } catch (_) {}
  });
  rail.addEventListener('pointercancel', () => { dragging = false; });

  document.addEventListener('scroll', (e) => {
    const target = e.target === document ? document.scrollingElement : e.target;
    if (target && canScroll(target)) useScroller(target);
    updateRail();
  }, true);

  window.addEventListener('resize', updateRail, { passive:true });
  const mutationObserver = new MutationObserver(() => {
    if (window.__neuralia_scroll_recheck) return;
    window.__neuralia_scroll_recheck = setTimeout(() => {
      window.__neuralia_scroll_recheck = 0;
      useScroller(findScroller());
      updateRail();
    }, 180);
  });
  mutationObserver.observe(document.documentElement, { childList:true, subtree:true });

  useScroller(findScroller());
  updateRail();

  document.addEventListener('dblclick', (e) => {
    if (!e.isTrusted) return;
    if (e.target && e.target.closest && (
      e.target.closest('#neuralia-comp-btn') || e.target.closest('#neuralia-scroll-rail')
    )) return;
    const tag = e.target && e.target.tagName ? e.target.tagName.toUpperCase() : '';
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (e.target && e.target.isContentEditable) return;
    nativeAction('expand', 'col=' + colIndex);
  }, true);
});
"#;
