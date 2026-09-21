#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    borrow::Cow,
    cell::Cell,
    collections::BinaryHeap,
    ffi::OsString,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

use image::RgbaImage;

#[cfg(test)]
use crate::ipc::constant_time_eq;
use crate::ipc::{IpcAction, parse_ipc_message};
use neural_core::{
    ActionRisk, AgentAction, AgentElement, AgentPermissionPolicy, AgentRuntimeConfig,
    AgentSecurityAction, CoreConfig, FieldKind, HistoryEntry, HistoryKind, HistoryStore, Intent,
    MemoryDocument, MemoryHit, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore,
    ObservedPage, ReaderArticle, ReaderBlock, ReaderClient, ResearchItemKind, ResearchSession,
    chatgpt_search_url, claude_search_url, google_ai_url, is_local_network_target, is_pdf_url,
    parse_intent, reader_html, redact_sensitive_text, tissue,
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        BeginPaint, BitBlt, CLEARTYPE_QUALITY, ClientToScreen, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen, CreateRoundRectRgn,
        CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER,
        DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
        Ellipse, EndPaint, FW_BOLD, FW_NORMAL, FillRect, GetDC, GetStockObject, InvalidateRect,
        LineTo, MoveToEx, NULL_BRUSH, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, ReleaseDC,
        SRCCOPY, ScreenToClient, SelectObject, SetBkColor, SetBkMode, SetTextColor, SetWindowRgn,
        StretchDIBits, TRANSPARENT,
    },
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetFocus, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, SendInput,
            SetFocus, VK_CONTROL, VK_ESCAPE, VK_NEXT, VK_RETURN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow,
            ES_AUTOHSCROLL, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowTextLengthW,
            GetWindowTextW, GetWindowThreadProcessId, IDYES, MB_ICONINFORMATION, MB_OK, MB_YESNO,
            MF_SEPARATOR, MF_STRING, MessageBoxW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
            SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
            TrackPopupMenu, WM_KEYDOWN, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
            WS_TABSTOP, WS_VISIBLE,
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
    /// O historico recente lido pelo worker; a caixa nativa e mostrada aqui,
    /// no event loop, e nunca a leitura do ficheiro.
    HistoryLoaded(Result<Vec<HistoryEntry>, String>),
    MemoryQueryReady {
        query: String,
        result: Result<Vec<MemoryHit>, String>,
    },
    MemoryCleared(Result<(), String>),
    ResearchAnswer {
        source_index: usize,
        text: String,
    },
    AgentObservation(ObservedPage),
    /// Esconde outra vez a barra em ecra completo, se nada a tiver reavivado.
    HideChrome(u64),
    SubmitText(String),
    OpenExternal(String),
    /// Popup pedido por uma coluna do comparador: carrega nessa coluna.
    OpenInColumn(usize, String),
    /// Clique simples num link: a mesma pagina nas TRES colunas, para se
    /// poder comparar o que cada IA diz dela. E o gesto que distingue este
    /// navegador de um normal, onde o clique so afeta o separador de onde
    /// partiu.
    OpenEverywhere(String),
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
    /// A pagina so pode PEDIR a palette (Ctrl+K/T ou o botao +): a coluna e
    /// a do proprio WebView; texto e destino nunca viajam por este canal.
    OpenPalette(usize),
    /// Enter no EDIT nativo da palette. `source_index` e `private` vem do
    /// estado nativo escrito ao abrir; `input` e lido do controlo Win32.
    PaletteSubmit {
        source_index: usize,
        input: String,
        private: bool,
    },
    /// Escape ou perda de foco no EDIT da palette; traz a geracao que viu.
    ClosePalette(u64),
    ExpandComparator(usize),
    MinimizeComparator(usize),
    /// Ha um arrasto de divisor por atender. O divisor e o x vem dos statics
    /// `RESIZE_*`, nao do evento: assim os movimentos que chegam enquanto
    /// este esta na fila substituem-se uns aos outros em vez de se somarem.
    ResizeComparator,
    /// Reaplica a geometria depois de o Windows terminar a transicao
    /// assíncrona para a janela sem decoracao. Nao depende de rato/teclado.
    RelayoutComparator,
    /// Restaura a moldura nativa da Home num ciclo posterior ao drop dos
    /// WebViews. Isto evita reparentear hosts WRY enquanto WebView2 ainda
    /// conclui a destruição dos controllers no pump de mensagens.
    RestoreHomeDecorations,
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
/// Cada aba visivel pode arrastar consigo a pilula do seu grupo, e um grupo
/// fechado ocupa um lugar sem mostrar abas nenhumas -- dai o dobro.
const MAX_VISIBLE_TAB_SLOTS: usize = MAX_VISIBLE_CONTEXT_TABS * 2;
const COMPARATOR_COLUMNS: usize = 3;
/// Intervalo da rolagem automatica de leitura, do primeiro avanco ao ultimo.
const AUTO_SCROLL_SECONDS: u64 = 30;
/// Quanto tempo a pergunta fica no ecra antes de se dar por respondida com
/// "nao". Sem resposta nao se mexe em nada: e uma pergunta, nao um aviso.
const AUTO_SCROLL_PROMPT_SECONDS: u64 = 20;
/// Quanto tempo a barra fica visivel em ecra completo depois do ultimo
/// movimento do rato no topo.
const CHROME_HIDE_DELAY_MS: u64 = 2500;
/// A moldura Win32 pode mudar o client rect um ciclo depois de
/// set_decorations(false). Fazemos dois relayouts baratos para nao deixar
/// WebViews presos na geometria anterior ate o primeiro movimento do rato.
const COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS: [u64; 2] = [40, 220];
/// Quanto tempo o aviso de correio novo fica no canto.
const GMAIL_TOAST_SECONDS: u64 = 7;
/// Quantas entradas do historico a caixa "history:" mostra.
const HISTORY_RECENT_LIMIT: usize = 20;
/// Tecto, em chars, de cada campo que o monitor do Gmail nos envia. O script
/// ja corta a 180, mas o script corre numa pagina remota: o lado nativo nao
/// pode confiar nesse corte e repete-o antes de guardar ou pintar.
const GMAIL_FIELD_MAX_CHARS: usize = 180;

const SPLASH_SUBCLASS_ID: usize = 0x4E4C;
const GMAIL_TOAST_SUBCLASS_ID: usize = 0x4E4D;
const PALETTE_SUBCLASS_ID: usize = 0x4E4E;
const PALETTE_EDIT_SUBCLASS_ID: usize = 0x4E4F;
/// Palette nativa, em pixeis logicos: nunca mais larga que isto nem que a
/// coluna a que pertence menos as margens; a altura e fixa.
const PALETTE_MAX_WIDTH: f64 = 680.0;
const PALETTE_MIN_WIDTH: f64 = 240.0;
const PALETTE_HEIGHT: f64 = 78.0;
const PALETTE_CORNER: f64 = 18.0;
const PALETTE_PAD_X: f64 = 16.0;
const PALETTE_EDIT_TOP: f64 = 14.0;
const PALETTE_EDIT_HEIGHT: f64 = 30.0;
const PALETTE_HINT_TOP: f64 = 48.0;
/// Fraccao da altura util (abaixo da barra) a que a palette pousa.
const PALETTE_TOP_RATIO: f64 = 0.18;
const SPLASH_WIDTH: f64 = 470.0;
const SPLASH_HEIGHT: f64 = 46.0;
const GMAIL_TOAST_WIDTH: f64 = 390.0;
const GMAIL_TOAST_HEIGHT: f64 = 68.0;

/// Texto do aviso flutuante. Vive fora do App porque quem o pinta e o
/// procedimento de janela, que nao tem acesso ao estado da aplicacao.
static SPLASH_TEXT: Mutex<String> = Mutex::new(String::new());
static GMAIL_TOAST_TEXT: Mutex<String> = Mutex::new(String::new());
/// Legenda da palette nativa (para onde vao URL e texto). Fora do App pela
/// mesma razao que SPLASH_TEXT: quem a pinta e o procedimento de janela.
static PALETTE_HINT: Mutex<String> = Mutex::new(String::new());
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
    /// A pilula de um grupo. O indice e o do grupo dentro da coluna.
    ContextGroup {
        source_index: usize,
        group_index: usize,
    },
    SplitExpand,
    SplitClose,
    Private,
    WindowMinimize,
    WindowMaximize,
    WindowClose,
}

/// O estado do comparador de que a barra precisa. Anda sempre junto -- quem
/// arrasta um divisor muda os pesos, quem minimiza muda as duas coisas -- e
/// agrupa-lo evita que a barra receba uma parte e esqueca a outra, que era
/// exactamente como os rotulos deixavam de estar sobre as colunas.
#[derive(Debug, Clone, Copy)]
struct BarColumns {
    count: usize,
    weights: [f64; COMPARATOR_COLUMNS],
    minimized: [bool; COMPARATOR_COLUMNS],
    /// Ha uma gaveta aberta: os botoes do Split ocupam o canto direito e os
    /// chips tem de parar antes deles.
    split_active: bool,
}

impl BarColumns {
    /// Colunas iguais, nenhuma minimizada -- o estado de partida, e o que os
    /// testes de geometria usam quando os pesos nao sao o assunto.
    fn even(count: usize) -> Self {
        Self {
            count,
            weights: [1.0; COMPARATOR_COLUMNS],
            minimized: [false; COMPARATOR_COLUMNS],
            split_active: false,
        }
    }
}

/// Geometria em duas linhas. As fontes ficam na title bar; os provedores ficam
/// numa segunda linha, sem disputar espaco com as abas.
#[derive(Debug, Clone, Copy)]
struct BarLayout {
    visible: bool,
    height: f64,
    home: UiRect,
    /// Por coluna: a pilula do provedor sobre a sua faixa, ou -- se estiver
    /// minimizada -- o chip compacto encostado aos controlos da direita.
    columns: [UiRect; COMPARATOR_COLUMNS],
    /// Quais das `columns` sao chips. O desenho precisa de saber porque o
    /// chip e apagado e nao leva o botao "+".
    minimized: [bool; COMPARATOR_COLUMNS],
    add_tabs: [UiRect; COMPARATOR_COLUMNS],
    context_tabs: [[UiRect; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    context_indices: [[usize; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    context_tab_counts: [usize; COMPARATOR_COLUMNS],
    /// Pilulas dos grupos, intercaladas com as abas na mesma fila.
    group_pills: [[UiRect; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
    group_pill_indices: [[usize; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
    group_pill_counts: [usize; COMPARATOR_COLUMNS],
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
            BarColumns::even(columns),
            [0; COMPARATOR_COLUMNS],
        )
    }

    /// Atalho para quem so sabe quantas abas tem cada coluna: nenhuma delas
    /// esta agrupada.
    #[cfg(test)]
    fn with_contexts(
        client_width: f64,
        scale: f64,
        visible: bool,
        columns: BarColumns,
        context_counts: [usize; COMPARATOR_COLUMNS],
    ) -> Self {
        Self::with_rows(
            client_width,
            scale,
            visible,
            columns,
            std::array::from_fn(|index| TabRow::plain(context_counts[index])),
        )
    }

    fn with_rows(
        client_width: f64,
        scale: f64,
        visible: bool,
        columns: BarColumns,
        rows: [TabRow; COMPARATOR_COLUMNS],
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
                minimized: [false; COMPARATOR_COLUMNS],
                add_tabs: [empty; COMPARATOR_COLUMNS],
                context_tabs: [[empty; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                context_indices: [[0; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                context_tab_counts: [0; COMPARATOR_COLUMNS],
                group_pills: [[empty; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
                group_pill_indices: [[0; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
                group_pill_counts: [0; COMPARATOR_COLUMNS],
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
        let mut pills = [[empty; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS];
        let mut pill_indices = [[0usize; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS];
        let mut pill_counts = [0usize; COMPARATOR_COLUMNS];
        let columns_len = columns.count.min(COMPARATOR_COLUMNS);

        // Linha dos provedores, agora livre das abas. As faixas vem da MESMA
        // funcao que posiciona os WebViews: depois de arrastar um divisor o
        // rotulo continua sobre a sua coluna, e o hit-testing com ele.
        let spans = visible_column_spans(
            client_width / scale,
            columns.count,
            &columns.weights,
            &columns.minimized,
        );
        let group_pad = 6.0 * scale;
        let gap = 4.0 * scale;
        let provider_width = 116.0 * scale;
        let plus_width = 26.0 * scale;
        let chip_w = 62.0 * scale;
        let chip_gap = 5.0 * scale;

        // Os chips das colunas minimizadas e os controlos da direita sao
        // reservados ANTES de distribuir as pilulas. Ao contrario, a pilula
        // estendia-se ate a borda da janela e aterrava por cima do botao
        // "Privado" ou de um chip -- e como o hit-testing resolve por ordem de
        // indice, o clique ia parar a coluna errada.
        let hidden: Vec<usize> = (0..columns_len)
            .filter(|index| columns.minimized[*index])
            .collect();
        let chips_w = if hidden.is_empty() {
            0.0
        } else {
            hidden.len() as f64 * chip_w + chip_gap * hidden.len().saturating_sub(1) as f64
        };
        let controls_left = right_controls(client_width, scale, columns.split_active)
            .private
            .x;
        let reserved = if hidden.is_empty() {
            0.0
        } else {
            chips_w + 8.0 * scale
        };
        let bar_right = (controls_left - 8.0 * scale - reserved).max(pad);

        for (slot, span) in spans.iter().enumerate() {
            let mut left = span.x * scale + group_pad;
            if slot == 0 {
                left = left.max(home.x + home.width + 8.0 * scale);
            }
            let right = ((span.x + span.width) * scale - group_pad).min(bar_right);
            // O `.max()` que aqui estava punha o chao ACIMA do tecto: garantia
            // `available >= provider_width + plus_width + gap`, o que tornava o
            // `.min()` de baixo matematicamente morto e a pilula nunca encolhia.
            let available = (right - left).max(0.0);
            let pill = provider_width.min((available - plus_width - gap).max(0.0));
            columns_rect[span.index] = UiRect {
                x: left,
                y: row_y,
                width: pill,
                height: row_h,
            };
            // O "+" fica sempre dentro da faixa da sua coluna. Se nao couber,
            // desaparece -- em vez de ficar invisivel mas clicavel por cima do
            // vizinho, que e o pior dos dois mundos.
            let plus_x = left + pill + gap;
            plus_rect[span.index] = UiRect {
                x: plus_x,
                y: row_y + 2.0 * scale,
                width: if plus_x + plus_width <= right {
                    plus_width
                } else {
                    0.0
                },
                height: row_h - 4.0 * scale,
            };
        }

        // Colunas minimizadas: nao tem faixa, mas nao podem desaparecer da
        // barra -- e o chip que as traz de volta com um clique. Encostam-se a
        // direita, logo antes de Privado/Split, para nao roubarem espaco as
        // colunas que estao mesmo a ser vistas.
        if !hidden.is_empty() {
            // O espaco ja foi reservado acima; o `.max(bar_right)` garante que
            // os chips nunca recuam para dentro da faixa das pilulas, mesmo com
            // a janela absurdamente estreita.
            let mut x = (controls_left - 8.0 * scale - chips_w).max(bar_right);
            for index in hidden {
                columns_rect[index] = UiRect {
                    x,
                    y: row_y,
                    width: chip_w,
                    height: row_h,
                };
                x += chip_w + chip_gap;
            }
        }

        // Linha superior: todas as fontes/abas, antes dos controles da janela.
        let tabs_left = 90.0 * scale;
        let tabs_right = (window_minimize.x - 8.0 * scale).max(tabs_left);
        let visible_rows = &rows[..columns_len];
        let total_slots: usize = visible_rows.iter().map(|row| row.len).sum();
        let total_pills: usize = visible_rows
            .iter()
            .map(|row| {
                row.visible()
                    .iter()
                    .filter(|slot| matches!(slot, TabSlot::Group(_)))
                    .count()
            })
            .sum();
        let total_tabs = total_slots - total_pills;
        if total_slots > 0 && tabs_right > tabs_left {
            let gap = 3.0 * scale;
            // A pilula do grupo leva largura fixa: e um rotulo, nao um titulo
            // de pagina. O que sobra e das abas.
            let pill_width = 74.0 * scale;
            let spent =
                gap * total_slots.saturating_sub(1) as f64 + pill_width * total_pills as f64;
            let usable = tabs_right - tabs_left - spent;
            let tab_width = if total_tabs == 0 {
                0.0
            } else {
                (usable / total_tabs as f64).clamp(56.0 * scale, 156.0 * scale)
            };
            let mut x = tabs_left;
            let tab_y = 3.0 * scale;
            let tab_h = (title_h - 6.0 * scale).max(20.0 * scale);

            for index in 0..columns_len {
                for slot in rows[index].visible().iter().copied() {
                    if x + 28.0 * scale > tabs_right {
                        break;
                    }
                    let desired = match slot {
                        TabSlot::Group(_) => pill_width,
                        TabSlot::Tab(_) => tab_width,
                    };
                    let width = desired.min(tabs_right - x).max(28.0 * scale);
                    let rect = UiRect {
                        x,
                        y: tab_y,
                        width,
                        height: tab_h,
                    };
                    match slot {
                        TabSlot::Group(group) => {
                            let visual = pill_counts[index];
                            pills[index][visual] = rect;
                            pill_indices[index][visual] = group;
                            pill_counts[index] += 1;
                        }
                        TabSlot::Tab(context) => {
                            let visual = tab_counts[index];
                            if visual >= MAX_VISIBLE_CONTEXT_TABS {
                                continue;
                            }
                            tabs[index][visual] = rect;
                            tab_indices[index][visual] = context;
                            tab_counts[index] += 1;
                        }
                    }
                    x += width + gap;
                }
            }
        }

        Self {
            visible: true,
            height,
            home,
            columns: columns_rect,
            minimized: columns.minimized,
            add_tabs: plus_rect,
            context_tabs: tabs,
            context_indices: tab_indices,
            context_tab_counts: tab_counts,
            group_pills: pills,
            group_pill_indices: pill_indices,
            group_pill_counts: pill_counts,
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
            for visual in 0..self.group_pill_counts[index] {
                if self.group_pills[index][visual].contains(x, y) {
                    return Some(BarHit::ContextGroup {
                        source_index: index,
                        group_index: self.group_pill_indices[index][visual],
                    });
                }
            }
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

/// Os controlos do canto direito da segunda linha.
#[derive(Debug, Clone, Copy)]
struct RightControls {
    private: UiRect,
    /// Rotulo, expandir e fechar da gaveta; `None` quando nao ha gaveta.
    split: Option<(UiRect, UiRect, UiRect)>,
}

/// Geometria dos controlos encostados a direita. A mesma conta estava escrita
/// tres vezes -- no desenho, no hit-testing e agora nos chips -- e as copias
/// ja tinham comecado a divergir; aqui ela e uma so.
fn right_controls(client_width: f64, scale: f64, split_active: bool) -> RightControls {
    let margin = 8.0 * scale;
    let row_y = (TITLE_TAB_HEIGHT + 7.0) * scale;
    let row_h = 30.0 * scale;
    let gap = 5.0 * scale;

    let split = split_active.then(|| {
        let close = UiRect {
            x: client_width - margin - 30.0 * scale,
            y: row_y,
            width: 30.0 * scale,
            height: row_h,
        };
        let expand = UiRect {
            x: close.x - gap - 30.0 * scale,
            y: row_y,
            width: 30.0 * scale,
            height: row_h,
        };
        let label = UiRect {
            x: expand.x - gap - 150.0 * scale,
            y: row_y,
            width: 150.0 * scale,
            height: row_h,
        };
        (label, expand, close)
    });

    let right = match split {
        Some((label, _, _)) => label.x - 6.0 * scale,
        None => client_width - margin,
    };
    let private = UiRect {
        x: right - 78.0 * scale,
        y: row_y,
        width: 78.0 * scale,
        height: row_h,
    };

    RightControls { private, split }
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

/// As cores que um grupo de abas pode ter. Poucas e nomeadas: uma paleta
/// aberta obrigaria a um seletor, e o que se quer e distinguir grupos de
/// relance, nao escolher tons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupColor {
    Blue,
    Green,
    Amber,
    Pink,
    Purple,
    Slate,
}

impl GroupColor {
    const ALL: [Self; 6] = [
        Self::Blue,
        Self::Green,
        Self::Amber,
        Self::Pink,
        Self::Purple,
        Self::Slate,
    ];

    fn rgb(self) -> Rgb {
        match self {
            Self::Blue => (66, 133, 244),
            Self::Green => (52, 168, 83),
            Self::Amber => (244, 180, 0),
            Self::Pink => (233, 30, 99),
            Self::Purple => (156, 39, 176),
            Self::Slate => (96, 125, 139),
        }
    }

    /// A proxima cor por usar numa coluna, para dois grupos seguidos nao
    /// nascerem iguais.
    fn next(used: &[Self]) -> Self {
        Self::ALL
            .into_iter()
            .find(|color| !used.contains(color))
            .unwrap_or(Self::Blue)
    }
}

/// Um grupo de abas na barra de titulo: nome, cor e se esta fechado.
#[derive(Debug, Clone)]
struct ContextGroup {
    id: u64,
    name: String,
    color: GroupColor,
    collapsed: bool,
}

/// Uma aba de contexto. O `group` e o id do grupo, nao um indice: fechar um
/// grupo no meio nao pode renumerar as abas dos outros.
#[derive(Debug, Clone)]
struct ContextTab {
    url: String,
    group: Option<u64>,
}

/// Um lugar na fila de abas de uma coluna: ou a pilula de um grupo, ou uma aba.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabSlot {
    /// Indice do grupo dentro de `groups` daquela coluna.
    Group(usize),
    /// Indice da aba dentro de `contexts` daquela coluna.
    Tab(usize),
}

/// A fila visivel de uma coluna, ja cortada ao que cabe na barra.
#[derive(Debug, Clone, Copy)]
struct TabRow {
    slots: [TabSlot; MAX_VISIBLE_TAB_SLOTS],
    len: usize,
}

impl TabRow {
    fn empty() -> Self {
        Self {
            slots: [TabSlot::Tab(0); MAX_VISIBLE_TAB_SLOTS],
            len: 0,
        }
    }

    fn push(&mut self, slot: TabSlot) {
        if self.len < MAX_VISIBLE_TAB_SLOTS {
            self.slots[self.len] = slot;
            self.len += 1;
        }
    }

    fn visible(&self) -> &[TabSlot] {
        &self.slots[..self.len]
    }

    /// Uma coluna sem grupo nenhum: as ultimas abas, como era antes de existirem
    /// grupos. Serve os chamadores que so sabem contar abas.
    #[cfg(test)]
    fn plain(count: usize) -> Self {
        let mut row = Self::empty();
        let shown = count.min(MAX_VISIBLE_CONTEXT_TABS);
        for offset in 0..shown {
            row.push(TabSlot::Tab(count - shown + offset));
        }
        row
    }
}

/// Decide o que aparece na barra de uma coluna: a pilula de cada grupo antes da
/// sua primeira aba, as abas de um grupo fechado escondidas, e o resto cortado
/// pelo fim -- as abas recentes ficam, as antigas saem.
///
/// Duas invariantes que os testes prendem: uma aba cujo grupo ja nao existe
/// volta a ser solta em vez de desaparecer, e nunca sobra uma aba agrupada sem
/// a pilula do seu grupo a acompanha-la.
fn plan_tab_row(tabs: &[ContextTab], groups: &[ContextGroup]) -> TabRow {
    let group_of = |index: usize| -> Option<usize> {
        tabs.get(index)
            .and_then(|tab| tab.group)
            .and_then(|id| groups.iter().position(|group| group.id == id))
    };

    let mut full: Vec<TabSlot> = Vec::new();
    let mut billed: Vec<usize> = Vec::new();
    for index in 0..tabs.len() {
        if let Some(group) = group_of(index) {
            if !billed.contains(&group) {
                billed.push(group);
                full.push(TabSlot::Group(group));
            }
            if groups[group].collapsed {
                continue;
            }
        }
        full.push(TabSlot::Tab(index));
    }

    // Janela deslizante a contar do fim: para quando estoirar o numero de
    // lugares ou o numero de abas.
    let mut start = full.len();
    let mut tabs_kept = 0usize;
    while start > 0 {
        let candidate = start - 1;
        let is_tab = matches!(full[candidate], TabSlot::Tab(_));
        if is_tab && tabs_kept == MAX_VISIBLE_CONTEXT_TABS {
            break;
        }
        if full.len() - candidate > MAX_VISIBLE_TAB_SLOTS {
            break;
        }
        if is_tab {
            tabs_kept += 1;
        }
        start = candidate;
    }

    // A pilula vem sempre antes das suas abas. Se o corte caiu no meio de um
    // grupo, a pilula ficou de fora -- desce-se ate a proxima pilula ou ate
    // uma aba solta, em vez de mostrar orfas.
    while start < full.len() {
        match full[start] {
            TabSlot::Group(_) => break,
            TabSlot::Tab(index) if group_of(index).is_none() => break,
            TabSlot::Tab(_) => start += 1,
        }
    }

    let mut row = TabRow::empty();
    for slot in full[start..].iter().copied() {
        row.push(slot);
    }
    row
}

/// Cria um grupo com a aba indicada e devolve o indice do grupo novo. O nome
/// sai do host da aba -- um grupo sem nome nao diz nada a ninguem.
fn create_context_group(
    tabs: &mut [ContextTab],
    groups: &mut Vec<ContextGroup>,
    next_id: &mut u64,
    tab_index: usize,
) -> Option<usize> {
    let name = context_tab_label(&tabs.get(tab_index)?.url);
    let used: Vec<GroupColor> = groups.iter().map(|group| group.color).collect();
    let id = *next_id;
    *next_id += 1;
    groups.push(ContextGroup {
        id,
        name,
        color: GroupColor::next(&used),
        collapsed: false,
    });
    tabs[tab_index].group = Some(id);
    Some(groups.len() - 1)
}

/// Poe a aba no grupo e encosta-a ao ultimo membro: os membros de um grupo tem
/// de ficar juntos na barra, senao a pilula fica a rotular abas que nao sao
/// dela.
fn join_context_group(tabs: &mut Vec<ContextTab>, group_id: u64, tab_index: usize) {
    if tab_index >= tabs.len() {
        return;
    }
    let mut tab = tabs.remove(tab_index);
    tab.group = Some(group_id);
    let target = tabs
        .iter()
        .rposition(|other| other.group == Some(group_id))
        .map(|last| last + 1)
        .unwrap_or(tabs.len());
    tabs.insert(target, tab);
}

/// Tira a aba do grupo. Um grupo que fique sem abas desaparece -- uma pilula
/// vazia so ocupava espaco e enganava.
fn leave_context_group(tabs: &mut [ContextTab], groups: &mut Vec<ContextGroup>, tab_index: usize) {
    if let Some(tab) = tabs.get_mut(tab_index) {
        tab.group = None;
    }
    prune_empty_groups(tabs, groups);
}

fn prune_empty_groups(tabs: &[ContextTab], groups: &mut Vec<ContextGroup>) {
    groups.retain(|group| tabs.iter().any(|tab| tab.group == Some(group.id)));
}

struct ComparatorState {
    views: Vec<ComparatorView>,
    expanded: Option<usize>,
    minimized: [bool; COMPARATOR_COLUMNS],
    weights: [f64; COMPARATOR_COLUMNS],
    split: Option<SplitView>,
    /// Abas/fontes agrupadas automaticamente pela IA que abriu cada link.
    contexts: [Vec<ContextTab>; COMPARATOR_COLUMNS],
    /// Grupos por coluna, na ordem em que aparecem na barra.
    groups: [Vec<ContextGroup>; COMPARATOR_COLUMNS],
    /// Contador dos ids de grupo. Nunca reutiliza.
    next_group_id: u64,
}

/// Janelas da palette nativa: o popup que desenha a caixa e o EDIT onde o
/// utilizador escreve. A fonte e nossa e morre com o popup.
struct PaletteWindow {
    popup: HWND,
    edit: HWND,
    font: *mut core::ffi::c_void,
}

/// O que o procedimento do EDIT da palette precisa e nao pode ir buscar ao
/// App. O App escreve aqui, ao abrir a palette, a coluna a que ela pertence
/// e se essa coluna esta em privado; e daqui, e so daqui, que a submissao
/// os le. A pagina remota nunca toca neste bloco -- e por isso que nao pode
/// forjar nem o destino nem o texto. Numa Box para o endereco ficar estavel
/// enquanto a subclasse o guardar.
struct PaletteHost {
    proxy: EventLoopProxy<UserEvent>,
    /// (coluna, privado) da palette aberta; `None` quando nao ha nenhuma.
    source: Cell<Option<(usize, bool)>>,
    /// Sobe a cada abertura. O pedido de fecho traz o valor que viu: se a
    /// palette entretanto ja e outra, o pedido vem de uma janela morta.
    generation: Cell<u64>,
}

/// O que a barra precisa de saber sobre o comparador, tirado do proprio
/// estado. Desenho e hit-testing chamam isto -- nunca montam o seu proprio.
fn bar_columns(comp: &ComparatorState) -> BarColumns {
    // Em ecra completo ou com a gaveta aberta o conteudo ja nao esta em
    // faixas por peso, por isso a barra tambem nao finge que esta: reparte-se
    // em partes iguais e nenhuma coluna vira chip.
    if comp.expanded.is_some() || comp.split.is_some() {
        return BarColumns {
            split_active: comp.split.is_some(),
            ..BarColumns::even(comp.views.len())
        };
    }
    BarColumns {
        count: comp.views.len(),
        weights: comp.weights,
        minimized: comp.minimized,
        split_active: false,
    }
}

/// Faixa horizontal de uma coluna visivel do comparador, em pixeis logicos.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ColumnSpan {
    index: usize,
    x: f64,
    width: f64,
}

/// Reparte a largura pelas colunas nao minimizadas na proporcao dos pesos; a
/// ultima fica com o resto para a soma fechar exatamente na largura. E a
/// unica fonte desta geometria: as WebViews, os divisores e a palette leem
/// todos daqui, por isso nunca discordam entre si.
fn visible_column_spans(
    logical_w: f64,
    columns: usize,
    weights: &[f64; COMPARATOR_COLUMNS],
    minimized: &[bool; COMPARATOR_COLUMNS],
) -> Vec<ColumnSpan> {
    let visible: Vec<usize> = (0..columns.min(COMPARATOR_COLUMNS))
        .filter(|index| !minimized[*index])
        .collect();
    let total_weight: f64 = visible
        .iter()
        .map(|index| weights[*index].max(0.05))
        .sum::<f64>()
        .max(0.05);
    let mut x = 0.0;
    visible
        .iter()
        .enumerate()
        .map(|(slot, index)| {
            let width = if slot + 1 == visible.len() {
                logical_w - x
            } else {
                logical_w * weights[*index].max(0.05) / total_weight
            };
            let span = ColumnSpan {
                index: *index,
                x,
                width,
            };
            x += width;
            span
        })
        .collect()
}

/// Pesos novos depois de o divisor `divider` (entre as colunas visiveis
/// `visible[divider]` e `visible[divider + 1]`) ser largado em `mouse_x`.
/// So o par vizinho se mexe: a soma dos pesos nao muda, e por isso as outras
/// colunas ficam exactamente onde estavam. O par nunca fecha abaixo de
/// `MIN_PANEL_WIDTH` -- ou de 45% da faixa do par, se ela for mais estreita
/// que dois minimos e um minimo fixo nao coubesse la dentro.
fn resized_weights(
    weights: &[f64; COMPARATOR_COLUMNS],
    visible: &[usize],
    divider: usize,
    mouse_x: f64,
    logical_w: f64,
) -> [f64; COMPARATOR_COLUMNS] {
    let mut next = *weights;
    if divider + 1 >= visible.len() {
        return next;
    }
    let total_weight: f64 = visible
        .iter()
        .map(|index| weights[*index].max(0.05))
        .sum::<f64>()
        .max(0.05);
    let left_index = visible[divider];
    let right_index = visible[divider + 1];
    let before_weight: f64 = visible[..divider]
        .iter()
        .map(|index| weights[*index].max(0.05))
        .sum();
    let pair_weight = weights[left_index].max(0.05) + weights[right_index].max(0.05);
    let left_edge = logical_w * before_weight / total_weight;
    let pair_span = logical_w * pair_weight / total_weight;
    if pair_span <= 1.0 {
        return next;
    }
    let min_width = MIN_PANEL_WIDTH.min(pair_span * 0.45);
    let left_width = (mouse_x - left_edge).clamp(min_width, pair_span - min_width);
    let left_weight = pair_weight * left_width / pair_span;
    next[left_index] = left_weight.max(0.05);
    next[right_index] = (pair_weight - left_weight).max(0.05);
    next
}

/// Geometria da palette (logicos) para a faixa da sua coluna: centrada nela,
/// a uma fraccao fixa da altura util abaixo da barra, nunca mais larga que a
/// coluna menos as margens.
fn palette_geometry(span: ColumnSpan, logical_h: f64) -> UiRect {
    let width = PALETTE_MAX_WIDTH
        .min(span.width - 48.0)
        .max(PALETTE_MIN_WIDTH);
    let content_h = (logical_h - COMPARATOR_CHROME_HEIGHT).max(100.0);
    UiRect {
        x: span.x + (span.width - width) / 2.0,
        y: COMPARATOR_CHROME_HEIGHT + content_h * PALETTE_TOP_RATIO,
        width,
        height: PALETTE_HEIGHT,
    }
}

/// Legenda por baixo do campo: diz para onde vai cada tipo de entrada.
fn palette_hint(source_name: &str, private: bool) -> String {
    if private {
        "URL → abre ao lado, em privado   ·   texto → pergunta em privado   ·   Esc fecha"
            .to_string()
    } else {
        format!("URL → abre ao lado   ·   texto → envia para {source_name}   ·   Esc fecha")
    }
}

const EM_SETSEL: u32 = 0x00B1;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_SETCUEBANNER: u32 = 0x1501;
const EM_SETMARGINS: u32 = 0x00D3;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_CHAR: u32 = 0x0102;

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
/// Mensagens privadas usadas somente pelo gate de lifecycle. Usamos
/// RegisterWindowMessageW em vez de IDs fixos em WM_APP para não colidir com
/// mensagens privadas do winit/WRY/WebView2. O script registra os mesmos nomes,
/// então Windows resolve os dois processos para os mesmos IDs de mensagem.
static LIFECYCLE_PROBE_HOME_MESSAGE: OnceLock<u32> = OnceLock::new();
static LIFECYCLE_PROBE_REOPEN_MESSAGE: OnceLock<u32> = OnceLock::new();
static LIFECYCLE_PROBE_READY_MESSAGE: OnceLock<u32> = OnceLock::new();
static LIFECYCLE_COMPARATOR_READY: AtomicBool = AtomicBool::new(false);

fn lifecycle_probe_home_message() -> u32 {
    *LIFECYCLE_PROBE_HOME_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.Home"))
    })
}

fn lifecycle_probe_reopen_message() -> u32 {
    *LIFECYCLE_PROBE_REOPEN_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.Reopen"))
    })
}

fn lifecycle_probe_ready_message() -> u32 {
    *LIFECYCLE_PROBE_READY_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.Ready"))
    })
}
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
const TAB_MENU_NEW_GROUP: usize = 6;
const TAB_MENU_UNGROUP: usize = 7;
/// Os grupos ja existentes ocupam ids a partir daqui, um por grupo da coluna.
const TAB_MENU_GROUP_BASE: usize = 100;
const SPLITTER_SUBCLASS_BASE: usize = 0x4E60;
const SPLITTER_WIDTH: f64 = 7.0;
const MIN_PANEL_WIDTH: f64 = 180.0;

/// Ultima posicao pedida pelo arrasto de um divisor, e se ja ha um pedido por
/// atender. O rato manda WM_MOUSEMOVE a mais de 100 Hz e cada um reposiciona
/// tres WebView2: enfileirar um evento por movimento enche a fila de pedidos
/// que nascem velhos e o divisor fica a arrastar-se atras do cursor. Em vez
/// disso a subclasse escreve SEMPRE aqui a posicao mais recente e so acorda o
/// event loop quando nao ha nenhum pedido pendente -- o que chega ao handler
/// e o estado de agora, nao o de ha dez eventos.
static RESIZE_DIVIDER: AtomicUsize = AtomicUsize::new(0);
static RESIZE_X: AtomicI32 = AtomicI32::new(0);
static RESIZE_PENDING: AtomicBool = AtomicBool::new(false);
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

fn lifecycle_probe_enabled() -> bool {
    std::env::var_os("NEURALIA_LIFECYCLE_PROBE").is_some()
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
    fn RegisterWindowMessageW(lp_string: *const u16) -> u32;
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
    reference_data: usize,
) -> LRESULT {
    let lifecycle_home = lifecycle_probe_home_message();
    let lifecycle_reopen = lifecycle_probe_reopen_message();
    let lifecycle_ready = lifecycle_probe_ready_message();
    if message == lifecycle_ready {
        return if lifecycle_probe_enabled() && LIFECYCLE_COMPARATOR_READY.load(Ordering::Acquire) {
            1
        } else {
            0
        };
    }
    if message == lifecycle_home || message == lifecycle_reopen {
        if lifecycle_probe_enabled() && reference_data != 0 {
            let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
            if message == lifecycle_home {
                let _ = proxy.send_event(UserEvent::HomeRequested);
            } else {
                let input = startup_input();
                if !input.is_empty() {
                    let _ = proxy.send_event(UserEvent::SubmitText(input));
                }
            }
        }
        return 0;
    }

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
        // A classe STATIC responde HTTRANSPARENT quando nao tem SS_NOTIFY: o
        // sistema entrega entao o rato a janela de baixo -- aqui, o WebView2.
        // Sem esta linha nenhum WM_LBUTTONDOWN chega, o SetCapture nunca corre
        // e o arrasto do divisor e codigo morto. O botao de saida (1299) e a
        // palette (1410) ja carregavam este mesmo override.
        WM_NCHITTEST => return HTCLIENT as LRESULT,
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            return 0;
        }
        WM_MOUSEMOVE => {
            if GetCapture() == hwnd {
                let mut point = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut point) != 0 {
                    let divider = subclass_id.saturating_sub(SPLITTER_SUBCLASS_BASE);
                    // Publicar antes de marcar o pedido: quem for atende-lo ja
                    // encontra a posicao nova.
                    RESIZE_DIVIDER.store(divider, Ordering::Release);
                    RESIZE_X.store(point.x, Ordering::Release);
                    if !RESIZE_PENDING.swap(true, Ordering::AcqRel) {
                        let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                        if proxy.send_event(UserEvent::ResizeComparator).is_err() {
                            // Ninguem vai limpar a marca; sem isto o arrasto
                            // ficava mudo para sempre depois de um erro.
                            RESIZE_PENDING.store(false, Ordering::Release);
                        }
                    }
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
    if message == lifecycle_probe_home_message() && lifecycle_probe_enabled() && reference_data != 0
    {
        let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
        SetWindowTextW(hwnd, windows_sys::w!(""));
        let _ = proxy.send_event(UserEvent::HomeRequested);
        return 0;
    }

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

/// Popup da palette. Pinta a caixa e a legenda e da ao EDIT filho as cores
/// da omnibox. E uma janela de topo propria pela mesma razao que o botao
/// de sair: nada pintado pela janela principal aparece por cima do WebView2.
unsafe extern "system" fn palette_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    match message {
        // STATIC devolve HTTRANSPARENT: o clique atravessava o popup e ia
        // parar ao WebView por baixo, roubando o foco ao EDIT.
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_CTLCOLOREDIT => {
            let theme = Theme::system();
            let hdc = wparam as *mut core::ffi::c_void;
            SetTextColor(hdc, rgb3(theme.fg));
            SetBkColor(hdc, rgb3(theme.surface));
            omnibox_brush(theme.surface) as LRESULT
        }
        // O WM_PAINT cobre o cliente todo; apagar antes so faria piscar.
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let theme = Theme::system();
                    let scale = ((client.bottom - client.top) as f64 / PALETTE_HEIGHT).max(1.0);

                    // Rebordo de 1px: enche-se com a cor da linha e volta a
                    // encher-se por dentro; a regiao arredondada trata dos cantos.
                    let line = CreateSolidBrush(rgb3(theme.surface_line));
                    FillRect(hdc, &client, line);
                    DeleteObject(line as _);
                    let border = scale.round().max(1.0) as i32;
                    let inner = RECT {
                        left: client.left + border,
                        top: client.top + border,
                        right: client.right - border,
                        bottom: client.bottom - border,
                    };
                    let background = CreateSolidBrush(rgb3(theme.surface));
                    FillRect(hdc, &inner, background);
                    DeleteObject(background as _);

                    let font = create_font((-11.0 * scale) as i32, FW_NORMAL as i32);
                    let old_font = SelectObject(hdc, font as _);
                    SetBkMode(hdc, TRANSPARENT as i32);
                    SetTextColor(hdc, rgb3(theme.fg_muted));
                    let text = PALETTE_HINT
                        .lock()
                        .map(|value| value.clone())
                        .unwrap_or_default();
                    let mut hint = RECT {
                        left: (PALETTE_PAD_X * scale) as i32,
                        top: (PALETTE_HINT_TOP * scale) as i32,
                        right: client.right - (PALETTE_PAD_X * scale) as i32,
                        bottom: client.bottom - (8.0 * scale) as i32,
                    };
                    draw_text(
                        hdc,
                        &text,
                        &mut hint,
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                    );
                    SelectObject(hdc, old_font);
                    DeleteObject(font as _);
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

/// EDIT da palette. So aqui se le o que o utilizador escreveu: o texto sai
/// do controlo nativo com `window_text`, e a coluna e a privacidade vem do
/// `PaletteHost` que o App preencheu ao abrir. Nenhum dos tres passa pela
/// pagina, que nem sequer sabe que a palette existe.
unsafe extern "system" fn palette_edit_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    let host = &*(reference_data as *const PaletteHost);
    match message {
        WM_KEYDOWN => {
            let ctrl = (GetAsyncKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0;
            match wparam as u16 {
                VK_RETURN => {
                    let input = window_text(hwnd);
                    if let Some((source_index, private)) = host.source.get() {
                        let _ = host.proxy.send_event(UserEvent::PaletteSubmit {
                            source_index,
                            input,
                            private,
                        });
                    }
                    return 0;
                }
                VK_ESCAPE => {
                    let _ = host
                        .proxy
                        .send_event(UserEvent::ClosePalette(host.generation.get()));
                    return 0;
                }
                // Um EDIT de uma linha nao trata Ctrl+A sozinho (ver omnibox).
                0x41 if ctrl => {
                    SendMessageW(hwnd, EM_SETSEL, 0, -1);
                    return 0;
                }
                _ => {}
            }
        }
        // O caracter de Enter/Escape ja foi tratado acima; deixa-lo chegar
        // ao EDIT fazia o sistema apitar a cada submissao.
        WM_CHAR if wparam == 13 || wparam == 27 => return 0,
        // Clicar fora (no WebView, na barra, noutra janela) fecha a palette:
        // nao fica uma caixa fantasma sobre uma coluna que entretanto mudou.
        WM_KILLFOCUS => {
            let _ = host
                .proxy
                .send_event(UserEvent::ClosePalette(host.generation.get()));
        }
        _ => {}
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

/// Um prazo na fila de temporizadores. A ordem e a do prazo; a igualdade de
/// prazos desempata pela ordem de chegada, para dois pedidos feitos no mesmo
/// instante sairem na ordem em que foram feitos.
struct TimerEntry<E> {
    deadline: Instant,
    seq: u64,
    event: E,
}

impl<E> PartialEq for TimerEntry<E> {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.seq == other.seq
    }
}

impl<E> Eq for TimerEntry<E> {}

impl<E> PartialOrd for TimerEntry<E> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<E> Ord for TimerEntry<E> {
    /// Invertida de proposito: o `BinaryHeap` e um max-heap e queremos que o
    /// prazo MAIS PROXIMO fique no topo.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

/// Fila de prazos, generica no evento para se poder testar sem `UserEvent`
/// (que nao e `Ord` nem precisa de ser). Nao sabe nada de threads: quem a
/// usa decide quando chamar `pop_due` e quanto dormir ate `next_deadline`.
struct TimerQueue<E> {
    heap: BinaryHeap<TimerEntry<E>>,
    next_seq: u64,
}

impl<E> TimerQueue<E> {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    fn push(&mut self, deadline: Instant, event: E) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.heap.push(TimerEntry {
            deadline,
            seq,
            event,
        });
    }

    /// O prazo mais proximo, ou `None` com a fila vazia.
    fn next_deadline(&self) -> Option<Instant> {
        self.heap.peek().map(|entry| entry.deadline)
    }

    /// Retira o evento mais proximo se o seu prazo ja venceu em `now`. Um de
    /// cada vez, para quem chama poder despachar entre chamadas.
    fn pop_due(&mut self, now: Instant) -> Option<E> {
        if self.heap.peek().is_some_and(|entry| entry.deadline <= now) {
            self.heap.pop().map(|entry| entry.event)
        } else {
            None
        }
    }
}

/// Servico unico de temporizadores da interface. Antes, cada aviso, cada
/// passo de zoom (`set_zoom` -> `show_splash`), cada sonda do Gmail e cada
/// avanco da rolagem criava uma thread do sistema operativo so para dormir e
/// devolver um evento; a barra em ecra completo tinha ainda uma thread a
/// sondar de 300 em 300 ms. Agora ha UMA thread, `neural-timers`, que dorme
/// exactamente ate ao prazo mais proximo e entrega o evento ao event loop. Os
/// tokens de invalidacao continuam do lado de quem agenda: um evento que chega
/// tarde e ignorado por quem o recebe, como sempre foi.
struct Timers {
    shared: Arc<(Mutex<TimerQueue<UserEvent>>, Condvar)>,
    /// So para o fallback: a thread de servico tem a sua propria copia.
    proxy: EventLoopProxy<UserEvent>,
    alive: bool,
}

impl Timers {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let shared = Arc::new((Mutex::new(TimerQueue::new()), Condvar::new()));
        let worker = Arc::clone(&shared);
        let worker_proxy = proxy.clone();

        let spawned = thread::Builder::new()
            .name("neural-timers".into())
            .spawn(move || {
                let (lock, wake) = &*worker;
                loop {
                    let due = {
                        let mut queue =
                            lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        let now = Instant::now();
                        let mut due = Vec::new();
                        while let Some(event) = queue.pop_due(now) {
                            due.push(event);
                        }
                        if due.is_empty() {
                            // Dorme ate ao proximo prazo, ou ate alguem
                            // agendar um; `after` acorda-nos para reavaliar,
                            // porque o novo prazo pode ser mais proximo.
                            match queue.next_deadline() {
                                Some(deadline) => {
                                    let _guard = wake
                                        .wait_timeout(
                                            queue,
                                            deadline.saturating_duration_since(now),
                                        )
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                }
                                None => {
                                    let _guard = wake
                                        .wait(queue)
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                }
                            }
                        }
                        due
                    };
                    // Fora do lock: `after` nunca fica a espera de uma entrega.
                    for event in due {
                        let _ = worker_proxy.send_event(event);
                    }
                }
            });

        Self {
            shared,
            proxy,
            alive: spawned.is_ok(),
        }
    }

    /// Entrega `event` ao event loop passado `delay`. Se a thread de servico
    /// nao pode ser criada no arranque, recorre a uma thread excepcional por
    /// pedido -- o comportamento antigo -- para nao deixar um aviso preso no
    /// ecra; e o mesmo compromisso de `HistoryWriter::clear`.
    fn after(&self, delay: Duration, event: UserEvent) {
        if self.alive {
            let (lock, wake) = &*self.shared;
            let mut queue = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.push(Instant::now() + delay, event);
            wake.notify_one();
            return;
        }

        let proxy = self.proxy.clone();
        let _ = thread::Builder::new()
            .name("neural-timer-fallback".into())
            .spawn(move || {
                thread::sleep(delay);
                let _ = proxy.send_event(event);
            });
    }
}

enum HistoryCommand {
    Append(HistoryEntry),
    Clear,
    /// Le as `n` entradas mais recentes e devolve-as por `HistoryLoaded`.
    Recent(usize),
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
                        HistoryCommand::Recent(limit) => {
                            let result = worker_store
                                .recent(limit)
                                .map_err(|error| error.to_string());
                            let _ = worker_proxy.send_event(UserEvent::HistoryLoaded(result));
                        }
                    }
                }
            });
        Self { tx, store, proxy }
    }

    /// Ler o historico e lock + leitura do ficheiro inteiro: nao se faz no
    /// event loop. Como em `clear`, o pedido nao pode ser perdido -- o
    /// utilizador esta a espera da caixa -- por isso, com o worker saturado,
    /// recorre-se a uma thread excepcional; so se essa tambem falhar e que o
    /// erro volta ja, para ser mostrado no lugar da lista.
    fn recent(&self, limit: usize) -> Option<Result<Vec<HistoryEntry>, String>> {
        if self.tx.try_send(HistoryCommand::Recent(limit)).is_ok() {
            return None;
        }

        let store = self.store.clone();
        let proxy = self.proxy.clone();
        match thread::Builder::new()
            .name("neural-history-recent".into())
            .spawn(move || {
                let result = store.recent(limit).map_err(|error| error.to_string());
                let _ = proxy.send_event(UserEvent::HistoryLoaded(result));
            }) {
            Ok(_) => None,
            Err(error) => Some(Err(format!(
                "não consegui criar a thread de leitura: {error}"
            ))),
        }
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
        // Um rectangulo sem area nao contem nada. As caixas por preencher da
        // barra ficam em (0,0) com lado zero e, sem esta guarda, o canto
        // superior esquerdo da janela acertava em todas elas ao mesmo tempo.
        self.width > 0.0
            && self.height > 0.0
            && x >= self.x
            && x <= self.x + self.width
            && y >= self.y
            && y <= self.y + self.height
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
        // `f64::clamp` entra em panico se o minimo for maior que o maximo, e
        // isso acontece sempre que a altura e inferior a 490*scale -- em
        // particular com altura ZERO, que e o que o winit reporta quando a
        // janela e minimizada (`WM_SIZE` sem filtro de `SIZE_MINIMIZED`). O
        // `with_min_inner_size` nao protege este caso: a minimizacao nao passa
        // pelo `WM_GETMINMAXINFO`. Numa janela dessas nao se desenha nada, mas
        // tambem nao se pode morrer a calcular onde.
        let row_top = 310.0 * scale;
        let row_bottom = (height - 180.0 * scale).max(row_top);
        let row_y = (height * 0.54).clamp(row_top, row_bottom);

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

#[derive(Debug, Clone)]
enum BrowserAgentCommand {
    Search(String),
    Click(String),
    Select { label: String, value: String },
    Extract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentTermination {
    Completed,
    UserStopped,
    Limit,
    ElementMissing,
    RestrictedAction,
    UserRejected,
    ExecutionError,
}

impl AgentTermination {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::UserStopped => "user-stopped",
            Self::Limit => "limit",
            Self::ElementMissing => "element-missing",
            Self::RestrictedAction => "restricted-action",
            Self::UserRejected => "user-rejected",
            Self::ExecutionError => "execution-error",
        }
    }
}

/// O aviso que o utilizador vê quando o agente pára, e por quantos segundos.
/// Ficam ao lado da razão para não se dizer uma coisa no trace e outra no ecrã.
fn agent_stop_message(reason: AgentTermination) -> &'static str {
    match reason {
        AgentTermination::Completed => "Agente concluiu a sequência.",
        AgentTermination::UserStopped => "Agente interrompido.",
        AgentTermination::Limit => "Agente interrompido pelo limite de execução.",
        AgentTermination::ElementMissing => "Agente não encontrou o elemento solicitado.",
        AgentTermination::RestrictedAction => "Ação restrita: controle devolvido ao usuário.",
        AgentTermination::UserRejected => "Ação do agente cancelada.",
        AgentTermination::ExecutionError => "Agente parou por erro de execução.",
    }
}

fn agent_stop_seconds(reason: AgentTermination) -> u64 {
    match reason {
        AgentTermination::Completed | AgentTermination::UserRejected => 3,
        AgentTermination::RestrictedAction => 5,
        _ => 4,
    }
}

struct BrowserAgentState {
    goal: String,
    commands: Vec<BrowserAgentCommand>,
    next_command: usize,
    steps: usize,
    started: Instant,
    policy: AgentPermissionPolicy,
    trace: Vec<String>,
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
    /// rato empurra-o sem agendar nada; quando o `HideChrome` agendado chega,
    /// `hide_chrome` compara com isto e reagenda so o que falta. Antes era uma
    /// thread do sistema operativo POR CADA evento de rato, depois uma thread
    /// de vigia a sondar de 300 em 300 ms; agora e so um numero.
    chrome_deadline: u64,
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
    /// Popup nativo da palette (Ctrl+K/T ou +), so enquanto esta aberta.
    palette: Option<PaletteWindow>,
    /// Estado nativo lido pela subclasse do EDIT da palette.
    palette_host: Box<PaletteHost>,
    config: CoreConfig,
    history: HistoryWriter,
    memory: MemoryWorker,
    /// Todos os prazos da interface (avisos, sondas, rolagem, barra) passam
    /// por aqui: uma thread para a aplicacao inteira.
    timers: Timers,
    current_research: Option<ResearchSession>,
    active_agent: Option<BrowserAgentState>,
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
    /// Janela inteiramente tapada por outra (ou minimizada), segundo o
    /// `WindowEvent::Occluded`. Animar nesse estado e gastar bateria a pintar
    /// pixeis que ninguem chega a ver.
    home_occluded: bool,
    /// Janela com o foco do teclado (`WindowEvent::Focused`). Em segundo plano
    /// a animacao continua, mas devagar.
    home_focused: bool,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let config = CoreConfig::default();
        let history_store =
            HistoryStore::with_limit(config.data_dir.join("history.jsonl"), config.history_limit);
        let history = HistoryWriter::new(history_store, proxy.clone());
        let memory = MemoryWorker::new(config.data_dir.join("memory"), proxy.clone());
        let timers = Timers::new(proxy.clone());
        let reader_client = ReaderClient::new(config.reader_timeout_secs, config.reader_max_bytes);
        let navigation_generation = Arc::new(AtomicU64::new(0));
        let reader = ReaderWorker::new(
            reader_client,
            proxy.clone(),
            Arc::clone(&navigation_generation),
        );
        let omnibox_proxy = Box::new(proxy.clone());
        let palette_host = Box::new(PaletteHost {
            proxy: proxy.clone(),
            source: Cell::new(None),
            generation: Cell::new(0),
        });
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
            chrome_deadline: 0,
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
            palette: None,
            palette_host,
            config,
            history,
            memory,
            timers,
            current_research: None,
            active_agent: None,
            reader,
            surface: Surface::Home,
            navigation_generation,
            status: None,
            cursor: (-1.0, -1.0),
            next_home_frame: Instant::now(),
            // A janela nasce visivel e com foco; os eventos corrigem se nao for.
            home_occluded: false,
            home_focused: true,
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

    /// A janela ganhou ou perdeu o foco do teclado: muda o ritmo da animacao
    /// da Home (66 ms com foco, 250 ms sem) e arruma as janelas auxiliares,
    /// que so fazem sentido enquanto a app esta a frente.
    fn on_focus_changed(&mut self, focused: bool) {
        if self.home_focused == focused {
            return;
        }
        self.home_focused = focused;
        if focused {
            self.resume_home_animation();
            // Os popups owned reaparecem com o dono, mas a geometria pode ter
            // mudado enquanto estivemos fora (outro ecra, outro DPI, outra
            // maximizacao), por isso recalcula-se em vez de se confiar nela.
            self.sync_comparator_splitters();
            self.sync_exit_button();
            return;
        }
        // Sem foco nao ha o que arrastar nem de onde sair: as auxiliares que
        // so servem o rato saem da frente ate a janela voltar.
        self.hide_comparator_splitters();
        self.hide_exit_button();
    }

    /// A janela ficou inteiramente tapada (ou deixou de estar). Enquanto esta
    /// tapada nao se pinta nada.
    fn on_occluded_changed(&mut self, occluded: bool) {
        if self.home_occluded == occluded {
            return;
        }
        self.home_occluded = occluded;
        if !occluded {
            self.resume_home_animation();
        }
    }

    /// Ao voltar a ser vista, a Home repinta ja: o prazo do frame seguinte
    /// pode ter ficado a 250 ms de distancia, e esperar por ele daria a
    /// sensacao de uma janela congelada.
    fn resume_home_animation(&mut self) {
        if self.surface != Surface::Home {
            return;
        }
        self.next_home_frame = Instant::now();
        self.request_redraw();
    }

    /// Reinstala a subclasse da janela principal depois de transições de
    /// decoração. No Windows, alternar a moldura pode substituir o HWND nativo;
    /// SetWindowSubclass é idempotente para o mesmo callback/id e atualiza o
    /// reference_data quando a janela continua a mesma.
    fn ensure_window_subclass(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(parent) = window_hwnd(window) else {
            return;
        };
        let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
        unsafe {
            SetWindowSubclass(parent, Some(window_subclass), WINDOW_SUBCLASS_ID, proxy_ptr);
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

            SetWindowSubclass(parent, Some(window_subclass), WINDOW_SUBCLASS_ID, proxy_ptr);

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
        if lifecycle_probe_enabled() {
            LIFECYCLE_COMPARATOR_READY.store(false, Ordering::Release);
        }
        self.mark_dirty();
        self.close_palette();
        self.finish_agent(AgentTermination::UserStopped);

        // Derruba as superfícies WebView ANTES de alterar fullscreen/decoração.
        // No Windows, essas transições podem substituir ou reparentear o HWND
        // principal. Fazer a troca de chrome com controllers ainda vivos deixa
        // hosts WRY_WEBVIEW da segunda abertura presos ao HWND anterior e eles
        // reaparecem sobre a Home mesmo depois do drop.
        if let Some(comparator) = self.comparator.take() {
            for view in &comparator.views {
                let _ = view.webview.set_visible(false);
                let _ = view.webview.focus_parent();
            }
            if let Some(split) = &comparator.split {
                let _ = split.webview.set_visible(false);
                let _ = split.webview.focus_parent();
            }
            drop(comparator);
        }
        if let Some(webview) = self.webview.take() {
            let _ = webview.set_visible(false);
            let _ = webview.focus_parent();
            drop(webview);
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

        // O drop dos controllers pode concluir a destruição dos HWNDs WRY no
        // pump de mensagens seguinte. Restaurar a decoração aqui, no mesmo
        // stack, pode reparentear esses hosts para o novo HWND da Home e deixá-los
        // visíveis a partir da segunda abertura. Deixe o event loop respirar
        // antes de trocar o chrome nativo.
        self.leave_fullscreen();
        self.timers
            .after(Duration::from_millis(40), UserEvent::RestoreHomeDecorations);
        if let Ok(mut bytes) = self.pdf_bytes.lock() {
            *bytes = Vec::new();
        }
        self.reading_pdf = false;
    }

    fn show_home(&mut self) {
        self.next_generation();
        self.surface = Surface::Home;

        // Home é uma fronteira de ciclo de vida real. Destruir os controllers
        // aqui garante que nenhum host WRY_WEBVIEW sobreviva oculto/reparentado
        // entre pesquisas. Reuso dentro do próprio comparador continua possível,
        // mas sair para Home sempre encerra as superfícies web.
        self.destroy_web_surfaces();

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

    /// Pede a lista ao worker; a caixa aparece quando `HistoryLoaded` voltar.
    /// A leitura (lock + ficheiro inteiro) nunca corre no event loop.
    fn show_recent_history(&self) {
        if let Some(result) = self.history.recent(HISTORY_RECENT_LIMIT) {
            self.show_history_entries(result);
        }
    }

    fn show_history_entries(&self, result: Result<Vec<HistoryEntry>, String>) {
        let text = match result {
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
        match route_input(&input) {
            InputRoute::Agent(spec) => self.start_browser_agent(&spec),
            InputRoute::MemoryQuery(query) => {
                self.memory.query(query);
                self.show_splash("Buscando na memória local…".to_string(), 2);
            }
            InputRoute::MemoryRebuild => {
                self.memory.rebuild();
                self.show_splash("Reconstrução da memória agendada.".to_string(), 3);
            }
            InputRoute::History => self.show_recent_history(),
            InputRoute::ResearchCompare => self.compare_current_research(),
            InputRoute::ResearchSynthesize => self.synthesize_current_research(),
            InputRoute::ResearchExport => self.export_current_research(),
            InputRoute::Intent => match parse_intent(&input) {
                Ok(Intent::Home) => self.show_home(),
                Ok(Intent::Ask(query)) => self.ask(query),
                Ok(Intent::Compare(query)) => self.compare(query),
                Ok(Intent::Read(url)) => self.read(url.to_string()),
                Ok(Intent::Web(url)) => self.web(url.to_string()),
                Err(error) => self.show_native_error(error.to_string()),
            },
        }
    }

    fn submit_current(&mut self) {
        let input = self.omnibox_text();
        if !input.is_empty() {
            self.handle_input(input);
        }
    }

    fn current_research_item_ids(&self) -> Vec<String> {
        self.current_research
            .as_ref()
            .map(|session| {
                session
                    .items
                    .iter()
                    .filter(|item| {
                        matches!(
                            item.kind,
                            ResearchItemKind::Source | ResearchItemKind::ProviderAnswer
                        )
                    })
                    .map(|item| item.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn compare_current_research(&self) {
        let Some(session) = &self.current_research else {
            self.show_native_text(
                "NeuralIA — Research Session",
                "Nenhuma sessão de pesquisa está ativa.",
            );
            return;
        };
        let ids = self.current_research_item_ids();
        let facts = session.comparison(&ids);
        let text = if facts.is_empty() {
            "Ainda não há fontes/respostas suficientes para comparar.".to_string()
        } else {
            facts
                .into_iter()
                .map(|fact| {
                    format!(
                        "{}\nEntidades: {}\nNúmeros: {}\nDatas: {}",
                        fact.source,
                        fact.entities.join(", "),
                        fact.numbers.join(", "),
                        fact.dates.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\r\n\r\n")
        };
        self.show_native_text("NeuralIA — Comparação da pesquisa", &text);
    }

    fn synthesize_current_research(&mut self) {
        let ids = self.current_research_item_ids();
        let Some(session) = &mut self.current_research else {
            self.show_native_text(
                "NeuralIA — Research Session",
                "Nenhuma sessão de pesquisa está ativa.",
            );
            return;
        };
        if ids.is_empty() {
            self.show_native_text(
                "NeuralIA — Síntese",
                "Ainda não há fontes/respostas para sintetizar.",
            );
            return;
        }
        let snapshot = session.synthesize(&ids).clone();
        self.memory.save_session(session.clone());
        self.show_native_text("NeuralIA — Síntese com proveniência", &snapshot.output);
    }

    fn export_current_research(&mut self) {
        let Some(session) = &self.current_research else {
            self.show_native_text(
                "NeuralIA — Research Session",
                "Nenhuma sessão de pesquisa está ativa.",
            );
            return;
        };
        let dir = self.config.data_dir.join("research-exports");
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.show_splash(format!("Export: {error}"), 4);
            return;
        }
        let path = dir.join(format!("{}.md", session.id));
        match std::fs::write(&path, session.export_markdown()) {
            Ok(()) => self.show_native_text(
                "NeuralIA — Pesquisa exportada",
                &format!("Markdown salvo em:\r\n{}", path.display()),
            ),
            Err(error) => self.show_splash(format!("Export: {error}"), 4),
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
        let ipc_proxy = self.proxy.clone();
        let navigation_proxy = self.proxy.clone();
        let bytes = Arc::clone(&self.pdf_bytes);
        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let init_script = NEURALIA_KEYMAP_SCRIPT.replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
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
        let navigation_proxy = self.proxy.clone();
        let ipc_proxy = self.proxy.clone();
        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let init_script = format!("{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}")
            .replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
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

    fn external_webview_builder(
        &self,
        local_origin: Option<String>,
        agent_enabled: bool,
    ) -> WebViewBuilder<'static> {
        let ipc_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let nav_origin = local_origin.clone();
        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let agent_script = if agent_enabled {
            AGENT_OBSERVER_SCRIPT
        } else {
            ""
        };
        let init_script =
            format!("{NEURALIA_KEYMAP_SCRIPT}\n{EXTERNAL_RETURN_BUTTON}\n{agent_script}")
                .replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                let event = match action {
                    IpcAction::AgentObservation { data } if agent_enabled => {
                        parse_agent_observation(&data).map(UserEvent::AgentObservation)
                    }
                    other => common_ipc_event(other),
                };
                if let Some(event) = event {
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
                if remote_web_target(&target, local_origin.as_deref()) {
                    let _ = new_window_proxy.send_event(UserEvent::OpenExternal(target));
                }
                NewWindowResponse::Deny
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(true)
    }

    fn start_browser_agent(&mut self, spec: &str) {
        let (url, commands) = match parse_browser_agent_plan(spec) {
            Ok(plan) => plan,
            Err(error) => {
                self.show_native_error(error);
                return;
            }
        };
        let Ok(valid) = neural_core::validate_web_url(&url) else {
            self.show_native_error("Agent: URL inválida.");
            return;
        };
        if neural_core::is_local_network_target(&valid) {
            self.show_native_error("Agent: destinos locais/privados não são permitidos.");
            return;
        }

        self.destroy_web_surfaces();
        self.show_omnibox(false);
        let origin = valid.origin().ascii_serialization();
        let mut policy = AgentPermissionPolicy::new(Some(origin));
        policy.grant_reversible_session_actions(true);
        self.active_agent = Some(BrowserAgentState {
            goal: spec.to_string(),
            commands,
            next_command: 0,
            steps: 0,
            started: Instant::now(),
            policy,
            trace: vec![format!("navigate {}", valid)],
        });

        let result = if let Some(window) = &self.window {
            self.external_webview_builder(None, true)
                .with_url(valid.as_str())
                .build(window)
        } else {
            self.active_agent = None;
            return;
        };

        match result {
            Ok(webview) => {
                let _ = webview.zoom(self.zoom);
                self.webview = Some(webview);
                self.surface = Surface::External;
                self.record(
                    HistoryKind::Web,
                    format!("agent:{}", spec),
                    valid.to_string(),
                );
                self.show_splash(
                    "Agente iniciado. Esc/Home interrompe imediatamente.".to_string(),
                    4,
                );
            }
            Err(error) => {
                self.active_agent = None;
                self.show_native_error(format!("Agent WebView: {error}"));
            }
        }
    }

    fn handle_agent_observation(&mut self, page: ObservedPage) {
        let Some(agent) = self.active_agent.as_ref() else {
            return;
        };
        let (commands, next_command, steps, elapsed) = (
            agent.commands.clone(),
            agent.next_command,
            agent.steps,
            agent.started.elapsed(),
        );

        let decision = {
            let Some(agent) = self.active_agent.as_mut() else {
                return;
            };
            decide_agent_step(
                &commands,
                next_command,
                steps,
                elapsed,
                &page,
                &mut agent.policy,
            )
        };

        match decision {
            AgentStepDecision::Stop(reason) => {
                self.show_splash(
                    agent_stop_message(reason).to_string(),
                    agent_stop_seconds(reason),
                );
                self.finish_agent(reason);
            }
            AgentStepDecision::Extract => self.extract_agent_observation(&page),
            AgentStepDecision::Act(act) => {
                let AgentAct {
                    action,
                    security,
                    confirmation,
                } = *act;
                if let Some(reason) = confirmation {
                    let approved = self.confirm_agent_action(&reason, &action);
                    if let Some(agent) = self.active_agent.as_mut() {
                        agent.policy.record_user_confirmation(&security, approved);
                    }
                    if !approved {
                        self.show_splash("Ação do agente cancelada.".to_string(), 3);
                        self.finish_agent(AgentTermination::UserRejected);
                        return;
                    }
                }

                match self.execute_agent_action(&action) {
                    Ok(()) => {
                        if let Some(agent) = self.active_agent.as_mut() {
                            agent.trace.push(agent_trace_action(&action));
                            agent.next_command += 1;
                            agent.steps += 1;
                        }
                    }
                    Err(error) => {
                        self.show_splash(format!("Agent: {error}"), 4);
                        self.finish_agent(AgentTermination::ExecutionError);
                    }
                }
            }
        }
    }

    /// O comando `extract`: o texto observado vai para a memória semântica, já
    /// redigido, e o agente termina.
    fn extract_agent_observation(&mut self, page: &ObservedPage) {
        let clean = redact_sensitive_text(&page.text_excerpt);
        let mut document = MemoryDocument::new(
            MemoryKind::ResearchResult,
            MemorySourceKind::Web,
            if page.title.is_empty() {
                "Extração do agente".to_string()
            } else {
                page.title.clone()
            },
            Some(page.url.clone()),
            clean.clone(),
        );
        if let Some(session) = &self.current_research {
            document = document.session(session.id.clone());
        }
        self.memory.capture(document);
        if let Some(agent) = &mut self.active_agent {
            agent.trace.push(format!(
                "extract {} chars from {}",
                clean.chars().count(),
                page.url
            ));
            agent.next_command += 1;
            agent.steps += 1;
        }
        self.show_native_text("NeuralIA Agent — Extração", &clean);
        self.finish_agent(AgentTermination::Completed);
    }

    fn execute_agent_action(&self, action: &AgentAction) -> Result<(), String> {
        let Some(webview) = &self.webview else {
            return Err("nenhuma página ativa".into());
        };
        let script = agent_action_script(action)?;
        webview
            .evaluate_script(&script)
            .map_err(|error| error.to_string())
    }

    fn confirm_agent_action(&self, reason: &str, action: &AgentAction) -> bool {
        let (Some(window), Some(hwnd)) = (&self.window, self.window.as_ref().and_then(window_hwnd))
        else {
            return false;
        };
        let _ = window;
        let body = wide_null(&format!(
            "O agente quer executar uma ação sensível.\n\n{reason}\n\n{action:?}\n\nAutorizar uma única vez?"
        ));
        let title = wide_null("NeuralIA — Confirmação do agente");
        unsafe {
            MessageBoxW(
                hwnd,
                body.as_ptr(),
                title.as_ptr(),
                MB_YESNO | MB_ICONINFORMATION,
            ) == IDYES
        }
    }

    fn finish_agent(&mut self, reason: AgentTermination) {
        let Some(agent) = self.active_agent.take() else {
            return;
        };

        let root = self.config.data_dir.join("agent");
        let _ = std::fs::create_dir_all(&root);
        let stamp = now_ms();
        let _ = std::fs::write(
            root.join(format!("trace-{stamp}.log")),
            format!(
                "termination: {}\ngoal: {}\nsteps: {}\n{}\n",
                reason.as_str(),
                redact_sensitive_text(&agent.goal),
                agent.steps,
                agent.trace.join("\n")
            ),
        );
        let _ = agent
            .policy
            .write_audit_log(root.join(format!("audit-{stamp}.json")));
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
        let reuse_comparator = self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.views.len() == COMPARATOR_COLUMNS);
        if !reuse_comparator {
            self.destroy_web_surfaces();
        }
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
        self.ensure_window_subclass();

        if reuse_comparator {
            let urls = [
                google_url.as_str(),
                chatgpt_url.as_str(),
                claude_url.as_str(),
            ];
            let mut reload_error = None;
            if let Some(comparator) = &mut self.comparator {
                comparator.expanded = None;
                comparator.minimized = [false; COMPARATOR_COLUMNS];
                comparator.weights = [1.0; COMPARATOR_COLUMNS];
                if let Some(split) = comparator.split.take() {
                    let _ = split.webview.set_visible(false);
                    let _ = split.webview.focus_parent();
                    drop(split);
                }
                comparator.contexts = std::array::from_fn(|_| Vec::new());
                comparator.groups = std::array::from_fn(|_| Vec::new());
                comparator.next_group_id = 1;
                for (view, url) in comparator.views.iter().zip(urls) {
                    let encoded = match serde_json::to_string(url) {
                        Ok(encoded) => encoded,
                        Err(error) => {
                            reload_error =
                                Some(format!("URL inválida ao reutilizar {}: {error}", view.name));
                            break;
                        }
                    };
                    let script = format!("window.location.replace({encoded});");
                    if let Err(error) = view.webview.evaluate_script(&script) {
                        reload_error = Some(format!(
                            "WebView2 não pôde reutilizar {}: {error}",
                            view.name
                        ));
                        break;
                    }
                }
            }
            if let Some(error) = reload_error {
                self.show_native_error(error);
                return;
            }
            self.activate_comparator(false);
            return;
        }

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
            groups: std::array::from_fn(|_| Vec::new()),
            next_group_id: 1,
        });
        self.activate_comparator(true);
    }

    fn activate_comparator(&mut self, sync_remote_buttons: bool) {
        self.bar_hover = None;
        self.surface = Surface::Comparator;

        // build_as_child nasce antes de self.comparator existir, portanto o
        // primeiro layout feito durante a construcao nao pode passar pela
        // rotina que tambem torna cada controller visivel. Reaplicar aqui e
        // essencial também ao reutilizar controllers estacionados na Home.
        self.needs_clear = true;
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        // Na reutilização acabámos de iniciar três navegações. Executar outro
        // script remoto aqui pode manter WebView2 dentro do pump aninhado e
        // impedir o callback de devolver o controlo ao winit. O relayout de
        // 40 ms sincroniza os botões depois que o event loop já respirou.
        if sync_remote_buttons {
            self.sync_comparator_buttons();
        }
        self.sync_exit_button();

        for delay_ms in COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS {
            self.timers.after(
                Duration::from_millis(delay_ms),
                UserEvent::RelayoutComparator,
            );
        }

        self.schedule_gmail_probe(4);
        self.begin_reading_session(false);
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
                let spans = visible_column_spans(
                    logical_w,
                    comp.views.len(),
                    &comp.weights,
                    &comp.minimized,
                );
                for span in &spans {
                    let v = &comp.views[span.index];
                    let _ = v.webview.set_bounds(wry::Rect {
                        position: LogicalPosition::new(span.x, content_y).into(),
                        size: LogicalSize::new(span.width.max(1.0), content_h).into(),
                    });
                    let _ = v.webview.set_visible(true);
                }

                for (index, v) in comp.views.iter().enumerate() {
                    if comp.minimized[index] {
                        let _ = v.webview.set_visible(false);
                    }
                }
            }
        }
    }

    /// Que evento nasce de uma mensagem vinda da coluna `col_index`.
    ///
    /// Vive fora do closure do IPC de proposito. A decisao que aqui se toma --
    /// em especial a de um clique num link ir para a propria coluna ou para o
    /// painel lateral -- e a que o utilizador ve, e dentro de um closure de
    /// `WebViewBuilder` nao havia forma de a exercitar sem abrir uma janela.
    #[allow(clippy::needless_pass_by_value)]
    fn column_ipc_event_impl(col_index: usize, action: IpcAction) -> Option<UserEvent> {
        match action {
            IpcAction::ResearchAnswer { col, text } if col == col_index => {
                Some(UserEvent::ResearchAnswer {
                    source_index: col_index,
                    text,
                })
            }
            // Clique simples: a pagina abre nas TRES colunas, para se ver o
            // que cada IA diz dela. Ctrl+clique: abre no painel lateral e a
            // barra de titulo guarda a aba -- o "novo separador" do Chrome.
            IpcAction::Link { col, url, aside } if col == col_index => Some(if aside {
                UserEvent::OpenSplit {
                    source_index: col_index,
                    url,
                }
            } else {
                UserEvent::OpenEverywhere(url)
            }),
            IpcAction::Split { col, url } if col == col_index => Some(UserEvent::OpenSplit {
                source_index: col_index,
                url,
            }),
            IpcAction::Palette { col } if col == col_index => {
                Some(UserEvent::OpenPalette(col_index))
            }
            IpcAction::Minimize { col } if col == col_index => {
                Some(UserEvent::MinimizeComparator(col_index))
            }
            IpcAction::NewTab { col: Some(col) } if col == col_index => {
                Some(UserEvent::NewTab(col_index))
            }
            IpcAction::Expand { col } => Some(UserEvent::ExpandComparator(col)),
            other => common_ipc_event(other),
        }
    }

    fn comparator_webview_builder(
        &self,
        col_index: usize,
        col_name: &'static str,
    ) -> WebViewBuilder<'static> {
        let ipc_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let capability = remote_capability();
        let ipc_capability = capability.clone();

        // Um script por chamada, e nao os tres concatenados num so. O
        // WebView2 executa cada script de inicializacao isoladamente: assim
        // uma excecao ao nivel de topo de um deles -- o `sessionStorage` do
        // auto-submit, por exemplo, que lanca com armazenamento particionado --
        // deixa de levar atras o COMPARATOR_INJECT_SCRIPT, e com ele os
        // cliques nos links e os controlos da coluna.
        let prelude = format!(
            "window.__neuralia_col_index = {col_index}; window.__neuralia_col_name = '{col_name}';"
        );
        let keymap = NEURALIA_KEYMAP_SCRIPT.replace("__NEURALIA_CAP__", &capability);
        let auto_submit = AI_AUTO_SUBMIT_SCRIPT.replace("__NEURALIA_CAP__", &capability);
        let inject = COMPARATOR_INJECT_SCRIPT.replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
            .with_initialization_script(prelude)
            .with_initialization_script(keymap)
            .with_initialization_script(auto_submit)
            .with_initialization_script(inject)
            .with_ipc_handler(move |request| {
                let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                let event = Self::column_ipc_event_impl(col_index, action);
                if let Some(event) = event {
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
                remote_web_target(&target, None) || is_view_source_target(&target, None)
            })
            .with_new_window_req_handler(move |target, _features| {
                // `about:blank` NAO. Muitos sites abrem uma ligacao com
                // `window.open('', '_blank')` e so depois atribuem o endereco
                // ao popup: o WebView2 levanta o pedido com `about:blank`, e
                // carregar isso na coluna apagava a conversa da IA e nao
                // abria link nenhum. Ficar quieto deixa a pagina como estava.
                if !target.eq_ignore_ascii_case("about:blank") && remote_web_target(&target, None) {
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

    /// A mesma pagina nas tres colunas.
    ///
    /// Nao mexe no comparador nem no painel lateral: so troca o endereco de
    /// cada coluna. Se alguma recusar -- uma coluna minimizada nao tem
    /// WebView --, as outras seguem na mesma; um clique num link nao pode
    /// desmontar a comparacao por causa de uma delas.
    fn open_everywhere(&mut self, url: String) {
        if self.surface != Surface::Comparator {
            self.web(url);
            return;
        }
        let Ok(valid) = neural_core::validate_web_url(&url) else {
            self.show_splash("URL da fonte inválida.".to_string(), 3);
            return;
        };
        if neural_core::is_local_network_target(&valid) {
            self.show_splash(
                "A página não pode redirecionar a fonte para a rede local.".to_string(),
                4,
            );
            return;
        }

        let target = valid.to_string();
        let Some(comp) = &self.comparator else {
            return;
        };
        for view in &comp.views {
            let _ = view.webview.load_url(&target);
        }
        self.request_redraw();
    }

    fn split_webview_builder(
        &self,
        source_index: usize,
        source_name: &'static str,
        local_origin: Option<String>,
        private: bool,
    ) -> WebViewBuilder<'static> {
        let ipc_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let init_script = format!(
            "window.__neuralia_col_index = {source_index}; window.__neuralia_col_name = '{source_name}';\n{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}"
        )
        .replace("__NEURALIA_CAP__", &capability);

        WebViewBuilder::new()
            .with_incognito(private)
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                let event = match action {
                    IpcAction::SplitClose => Some(UserEvent::CloseSplit),
                    IpcAction::SplitExpand => Some(UserEvent::ToggleSplitFullscreen),
                    IpcAction::Palette { col } if col == source_index => {
                        Some(UserEvent::OpenPalette(source_index))
                    }
                    IpcAction::NewTab { col: Some(col) } if col == source_index => {
                        Some(UserEvent::NewTab(source_index))
                    }
                    other => common_ipc_event(other),
                };
                if let Some(event) = event {
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
                remote_web_target(&target, local_origin.as_deref())
                    || is_view_source_target(&target, local_origin.as_deref())
            })
            .with_new_window_req_handler(move |target, _features| {
                if remote_web_target(&target, None) {
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

        if let Some((title, mut document)) = split_source_memory(&valid, source_name, private) {
            let value = valid.to_string();
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
            .split_webview_builder(
                source_index,
                source_name,
                allow_local.then(|| valid.origin().ascii_serialization()),
                private,
            )
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
                        if links.last().map(|tab| tab.url.as_str()) != Some(value.as_str()) {
                            links.push(ContextTab {
                                url: value,
                                group: None,
                            });
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
            self.open_ai_palette(source_index.min(COMPARATOR_COLUMNS - 1));
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

    /// URL de pergunta do fornecedor da coluna.
    fn provider_query_url(&self, source_index: usize, query: &str) -> neural_core::Result<Url> {
        match source_index {
            0 => google_ai_url(query, &self.config.language),
            1 => chatgpt_search_url(query),
            _ => claude_search_url(query),
        }
    }

    /// Entrada da palette nativa. `source_index` e `private` vem do estado
    /// nativo escrito ao abrir a palette, nunca da pagina; a decisao de rota
    /// e pura (`route_palette`) e testada sem janela.
    fn submit_palette(&mut self, source_index: usize, input: String, private: bool) {
        match route_palette(&input, source_index, private) {
            PaletteRoute::Invalid(message) => {
                if let Some(message) = message {
                    self.show_splash(message, 3);
                }
            }
            PaletteRoute::Home => self.show_home(),
            // allow_local: a URL foi digitada num controlo nativo, e entrada
            // do utilizador e nao da pagina (SPEC-0015). Em privado a fonte
            // abre privada: open_split_mode(private) nao grava memoria nem
            // abas.
            PaletteRoute::OpenSplit { url, private } => {
                self.open_split_mode(source_index, url.to_string(), true, private);
            }
            // Painel privado: a pergunta abre como fonte privada, nunca na
            // coluna normal (cookies normais) e nunca no historico.
            PaletteRoute::OpenPrivateProvider { query } => {
                match self.provider_query_url(source_index, &query) {
                    Ok(url) => self.open_split_mode(source_index, url.to_string(), false, true),
                    Err(error) => self.show_splash(error.to_string(), 3),
                }
            }
            PaletteRoute::LoadProvider { query } => {
                match self.provider_query_url(source_index, &query) {
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

        if self.splash.is_none() {
            unsafe {
                // Popup OWNED pela janela principal (`owner` em hWndParent), e
                // nao filha nem TOPMOST. Uma janela owned fica sempre acima do
                // dono e das filhas dele -- o WebView2 incluido -- e some com
                // ele quando a app vai para tras; o TOPMOST que aqui estava
                // punha este aviso por cima de TODAS as aplicacoes depois de
                // um Alt+Tab, que nunca foi o que se queria.
                let created = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
            }
        }

        self.position_splash();

        self.splash_token = self.splash_token.wrapping_add(1);
        self.timers.after(
            Duration::from_secs(seconds),
            UserEvent::HideSplash(self.splash_token),
        );
    }

    /// Centra o aviso no fundo da janela. Vive em coordenadas de ECRA: se
    /// so se calculasse ao nascer, arrastar a janela deixava-o para tras.
    fn position_splash(&self) {
        let (Some(window), Some(splash)) = (&self.window, self.splash) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SPLASH_WIDTH * scale).round() as i32;
        let height = (SPLASH_HEIGHT * scale).round() as i32;

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
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

        if self.gmail_toast.is_none() {
            unsafe {
                // Owned pela janela principal, como o splash: sobe acima do
                // WebView2 por ser owned, e nao acima do resto do ambiente de
                // trabalho -- um aviso de email nosso nao tem nada que tapar a
                // aplicacao de outra pessoa.
                let created = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
            }
        }

        self.position_gmail_toast();

        self.gmail_toast_token = self.gmail_toast_token.wrapping_add(1);
        self.timers.after(
            Duration::from_secs(GMAIL_TOAST_SECONDS),
            UserEvent::HideGmailToast(self.gmail_toast_token),
        );
    }

    /// Encosta o aviso ao canto inferior direito da janela. Como o splash,
    /// tem de ser refeito sempre que a janela se mexe.
    fn position_gmail_toast(&self) {
        let (Some(window), Some(toast)) = (&self.window, self.gmail_toast) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (GMAIL_TOAST_WIDTH * scale).round() as i32;
        let height = (GMAIL_TOAST_HEIGHT * scale).round() as i32;

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
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
        // Desligado por NEURALIA_NO_GMAIL nem se sonda: a sonda le cookies e
        // acabaria por criar o WebView que a variavel promete nao existir.
        if self.gmail_monitor.is_some() || !gmail_monitor_enabled() {
            return;
        }
        self.gmail_probe_token = self.gmail_probe_token.wrapping_add(1);
        self.timers.after(
            Duration::from_secs(seconds),
            UserEvent::GmailProbe(self.gmail_probe_token),
        );
    }

    fn maybe_start_gmail_monitor(&mut self) {
        if self.gmail_monitor.is_some()
            || !gmail_monitor_enabled()
            || !self.google_session_available()
        {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };

        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let proxy = self.proxy.clone();
        let init_script = GMAIL_MONITOR_SCRIPT.replace("__NEURALIA_CAP__", &capability);
        let bounds = wry::Rect {
            position: LogicalPosition::new(-10_000.0, -10_000.0).into(),
            size: LogicalSize::new(1.0, 1.0).into(),
        };

        let result = WebViewBuilder::new()
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                let Some(IpcAction::GmailState {
                    unread,
                    sender,
                    subject,
                    key,
                }) = parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                let _ = proxy.send_event(UserEvent::GmailInboxState {
                    unread,
                    sender,
                    subject,
                    key,
                });
            })
            .with_navigation_handler(move |target| {
                if target
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("neuralia:"))
                {
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
        // O corte do script nao conta: ele corre em mail.google.com.
        let sender = gmail_field(sender);
        let subject = gmail_field(subject);
        let key = gmail_field(key);
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
            // A pagina ja esta carregada: ver a fonte dela nao alarga nada.
            let current_origin = Url::parse(&current).ok().as_ref().and_then(local_origin_of);
            let target = format!("view-source:{current}");
            if is_view_source_target(&target, current_origin.as_deref()) {
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
        self.timers.after(
            Duration::from_secs(seconds),
            UserEvent::AutoScrollTick(self.auto_scroll_token),
        );
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
        // O mesmo plano que o desenho usa. Se aqui se contassem so as abas, o
        // rato acertaria noutro sitio que nao o que esta no ecra.
        Some(BarLayout::with_rows(
            window.inner_size().width as f64,
            window.scale_factor(),
            self.bar_visible(),
            bar_columns(comp),
            std::array::from_fn(|index| plan_tab_row(&comp.contexts[index], &comp.groups[index])),
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

        if self.exit_button.is_none() {
            unsafe {
                // Popup owned pela janela principal, e nao filha: uma filha
                // ficaria por baixo do WebView2 na ordem Z e nunca se veria.
                // Ser owned ja garante o lugar acima do dono e das filhas
                // dele; o TOPMOST so acrescentava ficar por cima das outras
                // aplicacoes depois de um Alt+Tab.
                let created = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
            }
        }

        self.position_exit_button();
    }

    /// Centrado no topo, logo abaixo da barra revelada. Em coordenadas de
    /// ecra, como as outras auxiliares: tem de seguir a janela que se arrasta.
    fn position_exit_button(&self) {
        let (Some(window), Some(button)) = (&self.window, self.exit_button) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (EXIT_BUTTON_WIDTH * scale).round() as i32;
        let height = (EXIT_BUTTON_HEIGHT * scale).round() as i32;

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
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

    /// Esconde o botao sem o destruir. Serve a perda de foco: a janela volta
    /// e o `sync_exit_button` decide de novo, sem recriar nada entretanto.
    fn hide_exit_button(&self) {
        if let Some(button) = self.exit_button {
            unsafe {
                ShowWindow(button, SW_HIDE);
            }
        }
    }

    /// Mostra a barra e marca-a para desaparecer sozinha. Cada chamada invalida
    /// o temporizador anterior, por isso ela fica enquanto o rato la andar.
    fn reveal_chrome(&mut self) {
        // Adiar e so escrever um numero; o rato mexe-se centenas de vezes por
        // segundo e nao se agenda nada por movimento.
        self.chrome_deadline = now_ms() + CHROME_HIDE_DELAY_MS;

        if self.chrome_revealed {
            return;
        }

        self.chrome_revealed = true;
        self.chrome_token = self.chrome_token.wrapping_add(1);
        self.timers.after(
            Duration::from_millis(CHROME_HIDE_DELAY_MS),
            UserEvent::HideChrome(self.chrome_token),
        );

        self.update_comparator_layout();
        self.sync_exit_button();
        self.request_redraw();
    }

    fn hide_chrome(&mut self, token: u64) {
        if token != self.chrome_token || !self.chrome_revealed {
            return;
        }
        // O rato empurrou o prazo desde que este pedido foi agendado: em vez
        // de sondar, reagenda-se exactamente o que falta, com o mesmo token.
        let remaining = self.chrome_deadline.saturating_sub(now_ms());
        if remaining > 0 {
            self.timers.after(
                Duration::from_millis(remaining),
                UserEvent::HideChrome(token),
            );
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
        // Todo o divisor que nao couber na geometria de agora e escondido
        // abaixo, um a um: uma transicao 3 -> 2 -> 1 colunas, ou Comparator
        // -> Split View, nunca deixa um splitter da geometria anterior a
        // vista. Antes escondiam-se todos aqui em cima e mostravam-se logo a
        // seguir -- o que piscava a cada WM_MOVE agora que esta funcao
        // tambem corre quando a janela e arrastada.
        let Some(window) = &self.window else {
            self.hide_comparator_splitters();
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            self.hide_comparator_splitters();
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
            // Um divisor por fronteira entre colunas visiveis: o fim de cada
            // faixa menos a ultima, na mesma geometria que as WebViews usam.
            let spans =
                visible_column_spans(logical_w, comp.views.len(), &comp.weights, &comp.minimized);
            let boundaries: Vec<f64> = spans
                .iter()
                .take(spans.len().saturating_sub(1))
                .map(|span| span.x + span.width)
                .collect();
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
                if let Some(hwnd) = self.splitters[slot] {
                    unsafe {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                }
                continue;
            }

            let hwnd = match self.splitters[slot] {
                Some(hwnd) => hwnd,
                None => unsafe {
                    let width = (SPLITTER_WIDTH * scale).round().max(3.0) as i32;
                    let height = (content_height * scale).round().max(1.0) as i32;
                    // Owned pela janela principal: e o que o poe acima dos
                    // WebView2 (filhas do dono) sem o pousar sobre o ambiente
                    // de trabalho inteiro. Um divisor a flutuar por cima de
                    // outra aplicacao era o que o TOPMOST daqui fazia.
                    let created = CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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

        comp.weights = resized_weights(&comp.weights, &visible, divider, mouse_x, logical_w);

        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    /// Controlos da direita tal como estao desenhados AGORA, ou `None` se a
    /// barra nao estiver a ser mostrada. A geometria vem toda de
    /// `right_controls`: nao ha uma segunda copia da conta por aqui.
    fn right_controls(&self) -> Option<RightControls> {
        let window = self.window.as_ref()?;
        if self.surface != Surface::Comparator || !self.bar_visible() {
            return None;
        }
        let split_active = self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some());
        Some(right_controls(
            window.inner_size().width as f64,
            window.scale_factor().max(1.0),
            split_active,
        ))
    }

    fn private_bar_rect(&self) -> Option<UiRect> {
        Some(self.right_controls()?.private)
    }

    fn split_bar_rects(&self) -> Option<(UiRect, UiRect, UiRect)> {
        self.right_controls()?.split
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

    /// Abre a palette nativa sobre a coluna `source_index`. E um popup Win32,
    /// nao um <input> no DOM: a pagina remota nem ve o que se escreve nem
    /// consegue submeter nada por ela.
    fn open_ai_palette(&mut self, source_index: usize) {
        if source_index >= COMPARATOR_COLUMNS || self.comparator.is_none() {
            return;
        }
        // A privacidade e a da fonte aberta ao lado desta coluna -- lida ANTES
        // de a fechar, porque e ela que decide para onde vai a submissao.
        let private = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| split.source_index == source_index && split.private);
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
        // Uma coluna minimizada nao tem faixa onde a palette possa pousar.
        if let Some(comp) = &mut self.comparator
            && comp.minimized[source_index]
        {
            comp.minimized[source_index] = false;
            self.update_comparator_layout();
            self.sync_comparator_splitters();
            self.sync_comparator_buttons();
        }
        self.show_palette(source_index, private);
    }

    /// Coluna e privacidade a que a palette aberta esta ligada: estado
    /// nativo, escrito por nos ao abrir, nunca pela pagina.
    fn palette_source(&self) -> Option<(usize, bool)> {
        self.palette_host.source.get()
    }

    /// Cria o popup da palette (owned pela janela principal, sem
    /// NOACTIVATE porque precisa de foco, sem TOPMOST porque nao e um aviso)
    /// com o EDIT dentro, e poe-lhe o foco.
    fn show_palette(&mut self, source_index: usize, private: bool) {
        // Uma palette de cada vez: a anterior (talvez noutra coluna) morre e
        // esta nasce ja com a geometria e a fonte do DPI atual.
        self.close_palette();
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let Some(source_name) = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(source_index))
            .map(|view| view.name)
        else {
            return;
        };
        let scale = window.scale_factor().max(1.0);

        if let Ok(mut slot) = PALETTE_HINT.lock() {
            *slot = palette_hint(source_name, private);
        }

        let created = unsafe {
            let popup = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                WS_POPUP,
                0,
                0,
                10,
                10,
                owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if popup.is_null() {
                return;
            }
            if SetWindowSubclass(popup, Some(palette_subclass), PALETTE_SUBCLASS_ID, 0) == 0 {
                DestroyWindow(popup);
                return;
            }
            // Sem WS_EX_CLIENTEDGE, como a omnibox: a caixa e a do popup.
            let edit = CreateWindowExW(
                0,
                windows_sys::w!("EDIT"),
                windows_sys::w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                0,
                0,
                10,
                10,
                popup,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if edit.is_null() {
                DestroyWindow(popup);
                return;
            }
            let cue = wide_null("Pergunte à IA ativa ou digite uma URL");
            SendMessageW(edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
            SendMessageW(edit, EM_SETLIMITTEXT, 2048, 0);
            let host_ptr = (&*self.palette_host as *const PaletteHost) as usize;
            if SetWindowSubclass(
                edit,
                Some(palette_edit_subclass),
                PALETTE_EDIT_SUBCLASS_ID,
                host_ptr,
            ) == 0
            {
                DestroyWindow(popup);
                return;
            }
            let font = create_font(-((18.0 * scale).round() as i32), FW_NORMAL as i32);
            if !font.is_null() {
                SendMessageW(edit, WM_SETFONT, font as usize, 1);
            }
            let margin = (6.0 * scale) as usize;
            SendMessageW(
                edit,
                EM_SETMARGINS,
                EC_LEFTMARGIN | EC_RIGHTMARGIN,
                ((margin << 16) | margin) as isize,
            );
            PaletteWindow { popup, edit, font }
        };

        let (popup, edit) = (created.popup, created.edit);
        self.palette = Some(created);
        self.palette_host.source.set(Some((source_index, private)));
        self.palette_host
            .generation
            .set(self.palette_host.generation.get().wrapping_add(1));
        self.position_palette();
        unsafe {
            ShowWindow(popup, SW_SHOW);
            InvalidateRect(popup, std::ptr::null(), 1);
            SetFocus(edit);
        }
    }

    /// Centra a palette sobre a faixa da sua coluna. Tudo em logicos e so
    /// depois escalado: a mesma geometria em qualquer DPI.
    fn position_palette(&self) {
        let (Some(window), Some(palette), Some((source_index, _))) =
            (&self.window, &self.palette, self.palette_source())
        else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;
        // Coluna sem faixa (minimizada, ou o layout mudou por baixo da
        // palette): usa-se a largura toda em vez de a esconder.
        let span = self
            .comparator
            .as_ref()
            .filter(|comp| comp.split.is_none() && comp.expanded.is_none())
            .and_then(|comp| {
                visible_column_spans(logical_w, comp.views.len(), &comp.weights, &comp.minimized)
                    .into_iter()
                    .find(|span| span.index == source_index)
            })
            .unwrap_or(ColumnSpan {
                index: source_index,
                x: 0.0,
                width: logical_w,
            });
        let geometry = palette_geometry(span, logical_h);
        let width = (geometry.width * scale).round() as i32;
        let height = (geometry.height * scale).round() as i32;

        let mut origin = POINT { x: 0, y: 0 };
        unsafe {
            ClientToScreen(owner, &mut origin);
            SetWindowPos(
                palette.popup,
                std::ptr::null_mut(),
                origin.x + (geometry.x * scale).round() as i32,
                origin.y + (geometry.y * scale).round() as i32,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let corner = (PALETTE_CORNER * scale).round() as i32;
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, corner, corner);
            if !region.is_null() {
                SetWindowRgn(palette.popup, region, 1);
            }
            let pad_x = (PALETTE_PAD_X * scale).round() as i32;
            SetWindowPos(
                palette.edit,
                std::ptr::null_mut(),
                pad_x,
                (PALETTE_EDIT_TOP * scale).round() as i32,
                (width - pad_x * 2).max(1),
                (PALETTE_EDIT_HEIGHT * scale).round() as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// Fecha a palette. Limpa primeiro o host: o WM_KILLFOCUS que a
    /// destruicao provoca ja nao encontra coluna nenhuma para submeter.
    fn close_palette(&mut self) {
        let source = self.palette_host.source.replace(None);
        let Some(palette) = self.palette.take() else {
            return;
        };
        // Escape/Enter: o foco ainda esta no EDIT e volta para a coluna. Se o
        // utilizador clicou noutro sitio, o foco ja e desse sitio e fica la.
        let had_focus = unsafe { GetFocus() } == palette.edit;
        unsafe {
            DestroyWindow(palette.popup);
            if !palette.font.is_null() {
                DeleteObject(palette.font as _);
            }
        }
        if had_focus
            && let Some((source_index, _)) = source
            && let Some(view) = self
                .comparator
                .as_ref()
                .and_then(|comp| comp.views.get(source_index))
        {
            let _ = view.webview.focus();
        }
    }

    fn context_tab_url(&self, source_index: usize, context_index: usize) -> Option<String> {
        self.comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .and_then(|tabs| tabs.get(context_index))
            .map(|tab| tab.url.clone())
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
            && context_index < comp.contexts[source_index].len()
        {
            comp.contexts[source_index].remove(context_index);
            prune_empty_groups(&comp.contexts[source_index], &mut comp.groups[source_index]);
        }
        self.request_redraw();
    }

    /// Um clique na pilula abre ou fecha o grupo. Fechado, as abas continuam
    /// abertas -- so deixam de ocupar a barra.
    fn toggle_context_group(&mut self, source_index: usize, group_index: usize) {
        if let Some(comp) = &mut self.comparator
            && let Some(group) = comp
                .groups
                .get_mut(source_index)
                .and_then(|groups| groups.get_mut(group_index))
        {
            group.collapsed = !group.collapsed;
        }
        self.request_redraw();
    }

    fn group_context_tab(&mut self, source_index: usize, context_index: usize) {
        if let Some(comp) = &mut self.comparator {
            let next_id = &mut comp.next_group_id;
            create_context_group(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                next_id,
                context_index,
            );
        }
        self.request_redraw();
    }

    fn join_context_tab_group(
        &mut self,
        source_index: usize,
        context_index: usize,
        group_index: usize,
    ) {
        if let Some(comp) = &mut self.comparator
            && let Some(id) = comp.groups[source_index]
                .get(group_index)
                .map(|group| group.id)
        {
            join_context_group(&mut comp.contexts[source_index], id, context_index);
            prune_empty_groups(&comp.contexts[source_index], &mut comp.groups[source_index]);
        }
        self.request_redraw();
    }

    fn ungroup_context_tab(&mut self, source_index: usize, context_index: usize) {
        if let Some(comp) = &mut self.comparator {
            leave_context_group(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
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
            // A aba que fica mantem o grupo a que pertencia.
            let group = tabs
                .iter()
                .find(|tab| tab.url == keep)
                .and_then(|tab| tab.group);
            tabs.clear();
            tabs.push(ContextTab { url: keep, group });
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

        // Lidos antes de abrir o menu: dentro do bloco `unsafe` ja nao ha
        // emprestimo do estado que sobreviva ao `TrackPopupMenu`.
        let (existing_groups, in_group) = self
            .comparator
            .as_ref()
            .map(|comp| {
                let names: Vec<String> = comp.groups[source_index]
                    .iter()
                    .map(|group| group.name.clone())
                    .collect();
                let member = comp.contexts[source_index]
                    .get(context_index)
                    .is_some_and(|tab| tab.group.is_some());
                (names, member)
            })
            .unwrap_or_default();

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
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            let new_group = wide_null("Novo grupo com esta aba");
            AppendMenuW(menu, MF_STRING, TAB_MENU_NEW_GROUP, new_group.as_ptr());
            // As entradas "juntar a" tem de sobreviver ao fim do bloco, senao
            // o Win32 le ponteiros ja libertados enquanto desenha o menu.
            let join_labels: Vec<Vec<u16>> = existing_groups
                .iter()
                .map(|name| wide_null(&format!("Juntar ao grupo \u{201C}{name}\u{201D}")))
                .collect();
            for (offset, label) in join_labels.iter().enumerate() {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    TAB_MENU_GROUP_BASE + offset,
                    label.as_ptr(),
                );
            }
            if in_group {
                let ungroup = wide_null("Remover do grupo");
                AppendMenuW(menu, MF_STRING, TAB_MENU_UNGROUP, ungroup.as_ptr());
            }

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
            TAB_MENU_NEW_GROUP => self.group_context_tab(source_index, context_index),
            TAB_MENU_UNGROUP => self.ungroup_context_tab(source_index, context_index),
            other if other >= TAB_MENU_GROUP_BASE => {
                let group_index = other - TAB_MENU_GROUP_BASE;
                if group_index < existing_groups.len() {
                    self.join_context_tab_group(source_index, context_index, group_index);
                }
            }
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
            Some(BarHit::ContextGroup {
                source_index,
                group_index,
            }) => self.toggle_context_group(source_index, group_index),
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

fn parse_browser_agent_plan(spec: &str) -> Result<(String, Vec<BrowserAgentCommand>), String> {
    let parts = spec
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some(url) = parts.first() else {
        return Err(
            "Use agent:https://site | search=texto | click=botão | select=filtro:valor | extract"
                .into(),
        );
    };
    neural_core::validate_web_url(url).map_err(|error| error.to_string())?;

    let mut commands = Vec::new();
    for raw in parts.iter().skip(1) {
        // Um valor vazio era descartado em silêncio. O plano seguia sem o
        // comando que o utilizador escreveu e, se fosse o único, o
        // `commands.is_empty()` lá em baixo punha um `extract` no lugar: um
        // `click=` mal escrito acabava a guardar a página na memória em vez de
        // clicar. Um comando que não dá para cumprir é um erro, não um salto.
        if let Some(value) = raw
            .strip_prefix("search=")
            .or_else(|| raw.strip_prefix("pesquisar="))
        {
            let value = value.trim();
            if value.is_empty() {
                return Err("search precisa do texto a procurar: search=termo".into());
            }
            commands.push(BrowserAgentCommand::Search(value.to_string()));
        } else if let Some(value) = raw
            .strip_prefix("click=")
            .or_else(|| raw.strip_prefix("clique="))
        {
            let value = value.trim();
            if value.is_empty() {
                return Err("click precisa do rótulo do elemento: click=Buscar".into());
            }
            commands.push(BrowserAgentCommand::Click(value.to_string()));
        } else if let Some(value) = raw
            .strip_prefix("select=")
            .or_else(|| raw.strip_prefix("selecionar="))
        {
            let Some((label, selected)) = value.split_once(':') else {
                return Err("select usa select=campo:valor".into());
            };
            let (label, selected) = (label.trim(), selected.trim());
            // Um rótulo vazio não é "qualquer campo": era o primeiro
            // `select`/`combobox` da página, escolhido por ordem do DOM.
            if label.is_empty() || selected.is_empty() {
                return Err("select usa select=campo:valor, com os dois preenchidos".into());
            }
            commands.push(BrowserAgentCommand::Select {
                label: label.to_string(),
                value: selected.to_string(),
            });
        } else if raw.eq_ignore_ascii_case("extract") || raw.eq_ignore_ascii_case("extrair") {
            commands.push(BrowserAgentCommand::Extract);
        } else {
            return Err(format!("comando de agente desconhecido: {raw}"));
        }
    }
    if commands.is_empty() {
        commands.push(BrowserAgentCommand::Extract);
    }
    Ok(((*url).to_string(), commands))
}

fn parse_agent_observation(data: &str) -> Option<ObservedPage> {
    let mut lines = data.lines();
    let generation = lines.next()?.parse::<u64>().ok()?;
    let url = lines.next()?.trim().to_string();
    let title = lines.next()?.trim().to_string();
    let text_excerpt = lines.next().unwrap_or_default().to_string();
    let origin = Url::parse(&url)
        .ok()
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or_else(|| url.clone());

    let mut elements = Vec::new();
    for line in lines.take(40) {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() < 5 {
            continue;
        }
        elements.push(AgentElement {
            id: fields[0].to_string(),
            generation,
            role: fields[1].to_string(),
            name: fields[2].to_string(),
            text: fields[2].to_string(),
            origin: origin.clone(),
            frame: "top".into(),
            visible: true,
            interactable: fields[4] == "1",
        });
    }

    Some(ObservedPage {
        generation,
        url,
        title,
        text_excerpt,
        elements,
    })
}

fn agent_field_kind(element: &AgentElement) -> FieldKind {
    let role = element.role.to_ascii_lowercase();
    if role.contains("password") {
        FieldKind::Password
    } else if role.contains("otp") {
        FieldKind::Otp
    } else if role.contains("card") || role.contains("payment") {
        FieldKind::PaymentCard
    } else if role.contains("email") {
        FieldKind::Email
    } else if role.contains("search") {
        FieldKind::Search
    } else if role.contains("input") || role.contains("textbox") || role.contains("textarea") {
        FieldKind::Text
    } else {
        FieldKind::Unknown
    }
}

fn find_agent_element<'a>(
    page: &'a ObservedPage,
    label: &str,
    select_only: bool,
) -> Option<&'a AgentElement> {
    let needle = label.to_lowercase();
    page.elements.iter().find(|element| {
        element.interactable
            && (!select_only
                || element.role.to_ascii_lowercase().contains("select")
                || element.role.to_ascii_lowercase().contains("combobox"))
            && (needle.is_empty()
                || element.name.to_lowercase().contains(&needle)
                || element.text.to_lowercase().contains(&needle))
    })
}

/// O que fazer com uma observação da página. Decidido sem tocar na UI, no
/// WebView nem na memória: os limites, a escolha do elemento e o gate da
/// política vivem aqui, e `handle_agent_observation` fica só com a execução do
/// que isto decidir.
///
/// A separação é o que torna a SPEC-0105 testável no código que embarca. O
/// `AgentRuntime` do `neural-core` -- sobre o qual corre
/// `spec_0105_agent_runtime_is_bounded_structured_and_human_gated` -- não é
/// usado por esta aplicação: o agente do produto é este. Enquanto a decisão
/// estivesse entalada entre `show_splash` e `evaluate_script`, nenhum teste
/// conseguia ficar vermelho quando o produto regredisse.
#[derive(Debug, Clone, PartialEq)]
enum AgentStepDecision {
    /// Terminar, com a razão que vai para o trace e para o utilizador.
    Stop(AgentTermination),
    /// O comando `extract`: guardar o texto observado e terminar.
    Extract,
    /// Executar a ação (em `Box` porque é muitas vezes maior do que as outras
    /// duas variantes).
    Act(Box<AgentAct>),
}

/// A ação aprovada pelo gate, com a razão da confirmação quando a política
/// exige um sim humano antes de ela acontecer.
#[derive(Debug, Clone, PartialEq)]
struct AgentAct {
    action: AgentAction,
    security: AgentSecurityAction,
    confirmation: Option<String>,
}

/// O orçamento do agente vem do `AgentRuntimeConfig::default()` da SPEC-0105 em
/// vez de números escritos à mão aqui: dois sítios com os mesmos limites
/// divergem sem nada os apanhar.
fn decide_agent_step(
    commands: &[BrowserAgentCommand],
    next_command: usize,
    steps: usize,
    elapsed: Duration,
    page: &ObservedPage,
    policy: &mut AgentPermissionPolicy,
) -> AgentStepDecision {
    let budget = AgentRuntimeConfig::default();
    if steps >= budget.max_steps || elapsed >= budget.max_wall_time {
        return AgentStepDecision::Stop(AgentTermination::Limit);
    }

    let Some(command) = commands.get(next_command) else {
        return AgentStepDecision::Stop(AgentTermination::Completed);
    };

    let action = match command {
        BrowserAgentCommand::Extract => return AgentStepDecision::Extract,
        BrowserAgentCommand::Search(value) => page
            .elements
            .iter()
            .find(|element| {
                element.interactable
                    && matches!(
                        agent_field_kind(element),
                        FieldKind::Search | FieldKind::Text
                    )
            })
            .cloned()
            .map(|target| AgentAction::TypeText {
                target,
                text: value.clone(),
                field: FieldKind::Search,
            }),
        BrowserAgentCommand::Click(label) => find_agent_element(page, label, false)
            .cloned()
            .map(|target| AgentAction::Click { target }),
        BrowserAgentCommand::Select { label, value } => find_agent_element(page, label, true)
            .cloned()
            .map(|target| AgentAction::Select {
                target,
                value: value.clone(),
            }),
    };

    let Some(action) = action else {
        return AgentStepDecision::Stop(AgentTermination::ElementMissing);
    };

    let security = app_agent_security_action(&action, page);
    let decision = policy.evaluate(&security);
    if decision.allowed {
        return AgentStepDecision::Act(Box::new(AgentAct {
            action,
            security,
            confirmation: None,
        }));
    }
    if decision.risk == ActionRisk::Restricted {
        return AgentStepDecision::Stop(AgentTermination::RestrictedAction);
    }
    if !decision.requires_confirmation {
        // Negada sem caminho de confirmação: fica no log de auditoria como
        // recusa, tal como a recusa explícita do utilizador.
        policy.record_user_confirmation(&security, false);
        return AgentStepDecision::Stop(AgentTermination::UserRejected);
    }
    AgentStepDecision::Act(Box::new(AgentAct {
        action,
        security,
        confirmation: Some(decision.reason),
    }))
}

/// O que uma fonte aberta em Split View deixa na memória semântica: o título e
/// o documento, ou **nada** quando o painel é privado.
///
/// "Private/incognito navigation never enters semantic memory" está na lista de
/// restrições inegociáveis do `md/README.md`. É aqui que isso se decide para
/// este caminho, fora de qualquer janela, para um teste poder ficar vermelho se
/// alguém inverter a condição.
fn split_source_memory(
    url: &Url,
    source_name: &str,
    private: bool,
) -> Option<(String, MemoryDocument)> {
    if private {
        return None;
    }
    let value = url.to_string();
    let title = url
        .host_str()
        .map(|host| format!("Fonte · {host}"))
        .unwrap_or_else(|| "Fonte Web".to_string());
    let document = MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Web,
        title.clone(),
        Some(value.clone()),
        value,
    )
    .provider(source_name);
    Some((title, document))
}

/// Para onde vai o texto que o utilizador submeteu, decidido sem tocar na
/// janela, na memória, na rede nem no agente. É a SPEC-0106 -- a composição do
/// produto -- num sítio onde um teste lhe consegue chegar.
#[derive(Debug, Clone, PartialEq, Eq)]
enum InputRoute {
    Agent(String),
    MemoryQuery(String),
    MemoryRebuild,
    History,
    ResearchCompare,
    ResearchSynthesize,
    ResearchExport,
    /// Sem comando próprio: segue para o `parse_intent`.
    Intent,
}

fn route_input(input: &str) -> InputRoute {
    // Os comandos exactos vêm ANTES dos prefixos. `memory:rebuild` começa por
    // `memory:`, por isso enquanto o prefixo foi testado primeiro o rebuild
    // nunca aconteceu: procurava-se a palavra "rebuild" na memória e
    // anunciava-se "Buscando na memória local…".
    let trimmed = input.trim();
    for (command, route) in [
        ("history:", InputRoute::History),
        ("research:compare", InputRoute::ResearchCompare),
        ("research:synthesize", InputRoute::ResearchSynthesize),
        ("research:export", InputRoute::ResearchExport),
        ("memory:rebuild", InputRoute::MemoryRebuild),
        ("mem:rebuild", InputRoute::MemoryRebuild),
    ] {
        if trimmed.eq_ignore_ascii_case(command) {
            return route;
        }
    }

    if let Some(spec) = input.strip_prefix("agent:") {
        return InputRoute::Agent(spec.trim().to_string());
    }
    if let Some(query) = input
        .strip_prefix("memory:")
        .or_else(|| input.strip_prefix("mem:"))
    {
        return InputRoute::MemoryQuery(query.trim().to_string());
    }
    InputRoute::Intent
}

fn app_agent_security_action(action: &AgentAction, page: &ObservedPage) -> AgentSecurityAction {
    let origin = Url::parse(&page.url)
        .ok()
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| page.url.clone());

    match action {
        AgentAction::Click { target } => {
            let label = target.name.to_lowercase();
            let role = target.role.to_lowercase();
            let material = format!("{role} {label}");
            if ["delete", "remove", "excluir", "apagar", "cancel account"]
                .iter()
                .any(|word| material.contains(word))
            {
                AgentSecurityAction::DeleteRemote {
                    origin,
                    description: target.name.clone(),
                }
            } else if [
                "buy",
                "purchase",
                "pay",
                "comprar",
                "pagar",
                "checkout",
                "transfer",
                "transferir",
                "subscribe",
                "assinar plano",
            ]
            .iter()
            .any(|word| material.contains(word))
            {
                AgentSecurityAction::Payment {
                    origin,
                    description: target.name.clone(),
                }
            } else if role.contains("submit")
                || [
                    "send",
                    "submit",
                    "confirm",
                    "enviar",
                    "confirmar",
                    "post",
                    "publish",
                    "publicar",
                    "save changes",
                    "salvar alterações",
                    "salvar alteracoes",
                    "create account",
                    "criar conta",
                    "authorize",
                    "autorizar",
                    "accept terms",
                    "aceitar termos",
                    "sign agreement",
                    "assinar acordo",
                    "finalize",
                    "finalizar",
                ]
                .iter()
                .any(|word| material.contains(word))
            {
                AgentSecurityAction::Submit {
                    origin,
                    description: target.name.clone(),
                }
            } else {
                AgentSecurityAction::Click {
                    origin,
                    label: target.name.clone(),
                }
            }
        }
        AgentAction::TypeText { field, text, .. } => AgentSecurityAction::TypeText {
            origin,
            field: *field,
            value_summary: format!("{} chars", text.chars().count()),
        },
        AgentAction::Select { target, value } => AgentSecurityAction::Click {
            origin,
            label: format!("select {} = {}", target.name, value),
        },
        AgentAction::Extract { .. } => AgentSecurityAction::Extract { origin },
        _ => AgentSecurityAction::Read { origin },
    }
}

fn agent_trace_action(action: &AgentAction) -> String {
    match action {
        AgentAction::TypeText {
            target,
            text,
            field,
        } => format!(
            "type target={} field={field:?} chars={}",
            target.id,
            text.chars().count()
        ),
        AgentAction::Select { target, value } => {
            format!(
                "select target={} chars={}",
                target.id,
                value.chars().count()
            )
        }
        AgentAction::Click { target } => format!(
            "click target={} label={}",
            target.id,
            redact_sensitive_text(&target.name)
        ),
        AgentAction::Extract { .. } => "extract".to_string(),
        AgentAction::Scroll { amount } => format!("scroll {amount}"),
        AgentAction::Submit { description, .. } => {
            format!("submit {}", redact_sensitive_text(description))
        }
        AgentAction::Navigate { url } => format!("navigate {}", redact_sensitive_text(url)),
        AgentAction::Wait { millis } => format!("wait {millis}ms"),
        AgentAction::AskUser { reason } => {
            format!("ask-user {}", redact_sensitive_text(reason))
        }
        AgentAction::Finish { summary } => {
            format!("finish {}", redact_sensitive_text(summary))
        }
    }
}

/// Percent-encoding para dentro de um literal JS que a pagina le com
/// `decodeURIComponent`. O serializador de formularios escreve o espaco como
/// `+` e `decodeURIComponent` nao o desfaz: um `+` aqui era um `+` escrito no
/// campo, e um nome com espacos nunca batia com o do DOM -- o guard do script
/// desistia em silencio e a accao do agente nao acontecia. Um `+` literal ja
/// chega como `%2B`, por isso os que sobram sao todos espacos.
fn js_percent(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn agent_action_script(action: &AgentAction) -> Result<String, String> {
    fn guard(target: &AgentElement) -> String {
        let id = js_percent(&target.id);
        let name = js_percent(&target.name);
        let role = js_percent(&target.role);
        format!(
            "const id=decodeURIComponent('{id}');const expectedName=decodeURIComponent('{name}');             const expectedRole=decodeURIComponent('{role}');             const el=document.querySelector('[data-neuralia-agent-id=\"'+id+'\"]');             if(!el)return;             const actualName=((el.getAttribute('aria-label')||el.name||el.innerText||el.textContent||'').replace(/\\s+/g,' ').trim().slice(0,96));             const actualRole=(el.getAttribute('role')||el.type||el.tagName||'').toLowerCase();             if(expectedName && actualName!==expectedName)return;             if(expectedRole && actualRole!==expectedRole)return;"
        )
    }

    let body = match action {
        AgentAction::TypeText { target, text, .. } => {
            let value = js_percent(text);
            format!(
                "{}const value=decodeURIComponent('{}');el.focus();                 const proto=Object.getPrototypeOf(el);const descriptor=Object.getOwnPropertyDescriptor(proto,'value');                 if(descriptor&&descriptor.set)descriptor.set.call(el,value);else el.value=value;                 el.dispatchEvent(new Event('input',{{bubbles:true}}));                 el.dispatchEvent(new Event('change',{{bubbles:true}}));",
                guard(target),
                value
            )
        }
        AgentAction::Click { target } => format!("{}el.click();", guard(target)),
        AgentAction::Select { target, value } => {
            let value = js_percent(value);
            format!(
                "{}el.value=decodeURIComponent('{}');                 el.dispatchEvent(new Event('input',{{bubbles:true}}));                 el.dispatchEvent(new Event('change',{{bubbles:true}}));",
                guard(target),
                value
            )
        }
        _ => return Err("ação ainda não executável pela bridge do navegador".into()),
    };

    Ok(format!(
        "(function(){{{body}window.dispatchEvent(new Event('neuralia-agent-rescan'));}})();"
    ))
}

/// Tecto do texto de um artigo que entra na memoria semantica.
const READER_MEMORY_MAX_BYTES: usize = 512 * 1024;

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

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attributes = Window::default_attributes()
            .with_title("NeuralIA")
            // Maximizada a abrir: e um browser, e o comparador de tres colunas
            // nao cabe com folga em 1120 px. O `inner_size` fica como o tamanho
            // de restauro, para quem carregar no botao do meio.
            .with_maximized(true)
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
        let interval = if self.surface == Surface::Home && home_animation_enabled() {
            // O `Occluded` do Windows nao cobre a minimizacao em todos os
            // casos, por isso pergunta-se tambem a janela.
            let minimized = self
                .window
                .as_ref()
                .and_then(|window| window.is_minimized())
                .unwrap_or(false);
            home_frame_interval(minimized, self.home_occluded, self.home_focused)
        } else {
            None
        };

        match interval {
            Some(interval) => {
                let now = Instant::now();
                if now >= self.next_home_frame {
                    self.next_home_frame = now + interval;
                    self.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_home_frame));
            }
            // Sem prazo nenhum: o laco dorme ate chegar um evento de verdade.
            None => event_loop.set_control_flow(ControlFlow::Wait),
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
            UserEvent::HistoryLoaded(result) => self.show_history_entries(result),
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
            UserEvent::ResearchAnswer { source_index, text } => {
                let provider = self
                    .comparator
                    .as_ref()
                    .and_then(|comp| comp.views.get(source_index))
                    .map(|view| view.name.to_string());
                if let (Some(provider), Some(session)) = (provider, &mut self.current_research) {
                    session.upsert_provider_answer(provider, text, None);
                    self.memory.save_session(session.clone());
                }
            }
            UserEvent::AgentObservation(page) => {
                self.handle_agent_observation(page);
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
            UserEvent::OpenEverywhere(url) => self.open_everywhere(url),
            UserEvent::OpenSplit { source_index, url } => {
                self.open_split(source_index, url, false);
            }
            UserEvent::OpenPrivateSplit { source_index, url } => {
                self.open_split_mode(source_index, url, false, true);
            }
            UserEvent::NewTab(index) => self.new_tab(index),
            UserEvent::CloseSplit => self.close_split(),
            UserEvent::ToggleSplitFullscreen => self.toggle_split_fullscreen(),
            UserEvent::OpenPalette(index) => {
                if self.surface == Surface::Comparator {
                    self.open_ai_palette(index);
                }
            }
            UserEvent::PaletteSubmit {
                source_index,
                input,
                private,
            } => {
                // So vale o que a palette aberta prometeu: se entretanto foi
                // fechada (a coluna pode ja nem existir), a entrada cai.
                if self.palette_source() == Some((source_index, private)) {
                    self.close_palette();
                    self.submit_palette(source_index, input, private);
                }
            }
            UserEvent::ClosePalette(generation) => {
                if generation == self.palette_host.generation.get() {
                    self.close_palette();
                }
            }
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
            UserEvent::ResizeComparator => {
                // Limpar a marca ANTES de ler: um movimento que chegue durante
                // o reposicionamento volta a enfileirar e nao se perde. Ao
                // contrario, um movimento entre a leitura e a limpeza seria
                // engolido e o divisor parava onde nao devia.
                RESIZE_PENDING.store(false, Ordering::Release);
                if self.surface == Surface::Comparator {
                    self.resize_comparator(
                        RESIZE_DIVIDER.load(Ordering::Acquire),
                        RESIZE_X.load(Ordering::Acquire),
                    );
                }
            }
            UserEvent::RelayoutComparator => {
                if self.surface == Surface::Comparator {
                    // set_decorations(false) pode substituir/reconfigurar o HWND
                    // depois de open_comparator() regressar. Reinstalar a subclass
                    // aqui prende os comandos nativos ao HWND que ficou realmente
                    // ativo, em vez de ao handle anterior da Home.
                    self.ensure_window_subclass();
                    self.needs_clear = true;
                    self.update_comparator_layout();
                    self.sync_comparator_splitters();
                    self.sync_comparator_buttons();
                    self.sync_exit_button();
                    if lifecycle_probe_enabled() {
                        // O probe só considera o comparador pronto depois de pelo
                        // menos um relayout pós-transição de decoração. Assim Home
                        // nunca é disparado contra um HWND ainda em substituição.
                        LIFECYCLE_COMPARATOR_READY.store(true, Ordering::Release);
                    }
                    self.request_redraw();
                }
            }
            UserEvent::RestoreHomeDecorations => {
                if self.surface == Surface::Home {
                    if let Some(window) = &self.window {
                        window.set_decorations(true);
                    }
                    self.ensure_window_subclass();
                    self.needs_clear = true;
                    self.position_omnibox();
                    self.request_redraw();
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
                    self.position_palette();
                    self.request_redraw();
                }
                _ => {}
            },
            // Splash, toast, botao de saida, divisores e palette sao popups em
            // coordenadas de ECRA: mover a janela nao lhes toca. Ate aqui so o
            // Resized os sincronizava, por isso arrastar a janela deixava-os
            // para tras, no sitio onde ela estava antes.
            WindowEvent::Moved(_) => {
                self.position_splash();
                self.position_gmail_toast();
                self.position_exit_button();
                self.position_palette();
                self.sync_comparator_splitters();
            }
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
            WindowEvent::Focused(focused) => self.on_focus_changed(focused),
            WindowEvent::Occluded(occluded) => self.on_occluded_changed(occluded),
            // O tema do sistema mudou: o cache de 1 s tem de cair agora, e o
            // fundo inteiro e repintado porque ate a cor da pagina mudou.
            WindowEvent::ThemeChanged(_) => {
                Theme::invalidate();
                self.needs_clear = true;
                self.request_redraw();
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
///
/// O documento e servido por faixas (RFC 9110, `Range`): o visualizador pede
/// `getDocument({url, rangeChunkSize})` e, mal ve `Accept-Ranges: bytes` e o
/// `Content-Length`, aborta o pedido inicial e passa a pedir so os bytes de
/// que precisa. Cada pedido copia apenas a sua fatia -- antes, cada GET
/// clonava o documento inteiro (ate 32 MiB) por baixo do Mutex.
fn serve_pdf_asset(
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
enum RangeParse {
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
fn parse_range(header: &str, total: u64) -> RangeParse {
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
fn parse_range_pos(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(text.bytes().fold(0u64, |value, byte| {
        value
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'))
    }))
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

fn common_ipc_event(action: IpcAction) -> Option<UserEvent> {
    Some(match action {
        IpcAction::Home => UserEvent::HomeRequested,
        IpcAction::Back => UserEvent::BackRequested,
        IpcAction::Restore => UserEvent::RestoreComparator,
        IpcAction::AutoScroll => UserEvent::ToggleAutoScroll,
        IpcAction::ZoomIn => UserEvent::ZoomIn,
        IpcAction::ZoomOut => UserEvent::ZoomOut,
        IpcAction::ZoomReset => UserEvent::ZoomReset,
        IpcAction::Reload => UserEvent::ReloadPage,
        IpcAction::Print => UserEvent::PrintPage,
        IpcAction::Omnibox => UserEvent::FocusOmnibox,
        IpcAction::History => UserEvent::ShowHistory,
        IpcAction::ClearHistory => UserEvent::ClearHistory,
        IpcAction::Fullscreen => UserEvent::ToggleColumnFullscreen,
        IpcAction::DevTools => UserEvent::OpenDevTools,
        IpcAction::ViewSource => UserEvent::ViewSource,
        IpcAction::NewTab { col } => UserEvent::NewTab(col.unwrap_or(0)),
        _ => return None,
    })
}
/// Para onde vai o que o utilizador escreveu na palette. Puro, para se poder
/// testar sem janela: e aqui que se decide que um painel privado nunca
/// carrega nada na coluna normal nem passa pelo historico.
#[derive(Debug, PartialEq)]
enum PaletteRoute {
    /// Nada a fazer; a mensagem, quando ha, e para mostrar ao utilizador.
    Invalid(Option<String>),
    Home,
    /// URL: abre ao lado da coluna, privada se a palette veio de um painel
    /// privado. A rede local e permitida porque a URL foi digitada.
    OpenSplit {
        url: Url,
        private: bool,
    },
    /// Texto numa coluna normal: a pergunta vai para o fornecedor da coluna
    /// e fica no historico.
    LoadProvider {
        query: String,
    },
    /// Texto num painel privado: a pergunta abre como fonte privada.
    OpenPrivateProvider {
        query: String,
    },
}

fn route_palette(input: &str, source_index: usize, private: bool) -> PaletteRoute {
    let input = input.trim();
    if input.is_empty() || source_index >= COMPARATOR_COLUMNS {
        return PaletteRoute::Invalid(None);
    }
    match parse_intent(input) {
        Ok(Intent::Read(url)) | Ok(Intent::Web(url)) => PaletteRoute::OpenSplit { url, private },
        Ok(Intent::Home) => PaletteRoute::Home,
        Ok(Intent::Ask(query)) | Ok(Intent::Compare(query)) => {
            if private {
                PaletteRoute::OpenPrivateProvider { query }
            } else {
                PaletteRoute::LoadProvider { query }
            }
        }
        Err(error) => PaletteRoute::Invalid(Some(error.to_string())),
    }
}

/// Token que so os scripts injetados conhecem: 128 bits do RNG do sistema.
/// Se o BCrypt falhar, o SipHash com semente aleatoria de antes entra a
/// misturar-se com o que houver no buffer, para nunca sair um token vazio.
fn capability_from_rng(status: i32, bytes: [u8; 16]) -> Option<String> {
    if status != 0 {
        return None;
    }

    use std::fmt::Write as _;
    let mut token = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    Some(token)
}

fn remote_capability() -> String {
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

    capability_from_rng(status, bytes)
        .expect("BCryptGenRandom failed; refusing to create an unauthenticated WebView capability")
}

/// A origem local que o utilizador autorizou ao escrever uma URL num controlo
/// nativo, ou `None` quando nao autorizou nenhuma.
///
/// Era um `bool`, e o handler de navegacao capturava-o para toda a vida da
/// WebView: autorizar `http://192.168.1.50:3000` abria a rede local INTEIRA a
/// essa pagina, que a partir daí podia navegar para o router ou para qualquer
/// outro host. O utilizador autorizou uma origem, nao uma rede.
fn local_origin_of(url: &Url) -> Option<String> {
    is_local_network_target(url).then(|| url.origin().ascii_serialization())
}

fn remote_web_target(target: &str, local_origin: Option<&str>) -> bool {
    if target.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    neural_core::validate_web_url(target).is_ok_and(|url| {
        !is_local_network_target(&url)
            || local_origin.is_some_and(|allowed| url.origin().ascii_serialization() == allowed)
    })
}

/// `view-source:` so e aceite sobre uma URL web que a propria superficie ja
/// deixaria abrir: a mesma politica de rede local, sem `about:` nem esquemas
/// aninhados.
fn is_view_source_target(target: &str, local_origin: Option<&str>) -> bool {
    let Some(rest) = target.strip_prefix("view-source:") else {
        return false;
    };
    neural_core::validate_web_url(rest).is_ok_and(|url| {
        !is_local_network_target(&url)
            || local_origin.is_some_and(|allowed| url.origin().ascii_serialization() == allowed)
    })
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

/// Ritmo da animacao da Home, ou `None` para nao animar de todo.
///
/// O laco pedia um frame a cada 66 ms enquanto a Home estivesse aberta, mesmo
/// com a janela minimizada, tapada por outra ou em segundo plano: 15 repinturas
/// GDI por segundo a gastar bateria sem ninguem a ver. A decisao esta aqui,
/// fora do `about_to_wait`, porque e a unica parte testavel sem event loop.
fn home_frame_interval(minimized: bool, occluded: bool, focused: bool) -> Option<Duration> {
    if minimized || occluded {
        return None;
    }
    // Em segundo plano a animacao nao para (a janela continua a ser vista),
    // mas 4 FPS chegam para nao parecer congelada.
    Some(Duration::from_millis(if focused { 66 } else { 250 }))
}

/// `NEURALIA_NO_GMAIL` desliga o monitor do Gmail por completo (SPEC-0005,
/// SECURITY.md): sem sonda, sem leitura de cookies, sem WebView escondido. O
/// gate de ciclo de vida define-a porque conta processos e nao distingue a
/// excepcao intencional de um vazamento.
fn gmail_monitor_enabled() -> bool {
    gmail_monitor_enabled_for(std::env::var_os("NEURALIA_NO_GMAIL"))
}

/// Basta a variavel EXISTIR, como em NEURALIA_REDUCE_MOTION: `=0` ou vazia
/// tambem desligam. Um interruptor de privacidade que dependesse do valor
/// deixava passar quem o definiu mal.
fn gmail_monitor_enabled_for(no_gmail: Option<OsString>) -> bool {
    no_gmail.is_none()
}

/// A tela do fundo da Home, com a zona limpa a volta da marca.
///
/// As contas vivem no `neural-core` e nao aqui: e o mesmo tecido que o
/// instalador desenha, e duas copias divergiam a primeira vez que uma fosse
/// afinada. O que fica deste lado e so o que e proprio da Home -- onde esta a
/// marca e quanto espaco ela reserva.
///
/// **Sem zona de silencio.** A marca vai para o ecra com o alfa dela, por
/// `AlphaBlend`, portanto os neuronios passam mesmo por tras dela -- que e o
/// efeito pedido. Qualquer zona limpa, por mais estreita, desenha uma mancha
/// escura a volta do logo; foi rejeitada duas vezes, como caixa e como oval.
fn home_tissue_field(width: f64, height: f64, scale: f64) -> tissue::Field {
    tissue::Field::new(width, height, scale)
}

/// Uma rede neuronal viva: neuronios a percorrer a tela, ligados aos vizinhos,
/// com impulsos a viajar pelas ligacoes e descargas onde dois se encontram.
///
/// A versao anterior lancava particulas das margens e fazia-as convergir TODAS
/// para o logo. O efeito era o contrario do pretendido: um amontoado a mexer
/// atras da marca, sem ligacoes estaveis e sem nada que se parecesse com
/// transmissao. E a versao a seguir a essa corrigiu o amontoado mas deixou-os
/// a oscilar nove pixeis, sem nunca chegarem ao vizinho -- um mobile, nao um
/// cerebro. Agora percorrem a sua celula, encontram-se, e o encontro e uma
/// descarga.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_neural_background(
    hdc: *mut core::ffi::c_void,
    width: f64,
    height: f64,
    scale: f64,
    theme: &Theme,
) {
    if width < 1.0 || height < 1.0 {
        return;
    }

    let field = home_tissue_field(width, height, scale);
    let seconds = now_ms() as f64 / 1000.0;
    draw_neural_tissue(hdc, &field, scale, seconds, theme);
}

/// Desenha as ligacoes, os impulsos, os neuronios e as descargas.
unsafe fn draw_neural_tissue(
    hdc: *mut core::ffi::c_void,
    field: &tissue::Field,
    scale: f64,
    seconds: f64,
    theme: &Theme,
) {
    let tissue::Tissue {
        nodes,
        branches,
        links,
        pulses,
        bursts,
    } = tissue::tissue_at(field, seconds);

    // A ramagem primeiro: e o que esta por tras de tudo. Tres canetas pela
    // espessura do ramo -- uma por segmento seria caro a 15 FPS.
    let twigs: [*mut core::ffi::c_void; 3] = std::array::from_fn(|step| {
        let weight = if theme.dark { 0.07 } else { 0.05 } + step as f32 * 0.10;
        CreatePen(
            PS_SOLID,
            1 + step as i32,
            rgb3(mix(theme.page_bg, theme.accent, weight)),
        )
    });
    let old_twig = SelectObject(hdc, twigs[0] as _);
    for branch in &branches {
        let bucket = ((branch.weight * 3.0) as usize).min(2);
        SelectObject(hdc, twigs[bucket] as _);
        MoveToEx(
            hdc,
            branch.ax.round() as i32,
            branch.ay.round() as i32,
            std::ptr::null_mut(),
        );
        LineTo(hdc, branch.bx.round() as i32, branch.by.round() as i32);
    }
    SelectObject(hdc, old_twig);
    for pen in twigs {
        DeleteObject(pen as _);
    }

    // As sinapses por cima da ramagem, mais acesas: sao ligacao, nao tecido.
    let pens: [*mut core::ffi::c_void; 3] = std::array::from_fn(|step| {
        let weight = if theme.dark { 0.18 } else { 0.13 } + (2 - step) as f32 * 0.10;
        CreatePen(PS_SOLID, 1, rgb3(mix(theme.page_bg, theme.accent, weight)))
    });
    let old_pen = SelectObject(hdc, pens[2] as _);
    for link in &links {
        let bucket = ((link.closeness * 3.0) as usize).min(2);
        SelectObject(hdc, pens[bucket] as _);
        MoveToEx(
            hdc,
            link.ax.round() as i32,
            link.ay.round() as i32,
            std::ptr::null_mut(),
        );
        LineTo(hdc, link.bx.round() as i32, link.by.round() as i32);
    }
    SelectObject(hdc, old_pen);
    for pen in pens {
        DeleteObject(pen as _);
    }

    let node_color = mix(
        theme.page_bg,
        theme.accent,
        if theme.dark { 0.72 } else { 0.50 },
    );
    let node_brush = CreateSolidBrush(rgb3(node_color));
    let node_pen = CreatePen(PS_SOLID, 1, rgb3(node_color));
    let old_brush = SelectObject(hdc, node_brush as _);
    let old_node_pen = SelectObject(hdc, node_pen as _);

    for node in &nodes {
        // O tamanho segue a profundidade: os da frente sao corpos, os do fundo
        // sao pontos. E o que da volume a folha.
        let radius =
            ((1.4 + 4.6 * node.depth) * (0.75 + 0.25 * node.energy) * scale).clamp(1.5, 9.0);
        Ellipse(
            hdc,
            (node.x - radius).round() as i32,
            (node.y - radius).round() as i32,
            (node.x + radius).round() as i32,
            (node.y + radius).round() as i32,
        );
    }

    let pulse_color = mix(
        theme.page_bg,
        theme.accent,
        if theme.dark { 0.95 } else { 0.78 },
    );
    let pulse_brush = CreateSolidBrush(rgb3(pulse_color));
    let pulse_pen = CreatePen(PS_SOLID, 1, rgb3(pulse_color));
    SelectObject(hdc, pulse_brush as _);
    SelectObject(hdc, pulse_pen as _);
    for pulse in &pulses {
        let radius = ((1.0 + pulse.glow * 2.2) * scale).clamp(1.5, 4.5);
        Ellipse(
            hdc,
            (pulse.x - radius).round() as i32,
            (pulse.y - radius).round() as i32,
            (pulse.x + radius).round() as i32,
            (pulse.y + radius).round() as i32,
        );
    }

    SelectObject(hdc, old_node_pen);
    SelectObject(hdc, old_brush);
    DeleteObject(pulse_pen as _);
    DeleteObject(pulse_brush as _);
    DeleteObject(node_pen as _);
    DeleteObject(node_brush as _);

    // As descargas por cima de tudo: sao o que acontece agora. Dois aneis --
    // o de fora largo e fraco, o de dentro apertado e forte -- e um nucleo
    // aceso. E assim que uma faisca se le sem haver alpha a disposicao.
    // Quente de proposito: num tecido todo azul, a descarga e o unico sitio
    // onde algo acontece, e tem de se ver a primeira vista.
    let spark: Rgb = (255, 138, 76);
    let hollow = GetStockObject(NULL_BRUSH);
    for burst in &bursts {
        for (factor, weight) in [(1.0, 0.22), (0.55, 0.55)] {
            let r = burst.radius * factor;
            let pen = CreatePen(
                PS_SOLID,
                (1.0_f64 + burst.glow).round().max(1.0) as i32,
                rgb3(mix(theme.page_bg, spark, (weight * burst.glow) as f32)),
            );
            let old_pen = SelectObject(hdc, pen as _);
            let old_brush = SelectObject(hdc, hollow as _);
            Ellipse(
                hdc,
                (burst.x - r).round() as i32,
                (burst.y - r).round() as i32,
                (burst.x + r).round() as i32,
                (burst.y + r).round() as i32,
            );
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(pen as _);
        }

        // Nucleo pequeno: uma faisca, nao um holofote.
        let core = (burst.radius * 0.11 * (0.4 + burst.glow)).max(1.0);
        let color = mix(spark, (255, 245, 235), (0.20 + 0.55 * burst.glow) as f32);
        let brush = CreateSolidBrush(rgb3(color));
        let pen = CreatePen(PS_SOLID, 1, rgb3(color));
        let old_brush = SelectObject(hdc, brush as _);
        let old_pen = SelectObject(hdc, pen as _);
        Ellipse(
            hdc,
            (burst.x - core).round() as i32,
            (burst.y - core).round() as i32,
            (burst.x + core).round() as i32,
            (burst.y + core).round() as i32,
        );
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(brush as _);
        DeleteObject(pen as _);
    }
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
            draw_neural_background(target, width, height, scale, &theme);
        }

        // Marca e omnibox continuam acima da rede neural.
        draw_brand(
            target,
            brand_x,
            brand_y,
            brand_width.round() as i32,
            brand_height.round() as i32,
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
            bar_columns(comp),
            &comp.contexts,
            &comp.groups,
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
    let empty: [Vec<ContextTab>; COMPARATOR_COLUMNS] = std::array::from_fn(|_| Vec::new());
    let no_groups: [Vec<ContextGroup>; COMPARATOR_COLUMNS] = std::array::from_fn(|_| Vec::new());
    paint_comparator_bar_with_contexts(
        target,
        width,
        scale,
        names,
        BarColumns::even(names.len()),
        &empty,
        &no_groups,
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
    columns: BarColumns,
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    active_context: Option<(usize, &str, bool, bool)>,
    visible: bool,
    hover: Option<BarHit>,
    auto_scroll: bool,
    theme: &Theme,
) {
    let layout = BarLayout::with_rows(
        width as f64,
        scale,
        visible,
        columns,
        std::array::from_fn(|index| plan_tab_row(&contexts[index], &groups[index])),
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
        // A pilula do grupo vem primeiro: a cor e do grupo, nao do provedor, e
        // o triangulo diz se esta aberto ou fechado.
        for visual in 0..layout.group_pill_counts[index] {
            let group_index = layout.group_pill_indices[index][visual];
            let Some(group) = groups[index].get(group_index) else {
                continue;
            };
            let color = group.color.rgb();
            let hovered = hover
                == Some(BarHit::ContextGroup {
                    source_index: index,
                    group_index,
                });
            let fill = mix(theme.bar_bg, color, if hovered { 0.62 } else { 0.42 });
            let arrow = if group.collapsed {
                "\u{25B8}"
            } else {
                "\u{25BE}"
            };
            draw_pill(
                target,
                layout.group_pills[index][visual],
                &format!("{arrow} {}", group.name),
                PillStyle::new(fill, color, theme.fg_muted),
                scale,
                tab_font,
                theme.bar_bg,
            );
        }
        for visual in 0..layout.context_tab_counts[index] {
            let context_index = layout.context_indices[index][visual];
            let Some(tab) = source_contexts.get(context_index) else {
                continue;
            };
            let url = tab.url.as_str();
            // Uma aba agrupada veste a cor do grupo, nao a do provedor: e assim
            // que se ve de relance onde acaba um grupo e comeca o outro.
            let brand = tab
                .group
                .and_then(|id| groups[index].iter().find(|group| group.id == id))
                .map(|group| group.color.rgb())
                .unwrap_or(brand);
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
        let hovered = hover == Some(BarHit::Column(index));

        // Coluna minimizada: um chip apagado, so com o nome. Fica na barra
        // de proposito -- e o unico sitio onde o clique a traz de volta --
        // mas sem "+", porque nao ha faixa onde abrir uma aba.
        if layout.minimized[index] {
            draw_pill(
                target,
                layout.columns[index],
                name,
                PillStyle::new(
                    mix(theme.bar_bg, brand, if hovered { 0.24 } else { 0.10 }),
                    mix(theme.bar_bg, brand, 0.30),
                    theme.fg_muted,
                ),
                scale,
                tab_font,
                theme.bar_bg,
            );
            continue;
        }

        let tint = if hovered { 0.30 } else { 0.16 };
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

    // Os mesmos rectangulos que o hit-testing usa; ver `right_controls`.
    let controls = right_controls(width as f64, scale, active_context.is_some());
    draw_button(
        target,
        controls.private,
        "Privado",
        hover == Some(BarHit::Private),
        scale,
        tab_font,
        theme,
    );

    if let (Some((source_index, _url, fullscreen, private_split)), Some((label, expand, close))) =
        (active_context, controls.split)
    {
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

/// Corta um campo vindo do monitor a `GMAIL_FIELD_MAX_CHARS` chars, na
/// fronteira de char e nao de byte: `String::truncate` a meio de um UTF-8
/// entra em panico, e um remetente com acentos e o caso normal. O que ja
/// cabe volta intacto, sem alocar.
fn gmail_field(mut value: String) -> String {
    if value.len() <= GMAIL_FIELD_MAX_CHARS {
        return value;
    }
    if let Some((offset, _)) = value.char_indices().nth(GMAIL_FIELD_MAX_CHARS) {
        value.truncate(offset);
    }
    value
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

/// (largura, altura, cor de fundo, pixeis BGRX ja compostos). Os pixeis estao
/// num `Arc` porque a tela inicial repinta-se a cada frame e um `Vec` clonado
/// ali custa uma copia de largura*altura*4 bytes por frame, so para o blit ler.
type SplashCache = Option<(i32, i32, Arc<Vec<u8>>)>;

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
const PDF_VIEWER_CSP: &str = "default-src 'none'; script-src 'self' blob: 'wasm-unsafe-eval'; worker-src 'self' blob:; connect-src 'self'; img-src 'self' blob: data:; style-src 'unsafe-inline'; font-src 'self' data:; object-src 'none'; base-uri 'none'; form-action 'none'";
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

/// A marca, misturada com o que estiver por tras dela.
///
/// Antes compunha-se o alfa da arte contra a cor da pagina e blitava-se um
/// retangulo opaco. Numa tela vazia isso era invisivel; com o tecido neuronal
/// por tras passou a ser uma caixa -- o retangulo apagava as linhas que
/// cruzavam a arte. Abrir uma zona limpa grande o suficiente para o conter
/// trocava a caixa por um buraco oval, igualmente visivel.
///
/// Agora a arte vai para o ecra com o alfa dela, por `AlphaBlend`: o tecido
/// continua a passar por tras e a marca pousa em cima. Nao ha retangulo
/// nenhum, em tema nenhum.
unsafe fn draw_brand(hdc: *mut core::ffi::c_void, x: i32, y: i32, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }

    let pixels = {
        let mut cache = SPLASH_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        match *cache {
            Some((cached_w, cached_h, ref cached_pixels))
                if cached_w == width && cached_h == height =>
            {
                // Clonar o Arc e copiar um ponteiro; clonar o Vec seria copiar
                // a imagem inteira a cada repintura.
                Arc::clone(cached_pixels)
            }
            _ => {
                let rendered = Arc::new(render_brand_pixels(width, height));
                *cache = Some((width, height, Arc::clone(&rendered)));
                rendered
            }
        }
    };

    alpha_blit(hdc, &pixels, x, y, width, height);
}

/// A arte vem com alfa e sai **pre-multiplicada**, que e o que o `AlphaBlend`
/// exige: com canais por multiplicar, o que aparece a volta das letras e uma
/// auréola clara.
fn render_brand_pixels(width: i32, height: i32) -> Vec<u8> {
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
            let alpha = pixel[3];
            let premultiply =
                |value: u8| ((value as u32 * alpha as u32 + 127) / 255).min(255) as u8;
            pixels.push(premultiply(pixel[2]));
            pixels.push(premultiply(pixel[1]));
            pixels.push(premultiply(pixel[0]));
            pixels.push(alpha);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::GetDIBits;
    /// O fundo da Home acompanha a marca, nao disputa com ela.
    ///
    /// A versao anterior lancava particulas das margens e fazia-as convergir
    /// TODAS para o logo: o que se via era um amontoado a mexer por tras da
    /// marca. Estes testes prendem as propriedades que fazem a diferenca --
    /// a zona limpa, a distribuicao pela tela, o movimento e as descargas --
    /// porque nenhuma delas se nota a faltar ate alguem olhar para o ecra.
    const BRAND: (f64, f64, f64, f64) = (700.0, 180.0, 520.0, 374.0);

    fn home_field() -> tissue::Field {
        home_tissue_field(1920.0, 1080.0, 1.0)
    }

    #[test]
    fn the_tissue_runs_right_through_where_the_brand_sits() {
        // Foi rejeitado tres vezes no ecra: primeiro um retangulo opaco a
        // apagar o tecido (caixa), depois uma zona limpa a conter esse
        // retangulo (buraco oval), depois uma zona limpa estreita (mancha
        // escura a volta do logo). O que o dono quer e simples de dizer e
        // simples de verificar: **nao ha buraco nenhum**. O tecido atravessa
        // o sitio onde a marca esta, e a marca pousa em cima dele com o alfa
        // que traz.
        let (bx, by, bw, bh) = BRAND;
        let field = home_field();
        // Uma grelha sobre o retangulo da marca. Contar o total nao chega:
        // uma zona limpa deixa o total alto e abre o buraco na mesma. O que
        // tem de valer e que NENHUMA celula fica vazia.
        const COLUMNS: usize = 4;
        const ROWS: usize = 3;
        for step in 0..40 {
            let frame = tissue::tissue_at(&field, step as f64 * 0.31);
            let mut cells = [[0usize; COLUMNS]; ROWS];
            let mut count = |x: f64, y: f64| {
                if x < bx || x > bx + bw || y < by || y > by + bh {
                    return;
                }
                let col = (((x - bx) / bw * COLUMNS as f64) as usize).min(COLUMNS - 1);
                let row = (((y - by) / bh * ROWS as f64) as usize).min(ROWS - 1);
                cells[row][col] += 1;
            };
            for branch in &frame.branches {
                count(branch.ax, branch.ay);
                count(branch.bx, branch.by);
            }
            for node in &frame.nodes {
                count(node.x, node.y);
            }
            for (row, line) in cells.iter().enumerate() {
                for (col, found) in line.iter().enumerate() {
                    assert!(
                        *found > 0,
                        "nada na celula ({row}, {col}) do retangulo da marca \
                         em t={:.2}: e um buraco, so que mais pequeno",
                        step as f64 * 0.31
                    );
                }
            }
        }
    }

    #[test]
    fn home_background_spreads_across_the_canvas() {
        let nodes = tissue::nodes_at(&home_field(), 3.0);

        // Uma convergencia para um ponto passaria a zona limpa mas continuaria
        // a ser um amontoado: exige-se ocupacao dos quatro quadrantes.
        let mut quadrants = [0usize; 4];
        for node in &nodes {
            let index = usize::from(node.x > 960.0) + 2 * usize::from(node.y > 540.0);
            quadrants[index] += 1;
        }
        assert!(
            quadrants.iter().all(|count| *count >= 3),
            "distribuicao amontoada: {quadrants:?}"
        );

        // E as energias tem de variar, senao nao ha pulsacao nenhuma.
        let energies: Vec<f64> = nodes.iter().map(|node| node.energy).collect();
        let min = energies.iter().copied().fold(f64::MAX, f64::min);
        let max = energies.iter().copied().fold(f64::MIN, f64::max);
        assert!(max - min > 0.1, "energias iguais: {min}..{max}");
    }

    #[test]
    fn home_neurons_travel_and_discharge_when_they_meet() {
        // O fundo da Home e o mesmo tecido do instalador: tem de ter o mesmo
        // comportamento, nao so o mesmo aspeto parado. Sem deslocacao nao ha
        // encontro, e sem encontro nao ha descarga -- fica um mobile.
        let field = home_field();
        let start = tissue::nodes_at(&field, 0.0);
        let mut furthest = 0.0f64;
        let mut discharges = 0usize;
        for step in 0..240 {
            let seconds = step as f64 * 0.05;
            let frame = tissue::tissue_at(&field, seconds);
            discharges += frame.bursts.len();
            for (a, b) in start.iter().zip(frame.nodes.iter()) {
                furthest = furthest.max(((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt());
            }
        }
        let spacing = (1920.0 * 1080.0 / start.len() as f64).sqrt();
        assert!(
            furthest > spacing * 0.5,
            "em 12 segundos o neuronio que mais andou fez {furthest:.0} px \
             para um espacamento de {spacing:.0} px"
        );
        assert!(
            discharges > 20,
            "em 12 segundos houve {discharges} descargas no fundo da Home"
        );
    }

    #[test]
    fn home_tissue_is_dense_enough_to_read_as_tissue() {
        // "Quero algo mais real, com muito mais conexoes." Uma rede rala le-se
        // como um grafo; o que se quer e tecido, com ramagem por tras.
        let frame = tissue::tissue_at(&home_field(), 6.0);
        let per_node = frame.links.len() as f64 / frame.nodes.len() as f64;
        assert!(
            per_node >= 2.5,
            "{per_node:.1} ligacoes por soma: ainda e um grafo, nao tecido"
        );
        assert!(
            frame.branches.len() > frame.nodes.len() * 12,
            "{} ramos para {} somas: falta a ramagem",
            frame.branches.len(),
            frame.nodes.len()
        );
    }

    /// A autorizacao de rede local vale para a ORIGEM que o utilizador
    /// escreveu, nao para a rede local inteira.
    ///
    /// O `allow_local` era um booleano capturado pelo handler de navegacao
    /// para toda a vida da WebView: depois de o utilizador abrir
    /// `http://192.168.1.50:3000` na palette nativa, essa pagina -- remota do
    /// ponto de vista do produto -- podia navegar para `http://192.168.1.1/`
    /// ou para qualquer outro host da rede, e o handler deixava passar.
    #[test]
    fn typed_local_url_authorizes_only_its_own_origin() {
        let typed = "http://192.168.1.50:3000";

        assert!(remote_web_target(
            "http://192.168.1.50:3000/painel",
            Some(typed)
        ));
        assert!(!remote_web_target(
            "view-source:http://192.168.1.50:3000/painel",
            None
        ));
        assert!(is_view_source_target(
            "view-source:http://192.168.1.50:3000/painel",
            Some(typed)
        ));

        // O pivot: outro host da mesma rede local.
        assert!(
            !remote_web_target("http://192.168.1.1/admin", Some(typed)),
            "outro host local nao esta autorizado"
        );
        assert!(
            !remote_web_target("http://127.0.0.1:8080/", Some(typed)),
            "loopback nao esta autorizado"
        );
        assert!(
            !is_view_source_target("view-source:http://192.168.1.1/admin", Some(typed)),
            "view-source nao contorna a mesma regra"
        );

        // Outra porta e outra origem.
        assert!(!remote_web_target("http://192.168.1.50:9000/", Some(typed)));

        // Sem autorizacao nenhuma, nada local passa; a web publica passa sempre.
        assert!(!remote_web_target("http://192.168.1.50:3000/", None));
        assert!(remote_web_target("https://example.com/x", None));
        assert!(remote_web_target("https://example.com/x", Some(typed)));
    }

    /// Minimizar a janela na Home matava a aplicacao.
    ///
    /// O winit trata o `WM_SIZE` sem filtrar `SIZE_MINIMIZED`, por isso emite
    /// `Resized(0, 0)`; o ramo `Surface::Home` chama `position_omnibox`, que
    /// passa a altura 0 ao `HomeLayout::new`. La dentro,
    /// `(0.0).clamp(310.0, -180.0)` faz `assert!(min <= max)` -- activo tambem
    /// em release -- e entra em panico dentro do callback do event loop.
    ///
    /// O `with_min_inner_size(700x500)` nao protege: a minimizacao nao passa
    /// pelo `WM_GETMINMAXINFO`.
    #[test]
    fn home_layout_survives_a_minimized_window() {
        for height in [0.0, 1.0, 100.0, 300.0, 489.0, 490.0, 760.0, 2000.0] {
            for width in [0.0, 320.0, 1120.0] {
                for scale in [1.0, 1.5, 2.0] {
                    let layout = HomeLayout::new(width, height, scale);
                    assert!(
                        layout.input.y.is_finite() && layout.go.y.is_finite(),
                        "{width}x{height} @{scale}"
                    );
                }
            }
        }
    }

    /// Geometria da barra: nada do que e desenhado numa coluna pode aterrar
    /// noutra, nem por cima dos controlos da direita. Os tres casos vieram da
    /// auditoria de 2026-09-19 e cada um tinha um clique concreto a ir para o
    /// sitio errado.
    mod bar_geometry {
        use super::*;

        /// Pesos que sobram depois de arrastar o divisor `divider` ate ao
        /// batente da esquerda ou da direita.
        fn dragged(weights: [f64; COMPARATOR_COLUMNS], divider: usize, mouse_x: f64) -> BarColumns {
            let visible: Vec<usize> = (0..COMPARATOR_COLUMNS).collect();
            BarColumns {
                count: COMPARATOR_COLUMNS,
                weights: resized_weights(&weights, &visible, divider, mouse_x, 1120.0),
                minimized: [false; COMPARATOR_COLUMNS],
                split_active: false,
            }
        }

        fn overlaps(left: UiRect, right: UiRect) -> bool {
            left.width > 0.0
                && right.width > 0.0
                && left.x < right.x + right.width
                && right.x < left.x + left.width
        }

        #[test]
        fn nothing_a_column_draws_leaves_that_column() {
            // Divisor 0 todo para a esquerda: a coluna 0 fecha no minimo e a
            // pilula de 116 px deixa de caber la dentro.
            let columns = dragged([1.0; COMPARATOR_COLUMNS], 0, 0.0);
            let layout =
                BarLayout::with_contexts(1120.0, 1.0, true, columns, [0; COMPARATOR_COLUMNS]);
            let spans = visible_column_spans(
                1120.0,
                COMPARATOR_COLUMNS,
                &columns.weights,
                &columns.minimized,
            );

            for span in &spans {
                let pill = layout.columns[span.index];
                let plus = layout.add_tabs[span.index];
                let right = span.x + span.width;
                assert!(
                    pill.x + pill.width <= right + 0.5,
                    "pilula da coluna {} sai da faixa: {:?} contra {right}",
                    span.index,
                    pill
                );
                assert!(
                    plus.x + plus.width <= right + 0.5,
                    "'+' da coluna {} sai da faixa: {:?} contra {right}",
                    span.index,
                    plus
                );
            }
        }

        #[test]
        fn the_plus_never_hides_under_the_private_button() {
            // Divisor 1 todo para a direita: a coluna 2 fica no minimo e o
            // "+" dela ia parar dentro de "Privado", que ganha o hit-test.
            let columns = dragged([1.0; COMPARATOR_COLUMNS], 1, 1120.0);
            let layout =
                BarLayout::with_contexts(1120.0, 1.0, true, columns, [0; COMPARATOR_COLUMNS]);
            let private = right_controls(1120.0, 1.0, false).private;

            for index in 0..COMPARATOR_COLUMNS {
                let plus = layout.add_tabs[index];
                assert!(
                    !overlaps(plus, private),
                    "'+' da coluna {index} debaixo de Privado: {plus:?} contra {private:?}"
                );
                let pill = layout.columns[index];
                assert!(
                    !overlaps(pill, private),
                    "pilula da coluna {index} debaixo de Privado: {pill:?} contra {private:?}"
                );
            }
        }

        #[test]
        fn a_minimized_chip_never_lands_on_a_visible_column() {
            // Coluna 1 minimizada e o unico divisor todo para a direita: a
            // coluna 2 fica estreita e o chip caia-lhe em cima.
            let visible = vec![0usize, 2];
            let weights = resized_weights(&[1.0; COMPARATOR_COLUMNS], &visible, 0, 1120.0, 1120.0);
            let columns = BarColumns {
                count: COMPARATOR_COLUMNS,
                weights,
                minimized: [false, true, false],
                split_active: false,
            };
            let layout =
                BarLayout::with_contexts(1120.0, 1.0, true, columns, [0; COMPARATOR_COLUMNS]);

            let chip = layout.columns[1];
            assert!(chip.width > 0.0, "a coluna minimizada tem de ter chip");
            for index in [0usize, 2] {
                assert!(
                    !overlaps(chip, layout.columns[index]),
                    "chip em cima da pilula da coluna {index}: {chip:?} contra {:?}",
                    layout.columns[index]
                );
                assert!(
                    !overlaps(chip, layout.add_tabs[index]),
                    "chip em cima do '+' da coluna {index}: {chip:?} contra {:?}",
                    layout.add_tabs[index]
                );
            }
        }
    }

    /// O divisor do comparador tem de aceitar o rato.
    ///
    /// A classe STATIC responde `HTTRANSPARENT` ao `WM_NCHITTEST` quando nao
    /// tem `SS_NOTIFY`, e o sistema entrega o rato a janela de baixo -- aqui, o
    /// WebView2. Sem a interceccao, nenhum `WM_LBUTTONDOWN` chega ao divisor:
    /// o `SetCapture` nunca corre, o `RESIZE_X` nunca e escrito, o
    /// `UserEvent::ResizeComparator` nunca e enviado, e arrastar o divisor
    /// seleciona texto na pagina em vez de mudar a largura das colunas.
    #[test]
    fn comparator_splitter_accepts_the_mouse() {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                WS_POPUP,
                0,
                0,
                8,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null(), "a janela do divisor tem de nascer");

            let subclassed = SetWindowSubclass(
                hwnd,
                Some(comparator_splitter_subclass),
                SPLITTER_SUBCLASS_BASE,
                0,
            );
            let hit = SendMessageW(hwnd, WM_NCHITTEST, 0, 0);
            DestroyWindow(hwnd);

            assert_ne!(subclassed, 0, "a subclasse tem de instalar");
            assert_eq!(
                hit, HTCLIENT as LRESULT,
                "o divisor devolveu {hit} (HTTRANSPARENT e -1): o rato atravessa-o"
            );
        }
    }

    #[test]
    fn the_brand_keeps_its_transparency_instead_of_becoming_a_rectangle() {
        // Isto foi rejeitado duas vezes no ecra: a arte compunha-se contra a
        // cor da pagina e ia para o ecra opaca, o que apagava o tecido num
        // retangulo. Os cantos da arte sao transparentes e tem de continuar a
        // ser depois de redimensionados.
        let size = 96;
        let pixels = render_brand_pixels(size, size);
        assert_eq!(pixels.len(), (size * size * 4) as usize);

        let at = |x: i32, y: i32| {
            let index = ((y * size + x) * 4) as usize;
            (
                pixels[index],
                pixels[index + 1],
                pixels[index + 2],
                pixels[index + 3],
            )
        };
        for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
            let (b, g, r, a) = at(x, y);
            assert_eq!(
                (b, g, r, a),
                (0, 0, 0, 0),
                "o canto ({x}, {y}) e opaco: vai aparecer um retangulo"
            );
        }

        // E alguma coisa tem de ser visivel, senao o que se corrigiu foi
        // apagar a marca.
        assert!(
            pixels.chunks(4).any(|px| px[3] > 200),
            "a marca ficou toda transparente"
        );

        // Pre-multiplicado: nenhum canal pode exceder o alfa. Sem isto o
        // AlphaBlend desenha uma aureola clara a volta das letras.
        for px in pixels.chunks(4) {
            assert!(
                px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3],
                "pixel por pre-multiplicar: {px:?}"
            );
        }
    }

    #[test]
    fn test_stretch_dibits_on_screen_dc() {
        unsafe {
            let hdc = GetDC(core::ptr::null_mut());
            assert!(!hdc.is_null());
            let img = get_brand_image();
            assert_eq!(img.width(), 1200);
            let size = 104;
            let pixels = render_brand_pixels(size, size);
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
    fn lru_evicts_the_oldest_and_keeps_the_used_alive() {
        // Simula o que a barra faz: enche o cache e depois continua a pedir
        // tamanhos novos, como durante um arrasto da borda da janela.
        let mut entries: Vec<((usize, u32), u32)> = Vec::new();
        for size in 0..ICON_CACHE_CAPACITY as u32 {
            lru_insert(&mut entries, (0, size), size, ICON_CACHE_CAPACITY);
        }
        assert_eq!(entries.len(), ICON_CACHE_CAPACITY);

        // Usar a mais antiga promove-a: deixa de ser a proxima a sair.
        assert_eq!(lru_promote(&mut entries, &(0, 0)), Some(0));
        lru_insert(&mut entries, (0, 100), 100, ICON_CACHE_CAPACITY);
        assert_eq!(entries.len(), ICON_CACHE_CAPACITY);
        assert_eq!(lru_promote(&mut entries, &(0, 0)), Some(0));
        // A vitima foi a que estava sem uso ha mais tempo, nao a recem-usada.
        assert_eq!(lru_promote(&mut entries, &(0, 1)), None);
    }

    #[test]
    fn lru_insert_replaces_instead_of_duplicating() {
        let mut entries: Vec<((usize, u32), u32)> = Vec::new();
        lru_insert(&mut entries, (1, 16), 1, 4);
        lru_insert(&mut entries, (1, 16), 2, 4);
        assert_eq!(entries.len(), 1);
        assert_eq!(lru_promote(&mut entries, &(1, 16)), Some(2));
        assert_eq!(lru_promote(&mut entries, &(2, 16)), None);
    }

    #[test]
    fn icon_cache_stays_bounded_across_many_sizes() {
        // Pelo cache verdadeiro: cada tamanho e uma entrada, e mesmo pedindo
        // muito mais do que o tecto a lista nao cresce.
        for size in 8..64u32 {
            let _ = icon_scaled(ICON_SLOT_HOME, size);
        }
        let entries = ICON_SCALE_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        assert!(entries.len() <= ICON_CACHE_CAPACITY);
    }

    #[test]
    fn home_animation_sleeps_when_nobody_is_looking() {
        // Minimizada ou tapada nao ha frame nenhum — o laco fica em Wait.
        assert_eq!(home_frame_interval(true, false, true), None);
        assert_eq!(home_frame_interval(false, true, true), None);
        assert_eq!(home_frame_interval(true, true, false), None);
        // Visivel e com foco: os ~15 FPS de sempre.
        assert_eq!(
            home_frame_interval(false, false, true),
            Some(Duration::from_millis(66))
        );
        // Visivel sem foco: continua a animar, mas quatro vezes mais devagar.
        assert_eq!(
            home_frame_interval(false, false, false),
            Some(Duration::from_millis(250))
        );
    }

    #[test]
    fn theme_carries_its_own_dark_flag() {
        // draw_neural_background le isto em vez de voltar ao registo.
        assert!(Theme::dark((0, 120, 212)).dark);
        assert!(!Theme::light((0, 120, 212)).dark);
    }

    #[test]
    fn theme_cache_is_reused_and_invalidated() {
        Theme::invalidate();
        assert!(
            THEME_CACHE
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_none()
        );
        let first = Theme::system();
        // A segunda chamada dentro da validade nao volta ao registo: o valor
        // guardado e o mesmo objecto que saiu da primeira.
        let stamp = THEME_CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|(stamp, _)| stamp)
            .expect("system() deve deixar o tema em cache");
        let second = Theme::system();
        let same_stamp = THEME_CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|(stamp, _)| stamp);
        assert_eq!(same_stamp, Some(stamp));
        assert_eq!(first.page_bg, second.page_bg);
        assert_eq!(first.dark, second.dark);

        Theme::invalidate();
        assert!(
            THEME_CACHE
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_none()
        );
    }

    #[test]
    fn agent_trace_never_records_typed_or_selected_values() {
        let target = AgentElement {
            id: "field-1".into(),
            generation: 1,
            role: "textbox".into(),
            name: "Search".into(),
            text: String::new(),
            origin: "https://example.com".into(),
            frame: "top".into(),
            visible: true,
            interactable: true,
        };
        let typed = agent_trace_action(&AgentAction::TypeText {
            target: target.clone(),
            text: "synthetic-secret-value".into(),
            field: FieldKind::Text,
        });
        let selected = agent_trace_action(&AgentAction::Select {
            target,
            value: "synthetic-secret-option".into(),
        });

        assert!(!typed.contains("synthetic-secret-value"));
        assert!(!selected.contains("synthetic-secret-option"));
        assert!(typed.contains("chars=22"));
    }

    #[test]
    fn agent_script_encodes_spaces_as_percent_twenty() {
        // `decodeURIComponent` nao converte '+' em espaco. Um '+' aqui e um
        // '+' escrito no campo, e um nome que nunca bate com o do DOM: o
        // guard do script desiste em silencio e a accao nao acontece.
        let target = AgentElement {
            id: "agent-7".into(),
            generation: 1,
            role: "textbox".into(),
            name: "Search the site".into(),
            text: String::new(),
            origin: "https://example.com".into(),
            frame: "top".into(),
            visible: true,
            interactable: true,
        };
        let script = agent_action_script(&AgentAction::TypeText {
            target,
            text: "duas palavras".into(),
            field: FieldKind::Text,
        })
        .expect("TypeText e executavel pela bridge");

        assert!(
            script.contains("decodeURIComponent('Search%20the%20site')"),
            "{script}"
        );
        assert!(
            script.contains("decodeURIComponent('duas%20palavras')"),
            "{script}"
        );
        assert!(!script.contains("Search+the+site"), "{script}");
        assert!(!script.contains("duas+palavras"), "{script}");
    }

    #[test]
    fn reader_memory_text_cuts_on_char_boundary() {
        // 512 KiB caem a meio de um caractere de dois bytes quando o corpo
        // comeca num offset impar: `String::truncate` nesse indice entra em
        // panico e leva a aplicacao inteira.
        let article = ReaderArticle {
            source_url: "https://example.com/artigo".into(),
            title: "Artigo".into(),
            byline: None,
            excerpt: Some("a".into()),
            blocks: vec![ReaderBlock::Paragraph("ç".repeat(400_000))],
        };

        let text = reader_article_memory_text(&article);
        assert!(text.len() <= 512 * 1024, "{}", text.len());
        assert!(text.starts_with("a\n\n"));
    }

    #[test]
    fn submit_role_and_sensitive_labels_are_not_plain_reversible_clicks() {
        let page = ObservedPage {
            generation: 1,
            url: "https://example.com/form".into(),
            title: String::new(),
            text_excerpt: String::new(),
            elements: Vec::new(),
        };
        let make = |role: &str, name: &str| AgentElement {
            id: name.into(),
            generation: 1,
            role: role.into(),
            name: name.into(),
            text: name.into(),
            origin: "https://example.com".into(),
            frame: "top".into(),
            visible: true,
            interactable: true,
        };

        for target in [
            make("submit", "Continue"),
            make("button", "Save changes"),
            make("button", "Authorize"),
        ] {
            assert!(matches!(
                app_agent_security_action(&AgentAction::Click { target }, &page),
                AgentSecurityAction::Submit { .. }
            ));
        }

        for target in [make("button", "Checkout"), make("button", "Transfer")] {
            assert!(matches!(
                app_agent_security_action(&AgentAction::Click { target }, &page),
                AgentSecurityAction::Payment { .. }
            ));
        }
    }

    /// A SPEC-0105 sobre o agente que EMBARCA. O teste de aceitação em
    /// `spec_010x_acceptance.rs` corre sobre o `AgentRuntime` do `neural-core`,
    /// que esta aplicação não usa: apagar o gate daqui deixava-o verde.
    /// Estes correm sobre `decide_agent_step`, que é o que decide no produto.
    mod spec_0105_shipping_agent {
        use super::*;

        fn element(role: &str, name: &str) -> AgentElement {
            AgentElement {
                id: format!("n1-{name}"),
                generation: 1,
                role: role.into(),
                name: name.into(),
                text: name.into(),
                origin: "https://example.com".into(),
                frame: "top".into(),
                visible: true,
                interactable: true,
            }
        }

        fn page(elements: Vec<AgentElement>) -> ObservedPage {
            ObservedPage {
                generation: 1,
                url: "https://example.com/loja".into(),
                title: "Loja".into(),
                text_excerpt: "texto observado".into(),
                elements,
            }
        }

        fn policy() -> AgentPermissionPolicy {
            let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
            policy.grant_reversible_session_actions(true);
            policy
        }

        fn decide(
            commands: &[BrowserAgentCommand],
            page: &ObservedPage,
            policy: &mut AgentPermissionPolicy,
        ) -> AgentStepDecision {
            decide_agent_step(commands, 0, 0, Duration::ZERO, page, policy)
        }

        #[test]
        fn payment_click_never_reaches_the_page() {
            let page = page(vec![element("button", "Comprar agora")]);
            let mut policy = policy();
            let decision = decide(
                &[BrowserAgentCommand::Click("comprar".into())],
                &page,
                &mut policy,
            );

            assert_eq!(
                decision,
                AgentStepDecision::Stop(AgentTermination::RestrictedAction)
            );
            // A decisão passou mesmo pela política, e ficou registada.
            assert_eq!(policy.audit().len(), 1);
            assert!(!policy.audit()[0].allowed);
        }

        #[test]
        fn sensitive_submit_waits_for_a_human_yes() {
            let page = page(vec![element("button", "Enviar formulário")]);
            let mut policy = policy();
            let decision = decide(
                &[BrowserAgentCommand::Click("enviar".into())],
                &page,
                &mut policy,
            );

            let AgentStepDecision::Act(act) = decision else {
                panic!("esperava Act, veio {decision:?}");
            };
            assert!(
                act.confirmation.is_some(),
                "ação sensível não pode seguir sem confirmação: {act:?}"
            );
            assert!(matches!(act.security, AgentSecurityAction::Submit { .. }));
        }

        #[test]
        fn reversible_click_runs_under_the_session_grant() {
            let page = page(vec![element("button", "Ver detalhes")]);
            let mut policy = policy();
            let decision = decide(
                &[BrowserAgentCommand::Click("ver detalhes".into())],
                &page,
                &mut policy,
            );

            let AgentStepDecision::Act(act) = decision else {
                panic!("esperava Act, veio {decision:?}");
            };
            assert_eq!(act.confirmation, None);
            assert!(policy.audit()[0].allowed);
        }

        #[test]
        fn cross_origin_element_needs_approval() {
            // O mesmo clique reversível, com a página noutra origem que a
            // sessão nunca aprovou.
            let mut other = page(vec![element("button", "Ver detalhes")]);
            other.url = "https://outra.example/loja".into();
            let mut policy = policy();
            let decision = decide(
                &[BrowserAgentCommand::Click("ver detalhes".into())],
                &other,
                &mut policy,
            );

            let AgentStepDecision::Act(act) = decision else {
                panic!("esperava Act, veio {decision:?}");
            };
            assert!(act.confirmation.is_some(), "{act:?}");
        }

        #[test]
        fn budget_comes_from_the_spec_and_stops_the_agent() {
            let budget = AgentRuntimeConfig::default();
            let page = page(vec![element("button", "Ver detalhes")]);
            let commands = [BrowserAgentCommand::Click("ver detalhes".into())];

            for (steps, elapsed) in [
                (budget.max_steps, Duration::ZERO),
                (0, budget.max_wall_time),
            ] {
                let mut policy = policy();
                let decision = decide_agent_step(&commands, 0, steps, elapsed, &page, &mut policy);
                assert_eq!(decision, AgentStepDecision::Stop(AgentTermination::Limit));
                // Parou antes de sequer consultar a política.
                assert!(policy.audit().is_empty());
            }
        }

        #[test]
        fn missing_element_and_exhausted_plan_are_distinct_stops() {
            let empty = page(Vec::new());
            let mut policy = policy();
            assert_eq!(
                decide(
                    &[BrowserAgentCommand::Click("comprar".into())],
                    &empty,
                    &mut policy
                ),
                AgentStepDecision::Stop(AgentTermination::ElementMissing)
            );
            assert_eq!(
                decide(&[], &empty, &mut policy),
                AgentStepDecision::Stop(AgentTermination::Completed)
            );
            assert!(policy.audit().is_empty());
        }

        #[test]
        fn typed_text_goes_through_the_gate_as_typed_text() {
            let page = page(vec![element("textbox", "Pesquisar")]);
            let mut policy = policy();
            let decision = decide(
                &[BrowserAgentCommand::Search("rust webview".into())],
                &page,
                &mut policy,
            );

            let AgentStepDecision::Act(act) = decision else {
                panic!("esperava Act, veio {decision:?}");
            };
            assert!(matches!(
                act.security,
                AgentSecurityAction::TypeText {
                    field: FieldKind::Search,
                    ..
                }
            ));
            assert_eq!(policy.audit().len(), 1);
        }
    }

    #[test]
    fn agent_termination_reason_is_explicit() {
        assert_eq!(AgentTermination::Completed.as_str(), "completed");
        assert_eq!(AgentTermination::UserStopped.as_str(), "user-stopped");
        assert_eq!(
            AgentTermination::RestrictedAction.as_str(),
            "restricted-action"
        );
        assert_eq!(AgentTermination::ExecutionError.as_str(), "execution-error");
    }

    #[test]
    fn private_panel_and_new_tab_are_wired() {
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("act('newtab', { col:colIndex })"));
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("key === 'escape'"));
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("act('back')"));
        assert!(format!("{:?}", neuralia_action("neuralia:newtab")).starts_with("Some(NewTab"));
        assert_ne!(BarHit::Private, BarHit::SplitClose);
    }

    #[test]
    fn reader_neuralia_actions_are_routed() {
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

        for target in [
            "https://example.com",
            "neuralia:inventado",
            "about:blank",
            "neuralia",
        ] {
            assert!(neuralia_action(target).is_none(), "{target}");
        }
    }

    #[test]
    fn remote_navigation_cannot_pivot_into_private_network() {
        assert!(remote_web_target("https://example.com/a", None));
        assert!(!remote_web_target("http://127.0.0.1:8000/", None));
        assert!(!remote_web_target("http://192.168.1.1/", None));
        assert!(remote_web_target(
            "http://127.0.0.1:8000/",
            Some("http://127.0.0.1:8000")
        ));
    }

    #[test]
    fn view_source_follows_the_surface_network_policy() {
        assert!(is_view_source_target(
            "view-source:https://example.com/a?b=c",
            None
        ));
        assert!(is_view_source_target(
            "view-source:http://example.com/",
            None
        ));
        assert!(!is_view_source_target(
            "view-source:http://127.0.0.1:8000/",
            None
        ));
        assert!(is_view_source_target(
            "view-source:http://127.0.0.1:8000/",
            Some("http://127.0.0.1:8000")
        ));
        assert!(!is_view_source_target(
            "view-source:http://192.168.1.1/",
            None
        ));
        assert!(!is_view_source_target(
            "view-source:http://neuralia-pdf.localhost/viewer.html",
            None
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
            assert!(
                !is_view_source_target(target, Some("http://127.0.0.1:8000")),
                "{target}"
            );
        }
        // O pedido por script da pagina continua a nao ser navegacao web.
        assert!(!remote_web_target(
            "view-source:https://example.com/",
            Some("http://127.0.0.1:8000")
        ));
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
        let flipped = if token.ends_with('0') { "1" } else { "0" };
        let wrong = format!("{}{flipped}", &token[..31]);
        assert!(constant_time_eq(token.as_bytes(), token.as_bytes()));
        assert!(!constant_time_eq(token.as_bytes(), wrong.as_bytes()));
        assert!(!constant_time_eq(token.as_bytes(), &token.as_bytes()[..31]));
    }

    #[test]
    fn pdf_assets_carry_nosniff_and_the_viewer_csp() {
        let html = std::str::from_utf8(PDF_VIEWER_HTML).expect("viewer.html e UTF-8");
        assert!(
            html.contains(PDF_VIEWER_CSP),
            "o cabecalho tem de ser igual ao <meta> do viewer.html"
        );

        let bytes = Arc::new(Mutex::new(b"%PDF-1.7".to_vec()));
        for (path, is_html, expected_type, expected_status) in [
            ("/viewer.html", true, "text/html; charset=utf-8", 200),
            ("/", true, "text/html; charset=utf-8", 200),
            ("/viewer.mjs", false, "text/javascript", 200),
            ("/pdf.mjs", false, "text/javascript", 200),
            ("/pdf.worker.mjs", false, "text/javascript", 200),
            ("/document.pdf", false, "application/pdf", 200),
            ("/wasm/openjpeg.wasm", false, "application/wasm", 200),
            ("/wasm/jbig2.wasm", false, "application/wasm", 200),
            ("/wasm/qcms_bg.wasm", false, "application/wasm", 200),
            (
                "/wasm/openjpeg_nowasm_fallback.js",
                false,
                "text/javascript",
                200,
            ),
            (
                "/cmaps/Adobe-Japan1-UCS2.bcmap",
                false,
                "application/octet-stream",
                200,
            ),
            (
                "/standard_fonts/LiberationSans-Regular.ttf",
                false,
                "font/ttf",
                200,
            ),
            (
                "/icc/CGATS001Compat-v2-micro.icc",
                false,
                "application/vnd.iccprofile",
                200,
            ),
            ("/wasm/../pdf.mjs", false, "text/plain", 404),
            ("/fixtures/bug_jpx.pdf", false, "text/plain", 404),
            ("/nada", false, "text/plain", 404),
        ] {
            let request = Request::builder()
                .uri(format!("{PDF_ORIGIN}{path}"))
                .body(Vec::new())
                .expect("pedido de teste");
            let response = serve_pdf_asset(&bytes, &request);
            assert_eq!(response.status().as_u16(), expected_status, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get("Content-Type")
                    .and_then(|value| value.to_str().ok()),
                Some(expected_type),
                "{path}"
            );
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
            // So o documento anuncia faixas: os ficheiros do visualizador sao
            // servidos inteiros e nunca por Range.
            assert_eq!(
                header("Accept-Ranges").is_some(),
                path == "/document.pdf",
                "{path}"
            );
        }
    }

    #[test]
    fn pdf_viewer_configures_complete_local_pdfjs_assets() {
        let viewer = std::str::from_utf8(PDF_VIEWER_JS).expect("viewer.mjs e UTF-8");
        for setting in [
            "cMapUrl: './cmaps/'",
            "cMapPacked: true",
            "standardFontDataUrl: './standard_fonts/'",
            "wasmUrl: './wasm/'",
            "iccUrl: './icc/'",
            "useWasm: true",
            "useWorkerFetch: true",
        ] {
            assert!(viewer.contains(setting), "configuração ausente: {setting}");
        }
        assert!(PDF_VIEWER_CSP.contains("'wasm-unsafe-eval'"));
        assert!(!PDF_VIEWER_CSP.contains("'unsafe-eval'"));
        assert!(PDF_VIEWER_CSP.contains("connect-src 'self'"));
        assert!(!PDF_VIEWER_CSP.contains("https:"));
        assert!(!PDF_VIEWER_CSP.contains("http:"));
    }

    #[test]
    fn parse_range_reads_single_byte_ranges() {
        use RangeParse::*;
        assert_eq!(parse_range("", 10), None);
        assert_eq!(parse_range("   ", 10), None);
        assert_eq!(
            parse_range("bytes=0-0", 10),
            Satisfiable { start: 0, end: 0 }
        );
        assert_eq!(
            parse_range("bytes=0-0", 1),
            Satisfiable { start: 0, end: 0 }
        );
        assert_eq!(
            parse_range("bytes=2-4", 10),
            Satisfiable { start: 2, end: 4 }
        );
        assert_eq!(
            parse_range("bytes=9-9", 10),
            Satisfiable { start: 9, end: 9 }
        );
        // Sem fim: ate ao ultimo byte. Fim para la do documento: cortado.
        assert_eq!(
            parse_range("bytes=5-", 10),
            Satisfiable { start: 5, end: 9 }
        );
        assert_eq!(
            parse_range("bytes=5-99", 10),
            Satisfiable { start: 5, end: 9 }
        );
        assert_eq!(
            parse_range("bytes=0-99999999999999999999999999", 10),
            Satisfiable { start: 0, end: 9 }
        );
        // Sufixo: os ultimos N bytes; maior do que o documento e tudo.
        assert_eq!(
            parse_range("bytes=-3", 10),
            Satisfiable { start: 7, end: 9 }
        );
        assert_eq!(
            parse_range("bytes=-10", 10),
            Satisfiable { start: 0, end: 9 }
        );
        assert_eq!(
            parse_range("bytes=-100", 10),
            Satisfiable { start: 0, end: 9 }
        );
        // Tolerancia: unidade sem distinguir maiusculas, espacos, elemento
        // vazio no fim da lista.
        assert_eq!(
            parse_range("BYTES=0-1", 10),
            Satisfiable { start: 0, end: 1 }
        );
        assert_eq!(
            parse_range(" bytes = 0 - 1 ", 10),
            Satisfiable { start: 0, end: 1 }
        );
        assert_eq!(
            parse_range("bytes=0-1,", 10),
            Satisfiable { start: 0, end: 1 }
        );
    }

    #[test]
    fn parse_range_separates_unsatisfiable_from_ignored() {
        use RangeParse::*;
        // Validas mas sem byte nenhum: 416.
        assert_eq!(parse_range("bytes=-0", 10), Unsatisfiable);
        assert_eq!(parse_range("bytes=10-", 10), Unsatisfiable);
        assert_eq!(parse_range("bytes=10-20", 10), Unsatisfiable);
        assert_eq!(
            parse_range("bytes=99999999999999999999999999-", 10),
            Unsatisfiable
        );
        // Documento vazio nao tem nenhum byte para dar, venha o que vier.
        assert_eq!(parse_range("bytes=0-", 0), Unsatisfiable);
        assert_eq!(parse_range("bytes=0-0", 0), Unsatisfiable);
        assert_eq!(parse_range("bytes=-1", 0), Unsatisfiable);
        assert_eq!(parse_range("", 0), None);
        // Invalidas: nao sao pedidos de faixa, serve-se tudo.
        assert_eq!(parse_range("bytes=3-2", 10), Ignored);
        assert_eq!(parse_range("items=0-1", 10), Ignored);
        assert_eq!(parse_range("bytes", 10), Ignored);
        assert_eq!(parse_range("bytes=", 10), Ignored);
        assert_eq!(parse_range("bytes=-", 10), Ignored);
        assert_eq!(parse_range("bytes=,", 10), Ignored);
        assert_eq!(parse_range("bytes=a-b", 10), Ignored);
        assert_eq!(parse_range("bytes=0-1x", 10), Ignored);
        assert_eq!(parse_range("bytes=+0-1", 10), Ignored);
        assert_eq!(parse_range("bytes=0", 10), Ignored);
        // Varias faixas seriam multipart/byteranges: 200 completo.
        assert_eq!(parse_range("bytes=0-1,3-4", 10), Ignored);
        assert_eq!(parse_range("bytes=0-1, 3-4", 10), Ignored);
    }

    #[test]
    fn pdf_document_is_served_by_range() {
        let document = b"%PDF-1.7 0123456789".to_vec();
        let total = document.len();
        let bytes = Arc::new(Mutex::new(document.clone()));
        let serve = |method: &str, range: Option<&str>| {
            let mut request = Request::builder()
                .method(method)
                .uri(format!("{PDF_ORIGIN}/document.pdf"));
            if let Some(range) = range {
                request = request.header("Range", range);
            }
            serve_pdf_asset(&bytes, &request.body(Vec::new()).expect("pedido de teste"))
        };
        let header = |response: &HttpResponse<Cow<'static, [u8]>>, name: &str| {
            response
                .headers()
                .get(name)
                .map(|value| value.to_str().unwrap_or("").to_string())
        };
        let common = |response: &HttpResponse<Cow<'static, [u8]>>| {
            assert_eq!(
                header(response, "Content-Type").as_deref(),
                Some("application/pdf")
            );
            assert_eq!(header(response, "Accept-Ranges").as_deref(), Some("bytes"));
            assert_eq!(
                header(response, "Cache-Control").as_deref(),
                Some("no-store")
            );
            assert_eq!(
                header(response, "X-Content-Type-Options").as_deref(),
                Some("nosniff")
            );
        };

        // Sem Range: o documento inteiro, com o tamanho anunciado.
        let full = serve("GET", None);
        common(&full);
        assert_eq!(full.status(), 200);
        assert_eq!(header(&full, "Content-Length"), Some(total.to_string()));
        assert_eq!(header(&full, "Content-Range"), None);
        assert_eq!(full.body().as_ref(), document.as_slice());

        // Faixa: so a fatia, com o Content-Range certo.
        let slice = serve("GET", Some("bytes=9-12"));
        common(&slice);
        assert_eq!(slice.status(), 206);
        assert_eq!(header(&slice, "Content-Length").as_deref(), Some("4"));
        assert_eq!(
            header(&slice, "Content-Range"),
            Some(format!("bytes 9-12/{total}"))
        );
        assert_eq!(slice.body().as_ref(), b"0123");

        let tail = serve("GET", Some("bytes=15-"));
        assert_eq!(tail.status(), 206);
        assert_eq!(
            header(&tail, "Content-Range"),
            Some(format!("bytes 15-{}/{total}", total - 1))
        );
        assert_eq!(tail.body().as_ref(), b"6789");

        let suffix = serve("GET", Some("bytes=-2"));
        assert_eq!(suffix.status(), 206);
        assert_eq!(
            header(&suffix, "Content-Range"),
            Some(format!("bytes {}-{}/{total}", total - 2, total - 1))
        );
        assert_eq!(suffix.body().as_ref(), b"89");

        // Varias faixas: 200 com tudo, e sem Content-Range.
        let multi = serve("GET", Some("bytes=0-1,3-4"));
        common(&multi);
        assert_eq!(multi.status(), 200);
        assert_eq!(header(&multi, "Content-Range"), None);
        assert_eq!(multi.body().as_ref(), document.as_slice());

        // Insatisfazivel: 416, corpo vazio, o total no Content-Range.
        let beyond = serve("GET", Some("bytes=100-200"));
        common(&beyond);
        assert_eq!(beyond.status(), 416);
        assert_eq!(header(&beyond, "Content-Length").as_deref(), Some("0"));
        assert_eq!(
            header(&beyond, "Content-Range"),
            Some(format!("bytes */{total}"))
        );
        assert!(beyond.body().is_empty());

        // HEAD: os cabecalhos do GET correspondente, sem corpo.
        let head = serve("HEAD", None);
        common(&head);
        assert_eq!(head.status(), 200);
        assert_eq!(header(&head, "Content-Length"), Some(total.to_string()));
        assert!(head.body().is_empty());
        let head_range = serve("HEAD", Some("bytes=9-12"));
        assert_eq!(head_range.status(), 206);
        assert_eq!(header(&head_range, "Content-Length").as_deref(), Some("4"));
        assert_eq!(
            header(&head_range, "Content-Range"),
            Some(format!("bytes 9-12/{total}"))
        );
        assert!(head_range.body().is_empty());

        // Sem documento aberto: 200 vazio sem Range, 416 com Range.
        bytes.lock().expect("slot de teste").clear();
        let empty = serve("GET", None);
        assert_eq!(empty.status(), 200);
        assert_eq!(header(&empty, "Content-Length").as_deref(), Some("0"));
        assert!(empty.body().is_empty());
        let empty_range = serve("GET", Some("bytes=0-"));
        assert_eq!(empty_range.status(), 416);
        assert_eq!(
            header(&empty_range, "Content-Range").as_deref(),
            Some("bytes */0")
        );
        assert!(empty_range.body().is_empty());
    }

    #[test]
    fn injected_scripts_capture_globals_before_the_page_runs() {
        // A capability nunca entra numa URL. O transporte e o serializador
        // sao capturados no document-created, antes de qualquer script remoto.
        for (name, script) in [
            ("keymap", NEURALIA_KEYMAP_SCRIPT),
            ("return", EXTERNAL_RETURN_BUTTON),
            ("gmail", GMAIL_MONITOR_SCRIPT),
            ("agent", AGENT_OBSERVER_SCRIPT),
            ("comparator", COMPARATOR_INJECT_SCRIPT),
        ] {
            assert!(script.contains("__NEURALIA_CAP__"), "{name}");
            assert_eq!(
                script.matches("window.chrome.webview.postMessage").count(),
                1,
                "{name}: postMessage deve ser capturado uma unica vez"
            );
            assert_eq!(
                script.matches("JSON.stringify").count(),
                1,
                "{name}: JSON.stringify deve ser capturado uma unica vez"
            );
            assert!(
                script.contains(
                    "const post = window.chrome.webview.postMessage.bind(window.chrome.webview);"
                ),
                "{name}"
            );
            assert!(
                script.contains("const stringify = JSON.stringify;"),
                "{name}"
            );
            assert!(!script.contains("?cap="), "{name}");
            assert!(script.trim_start().starts_with("(function"), "{name}");
        }

        // Os scripts que correm depois do DOMContentLoaded usam referencias
        // capturadas para as primitivas DOM que carregam autoridade.
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

        // Nenhum handler que dispare acao nativa aceita evento sintetico.
        assert_eq!(
            COMPARATOR_INJECT_SCRIPT
                .matches("if (!event.isTrusted")
                .count(),
            4
        );
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("expand.onclick"));
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("minimize.onclick"));
        assert!(EXTERNAL_RETURN_BUTTON.contains("if (!event.isTrusted) return;"));
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("if (!e.isTrusted) { return; }"));

        assert!(
            COMPARATOR_INJECT_SCRIPT
                .contains("host === 'google.com' || host.endsWith('.google.com')")
        );
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("endsWith('google.com')"));
    }

    #[test]
    fn browser_agent_bridge_is_bounded_and_has_no_arbitrary_js_channel() {
        assert!(AGENT_OBSERVER_SCRIPT.contains("rows.length >= 32"));
        assert!(AGENT_OBSERVER_SCRIPT.contains("pageText"));
        assert!(AGENT_OBSERVER_SCRIPT.contains("action:'agent-observation'"));
        assert!(AGENT_OBSERVER_SCRIPT.contains(".join('\\n').slice(0, 1200)"));
        assert!(AGENT_OBSERVER_SCRIPT.contains("post(stringify("));
        assert!(!AGENT_OBSERVER_SCRIPT.contains("?cap="));
        assert!(!AGENT_OBSERVER_SCRIPT.contains("eval("));
        assert!(!AGENT_OBSERVER_SCRIPT.contains("new Function"));
    }

    #[test]
    fn browser_agent_plan_parses_search_filter_click_and_extract() {
        let (url, commands) = parse_browser_agent_plan(
            "https://example.com | search=rust | select=tipo:artigo | click=Buscar | extract",
        )
        .unwrap();
        assert_eq!(url, "https://example.com");
        assert_eq!(commands.len(), 4);
        assert!(matches!(commands[0], BrowserAgentCommand::Search(_)));
        assert!(matches!(commands[1], BrowserAgentCommand::Select { .. }));
        assert!(matches!(commands[2], BrowserAgentCommand::Click(_)));
        assert!(matches!(commands[3], BrowserAgentCommand::Extract));
    }

    /// SPEC-0106 sobre o caminho que embarca: o que o utilizador escreve na
    /// omnibox vai para onde deve ir. `handle_input` só executa o que esta
    /// função decidir.
    #[test]
    fn route_input_sends_each_command_where_it_belongs() {
        assert_eq!(
            route_input("agent:https://example.com | extract"),
            InputRoute::Agent("https://example.com | extract".into())
        );
        assert_eq!(
            route_input("memory:rust ownership"),
            InputRoute::MemoryQuery("rust ownership".into())
        );
        assert_eq!(
            route_input("mem: borrow checker "),
            InputRoute::MemoryQuery("borrow checker".into())
        );
        assert_eq!(route_input("history:"), InputRoute::History);
        assert_eq!(
            route_input(" research:compare "),
            InputRoute::ResearchCompare
        );
        assert_eq!(
            route_input("RESEARCH:SYNTHESIZE"),
            InputRoute::ResearchSynthesize
        );
        assert_eq!(route_input("research:export"), InputRoute::ResearchExport);
        assert_eq!(route_input("o que é ownership"), InputRoute::Intent);
        assert_eq!(route_input("https://example.com"), InputRoute::Intent);
    }

    /// SPEC-0100 / SPEC-0006 no caminho que embarca: "Private/incognito
    /// navigation never enters semantic memory" é uma restrição inegociável do
    /// `md/README.md`. O gate que existia para isto contava ocorrências de
    /// `if !private` no texto do ficheiro — passava com a condição invertida.
    #[test]
    fn private_split_source_never_becomes_a_memory_document() {
        let url = Url::parse("https://exemplo.pt/artigo").unwrap();

        assert!(split_source_memory(&url, "ChatGPT", true).is_none());

        let (title, document) = split_source_memory(&url, "ChatGPT", false)
            .expect("uma fonte não privada entra na memória");
        assert_eq!(title, "Fonte · exemplo.pt");
        assert!(!document.private);
        assert_eq!(document.provider.as_deref(), Some("ChatGPT"));
        assert_eq!(document.url.as_deref(), Some("https://exemplo.pt/artigo"));
        assert!(matches!(document.kind, MemoryKind::Source));
        assert!(matches!(document.source_kind, MemorySourceKind::Web));
    }

    #[test]
    fn memory_rebuild_is_not_swallowed_by_the_memory_prefix() {
        // `memory:rebuild` começa por `memory:`. Enquanto o prefixo foi
        // testado primeiro, o ramo do rebuild era inalcançável: o comando
        // procurava a palavra "rebuild" na memória e dizia "Buscando na
        // memória local…". A ordem aqui é o próprio comportamento.
        assert_eq!(route_input("memory:rebuild"), InputRoute::MemoryRebuild);
        assert_eq!(route_input(" Memory:Rebuild "), InputRoute::MemoryRebuild);
        assert_eq!(route_input("mem:rebuild"), InputRoute::MemoryRebuild);

        // E o prefixo continua a funcionar para tudo o resto.
        assert_eq!(
            route_input("memory:rebuilding a parser"),
            InputRoute::MemoryQuery("rebuilding a parser".into())
        );
    }

    #[test]
    fn browser_agent_plan_refuses_empty_commands_instead_of_dropping_them() {
        // Antes, estes eram descartados em silêncio. Como eram os únicos
        // comandos do plano, o agente acabava a correr um `extract` -- a
        // guardar a página na memória semântica em vez de fazer o que lhe foi
        // pedido, sem uma palavra ao utilizador.
        for spec in [
            "https://example.com | click=",
            "https://example.com | click=   ",
            "https://example.com | clique=",
            "https://example.com | search=",
            "https://example.com | pesquisar=  ",
        ] {
            let result = parse_browser_agent_plan(spec);
            assert!(result.is_err(), "{spec} devia ser recusado: {result:?}");
        }
    }

    #[test]
    fn browser_agent_plan_refuses_half_written_select() {
        // Um rótulo vazio não é "qualquer campo": o `find_agent_element`
        // devolvia o primeiro select/combobox da página, por ordem do DOM.
        for spec in [
            "https://example.com | select=:artigo",
            "https://example.com | select=  :artigo",
            "https://example.com | select=tipo:",
            "https://example.com | select=tipo",
        ] {
            let result = parse_browser_agent_plan(spec);
            assert!(result.is_err(), "{spec} devia ser recusado: {result:?}");
        }

        assert!(parse_browser_agent_plan("https://example.com | select=tipo:artigo").is_ok());
    }

    #[test]
    fn comparator_captures_provider_answers_for_research_session() {
        assert!(
            COMPARATOR_INJECT_SCRIPT.contains("act('research-answer', { col:colIndex, text })")
        );
        assert!(COMPARATOR_INJECT_SCRIPT.contains("data-message-author-role"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("scheduleResearchAnswer"));
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("neuralia:research-answer"));
    }

    /// O comportamento que o dono descreveu em duas frases: "clicou abre,
    /// segurou control abre em outra aba".
    ///
    /// O guard que b8f6fc2 acrescentou tirou o desvio do clique simples e nao
    /// pos nada no lugar: o clique deixou de ter tratamento nenhum. E o
    /// caminho do Ctrl era testado apenas por uma assercao sobre o TEXTO do
    /// script, que continuava verde com a funcionalidade partida.
    #[test]
    fn a_plain_click_opens_in_all_three_panels_and_ctrl_click_opens_beside() {
        let url = "https://example.com/fonte".to_string();

        match App::column_ipc_event_impl(
            1,
            IpcAction::Link {
                col: 1,
                url: url.clone(),
                aside: false,
            },
        ) {
            Some(UserEvent::OpenEverywhere(opened)) => assert_eq!(opened, url),
            other => panic!("clique simples devia abrir nas tres colunas, veio {other:?}"),
        }

        match App::column_ipc_event_impl(
            1,
            IpcAction::Link {
                col: 1,
                url: url.clone(),
                aside: true,
            },
        ) {
            Some(UserEvent::OpenSplit {
                source_index,
                url: opened,
            }) => {
                assert_eq!(source_index, 1);
                assert_eq!(opened, url);
            }
            other => panic!("Ctrl+clique devia abrir ao lado, veio {other:?}"),
        }
    }

    #[test]
    fn a_click_reported_by_another_column_is_ignored() {
        // Cada coluna tem o seu handler de IPC. Sem esta verificacao, uma
        // pagina numa coluna mandava a outra abrir o que lhe apetecesse.
        assert!(
            App::column_ipc_event_impl(
                0,
                IpcAction::Link {
                    col: 2,
                    url: "https://example.com/".into(),
                    aside: true,
                },
            )
            .is_none()
        );
    }

    #[test]
    fn comparator_has_split_palette_and_real_three_way_submit() {
        assert!(
            COMPARATOR_INJECT_SCRIPT
                .contains("act('link', { col:colIndex, url:target.href, aside:aside })")
        );
        // O botao do meio chega como `auxclick`; dentro de `click` o
        // `event.button` e sempre 0. Ter isto aqui e presenca, nao
        // comportamento -- o que decide para onde vai o clique esta em
        // `column_ipc_event_impl`, e esse tem teste a serio.
        assert!(COMPARATOR_INJECT_SCRIPT.contains("'auxclick'"));
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("act('palette', { col:colIndex })"));
        assert!(!NEURALIA_KEYMAP_SCRIPT.contains("q="));
        assert!(SPLIT_SCROLL_RAIL_SCRIPT.contains("neuralia-split-scroll-rail"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("chatgpt.com"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("claude.ai"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("button.click()"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("form.requestSubmit"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("lastSubmitAt"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("setTimeout(submitWhenReady, 150)"));
    }

    #[test]
    fn palette_is_native_and_the_page_can_only_ask_for_it() {
        let source = include_str!("windows_app.rs");
        assert!(!source.contains(concat!("NEURALIA_PALETTE", "_SCRIPT")));
        assert!(!source.contains(concat!("neuralia-open-", "palette")));
        assert!(!NEURALIA_KEYMAP_SCRIPT.contains("CustomEvent"));
        assert!(NEURALIA_KEYMAP_SCRIPT.contains("act('palette', { col:colIndex })"));

        for builder in ["fn comparator_webview_builder", "fn split_webview_builder"] {
            let body = source
                .split(builder)
                .nth(1)
                .and_then(|part| part.split(".with_new_window_req_handler").next())
                .expect(builder);
            assert!(body.contains("with_ipc_handler"), "{builder}");
            assert!(!body.contains("neuralia_query_param"), "{builder}");
            assert!(!body.contains("PaletteSubmit"), "{builder}");
        }

        // A palette da COLUNA ja nao se verifica por texto: o despacho saiu do
        // closure para uma funcao, e agora chama-se.
        assert!(matches!(
            App::column_ipc_event_impl(1, IpcAction::Palette { col: 1 }),
            Some(UserEvent::OpenPalette(1))
        ));
        // O painel lateral mantem o despacho dentro do closure, e por isso
        // continua a ser so presenca.
        let split_body = source
            .split("fn split_webview_builder")
            .nth(1)
            .and_then(|part| part.split(".with_new_window_req_handler").next())
            .expect("split builder");
        assert!(split_body.contains("IpcAction::Palette"));
        assert!(split_body.contains("UserEvent::OpenPalette("));

        let edit = source
            .split("fn palette_edit_subclass")
            .nth(1)
            .and_then(|part| part.split("unsafe fn window_text").next())
            .expect("edit subclass");
        assert!(edit.contains("window_text(hwnd)"));
        assert!(edit.contains("host.source.get()"));
        assert!(edit.contains("UserEvent::PaletteSubmit {"));
        assert!(edit.contains("VK_ESCAPE"));
        assert!(edit.contains("WM_KILLFOCUS"));

        let show = source
            .split("fn show_palette")
            .nth(1)
            .and_then(|part| part.split("fn position_palette").next())
            .expect("show_palette body");
        assert!(!show.contains("WS_EX_NOACTIVATE"));
        assert!(!show.contains("WS_EX_TOPMOST"));
        assert!(show.contains("WS_POPUP"));
        assert!(show.contains("ES_AUTOHSCROLL"));
        assert!(show.contains("EM_SETLIMITTEXT, 2048"));
        assert!(show.contains("EM_SETCUEBANNER"));
        assert!(show.contains("SetFocus(edit)"));
    }

    #[test]
    fn palette_routes_private_input_away_from_the_normal_column() {
        let split = |route: PaletteRoute| match route {
            PaletteRoute::OpenSplit { url, private } => (url.to_string(), private),
            other => panic!("esperava OpenSplit, veio {other:?}"),
        };
        assert_eq!(
            split(route_palette("https://example.org/a", 0, true)),
            ("https://example.org/a".to_string(), true)
        );
        assert_eq!(
            split(route_palette("https://example.org/a", 1, false)),
            ("https://example.org/a".to_string(), false)
        );
        // Rede local digitada pelo utilizador: a palette e entrada nativa, nao
        // da pagina, por isso a rota nao a barra (SPEC-0015).
        assert_eq!(
            split(route_palette("http://localhost:8080/", 0, false)),
            ("http://localhost:8080/".to_string(), false)
        );
        assert_eq!(
            split(route_palette("http://192.168.1.1/", 2, true)),
            ("http://192.168.1.1/".to_string(), true)
        );

        assert_eq!(
            route_palette("qual a capital de Angola", 2, true),
            PaletteRoute::OpenPrivateProvider {
                query: "qual a capital de Angola".to_string()
            }
        );
        assert_eq!(
            route_palette("  qual a capital de Angola  ", 2, false),
            PaletteRoute::LoadProvider {
                query: "qual a capital de Angola".to_string()
            }
        );
        assert_eq!(route_palette("home:", 0, true), PaletteRoute::Home);
        assert_eq!(route_palette("   ", 0, false), PaletteRoute::Invalid(None));
        assert_eq!(
            route_palette("texto", COMPARATOR_COLUMNS, false),
            PaletteRoute::Invalid(None)
        );
        assert!(matches!(
            route_palette("javascript:alert(1)", 0, false),
            PaletteRoute::Invalid(Some(_))
        ));
    }

    #[test]
    fn private_palette_paths_never_touch_history_or_context_tabs() {
        let source = include_str!("windows_app.rs");
        let submit = source
            .split("fn submit_palette")
            .nth(1)
            .and_then(|part| part.split("fn go_back").next())
            .expect("submit body");
        // So o caminho normal (LoadProvider) grava historico, e e o ultimo
        // braco: tudo o que vem antes (URL, privado) nunca chama record().
        let (before, load_provider) = submit
            .split_once("PaletteRoute::LoadProvider")
            .expect("LoadProvider arm");
        assert!(!before.contains("record("));
        assert_eq!(load_provider.matches("self.record(").count(), 1);
        assert!(before.contains("PaletteRoute::OpenPrivateProvider"));
        assert!(before.contains("open_split_mode(source_index, url.to_string(), false, true)"));

        // A parte da memória passou a ser testada pelo comportamento, em
        // `private_split_source_never_becomes_a_memory_document`: contar
        // ocorrências de `if !private` passava com a condição invertida. Aqui
        // fica o que só o texto prova -- que este caminho não escreve
        // histórico -- e a ligação à função que decide.
        let split = source
            .split("fn open_split_mode")
            .nth(1)
            .and_then(|part| part.split("fn open_private_panel").next())
            .expect("split body");
        assert!(split.contains("split_source_memory(&valid, source_name, private)"));
        assert_eq!(split.matches("if !private").count(), 1);
        assert!(!split.contains("self.record("));
        let private_split = source
            .split("UserEvent::OpenPrivateSplit { source_index, url } =>")
            .nth(1)
            .and_then(|part| part.split("UserEvent::NewTab").next())
            .expect("OpenPrivateSplit arm");
        assert!(private_split.contains("open_split_mode(source_index, url, false, true)"));
    }

    #[test]
    fn column_spans_share_the_width_and_the_palette_sits_on_its_column() {
        let spans = visible_column_spans(1200.0, 3, &[1.0; 3], &[false; 3]);
        assert_eq!(spans.len(), 3);
        assert_eq!(
            spans[1],
            ColumnSpan {
                index: 1,
                x: 400.0,
                width: 400.0
            }
        );
        assert_eq!(spans[2].x + spans[2].width, 1200.0);

        // Coluna minimizada nao ocupa faixa; a ultima absorve o resto.
        let spans = visible_column_spans(1000.0, 3, &[2.0, 1.0, 1.0], &[false, true, false]);
        assert_eq!(
            spans.iter().map(|span| span.index).collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert!((spans[0].width - 2000.0 / 3.0).abs() < 1e-9);
        assert!((spans[1].x + spans[1].width - 1000.0).abs() < 1e-9);

        // Pesos nulos nao dividem por zero; menos colunas que o maximo tambem.
        let spans = visible_column_spans(900.0, 3, &[0.0; 3], &[false; 3]);
        assert!((spans[0].width - 300.0).abs() < 1e-9);
        assert_eq!(
            visible_column_spans(900.0, 2, &[1.0; 3], &[false; 3]).len(),
            2
        );
        assert!(visible_column_spans(900.0, 3, &[1.0; 3], &[true; 3]).is_empty());

        // Palette: centrada na coluna, nunca mais larga que ela menos as
        // margens, e nunca acima da barra.
        let geometry = palette_geometry(
            ColumnSpan {
                index: 1,
                x: 400.0,
                width: 400.0,
            },
            800.0,
        );
        assert_eq!(geometry.width, 352.0);
        assert_eq!(geometry.x, 424.0);
        assert_eq!(geometry.height, PALETTE_HEIGHT);
        assert!(geometry.y > COMPARATOR_CHROME_HEIGHT);
        let wide = palette_geometry(
            ColumnSpan {
                index: 0,
                x: 0.0,
                width: 1600.0,
            },
            800.0,
        );
        assert_eq!(wide.width, PALETTE_MAX_WIDTH);
        assert_eq!(wide.x, 460.0);
        let narrow = palette_geometry(
            ColumnSpan {
                index: 2,
                x: 700.0,
                width: 100.0,
            },
            800.0,
        );
        assert_eq!(narrow.width, PALETTE_MIN_WIDTH);

        assert!(palette_hint("ChatGPT", false).contains("ChatGPT"));
        assert!(palette_hint("ChatGPT", true).contains("privado"));
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

        // O arrasto e coalescido: a subclasse publica a ultima posicao e so
        // acorda o event loop quando nao ha pedido pendente. Sem isto cada
        // WM_MOUSEMOVE reposicionava tres WebView2 a mais de 100 Hz.
        let source = include_str!("windows_app.rs");
        let subclass = source
            .split("fn comparator_splitter_subclass")
            .nth(1)
            .and_then(|part| part.split("fn exit_button_subclass").next())
            .expect("subclass body");
        assert!(subclass.contains("RESIZE_X.store("));
        assert!(subclass.contains("RESIZE_PENDING.swap(true"));
        assert!(!subclass.contains("ResizeComparator {"));

        // E o handler liberta a marca ANTES de ler, para nao engolir o
        // movimento que chegar a meio do reposicionamento.
        let handler = source
            .split("UserEvent::ResizeComparator =>")
            .nth(1)
            .and_then(|part| part.split("UserEvent::RestoreComparator").next())
            .expect("handler body");
        let cleared = handler
            .find("RESIZE_PENDING.store(false")
            .expect("limpa a marca");
        let read = handler.find("RESIZE_X.load(").expect("le a posicao");
        assert!(cleared < read);
    }

    #[test]
    fn first_comparator_layout_retries_without_waiting_for_mouse_input() {
        assert_eq!(COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS.len(), 2);
        assert!(COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS[0] > 0);
        assert!(
            COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS[1] > COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS[0]
        );
    }

    #[test]
    fn comparator_minimize_control_is_wired_and_layout_keeps_one_visible() {
        assert!(COMPARATOR_INJECT_SCRIPT.contains("neuralia-comp-minimize"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("act('minimize', { col:colIndex })"));
        assert!(!COMPARATOR_INJECT_SCRIPT.contains("neuralia:minimize"));
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
        assert!(GMAIL_MONITOR_SCRIPT.contains("action:'gmail-state'"));
        assert!(GMAIL_MONITOR_SCRIPT.contains("post(stringify("));
    }

    #[test]
    fn spec_0108_remote_scripts_use_message_transport_without_capability_urls() {
        for (name, script) in [
            ("keymap", NEURALIA_KEYMAP_SCRIPT),
            ("return", EXTERNAL_RETURN_BUTTON),
            ("gmail", GMAIL_MONITOR_SCRIPT),
            ("agent", AGENT_OBSERVER_SCRIPT),
            ("comparator", COMPARATOR_INJECT_SCRIPT),
        ] {
            assert!(
                script.contains("window.chrome.webview.postMessage"),
                "{name}"
            );
            assert!(script.contains("JSON.stringify"), "{name}");
            assert!(!script.contains("?cap="), "{name}");
            assert!(
                !script.contains("window.location.href = 'neuralia:"),
                "{name}"
            );
        }
    }

    #[test]
    fn spec_0108_remote_navigation_handlers_reject_neuralia_scheme() {
        let source = include_str!("windows_app.rs");
        for builder in [
            "fn pdf_webview_builder",
            "fn external_webview_builder",
            "fn comparator_webview_builder",
            "fn split_webview_builder",
            "fn maybe_start_gmail_monitor",
        ] {
            let body = source
                .split(builder)
                .nth(1)
                .and_then(|part| part.split(".with_permission_handler").next())
                .expect(builder);
            assert!(body.contains("with_ipc_handler"), "{builder}");
            assert!(
                body.contains("eq_ignore_ascii_case(\"neuralia:\")"),
                "{builder}"
            );
            assert!(!body.contains("remote_neuralia_action"), "{builder}");
        }
    }

    #[test]
    fn spec_0108_capability_scripts_are_top_frame_only() {
        for (name, script) in [
            ("keymap", NEURALIA_KEYMAP_SCRIPT),
            ("return", EXTERNAL_RETURN_BUTTON),
            ("gmail", GMAIL_MONITOR_SCRIPT),
            ("agent", AGENT_OBSERVER_SCRIPT),
            ("comparator", COMPARATOR_INJECT_SCRIPT),
        ] {
            let guard = script
                .find("if (window.top !== window) return;")
                .expect("top-frame guard");
            let capability = script
                .find("const capability = '__NEURALIA_CAP__';")
                .expect("capability declaration");
            assert!(
                guard < capability,
                "{name}: frame guard must run before capability use"
            );
        }
    }

    #[test]
    fn spec_0108_agent_observation_stays_below_ipc_envelope_limit() {
        assert!(
            AGENT_OBSERVER_SCRIPT.contains(".join('\\n').slice(0, 1200)"),
            "agent payload must be bounded before JSON serialization"
        );
        // JSON escaping may expand one UTF-16 code unit to six ASCII bytes.
        // 1200 * 6 leaves >900 bytes for the protocol envelope under 8 KiB.
        const { assert!(1200 * 6 + 900 < crate::ipc::IPC_MAX_BYTES) };
    }

    #[test]
    fn spec_0108_comparator_captures_timers_with_ipc_primitives() {
        let top = COMPARATOR_INJECT_SCRIPT
            .split("listen(document, 'DOMContentLoaded'")
            .next()
            .expect("comparator prelude");
        assert!(top.contains("window.chrome.webview.postMessage"));
        assert!(top.contains("JSON.stringify"));
        assert!(top.contains("const defer = setTimeout;"));
        assert!(top.contains("const cancelDefer = clearTimeout;"));
    }

    #[test]
    fn capability_generation_fails_closed_when_system_rng_fails() {
        let bytes = [0xabu8; 16];
        assert_eq!(
            capability_from_rng(0, bytes),
            Some("abababababababababababababababab".to_string())
        );
        assert_eq!(capability_from_rng(-1, bytes), None);
    }

    #[test]
    fn gmail_monitor_observer_is_coalesced_like_the_others() {
        // O observer so ve a arvore (childList+subtree), passa por um quadro
        // e so depois pelo debounce; o emit periodico continua como rede.
        assert!(GMAIL_MONITOR_SCRIPT.contains("requestAnimationFrame"));
        assert!(GMAIL_MONITOR_SCRIPT.contains("childList:true, subtree:true"));
        // Pela sintaxe das opcoes, nao pela palavra: o comentario do script
        // explica porque se tiraram e usa os mesmos nomes.
        assert!(!GMAIL_MONITOR_SCRIPT.contains("characterData:true"));
        assert!(!GMAIL_MONITOR_SCRIPT.contains("attributes:true"));
        assert!(GMAIL_MONITOR_SCRIPT.contains("setTimeout(emit, 450)"));
        assert!(GMAIL_MONITOR_SCRIPT.contains("setInterval(emit, 15000)"));
    }

    #[test]
    fn gmail_fields_are_capped_natively_on_char_boundary() {
        let long = "a".repeat(1000);
        assert_eq!(gmail_field(long).chars().count(), GMAIL_FIELD_MAX_CHARS);

        // Dois bytes por char: cortar por bytes cairia a meio de um 'ç'.
        let accented = "ç".repeat(1000);
        let cut = gmail_field(accented);
        assert_eq!(cut.chars().count(), GMAIL_FIELD_MAX_CHARS);
        assert!(cut.chars().all(|c| c == 'ç'));

        // O que cabe nao e tocado.
        assert_eq!(
            gmail_field("Ana <ana@example.com>".to_string()),
            "Ana <ana@example.com>"
        );
        assert_eq!(gmail_field(String::new()), "");
        let exact = "x".repeat(GMAIL_FIELD_MAX_CHARS);
        assert_eq!(gmail_field(exact.clone()), exact);
    }

    #[test]
    fn gmail_monitor_switch_depends_on_presence_not_value() {
        assert!(gmail_monitor_enabled_for(None));
        assert!(!gmail_monitor_enabled_for(Some(OsString::from("1"))));
        // Interruptor de privacidade: definir mal ainda desliga.
        assert!(!gmail_monitor_enabled_for(Some(OsString::from("0"))));
        assert!(!gmail_monitor_enabled_for(Some(OsString::new())));
    }

    #[test]
    fn timer_queue_pops_in_deadline_order_with_arrival_tiebreak() {
        let base = Instant::now();
        let mut queue = TimerQueue::new();
        queue.push(base + Duration::from_millis(300), "c");
        queue.push(base + Duration::from_millis(100), "a1");
        queue.push(base + Duration::from_millis(200), "b");
        queue.push(base + Duration::from_millis(100), "a2");

        assert_eq!(
            queue.next_deadline(),
            Some(base + Duration::from_millis(100))
        );

        let far = base + Duration::from_secs(10);
        let mut order = Vec::new();
        while let Some(event) = queue.pop_due(far) {
            order.push(event);
        }
        // Prazos iguais saem na ordem em que foram pedidos.
        assert_eq!(order, vec!["a1", "a2", "b", "c"]);
        assert_eq!(queue.next_deadline(), None);
    }

    #[test]
    fn timer_queue_only_pops_what_is_due() {
        let base = Instant::now();
        let mut queue = TimerQueue::new();
        queue.push(base + Duration::from_millis(50), "soon");
        queue.push(base + Duration::from_millis(500), "later");

        // Antes do primeiro prazo nada sai, mas a fila diz quanto dormir.
        assert_eq!(queue.pop_due(base), None);
        assert_eq!(
            queue.next_deadline(),
            Some(base + Duration::from_millis(50))
        );

        // No prazo exacto ja conta como vencido.
        assert_eq!(
            queue.pop_due(base + Duration::from_millis(50)),
            Some("soon")
        );
        assert_eq!(queue.pop_due(base + Duration::from_millis(51)), None);
        assert_eq!(
            queue.next_deadline(),
            Some(base + Duration::from_millis(500))
        );

        assert_eq!(
            queue.pop_due(base + Duration::from_millis(500)),
            Some("later")
        );
        assert_eq!(queue.next_deadline(), None);
    }

    #[test]
    fn timer_queue_empty_has_no_deadline_and_pops_nothing() {
        let mut queue: TimerQueue<u8> = TimerQueue::new();
        assert_eq!(queue.next_deadline(), None);
        assert_eq!(queue.pop_due(Instant::now()), None);
        assert_eq!(
            queue.pop_due(Instant::now() + Duration::from_secs(3600)),
            None
        );

        // Esvaziar e voltar a encher nao deixa nada para tras.
        let now = Instant::now();
        queue.push(now, 7);
        assert_eq!(queue.pop_due(now), Some(7));
        assert_eq!(queue.next_deadline(), None);
        assert_eq!(queue.pop_due(now), None);
    }

    #[test]
    fn context_tabs_live_in_browser_title_bar() {
        let layout = BarLayout::with_contexts(1600.0, 1.0, true, BarColumns::even(3), [3, 3, 3]);
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
        let layout = BarLayout::with_contexts(1400.0, 1.0, true, BarColumns::even(3), [1, 1, 1]);
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
        let layout = BarLayout::with_contexts(1600.0, 1.0, true, BarColumns::even(3), [2, 1, 4]);

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

    /// A barra tem de usar a MESMA reparticao que os WebViews. Antes recebia
    /// so o numero de colunas e desenhava tres partes iguais: depois de
    /// arrastar um divisor o rotulo da IA ficava sobre a coluna do lado, e o
    /// hit-testing -- que le as mesmas caixas -- ia atras dele.
    #[test]
    fn bar_columns_follow_the_dragged_weights() {
        let dragged = BarColumns {
            count: 3,
            weights: [2.0, 1.0, 1.0],
            minimized: [false; COMPARATOR_COLUMNS],
            split_active: false,
        };
        let layout = BarLayout::with_contexts(1600.0, 1.0, true, dragged, [0; 3]);
        let spans = visible_column_spans(1600.0, 3, &dragged.weights, &dragged.minimized);

        for span in &spans {
            let provider = layout.columns[span.index];
            let plus = layout.add_tabs[span.index];
            assert!(
                provider.x >= span.x,
                "coluna {} comeca antes da sua faixa",
                span.index
            );
            assert!(
                plus.x + plus.width <= span.x + span.width,
                "o + da coluna {} passa para a faixa seguinte",
                span.index
            );
            let center = (
                provider.x + provider.width / 2.0,
                provider.y + provider.height / 2.0,
            );
            assert_eq!(
                layout.hit(center.0, center.1),
                Some(BarHit::Column(span.index))
            );
        }

        // A primeira coluna e a mais larga: o rotulo do meio tem de ter
        // andado para a direita face as colunas iguais.
        let even = BarLayout::new(1600.0, 1.0, true, 3);
        assert!(layout.columns[1].x > even.columns[1].x);
    }

    /// Coluna minimizada continua na barra, como chip encostado a direita:
    /// e o unico sitio por onde ela volta.
    #[test]
    fn minimized_columns_become_chips_next_to_the_right_controls() {
        let state = BarColumns {
            count: 3,
            weights: [1.0; COMPARATOR_COLUMNS],
            minimized: [false, true, false],
            split_active: false,
        };
        let layout = BarLayout::with_contexts(1600.0, 1.0, true, state, [0; 3]);
        assert_eq!(layout.minimized, [false, true, false]);

        let chip = layout.columns[1];
        let controls = right_controls(1600.0, 1.0, false);
        assert!(chip.width > 0.0);
        assert!(
            chip.x + chip.width <= controls.private.x,
            "o chip nao pode tapar o botao Privado"
        );
        assert!(
            chip.x > layout.columns[2].x + layout.columns[2].width,
            "o chip fica a direita das colunas que ainda se veem"
        );
        assert_eq!(
            layout.hit(chip.x + chip.width / 2.0, chip.y + chip.height / 2.0),
            Some(BarHit::Column(1)),
            "clicar no chip tem de restaurar a coluna"
        );
        // Sem faixa nao ha onde abrir uma aba: o "+" desaparece e nao rouba
        // o clique ao canto superior esquerdo da janela.
        assert_eq!(layout.add_tabs[1].width, 0.0);
        assert_eq!(layout.hit(0.0, 0.0), None);

        // As duas que ficam repartem a largura toda, tal como as WebViews.
        let spans = visible_column_spans(1600.0, 3, &state.weights, &state.minimized);
        assert_eq!(spans.len(), 2);
        for span in &spans {
            assert!(layout.columns[span.index].x >= span.x);
        }
    }

    /// Com a gaveta aberta os controlos do Split ocupam o canto; o Privado
    /// recua e tudo o que se encosta a direita recua com ele.
    #[test]
    fn right_controls_make_room_for_the_split_drawer() {
        let plain = right_controls(1600.0, 1.0, false);
        assert!(plain.split.is_none());
        assert_eq!(plain.private.x + plain.private.width, 1600.0 - 8.0);

        let drawer = right_controls(1600.0, 1.0, true);
        let (label, expand, close) = drawer.split.expect("ha gaveta");
        assert_eq!(close.x + close.width, 1600.0 - 8.0);
        assert!(expand.x + expand.width < close.x);
        assert!(label.x + label.width < expand.x);
        assert!(drawer.private.x + drawer.private.width <= label.x);
    }

    /// Arrastar um divisor so mexe no par vizinho, nunca fecha um painel
    /// abaixo do minimo e nao inventa nem perde largura pelo caminho.
    #[test]
    fn resized_weights_keep_the_total_and_the_minimum() {
        let weights = [1.0, 1.0, 1.0];
        let visible = [0usize, 1, 2];
        let total = |w: &[f64; COMPARATOR_COLUMNS]| w.iter().sum::<f64>();

        // Divisor 0 largado a 600 de 1200: a esquerda fica com 600 dos 800
        // do par, a direita com o resto, e a terceira coluna nao se mexe.
        let next = resized_weights(&weights, &visible, 0, 600.0, 1200.0);
        assert!((next[0] - 1.5).abs() < 1e-9);
        assert!((next[1] - 0.5).abs() < 1e-9);
        assert_eq!(next[2], weights[2]);
        assert!((total(&next) - total(&weights)).abs() < 1e-9);

        // Puxar para fora do ecra nao colapsa o painel: para no minimo.
        let crushed = resized_weights(&weights, &visible, 0, -5000.0, 1200.0);
        assert!((total(&crushed) - total(&weights)).abs() < 1e-9);
        let span = 1200.0 * (crushed[0] + crushed[1]) / total(&crushed);
        assert!((1200.0 * crushed[0] / total(&crushed) - MIN_PANEL_WIDTH).abs() < 1e-9);
        let stretched = resized_weights(&weights, &visible, 0, 9000.0, 1200.0);
        assert!(
            (1200.0 * stretched[1] / total(&stretched) - MIN_PANEL_WIDTH).abs() < 1e-9,
            "o painel da direita tambem tem minimo"
        );
        assert!(span > 2.0 * MIN_PANEL_WIDTH);

        // O segundo divisor conta a partir do fim da primeira coluna.
        let second = resized_weights(&weights, &visible, 1, 1000.0, 1200.0);
        assert_eq!(second[0], weights[0]);
        assert!(second[1] > weights[1] && second[2] < weights[2]);
        assert!((total(&second) - total(&weights)).abs() < 1e-9);

        // Uma coluna minimizada tira um divisor da conta: o que sobra nao
        // existe e os pesos voltam intactos.
        assert_eq!(
            resized_weights(&weights, &[0, 2], 1, 600.0, 1200.0),
            weights
        );
        assert_eq!(resized_weights(&weights, &[0], 0, 600.0, 1200.0), weights);
    }

    /// Splash, toast, botao de saida e divisores sao popups OWNED: ficam
    /// acima do WebView2 por serem owned, nao por serem TOPMOST. Com TOPMOST
    /// flutuavam sobre outras aplicacoes depois de um Alt+Tab.
    #[test]
    fn owned_popups_are_not_topmost() {
        let source = include_str!("windows_app.rs");
        let body = |from: &str, to: &str| {
            source
                .split(from)
                .nth(1)
                .and_then(|part| part.split(to).next())
                .unwrap_or_else(|| panic!("corpo de {from}"))
                .to_string()
        };
        for (from, to) in [
            ("fn show_splash", "fn position_splash"),
            ("fn show_gmail_toast", "fn position_gmail_toast"),
            ("fn sync_exit_button", "fn position_exit_button"),
            ("fn sync_comparator_splitters", "fn resize_comparator"),
        ] {
            let text = body(from, to);
            assert!(
                text.contains("CreateWindowExW"),
                "{from} devia criar a janela"
            );
            assert!(
                !text.contains("WS_EX_TOPMOST"),
                "{from} nao pode pousar sobre as outras aplicacoes"
            );
        }
    }

    #[test]
    fn bar_layout_hit_matches_drawing() {
        let layout = BarLayout::new(1600.0, 1.0, true, 3);
        assert_eq!(layout.minimized, [false; COMPARATOR_COLUMNS]);

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

    // ---------- grupos de abas ----------

    fn tab(url: &str, group: Option<u64>) -> ContextTab {
        ContextTab {
            url: url.to_string(),
            group,
        }
    }

    fn group(id: u64, collapsed: bool) -> ContextGroup {
        ContextGroup {
            id,
            name: format!("G{id}"),
            color: GroupColor::Blue,
            collapsed,
        }
    }

    #[test]
    fn a_collapsed_group_hides_its_tabs_and_keeps_its_pill() {
        let tabs = vec![
            tab("https://a.example/1", Some(7)),
            tab("https://b.example/2", Some(7)),
            tab("https://c.example/3", None),
        ];
        let open = plan_tab_row(&tabs, &[group(7, false)]);
        assert_eq!(
            open.visible(),
            &[
                TabSlot::Group(0),
                TabSlot::Tab(0),
                TabSlot::Tab(1),
                TabSlot::Tab(2)
            ]
        );

        let shut = plan_tab_row(&tabs, &[group(7, true)]);
        // A pilula fica -- e o unico sitio onde o grupo se reabre. As abas
        // saem da barra sem deixarem de estar abertas.
        assert_eq!(shut.visible(), &[TabSlot::Group(0), TabSlot::Tab(2)]);
    }

    #[test]
    fn a_tab_whose_group_vanished_stays_on_the_bar_as_a_loose_tab() {
        // O grupo 7 ja nao existe: a aba tem de voltar a ser solta, nao
        // desaparecer com ele.
        let tabs = vec![tab("https://a.example/1", Some(7))];
        let row = plan_tab_row(&tabs, &[]);
        assert_eq!(row.visible(), &[TabSlot::Tab(0)]);
    }

    /// Confirma a invariante em qualquer fila: nenhuma aba agrupada aparece
    /// sem a pilula do seu grupo antes dela, e o tecto de abas e respeitado.
    fn assert_no_orphans(row: &TabRow, tabs: &[ContextTab], groups: &[ContextGroup]) {
        let mut seen: Vec<usize> = Vec::new();
        for slot in row.visible() {
            match slot {
                TabSlot::Group(index) => seen.push(*index),
                TabSlot::Tab(index) => {
                    if let Some(id) = tabs[*index].group
                        && let Some(owner) = groups.iter().position(|group| group.id == id)
                    {
                        assert!(
                            seen.contains(&owner),
                            "aba {index} aparece sem a pilula do grupo {owner}"
                        );
                    }
                }
            }
        }
        assert!(
            row.visible()
                .iter()
                .filter(|slot| matches!(slot, TabSlot::Tab(_)))
                .count()
                <= MAX_VISIBLE_CONTEXT_TABS
        );
    }

    #[test]
    fn a_group_that_fits_is_shown_whole() {
        let tabs = vec![
            tab("https://a.example/1", Some(1)),
            tab("https://b.example/2", Some(1)),
            tab("https://c.example/3", None),
        ];
        let groups = vec![group(1, false)];
        let row = plan_tab_row(&tabs, &groups);
        assert_eq!(
            row.visible(),
            &[
                TabSlot::Group(0),
                TabSlot::Tab(0),
                TabSlot::Tab(1),
                TabSlot::Tab(2)
            ]
        );
        assert_no_orphans(&row, &tabs, &groups);
    }

    #[test]
    fn the_overflow_cut_never_leaves_a_tab_without_its_pill() {
        // O corte cai a meio do grupo: sem a correcao sobravam as duas ultimas
        // abas do grupo sem pilula nenhuma a dizer de quem sao.
        let tabs = vec![
            tab("https://a.example/1", Some(1)),
            tab("https://b.example/2", Some(1)),
            tab("https://c.example/3", Some(1)),
            tab("https://d.example/4", None),
        ];
        let groups = vec![group(1, false)];
        let row = plan_tab_row(&tabs, &groups);
        assert_no_orphans(&row, &tabs, &groups);
        assert_eq!(row.visible(), &[TabSlot::Tab(3)]);

        // E com dois grupos seguidos, o mesmo: o que entra entra inteiro.
        let many = vec![
            tab("https://a.example/1", None),
            tab("https://b.example/2", Some(1)),
            tab("https://c.example/3", Some(1)),
            tab("https://d.example/4", Some(2)),
            tab("https://e.example/5", Some(2)),
            tab("https://f.example/6", None),
        ];
        let pair = vec![group(1, false), group(2, false)];
        assert_no_orphans(&plan_tab_row(&many, &pair), &many, &pair);
    }

    #[test]
    fn joining_a_group_parks_the_tab_next_to_the_other_members() {
        // Sem isto a pilula ficava a rotular a aba errada: os membros tem de
        // ser contiguos na barra.
        let mut tabs = vec![
            tab("https://a.example/1", Some(1)),
            tab("https://b.example/2", None),
            tab("https://c.example/3", None),
        ];
        join_context_group(&mut tabs, 1, 2);
        assert_eq!(tabs[0].url, "https://a.example/1");
        assert_eq!(tabs[1].url, "https://c.example/3");
        assert_eq!(tabs[1].group, Some(1));
        assert_eq!(tabs[2].url, "https://b.example/2");
        assert_eq!(tabs[2].group, None);
    }

    #[test]
    fn a_group_that_loses_its_last_tab_disappears() {
        let mut tabs = vec![tab("https://a.example/1", Some(3))];
        let mut groups = vec![group(3, false)];
        leave_context_group(&mut tabs, &mut groups, 0);
        assert_eq!(tabs[0].group, None);
        assert!(groups.is_empty(), "pilula vazia nao pode ficar na barra");
    }

    #[test]
    fn a_new_group_takes_the_host_for_a_name_and_a_colour_nobody_is_using() {
        let mut tabs = vec![
            tab("https://www.arxiv.org/abs/1", None),
            tab("https://b.example/2", None),
        ];
        let mut groups = Vec::new();
        let mut next_id = 1;
        let first =
            create_context_group(&mut tabs, &mut groups, &mut next_id, 0).expect("aba existe");
        let second =
            create_context_group(&mut tabs, &mut groups, &mut next_id, 1).expect("aba existe");
        assert_eq!(groups[first].name, "arxiv.org");
        assert_eq!(tabs[0].group, Some(groups[first].id));
        assert_ne!(
            groups[first].color, groups[second].color,
            "dois grupos seguidos nao podem nascer da mesma cor"
        );
        assert_ne!(groups[first].id, groups[second].id);
    }

    #[test]
    fn the_group_pill_is_hit_tested_where_it_is_drawn() {
        let tabs = vec![
            tab("https://a.example/1", Some(1)),
            tab("https://b.example/2", None),
        ];
        let groups = vec![group(1, false)];
        let mut rows = [TabRow::empty(); COMPARATOR_COLUMNS];
        rows[0] = plan_tab_row(&tabs, &groups);
        let layout = BarLayout::with_rows(1600.0, 1.0, true, BarColumns::even(3), rows);

        assert_eq!(layout.group_pill_counts[0], 1);
        let pill = layout.group_pills[0][0];
        assert!(pill.width > 0.0);
        assert_eq!(
            layout.hit(pill.x + pill.width / 2.0, pill.y + pill.height / 2.0),
            Some(BarHit::ContextGroup {
                source_index: 0,
                group_index: 0
            })
        );

        // E as abas continuam a acertar nelas proprias, nao na pilula.
        assert_eq!(layout.context_tab_counts[0], 2);
        let first = layout.context_tabs[0][0];
        assert!(
            first.x >= pill.x + pill.width,
            "a pilula vem antes da sua primeira aba"
        );
        assert_eq!(
            layout.hit(first.x + first.width / 2.0, first.y + first.height / 2.0),
            Some(BarHit::ContextTab {
                source_index: 0,
                context_index: 0
            })
        );
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

/// Quanto tempo um tema lido do registo continua a valer. `Theme::system()` e
/// chamada em WM_CTLCOLOREDIT, WM_ERASEBKGND, em cada divisor pintado e a cada
/// frame da Home (15 FPS) — e cada chamada fazia DUAS leituras de registo. Com
/// 1 s de validade o registo passa a ser lido uma vez por segundo, e a mudanca
/// de tema nao fica por notar porque `ThemeChanged` invalida isto de imediato.
const THEME_CACHE_TTL: Duration = Duration::from_secs(1);
static THEME_CACHE: Mutex<Option<(Instant, Theme)>> = Mutex::new(None);

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
    /// Guardado com o resto do tema para quem desenha nao voltar ao registo so
    /// para saber se esta escuro (o fundo neural fazia-o duas vezes por frame).
    dark: bool,
}

impl Theme {
    fn system() -> Self {
        let mut cache = THEME_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((stamp, theme)) = *cache
            && stamp.elapsed() < THEME_CACHE_TTL
        {
            return theme;
        }
        let theme = Self::read_system();
        *cache = Some((Instant::now(), theme));
        theme
    }

    /// A leitura verdadeira do registo; quem decide quando ela acontece e o
    /// cache acima.
    fn read_system() -> Self {
        let accent = system_accent();
        if system_dark_mode() {
            Self::dark(accent)
        } else {
            Self::light(accent)
        }
    }

    /// Obriga a proxima `system()` a reler o registo. Chamada quando o Windows
    /// avisa que o tema mudou: esperar ate 1 s daria um piscar de cores velhas.
    fn invalidate() {
        *THEME_CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
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
            dark: true,
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
            dark: false,
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

/// Poe uma imagem BGRA **pre-multiplicada** por cima do que ja esta no DC,
/// respeitando o alfa. E o unico sitio onde a NeuralIA usa a `msimg32`, e usa-a
/// porque a alternativa -- ler o fundo de volta com `GetDIBits`, compor a mao e
/// voltar a escrever -- e tres vezes o trabalho para o mesmo resultado.
unsafe fn alpha_blit(
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
            // Negativo: a imagem vem de cima para baixo, como a `image` a da.
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

    // O `AlphaBlend` precisa de um bitmap com alfa de verdade, e um
    // `CreateCompatibleBitmap` nao o tem: dai a seccao DIB.
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(
        hdc as _,
        &bmi,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    if dib.is_null() || bits.is_null() {
        if !dib.is_null() {
            DeleteObject(dib as _);
        }
        return;
    }
    std::ptr::copy_nonoverlapping(
        pixels.as_ptr(),
        bits as *mut u8,
        (width * height * 4) as usize,
    );

    let mem = CreateCompatibleDC(hdc as _);
    if mem.is_null() {
        DeleteObject(dib as _);
        return;
    }
    let old = SelectObject(mem, dib as _);
    AlphaBlend(
        hdc as _,
        x,
        y,
        width,
        height,
        mem,
        0,
        0,
        width,
        height,
        BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        },
    );
    SelectObject(mem, old);
    DeleteDC(mem);
    DeleteObject(dib as _);
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
/// Tecto do cache de icones redimensionados. Ha 4 slots, mas o tamanho vem da
/// escala da janela: arrastar a borda gera um tamanho novo por pixel percorrido
/// e o cache antigo, sem limite, guardava um bitmap por cada um deles para
/// sempre. 24 entradas chegam para os tamanhos que a barra usa de facto.
const ICON_CACHE_CAPACITY: usize = 24;
/// (slot, lado em pixeis) -> bitmap ja redimensionado, partilhado por `Arc`
/// para o desenho nao copiar a imagem a cada WM_PAINT.
type IconCacheEntry = ((usize, u32), Arc<RgbaImage>);
static ICON_SCALE_CACHE: Mutex<Vec<IconCacheEntry>> = Mutex::new(Vec::new());

/// LRU minimo sobre um vector: o fim e o mais recentemente usado, o inicio e o
/// candidato a sair. Estao separadas do cache de icones de proposito — assim a
/// politica de eviccao testa-se sem GDI, sem PNGs e sem estado global.
///
/// Devolve o valor se a chave existir, promovendo a entrada a mais recente.
fn lru_promote<K: PartialEq, V: Clone>(entries: &mut Vec<(K, V)>, key: &K) -> Option<V> {
    let index = entries.iter().position(|(cached, _)| cached == key)?;
    let entry = entries.remove(index);
    let value = entry.1.clone();
    entries.push(entry);
    Some(value)
}

/// Insere como mais recente, deitando fora as mais antigas ate caber em
/// `capacity`. Uma chave repetida substitui a entrada antiga em vez de crescer.
fn lru_insert<K: PartialEq, V>(entries: &mut Vec<(K, V)>, key: K, value: V, capacity: usize) {
    if capacity == 0 {
        entries.clear();
        return;
    }
    if let Some(index) = entries.iter().position(|(cached, _)| *cached == key) {
        entries.remove(index);
    }
    while entries.len() >= capacity {
        entries.remove(0);
    }
    entries.push((key, value));
}

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

fn icon_scaled(slot: usize, size: u32) -> Arc<RgbaImage> {
    let key = (slot, size);
    let mut guard = ICON_SCALE_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let entries = &mut *guard;
    if let Some(image) = lru_promote(entries, &key) {
        return image;
    }

    let source = if slot == ICON_SLOT_HOME {
        home_icon()
    } else {
        ai_icon(slot)
    };
    let scaled = Arc::new(image::imageops::resize(
        source,
        size,
        size,
        image::imageops::FilterType::Lanczos3,
    ));
    lru_insert(entries, key, Arc::clone(&scaled), ICON_CACHE_CAPACITY);
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

    // O cache entrega o `Arc`; aqui so se le, por isso basta emprestar.
    let scaled = icon_scaled(slot, size as u32);
    let image: &RgbaImage = &scaled;
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
  // WRY/WebView2 injeta initialization scripts em child frames no Windows.
  // Capability e controles nativos pertencem somente ao documento principal.
  if (window.top !== window) return;
  if (window.__neuralia_keymap) { return; }
  window.__neuralia_keymap = true;

  // Capturas no document-created, antes de a pagina correr: o que os atalhos
  // usam mais tarde com o token nao pode ser um global ja envenenado.
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  const colIndex = window.__neuralia_col_index;
  function act(action, args) {
    post(stringify({ v:1, cap:capability, action, args:args || {} }));
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
            act('newtab', { col:colIndex });
          } else {
            act('newtab');
          }
          return;
        case 'k':
        case 't':
          e.preventDefault();
          if (typeof colIndex === 'number') {
            // So o pedido de abertura: o texto vai ser escrito num controlo
            // nativo, fora do alcance da pagina.
            act('palette', { col:colIndex });
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
        act('expand', { col:(parseInt(key, 10) - 1) });
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
  if (window.top !== window) return;
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  const defer = setTimeout;
  const cancelDefer = clearTimeout;
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
      post(stringify({ v:1, cap:capability, action:'home', args:{} }));
    });
    append(document.documentElement, b);
  });
})();
"#;

const GMAIL_MONITOR_SCRIPT: &str = r#"
(function () {
  if (window.top !== window) return;
  if (location.hostname !== 'mail.google.com' || window.__neuralia_gmail_monitor) return;
  window.__neuralia_gmail_monitor = true;
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
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

    post(stringify({
      v:1,
      cap:capability,
      action:'gmail-state',
      args:{ count, sender:first.sender, subject:first.subject, key:first.key }
    }));
  }

  function schedule() {
    clearTimeout(debounce);
    debounce = setTimeout(emit, 450);
  }

  // Como os outros observers deste ficheiro: uma chamada por quadro, nao uma
  // por mutacao. A caixa de entrada muda o DOM em rajadas de centenas de
  // registos e cada um fazia clearTimeout/setTimeout.
  let raf = 0;
  const coalesce = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => { raf = 0; schedule(); });
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', schedule, { once:true });
  } else {
    schedule();
  }

  // So childList+subtree: attributes e characterData disparavam a cada
  // realce de linha e a cada relogio que o Gmail redesenha. O que escapar
  // apanha-se no emit() periodico.
  new MutationObserver(coalesce).observe(document.documentElement, {
    childList:true, subtree:true
  });
  setInterval(emit, 15000);
})();
"#;

/// ChatGPT e Claude aceitam a consulta por ?q=, mas hoje apenas preenchem o
/// compositor. O comparador tem semântica de "perguntar às três", portanto o
/// NeuralIA confirma o envio assim que o botão real do fornecedor fica pronto.
/// Envia a pergunta no fornecedor em vez de a deixar na caixa.
///
/// O Gemini abre directamente numa pagina de resultados; o ChatGPT e o Claude
/// recebem `?q=` que so PREENCHE a caixa. A versao anterior esperava que
/// `promptText()` devolvesse texto antes de carregar em enviar -- mas lia o
/// PRIMEIRO `textarea` da pagina, que nestes sitios e um campo escondido e
/// vazio. Ficava a tentar 120 vezes e desistia, e a pergunta ficava na barra a
/// espera de um Enter manual: exactamente o que o utilizador via.
///
/// Agora procura o editor que TEM texto, e se nenhum tiver escreve a pergunta
/// ele proprio antes de enviar.
const AI_AUTO_SUBMIT_SCRIPT: &str = r#"
(function () {
  // So no frame de topo. Este script ESCREVE numa caixa de texto, e o WebView2
  // injeta os scripts de inicializacao tambem nos frames filhos: sem esta
  // guarda, um iframe da mesma origem levava com a pergunta escrita dentro.
  if (window.top !== window) return;
  const host = location.hostname.toLowerCase();
  if (host !== 'chatgpt.com' && host !== 'claude.ai') return;
  const query = new URL(location.href).searchParams.get('q');
  if (!query || !query.trim()) return;

  // O acesso ao sessionStorage pode LANCAR -- armazenamento particionado,
  // cookies de terceiros bloqueados, modo restrito. Sem rede, um throw aqui
  // ao nivel de topo abortava o script todo.
  function stampRead(key) {
    try { return Number(sessionStorage.getItem(key) || '0'); } catch (_) { return 0; }
  }
  function stampWrite(key, value) {
    try { sessionStorage.setItem(key, String(value)); } catch (_) {}
  }

  const stampKey = 'neuralia:auto-submit:' + host + ':' + query;
  if (Date.now() - stampRead(stampKey) < 10000) return;

  const EDITORS = 'div[contenteditable="true"][role="textbox"], div[contenteditable="true"], [data-testid="prompt-textarea"], textarea';

  function textOf(el) {
    if (!el) return '';
    return String('value' in el && typeof el.value === 'string' ? el.value : el.innerText || el.textContent || '').trim();
  }

  function visible(el) {
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  // O editor certo e o que ESTA VISIVEL e, de preferencia, o que ja tem texto.
  // Ler so o primeiro `textarea` apanhava um campo escondido e vazio.
  function editor() {
    const all = Array.from(document.querySelectorAll(EDITORS)).filter(visible);
    return all.find((el) => textOf(el)) || all[0] || null;
  }

  function fill(el) {
    el.focus();
    if (el.isContentEditable) {
      // `execCommand` e o que os editores com React por tras aceitam sem
      // reescrever o estado deles por baixo.
      if (!document.execCommand('insertText', false, query)) {
        el.textContent = query;
        el.dispatchEvent(new InputEvent('input', { bubbles:true, data:query, inputType:'insertText' }));
      }
      return;
    }
    const descriptor = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
    if (descriptor && descriptor.set) descriptor.set.call(el, query);
    else el.value = query;
    el.dispatchEvent(new Event('input', { bubbles:true }));
  }

  // Apagar o que NOS escrevemos. So se usa quando desistimos: texto que o
  // utilizador nao escreveu nao pode ficar na caixa de outra pessoa.
  function clear(el) {
    if (!el) return;
    el.focus();
    if (el.isContentEditable) {
      const range = document.createRange();
      range.selectNodeContents(el);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      if (!document.execCommand('insertText', false, '')) {
        el.textContent = '';
        el.dispatchEvent(new InputEvent('input', { bubbles:true, inputType:'deleteContentBackward' }));
      }
      return;
    }
    const descriptor = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
    if (descriptor && descriptor.set) descriptor.set.call(el, '');
    else el.value = '';
    el.dispatchEvent(new Event('input', { bubbles:true }));
  }

  function sendButton() {
    const selectors = host === 'chatgpt.com'
      ? ['button[data-testid="send-button"]', 'button[aria-label*="Send prompt"]', 'button[aria-label*="Send message"]', 'button[aria-label*="Enviar"]', 'form button[type="submit"]']
      : ['button[aria-label*="Send"]', 'button[aria-label*="Enviar"]', 'button[data-testid*="send"]', 'form button[type="submit"]'];
    for (const selector of selectors) {
      const button = document.querySelector(selector);
      if (!button || button.disabled || button.getAttribute('aria-disabled') === 'true') continue;
      if (!visible(button)) continue;
      return button;
    }
    return null;
  }

  // O sitio ja tratou da pergunta sozinho?
  //
  // O ChatGPT, com `?q=...&hints=search`, NAO se limita a preencher a caixa:
  // envia a pergunta e troca a URL para `/uc/<id>` sem recarregar a pagina.
  // A caixa fica entao vazia -- e o script, que guardou a pergunta no
  // arranque, via-a vazia e escrevia-a de volta. Era isso que ficava escrito
  // no ChatGPT depois de a resposta ja estar na tela.
  //
  // O sinal e a propria URL e nao o DOM: o `?q=` desaparece quando o site o
  // consome, em qualquer provedor e em qualquer versao do HTML deles. Ler o
  // DOM obrigava a conhecer os seletores de cada um -- e o ChatGPT tem pelo
  // menos duas variantes (ligado e desligado) com marcadores diferentes.
  function consumed() {
    try {
      return new URL(location.href).searchParams.get('q') !== query;
    } catch (_) {
      return true;
    }
  }

  // Reaviva o estado do framework quando o proprio ?q= desenhou texto no
  // editor mas ainda nao habilitou o botao de envio.
  function nudge(el) {
    if (!el) return;
    try {
      el.dispatchEvent(new InputEvent('input', { bubbles:true, inputType:'insertText' }));
    } catch (_) {
      el.dispatchEvent(new Event('input', { bubbles:true }));
    }
    el.dispatchEvent(new Event('change', { bubbles:true }));
  }

  // Primeiro tenta o botao real. Se o fornecedor escondeu o botao mas o
  // editor pertence a um form, requestSubmit() percorre o caminho nativo do
  // formulario. O KeyboardEvent sintetico fica apenas como ultimo recurso:
  // Chromium marca-o isTrusted=false e os fornecedores podem ignora-lo.
  function submitEditor(el) {
    const button = sendButton();
    if (button) {
      button.click();
      return 'button';
    }

    const form = el && typeof el.closest === 'function' ? el.closest('form') : null;
    if (form && typeof form.requestSubmit === 'function') {
      try {
        const submitter = form.querySelector(
          'button[type="submit"]:not([disabled]), input[type="submit"]:not([disabled])'
        );
        if (submitter) form.requestSubmit(submitter);
        else form.requestSubmit();
        return 'form';
      } catch (_) {}
    }

    el.focus();
    for (const type of ['keydown', 'keypress', 'keyup']) {
      el.dispatchEvent(new KeyboardEvent(type, {
        key:'Enter', code:'Enter', keyCode:13, which:13,
        bubbles:true, cancelable:true
      }));
    }
    return 'keyboard';
  }

  let attempts = 0;
  let filled = false;
  let nudged = false;
  let lastSubmitAt = 0;

  function submitWhenReady() {
    attempts += 1;

    if (consumed()) {
      stampWrite(stampKey, Date.now());
      return;
    }

    const el = editor();

    // Depois de uma tentativa, o compositor vazio e o melhor reconhecimento
    // transversal de que o site aceitou a pergunta. Nao ha novo clique.
    if (lastSubmitAt && el && !textOf(el)) {
      stampWrite(stampKey, Date.now());
      return;
    }

    if (el) {
      // Dez tentativas (~1,5 s) para o proprio site preencher o compositor.
      // Depois disso escrevemos nos, caso ele ainda esteja vazio.
      if (!textOf(el) && !filled && attempts > 10) {
        fill(el);
        filled = true;
      }

      if (textOf(el)) {
        // Um ?q= pode pintar a string sem acordar o estado React. Reemitir
        // input/change uma vez deixa o botao real nascer/habilitar.
        if (!nudged && attempts > 8) {
          nudge(el);
          nudged = true;
        }

        const now = Date.now();
        // Nao martelar o endpoint enquanto uma submissao anterior ainda pode
        // estar em voo. Se um Enter sintetico for ignorado, continuamos a
        // observar e tentamos o botao/formulario assim que aparecer.
        if (attempts > 10 && now - lastSubmitAt >= 2500) {
          lastSubmitAt = now;
          submitEditor(el);
        }
      }
    }

    if (attempts < 160) {
      setTimeout(submitWhenReady, 150);
      return;
    }

    // Desistimos ao fim de ~24 s. Se fomos NOS a escrever e nunca chegou a ser
    // enviado, a pergunta nao pode ficar la a fingir que o utilizador a
    // escreveu.
    if (filled) clear(el || editor());
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => setTimeout(submitWhenReady, 100), { once:true });
  } else {
    setTimeout(submitWhenReady, 100);
  }
})();
"#;

const AGENT_OBSERVER_SCRIPT: &str = r#"
(function () {
  if (window.top !== window) return;
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const defer = setTimeout;
  let generation = 0;
  let lastMaterial = '';
  let timer = 0;

  function clean(value, limit) {
    return String(value || '').replace(/[\t\r\n]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, limit);
  }

  function fieldRole(el) {
    const tag = (el.tagName || '').toLowerCase();
    const type = (el.type || '').toLowerCase();
    const autocomplete = (el.autocomplete || '').toLowerCase();
    const role = (el.getAttribute('role') || '').toLowerCase();
    const name = (el.name || '').toLowerCase();
    const combined = [type, autocomplete, role, name].join(' ');
    if (combined.includes('password')) return 'password';
    if (combined.includes('one-time') || combined.includes('otp')) return 'otp';
    if (combined.includes('cc-') || combined.includes('card') || combined.includes('payment')) return 'payment-card';
    if (combined.includes('email')) return 'email';
    if (combined.includes('search')) return 'search';
    if (tag === 'select') return 'select';
    if (tag === 'input' || tag === 'textarea') return 'textbox';
    return role || tag || 'element';
  }

  function observe() {
    timer = 0;
    const root = document.querySelector('main,[role="main"]') || document.body || document.documentElement;
    const pageText = clean(root ? (root.innerText || root.textContent) : '', 1600);
    const candidates = document.querySelectorAll(
      'input,textarea,select,button,a[href],[role="button"],[role="textbox"],[role="combobox"]'
    );
    const rows = [];
    let ordinal = 0;
    for (const el of candidates) {
      if (rows.length >= 32) break;
      const rect = el.getBoundingClientRect();
      const css = getComputedStyle(el);
      if (rect.width <= 0 || rect.height <= 0 || css.display === 'none' || css.visibility === 'hidden') continue;
      const id = 'n' + (generation + 1) + '-' + ordinal++;
      el.setAttribute('data-neuralia-agent-id', id);
      const name = clean(el.getAttribute('aria-label') || el.name || el.innerText || el.textContent || el.placeholder, 96);
      rows.push([id, fieldRole(el), name, clean(el.tagName, 20), el.disabled ? '0' : '1'].join('\t'));
    }

    const material = [location.href, document.title || '', pageText, rows.join('\n')].join('\n');
    if (material === lastMaterial) return;
    lastMaterial = material;
    generation += 1;
    // IDs carry the generation used by the native stale-element guard.
    rows.forEach((row, index) => {
      const oldId = row.split('\t', 1)[0];
      const newId = 'n' + generation + '-' + index;
      const el = document.querySelector('[data-neuralia-agent-id="' + oldId + '"]');
      if (el) el.setAttribute('data-neuralia-agent-id', newId);
      rows[index] = row.replace(oldId, newId);
    });

    // O envelope nativo aceita no maximo 8 KiB. 1200 unidades UTF-16
    // continuam abaixo desse teto mesmo no pior caso JSON (surrogates
    // escapados como \\uXXXX), deixando margem para cap/action/args.
    const payload = [
      String(generation),
      clean(location.href, 1200),
      clean(document.title, 256),
      pageText,
      ...rows
    ].join('\n').slice(0, 1200);
    post(stringify({
      v:1,
      cap:capability,
      action:'agent-observation',
      args:{ data:payload }
    }));
  }

  function schedule() {
    clearTimeout(timer);
    timer = defer(observe, 700);
  }

  if (document.readyState === 'loading') {
    listen(document, 'DOMContentLoaded', schedule, { once:true });
  } else {
    schedule();
  }
  listen(window, 'neuralia-agent-rescan', schedule);
  new MutationObserver(schedule).observe(document.documentElement, {
    childList:true, subtree:true, attributes:true
  });
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
  if (window.top !== window) return;
  const colIndex = window.__neuralia_col_index ?? 0;
  const colName = window.__neuralia_col_name ?? 'IA';
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  const defer = setTimeout;
  const cancelDefer = clearTimeout;
  function act(action, args) {
    post(stringify({ v:1, cap:capability, action, args:args || {} }));
  }
  const byId = document.getElementById.bind(document);
  const createElement = document.createElement.bind(document);
  const assign = Object.assign;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const append = Function.prototype.call.bind(Node.prototype.appendChild);

  // Semantica do Chrome: clique abre onde se esta, Ctrl+clique (ou clique do
  // meio) abre "noutro separador" -- aqui, o painel lateral, com a aba na
  // barra de titulo.
  //
  // Os listeners ficam registados JA, fora do DOMContentLoaded. Os scripts da
  // propria pagina correm durante o parse, ou seja antes desse evento, e
  // registam os deles em captura primeiro; quem chega depois recebe os
  // eventos ja com `defaultPrevented` posto e desiste sem fazer nada.
  const GOOGLE_REDIRECT_PARAMS = ['q', 'url', 'imgurl', 'adurl'];

  function linkUrl(node) {
    // Nem toda a fonte e uma <a href>: o AI Mode do Google e as citacoes do
    // ChatGPT usam chips que trazem o endereco num atributo. Ler apenas
    // `a[href]` deixava de fora justamente as ligacoes destas paginas -- que
    // sao as unicas paginas onde isto corre.
    const anchor = node.closest('a[href], [role="link"], [data-href], [data-url]');
    if (!anchor) return null;
    const raw = anchor.getAttribute('href')
      || anchor.getAttribute('data-href')
      || anchor.getAttribute('data-url');
    if (!raw) return null;

    let target;
    try { target = new URL(raw, location.href); } catch (_) { return null; }
    if (target.protocol !== 'http:' && target.protocol !== 'https:') return null;

    // O Google embrulha as fontes num redirecionamento seu. Desembrulhar pelo
    // PARAMETRO e nao pelo caminho: /url, /imgres e /aclk sao caminhos
    // diferentes para a mesma coisa, e so o primeiro estava coberto.
    const host = target.hostname;
    if (host === 'google.com' || host.endsWith('.google.com')) {
      for (const name of GOOGLE_REDIRECT_PARAMS) {
        const actual = target.searchParams.get(name);
        if (!actual) continue;
        try {
          const unwrapped = new URL(actual, location.href);
          if (unwrapped.protocol === 'http:' || unwrapped.protocol === 'https:') {
            target = unwrapped;
            break;
          }
        } catch (_) {}
      }
    }
    return target;
  }

  function routeLink(event, aside) {
    if (!event.isTrusted || event.defaultPrevented) return;
    // Alt e Shift sao gestos do proprio navegador (descarregar, nova janela);
    // nao os roubamos.
    if (event.altKey || event.shiftKey) return;
    const node = event.target;
    if (!node || !node.closest) return;
    if (node.closest('#neuralia-comp-controls,#neuralia-palette')) return;

    const target = linkUrl(node);
    if (!target) return;

    // Clique simples numa ligacao do proprio sitio e navegacao interna da
    // aplicacao: a SPA trata disso melhor do que um load_url, que recarregava
    // a pagina toda e perdia a conversa. Com Ctrl a intencao e explicita e
    // vale para qualquer endereco, incluindo o do proprio sitio.
    if (!aside && target.origin === location.origin) return;

    event.preventDefault();
    event.stopPropagation();
    act('link', { col:colIndex, url:target.href, aside:aside });
  }

  listen(document, 'click', (event) => {
    if (event.button !== 0) return;
    routeLink(event, !!(event.ctrlKey || event.metaKey));
  }, true);

  // O botao do meio NAO dispara 'click' desde o Chrome 55 -- dispara
  // 'auxclick'. O `event.button === 1` que aqui estava dentro do 'click' era
  // codigo morto: naquele evento o botao e sempre 0.
  listen(document, 'auxclick', (event) => {
    if (event.button !== 1) return;
    routeLink(event, true);
  }, true);

  listen(document, 'dblclick', (event) => {
    if (!event.isTrusted || event.defaultPrevented) return;
    if (event.target && event.target.closest
        && event.target.closest('#neuralia-comp-controls,#neuralia-palette')) return;
    const tag = event.target && event.target.tagName
      ? event.target.tagName.toUpperCase() : '';
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (event.target && event.target.isContentEditable) return;
    act('expand', { col:colIndex });
  }, true);

  listen(document, 'DOMContentLoaded', () => {
    let researchAnswerTimer = 0;
    let lastResearchAnswer = '';

    function researchAnswerText() {
      const preferred = Array.from(document.querySelectorAll(
        '[data-message-author-role="assistant"],'
        + '[data-testid*="assistant"],[class*="assistant"],article,[role="article"]'
      ));
      let best = '';
      for (const element of preferred) {
        if (!element || (element.closest && element.closest('#neuralia-comp-controls,#neuralia-palette'))) {
          continue;
        }
        const text = (element.innerText || element.textContent || '')
          .replace(/\s+/g, ' ')
          .trim();
        if (text.length > best.length) best = text;
      }
      if (!best) {
        const main = document.querySelector('main,[role="main"]');
        best = main ? (main.innerText || main.textContent || '').replace(/\s+/g, ' ').trim() : '';
      }
      return best.slice(0, 1800);
    }

    function scheduleResearchAnswer() {
      cancelDefer(researchAnswerTimer);
      researchAnswerTimer = defer(() => {
        const text = researchAnswerText();
        if (text.length < 24 || text === lastResearchAnswer) return;
        lastResearchAnswer = text;
        act('research-answer', { col:colIndex, text });
      }, 1800);
    }

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
        act('expand', { col:colIndex });
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
        act('minimize', { col:colIndex });
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
      new MutationObserver(() => {
        scheduleSync();
        scheduleResearchAnswer();
      }).observe(document.documentElement, {
        childList:true, subtree:true
      });
      syncTicks();
      scheduleResearchAnswer();
    }

    mountControls();
    new MutationObserver(() => {
      if (!byId('neuralia-comp-controls')) mountControls();
    }).observe(document.documentElement, { childList:true, subtree:true });

  });
})();
"#;
