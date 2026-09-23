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
use crate::ipc::{IpcAction, SEARCH_MAX_CHARS, parse_ipc_message};
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
            EnableWindow, GetAsyncKeyState, GetFocus, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP,
            SendInput, SetFocus, VK_CONTROL, VK_ESCAPE, VK_NEXT, VK_RETURN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow,
            ES_AUTOHSCROLL, EnumChildWindows, GetClassNameW, GetClientRect, GetCursorPos,
            GetForegroundWindow, GetParent, GetWindowTextLengthW, GetWindowTextW,
            GetWindowThreadProcessId, IDYES, IsZoomed, MB_DEFBUTTON2, MB_ICONINFORMATION,
            MB_ICONWARNING, MB_OK, MB_YESNO, MF_SEPARATOR, MF_STRING, MessageBoxW, SW_HIDE,
            SW_SHOW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetParent,
            SetWindowPos, SetWindowTextW, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
            TrackPopupMenu, WM_CANCELMODE, WM_CAPTURECHANGED, WM_KEYDOWN, WS_CHILD,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
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
    window::{CursorIcon, Fullscreen, Icon, Window, WindowId},
};
use wry::{
    NewWindowResponse, PermissionKind, PermissionResponse, WebView, WebViewBuilder,
    http::{Request, Response as HttpResponse},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageTarget {
    Column(usize),
    Split,
}

#[derive(Debug)]
enum UserEvent {
    /// Escolha de tema feita no menu do botao Home.
    ThemeChosen(ThemeChoice),
    /// Pedido da pagina local do painel lateral (canal proprio).
    Panel(PanelMessage),
    /// "Abrir?" do aviso do Gmail: Sim (true) ou Nao.
    GmailAnswer(bool),
    HomeRequested,
    /// Voltar um nivel: de ecra completo para tres colunas, de la para a Home.
    BackRequested,
    ToggleAutoScroll,
    AutoScrollAnswer(bool),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ReloadPage,
    ReloadTarget(PageTarget),
    PrintPage,
    PrintTarget(PageTarget),
    FocusOmnibox,
    ToggleColumnFullscreen,
    OpenDevTools,
    OpenDevToolsTarget(PageTarget),
    ViewSource,
    ViewSourceTarget(PageTarget),
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
    /// Falha de persistencia do historico. Append e assíncrono, mas erro de
    /// disco/permissão não pode desaparecer só no stderr.
    HistoryWriteFailed(String),
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
    /// Pergunta enviada na caixa de uma coluna: vai tambem as outras.
    AskEverywhere {
        source_index: usize,
        text: String,
    },
    /// "Pesquisar" da barra de selecao: o texto selecionado e uma pergunta
    /// para as tres IAs (`App::compare`), nunca um comando da omnibox.
    SearchSelection(String),
    AgentObservation(ObservedPage),
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

/// O que fazer com um pedido de split conforme a superficie atual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitFallback {
    OpenSplit,
    OpenWeb,
    Ignore,
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
/// A moldura Win32 pode mudar o client rect um ciclo depois de
/// set_decorations(false). Fazemos dois relayouts baratos para nao deixar
/// WebViews presos na geometria anterior ate o primeiro movimento do rato.
const COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS: [u64; 2] = [40, 220];
/// Quanto tempo o aviso de correio novo fica no canto.
/// Com a pergunta "Abrir?" o aviso fica mais tempo a vista.
const GMAIL_TOAST_SECONDS: u64 = 12;
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

/// O que esta debaixo do rato no chrome nativo do comparador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarHit {
    Home,
    /// ‹ e › da fonte aberta ao lado de uma coluna, junto do rotulo dela.
    Back,
    Forward,
    /// ‹ e › de cada IA, logo depois do "+" da coluna.
    ColumnBack(usize),
    ColumnForward(usize),
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
    /// Icones do canto direito: servicos no painel e avisos do Gmail.
    Service(Service),
    GmailToggle,
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
    /// Largura logica do painel lateral a direita; as colunas ficam antes dele.
    panel_width: f64,
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
            panel_width: 0.0,
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
    back: UiRect,
    forward: UiRect,
    column_back: [UiRect; COMPARATOR_COLUMNS],
    column_forward: [UiRect; COMPARATOR_COLUMNS],
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
                back: empty,
                forward: empty,
                column_back: [empty; COMPARATOR_COLUMNS],
                column_forward: [empty; COMPARATOR_COLUMNS],
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
        let mut column_back = [empty; COMPARATOR_COLUMNS];
        let mut column_forward = [empty; COMPARATOR_COLUMNS];
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
            (client_width / scale - columns.panel_width).max(1.0),
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
        let controls = right_controls(client_width, scale, columns.split_active);
        let controls_left = controls.leftmost();
        let (back, forward) = controls.split_nav.unwrap_or((empty, empty));
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
            // "+", ‹ e › depois da pilula: ela encolhe primeiro.
            let nav_width = plus_width;
            let nav_gap = 4.0 * scale;
            let reserved_after = plus_width + gap + 2.0 * (nav_width + nav_gap);
            let pill = provider_width.min((available - reserved_after).max(0.0));
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
            // ‹ e › desta IA. Tal como o "+", ou cabem na faixa ou nao existem.
            let back_x = plus_x + plus_width + nav_gap;
            let forward_x = back_x + nav_width + nav_gap;
            let fits = forward_x + nav_width <= right;
            column_back[span.index] = UiRect {
                x: back_x,
                y: row_y + 2.0 * scale,
                width: if fits { nav_width } else { 0.0 },
                height: row_h - 4.0 * scale,
            };
            column_forward[span.index] = UiRect {
                x: forward_x,
                width: if fits { nav_width } else { 0.0 },
                ..column_back[span.index]
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
            back,
            forward,
            column_back,
            column_forward,
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
        if self.back.contains(x, y) {
            return Some(BarHit::Back);
        }
        if self.forward.contains(x, y) {
            return Some(BarHit::Forward);
        }
        for index in 0..self.columns_len {
            if self.column_back[index].contains(x, y) {
                return Some(BarHit::ColumnBack(index));
            }
            if self.column_forward[index].contains(x, y) {
                return Some(BarHit::ColumnForward(index));
            }
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

fn surface_accepts_omnibox_submit(surface: Surface) -> bool {
    matches!(surface, Surface::Home)
}

/// Mantem o HWND da omnibox vivo entre trocas de decoracao, mas remove a sua
/// autoridade de teclado fora da Home. Esta e a unica funcao que decide a
/// interatividade do EDIT nativo; producao e gate exercitam o mesmo caminho.
unsafe fn apply_omnibox_interactivity(edit: HWND, surface: Surface) {
    let interactive = surface_accepts_omnibox_submit(surface);
    EnableWindow(edit, if interactive { 1 } else { 0 });
    if !interactive && GetFocus() == edit {
        let parent = GetParent(edit);
        if !parent.is_null() {
            SetFocus(parent);
        }
    }
}

/// Os controlos do canto direito da segunda linha.
#[derive(Debug, Clone, Copy)]
struct RightControls {
    private: UiRect,
    /// Videochamada, WhatsApp, YouTube e Gmail, a esquerda do Privado.
    services: [UiRect; 4],
    /// Rotulo, expandir e fechar da gaveta; `None` quando nao ha gaveta.
    split: Option<(UiRect, UiRect, UiRect)>,
    /// ‹ e › da fonte da gaveta, a esquerda do rotulo.
    split_nav: Option<(UiRect, UiRect)>,
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

    let split_nav = split.map(|(label, _, _)| {
        let size = row_h - 4.0 * scale;
        let forward = UiRect {
            x: label.x - 6.0 * scale - size,
            y: row_y + 2.0 * scale,
            width: size,
            height: size,
        };
        let back = UiRect {
            x: forward.x - 4.0 * scale - size,
            ..forward
        };
        (back, forward)
    });
    let right = match split_nav {
        Some((back, _)) => back.x - 6.0 * scale,
        None => client_width - margin,
    };
    // Botoes redondos so com icone, como no Chrome: Privado a direita e, a
    // esquerda dele, videochamada, WhatsApp, YouTube e Gmail.
    let icon = row_h;
    let icon_gap = 4.0 * scale;
    let private = UiRect {
        x: right - icon,
        y: row_y,
        width: icon,
        height: icon,
    };
    let services = std::array::from_fn(|index| UiRect {
        x: private.x - (4 - index) as f64 * (icon + icon_gap),
        y: row_y,
        width: icon,
        height: icon,
    });

    RightControls {
        private,
        services,
        split,
        split_nav,
    }
}

impl RightControls {
    /// Onde comecam os controlos da direita: o resto da barra acaba aqui.
    fn leftmost(&self) -> f64 {
        self.services[0].x
    }
}

/// O que cada icone do canto direito faz, na ordem de `RightControls::services`.
const SERVICE_BUTTON_HITS: [BarHit; 4] = [
    BarHit::Service(Service::Meet),
    BarHit::Service(Service::WhatsApp),
    BarHit::Service(Service::YouTube),
    BarHit::GmailToggle,
];

fn right_controls_hit(controls: RightControls, x: f64, y: f64) -> Option<BarHit> {
    for (rect, hit) in controls.services.iter().zip(SERVICE_BUTTON_HITS) {
        if rect.contains(x, y) {
            return Some(hit);
        }
    }
    if controls.private.contains(x, y) {
        return Some(BarHit::Private);
    }
    if let Some((_label, expand, close)) = controls.split {
        if close.contains(x, y) {
            return Some(BarHit::SplitClose);
        }
        if expand.contains(x, y) {
            return Some(BarHit::SplitExpand);
        }
    }
    None
}

struct ComparatorView {
    webview: WebView,
    name: &'static str,
}

struct SplitView {
    webview: WebView,
    source_index: usize,
    /// Identidade da aba que originou este Split. URL nao e identidade:
    /// a mesma fonte pode existir em dois grupos diferentes.
    context_id: Option<u64>,
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
    /// Identidade estavel. A URL pode repetir em grupos diferentes e por isso
    /// nunca serve para decidir qual aba esta aberta ou deve ser fechada.
    id: u64,
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
    let window_start = start;
    while start < full.len() {
        match full[start] {
            TabSlot::Group(_) => break,
            TabSlot::Tab(index) if group_of(index).is_none() => break,
            TabSlot::Tab(_) => start += 1,
        }
    }

    let mut row = TabRow::empty();
    if start == full.len() && window_start < full.len() {
        // O corte caiu dentro de um grupo com mais abas abertas do que cabem e
        // nao ha nada inteiro depois dele: a linha ficava vazia e a pilula --
        // o unico caminho para fechar ou reabrir o grupo -- sumia. Mostra-se a
        // pilula antes das abas recentes do grupo, e as pilulas dos grupos
        // fechados anteriores enquanto houver lugar.
        let mut kept: Vec<TabSlot> = Vec::new();
        for slot in full[window_start..].iter().copied() {
            if let TabSlot::Tab(index) = slot
                && let Some(group) = group_of(index)
                && !kept.contains(&TabSlot::Group(group))
            {
                kept.push(TabSlot::Group(group));
            }
            kept.push(slot);
        }
        let mut lead: Vec<TabSlot> = Vec::new();
        for slot in full[..window_start].iter().rev().copied() {
            if kept.len() + lead.len() >= MAX_VISIBLE_TAB_SLOTS {
                break;
            }
            if let TabSlot::Group(group) = slot
                && groups[group].collapsed
            {
                lead.push(slot);
            }
        }
        for slot in lead.into_iter().rev().chain(kept) {
            row.push(slot);
        }
        return row;
    }
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

/// Guarda uma nova aba de contexto e devolve a sua identidade estavel. Se a
/// ultima aba ja e a mesma URL, reutiliza-a; ao aplicar o limite, poda tambem
/// o grupo que eventualmente ficou sem o seu ultimo membro.
fn remember_context_tab(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    next_id: &mut u64,
    url: String,
) -> u64 {
    if let Some(last) = tabs.last()
        && last.url == url
    {
        return last.id;
    }

    let id = *next_id;
    *next_id = (*next_id).wrapping_add(1).max(1);
    tabs.push(ContextTab {
        id,
        url,
        group: None,
    });
    if tabs.len() > 32 {
        tabs.remove(0);
        prune_empty_groups(tabs, groups);
    }
    id
}

/// Decide se uma coluna ainda pode ser minimizada sem esconder todas as IAs.
/// Esta decisao acontece ANTES de fechar um Split ativo: um clique rejeitado
/// nao pode destruir estado que o utilizador tinha aberto.
fn can_minimize_column(
    minimized: &[bool; COMPARATOR_COLUMNS],
    columns: usize,
    index: usize,
) -> bool {
    index < columns
        && !minimized[index]
        && (0..columns.min(COMPARATOR_COLUMNS))
            .filter(|slot| !minimized[*slot])
            .count()
            > 1
}

/// Fecha as outras abas do MESMO escopo da aba selecionada. Um grupo real
/// usa o seu id; abas soltas partilham o escopo `None`. Abas de outros grupos
/// nunca sao tocadas.
fn active_context_removed_by_scope(
    tabs: &[ContextTab],
    context_index: usize,
    active_id: Option<u64>,
    keep_selected: bool,
) -> bool {
    let Some(selected) = tabs.get(context_index) else {
        return false;
    };
    let Some(active_id) = active_id else {
        return false;
    };
    tabs.iter().any(|tab| {
        tab.id == active_id
            && tab.group == selected.group
            && (!keep_selected || tab.id != selected.id)
    })
}

fn close_other_context_tabs_in_scope(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    context_index: usize,
) -> bool {
    let Some(scope) = tabs.get(context_index).map(|tab| tab.group) else {
        return false;
    };
    let mut index = 0usize;
    tabs.retain(|tab| {
        let keep = index == context_index || tab.group != scope;
        index += 1;
        keep
    });
    prune_empty_groups(tabs, groups);
    true
}

/// Fecha todas as abas do escopo selecionado e preserva integralmente os
/// demais grupos da coluna.
fn close_context_tab_scope(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    context_index: usize,
) -> bool {
    let Some(scope) = tabs.get(context_index).map(|tab| tab.group) else {
        return false;
    };
    let before = tabs.len();
    tabs.retain(|tab| tab.group != scope);
    prune_empty_groups(tabs, groups);
    tabs.len() != before
}

/// Reagrupar uma aba pode esvaziar o grupo anterior. A criacao e a poda
/// pertencem a uma unica operacao para nunca deixar pilulas fantasmas.
fn regroup_context_tab(
    tabs: &mut [ContextTab],
    groups: &mut Vec<ContextGroup>,
    next_id: &mut u64,
    context_index: usize,
) -> Option<usize> {
    let created = create_context_group(tabs, groups, next_id, context_index)?;
    let created_id = groups.get(created)?.id;
    prune_empty_groups(tabs, groups);
    groups.iter().position(|group| group.id == created_id)
}

fn split_build_is_current(
    start_generation: u64,
    current_generation: u64,
    surface: Surface,
) -> bool {
    start_generation == current_generation && surface == Surface::Comparator
}

fn commit_split_build<T, E, P>(
    result: Result<T, E>,
    current_split: &mut Option<P>,
    expanded: &mut Option<usize>,
) -> Result<(T, Option<P>), E> {
    match result {
        Ok(value) => {
            *expanded = None;
            Ok((value, current_split.take()))
        }
        Err(error) => Err(error),
    }
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
    /// Contador das identidades de abas. Nunca reutiliza durante a sessao.
    next_context_id: u64,
    /// Largura logica ocupada a direita por um painel lateral aberto (0 sem
    /// painel): as colunas repartem so o que sobra, como no Chrome.
    panel_width: f64,
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
            panel_width: comp.panel_width,
            ..BarColumns::even(comp.views.len())
        };
    }
    BarColumns {
        count: comp.views.len(),
        weights: comp.weights,
        minimized: comp.minimized,
        split_active: false,
        panel_width: comp.panel_width,
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
    let available = (span.width - 48.0).max(1.0);
    let width = if available < PALETTE_MIN_WIDTH {
        available
    } else {
        available.min(PALETTE_MAX_WIDTH)
    };
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
static LIFECYCLE_PROBE_HOME_READY_MESSAGE: OnceLock<u32> = OnceLock::new();
static LIFECYCLE_COMPARATOR_READY: AtomicBool = AtomicBool::new(false);
static LIFECYCLE_HOME_READY: AtomicBool = AtomicBool::new(false);
/// O probe transmite comandos a todas as janelas do processo porque o HWND
/// principal pode mudar com decorations. O nonce impede que o mesmo comando,
/// recebido por um HWND antigo e pelo atual, gere eventos duplicados.
static LIFECYCLE_LAST_HOME_NONCE: AtomicUsize = AtomicUsize::new(0);
static LIFECYCLE_LAST_REOPEN_NONCE: AtomicUsize = AtomicUsize::new(0);

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

fn lifecycle_probe_home_ready_message() -> u32 {
    *LIFECYCLE_PROBE_HOME_READY_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.HomeReady"))
    })
}
const EXIT_BUTTON_SUBCLASS_ID: usize = 0x4E4B;
const HOME_BUTTON_SUBCLASS_ID: usize = 0x4E4C;
const CAPTION_BUTTONS_SUBCLASS_ID: usize = 0x4E70;
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
/// Chega depois de `track_mouse_leave`: o rato saiu de um botao nativo.
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_RBUTTONUP: u32 = 0x0205;

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

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "SystemFunction036"]
    fn rtl_gen_random(buffer: *mut core::ffi::c_void, length: u32) -> u8;
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
    let lifecycle_home_ready = lifecycle_probe_home_ready_message();
    if message == lifecycle_home_ready {
        return if lifecycle_probe_enabled() && LIFECYCLE_HOME_READY.load(Ordering::Acquire) {
            1
        } else {
            0
        };
    }
    if message == lifecycle_ready {
        return if lifecycle_probe_enabled() && LIFECYCLE_COMPARATOR_READY.load(Ordering::Acquire) {
            1
        } else {
            0
        };
    }
    if message == lifecycle_home || message == lifecycle_reopen {
        if lifecycle_probe_enabled() && reference_data != 0 {
            let nonce = wparam;
            let seen = if message == lifecycle_home {
                &LIFECYCLE_LAST_HOME_NONCE
            } else {
                &LIFECYCLE_LAST_REOPEN_NONCE
            };

            // O script faz broadcast process-wide por desenho: decorations pode
            // deixar mais de um HWND transitório vivo. Um único comando lógico
            // não pode virar dois HomeRequested/SubmitText. Sem este filtro,
            // um Reopen atrasado podia chegar depois da Home do ciclo seguinte
            // e recriar exactamente as três superfícies que o gate acabara de
            // derrubar.
            if nonce == 0 || seen.swap(nonce, Ordering::AcqRel) != nonce {
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
    reference_data: usize,
) -> LRESULT {
    // A pergunta recebe cliques: um STATIC devolve HTTRANSPARENT.
    if message == WM_NCHITTEST {
        return HTCLIENT as LRESULT;
    }
    if message == WM_LBUTTONUP && reference_data != 0 {
        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client) != 0 {
            let scale = ((client.bottom - client.top) as f64 / GMAIL_TOAST_HEIGHT).max(1.0);
            let (open, no) = gmail_toast_buttons(&client, scale);
            let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
            let inside =
                |rect: &RECT| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
            let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
            if inside(&open) {
                let _ = proxy.send_event(UserEvent::GmailAnswer(true));
            } else if inside(&no) {
                let _ = proxy.send_event(UserEvent::GmailAnswer(false));
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
                let background = CreateSolidBrush(rgb3(theme.surface));
                FillRect(hdc, &client, background);
                DeleteObject(background as _);

                let scale = ((client.bottom - client.top) as f64 / GMAIL_TOAST_HEIGHT).max(1.0);
                let title_font = create_font((-13.0 * scale) as i32, FW_BOLD as i32);
                let body_font = create_font((-12.0 * scale) as i32, FW_NORMAL as i32);
                let (open_button, no_button) = gmail_toast_buttons(&client, scale);
                let buttons_left = open_button.left - (8.0 * scale) as i32;
                let old_font = SelectObject(hdc, title_font as _);
                SetBkMode(hdc, TRANSPARENT as i32);

                SetTextColor(hdc, rgb3(theme.accent));
                let mut title = RECT {
                    left: (16.0 * scale) as i32,
                    top: (7.0 * scale) as i32,
                    right: buttons_left,
                    bottom: (28.0 * scale) as i32,
                };
                draw_text(
                    hdc,
                    "Gmail · novo e-mail — abrir?",
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
                    right: buttons_left,
                    bottom: client.bottom - (7.0 * scale) as i32,
                };
                draw_text(
                    hdc,
                    &text,
                    &mut body,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );

                for (rect, label, primary) in
                    [(open_button, "Abrir", true), (no_button, "Não", false)]
                {
                    let pill = UiRect {
                        x: rect.left as f64,
                        y: rect.top as f64,
                        width: (rect.right - rect.left) as f64,
                        height: (rect.bottom - rect.top) as f64,
                    };
                    let style = if primary {
                        PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
                    } else {
                        PillStyle::new(theme.surface_line, theme.surface_line, theme.fg)
                    };
                    draw_pill(hdc, pill, label, style, scale, body_font, theme.surface);
                }

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

// Popups auxiliares owned pela janela principal -- divisores do comparador,
// botao de saida, splash e aviso do Gmail. Nascem invisiveis: WS_VISIBLE no
// CreateWindowExW mostra-os com SW_SHOW, que os ativa.
const AUX_POPUP_EX_STYLE: u32 = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
const AUX_POPUP_STYLE: u32 = WS_POPUP;

/// Mostra um popup auxiliar SEM lhe dar a ativacao. `SW_SHOW` ativa a janela
/// mesmo com `WS_EX_NOACTIVATE` (a flag so trava a ativacao pelo clique), e
/// `on_focus_changed` volta a mostrar divisores e botao de saida sempre que a
/// janela ganha foco. Resultado na 2.1.5: cada clique numa pagina devolvia a
/// ativacao a um divisor, que depois ficava escondido e ATIVO -- a pagina
/// nunca tinha foco: links sem efeito, nenhum cursor de texto, teclado no
/// vazio. So a roda, que vai para a janela debaixo do cursor, funcionava.
/// Canto do splash (pergunta da rolagem automatica e afins) relativo ao
/// cliente: centrado nos dois eixos. Ficava a 48 px do fundo, e ao arrancar
/// lia-se como um rodape perdido por baixo das colunas. Nunca sai pelo topo
/// nem pela esquerda numa janela mais pequena do que ele.
fn splash_origin(client_w: i32, client_h: i32, width: i32, height: i32) -> (i32, i32) {
    (
        ((client_w - width) / 2).max(0),
        ((client_h - height) / 2).max(0),
    )
}

// Painel lateral (Ctrl+H): historico inteligente -- busca semantica, sugestoes
// de sites e os recentes. E uma WebView LOCAL com canal IPC proprio: so esta
// WebView fala por `parse_panel_message`, e ela so carrega o HTML abaixo
// (`panel_allows_navigation`). Nenhuma pagina da internet alcanca este canal,
// e os dados entram na pagina como texto (`textContent`), nunca como HTML.

/// O que a pagina do painel pode pedir. Lista fechada.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PanelMessage {
    Ready,
    Search(String),
    Open(String),
    Close,
}

const PANEL_MESSAGE_MAX_BYTES: usize = 4 * 1024;
const PANEL_QUERY_MAX_CHARS: usize = 500;
const PANEL_INPUT_MAX_CHARS: usize = 2048;
/// Quantos recentes e quantas sugestoes o painel mostra.
const PANEL_RECENT_LIMIT: usize = 30;
const PANEL_SUGGESTION_LIMIT: usize = 6;

fn parse_panel_message(body: &str) -> Option<PanelMessage> {
    if body.len() > PANEL_MESSAGE_MAX_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = |key: &str, max: usize| -> Option<String> {
        let text = value.get("args")?.get(key)?.as_str()?.trim();
        (!text.is_empty() && text.chars().count() <= max).then(|| text.to_string())
    };
    match value.get("action")?.as_str()? {
        "ready" => Some(PanelMessage::Ready),
        "close" => Some(PanelMessage::Close),
        "search" => text("query", PANEL_QUERY_MAX_CHARS).map(PanelMessage::Search),
        "open" => text("input", PANEL_INPUT_MAX_CHARS).map(PanelMessage::Open),
        _ => None,
    }
}

/// So o proprio HTML local (NavigateToString chega como about:blank ou
/// data:). Um link, um redirect, um file: ou um javascript: nao passam.
fn panel_allows_navigation(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    lower == "about:blank" || lower.starts_with("data:text/html")
}

/// Encostado a direita, abaixo da barra do comparador (ou do topo, fora
/// dele): 34% da largura, entre 320 e 440 px logicos, nunca mais que a janela.
fn side_panel_bounds(logical_w: f64, logical_h: f64, top: f64) -> (f64, f64, f64, f64) {
    let width = (logical_w * 0.34)
        .clamp(320.0, 440.0)
        .min(logical_w.max(0.0));
    let top = top.clamp(0.0, logical_h.max(0.0));
    (logical_w - width, top, width, logical_h - top)
}

/// Um item do painel: o que se le e o que o clique volta a abrir.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelItem {
    title: String,
    detail: String,
    input: String,
}

fn history_panel_items(entries: &[HistoryEntry]) -> Vec<PanelItem> {
    entries
        .iter()
        .filter(|entry| !entry.input.trim().is_empty())
        .map(|entry| {
            let kind = match entry.kind {
                HistoryKind::Ask => "IA",
                HistoryKind::Read => "Leitor",
                HistoryKind::Web => "Web",
            };
            let detail = if entry.target.trim().is_empty() || entry.target == entry.input {
                kind.to_string()
            } else {
                format!("{kind} · {}", entry.target)
            };
            PanelItem {
                title: entry.input.clone(),
                detail,
                input: entry.input.clone(),
            }
        })
        .collect()
}

fn memory_panel_items(hits: &[MemoryHit]) -> Vec<PanelItem> {
    hits.iter()
        .map(|hit| {
            let source = hit
                .provider
                .as_deref()
                .or(hit.url.as_deref())
                .unwrap_or("memória local");
            PanelItem {
                title: hit.title.clone(),
                detail: if hit.excerpt.trim().is_empty() {
                    source.to_string()
                } else {
                    format!("{source} · {}", hit.excerpt)
                },
                // Com endereco, o clique abre a pagina; sem, repete a busca.
                input: hit.url.clone().unwrap_or_else(|| hit.title.clone()),
            }
        })
        .collect()
}

/// Sugestoes de sites: os resultados da memoria que tem endereco web, um por
/// dominio, na ordem de relevancia.
fn suggestion_panel_items(hits: &[MemoryHit], limit: usize) -> Vec<PanelItem> {
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for hit in hits {
        let Some(url) = hit.url.as_deref() else {
            continue;
        };
        let Ok(parsed) = Url::parse(url) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let domain = host.trim_start_matches("www.").to_string();
        if !seen.insert(domain.clone()) {
            continue;
        }
        items.push(PanelItem {
            title: if hit.title.trim().is_empty() {
                domain.clone()
            } else {
                hit.title.clone()
            },
            detail: domain,
            input: url.to_string(),
        });
        if items.len() >= limit {
            break;
        }
    }
    items
}

/// O JS que preenche uma secao. Os dados vao como JSON (literal JS valido) e a
/// pagina so os usa com `textContent`.
fn panel_render_script(section: &str, title: &str, empty: &str, items: &[PanelItem]) -> String {
    let items: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "title": item.title,
                "detail": item.detail,
                "input": item.input,
            })
        })
        .collect();
    let data = serde_json::json!({
        "id": section,
        "title": title,
        "empty": empty,
        "items": items,
    });
    format!("window.__neuraliaPanel && window.__neuraliaPanel.render({data});")
}

fn css_color(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.0, color.1, color.2)
}

/// As cores do tema em vigor, como variaveis CSS do painel.
fn panel_theme_vars(theme: &Theme) -> serde_json::Value {
    serde_json::json!({
        "--bg": css_color(theme.page_bg),
        "--surface": css_color(theme.surface),
        "--fg": css_color(theme.fg),
        "--muted": css_color(theme.fg_muted),
        "--line": css_color(theme.surface_line),
        "--accent": css_color(theme.accent),
    })
}

fn panel_html(theme: &Theme) -> String {
    PANEL_HTML.replace("__THEME__", &panel_theme_vars(theme).to_string())
}

const PANEL_HTML: &str = r#"<!doctype html>
<html lang="pt-BR"><head><meta charset="utf-8"><title>Histórico inteligente</title>
<style>
*{box-sizing:border-box}
html,body{margin:0;height:100%;background:var(--bg);color:var(--fg);font:15px "Segoe UI",system-ui,sans-serif}
body{display:flex;flex-direction:column;border-left:1px solid var(--line)}
header{display:flex;align-items:center;justify-content:space-between;padding:14px 12px 8px 18px}
h1{font-size:17px;font-weight:600;margin:0}
#close{background:none;border:0;color:var(--muted);font-size:18px;cursor:pointer;border-radius:8px;width:32px;height:32px}
#close:hover{background:#e81123;color:#fff}
.search{padding:4px 16px 10px}
#q{width:100%;padding:10px 14px;border-radius:999px;border:1px solid var(--line);background:var(--surface);color:var(--fg);font:inherit;outline:none}
#q:focus{border-color:var(--accent)}
main{overflow:auto;flex:1;padding:0 8px 16px}
section[hidden]{display:none}
h2{font-size:12px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);margin:14px 10px 6px;font-weight:600}
.item{display:block;width:100%;text-align:left;background:none;border:0;color:inherit;font:inherit;padding:8px 10px;border-radius:10px;cursor:pointer}
.item:hover,.item:focus{background:var(--surface);outline:none}
.title,.detail{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.detail{font-size:12px;color:var(--muted);margin-top:2px}
.empty{color:var(--muted);font-size:13px;padding:6px 10px}
</style></head><body>
<header><h1>Histórico inteligente</h1><button id="close" title="Fechar (Esc)">✕</button></header>
<div class="search"><input id="q" placeholder="Descreva o que quer reencontrar e tecle Enter" autocomplete="off" spellcheck="false"></div>
<main>
<section id="busca" hidden><h2></h2><div></div></section>
<section id="sugestoes" hidden><h2></h2><div></div></section>
<section id="recentes" hidden><h2></h2><div></div></section>
</main>
<script>
(() => {
  if (window.top !== window) return;
  const post = (action, args) => window.ipc.postMessage(JSON.stringify({ action, args: args || {} }));
  const q = document.getElementById('q');
  document.getElementById('close').addEventListener('click', () => post('close'));
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); post('close'); }
  });
  q.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && q.value.trim()) { e.preventDefault(); post('search', { query: q.value.trim() }); }
  });
  const theme = (vars) => { for (const k of Object.keys(vars)) document.documentElement.style.setProperty(k, vars[k]); };
  window.__neuraliaPanel = {
    theme,
    render(data) {
      const section = document.getElementById(data.id);
      if (!section) return;
      section.hidden = false;
      section.querySelector('h2').textContent = data.title;
      const box = section.querySelector('div');
      box.textContent = '';
      if (!data.items.length) {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = data.empty;
        box.appendChild(empty);
        return;
      }
      for (const item of data.items) {
        const button = document.createElement('button');
        button.className = 'item';
        const title = document.createElement('span');
        title.className = 'title';
        title.textContent = item.title;
        const detail = document.createElement('span');
        detail.className = 'detail';
        detail.textContent = item.detail;
        button.append(title, detail);
        button.addEventListener('click', () => post('open', { input: item.input }));
        box.appendChild(button);
      }
    }
  };
  theme(__THEME__);
  q.focus();
  post('ready');
})();
</script></body></html>"#;

// Servicos no painel lateral (caminho A do WebRTC, aprovado pelo dono): o
// servico corre como uma pagina da internet comum -- contatos e chamadas sao
// os dele; o NeuralIA so libera camera e microfone pelo aviso do WebView2.
// Sem scripts injetados e sem o canal IPC do painel do historico.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
    /// Videochamada: o Google Meet.
    Meet,
    WhatsApp,
    YouTube,
    Gmail,
}

impl Service {
    fn url(self) -> &'static str {
        match self {
            Self::Meet => "https://meet.google.com/",
            Self::WhatsApp => "https://web.whatsapp.com/",
            Self::YouTube => "https://www.youtube.com/",
            Self::Gmail => "https://mail.google.com/mail/u/0/#inbox",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Meet => "Videochamada (Google Meet)",
            Self::WhatsApp => "WhatsApp",
            Self::YouTube => "YouTube",
            Self::Gmail => "Gmail",
        }
    }
}

/// So paginas da internet: um servico nunca abre file:, javascript: nem os
/// esquemas internos do NeuralIA.
fn service_panel_allows_navigation(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    lower == "about:blank" || lower.starts_with("https://") || lower.starts_with("http://")
}

/// Mais largo do que o do historico: o WhatsApp e o Meet precisam de espaco.
fn service_panel_bounds(logical_w: f64, logical_h: f64, top: f64) -> (f64, f64, f64, f64) {
    let width = (logical_w * 0.42)
        .clamp(400.0, 640.0)
        .min(logical_w.max(0.0));
    let top = top.clamp(0.0, logical_h.max(0.0));
    (logical_w - width, top, width, logical_h - top)
}

/// Botao redondo so com icone (servicos, Gmail, Privado). O icone branco e
/// pintado com `tint`; os coloridos (WhatsApp, YouTube) vao com `None`.
unsafe fn draw_icon_button(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    slot: usize,
    tint: Option<Rgb>,
    hovered: bool,
    scale: f64,
    theme: &Theme,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let fill = if hovered {
        theme.surface_line
    } else {
        theme.surface
    };
    fill_pill(
        hdc,
        rect,
        rect.height / 2.0,
        fill,
        Some((theme.surface_line, scale)),
        theme.bar_bg,
    );
    let size = (rect.height * 0.6).round() as i32;
    let x = (rect.x + (rect.width - size as f64) / 2.0).round() as i32;
    let y = (rect.y + (rect.height - size as f64) / 2.0).round() as i32;
    draw_icon(hdc, slot, x, y, size, fill, tint);
}

/// Avisos do Gmail ligados (o botao do envelope). Guardado em
/// `<data_dir>/gmail` como "ligado"/"desligado"; sem ficheiro, ligado.
static GMAIL_NOTIFICATIONS: AtomicBool = AtomicBool::new(true);

fn load_gmail_setting(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|text| !text.trim().eq_ignore_ascii_case("desligado"))
        .unwrap_or(true)
}

fn save_gmail_setting(path: &std::path::Path, on: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, if on { "ligado" } else { "desligado" })?;
    std::fs::rename(&temp, path)
}

/// "Abrir" e "Nao" no canto direito do aviso do Gmail, em pixeis do cliente.
fn gmail_toast_buttons(client: &RECT, scale: f64) -> (RECT, RECT) {
    let height = (26.0 * scale).round() as i32;
    let top = (client.bottom - height) / 2;
    let gap = (6.0 * scale).round() as i32;
    let right = client.right - (12.0 * scale).round() as i32;
    let no_width = (54.0 * scale).round() as i32;
    let open_width = (70.0 * scale).round() as i32;
    let no = RECT {
        left: right - no_width,
        top,
        right,
        bottom: top + height,
    };
    let open = RECT {
        left: no.left - gap - open_width,
        top,
        right: no.left - gap,
        bottom: top + height,
    };
    (open, no)
}

/// Log de depuracao em tempo de execucao, pedido pelo dono para achar bugs
/// intermitentes. Desligado por padrao; `NEURALIA_DEBUG_LOG=<ficheiro>` liga.
/// Cada linha: milissegundos desde o arranque e o evento. Nunca leva URLs,
/// texto de paginas nem nada da memoria -- so transicoes da janela.
fn debug_log(event: std::fmt::Arguments<'_>) {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let Some(path) = std::env::var_os("NEURALIA_DEBUG_LOG") else {
        return;
    };
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    append_debug_line(std::path::Path::new(&path), elapsed, event);
}

/// Acrescenta uma linha ao log; um log que nao abre nunca derruba o app.
fn append_debug_line(path: &std::path::Path, elapsed_ms: u128, event: std::fmt::Arguments<'_>) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{elapsed_ms:>8} ms  {event}");
    }
}

fn show_popup_without_activation(hwnd: HWND) {
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

// Dicas. A barra e os botoes nativos sao desenhados a mao, por isso o Windows
// nao tem texto nenhum para mostrar sozinho. A dica e uma MENSAGEM no centro da
// janela, com o visual dos avisos (fundo e texto do tema, letra grande), como o
// dono pediu: o balao do Windows junto ao cursor era pequeno -- e, em modo
// TTF_SUBCLASS, nem aparecia sobre a janela do winit. Aparece depois de o rato
// parar TOOLTIP_DELAY_MS sobre um alvo; some quando ele sai ou clica.
static HINT_HWND: AtomicUsize = AtomicUsize::new(0);
static HINT_TEXT: Mutex<String> = Mutex::new(String::new());
/// A dica que o temporizador vai mostrar: (janela raiz, texto).
static TOOLTIP_PENDING: Mutex<Option<(usize, String)>> = Mutex::new(None);
static TOOLTIP_TIMER: AtomicUsize = AtomicUsize::new(0);
/// O botao Home ja agendou a sua dica nesta passagem do rato.
static HOME_TOOLTIP_ARMED: AtomicBool = AtomicBool::new(false);
const TOOLTIP_DELAY_MS: u32 = 450;
/// Letra, margem, largura maxima e raio da dica, em pixels a 96 dpi.
const HINT_FONT_PX: f64 = 20.0;
/// Margens da dica: mais largas dos lados, onde a pilula se arredonda.
const HINT_PADDING_X_PX: f64 = 24.0;
const HINT_PADDING_Y_PX: f64 = 14.0;
const HINT_MAX_WIDTH_PX: f64 = 640.0;

/// O que o clique em cada alvo da barra FAZ -- o mesmo match que trata o
/// clique --, nao so o nome do botao.
fn bar_tooltip_label(
    hit: BarHit,
    provider: &str,
    maximized: bool,
    tab_url: Option<&str>,
    group: Option<(&str, bool)>,
) -> Option<String> {
    Some(match hit {
        BarHit::Home => "Voltar à Home".to_string(),
        BarHit::Back => "Voltar na fonte aberta ao lado".to_string(),
        BarHit::Forward => "Avançar na fonte aberta ao lado".to_string(),
        BarHit::ColumnBack(_) => format!("Voltar no {provider}"),
        BarHit::ColumnForward(_) => format!("Avançar no {provider}"),
        BarHit::Column(_) => format!("{provider}: expandir esta coluna"),
        BarHit::AddTab(_) => format!("Nova pergunta ao {provider}"),
        BarHit::ContextTab { .. } => {
            format!(
                "{}\nClique: abrir ao lado · Botão direito: fechar e grupos",
                tab_url?
            )
        }
        BarHit::ContextGroup { .. } => {
            let (name, collapsed) = group?;
            let action = if collapsed {
                "mostrar as abas"
            } else {
                "recolher"
            };
            format!("Grupo \"{name}\": clique para {action}")
        }
        BarHit::SplitExpand => "Expandir ou reduzir a fonte aberta ao lado".to_string(),
        BarHit::SplitClose => "Fechar a fonte aberta ao lado".to_string(),
        BarHit::Private => {
            "Painel privado: abre ao lado sem gravar histórico nem memória".to_string()
        }
        BarHit::Service(service) => format!("{} no painel ao lado", service.label()),
        BarHit::GmailToggle => if GMAIL_NOTIFICATIONS.load(Ordering::Acquire) {
            "Avisos do Gmail: ligados · clique para desligar"
        } else {
            "Avisos do Gmail: desligados · clique para ligar"
        }
        .to_string(),
        BarHit::WindowMinimize => caption_tooltip_label(0, maximized).to_string(),
        BarHit::WindowMaximize => caption_tooltip_label(1, maximized).to_string(),
        BarHit::WindowClose => caption_tooltip_label(2, maximized).to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryStep {
    Back,
    Forward,
}

/// Qual pagina o ‹ › move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryNav {
    /// A fonte aberta ao lado de uma coluna (onde se seguem links).
    Split,
    /// A coluna expandida.
    Column(usize),
    /// A pagina cheia (Web ou Leitor).
    Page,
    /// Sem uma pagina so: voltar e o do app.
    App,
}

fn history_nav_target(
    surface: Surface,
    split_open: bool,
    expanded: Option<usize>,
    has_page: bool,
) -> HistoryNav {
    match surface {
        Surface::Comparator if split_open => HistoryNav::Split,
        Surface::Comparator => expanded.map_or(HistoryNav::App, HistoryNav::Column),
        Surface::Home => HistoryNav::App,
        _ if has_page => HistoryNav::Page,
        _ => HistoryNav::App,
    }
}

/// Os botoes da janela do proprio app aparecem sempre que a moldura do Windows
/// nao esta la: na Home e no comparador (com a barra).
fn caption_buttons_wanted(surface: Surface, bar_visible: bool) -> bool {
    match surface {
        Surface::Home => true,
        Surface::Comparator => bar_visible,
        _ => false,
    }
}

/// A faixa de cima da Home, onde se agarra a janela sem moldura.
fn home_drag_strip(y: f64, scale: f64) -> bool {
    y >= 0.0 && y <= TITLE_TAB_HEIGHT * scale.max(1.0)
}

/// Vermelho do fechar ao passar o rato, o mesmo do Chrome e do Windows.
const CLOSE_HOVER_RED: Rgb = (232, 17, 35);

/// Botoes da janela: o fechar fica vermelho com a cruz branca debaixo do rato,
/// como no Chrome; minimizar e maximizar so realcam.
fn caption_button_style(index: usize, hovered: bool, theme: &Theme) -> PillStyle {
    match (index, hovered) {
        (2, true) => PillStyle::new(CLOSE_HOVER_RED, CLOSE_HOVER_RED, (255, 255, 255)),
        (_, true) => PillStyle::new(theme.surface_line, theme.surface_line, theme.fg),
        _ => PillStyle::new(theme.surface, theme.surface_line, theme.fg),
    }
}

/// Os tres botoes da janela, na ordem em que `native_button_index` os conta.
fn caption_tooltip_label(index: usize, maximized: bool) -> &'static str {
    match index {
        0 => "Minimizar",
        1 if maximized => "Restaurar",
        1 => "Maximizar",
        _ => "Fechar",
    }
}

/// O rato entrou num alvo com dica `text`, ou saiu de todos (""). Esconde a
/// dica que estiver a vista e, se houver texto, agenda a nova.
fn hover_tooltip(window: HWND, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, KillTimer, SetTimer};
    hide_tooltip();
    let timer = TOOLTIP_TIMER.swap(0, Ordering::AcqRel);
    if timer != 0 {
        unsafe {
            KillTimer(std::ptr::null_mut(), timer);
        }
    }
    let mut pending = TOOLTIP_PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if window.is_null() || text.is_empty() {
        *pending = None;
        return;
    }
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    *pending = Some((root as usize, text.to_string()));
    drop(pending);
    let id = unsafe {
        SetTimer(
            std::ptr::null_mut(),
            0,
            TOOLTIP_DELAY_MS,
            Some(tooltip_timer),
        )
    };
    TOOLTIP_TIMER.store(id, Ordering::Release);
}

unsafe extern "system" fn tooltip_timer(_hwnd: HWND, _message: u32, id: usize, _time: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::KillTimer;
    unsafe {
        KillTimer(std::ptr::null_mut(), id);
    }
    let _ = TOOLTIP_TIMER.compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire);
    show_pending_tooltip();
}

/// Escala do ecra (1.0 a 96 dpi): a letra da dica acompanha o DPI.
fn screen_scale() -> f64 {
    use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSY, ReleaseDC};
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return 1.0;
        }
        let dpi = GetDeviceCaps(screen, LOGPIXELSY as i32);
        ReleaseDC(std::ptr::null_mut(), screen);
        (dpi as f64 / 96.0).max(1.0)
    }
}

/// Largura e altura do texto da dica, medidas com a letra dela.
fn hint_text_size(text: &str, scale: f64) -> (i32, i32) {
    use windows_sys::Win32::Graphics::Gdi::{DT_CALCRECT, DT_WORDBREAK, GetDC, ReleaseDC};
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return (0, 0);
        }
        let font = create_font(-(HINT_FONT_PX * scale).round() as i32, FW_NORMAL as i32);
        let old = SelectObject(screen, font as _);
        let mut area = RECT {
            left: 0,
            top: 0,
            right: (HINT_MAX_WIDTH_PX * scale).round() as i32,
            bottom: 0,
        };
        draw_text(
            screen,
            text,
            &mut area,
            DT_CALCRECT | DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
        );
        SelectObject(screen, old);
        DeleteObject(font as _);
        ReleaseDC(std::ptr::null_mut(), screen);
        (area.right - area.left, area.bottom - area.top)
    }
}

/// A janela da dica, criada na primeira vez (e de novo se o dono mudou).
fn hint_popup(root: HWND) -> Option<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_OWNER, GetWindow, IsWindow, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };
    let existing = HINT_HWND.load(Ordering::Acquire) as HWND;
    unsafe {
        if !existing.is_null() && IsWindow(existing) != 0 {
            if GetWindow(existing, GW_OWNER) == root {
                return Some(existing);
            }
            DestroyWindow(existing);
        }
        // Transparente ao rato: fica por cima das paginas e nao pode comer o
        // clique de ninguem.
        let hint = CreateWindowExW(
            AUX_POPUP_EX_STYLE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            windows_sys::w!("STATIC"),
            windows_sys::w!(""),
            AUX_POPUP_STYLE,
            0,
            0,
            1,
            1,
            root,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        if hint.is_null() {
            return None;
        }
        // Sem SetLayeredWindowAttributes: quem pinta e o UpdateLayeredWindow,
        // com alfa por pixel (render_hint).
        HINT_HWND.store(hint as usize, Ordering::Release);
        Some(hint)
    }
}

/// Raio da dica: pilula, como os botoes (metade da altura), limitado para
/// dicas de varias linhas nao ficarem ovais.
fn hint_radius(height: f64, scale: f64) -> f64 {
    (height / 2.0).min(28.0 * scale.max(1.0))
}

/// Aplica a forma da dica a um bitmap BGRA ja pintado (fundo + texto):
/// cobertura suave pela distancia ao rectangulo arredondado, um fio de borda
/// na cor dos botoes e o resultado PRE-MULTIPLICADO, como o
/// UpdateLayeredWindow exige. O recorte por regiao do GDI deixava escadinhas.
fn apply_hint_shape(pixels: &mut [u8], width: usize, height: usize, radius: f32, line: Rgb) {
    for y in 0..height {
        for x in 0..width {
            let distance = round_rect_sdf(
                x as f32 + 0.5,
                y as f32 + 0.5,
                width as f32,
                height as f32,
                radius,
            );
            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            // Fio de 1 px por dentro do contorno.
            let border = (1.0 - (distance + 1.0).abs()).clamp(0.0, 1.0);
            let index = (y * width + x) * 4;
            let Some(pixel) = pixels.get_mut(index..index + 4) else {
                return;
            };
            for (channel, target) in [(0, line.2), (1, line.1), (2, line.0)] {
                let base = pixel[channel] as f32;
                let mixed = base + (target as f32 - base) * border;
                pixel[channel] = (mixed * coverage).round() as u8;
            }
            pixel[3] = (coverage * 255.0).round() as u8;
        }
    }
}

/// Pinta a dica e entrega-a ao Windows com alfa por pixel (cantos suaves).
/// Posiciona e dimensiona a janela na mesma chamada.
fn render_hint(hint: HWND, x: i32, y: i32, width: i32, height: i32, text: &str, scale: f64) {
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DT_WORDBREAK, DeleteDC, GetDC,
        RGBQUAD, ReleaseDC,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
    if width <= 0 || height <= 0 {
        return;
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return;
        }
        let memory = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }; 1],
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            screen,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if memory.is_null() || bitmap.is_null() || bits.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap as _);
            }
            if !memory.is_null() {
                DeleteDC(memory);
            }
            ReleaseDC(std::ptr::null_mut(), screen);
            return;
        }
        let old_bitmap = SelectObject(memory, bitmap as _);

        // Fundo opaco e texto primeiro (o GDI nao escreve alfa); a forma vem
        // depois, pixel a pixel.
        let theme = Theme::system();
        let all = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let fill = CreateSolidBrush(rgb3(theme.surface));
        FillRect(memory, &all, fill);
        DeleteObject(fill as _);
        let font = create_font(-(HINT_FONT_PX * scale).round() as i32, FW_NORMAL as i32);
        let old_font = SelectObject(memory, font as _);
        SetBkMode(memory, TRANSPARENT as i32);
        SetTextColor(memory, rgb3(theme.fg));
        let pad_x = (HINT_PADDING_X_PX * scale).round() as i32;
        let pad_y = (HINT_PADDING_Y_PX * scale).round() as i32;
        let mut area = RECT {
            left: pad_x,
            top: pad_y,
            right: width - pad_x,
            bottom: height - pad_y,
        };
        draw_text(
            memory,
            text,
            &mut area,
            DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
        );
        SelectObject(memory, old_font);
        DeleteObject(font as _);

        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (width * height * 4) as usize);
        apply_hint_shape(
            pixels,
            width as usize,
            height as usize,
            hint_radius(height as f64, scale) as f32,
            theme.surface_line,
        );

        let position = POINT { x, y };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hint, screen, &position, &size, memory, &source, 0, &blend, ULW_ALPHA,
        );

        SelectObject(memory, old_bitmap);
        DeleteObject(bitmap as _);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
}

/// Mostra a dica agendada no centro da janela. Chamada pelo temporizador (e
/// pelos testes, sem esperar).
fn show_pending_tooltip() {
    let Some((root, text)) = TOOLTIP_PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    else {
        return;
    };
    let root = root as HWND;
    let Some(hint) = hint_popup(root) else {
        return;
    };
    let scale = screen_scale();
    let pad_x = (HINT_PADDING_X_PX * scale).round() as i32;
    let pad_y = (HINT_PADDING_Y_PX * scale).round() as i32;
    let (text_w, text_h) = hint_text_size(&text, scale);
    let (width, height) = (text_w + 2 * pad_x, text_h + 2 * pad_y);
    unsafe {
        let mut client = RECT::default();
        if GetClientRect(root, &mut client) == 0 {
            return;
        }
        let mut origin = POINT { x: 0, y: 0 };
        ClientToScreen(root, &mut origin);
        let (x, y) = splash_origin(client.right, client.bottom, width, height);
        render_hint(
            hint,
            origin.x + x,
            origin.y + y,
            width,
            height,
            &text,
            scale,
        );
        show_popup_without_activation(hint);
    }
    *HINT_TEXT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = text;
}

fn hide_tooltip() {
    let hint = HINT_HWND.load(Ordering::Acquire) as HWND;
    if !hint.is_null() {
        unsafe {
            ShowWindow(hint, SW_HIDE);
        }
    }
}

/// Menu de tema no cursor, com a escolha em vigor marcada. Devolve a opcao
/// clicada, ou None se o menu foi fechado sem escolha.
fn pick_theme_from_menu(hwnd: HWND) -> Option<ThemeChoice> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GA_ROOT, GetAncestor, MF_CHECKED, MF_STRING,
        SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    };
    let current = ThemeChoice::current();
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        for (index, choice) in ThemeChoice::ALL.iter().enumerate() {
            let flags = if *choice == current {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let label: Vec<u16> = choice
                .label()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            AppendMenuW(menu, flags, index + 1, label.as_ptr());
        }
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        // Sem o dono em primeiro plano, o menu nao fecha ao clicar fora.
        let root = GetAncestor(hwnd, GA_ROOT);
        SetForegroundWindow(root);
        let picked = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            0,
            root,
            std::ptr::null(),
        );
        DestroyMenu(menu);
        usize::try_from(picked)
            .ok()
            .and_then(|id| id.checked_sub(1))
            .and_then(|index| ThemeChoice::ALL.get(index).copied())
    }
}

/// Pede o WM_MOUSELEAVE a um botao nativo: sem ele a dica ficava a vista
/// depois de o rato sair.
fn track_mouse_leave(hwnd: HWND) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        TrackMouseEvent(&mut track);
    }
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

const NATIVE_BUTTON_NONE: usize = usize::MAX;
static CAPTION_PRESSED_BUTTON: AtomicUsize = AtomicUsize::new(NATIVE_BUTTON_NONE);
/// O botao da janela cuja dica esta carregada no tooltip.
static CAPTION_TOOLTIP_BUTTON: AtomicUsize = AtomicUsize::new(NATIVE_BUTTON_NONE);

fn native_button_index(width: i32, x: i32) -> Option<usize> {
    if width <= 0 || x < 0 || x >= width {
        return None;
    }
    if x < width / 3 {
        Some(0)
    } else if x < (width * 2) / 3 {
        Some(1)
    } else {
        Some(2)
    }
}

fn native_release_matches(pressed: Option<usize>, released: Option<usize>) -> Option<usize> {
    pressed.filter(|index| Some(*index) == released)
}

// ReleaseCapture envia WM_CAPTURECHANGED antes de voltar. Retirar o estado
// antes da chamada preserva o clique legitimo sem deixar um press pendurado.
fn take_native_pressed_button(
    pressed: &AtomicUsize,
    release_capture: impl FnOnce(),
) -> Option<usize> {
    let index = pressed.swap(NATIVE_BUTTON_NONE, Ordering::AcqRel);
    release_capture();
    (index != NATIVE_BUTTON_NONE).then_some(index)
}

fn point_inside_client(width: i32, height: i32, x: i32, y: i32) -> bool {
    width > 0 && height > 0 && x >= 0 && x < width && y >= 0 && y < height
}

fn native_caption_release(
    pressed: Option<usize>,
    captured: bool,
    width: i32,
    height: i32,
    x: i32,
    y: i32,
) -> Option<usize> {
    (captured && point_inside_client(width, height, x, y))
        .then(|| native_release_matches(pressed, native_button_index(width, x)))
        .flatten()
}

unsafe extern "system" fn caption_buttons_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    const WM_SYSCOMMAND_NATIVE: u32 = 0x0112;
    const SC_MINIMIZE_NATIVE: usize = 0xF020;
    const SC_MAXIMIZE_NATIVE: usize = 0xF030;
    const SC_CLOSE_NATIVE: usize = 0xF060;
    const SC_RESTORE_NATIVE: usize = 0xF120;

    match message {
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let theme = Theme::system();
                    let width = (client.right - client.left).max(1) as f64;
                    let height = (client.bottom - client.top).max(1) as f64;
                    let scale = (height / TITLE_TAB_HEIGHT).max(1.0);
                    let third = width / 3.0;
                    let font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);
                    let maximized = IsZoomed(GetParent(hwnd)) != 0;
                    let labels = ["—", if maximized { "❐" } else { "□" }, "×"];
                    let hovered = CAPTION_TOOLTIP_BUTTON.load(Ordering::Acquire);
                    for (index, label) in labels.into_iter().enumerate() {
                        draw_pill(
                            hdc,
                            UiRect {
                                x: index as f64 * third,
                                y: 0.0,
                                width: if index == 2 {
                                    width - third * 2.0
                                } else {
                                    third
                                },
                                height,
                            },
                            label,
                            caption_button_style(index, hovered == index, &theme),
                            scale,
                            font,
                            theme.page_bg,
                        );
                    }
                    DeleteObject(font as _);
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let width = client.right - client.left;
                let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                if let Some(index) = native_button_index(width, x) {
                    CAPTION_PRESSED_BUTTON.store(index, Ordering::Release);
                    hover_tooltip(hwnd, "");
                    SetCapture(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let captured = GetCapture() == hwnd;
            let pressed = take_native_pressed_button(&CAPTION_PRESSED_BUTTON, || {
                if captured {
                    ReleaseCapture();
                }
            });
            let parent = GetParent(hwnd);
            if parent.is_null() {
                return 0;
            }
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) == 0 {
                return 0;
            }
            let width = client.right - client.left;
            let height = client.bottom - client.top;
            let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
            let Some(index) = native_caption_release(pressed, captured, width, height, x, y) else {
                return 0;
            };
            let command = match index {
                0 => SC_MINIMIZE_NATIVE,
                1 if IsZoomed(parent) != 0 => SC_RESTORE_NATIVE,
                1 => SC_MAXIMIZE_NATIVE,
                _ => SC_CLOSE_NATIVE,
            };
            SendMessageW(parent, WM_SYSCOMMAND_NATIVE, command, 0);
            0
        }
        WM_MOUSEMOVE => {
            // A dica muda quando o rato passa de um botao para outro.
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                let index = native_button_index(client.right - client.left, x);
                let last = CAPTION_TOOLTIP_BUTTON
                    .swap(index.unwrap_or(NATIVE_BUTTON_NONE), Ordering::AcqRel);
                if index != Some(last) {
                    if last == NATIVE_BUTTON_NONE {
                        track_mouse_leave(hwnd);
                    }
                    let maximized = IsZoomed(GetParent(hwnd)) != 0;
                    let text = index.map_or("", |index| caption_tooltip_label(index, maximized));
                    hover_tooltip(hwnd, text);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        WM_MOUSELEAVE => {
            CAPTION_TOOLTIP_BUTTON.store(NATIVE_BUTTON_NONE, Ordering::Release);
            hover_tooltip(hwnd, "");
            InvalidateRect(hwnd, std::ptr::null(), 0);
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            CAPTION_PRESSED_BUTTON.store(NATIVE_BUTTON_NONE, Ordering::Release);
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn home_button_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    match message {
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let theme = Theme::system();
                    let width = (client.right - client.left).max(1) as f64;
                    let height = (client.bottom - client.top).max(1) as f64;
                    let scale = (height / 30.0).max(1.0);
                    let font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);
                    let rect = UiRect {
                        x: 0.0,
                        y: 0.0,
                        width,
                        height,
                    };
                    draw_pill(
                        hdc,
                        rect,
                        "Home",
                        PillStyle::new(theme.surface, theme.surface_line, theme.fg),
                        scale,
                        font,
                        theme.bar_bg,
                    );
                    DeleteObject(font as _);
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !HOME_TOOLTIP_ARMED.swap(true, Ordering::AcqRel) {
                track_mouse_leave(hwnd);
                hover_tooltip(hwnd, "Voltar à Home · Botão direito: tema claro ou escuro");
            }
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        WM_MOUSELEAVE => {
            HOME_TOOLTIP_ARMED.store(false, Ordering::Release);
            hover_tooltip(hwnd, "");
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            hover_tooltip(hwnd, "");
            SetCapture(hwnd);
            0
        }
        WM_RBUTTONUP => {
            hover_tooltip(hwnd, "");
            if let Some(choice) = pick_theme_from_menu(hwnd)
                && reference_data != 0
            {
                let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                let _ = proxy.send_event(UserEvent::ThemeChosen(choice));
            }
            0
        }
        WM_LBUTTONUP => {
            let captured = GetCapture() == hwnd;
            if captured {
                ReleaseCapture();
            }
            let mut client = RECT::default();
            let inside = GetClientRect(hwnd, &mut client) != 0 && {
                let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
                point_inside_client(client.right - client.left, client.bottom - client.top, x, y)
            };
            if captured && inside && reference_data != 0 {
                let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                let _ = proxy.send_event(UserEvent::HomeRequested);
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
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
        let nonce = wparam;
        if nonce == 0 || LIFECYCLE_LAST_HOME_NONCE.swap(nonce, Ordering::AcqRel) != nonce {
            let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
            SetWindowTextW(hwnd, windows_sys::w!(""));
            let _ = proxy.send_event(UserEvent::HomeRequested);
        }
        return 0;
    }

    if message == WM_KEYDOWN {
        let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
        let ctrl = (GetAsyncKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0;
        let shift = (GetAsyncKeyState(VK_SHIFT as i32) as u16 & 0x8000) != 0;

        match wparam as u32 {
            13 => {
                let text = window_text(hwnd);
                debug_log(format_args!(
                    "omnibox: Enter ({} chars)",
                    text.chars().count()
                ));
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
            0x52 if ctrl && !shift => {
                let _ = proxy.send_event(UserEvent::ToggleAutoScroll);
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
                            if let Err(error) = worker_store.append(&entry) {
                                let _ = worker_proxy
                                    .send_event(UserEvent::HistoryWriteFailed(error.to_string()));
                            }
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
            let message = "fila do histórico saturada; uma entrada não foi gravada".to_string();
            eprintln!("{message}");
            let _ = self
                .proxy
                .send_event(UserEvent::HistoryWriteFailed(message));
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

    /// Apagar a memoria esquece tambem a sessao de pesquisa viva. O worker
    /// apaga sessions/<id>.json e poe tombstone no id; uma sessao que ficasse
    /// na app voltava ao disco no proximo save_session (com a pergunta ja
    /// apagada) e cada captura nova com esse id era recusada em silencio.
    /// Pedir a sessao aqui obriga quem apaga a larga-la.
    fn clear(&self, current_research: &mut Option<ResearchSession>) {
        *current_research = None;
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
            && x < self.x + self.width
            && y >= self.y
            && y < self.y + self.height
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
    /// Ctrl/Shift/Alt no teclado da janela principal (a barra com o foco).
    modifiers: winit::keyboard::ModifiersState,
    /// O rato esta em cima do "Ir" da Home: pinta-se em degradê.
    home_go_hover: bool,
    exit_button: Option<HWND>,
    home_button: Option<HWND>,
    caption_buttons: Option<HWND>,
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
    /// Painel lateral do historico inteligente (Ctrl+H).
    side_panel: Option<WebView>,
    /// A consulta de memoria que alimenta as sugestoes do painel.
    panel_suggestion_query: Option<String>,
    /// Servico aberto no painel lateral (WhatsApp, Meet, YouTube, Gmail).
    service_panel: Option<(Service, WebView)>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let config = CoreConfig::default();
        let history_store =
            HistoryStore::with_limit(config.data_dir.join("history.jsonl"), config.history_limit);
        // A escolha de tema vale antes do primeiro desenho.
        ThemeChoice::load(&config.data_dir.join("theme")).apply();
        GMAIL_NOTIFICATIONS.store(
            load_gmail_setting(&config.data_dir.join("gmail")),
            Ordering::Release,
        );
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
            modifiers: winit::keyboard::ModifiersState::empty(),
            home_go_hover: false,
            exit_button: None,
            home_button: None,
            caption_buttons: None,
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
            side_panel: None,
            panel_suggestion_query: None,
            service_panel: None,
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
        debug_log(format_args!(
            "focus={focused} surface={:?} home_focused={}",
            self.surface, self.home_focused
        ));
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
            self.sync_caption_buttons();
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
            if SetWindowSubclass(parent, Some(window_subclass), WINDOW_SUBCLASS_ID, proxy_ptr) == 0
            {
                eprintln!("failed to subclass effective NeuralIA HWND");
            }

            // A troca de decorations pode substituir/reparentar o HWND nativo.
            // A omnibox e o Home sao filhos Win32 reais: se continuarem ligados
            // ao HWND antigo, ficam invisiveis ou deixam de receber teclado/rato.
            for child in [self.omnibox, self.home_button].into_iter().flatten() {
                if GetParent(child) != parent {
                    SetParent(child, parent);
                    if GetParent(child) != parent {
                        eprintln!("failed to reparent native NeuralIA control to effective HWND");
                    }
                }
            }
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
                DestroyWindow(edit);
                return;
            }

            if SetWindowSubclass(parent, Some(window_subclass), WINDOW_SUBCLASS_ID, proxy_ptr) == 0
            {
                eprintln!("failed to subclass NeuralIA parent HWND while creating omnibox");
            }

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
        let scale = window.scale_factor().max(1.0);

        // O EDIT precisa manter a mesma identidade Win32 durante as trocas de
        // decorations/HWND. Fora da Home ele continua WS_VISIBLE, mas fica
        // estacionado muito fora do cliente e com 1x1 px: não aparece na
        // titlebar nem disputa espaço com as abas.
        let inner = if self.surface == Surface::Home {
            let layout =
                HomeLayout::new(size.width as f64, size.height as f64, window.scale_factor());
            let pad_x = 22.0 * scale;
            let pad_y = 5.0 * scale;
            UiRect {
                x: layout.input.x + pad_x,
                y: layout.input.y + pad_y,
                width: (layout.input.width - pad_x * 2.0).max(1.0),
                height: (layout.input.height - pad_y * 2.0).max(1.0),
            }
        } else {
            UiRect {
                x: -4096.0 * scale,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }
        };

        unsafe {
            SetWindowPos(
                edit,
                std::ptr::null_mut(),
                inner.x.round() as i32,
                inner.y.round() as i32,
                inner.width.round() as i32,
                inner.height.round() as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            apply_omnibox_interactivity(edit, self.surface);
            ShowWindow(edit, SW_SHOW);
        }
        if self.surface == Surface::Home {
            self.apply_omnibox_font(inner.height);
        }
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

    fn set_omnibox_visibility(&self, visible: bool, focus: bool) {
        let Some(edit) = self.omnibox else {
            return;
        };
        unsafe {
            ShowWindow(edit, if visible { SW_SHOW } else { SW_HIDE });
            if visible && focus {
                EnableWindow(edit, 1);
                SetFocus(edit);
            }
        }
    }

    fn show_omnibox(&self, visible: bool) {
        self.set_omnibox_visibility(visible, visible);
    }

    fn show_omnibox_passive(&self, visible: bool) {
        self.set_omnibox_visibility(visible, false);
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
        if let Some(window) = &self.window {
            window.set_fullscreen(None);
        }
    }

    fn destroy_web_surfaces(&mut self) {
        if lifecycle_probe_enabled() {
            LIFECYCLE_COMPARATOR_READY.store(false, Ordering::Release);
            LIFECYCLE_HOME_READY.store(false, Ordering::Release);
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
        if let Some(button) = self.home_button.take() {
            unsafe {
                DestroyWindow(button);
            }
        }
        if let Some(buttons) = self.caption_buttons.take() {
            unsafe {
                DestroyWindow(buttons);
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

        // Teardown e transicao para Home sao coisas diferentes. Antes esta
        // funcao sempre agendava RestoreHomeDecorations; quando um novo
        // comparador era aberto a partir da propria Home, esse timer podia
        // disparar dentro do pump aninhado de build_as_child e recolocar a
        // moldura da Home no meio da criacao dos tres WRY_WEBVIEW. O resultado
        // era exatamente a regressao dos ciclos 2+: hosts visiveis presos ao
        // HWND errado. Aqui so destruimos. Quem realmente entra na Home agenda
        // a restauracao depois.
        if let Some(window) = &self.window {
            hide_orphaned_wry_hosts(window);
        }

        if let Ok(mut bytes) = self.pdf_bytes.lock() {
            *bytes = Vec::new();
        }
        self.reading_pdf = false;
    }

    fn schedule_home_restoration(&self) {
        if lifecycle_probe_enabled() {
            LIFECYCLE_HOME_READY.store(false, Ordering::Release);
        }
        self.timers
            .after(Duration::from_millis(40), UserEvent::RestoreHomeDecorations);
    }

    fn show_home(&mut self) {
        debug_log(format_args!("show_home (surface era {:?})", self.surface));
        self.close_side_panel();
        self.close_service_panel();
        self.next_generation();
        self.surface = Surface::Home;

        // Home é uma fronteira de ciclo de vida real. Destruir os controllers
        // aqui garante que nenhum host WRY_WEBVIEW sobreviva oculto/reparentado
        // entre pesquisas. Reuso dentro do próprio comparador continua possível,
        // mas sair para Home sempre encerra as superfícies web.
        self.destroy_web_surfaces();
        self.schedule_home_restoration();

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
        self.schedule_home_restoration();
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

    /// Ctrl+Shift+Delete apagava historico e memoria local de uma vez, sem
    /// perguntar e sem volta. Agora pergunta, com o "Nao" por omissao.
    fn confirm_clear_history(&self) -> bool {
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return false;
        };
        let body = wide_null(
            "Apagar TODO o histórico e a memória local da NeuralIA?\n\nIsto não pode ser desfeito.",
        );
        let title = wide_null("NeuralIA — Apagar histórico");
        let answer = unsafe {
            MessageBoxW(
                hwnd,
                body.as_ptr(),
                title.as_ptr(),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        clear_history_confirmed(answer)
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
        debug_log(format_args!(
            "handle_input ({} chars) surface={:?}",
            input.chars().count(),
            self.surface
        ));
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
            InputRoute::Theme(Some(choice)) => self.choose_theme(choice),
            InputRoute::Theme(None) => self.show_splash(
                "Use tema:sistema, tema:claro ou tema:escuro.".to_string(),
                3,
            ),
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

    /// "Pesquisar" da barra de selecao. Vai direto ao `compare`: o texto
    /// selecionado numa pagina e uma pergunta, nunca um comando da omnibox
    /// (ver `selection_search_question`).
    fn search_selection(&mut self, text: &str) {
        if let Some(question) = selection_search_question(text) {
            self.compare(question);
        }
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
        self.schedule_home_restoration();
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

    fn pdf_webview_builder(&self) -> WebViewBuilder<'static> {
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
        let init_script = bind_page_script(
            &format!("{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}"),
            &capability,
            false,
        );

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
        let init_script = bind_page_script(
            &format!("{NEURALIA_KEYMAP_SCRIPT}\n{EXTERNAL_RETURN_BUTTON}\n{agent_script}"),
            &capability,
            false,
        );

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
            AgentStepDecision::ConfirmExtract { security, reason } => {
                let action = AgentAction::Extract {
                    target: None,
                    schema: "page-text".into(),
                };
                let approved =
                    self.confirm_agent_action(&format!("{reason}: {}", page.url), &action);
                if let Some(agent) = self.active_agent.as_mut() {
                    agent.policy.record_user_confirmation(&security, approved);
                }
                if !approved {
                    self.show_splash("Ação do agente cancelada.".to_string(), 3);
                    self.finish_agent(AgentTermination::UserRejected);
                    return;
                }
                self.extract_agent_observation(&page);
            }
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
        self.close_side_panel();
        self.close_service_panel();
        let reuse_comparator = self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.views.len() == COMPARATOR_COLUMNS);
        if !reuse_comparator {
            self.destroy_web_surfaces();
        }
        self.show_omnibox_passive(true);
        self.position_omnibox();

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
        debug_log(format_args!("open_comparator: set_decorations(false)"));
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
            next_context_id: 1,
            panel_width: 0.0,
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
        self.sync_home_button();
        self.sync_caption_buttons();
        self.show_omnibox_passive(true);
        self.position_omnibox();

        for delay_ms in COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS {
            self.timers.after(
                Duration::from_millis(delay_ms),
                UserEvent::RelayoutComparator,
            );
        }

        // Chegar aqui significa que os tres build_as_child ja retornaram,
        // self.comparator ja existe e o layout inicial foi aplicado. Nesta
        // altura set_decorations(false) pode ja ter trocado/reparentado o HWND
        // nativo. Reinstale a subclass NO HWND efetivo antes de publicar Ready:
        // o gate pode enviar Home imediatamente depois de observar o flag.
        self.ensure_window_subclass();

        // Ready só é publicado em about_to_wait(), depois de devolver o
        // controlo ao event loop fora do pump aninhado do WebView2.
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

        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
        {
            comp.expanded = if comp.expanded == Some(idx) {
                None
            } else {
                Some(idx)
            };
        }
        self.bar_hover = None;
        self.needs_clear = true;

        // Expandir ocupa apenas a área de conteúdo. A titlebar do NeuralIA e
        // minimizar/maximizar/fechar permanecem acessíveis.
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.sync_comparator_buttons();
        self.sync_exit_button();
        self.sync_home_button();
        self.sync_caption_buttons();
        self.show_omnibox_passive(true);
        self.position_omnibox();
        self.request_redraw();
    }

    fn minimize_comparator(&mut self, idx: usize) {
        if self.surface != Surface::Comparator {
            return;
        }

        let can_minimize = self
            .comparator
            .as_ref()
            .is_some_and(|comp| can_minimize_column(&comp.minimized, comp.views.len(), idx));
        if !can_minimize {
            self.show_splash(
                "Pelo menos um painel precisa continuar visível.".to_string(),
                2,
            );
            return;
        }

        // So agora a operacao foi validada. Antes, um pedido impossivel para a
        // ultima coluna visivel fechava a fonte lateral e depois dizia que nao
        // podia minimizar: o clique rejeitado destruia estado.
        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some())
        {
            self.close_split();
        }

        let mut was_expanded = false;
        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
        {
            was_expanded = comp.expanded == Some(idx);
            comp.expanded = None;
            comp.minimized[idx] = true;
        }

        if was_expanded && let Some(window) = &self.window {
            window.set_fullscreen(None);
        }

        self.bar_hover = None;
        self.needs_clear = true;
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.sync_comparator_buttons();
        self.sync_exit_button();
        self.sync_home_button();
        self.sync_caption_buttons();
        self.show_omnibox_passive(true);
        self.position_omnibox();
        self.request_redraw();
    }

    fn restore_comparator(&mut self) {
        if let Some(comp) = &mut self.comparator {
            comp.expanded = None;
        }
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.sync_comparator_buttons();
        self.sync_exit_button();
        self.sync_home_button();
        self.sync_caption_buttons();
        self.show_omnibox_passive(true);
        self.position_omnibox();
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
        let logical_w = (size.width as f64 / scale
            - self
                .comparator
                .as_ref()
                .map_or(0.0, |comp| comp.panel_width))
        .max(1.0);
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
                // Fullscreen fica geometricamente estavel. A versao anterior
                // mudava o bounds do WebView toda vez que o cursor tocava o
                // topo para mostrar/esconder chrome, causando flicker e pump
                // de layout em cascata no WebView2.
                for (i, v) in comp.views.iter().enumerate() {
                    if i == idx {
                        let _ = v.webview.set_bounds(wry::Rect {
                            position: LogicalPosition::new(0.0, content_y).into(),
                            size: LogicalSize::new(logical_w, content_h).into(),
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
            // Uma coluna so fala por si: o `col` tem de ser o dela.
            IpcAction::Ask { col, text } if col == col_index => Some(UserEvent::AskEverywhere {
                source_index: col_index,
                text,
            }),
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
            IpcAction::Omnibox => Some(UserEvent::OpenPalette(col_index)),
            IpcAction::Reload => Some(UserEvent::ReloadTarget(PageTarget::Column(col_index))),
            IpcAction::Print => Some(UserEvent::PrintTarget(PageTarget::Column(col_index))),
            IpcAction::DevTools => {
                Some(UserEvent::OpenDevToolsTarget(PageTarget::Column(col_index)))
            }
            IpcAction::ViewSource => {
                Some(UserEvent::ViewSourceTarget(PageTarget::Column(col_index)))
            }
            IpcAction::Fullscreen => Some(UserEvent::ExpandComparator(col_index)),
            IpcAction::ShortcutExpand { col } => Some(UserEvent::ExpandComparator(col)),
            IpcAction::Minimize { col } if col == col_index => {
                Some(UserEvent::MinimizeComparator(col_index))
            }
            IpcAction::NewTab { col: Some(col) } if col == col_index => {
                Some(UserEvent::NewTab(col_index))
            }
            IpcAction::Expand { col } if col == col_index => {
                Some(UserEvent::ExpandComparator(col_index))
            }
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
        let keymap = bind_page_script(NEURALIA_KEYMAP_SCRIPT, &capability, false);
        let auto_submit = AI_AUTO_SUBMIT_SCRIPT.replace("__NEURALIA_CAP__", &capability);
        let inject = COMPARATOR_INJECT_SCRIPT.replace("__NEURALIA_CAP__", &capability);

        themed_webview_builder()
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
            .with_permission_handler(|kind| web_media_permission(kind, true))
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

        let Ok(valid) = neural_core::validate_web_url(&url) else {
            self.show_splash("URL do link inválida.".to_string(), 3);
            return;
        };
        if neural_core::is_local_network_target(&valid) {
            self.show_splash(
                "O link não pode redirecionar a coluna para a rede local.".to_string(),
                4,
            );
            return;
        }

        let loaded = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(index))
            .is_some_and(|view| view.webview.load_url(valid.as_str()).is_ok());

        // Um popup/link que falha pertence à coluna que o originou. Nunca
        // derrube as três colunas para abrir um WebView solitário como fallback:
        // além de destruir a comparação, isso reabria a corrida de lifecycle.
        if !loaded {
            self.show_splash(
                "Não consegui abrir esse link na coluna sem perder a comparação.".to_string(),
                4,
            );
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

    fn split_ipc_event_impl(
        source_index: usize,
        private: bool,
        action: IpcAction,
    ) -> Option<UserEvent> {
        match action {
            // Defesa em profundidade: a barra de um painel privado nem mostra
            // o "Pesquisar", e mesmo que uma mensagem chegasse o texto nao
            // pode sair para o comparador (historico e memoria).
            IpcAction::Search { .. } if private => None,
            IpcAction::SplitClose => Some(UserEvent::CloseSplit),
            IpcAction::SplitExpand | IpcAction::Fullscreen => {
                Some(UserEvent::ToggleSplitFullscreen)
            }
            IpcAction::Palette { col } if col == source_index => {
                Some(UserEvent::OpenPalette(source_index))
            }
            IpcAction::Omnibox => Some(UserEvent::OpenPalette(source_index)),
            IpcAction::NewTab { col: Some(col) } if col == source_index => {
                Some(UserEvent::NewTab(source_index))
            }
            IpcAction::ShortcutExpand { col } => Some(UserEvent::ExpandComparator(col)),
            IpcAction::Reload => Some(UserEvent::ReloadTarget(PageTarget::Split)),
            IpcAction::Print => Some(UserEvent::PrintTarget(PageTarget::Split)),
            IpcAction::DevTools => Some(UserEvent::OpenDevToolsTarget(PageTarget::Split)),
            IpcAction::ViewSource => Some(UserEvent::ViewSourceTarget(PageTarget::Split)),
            other => common_ipc_event(other),
        }
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
        let init_script = split_init_script(source_index, source_name, &capability, private);

        themed_webview_builder()
            .with_incognito(private)
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                let event = Self::split_ipc_event_impl(source_index, private, action);
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
            .with_permission_handler(|kind| web_media_permission(kind, true))
            .with_focused(true)
    }

    fn open_split(&mut self, source_index: usize, url: String, allow_local: bool) -> bool {
        self.open_split_mode(source_index, url, allow_local, false, None)
    }

    /// Um pedido de split que chega depois de o comparador desaparecer (o
    /// popup da pagina ficou na fila atras do Home) ainda pode abrir como Web
    /// normal. Um pedido PRIVADO nao: web() grava historico, captura memoria
    /// e usa o perfil com cookies normais.
    fn split_request_fallback(surface: Surface, private: bool) -> SplitFallback {
        if surface == Surface::Comparator {
            SplitFallback::OpenSplit
        } else if private {
            SplitFallback::Ignore
        } else {
            SplitFallback::OpenWeb
        }
    }

    fn open_split_mode(
        &mut self,
        source_index: usize,
        url: String,
        allow_local: bool,
        private: bool,
        existing_context_id: Option<u64>,
    ) -> bool {
        match Self::split_request_fallback(self.surface, private) {
            SplitFallback::OpenSplit => {}
            SplitFallback::OpenWeb => {
                self.web(url);
                return true;
            }
            SplitFallback::Ignore => return false,
        }

        let Ok(valid) = neural_core::validate_web_url(&url) else {
            self.show_splash("URL da fonte inválida.".to_string(), 3);
            return false;
        };
        if !allow_local && neural_core::is_local_network_target(&valid) {
            self.show_splash(
                "A página não pode redirecionar a fonte para a rede local.".to_string(),
                4,
            );
            return false;
        }

        let Some(source_name) = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(source_index))
            .map(|view| view.name)
        else {
            return false;
        };

        let generation = self.current_generation();
        let Some(window) = &self.window else {
            return false;
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

        // Construir primeiro, com o estado antigo intacto. WebView2 pode falhar
        // ou entrar num pump aninhado; uma tentativa falhada nao pode destruir
        // o Split que o utilizador ainda esta a ver nem sair da expansao atual.
        let built = self
            .split_webview_builder(
                source_index,
                source_name,
                allow_local.then(|| valid.origin().ascii_serialization()),
                private,
            )
            .with_bounds(bounds)
            .with_url(valid.as_str())
            .build_as_child(window);

        if !split_build_is_current(generation, self.current_generation(), self.surface) {
            if let Ok(webview) = built {
                drop(webview);
            }
            return false;
        }

        let committed = match &mut self.comparator {
            Some(comp) => commit_split_build(built, &mut comp.split, &mut comp.expanded),
            None => {
                if let Ok(webview) = built {
                    drop(webview);
                }
                return false;
            }
        };

        match committed {
            Ok((webview, previous)) => {
                if let Some(previous) = previous {
                    let _ = previous.webview.set_visible(false);
                    let _ = previous.webview.focus_parent();
                    drop(previous);
                }

                self.leave_fullscreen();
                self.hide_comparator_splitters();

                // So uma fonte que abriu de verdade entra na memoria/sessao.
                // Antes, uma falha de build deixava uma fonte fantasma gravada.
                if split_open_records_source(existing_context_id, private)
                    && let Some((title, mut document)) =
                        split_source_memory(&valid, source_name, private)
                {
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

                let _ = webview.zoom(self.zoom);
                if let Some(comp) = &mut self.comparator {
                    let context_id = if private {
                        None
                    } else if let Some(id) = existing_context_id {
                        Some(id)
                    } else {
                        let ComparatorState {
                            contexts,
                            groups,
                            next_context_id,
                            ..
                        } = comp;
                        Some(remember_context_tab(
                            &mut contexts[source_index],
                            &mut groups[source_index],
                            next_context_id,
                            valid.to_string(),
                        ))
                    };
                    comp.split = Some(SplitView {
                        webview,
                        source_index,
                        context_id,
                        fullscreen: false,
                        private,
                    });
                }
                self.update_comparator_layout();
                self.sync_comparator_splitters();
                self.request_redraw();
                true
            }
            Err(error) => {
                // O estado anterior continua vivo. Reaplica a geometria para
                // garantir que nem um resize ocorrido durante o pump do build
                // deixe uma metade vazia.
                self.update_comparator_layout();
                self.sync_comparator_splitters();
                self.request_redraw();
                self.show_splash(format!("Não consegui abrir a fonte ao lado: {error}"), 4);
                false
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
        let _ = self.open_split_mode(
            source_index,
            "https://www.google.com/".to_string(),
            false,
            true,
            None,
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
    /// Pergunta escrita e enviada numa coluna: segue tambem para as outras,
    /// cada uma no seu fornecedor -- como o clique num link, que abre em
    /// todas. A coluna de origem nao e tocada: ja esta a enviar e mantem o
    /// contexto da conversa dela.
    fn ask_other_columns(&mut self, source_index: usize, text: String) {
        if self.surface != Surface::Comparator {
            return;
        }
        let Some(count) = self.comparator.as_ref().map(|comp| comp.views.len()) else {
            return;
        };
        let mut urls = Vec::new();
        for index in ask_targets(source_index, count) {
            match self.provider_query_url(index, &text) {
                Ok(url) => urls.push((index, url)),
                Err(error) => {
                    self.show_splash(error.to_string(), 3);
                    return;
                }
            }
        }
        debug_log(format_args!(
            "ask: coluna {source_index} -> {} coluna(s), {} chars",
            urls.len(),
            text.chars().count()
        ));
        if let Some(comp) = &self.comparator {
            for (index, url) in &urls {
                if let Some(view) = comp.views.get(*index) {
                    let _ = view.webview.load_url(url.as_str());
                }
            }
        }
        if let Some((_, url)) = urls.first() {
            self.record(HistoryKind::Ask, text, url.to_string());
        }
    }

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
                let _ = self.open_split_mode(source_index, url.to_string(), true, private, None);
            }
            // Painel privado: a pergunta abre como fonte privada, nunca na
            // coluna normal (cookies normais) e nunca no historico.
            PaletteRoute::OpenPrivateProvider { query } => {
                match self.provider_query_url(source_index, &query) {
                    Ok(url) => {
                        let _ =
                            self.open_split_mode(source_index, url.to_string(), false, true, None);
                    }
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

    /// ‹ e › da barra: o historico da PAGINA, como no Chrome -- na fonte aberta
    /// ao lado, na coluna expandida ou na pagina cheia. Com as tres colunas
    /// lado a lado nao ha uma pagina so: o ‹ faz o voltar do app.
    fn navigate_history(&mut self, step: HistoryStep) {
        let target = history_nav_target(
            self.surface,
            self.comparator
                .as_ref()
                .is_some_and(|comp| comp.split.is_some()),
            self.comparator.as_ref().and_then(|comp| comp.expanded),
            self.webview.is_some(),
        );
        let script = match step {
            HistoryStep::Back => "window.history.back();",
            HistoryStep::Forward => "window.history.forward();",
        };
        let webview = match target {
            HistoryNav::Split => self
                .comparator
                .as_ref()
                .and_then(|comp| comp.split.as_ref())
                .map(|split| &split.webview),
            HistoryNav::Column(index) => self
                .comparator
                .as_ref()
                .and_then(|comp| comp.views.get(index))
                .map(|view| &view.webview),
            HistoryNav::Page => self.webview.as_ref(),
            HistoryNav::App => None,
        };
        match (webview, step) {
            (Some(webview), _) => {
                let _ = webview.evaluate_script(script);
            }
            (None, HistoryStep::Back) => self.go_back(),
            (None, HistoryStep::Forward) => {}
        }
    }

    /// ‹ › de uma IA: o historico da pagina daquela coluna, so dela.
    fn navigate_column(&mut self, index: usize, step: HistoryStep) {
        let script = match step {
            HistoryStep::Back => "window.history.back();",
            HistoryStep::Forward => "window.history.forward();",
        };
        if let Some(view) = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(index))
        {
            let _ = view.webview.evaluate_script(script);
        }
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

        // A mensagem e a do meio da janela, como as outras dicas: o aviso
        // dentro da pagina ficava no fundo e so aparecia nas colunas.
        self.show_splash(auto_scroll_message(self.auto_scroll), 3);
        self.request_redraw();
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
                    AUX_POPUP_EX_STYLE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    AUX_POPUP_STYLE,
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
            let (x, y) = splash_origin(client.right, client.bottom, width, height);
            SetWindowPos(
                splash,
                std::ptr::null_mut(),
                origin.x + x,
                origin.y + y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(splash);
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
                    AUX_POPUP_EX_STYLE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    AUX_POPUP_STYLE,
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
                    (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize,
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
            show_popup_without_activation(toast);
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

        let result = themed_webview_builder()
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
            self.show_splash(auto_scroll_message(true), 4);
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
            self.show_splash(auto_scroll_message(true), 4);
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

    fn page_target_webview(&self, target: PageTarget) -> Option<&WebView> {
        let comp = self.comparator.as_ref()?;
        match target {
            PageTarget::Column(index) => comp.views.get(index).map(|view| &view.webview),
            PageTarget::Split => comp.split.as_ref().map(|split| &split.webview),
        }
    }

    fn reload_target(&mut self, target: PageTarget) {
        let reloaded = self
            .page_target_webview(target)
            .is_some_and(|webview| webview.reload().is_ok());
        if !reloaded {
            self.show_splash("Não há página ativa para recarregar.".to_string(), 3);
        }
    }

    fn print_target(&mut self, target: PageTarget) {
        let printed = self
            .page_target_webview(target)
            .is_some_and(|webview| webview.print().is_ok());
        if !printed {
            self.show_splash("Não há página ativa para imprimir.".to_string(), 3);
        }
    }

    fn open_devtools_target(&mut self, target: PageTarget) {
        let opened = if let Some(webview) = self.page_target_webview(target) {
            webview.open_devtools();
            true
        } else {
            false
        };
        if !opened {
            self.show_splash("Não há página ativa para inspecionar.".to_string(), 3);
        }
    }

    fn view_source_target(&mut self, target: PageTarget) {
        let current = self
            .page_target_webview(target)
            .and_then(|webview| webview.url().ok());
        let Some(current) = current else {
            self.show_splash(
                "Não há página ativa para ver o código-fonte.".to_string(),
                3,
            );
            return;
        };
        let current_origin = Url::parse(&current).ok().as_ref().and_then(local_origin_of);
        let source = format!("view-source:{current}");
        let loaded = is_view_source_target(&source, current_origin.as_deref())
            && self
                .page_target_webview(target)
                .is_some_and(|webview| webview.load_url(&source).is_ok());
        if !loaded {
            self.show_splash(
                "Não foi possível abrir o código-fonte desta página.".to_string(),
                3,
            );
        }
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

    /// O equivalente ao Ctrl+L do Chrome. Na Home foca a caixa principal;
    /// no comparador abre a palette flutuante da coluna ativa.
    fn focus_omnibox(&mut self) {
        if self.surface == Surface::Comparator {
            let index = self
                .comparator
                .as_ref()
                .and_then(|comp| {
                    comp.expanded
                        .or_else(|| (0..comp.views.len()).find(|index| !comp.minimized[*index]))
                })
                .unwrap_or(0);
            self.open_ai_palette(index);
            return;
        }

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
        false
    }

    /// O chrome permanece visível também quando uma IA ocupa toda a área de
    /// conteúdo. "Expandir" não significa tomar o monitor inteiro.
    fn bar_visible(&self) -> bool {
        self.comparator.is_some()
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

    /// Home nativo da barra. O desenho da barra continua existindo por baixo,
    /// mas o clique pertence a uma janela Win32 real, acima de qualquer filho
    /// WebView2. Assim o controlo nao depende do foco nem da entrega de eventos
    /// do winit para voltar à Home.
    fn sync_home_button(&mut self) {
        let wanted = self.surface == Surface::Comparator && !self.is_fullscreen_column();
        if !wanted {
            if let Some(button) = self.home_button.take() {
                unsafe {
                    DestroyWindow(button);
                }
            }
            return;
        }

        let (Some(window), Some(layout)) = (&self.window, self.bar_layout()) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let rect = layout.home;
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        if self.home_button.is_none() {
            unsafe {
                let created = CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("NeuralIA.Home"),
                    WS_CHILD | WS_VISIBLE,
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    rect.width.round() as i32,
                    rect.height.round() as i32,
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
                    Some(home_button_subclass),
                    HOME_BUTTON_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let width = rect.width.round() as i32;
                let height = rect.height.round() as i32;
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, height, height);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.home_button = Some(created);
            }
        }

        if let Some(button) = self.home_button {
            unsafe {
                SetWindowPos(
                    button,
                    std::ptr::null_mut(),
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    rect.width.round() as i32,
                    rect.height.round() as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                ShowWindow(button, SW_SHOW);
                InvalidateRect(button, std::ptr::null(), 1);
            }
        }
    }

    /// Controles de janela reais acima dos filhos WebView2. Se a troca de
    /// decoracao produzir outro HWND, o controlo e destruido e recriado no
    /// novo pai; nao se usa SetParent neste overlay.
    fn sync_caption_buttons(&mut self) {
        let wanted = caption_buttons_wanted(self.surface, self.bar_visible());
        if !wanted {
            if let Some(buttons) = self.caption_buttons.take() {
                unsafe {
                    DestroyWindow(buttons);
                }
            }
            return;
        }

        let Some(window) = &self.window else {
            return;
        };
        // Na Home nao ha barra do comparador, mas os botoes da janela ficam no
        // mesmo sitio: a geometria deles so depende da largura.
        let layout = match self.bar_layout() {
            Some(layout) if self.surface == Surface::Comparator => layout,
            _ => BarLayout::with_rows(
                window.inner_size().width as f64,
                window.scale_factor(),
                true,
                BarColumns::even(COMPARATOR_COLUMNS),
                std::array::from_fn(|_| plan_tab_row(&[], &[])),
            ),
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let left = layout.window_minimize.x;
        let right = layout.window_close.x + layout.window_close.width;
        let width = (right - left).max(1.0);
        let height = layout.window_close.height.max(1.0);

        if let Some(buttons) = self.caption_buttons
            && unsafe { GetParent(buttons) } != owner
        {
            unsafe {
                DestroyWindow(buttons);
            }
            self.caption_buttons = None;
        }

        if self.caption_buttons.is_none() {
            unsafe {
                let created = CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("NeuralIA.CaptionControls"),
                    WS_CHILD | WS_VISIBLE,
                    left.round() as i32,
                    0,
                    width.round() as i32,
                    height.round() as i32,
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
                    Some(caption_buttons_subclass),
                    CAPTION_BUTTONS_SUBCLASS_ID,
                    0,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                self.caption_buttons = Some(created);
            }
        }

        if let Some(buttons) = self.caption_buttons {
            unsafe {
                SetWindowPos(
                    buttons,
                    std::ptr::null_mut(),
                    left.round() as i32,
                    0,
                    width.round() as i32,
                    height.round() as i32,
                    SWP_NOACTIVATE,
                );
                ShowWindow(buttons, SW_SHOW);
                InvalidateRect(buttons, std::ptr::null(), 1);
            }
        }
    }

    /// Cria/mostra/esconde o botao flutuante de saida. Existe apenas enquanto
    /// houver uma coluna em ecra completo -- e a unica saida sempre visivel,
    /// porque a barra de titulo desapareceu e a barra da app auto-esconde-se.
    fn sync_exit_button(&mut self) {
        // Em fullscreen e o controlo nativo permanente de saida. Nao depende
        // de hover nem de redimensionar o WebView.
        let wanted = self.surface == Surface::Comparator && self.is_fullscreen_column();

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
                    AUX_POPUP_EX_STYLE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    AUX_POPUP_STYLE,
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
            show_popup_without_activation(button);
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
            let logical_w = (size.width as f64 / scale
                - self
                    .comparator
                    .as_ref()
                    .map_or(0.0, |comp| comp.panel_width))
            .max(1.0);
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
                        AUX_POPUP_EX_STYLE,
                        windows_sys::w!("STATIC"),
                        windows_sys::w!(""),
                        AUX_POPUP_STYLE,
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
                show_popup_without_activation(hwnd);
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

    fn comparator_bar_hit(&self) -> Option<BarHit> {
        if let Some(controls) = self.right_controls()
            && let Some(hit) = right_controls_hit(controls, self.cursor.0, self.cursor.1)
        {
            return Some(hit);
        }
        self.bar_layout()
            .and_then(|layout| layout.hit(self.cursor.0, self.cursor.1))
    }

    /// "Ir" da Home sob o rato: degradê e mao, para se ver que esta vivo e
    /// responde ao clique. Fora da Home volta tudo ao normal.
    fn update_home_go_hover(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let hovered = home_go_hovered(
            self.surface,
            (size.width as f64, size.height as f64),
            window.scale_factor(),
            self.cursor,
        );
        if hovered == self.home_go_hover {
            return;
        }
        self.home_go_hover = hovered;
        window.set_cursor(if hovered {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        });
        self.request_redraw();
    }

    fn update_bar_hover(&mut self) {
        let next = self.comparator_bar_hit();
        if next != self.bar_hover {
            self.bar_hover = next;
            self.request_redraw();
            if let Some(owner) = self.window.as_ref().and_then(window_hwnd) {
                let text = next.and_then(|hit| self.bar_tooltip_text(hit, owner));
                hover_tooltip(owner, text.as_deref().unwrap_or(""));
            }
        }
    }

    /// Os icones da barra: o servico abre no painel ao lado; de novo, fecha.
    fn open_service_panel(&mut self, service: Service) {
        let already = self
            .service_panel
            .as_ref()
            .is_some_and(|(open, _)| *open == service);
        self.close_service_panel();
        if already {
            return;
        }
        // Um painel de cada vez.
        self.close_side_panel();
        let Some(window) = &self.window else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let size = window.inner_size();
        let top = if self.surface == Surface::Comparator {
            COMPARATOR_CHROME_HEIGHT
        } else {
            0.0
        };
        let (x, y, width, height) =
            service_panel_bounds(size.width as f64 / scale, size.height as f64 / scale, top);
        let built = themed_webview_builder()
            .with_url(service.url())
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(x, y).into(),
                size: LogicalSize::new(width, height).into(),
            })
            .with_navigation_handler(|target| service_panel_allows_navigation(&target))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            // Caminho A do WebRTC: camera e microfone pelo aviso do WebView2.
            .with_permission_handler(|kind| web_media_permission(kind, true))
            .build_as_child(window);
        match built {
            Ok(panel) => {
                let _ = panel.focus();
                debug_log(format_args!("service panel: {service:?}"));
                self.service_panel = Some((service, panel));
                self.fit_comparator_to_panel();
            }
            Err(error) => {
                self.show_splash(
                    format!("Não foi possível abrir {}: {error}", service.label()),
                    3,
                );
            }
        }
    }

    fn close_service_panel(&mut self) {
        if self.service_panel.take().is_some() {
            debug_log(format_args!("service panel: fechado"));
            self.fit_comparator_to_panel();
        }
    }

    fn position_service_panel(&self) {
        let (Some((_, panel)), Some(window)) = (&self.service_panel, &self.window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let size = window.inner_size();
        let top = if self.surface == Surface::Comparator {
            COMPARATOR_CHROME_HEIGHT
        } else {
            0.0
        };
        let (x, y, width, height) =
            service_panel_bounds(size.width as f64 / scale, size.height as f64 / scale, top);
        let _ = panel.set_bounds(wry::Rect {
            position: LogicalPosition::new(x, y).into(),
            size: LogicalSize::new(width, height).into(),
        });
    }

    /// O envelope da barra: liga e desliga os avisos do Gmail, e guarda.
    fn toggle_gmail_notifications(&mut self) {
        let on = !GMAIL_NOTIFICATIONS.load(Ordering::Acquire);
        GMAIL_NOTIFICATIONS.store(on, Ordering::Release);
        if let Err(error) = save_gmail_setting(&self.config.data_dir.join("gmail"), on) {
            self.show_native_error(format!("Não foi possível guardar a escolha: {error}"));
        }
        if on {
            self.schedule_gmail_probe(1);
        } else {
            // Desligar e mesmo desligar: sem WebView escondida a ler o Gmail.
            self.gmail_monitor = None;
            if let Some(toast) = self.gmail_toast {
                unsafe {
                    ShowWindow(toast, SW_HIDE);
                }
            }
        }
        self.show_splash(
            if on {
                "Avisos do Gmail ligados.".to_string()
            } else {
                "Avisos do Gmail desligados.".to_string()
            },
            2,
        );
        self.request_redraw();
    }

    /// Resposta ao "Abrir?" do aviso do Gmail.
    fn answer_gmail(&mut self, open: bool) {
        if let Some(toast) = self.gmail_toast {
            unsafe {
                ShowWindow(toast, SW_HIDE);
            }
        }
        if open {
            self.open_service_panel(Service::Gmail);
        }
    }

    /// Largura que o painel aberto ocupa a direita do comparador (0 sem painel).
    fn open_panel_width(&self) -> f64 {
        let Some(window) = &self.window else {
            return 0.0;
        };
        if self.surface != Surface::Comparator {
            return 0.0;
        }
        let scale = window.scale_factor().max(1.0);
        let size = window.inner_size();
        let (width, height) = (size.width as f64 / scale, size.height as f64 / scale);
        if self.service_panel.is_some() {
            service_panel_bounds(width, height, COMPARATOR_CHROME_HEIGHT).2
        } else if self.side_panel.is_some() {
            side_panel_bounds(width, height, COMPARATOR_CHROME_HEIGHT).2
        } else {
            0.0
        }
    }

    /// O comparador encolhe para o lado do painel, como no Chrome. Antes o
    /// painel ficava POR CIMA das colunas, e os divisores e a paleta (popups)
    /// apareciam por cima dele.
    fn fit_comparator_to_panel(&mut self) {
        let width = self.open_panel_width();
        let Some(comp) = &mut self.comparator else {
            return;
        };
        if (comp.panel_width - width).abs() < 0.5 {
            return;
        }
        comp.panel_width = width;
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.position_palette();
        self.request_redraw();
    }

    /// Ctrl+H: abre o historico inteligente ao lado; de novo (ou Esc), fecha.
    fn toggle_side_panel(&mut self) {
        if self.side_panel.is_some() {
            self.close_side_panel();
        } else {
            self.open_side_panel();
        }
    }

    fn side_panel_rect(&self) -> Option<wry::Rect> {
        let window = self.window.as_ref()?;
        let scale = window.scale_factor().max(1.0);
        let size = window.inner_size();
        let top = if self.surface == Surface::Comparator {
            COMPARATOR_CHROME_HEIGHT
        } else {
            0.0
        };
        let (x, y, width, height) =
            side_panel_bounds(size.width as f64 / scale, size.height as f64 / scale, top);
        Some(wry::Rect {
            position: LogicalPosition::new(x, y).into(),
            size: LogicalSize::new(width, height).into(),
        })
    }

    fn open_side_panel(&mut self) {
        // Um painel de cada vez.
        self.close_service_panel();
        let Some(bounds) = self.side_panel_rect() else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let proxy = self.proxy.clone();
        // Criado por ultimo, fica por cima das outras WebViews.
        let built = themed_webview_builder()
            .with_html(panel_html(&Theme::system()))
            .with_bounds(bounds)
            .with_ipc_handler(move |request| {
                if let Some(message) = parse_panel_message(request.body()) {
                    let _ = proxy.send_event(UserEvent::Panel(message));
                }
            })
            .with_navigation_handler(|target| panel_allows_navigation(&target))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .build_as_child(window);
        match built {
            Ok(panel) => {
                let _ = panel.focus();
                self.side_panel = Some(panel);
                self.fit_comparator_to_panel();
                debug_log(format_args!(
                    "side panel: aberto surface={:?}",
                    self.surface
                ));
            }
            Err(error) => {
                self.show_splash(format!("Não foi possível abrir o painel: {error}"), 3);
            }
        }
    }

    fn close_side_panel(&mut self) {
        if self.side_panel.take().is_none() {
            return;
        }
        self.panel_suggestion_query = None;
        debug_log(format_args!("side panel: fechado"));
        self.fit_comparator_to_panel();
        // Largar a WebView nao devolve o teclado a ninguem.
        if self.surface == Surface::Home {
            self.focus_omnibox();
        }
    }

    fn position_side_panel(&self) {
        if let (Some(panel), Some(bounds)) = (&self.side_panel, self.side_panel_rect()) {
            let _ = panel.set_bounds(bounds);
        }
    }

    fn panel_eval(&self, script: &str) {
        if let Some(panel) = &self.side_panel {
            let _ = panel.evaluate_script(script);
        }
    }

    fn handle_panel_message(&mut self, message: PanelMessage) {
        match message {
            PanelMessage::Ready => {
                if let Some(result) = self.history.recent(PANEL_RECENT_LIMIT) {
                    self.panel_show_history(result);
                }
                // Sugestoes: a pergunta da pesquisa em curso contra a memoria
                // local. Nada sai do computador.
                if let Some(question) = self
                    .current_research
                    .as_ref()
                    .map(|session| session.question.trim().to_string())
                    .filter(|question| !question.is_empty())
                {
                    self.panel_suggestion_query = Some(question.clone());
                    self.memory.query(question);
                }
            }
            PanelMessage::Search(query) => self.memory.query(query),
            PanelMessage::Open(input) => {
                self.close_side_panel();
                self.handle_input(input);
            }
            PanelMessage::Close => self.close_side_panel(),
        }
    }

    fn panel_show_history(&self, result: Result<Vec<HistoryEntry>, String>) {
        let (items, empty) = match result {
            Ok(entries) => (
                history_panel_items(&entries),
                "Nenhuma pesquisa gravada ainda.".to_string(),
            ),
            Err(error) => (
                Vec::new(),
                format!("Não foi possível ler o histórico: {error}"),
            ),
        };
        self.panel_eval(&panel_render_script("recentes", "Recentes", &empty, &items));
    }

    fn panel_show_memory(&mut self, query: &str, result: Result<Vec<MemoryHit>, String>) {
        if self.panel_suggestion_query.as_deref() == Some(query) {
            self.panel_suggestion_query = None;
            if let Ok(hits) = result {
                let items = suggestion_panel_items(&hits, PANEL_SUGGESTION_LIMIT);
                self.panel_eval(&panel_render_script(
                    "sugestoes",
                    "Sugestões para esta pesquisa",
                    "Nenhum site relacionado na sua memória ainda.",
                    &items,
                ));
            }
            return;
        }
        let script = match result {
            Ok(hits) => panel_render_script(
                "busca",
                &format!("Busca: {query}"),
                "Nada encontrado na memória local.",
                &memory_panel_items(&hits),
            ),
            Err(error) => panel_render_script(
                "busca",
                "Busca",
                &format!("Não foi possível consultar a memória: {error}"),
                &[],
            ),
        };
        self.panel_eval(&script);
    }

    /// Tema novo (mudou no Windows ou foi escolhido): barra, botoes nativos,
    /// popups auxiliares e paginas. Antes, na mudanca do Windows, so a barra
    /// se redesenhava e os botoes nativos ficavam com as cores velhas.
    fn refresh_theme(&mut self) {
        use windows_sys::Win32::Graphics::Gdi::{
            RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow,
        };
        use wry::WebViewExtWindows;
        Theme::invalidate();
        self.needs_clear = true;
        self.request_redraw();
        if let Some(owner) = self.window.as_ref().and_then(window_hwnd) {
            unsafe {
                RedrawWindow(
                    owner,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                );
            }
        }
        // Os popups owned nao sao filhos: o RDW_ALLCHILDREN nao os alcanca.
        for hwnd in self.splitters.iter().flatten() {
            unsafe {
                InvalidateRect(*hwnd, std::ptr::null(), 1);
            }
        }
        self.panel_eval(&format!(
            "window.__neuraliaPanel && window.__neuraliaPanel.theme({});",
            panel_theme_vars(&Theme::system())
        ));
        let theme = ThemeChoice::current().webview_theme();
        if let Some(comp) = &self.comparator {
            for view in &comp.views {
                let _ = view.webview.set_theme(theme);
            }
            if let Some(split) = &comp.split {
                let _ = split.webview.set_theme(theme);
            }
        }
    }

    fn choose_theme(&mut self, choice: ThemeChoice) {
        choice.apply();
        if let Err(error) = choice.save(&self.config.data_dir.join("theme")) {
            self.show_native_error(format!("Não foi possível guardar o tema: {error}"));
        }
        self.refresh_theme();
        self.show_splash(format!("{} ativado.", choice.label()), 2);
    }

    /// A dica do alvo `hit`, com o nome da IA, o endereco da aba ou o estado
    /// do grupo que o clique vai usar.
    fn bar_tooltip_text(&self, hit: BarHit, owner: HWND) -> Option<String> {
        let comp = self.comparator.as_ref();
        let column = match hit {
            BarHit::Column(index)
            | BarHit::AddTab(index)
            | BarHit::ColumnBack(index)
            | BarHit::ColumnForward(index) => Some(index),
            BarHit::ContextTab { source_index, .. } | BarHit::ContextGroup { source_index, .. } => {
                Some(source_index)
            }
            _ => None,
        };
        let provider = column
            .and_then(|index| comp.and_then(|comp| comp.views.get(index)))
            .map_or("IA", |view| view.name);
        let tab_url = match hit {
            BarHit::ContextTab {
                source_index,
                context_index,
            } => comp
                .and_then(|comp| comp.contexts.get(source_index))
                .and_then(|tabs| tabs.get(context_index))
                .map(|tab| tab.url.as_str()),
            _ => None,
        };
        let group = match hit {
            BarHit::ContextGroup {
                source_index,
                group_index,
            } => comp
                .and_then(|comp| comp.groups.get(source_index))
                .and_then(|groups| groups.get(group_index))
                .map(|group| (group.name.as_str(), group.collapsed)),
            _ => None,
        };
        let maximized = unsafe { IsZoomed(owner) != 0 };
        bar_tooltip_label(hit, provider, maximized, tab_url, group)
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
        let logical_w = (size.width as f64 / scale
            - self
                .comparator
                .as_ref()
                .map_or(0.0, |comp| comp.panel_width))
        .max(1.0);
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

    fn context_tab_identity(
        &self,
        source_index: usize,
        context_index: usize,
    ) -> Option<(u64, String)> {
        self.comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .and_then(|tabs| tabs.get(context_index))
            .map(|tab| (tab.id, tab.url.clone()))
    }

    fn open_context_tab(&mut self, source_index: usize, context_index: usize) -> bool {
        let active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .map(|split| (split.source_index, split.context_id));
        self.context_tab_identity(source_index, context_index)
            .is_some_and(|(context_id, url)| {
                context_tab_click_is_noop(active, source_index, context_id)
                    || self.open_split_mode(source_index, url, false, false, Some(context_id))
            })
    }

    fn open_context_tab_fullscreen(&mut self, source_index: usize, context_index: usize) {
        if self.open_context_tab(source_index, context_index)
            && !self
                .comparator
                .as_ref()
                .and_then(|comp| comp.split.as_ref())
                .is_some_and(|split| split.fullscreen)
        {
            self.toggle_split_fullscreen();
        }
    }

    fn close_context_tab(&mut self, source_index: usize, context_index: usize) {
        let Some((context_id, _url)) = self.context_tab_identity(source_index, context_index)
        else {
            return;
        };
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| {
                split.source_index == source_index && split.context_id == Some(context_id)
            });
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
            let ComparatorState {
                contexts,
                groups,
                next_group_id,
                ..
            } = comp;
            let _ = regroup_context_tab(
                &mut contexts[source_index],
                &mut groups[source_index],
                next_group_id,
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
        let valid_context = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .is_some_and(|tabs| context_index < tabs.len());
        if !valid_context {
            return;
        }
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref().map(|split| (comp, split)))
            .is_some_and(|(comp, split)| {
                split.source_index == source_index
                    && active_context_removed_by_scope(
                        &comp.contexts[source_index],
                        context_index,
                        split.context_id,
                        true,
                    )
            });
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator {
            let _ = close_other_context_tabs_in_scope(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
        }
        self.request_redraw();
    }

    fn close_all_context_tabs(&mut self, source_index: usize, context_index: usize) {
        let valid_context = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .is_some_and(|tabs| context_index < tabs.len());
        if !valid_context {
            return;
        }
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref().map(|split| (comp, split)))
            .is_some_and(|(comp, split)| {
                split.source_index == source_index
                    && active_context_removed_by_scope(
                        &comp.contexts[source_index],
                        context_index,
                        split.context_id,
                        false,
                    )
            });
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator {
            let _ = close_context_tab_scope(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
        }
        self.request_redraw();
    }

    fn joinable_context_groups(
        groups: &[ContextGroup],
        current_group: Option<u64>,
    ) -> Vec<(usize, String)> {
        groups
            .iter()
            .enumerate()
            .filter(|(_, group)| Some(group.id) != current_group)
            .map(|(index, group)| (index, group.name.clone()))
            .collect()
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
                let current_group = comp.contexts[source_index]
                    .get(context_index)
                    .and_then(|tab| tab.group);
                (
                    Self::joinable_context_groups(&comp.groups[source_index], current_group),
                    current_group.is_some(),
                )
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
            let close_others = wide_null(if in_group {
                "Fechar outras abas deste grupo"
            } else {
                "Fechar outras abas sem grupo"
            });
            let close_all = wide_null(if in_group {
                "Fechar todas deste grupo"
            } else {
                "Fechar todas as abas sem grupo"
            });
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
                .map(|(_, name)| wide_null(&format!("Juntar ao grupo \u{201C}{name}\u{201D}")))
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
            TAB_MENU_OPEN => {
                self.open_context_tab(source_index, context_index);
            }
            TAB_MENU_FULLSCREEN => self.open_context_tab_fullscreen(source_index, context_index),
            TAB_MENU_CLOSE => self.close_context_tab(source_index, context_index),
            TAB_MENU_CLOSE_OTHERS => self.close_other_context_tabs(source_index, context_index),
            TAB_MENU_CLOSE_ALL => self.close_all_context_tabs(source_index, context_index),
            TAB_MENU_NEW_GROUP => self.group_context_tab(source_index, context_index),
            TAB_MENU_UNGROUP => self.ungroup_context_tab(source_index, context_index),
            other if other >= TAB_MENU_GROUP_BASE => {
                let menu_index = other - TAB_MENU_GROUP_BASE;
                if let Some((group_index, _)) = existing_groups.get(menu_index) {
                    self.join_context_tab_group(source_index, context_index, *group_index);
                }
            }
            _ => {}
        }
    }

    fn click_comparator(&mut self) {
        let hit = self.comparator_bar_hit();
        // O clique pode trocar a superficie; a dica nao fica a flutuar sobre
        // a tela nova.
        hover_tooltip(std::ptr::null_mut(), "");
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
            Some(BarHit::Service(service)) => self.open_service_panel(service),
            Some(BarHit::GmailToggle) => self.toggle_gmail_notifications(),
            Some(BarHit::SplitClose) => self.close_split(),
            Some(BarHit::SplitExpand) => self.toggle_split_fullscreen(),
            Some(BarHit::Home) => self.show_home(),
            Some(BarHit::Back) => self.navigate_history(HistoryStep::Back),
            Some(BarHit::Forward) => self.navigate_history(HistoryStep::Forward),
            Some(BarHit::ColumnBack(index)) => self.navigate_column(index, HistoryStep::Back),
            Some(BarHit::ColumnForward(index)) => self.navigate_column(index, HistoryStep::Forward),
            Some(BarHit::Column(index)) => self.expand_comparator(index),
            Some(BarHit::AddTab(index)) => self.open_ai_palette(index),
            Some(BarHit::ContextTab {
                source_index,
                context_index,
            }) => {
                self.open_context_tab(source_index, context_index);
            }
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

        // Sem a barra do Windows, a faixa de cima arrasta a janela -- como a
        // barra do comparador.
        if home_drag_strip(y, window.scale_factor()) && !layout.go.contains(x, y) {
            let _ = window.drag_window();
            return;
        }

        if layout.go.contains(x, y) {
            debug_log(format_args!("click_home: botao Ir"));
            self.submit_current();
        }
    }
}

/// Atalhos com Ctrl quando o teclado esta na propria janela (depois de um
/// clique na barra): os mesmos que o mapa de teclas das paginas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainShortcut {
    AutoScroll,
    Reload,
    History,
    NewTab,
}

fn main_window_shortcut(
    key: &Key,
    modifiers: winit::keyboard::ModifiersState,
) -> Option<MainShortcut> {
    if !modifiers.control_key() || modifiers.alt_key() {
        return None;
    }
    let Key::Character(text) = key else {
        return None;
    };
    match (text.to_lowercase().as_str(), modifiers.shift_key()) {
        ("r", false) => Some(MainShortcut::AutoScroll),
        ("r", true) => Some(MainShortcut::Reload),
        ("h", false) => Some(MainShortcut::History),
        ("n", false) => Some(MainShortcut::NewTab),
        _ => None,
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
    /// O `extract` que a política só deixa seguir com um sim humano (a
    /// página está numa origem que a sessão não aprovou).
    ConfirmExtract {
        security: AgentSecurityAction,
        reason: String,
    },
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
        // O `extract` escreve a página na memória semântica: passa pelo gate
        // como os outros. Saltava-o, e depois de um clique que levasse a outra
        // origem a página dessa origem entrava na memória sem diálogo e sem
        // entrada na auditoria (SPEC-0105 §4).
        BrowserAgentCommand::Extract => {
            let security = app_agent_security_action(
                &AgentAction::Extract {
                    target: None,
                    schema: "page-text".into(),
                },
                page,
            );
            let decision = policy.evaluate(&security);
            if decision.allowed {
                return AgentStepDecision::Extract;
            }
            if decision.risk == ActionRisk::Restricted {
                return AgentStepDecision::Stop(AgentTermination::RestrictedAction);
            }
            if !decision.requires_confirmation {
                policy.record_user_confirmation(&security, false);
                return AgentStepDecision::Stop(AgentTermination::UserRejected);
            }
            return AgentStepDecision::ConfirmExtract {
                security,
                reason: decision.reason,
            };
        }
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
/// Reabrir uma aba de contexto ja gravada nao e uma fonte nova: a fonte
/// entrou na sessao e na memoria quando a aba nasceu. Sem isto, cada clique
/// A, B, A, B acrescentava outra copia a sessao persistida.
fn split_open_records_source(existing_context_id: Option<u64>, private: bool) -> bool {
    existing_context_id.is_none() && !private
}

/// Clicar na aba de contexto que o split ja mostra nao reconstroi o WebView:
/// reconstruir voltava a URL original da aba e perdia o que o utilizador
/// escreveu ou navegou.
fn context_tab_click_is_noop(
    active: Option<(usize, Option<u64>)>,
    source_index: usize,
    context_id: u64,
) -> bool {
    active == Some((source_index, Some(context_id)))
}

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
    /// `tema:claro`, `tema:escuro`, `tema:sistema` (None: palavra desconhecida).
    Theme(Option<ThemeChoice>),
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

    if let Some(word) = trimmed
        .strip_prefix("tema:")
        .or_else(|| trimmed.strip_prefix("theme:"))
    {
        return InputRoute::Theme(ThemeChoice::parse(word));
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

/// Como o agente dá papel e nome a um elemento, num só texto que entra no
/// `AGENT_OBSERVER_SCRIPT` e no guard do `agent_action_script`.
///
/// Eram duas fórmulas: o observador dizia `textbox`/`button`/`select` e o
/// guard recalculava `el.type` (`text`, `submit`, `select-one`), sem o
/// placeholder no nome. Nos controlos mais comuns o guard desistia em silêncio
/// e o passo ficava no trace como feito. Com uma só definição não há o que
/// divergir.
macro_rules! agent_element_identity_js {
    () => {
        r#"
  // `slice` conta unidades UTF-16 e pode partir um emoji: o surrogate que
  // fica sozinho vira `\udXXX` no JSON, que o serde_json recusa, e a
  // observacao inteira sumia. Qualquer surrogate sem par sai.
  function clean(value, limit) {
    return String(value || '').replace(/[\t\r\n]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, limit)
      .replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, '');
  }

  function fieldRole(el) {
    const tag = (el.tagName || '').toLowerCase();
    const type = (el.type || '').toLowerCase();
    const autocomplete = (el.autocomplete || '').toLowerCase();
    const role = (el.getAttribute('role') || '').toLowerCase();
    const name = (el.name || '').toLowerCase();
    // Antes de tudo: o que o clique FAZ. Um submit de formulario e `submit`
    // seja qual for o role ou o nome que a pagina lhe der; e o unico papel
    // que faz o `app_agent_security_action` pedir confirmacao sem depender de
    // o rotulo estar na lista de palavras.
    if ((tag === 'button' && type === 'submit' && el.form) ||
        (tag === 'input' && (type === 'submit' || type === 'image'))) return 'submit';
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

  function elementName(el) {
    return clean(el.getAttribute('aria-label') || el.name || el.innerText || el.textContent || el.placeholder, 96);
  }
"#
    };
}

fn agent_action_script(action: &AgentAction) -> Result<String, String> {
    fn guard(target: &AgentElement) -> String {
        let id = js_percent(&target.id);
        let name = js_percent(&target.name);
        let role = js_percent(&target.role);
        format!(
            "{identity}const id=decodeURIComponent('{id}');const expectedName=decodeURIComponent('{name}');             const expectedRole=decodeURIComponent('{role}');             const el=document.querySelector('[data-neuralia-agent-id=\"'+id+'\"]');             if(!el)return;             if(expectedName && elementName(el)!==expectedName)return;             if(expectedRole && fieldRole(el)!==expectedRole)return;",
            identity = agent_element_identity_js!()
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
            // Sem a barra do Windows em lado nenhum: a Home e o comparador desenham
            // os seus proprios botoes da janela (pedido do dono).
            .with_decorations(false)
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
                self.sync_caption_buttons();
                self.request_redraw();

                // Abertura: a consulta padrao ja entra na omnibox e vai direto
                // para a tela de resultados, sem esperar Enter do utilizador.
                let startup = startup_input();
                if !startup.is_empty() {
                    self.set_omnibox_text(&startup);
                    debug_log(format_args!(
                        "startup: SubmitText ({} chars)",
                        startup.chars().count()
                    ));
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
        // set_decorations pode trocar/reparentear o HWND depois do callback que
        // pediu a mudança. Este é o primeiro ponto garantido depois de cada lote
        // de eventos, já fora do pump aninhado do WebView2. Reinstalar a
        // subclass aqui é idempotente e garante que Home, atalhos e o probe
        // continuem chegando à janela REAL também na segunda abertura.
        self.ensure_window_subclass();

        if lifecycle_probe_enabled()
            && self.surface == Surface::Comparator
            && self.comparator.is_some()
            && !LIFECYCLE_COMPARATOR_READY.load(Ordering::Acquire)
        {
            self.needs_clear = true;
            self.update_comparator_layout();
            self.sync_comparator_splitters();
            self.sync_exit_button();
            self.sync_home_button();
            self.sync_caption_buttons();
            self.show_omnibox_passive(true);
            self.position_omnibox();
            LIFECYCLE_COMPARATOR_READY.store(true, Ordering::Release);
            self.request_redraw();
        }

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
            UserEvent::ReloadTarget(target) => self.reload_target(target),
            UserEvent::PrintPage => self.print_page(),
            UserEvent::PrintTarget(target) => self.print_target(target),
            UserEvent::FocusOmnibox => self.focus_omnibox(),
            UserEvent::ToggleColumnFullscreen => self.toggle_column_fullscreen(),
            UserEvent::OpenDevTools => self.open_devtools(),
            UserEvent::OpenDevToolsTarget(target) => self.open_devtools_target(target),
            UserEvent::ViewSource => self.view_source(),
            UserEvent::ViewSourceTarget(target) => self.view_source_target(target),
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
            UserEvent::ShowHistory => self.toggle_side_panel(),
            UserEvent::ThemeChosen(choice) => self.choose_theme(choice),
            UserEvent::Panel(message) => self.handle_panel_message(message),
            UserEvent::GmailAnswer(open) => self.answer_gmail(open),
            UserEvent::ClearHistory => {
                if !self.confirm_clear_history() {
                    return;
                }
                self.memory.clear(&mut self.current_research);
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
            UserEvent::HistoryLoaded(result) => {
                if self.side_panel.is_some() {
                    self.panel_show_history(result);
                } else {
                    self.show_history_entries(result);
                }
            }
            UserEvent::HistoryWriteFailed(error) => {
                self.show_splash(format!("Histórico não foi gravado: {error}"), 4);
            }
            UserEvent::MemoryQueryReady { query, result } => {
                if self.side_panel.is_some() {
                    self.panel_show_memory(&query, result);
                } else {
                    self.show_memory_results(&query, result);
                }
            }
            UserEvent::MemoryCleared(result) => {
                if let Err(error) = result {
                    self.show_splash(format!("Memória: {error}"), 4);
                } else {
                    self.status = Some("Histórico e memória semântica apagados.".to_string());
                    self.request_redraw();
                }
            }
            UserEvent::AskEverywhere { source_index, text } => {
                self.ask_other_columns(source_index, text)
            }
            UserEvent::SearchSelection(text) => self.search_selection(&text),
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
            UserEvent::SubmitText(input) => {
                if surface_accepts_omnibox_submit(self.surface) {
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
                let _ = self.open_split(source_index, url, false);
            }
            UserEvent::OpenPrivateSplit { source_index, url } => {
                let _ = self.open_split_mode(source_index, url, false, true, None);
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
                    self.sync_home_button();
                    self.sync_caption_buttons();
                    self.show_omnibox_passive(true);
                    self.position_omnibox();
                    self.request_redraw();
                }
            }
            UserEvent::RestoreHomeDecorations => {
                debug_log(format_args!(
                    "RestoreHomeDecorations: surface={:?}",
                    self.surface
                ));
                if self.surface == Surface::Home {
                    // O controller já foi descartado: esconde-se qualquer host
                    // WRY ainda preso ao HWND (a regressão em que, do segundo
                    // ciclo em diante, três WRY_WEBVIEW ficavam visíveis sobre a
                    // Home apesar de os WebView Rust já terem sido dropados).
                    // A Home fica sem a moldura do Windows, como o comparador;
                    // aqui so se limpam os hosts WRY orfaos e se mostram os
                    // botoes da janela do proprio app.
                    if let Some(window) = &self.window {
                        hide_orphaned_wry_hosts(window);
                    }
                    self.ensure_window_subclass();
                    self.sync_caption_buttons();
                    self.needs_clear = true;
                    self.position_omnibox();
                    self.request_redraw();
                    if lifecycle_probe_enabled() {
                        LIFECYCLE_HOME_READY.store(true, Ordering::Release);
                    }
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
                            draw_home(window, self.status.as_deref(), self.home_go_hover);
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
            WindowEvent::Resized(size) => {
                debug_log(format_args!(
                    "resized {}x{} surface={:?}",
                    size.width, size.height, self.surface
                ));
                self.fit_comparator_to_panel();
                self.position_side_panel();
                self.position_service_panel();
                if self.surface == Surface::Home {
                    self.sync_caption_buttons();
                }
                match self.surface {
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
                        self.sync_home_button();
                        self.sync_caption_buttons();
                        self.position_omnibox();
                        self.position_palette();
                        self.request_redraw();
                    }
                    _ => {}
                }
            }
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
                if self.surface == Surface::Comparator && self.bar_visible() {
                    self.update_bar_hover();
                }
                self.update_home_go_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = (-1.0, -1.0);
                if self.surface == Surface::Comparator {
                    self.update_bar_hover();
                }
                self.update_home_go_hover();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Focused(focused) => self.on_focus_changed(focused),
            WindowEvent::Occluded(occluded) => self.on_occluded_changed(occluded),
            // O tema do sistema mudou: o cache de 1 s tem de cair agora, e o
            // fundo inteiro e repintado porque ate a cor da pagina mudou.
            WindowEvent::ThemeChanged(_) => self.refresh_theme(),
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
                if let Some(shortcut) = main_window_shortcut(&event.logical_key, self.modifiers) {
                    match shortcut {
                        MainShortcut::AutoScroll => self.toggle_auto_scroll(),
                        MainShortcut::Reload => self.reload_page(),
                        MainShortcut::History => self.toggle_side_panel(),
                        MainShortcut::NewTab => self.new_tab(0),
                    }
                    return;
                }
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

/// Colunas que recebem a pergunta enviada em `source`: todas as outras.
fn ask_targets(source: usize, count: usize) -> Vec<usize> {
    (0..count).filter(|index| *index != source).collect()
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
        // Colunas, Split normal, Web externa, Reader e PDF. O Split privado
        // recusa antes de chegar aqui (`split_ipc_event_impl`).
        IpcAction::Search { text } => UserEvent::SearchSelection(text),
        _ => return None,
    })
}

/// O que um WebView de Web externa pode pedir. Fora do closure do builder
/// para se poder exercitar sem janela.
fn external_ipc_event(action: IpcAction, agent_enabled: bool) -> Option<UserEvent> {
    match action {
        IpcAction::AgentObservation { data } if agent_enabled => {
            parse_agent_observation(&data).map(UserEvent::AgentObservation)
        }
        other => common_ipc_event(other),
    }
}

/// A pergunta que o "Pesquisar" da barra de selecao leva ao comparador.
///
/// E o texto selecionado, aparado, e mais nada: nao passa pelo
/// `route_input` nem pelo `parse_intent`. Quem seleciona "agent:https://x"
/// ou "tema:escuro" numa pagina quer saber o que aquilo e, nao correr um
/// agente nem mudar o tema -- e a pagina, que escolhe o texto, nunca pode
/// dar ordens ao navegador por esta via.
fn selection_search_question(text: &str) -> Option<String> {
    let question = text.trim();
    (!question.is_empty() && question.chars().count() <= SEARCH_MAX_CHARS)
        .then(|| question.to_string())
}

/// O mapa de teclas (e a barra de selecao que vive nele) com a capability e
/// o sinal de superficie privada postos. Um painel privado nao mostra o
/// "Pesquisar": o texto dele nao pode ir parar ao historico nem a memoria.
fn bind_page_script(script: &str, capability: &str, private: bool) -> String {
    script.replace("__NEURALIA_CAP__", capability).replace(
        "__NEURALIA_PRIVATE__",
        if private { "true" } else { "false" },
    )
}

/// Scripts de inicializacao do painel Split, tal como o builder os injeta.
fn split_init_script(
    source_index: usize,
    source_name: &str,
    capability: &str,
    private: bool,
) -> String {
    bind_page_script(
        &format!(
            "window.__neuralia_col_index = {source_index}; window.__neuralia_col_name = '{source_name}';\n{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}"
        ),
        capability,
        private,
    )
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

/// Token que so os scripts injetados conhecem: 128 bits de CSPRNG do Windows.
/// O caminho principal usa BCryptGenRandom. Se essa API falhar, tentamos a
/// segunda interface criptografica do proprio Windows (RtlGenRandom) antes de
/// falhar fechado; nunca degradamos para tempo, PID ou outro pseudo-segredo.
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

fn capability_from_sources<F>(
    primary_status: i32,
    primary_bytes: [u8; 16],
    mut fallback: F,
) -> Option<String>
where
    F: FnMut(&mut [u8; 16]) -> bool,
{
    if let Some(token) = capability_from_rng(primary_status, primary_bytes) {
        return Some(token);
    }

    let mut secondary = [0u8; 16];
    if !fallback(&mut secondary) {
        return None;
    }
    capability_from_rng(0, secondary)
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

    capability_from_sources(status, bytes, |secondary| unsafe {
        rtl_gen_random(
            secondary.as_mut_ptr().cast::<core::ffi::c_void>(),
            secondary.len() as u32,
        ) != 0
    })
    .expect("both Windows CSPRNG providers failed; refusing unauthenticated IPC capability")
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

fn web_media_permission(kind: PermissionKind, user_visible: bool) -> PermissionResponse {
    if !user_visible {
        return PermissionResponse::Deny;
    }
    match kind {
        PermissionKind::Microphone | PermissionKind::Camera | PermissionKind::DisplayCapture => {
            // Default continua o fluxo nativo do WebView2: o utilizador decide
            // no prompt do runtime. NeuralIA nunca concede Allow silenciosamente.
            PermissionResponse::Default
        }
        _ => PermissionResponse::Deny,
    }
}

/// Pedido de janela nova vindo da pagina em Web completa. `window.open('',
/// '_blank')` chega como `about:blank`: aceite pelo `remote_web_target` (para
/// a navegacao), mas como destino de OpenExternal falha no validate_web_url e
/// o erro destruia a pagina do utilizador e voltava ao Home.
fn external_new_window_event(target: String, local_origin: Option<&str>) -> Option<UserEvent> {
    (!target.eq_ignore_ascii_case("about:blank") && remote_web_target(&target, local_origin))
        .then_some(UserEvent::OpenExternal(target))
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

/// Esconde containers WRY que ficaram órfãos depois do drop dos controllers.
///
/// WebView2 pode concluir a destruição de forma assíncrona quando a janela pai
/// troca de decorations. Nessa janela curta, um host `WRY_WEBVIEW` já sem
/// controller pode continuar com WS_VISIBLE e ficar pintado por cima da Home.
/// A Home nunca deve mostrar esses hosts. O pool de processos pode continuar
/// quente para reutilização, mas a superfície nativa precisa desaparecer.
unsafe extern "system" fn hide_wry_webview_host(hwnd: HWND, _lparam: LPARAM) -> i32 {
    let mut class_name = [0u16; 64];
    let len = GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32);
    if len > 0
        && String::from_utf16_lossy(&class_name[..len as usize]).eq_ignore_ascii_case("WRY_WEBVIEW")
    {
        ShowWindow(hwnd, SW_HIDE);
    }
    1
}

fn hide_orphaned_wry_hosts(window: &Window) {
    let Some(parent) = window_hwnd(window) else {
        return;
    };
    unsafe {
        EnumChildWindows(parent, Some(hide_wry_webview_host), 0);
    }
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
        && GMAIL_NOTIFICATIONS.load(Ordering::Acquire)
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

fn draw_home(window: &Window, status: Option<&str>, go_hover: bool) {
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
        if go_hover {
            // Parado com NEURALIA_REDUCE_MOTION; senao o degradê desliza ao
            // ritmo dos frames da Home.
            let phase = if home_animation_enabled() {
                (now_ms() % GO_GRADIENT_PERIOD_MS) as f32 / GO_GRADIENT_PERIOD_MS as f32
            } else {
                0.0
            };
            draw_go_gradient(target, layout.go, phase, body_font, &theme);
        } else {
            draw_button(target, layout.go, "Ir", true, scale, body_font, &theme);
        }

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
                    split.context_id,
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
    active_context: Option<(usize, Option<u64>, bool, bool)>,
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
            let active = active_context.is_some_and(|(source, active_id, _, _)| {
                source == index && active_id == Some(tab.id)
            });
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
        PillStyle::new(home_fill, theme.surface_line, theme.fg),
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

    // ‹ e › da fonte aberta ao lado (so existem com a gaveta) e de cada IA.
    let mut pairs = vec![
        (layout.back, "‹", BarHit::Back),
        (layout.forward, "›", BarHit::Forward),
    ];
    for index in 0..layout.columns_len {
        pairs.push((layout.column_back[index], "‹", BarHit::ColumnBack(index)));
        pairs.push((
            layout.column_forward[index],
            "›",
            BarHit::ColumnForward(index),
        ));
    }
    for (rect, label, hit) in pairs {
        if rect.width > 0.0 {
            draw_button(target, rect, label, hover == Some(hit), scale, font, theme);
        }
    }

    // Os mesmos rectangulos que o hit-testing usa; ver `right_controls`.
    let controls = right_controls(width as f64, scale, active_context.is_some());
    let gmail_tint = if GMAIL_NOTIFICATIONS.load(Ordering::Acquire) {
        theme.fg
    } else {
        theme.fg_muted
    };
    let icons = [
        (ICON_SLOT_VIDEO, Some(theme.fg)),
        (ICON_SLOT_WHATSAPP, None),
        (ICON_SLOT_YOUTUBE, None),
        (ICON_SLOT_MAIL, Some(gmail_tint)),
    ];
    for ((rect, hit), (slot, tint)) in controls.services.iter().zip(SERVICE_BUTTON_HITS).zip(icons)
    {
        draw_icon_button(target, *rect, slot, tint, hover == Some(hit), scale, theme);
    }
    // Privado: o chapeu e os oculos, sem nome (pedido do dono).
    draw_icon_button(
        target,
        controls.private,
        ICON_SLOT_INCOGNITO,
        Some(theme.fg),
        hover == Some(BarHit::Private),
        scale,
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
        // Fechar a fonte: vermelho debaixo do rato, como o fechar da janela.
        draw_pill(
            target,
            close,
            "×",
            caption_button_style(2, hover == Some(BarHit::SplitClose), theme),
            scale,
            font,
            theme.bar_bg,
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

    #[test]
    fn spec_0109_webrtc_media_requires_native_user_consent() {
        for kind in [
            PermissionKind::Microphone,
            PermissionKind::Camera,
            PermissionKind::DisplayCapture,
        ] {
            assert_eq!(
                web_media_permission(kind, true),
                PermissionResponse::Default,
                "{kind:?} deve continuar pelo prompt nativo do WebView2"
            );
            assert_ne!(
                web_media_permission(kind, true),
                PermissionResponse::Allow,
                "NeuralIA nunca deve conceder captura silenciosamente"
            );
        }
    }

    #[test]
    fn spec_0109_webrtc_media_stays_fail_closed_outside_visible_capture() {
        for kind in [
            PermissionKind::Geolocation,
            PermissionKind::Notifications,
            PermissionKind::ClipboardRead,
            PermissionKind::Sensors,
            PermissionKind::LocalFonts,
            PermissionKind::FileSystemAccess,
        ] {
            assert_eq!(web_media_permission(kind, true), PermissionResponse::Deny);
        }
        for kind in [
            PermissionKind::Microphone,
            PermissionKind::Camera,
            PermissionKind::DisplayCapture,
        ] {
            assert_eq!(
                web_media_permission(kind, false),
                PermissionResponse::Deny,
                "agente/superficie nao visivel nao pode pedir captura"
            );
        }
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
                panel_width: 0.0,
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
                panel_width: 0.0,
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

    #[test]
    fn home_button_is_text_only_without_an_invented_icon() {
        let source = include_str!("windows_app.rs");
        let native = source
            .split("fn home_button_subclass")
            .nth(1)
            .and_then(|part| part.split("fn exit_button_subclass").next())
            .expect("home_button_subclass body");
        assert!(!native.contains("with_icon("));

        let bar = source
            .split("let home_fill =")
            .nth(1)
            .and_then(|part| part.split("for (index, name)").next())
            .expect("painted Home body");
        assert!(!bar.contains("with_icon("));
    }

    #[test]
    fn native_caption_buttons_accept_the_mouse() {
        unsafe {
            let parent = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                WS_POPUP,
                0,
                0,
                200,
                80,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!parent.is_null(), "parent do caption tem de nascer");
            let hwnd = CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                138,
                32,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null(), "caption child tem de nascer");
            let subclassed = SetWindowSubclass(
                hwnd,
                Some(caption_buttons_subclass),
                CAPTION_BUTTONS_SUBCLASS_ID,
                0,
            );
            let hit = SendMessageW(hwnd, WM_NCHITTEST, 0, 0);
            DestroyWindow(parent);
            assert_ne!(subclassed, 0);
            assert_eq!(hit, HTCLIENT as LRESULT);
        }
    }

    #[test]
    fn native_home_button_has_a_stable_window_identity_for_the_shipping_gate() {
        let source = include_str!("windows_app.rs");
        let body = source
            .split("fn sync_home_button")
            .nth(1)
            .and_then(|part| part.split("fn sync_exit_button").next())
            .expect("sync_home_button body");
        assert!(body.contains(r#"windows_sys::w!("NeuralIA.Home")"#));
    }

    #[test]
    fn native_home_button_accepts_the_mouse() {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                WS_POPUP,
                0,
                0,
                80,
                30,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null(), "o Home nativo tem de nascer");
            let subclassed =
                SetWindowSubclass(hwnd, Some(home_button_subclass), HOME_BUTTON_SUBCLASS_ID, 0);
            let hit = SendMessageW(hwnd, WM_NCHITTEST, 0, 0);
            DestroyWindow(hwnd);
            assert_ne!(subclassed, 0);
            assert_eq!(hit, HTCLIENT as LRESULT);
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
    fn auxiliary_popups_never_steal_activation_from_the_main_window() {
        // Regressao 2.1.5: on_focus_changed volta a mostrar divisores e botao
        // de saida a cada foco, e SW_SHOW ativava-os apesar de
        // WS_EX_NOACTIVATE. A pagina clicada perdia o foco para um divisor
        // escondido: cliques sem efeito, sem cursor, teclado no vazio.
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetActiveWindow, SetActiveWindow};
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindowVisible, WS_OVERLAPPEDWINDOW};
        unsafe {
            let owner = CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                windows_sys::w!("NeuralIA dono"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                0,
                0,
                320,
                240,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!owner.is_null(), "a janela dona tem de nascer");
            SetActiveWindow(owner);
            assert_eq!(
                GetActiveWindow(),
                owner,
                "pre-condicao: o dono e a janela ativa"
            );

            // A mesma receita de criacao que o produto usa nos quatro popups.
            let popup = CreateWindowExW(
                AUX_POPUP_EX_STYLE,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                AUX_POPUP_STYLE,
                0,
                0,
                7,
                100,
                owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!popup.is_null(), "o popup auxiliar tem de nascer");
            let after_create = GetActiveWindow();

            // Cada ganho de foco da janela volta a mostrar o popup.
            let mut stolen_on_show = None;
            for cycle in 0..3 {
                show_popup_without_activation(popup);
                if GetActiveWindow() != owner && stolen_on_show.is_none() {
                    stolen_on_show = Some(cycle);
                }
            }
            let visible = IsWindowVisible(popup) != 0;
            DestroyWindow(popup);
            DestroyWindow(owner);

            assert_eq!(
                after_create, owner,
                "criar o popup roubou a ativacao ao dono"
            );
            assert_eq!(
                stolen_on_show, None,
                "mostrar o popup roubou a ativacao ao dono no ciclo {stolen_on_show:?}"
            );
            assert!(visible, "o popup tem de ficar visivel depois de mostrado");
        }
    }

    #[test]
    fn small_button_glyphs_draw_on_the_pill_not_on_a_white_box() {
        // Regressao 2.1.5: o botao Home e os botoes -/□/x pintam num DC de
        // BeginPaint, que nasce OPAQUE com fundo branco -- o texto saia num
        // quadrado branco. E o "+" dos botoes redondos virava "-." porque a
        // margem de 11 px deixava ~10 px de texto e o DT_END_ELLIPSIS cortava.
        use windows_sys::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
            DIB_RGB_COLORS, DeleteDC, GetDC, RGBQUAD, ReleaseDC,
        };
        // O botao "+" tal como a barra real o calcula (26x26 a escala 1).
        let plus =
            BarLayout::with_contexts(1440.0, 1.0, true, BarColumns::even(3), [0, 0, 0]).add_tabs[0];
        let (width, height) = (plus.width.round() as i32, plus.height.round() as i32);
        assert!(width > 0 && height > 0, "a barra tem de ter o botao \"+\"");
        let mut theme = Theme::dark((0, 120, 215));
        // Texto vermelho: distinguivel do fundo escuro e do branco do bug.
        theme.fg = (220, 30, 30);
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            ReleaseDC(std::ptr::null_mut(), screen);
            assert!(!mem.is_null() && !bitmap.is_null());
            let old = SelectObject(mem, bitmap as _);
            let font = create_font(-13, FW_NORMAL as i32);
            draw_button(
                mem,
                UiRect {
                    x: 0.0,
                    y: 0.0,
                    width: width as f64,
                    height: height as f64,
                },
                "+",
                false,
                1.0,
                font,
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
                bmiColors: [RGBQUAD {
                    rgbBlue: 0,
                    rgbGreen: 0,
                    rgbRed: 0,
                    rgbReserved: 0,
                }; 1],
            };
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            let read = GetDIBits(
                mem,
                bitmap,
                0,
                height as u32,
                pixels.as_mut_ptr() as _,
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(mem, old);
            DeleteObject(font as _);
            DeleteObject(bitmap as _);
            DeleteDC(mem);
            assert_eq!(read, height, "GetDIBits tem de ler o botao inteiro");

            let at = |x: i32, y: i32| {
                let i = ((y * width + x) * 4) as usize;
                (pixels[i + 2], pixels[i + 1], pixels[i]) // BGRA -> RGB
            };
            let white = (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .filter(|&(x, y)| at(x, y) == (255, 255, 255))
                .count();
            assert_eq!(
                white, 0,
                "{white} pixels brancos: o texto pintou o seu fundo opaco"
            );

            // O "+" tem traco vertical: tinta vermelha acima E abaixo do centro.
            let red = |x: i32, y: i32| {
                let (r, g, _) = at(x, y);
                r > 110 && r as i32 > g as i32 + 50
            };
            let column = |ys: std::ops::Range<i32>| {
                ys.into_iter()
                    .any(|y| (width / 2 - 2..=width / 2 + 2).any(|x| red(x, y)))
            };
            let mid = height / 2;
            assert!(
                column(mid - 6..mid - 1) && column(mid + 2..mid + 7),
                "sem traco vertical no centro: o \"+\" foi cortado em reticencias"
            );
        }
    }

    #[test]
    fn every_bar_target_has_a_tooltip_that_says_what_the_click_does() {
        let url = "https://exemplo.pt/artigo";
        for hit in [
            BarHit::Home,
            BarHit::Back,
            BarHit::Forward,
            BarHit::ColumnBack(1),
            BarHit::ColumnForward(1),
            BarHit::Column(1),
            BarHit::AddTab(1),
            BarHit::ContextTab {
                source_index: 1,
                context_index: 0,
            },
            BarHit::ContextGroup {
                source_index: 1,
                group_index: 0,
            },
            BarHit::SplitExpand,
            BarHit::SplitClose,
            BarHit::Private,
            BarHit::Service(Service::WhatsApp),
            BarHit::GmailToggle,
            BarHit::WindowMinimize,
            BarHit::WindowMaximize,
            BarHit::WindowClose,
        ] {
            let label =
                bar_tooltip_label(hit, "ChatGPT", false, Some(url), Some(("Pesquisa", true)));
            assert!(
                label.as_deref().is_some_and(|text| !text.trim().is_empty()),
                "{hit:?} ficou sem dica"
            );
        }
        let label = |hit, maximized| {
            bar_tooltip_label(
                hit,
                "ChatGPT",
                maximized,
                Some(url),
                Some(("Pesquisa", true)),
            )
        };
        assert_eq!(
            label(BarHit::AddTab(1), false).as_deref(),
            Some("Nova pergunta ao ChatGPT")
        );
        assert_eq!(
            label(BarHit::WindowMaximize, false).as_deref(),
            Some("Maximizar")
        );
        assert_eq!(
            label(BarHit::WindowMaximize, true).as_deref(),
            Some("Restaurar")
        );
        let tab = BarHit::ContextTab {
            source_index: 1,
            context_index: 0,
        };
        assert!(label(tab, false).is_some_and(|text| text.starts_with(url)));
        let group = BarHit::ContextGroup {
            source_index: 1,
            group_index: 0,
        };
        assert!(label(group, false).is_some_and(|text| text.contains("mostrar as abas")));
    }

    #[test]
    fn hovering_a_target_shows_its_hint_in_the_center_and_leaving_hides_it() {
        // O que o dono ve: a dica como mensagem no MEIO da janela, visivel,
        // com o texto do alvo, sem roubar a ativacao, e escondida ao sair.
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetActiveWindow, SetActiveWindow};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowRect, IsWindowVisible, WS_OVERLAPPEDWINDOW,
        };
        unsafe {
            let owner = CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                windows_sys::w!("NeuralIA dono"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                0,
                0,
                900,
                600,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!owner.is_null(), "a janela tem de nascer");
            SetActiveWindow(owner);

            // O rato para no minimizar, passa para o fechar e o temporizador
            // dispara (aqui sem esperar os 450 ms).
            hover_tooltip(owner, "Minimizar");
            hover_tooltip(owner, "Fechar");
            show_pending_tooltip();
            let hint = HINT_HWND.load(Ordering::Acquire) as HWND;
            let shown = !hint.is_null() && IsWindowVisible(hint) != 0;
            let text = HINT_TEXT
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            let active = GetActiveWindow();
            let mut box_rect = RECT::default();
            GetWindowRect(hint, &mut box_rect);
            let mut client = RECT::default();
            GetClientRect(owner, &mut client);
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let owner_center = (origin.x + client.right / 2, origin.y + client.bottom / 2);
            let hint_center = (
                (box_rect.left + box_rect.right) / 2,
                (box_rect.top + box_rect.bottom) / 2,
            );

            // O rato sai de todos os alvos.
            hover_tooltip(owner, "");
            let hidden = IsWindowVisible(hint) == 0;
            DestroyWindow(owner);

            assert!(shown, "a dica nao apareceu depois do atraso");
            assert_eq!(text, "Fechar", "a dica nao acompanhou o rato");
            assert_eq!(active, owner, "a dica roubou a ativacao a janela");
            assert!(
                (hint_center.0 - owner_center.0).abs() <= 1
                    && (hint_center.1 - owner_center.1).abs() <= 1,
                "a dica nao esta no meio: {hint_center:?} vs {owner_center:?}"
            );
            assert!(hidden, "a dica ficou a vista depois de o rato sair");
        }
    }

    #[test]
    fn theme_choice_is_saved_loaded_and_overrides_the_system() {
        let dir = std::env::temp_dir().join(format!("neuralia-theme-{}", std::process::id()));
        let path = dir.join("theme");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            ThemeChoice::load(&path),
            ThemeChoice::System,
            "sem ficheiro vale o sistema"
        );
        ThemeChoice::Dark
            .save(&path)
            .expect("o tema tem de ficar guardado");
        assert_eq!(
            ThemeChoice::load(&path),
            ThemeChoice::Dark,
            "a escolha nao voltou"
        );
        std::fs::write(&path, "roxo").expect("escreve lixo");
        assert_eq!(
            ThemeChoice::load(&path),
            ThemeChoice::System,
            "lixo vale o sistema"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // A escolha manda sobre o Windows; so "sistema" o segue.
        let accent = system_accent();
        assert_eq!(
            Theme::read_for(ThemeChoice::Dark).page_bg,
            Theme::dark(accent).page_bg
        );
        assert_eq!(
            Theme::read_for(ThemeChoice::Light).page_bg,
            Theme::light(accent).page_bg
        );
        let system = if system_dark_mode() {
            Theme::dark(accent)
        } else {
            Theme::light(accent)
        };
        assert_eq!(Theme::read_for(ThemeChoice::System).page_bg, system.page_bg);

        assert_eq!(
            route_input("tema:escuro"),
            InputRoute::Theme(Some(ThemeChoice::Dark))
        );
        assert_eq!(
            route_input("tema: claro"),
            InputRoute::Theme(Some(ThemeChoice::Light))
        );
        assert_eq!(
            route_input("tema:sistema"),
            InputRoute::Theme(Some(ThemeChoice::System))
        );
        assert_eq!(route_input("tema:roxo"), InputRoute::Theme(None));
    }

    #[test]
    fn debug_log_appends_timestamped_lines_and_never_panics() {
        let dir = std::env::temp_dir().join(format!("neuralia-debuglog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pasta temporaria");
        let path = dir.join("debug.log");
        append_debug_line(
            &path,
            7,
            format_args!("focus=true surface={:?}", Surface::Home),
        );
        append_debug_line(
            &path,
            1234,
            format_args!("open_comparator: set_decorations(false)"),
        );
        let text = std::fs::read_to_string(&path).expect("o log tem de existir");
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "       7 ms  focus=true surface=Home",
                "    1234 ms  open_comparator: set_decorations(false)"
            ]
        );
        // Um caminho impossivel nao derruba o app.
        append_debug_line(&dir.join("nao/existe/debug.log"), 1, format_args!("x"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn side_panel_messages_are_a_closed_list_with_limits() {
        assert_eq!(
            parse_panel_message(r#"{"action":"ready"}"#),
            Some(PanelMessage::Ready)
        );
        assert_eq!(
            parse_panel_message(r#"{"action":"close","args":{}}"#),
            Some(PanelMessage::Close)
        );
        assert_eq!(
            parse_panel_message(r#"{"action":"search","args":{"query":"  receita de bolo "}}"#),
            Some(PanelMessage::Search("receita de bolo".to_string()))
        );
        assert_eq!(
            parse_panel_message(r#"{"action":"open","args":{"input":"https://exemplo.pt"}}"#),
            Some(PanelMessage::Open("https://exemplo.pt".to_string()))
        );
        for bad in [
            r#"{"action":"clearhistory"}"#,
            r#"{"action":"search","args":{"query":"   "}}"#,
            r#"{"action":"search"}"#,
            r#"{"action":"open","args":{"input":5}}"#,
            "nao e json",
        ] {
            assert_eq!(parse_panel_message(bad), None, "{bad}");
        }
        let long = format!(
            r#"{{"action":"search","args":{{"query":"{}"}}}}"#,
            "a".repeat(PANEL_QUERY_MAX_CHARS + 1)
        );
        assert_eq!(parse_panel_message(&long), None, "consulta acima do limite");
        let huge = format!(
            r#"{{"action":"ready","pad":"{}"}}"#,
            "x".repeat(PANEL_MESSAGE_MAX_BYTES)
        );
        assert_eq!(parse_panel_message(&huge), None, "mensagem acima de 4 KiB");
    }

    #[test]
    fn side_panel_only_ever_shows_its_local_page() {
        assert!(panel_allows_navigation("about:blank"));
        assert!(panel_allows_navigation("data:text/html,<p>x</p>"));
        for target in [
            "https://exemplo.pt",
            "http://127.0.0.1:8080/",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "neuralia-pdf://viewer",
            "about:blank.evil",
        ] {
            assert!(!panel_allows_navigation(target), "{target}");
        }
    }

    #[test]
    fn side_panel_sits_on_the_right_below_the_bar() {
        // 34% de 1440 = 489.6, limitado a 440; por baixo da barra do comparador.
        assert_eq!(
            side_panel_bounds(1440.0, 900.0, 76.0),
            (1000.0, 76.0, 440.0, 824.0)
        );
        // 34% de 900 = 306, levado ao minimo de 320; fora do comparador, do topo.
        assert_eq!(
            side_panel_bounds(900.0, 600.0, 0.0),
            (580.0, 0.0, 320.0, 600.0)
        );
        // Janela mais estreita do que o minimo: o painel ocupa-a, nunca sai dela.
        assert_eq!(
            side_panel_bounds(250.0, 400.0, 0.0),
            (0.0, 0.0, 250.0, 400.0)
        );
    }

    #[test]
    fn side_panel_data_reaches_the_page_as_text_never_as_html() {
        // Os titulos vem de paginas remotas: nunca podem virar HTML no painel.
        assert!(!PANEL_HTML.contains("innerHTML"));
        assert!(!PANEL_HTML.contains("insertAdjacentHTML"));
        assert!(!PANEL_HTML.contains("document.write"));
        let hostile = PanelItem {
            title: "<img src=x onerror=alert(1)>".to_string(),
            detail: "</script><script>alert(2)</script>".to_string(),
            input: "javascript:alert(3)".to_string(),
        };
        let script = panel_render_script("busca", "Busca", "vazio", std::slice::from_ref(&hostile));
        let json = script
            .strip_prefix("window.__neuraliaPanel && window.__neuraliaPanel.render(")
            .and_then(|rest| rest.strip_suffix(");"))
            .expect("formato do script");
        let value: serde_json::Value = serde_json::from_str(json).expect("os dados vao como JSON");
        assert_eq!(value["items"][0]["title"], hostile.title.as_str());
        assert_eq!(value["items"][0]["detail"], hostile.detail.as_str());
        assert_eq!(value["id"], "busca");
    }

    #[test]
    fn side_panel_lists_history_search_and_one_suggestion_per_site() {
        let entry = |kind, input: &str, target: &str| HistoryEntry {
            timestamp_unix: 1,
            kind,
            input: input.to_string(),
            target: target.to_string(),
        };
        let items = history_panel_items(&[
            entry(HistoryKind::Ask, "o que e rust", ""),
            entry(HistoryKind::Web, "exemplo.pt", "https://exemplo.pt/"),
            entry(HistoryKind::Read, "   ", "https://vazio.pt/"),
        ]);
        assert_eq!(items.len(), 2, "entrada vazia nao vira item");
        assert_eq!(items[0].detail, "IA");
        assert_eq!(items[1].detail, "Web · https://exemplo.pt/");
        assert_eq!(
            items[1].input, "exemplo.pt",
            "o clique repete o que foi escrito"
        );

        let hit = |title: &str, url: Option<&str>| MemoryHit {
            id: title.to_string(),
            title: title.to_string(),
            url: url.map(str::to_string),
            provider: None,
            session_id: None,
            excerpt: String::new(),
            score: 1.0,
            matched_by: Vec::new(),
        };
        let hits = [
            hit("Aprender Rust", Some("https://www.rust-lang.org/learn")),
            hit("Ferramentas", Some("https://rust-lang.org/tools")),
            hit("Nota sem endereco", None),
            hit("Ficheiro local", Some("file:///C:/notas.txt")),
            hit("Docs", Some("https://docs.rs/")),
        ];
        let suggestions = suggestion_panel_items(&hits, 6);
        assert_eq!(
            suggestions
                .iter()
                .map(|item| item.detail.as_str())
                .collect::<Vec<_>>(),
            ["rust-lang.org", "docs.rs"],
            "um site por dominio, so http(s)"
        );
        assert_eq!(suggestions[0].input, "https://www.rust-lang.org/learn");
        assert_eq!(
            suggestion_panel_items(&hits, 1).len(),
            1,
            "respeita o limite"
        );

        let search = memory_panel_items(&hits[2..3]);
        assert_eq!(
            search[0].input, "Nota sem endereco",
            "sem endereco, o clique repete a busca"
        );
    }

    #[test]
    fn every_ai_column_has_its_own_back_and_forward_after_its_plus() {
        let layout = BarLayout::with_contexts(1440.0, 1.0, true, BarColumns::even(3), [0, 0, 0]);
        for index in 0..3 {
            let (plus, back, forward) = (
                layout.add_tabs[index],
                layout.column_back[index],
                layout.column_forward[index],
            );
            assert!(
                back.width > 0.0 && forward.width > 0.0,
                "coluna {index} sem ‹ ›"
            );
            assert!(
                back.x >= plus.x + plus.width,
                "o ‹ vem depois do + na coluna {index}"
            );
            assert!(
                forward.x >= back.x + back.width,
                "o › vem depois do ‹ na coluna {index}"
            );
            if index + 1 < 3 {
                assert!(
                    forward.x + forward.width <= layout.columns[index + 1].x,
                    "os ‹ › da coluna {index} invadem a coluna seguinte"
                );
            }
            let center = |rect: UiRect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            assert_eq!(
                layout.hit(center(back).0, center(back).1),
                Some(BarHit::ColumnBack(index))
            );
            assert_eq!(
                layout.hit(center(forward).0, center(forward).1),
                Some(BarHit::ColumnForward(index))
            );
        }
        // Janela estreita: a pilula encolhe primeiro; o par ou cabe na faixa da
        // coluna ou desaparece -- nunca fica por cima da IA seguinte.
        for width in (560..=1600).step_by(20) {
            let narrow =
                BarLayout::with_contexts(width as f64, 1.0, true, BarColumns::even(3), [0, 0, 0]);
            for index in 0..2 {
                let forward = narrow.column_forward[index];
                assert!(
                    forward.width == 0.0
                        || forward.x + forward.width <= narrow.columns[index + 1].x,
                    "a {width}px os ‹ › da coluna {index} invadem a coluna seguinte"
                );
            }
        }
        // Sem fonte aberta ao lado, nao ha o par da fonte.
        assert_eq!(layout.back.width, 0.0);
        // Com a fonte aberta, o par dela fica a esquerda do rotulo.
        let drawer = right_controls(1440.0, 1.0, true);
        let ((back, forward), (label, _, _)) = (
            drawer.split_nav.expect("‹ › da fonte"),
            drawer.split.expect("gaveta"),
        );
        assert!(forward.x + forward.width <= label.x && back.x + back.width <= forward.x);
        assert!(
            drawer.private.x + drawer.private.width <= back.x,
            "Privado antes do par"
        );
    }

    #[test]
    fn back_and_forward_move_the_page_the_user_is_reading() {
        use HistoryNav::*;
        // A fonte aberta ao lado ganha a tudo: e la que se seguem links.
        assert_eq!(
            history_nav_target(Surface::Comparator, true, Some(1), false),
            Split
        );
        assert_eq!(
            history_nav_target(Surface::Comparator, false, Some(2), false),
            Column(2)
        );
        // Tres colunas lado a lado: nao ha uma pagina so.
        assert_eq!(
            history_nav_target(Surface::Comparator, false, None, false),
            App
        );
        assert_eq!(history_nav_target(Surface::Home, false, None, false), App);
    }

    #[test]
    fn home_has_no_windows_title_bar_but_keeps_its_window_buttons() {
        // A Home ficou sem a barra do Windows (pedido do dono): sem os botoes
        // do proprio app, nao haveria como minimizar nem fechar.
        assert!(caption_buttons_wanted(Surface::Home, false));
        assert!(caption_buttons_wanted(Surface::Comparator, true));
        assert!(!caption_buttons_wanted(Surface::Comparator, false));
        // E a janela agarra-se pela faixa de cima, nao pelo meio da Home.
        assert!(home_drag_strip(4.0, 1.0));
        assert!(home_drag_strip(TITLE_TAB_HEIGHT * 2.0 - 1.0, 2.0));
        assert!(!home_drag_strip(TITLE_TAB_HEIGHT + 20.0, 1.0));
    }

    #[test]
    fn the_close_button_turns_red_under_the_mouse_like_chrome() {
        let theme = Theme::dark((0, 120, 215));
        let close = caption_button_style(2, true, &theme);
        assert_eq!(
            close.fill, CLOSE_HOVER_RED,
            "o fechar debaixo do rato e vermelho"
        );
        assert_eq!(close.text, (255, 255, 255), "com a cruz branca");
        assert_ne!(caption_button_style(2, false, &theme).fill, CLOSE_HOVER_RED);
        // O ✕ do painel lateral segue a mesma regra (CSS da pagina local).
        assert!(PANEL_HTML.contains("#close:hover{background:#e81123;color:#fff}"));
        for index in [0, 1] {
            let style = caption_button_style(index, true, &theme);
            assert_ne!(style.fill, CLOSE_HOVER_RED, "so o fechar fica vermelho");
            assert_ne!(
                style.fill,
                caption_button_style(index, false, &theme).fill,
                "realce"
            );
        }
    }

    #[test]
    fn service_icons_sit_left_of_private_without_overlap_and_hit_their_service() {
        let controls = right_controls(1600.0, 1.0, false);
        let order = [
            BarHit::Service(Service::Meet),
            BarHit::Service(Service::WhatsApp),
            BarHit::Service(Service::YouTube),
            BarHit::GmailToggle,
        ];
        let mut previous_right = f64::MIN;
        for (rect, hit) in controls.services.iter().zip(order) {
            assert!(rect.width > 0.0, "{hit:?} tem de existir");
            assert!(rect.x >= previous_right, "{hit:?} sobrepoe o vizinho");
            previous_right = rect.x + rect.width;
            let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            assert_eq!(right_controls_hit(controls, center.0, center.1), Some(hit));
        }
        assert!(
            previous_right <= controls.private.x,
            "os icones ficam a esquerda do Privado"
        );
        assert_eq!(controls.leftmost(), controls.services[0].x);
        // O Privado passa a ser um botao redondo so com o icone.
        assert_eq!(controls.private.width, controls.private.height);
        // Com a gaveta aberta tudo continua a esquerda dela.
        let drawer = right_controls(1600.0, 1.0, true);
        let (label, _, _) = drawer.split.expect("gaveta");
        assert!(drawer.private.x + drawer.private.width <= label.x);
    }

    #[test]
    fn a_service_panel_only_loads_web_pages_and_is_wider() {
        for target in [
            "https://web.whatsapp.com/",
            "https://meet.google.com/abc",
            "about:blank",
        ] {
            assert!(service_panel_allows_navigation(target), "{target}");
        }
        for target in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "neuralia-pdf://x",
            "data:text/html,x",
        ] {
            assert!(!service_panel_allows_navigation(target), "{target}");
        }
        // 42% de 1440 = 604.8 (entre 400 e 640), encostado a direita.
        let (x, top, width, height) = service_panel_bounds(1440.0, 900.0, 76.0);
        assert!((width - 604.8).abs() < 1e-6, "{width}");
        assert!((x + width - 1440.0).abs() < 1e-6 && top == 76.0 && height == 824.0);
        // 42% de 900 = 378, levado ao minimo de 400.
        assert_eq!(
            service_panel_bounds(900.0, 600.0, 0.0),
            (500.0, 0.0, 400.0, 600.0)
        );
    }

    #[test]
    fn gmail_setting_round_trips_and_the_toast_answers_by_button() {
        let dir = std::env::temp_dir().join(format!("neuralia-gmail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("gmail");
        assert!(load_gmail_setting(&path), "sem ficheiro, ligado");
        save_gmail_setting(&path, false).expect("grava");
        assert!(!load_gmail_setting(&path), "desligado voltou ligado");
        save_gmail_setting(&path, true).expect("grava");
        assert!(load_gmail_setting(&path));
        let _ = std::fs::remove_dir_all(&dir);

        let client = RECT {
            left: 0,
            top: 0,
            right: 360,
            bottom: 64,
        };
        let (open, no) = gmail_toast_buttons(&client, 1.0);
        assert!(
            open.right <= no.left,
            "Abrir fica antes de Nao, sem se tocarem"
        );
        assert!(no.right <= client.right && open.left >= 0);
        assert!(open.top >= 0 && open.bottom <= client.bottom);
    }

    #[test]
    fn hint_is_a_smooth_pill_like_the_buttons() {
        // Pilula como os botoes: metade da altura de raio (limitado).
        assert_eq!(hint_radius(48.0, 1.0), 24.0);
        assert_eq!(hint_radius(120.0, 1.0), 28.0, "dica alta nao fica oval");
        let (width, height) = (160usize, 48usize);
        let surface = (37u8, 41u8, 44u8);
        let mut pixels: Vec<u8> = (0..width * height)
            .flat_map(|_| [surface.2, surface.1, surface.0, 255])
            .collect();
        apply_hint_shape(&mut pixels, width, height, 24.0, (70, 76, 80));
        let alpha = |x: usize, y: usize| pixels[(y * width + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "o canto e transparente");
        assert_eq!(alpha(width / 2, height / 2), 255, "o meio e opaco");
        // Anti-aliasing: ao longo da curva ha alfa intermedio, nao escadinhas.
        let soft = (0..height).any(|y| (0..24).any(|x| (1..255).contains(&alpha(x, y))));
        assert!(soft, "a borda nao e suave: so ha alfa 0 ou 255");
        // Pre-multiplicado: nenhum canal de cor acima do alfa.
        assert!(
            pixels
                .chunks(4)
                .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
        );
    }

    #[test]
    fn an_open_side_panel_shrinks_the_comparator_instead_of_covering_it() {
        let columns = BarColumns {
            panel_width: 440.0,
            ..BarColumns::even(3)
        };
        let layout = BarLayout::with_contexts(1600.0, 1.0, true, columns, [0, 0, 0]);
        let content_right = 1600.0 - 440.0;
        for index in 0..3 {
            let pill = layout.columns[index];
            let plus = layout.add_tabs[index];
            assert!(
                pill.x + pill.width <= content_right + 0.5
                    && plus.x + plus.width <= content_right + 0.5,
                "a coluna {index} ficou por baixo do painel"
            );
        }
        // O painel empurra: a terceira coluna recua em relacao a barra sem painel.
        let plain = BarLayout::with_contexts(1600.0, 1.0, true, BarColumns::even(3), [0, 0, 0]);
        assert!(
            layout.columns[2].x < plain.columns[2].x - 100.0,
            "a terceira coluna nao recuou: {} vs {}",
            layout.columns[2].x,
            plain.columns[2].x
        );
    }

    #[test]
    fn the_go_button_lights_up_only_under_the_mouse_on_home() {
        let (size, scale) = ((1600.0, 900.0), 1.0);
        let go = HomeLayout::new(size.0, size.1, scale).go;
        let center = (go.x + go.width / 2.0, go.y + go.height / 2.0);
        assert!(home_go_hovered(Surface::Home, size, scale, center));
        assert!(!home_go_hovered(
            Surface::Home,
            size,
            scale,
            (go.x - 2.0, center.1)
        ));
        assert!(!home_go_hovered(Surface::Home, size, scale, (-1.0, -1.0)));
        assert!(
            !home_go_hovered(Surface::Comparator, size, scale, center),
            "o Ir nao existe fora da Home"
        );
    }

    #[test]
    fn the_go_button_turns_into_a_sliding_gradient() {
        let (from, to) = ((26, 115, 232), GO_GRADIENT_END);
        // Fase 0: da cor de destaque (esquerda) ao violeta (direita).
        assert_eq!(go_gradient_color(from, to, 0.0, 0.0), from);
        assert_eq!(go_gradient_color(from, to, 1.0, 0.0), to);
        // A fase desliza a onda: a ponta esquerda muda de cor com o tempo.
        assert_ne!(go_gradient_color(from, to, 0.0, 0.25), from);
        // Nos pixels da pilula: o corpo a esquerda e a direita difere mesmo.
        let (width, height) = (84, 54);
        let pixels = pill_pixels(
            width,
            height,
            27.0,
            &|t| go_gradient_color(from, to, t, 0.0),
            None,
            (255, 255, 255),
        );
        let at = |x: i32| {
            let index = ((height / 2 * width + x) * 4) as usize;
            (pixels[index + 2], pixels[index + 1], pixels[index])
        };
        let near = |a: Rgb, b: Rgb| {
            (a.0 as i32 - b.0 as i32).abs() <= 24
                && (a.1 as i32 - b.1 as i32).abs() <= 24
                && (a.2 as i32 - b.2 as i32).abs() <= 24
        };
        assert!(near(at(2), from), "esquerda {:?}", at(2));
        assert!(near(at(width - 3), to), "direita {:?}", at(width - 3));
        // Solido continua solido: o refactor nao mexeu nos outros botoes.
        let solid = pill_pixels(width, height, 27.0, &|_| from, None, (255, 255, 255));
        let index = ((height / 2 * width + width / 2) * 4) as usize;
        assert_eq!((solid[index + 2], solid[index + 1], solid[index]), from);
    }

    #[test]
    fn the_splash_question_opens_in_the_center_of_the_window() {
        // Janela 1440x900, splash 400x120: centro exacto, nao o rodape.
        assert_eq!(splash_origin(1440, 900, 400, 120), (520, 390));
        // Janela mais pequena do que o splash: encosta ao canto, nao foge.
        assert_eq!(splash_origin(300, 100, 400, 120), (0, 0));
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

        #[test]
        fn extract_goes_through_the_policy_gate() {
            // Um clique em A leva a B; o `extract` seguinte guardava a página
            // de B na memória semântica sem diálogo e sem entrada de auditoria.
            let mut other = page(vec![]);
            other.url = "https://outra.example/conta".into();
            let mut policy = policy();
            let decision = decide(&[BrowserAgentCommand::Extract], &other, &mut policy);

            assert_ne!(
                decision,
                AgentStepDecision::Extract,
                "extract noutra origem sem um sim humano"
            );
            assert!(
                matches!(
                    &decision,
                    AgentStepDecision::ConfirmExtract {
                        security: AgentSecurityAction::Extract { origin },
                        ..
                    } if origin == "https://outra.example"
                ),
                "{decision:?}"
            );
            assert_eq!(policy.audit().len(), 1, "extract fora da auditoria");
            let entry = &policy.audit()[0];
            assert_eq!(entry.action, "extract");
            assert!(entry.confirmation_required);
            assert!(entry.reason.contains("cross-origin"), "{}", entry.reason);

            // Na origem aprovada segue sem diálogo, mas fica auditado.
            let same = page(vec![]);
            let mut policy = self::policy();
            let decision = decide(&[BrowserAgentCommand::Extract], &same, &mut policy);
            assert_eq!(decision, AgentStepDecision::Extract);
            assert_eq!(policy.audit().len(), 1);
            assert!(policy.audit()[0].allowed);
        }
    }

    /// Os gates do agente sobre o JavaScript que EMBARCA.
    ///
    /// O `AGENT_OBSERVER_SCRIPT` e o `agent_action_script` correm aqui dentro
    /// de um DOM mínimo em Node (`node:vm`), e o que eles publicam passa pelo
    /// mesmo caminho nativo do produto: `parse_ipc_message` →
    /// `parse_agent_observation` → `decide_agent_step`. Asserções sobre o texto
    /// do script não apanhavam nenhum destes defeitos (AGENTS.md §4.3): o
    /// observador e o guard falavam vocabulários diferentes e os testes de
    /// texto ficavam verdes.
    mod agent_dom_gates {
        use super::*;
        use serde_json::{Value, json};
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        const CAP: &str = "0123456789abcdef0123456789abcdef";

        /// Um DOM de brinquedo, com o que o observador e o guard usam: tipos
        /// por omissão como no HTML (`<button>` é `submit`, `<select>` é
        /// `select-one`), `setAttribute` a disparar o `MutationObserver`,
        /// relógio falso para os `setTimeout` e o `postMessage` capturado.
        const HARNESS: &str = r#"
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
const page = input.page;
const posts = [];
const timers = new Map();
const observers = [];
let now = 0, seq = 0, mutated = false;
function listeners(target, type) {
  if (!target.__l) target.__l = {};
  return target.__l[type] || (target.__l[type] = []);
}
class FakeEventTarget {}
FakeEventTarget.prototype.addEventListener = function (type, fn) { listeners(this, type).push(fn); };
FakeEventTarget.prototype.dispatchEvent = function (event) {
  if (this.events) this.events.push(event.type);
  for (const fn of listeners(this, event.type).slice()) fn.call(this, event);
  return true;
};
class FakeEvent { constructor(type, init) { this.type = type; this.bubbles = !!(init && init.bubbles); this.isTrusted = false; } }
const form = { submits: 0 };
const NAMED = ['input', 'select', 'textarea', 'button', 'a'];
class FakeElement extends FakeEventTarget {
  constructor(spec) {
    super();
    this.spec = spec; this.tag = spec.tag; this.tagName = spec.tag.toUpperCase();
    this.attrs = new Map(Object.entries(spec.attrs || {}));
    this.clicks = 0; this.events = []; this.form = spec.form ? form : null;
  }
  getAttribute(name) { return this.attrs.has(name) ? String(this.attrs.get(name)) : null; }
  setAttribute(name, value) { this.attrs.set(name, String(value)); mutated = true; }
  get disabled() { return this.attrs.has('disabled'); }
  get type() {
    const t = (this.getAttribute('type') || '').toLowerCase();
    if (this.tag === 'input') return t || 'text';
    if (this.tag === 'button') return t === 'button' || t === 'reset' ? t : 'submit';
    if (this.tag === 'select') return 'select-one';
    if (this.tag === 'textarea') return 'textarea';
    if (this.tag === 'a') return this.getAttribute('type') || '';
    return undefined;
  }
  get name() { return NAMED.includes(this.tag) ? (this.getAttribute('name') || '') : undefined; }
  get autocomplete() { return ['input', 'select', 'textarea'].includes(this.tag) ? (this.getAttribute('autocomplete') || '') : undefined; }
  get placeholder() { return ['input', 'textarea'].includes(this.tag) ? (this.getAttribute('placeholder') || '') : undefined; }
  get innerText() {
    if (this.tag === 'select') return (this.spec.options || []).join('\n');
    if (this.tag === 'input' || this.tag === 'textarea') return '';
    return this.spec.text || '';
  }
  get textContent() {
    if (this.tag === 'select') return (this.spec.options || []).join('');
    if (this.tag === 'input') return '';
    return this.spec.text || '';
  }
  getBoundingClientRect() { return this.spec.hidden ? { width: 0, height: 0 } : { width: 120, height: 24 }; }
  focus() {}
  click() { this.clicks += 1; if (this.form && this.type === 'submit') form.submits += 1; }
}
class FakeField extends FakeElement {
  get value() { return this._value !== undefined ? this._value : (this.getAttribute('value') || ''); }
  set value(v) { this._value = String(v); }
}
class FakeSelect extends FakeElement {
  get value() { return this._value !== undefined ? this._value : ((this.spec.options || [])[0] || ''); }
  set value(v) { v = String(v); this._value = (this.spec.options || []).includes(v) ? v : ''; }
}
const elements = (page.elements || []).map((spec) =>
  spec.tag === 'select' ? new FakeSelect(spec)
    : (spec.tag === 'input' || spec.tag === 'textarea') ? new FakeField(spec)
    : new FakeElement(spec));
function matchesPart(el, part) {
  const m = /^([a-z]*)(?:\[([a-z-]+)(?:="([^"]*)")?\])?$/.exec(part.trim());
  if (!m) throw new Error('unsupported selector: ' + part);
  const [, tag, attr, value] = m;
  if (tag && el.tag !== tag) return false;
  if (attr) {
    if (!el.attrs.has(attr)) return false;
    if (value !== undefined && el.getAttribute(attr) !== value) return false;
  }
  return true;
}
function matches(el, selector) { return selector.split(',').some((part) => matchesPart(el, part)); }
const textRoot = (t) => ({ innerText: t, textContent: t });
const main = page.main !== undefined ? textRoot(page.main) : null;
const document = Object.assign(new FakeEventTarget(), {
  readyState: 'complete',
  title: page.title || '',
  documentElement: {},
  body: textRoot(page.body || ''),
  querySelectorAll(selector) { return elements.filter((el) => matches(el, selector)); },
  querySelector(selector) {
    if (selector === 'main,[role="main"]') return main;
    return elements.find((el) => matches(el, selector)) || null;
  }
});
function advance(ms) {
  const target = now + ms;
  for (let guard = 0; guard < 100000; guard++) {
    if (mutated) { mutated = false; for (const o of observers) o.cb([], o); continue; }
    let next = null;
    for (const t of timers.values()) {
      if (t.due <= target && (!next || t.due < next.due || (t.due === next.due && t.id < next.id))) next = t;
    }
    if (!next) break;
    timers.delete(next.id);
    now = next.due;
    next.fn();
  }
  now = target;
}
const sandbox = {
  document,
  location: { href: page.url },
  getComputedStyle: () => ({ display: 'block', visibility: 'visible' }),
  MutationObserver: class { constructor(cb) { this.cb = cb; observers.push(this); } observe() {} disconnect() {} },
  setTimeout: (fn, ms) => { const id = ++seq; timers.set(id, { id, due: now + (ms || 0), fn }); return id; },
  clearTimeout: (id) => { timers.delete(id); },
  Event: FakeEvent,
  EventTarget: FakeEventTarget,
  chrome: { webview: { postMessage: (message) => posts.push(String(message)) } }
};
sandbox.window = sandbox;
sandbox.top = sandbox;
sandbox.addEventListener = FakeEventTarget.prototype.addEventListener;
sandbox.dispatchEvent = FakeEventTarget.prototype.dispatchEvent;
vm.createContext(sandbox);
vm.runInContext(input.observer, sandbox);
for (const step of input.steps) {
  if ('advance' in step) advance(step.advance);
  else vm.runInContext(step.eval, sandbox);
}
const state = {};
for (const el of elements) {
  if (el.spec.key) state[el.spec.key] = { value: 'value' in el ? String(el.value) : null, clicks: el.clicks, events: el.events };
}
process.stdout.write(JSON.stringify({ posts, state, submits: form.submits }));
"#;

        struct DomRun {
            posts: Vec<String>,
            state: Value,
            submits: u64,
        }

        /// Corre o observador que embarca num DOM descrito por `page` e depois
        /// os `steps` (`{"advance": ms}` ou `{"eval": script}`), por ordem.
        /// O DOM é determinístico: a mesma página e os mesmos passos dão os
        /// mesmos ids, e é isso que deixa um teste observar numa corrida e
        /// executar noutra.
        fn run_page(page: &Value, steps: &[Value]) -> DomRun {
            let input = json!({
                "observer": AGENT_OBSERVER_SCRIPT.replace("__NEURALIA_CAP__", CAP),
                "page": page,
                "steps": steps,
            });
            let mut child = Command::new("node")
                .arg("-e")
                .arg(HARNESS)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("os gates do agente precisam do `node` no PATH (o CI já o usa)");
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(input.to_string().as_bytes())
                .expect("escrever o cenário");
            let output = child.wait_with_output().expect("node terminou");
            assert!(
                output.status.success(),
                "harness falhou: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: Value = serde_json::from_slice(&output.stdout).expect("JSON do harness");
            DomRun {
                posts: result["posts"]
                    .as_array()
                    .expect("posts")
                    .iter()
                    .map(|post| post.as_str().expect("post").to_string())
                    .collect(),
                state: result["state"].clone(),
                submits: result["submits"].as_u64().unwrap_or(0),
            }
        }

        /// O caminho nativo de uma mensagem publicada, igual ao do
        /// `external_webview_builder`: `None` é uma observação que o produto
        /// deita fora.
        fn observed(post: &str) -> Option<ObservedPage> {
            match parse_ipc_message(post, CAP, COMPARATOR_COLUMNS)? {
                IpcAction::AgentObservation { data } => parse_agent_observation(&data),
                _ => None,
            }
        }

        fn first_observation(page: &Value) -> ObservedPage {
            let run = run_page(page, &[json!({ "advance": 800 })]);
            let post = run.posts.first().expect("o observador publicou");
            observed(post).expect("a observação chegou ao nativo")
        }

        fn policy_for(origin: &str) -> AgentPermissionPolicy {
            let mut policy = AgentPermissionPolicy::new(Some(origin.into()));
            policy.grant_reversible_session_actions(true);
            policy
        }

        fn act(
            commands: &[BrowserAgentCommand],
            next: usize,
            page: &ObservedPage,
            policy: &mut AgentPermissionPolicy,
        ) -> AgentAct {
            match decide_agent_step(commands, next, 0, Duration::ZERO, page, policy) {
                AgentStepDecision::Act(act) => *act,
                other => panic!("esperava Act para {:?}, veio {other:?}", commands[next]),
            }
        }

        #[test]
        fn approved_action_still_finds_its_element_after_the_dialog() {
            // O MessageBox de confirmação é modal: o utilizador lê-o durante
            // segundos. O script aprovado tem de encontrar o mesmo elemento
            // depois disso, numa página que não mudou.
            let page = json!({
                "url": "https://site.example/",
                "title": "Contacto",
                "main": "Formulário de contacto",
                "elements": [
                    {"key": "send", "tag": "a", "attrs": {"role": "button", "href": "#"}, "text": "Enviar"}
                ]
            });
            let first = first_observation(&page);
            let mut policy = policy_for("https://site.example");
            let commands = [BrowserAgentCommand::Click("Enviar".into())];
            let act = act(&commands, 0, &first, &mut policy);
            assert!(act.confirmation.is_some(), "Enviar pede um sim: {act:?}");
            let script = agent_action_script(&act.action).expect("click executável");

            let run = run_page(
                &page,
                &[
                    json!({ "advance": 800 }),
                    json!({ "advance": 2000 }),
                    json!({ "eval": script }),
                ],
            );
            assert_eq!(
                run.posts.len(),
                1,
                "página parada não pode ser re-observada com ids novos"
            );
            assert_eq!(
                run.state["send"]["clicks"], 1,
                "o clique aprovado não chegou ao elemento"
            );
        }

        /// Observa, decide os `commands` sobre a primeira observação e corre os
        /// scripts resultantes, logo a seguir, na mesma página.
        fn plan_and_run(page: &Value, origin: &str, commands: &[BrowserAgentCommand]) -> DomRun {
            let first = first_observation(page);
            let mut policy = policy_for(origin);
            let mut steps = vec![json!({ "advance": 800 })];
            for next in 0..commands.len() {
                let act = act(commands, next, &first, &mut policy);
                let script = agent_action_script(&act.action).expect("executável");
                steps.push(json!({ "eval": script }));
            }
            run_page(page, &steps)
        }

        #[test]
        fn observer_and_guard_agree_on_ordinary_controls() {
            // `<input type=text>`, `<button>` e `<select>`: o observador dizia
            // textbox/button/select e o guard recalculava text/submit/select-one,
            // desistia, e o passo ficava no trace como feito.
            let page = json!({
                "url": "https://shop.example/",
                "title": "Loja",
                "main": "Produtos",
                "elements": [
                    {"key": "q", "tag": "input", "attrs": {"type": "text", "name": "q"}},
                    {"key": "go", "tag": "button", "text": "Buscar"},
                    {"key": "sort", "tag": "select", "attrs": {"name": "ordenar"}, "options": ["a", "preco"]}
                ]
            });
            let run = plan_and_run(
                &page,
                "https://shop.example",
                &[
                    BrowserAgentCommand::Search("rust".into()),
                    BrowserAgentCommand::Click("Buscar".into()),
                    BrowserAgentCommand::Select {
                        label: "ordenar".into(),
                        value: "preco".into(),
                    },
                ],
            );
            assert_eq!(run.state["q"]["value"], "rust", "texto não escrito");
            assert_eq!(run.state["go"]["clicks"], 1, "botão não clicado");
            assert_eq!(run.state["sort"]["value"], "preco", "select não mudou");
        }

        #[test]
        fn guard_accepts_combobox_textarea_and_placeholder_named_input() {
            // A caixa do Google (`<textarea role=combobox>`) e um campo cujo
            // único nome é o placeholder.
            for (field, expected) in [
                (
                    json!({"key": "f", "tag": "textarea", "attrs": {"role": "combobox", "name": "q", "aria-label": "Pesquisar"}}),
                    "rust",
                ),
                (
                    json!({"key": "f", "tag": "input", "attrs": {"type": "text", "placeholder": "Pesquisar"}}),
                    "rust",
                ),
            ] {
                let page = json!({
                    "url": "https://www.example.com/",
                    "title": "Busca",
                    "main": "",
                    "elements": [field]
                });
                let run = plan_and_run(
                    &page,
                    "https://www.example.com",
                    &[BrowserAgentCommand::Search("rust".into())],
                );
                assert_eq!(run.state["f"]["value"], expected, "{field}");
            }
        }

        #[test]
        fn observation_keeps_controls_on_text_heavy_pages() {
            // Um artigo com mais de ~1.1K caracteres de texto: o corte do
            // payload inteiro a 1200 unidades levava todas as linhas de
            // elementos, e click/search paravam com ElementMissing.
            let text = "Rust é uma linguagem de programação de sistemas. ".repeat(60);
            let page = json!({
                "url": "https://pt.wikipedia.org/wiki/Rust",
                "title": "Rust – Wikipédia",
                "main": text,
                "elements": [
                    {"key": "search", "tag": "input", "attrs": {"type": "search", "name": "search"}},
                    {"key": "edit", "tag": "a", "attrs": {"role": "button", "href": "#editar"}, "text": "Editar"}
                ]
            });
            let first = first_observation(&page);
            assert_eq!(first.elements.len(), 2, "{first:?}");
            assert!(
                first.text_excerpt.chars().count() >= 1000,
                "o extract continua a levar o texto: {}",
                first.text_excerpt.chars().count()
            );

            let mut policy = policy_for("https://pt.wikipedia.org");
            let commands = [
                BrowserAgentCommand::Click("Editar".into()),
                BrowserAgentCommand::Search("ownership".into()),
            ];
            let click = act(&commands, 0, &first, &mut policy);
            assert!(
                matches!(&click.action, AgentAction::Click { target } if target.name == "Editar")
            );
            let search = act(&commands, 1, &first, &mut policy);
            assert!(
                matches!(&search.action, AgentAction::TypeText { target, .. } if target.role == "search")
            );
        }

        #[test]
        fn spec_0108_agent_observation_stays_below_ipc_envelope_limit() {
            // O pior caso do JSON: cada unidade de controlo vira `\u0001`, seis
            // bytes. Nenhuma observação pode passar do envelope de 8 KiB, que o
            // nativo recusa por inteiro.
            let control = "\u{1}";
            let elements = (0..40)
                .map(|index| {
                    json!({
                        "key": format!("b{index}"),
                        "tag": "button",
                        "attrs": {"aria-label": format!("{index}{}", control.repeat(95))}
                    })
                })
                .collect::<Vec<_>>();
            let page = json!({
                "url": format!("https://example.com/{}", "a".repeat(1300)),
                "title": control.repeat(300),
                "main": control.repeat(2000),
                "elements": elements
            });
            let run = run_page(&page, &[json!({ "advance": 800 })]);
            assert!(!run.posts.is_empty());
            for post in &run.posts {
                assert!(
                    post.len() <= crate::ipc::IPC_MAX_BYTES,
                    "observação com {} bytes",
                    post.len()
                );
                assert!(observed(post).is_some(), "observação recusada pelo nativo");
            }
        }

        #[test]
        fn submit_button_in_a_form_waits_for_confirmation() {
            // `<button type=submit role=button>Salvar</button>`: nenhuma
            // palavra da lista, e o observador dizia `button`. O ramo
            // `role.contains("submit")` nunca disparava e o formulário seguia
            // como clique reversível, sem diálogo.
            let page = json!({
                "url": "https://example.com/settings",
                "title": "Definições",
                "main": "Preferências",
                "elements": [
                    {"key": "save", "tag": "button", "form": true, "attrs": {"type": "submit", "role": "button"}, "text": "Salvar"}
                ]
            });
            let first = first_observation(&page);
            let mut policy = policy_for("https://example.com");
            let commands = [BrowserAgentCommand::Click("salvar".into())];
            let act = act(&commands, 0, &first, &mut policy);
            assert!(
                act.confirmation.is_some(),
                "submit de formulário sem confirmação: {act:?}"
            );
            assert!(matches!(act.security, AgentSecurityAction::Submit { .. }));

            // Depois do sim, o guard continua a reconhecer o botão.
            let script = agent_action_script(&act.action).expect("click executável");
            let run = run_page(
                &page,
                &[json!({ "advance": 800 }), json!({ "eval": script })],
            );
            assert_eq!(run.submits, 1, "o submit aprovado não aconteceu");
        }

        #[test]
        fn emoji_on_a_cut_boundary_does_not_drop_the_observation() {
            // `slice` conta unidades UTF-16: um emoji na unidade 96 do nome
            // (ou 1600 do texto) deixava um surrogate alto sozinho, o JSON
            // levava `\ud83d`, o serde_json recusava e a observação sumia sem
            // erro nenhum.
            let label = format!("{}😀", "a".repeat(95));
            let page = json!({
                "url": "https://example.com/",
                "title": "Emoji",
                "main": format!("{}😀 fim", "b".repeat(1599)),
                "elements": [
                    {"key": "b", "tag": "button", "text": label}
                ]
            });
            let run = run_page(&page, &[json!({ "advance": 800 })]);
            let post = run.posts.first().expect("o observador publicou");
            let page = observed(post).expect("observação recusada pelo nativo");
            assert_eq!(page.elements.len(), 1);
            assert_eq!(page.elements[0].name, "a".repeat(95));
            assert!(page.text_excerpt.starts_with("bbbb"));
        }

        #[test]
        fn search_never_types_into_a_submit_input() {
            // `<input type=submit>` era `textbox`: o search escrevia no botão.
            let page = json!({
                "url": "https://example.com/",
                "title": "Busca",
                "main": "",
                "elements": [
                    {"key": "go", "tag": "input", "form": true, "attrs": {"type": "submit", "name": "q", "value": "Buscar"}}
                ]
            });
            let first = first_observation(&page);
            let mut policy = policy_for("https://example.com");
            assert_eq!(
                decide_agent_step(
                    &[BrowserAgentCommand::Search("rust".into())],
                    0,
                    0,
                    Duration::ZERO,
                    &first,
                    &mut policy,
                ),
                AgentStepDecision::Stop(AgentTermination::ElementMissing)
            );
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
    fn capability_falls_back_to_the_secondary_windows_csprng() {
        let expected = [0xabu8; 16];
        let token = capability_from_sources(-1, [0u8; 16], |output| {
            *output = expected;
            true
        })
        .expect("fallback valido");
        assert_eq!(token, "ab".repeat(16));

        assert!(
            capability_from_sources(-1, [0u8; 16], |_| false).is_none(),
            "se os dois CSPRNG falham, o canal deve falhar fechado"
        );
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

        // Nenhum handler que dispare acao nativa aceita evento sintetico:
        // os 4 de sempre + os 4 da pergunta replicada (focusin, Enter,
        // botao de enviar, submit).
        assert_eq!(
            COMPARATOR_INJECT_SCRIPT
                .matches("if (!event.isTrusted")
                .count(),
            8
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
        assert!(AGENT_OBSERVER_SCRIPT.contains("post(envelope('agent-observation'"));
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
    fn a_question_typed_in_one_column_goes_to_the_others() {
        match App::column_ipc_event_impl(
            1,
            IpcAction::Ask {
                col: 1,
                text: "capital da França".to_string(),
            },
        ) {
            Some(UserEvent::AskEverywhere { source_index, text }) => {
                assert_eq!(source_index, 1);
                assert_eq!(text, "capital da França");
            }
            other => panic!("a pergunta devia ir as outras colunas, veio {other:?}"),
        }
        // Uma pagina nao fala por outra coluna.
        assert!(
            App::column_ipc_event_impl(
                1,
                IpcAction::Ask {
                    col: 0,
                    text: "x".to_string()
                }
            )
            .is_none()
        );
        // A origem nao e tocada; as outras sim.
        assert_eq!(ask_targets(1, 3), vec![0, 2]);
        assert_eq!(ask_targets(0, 3), vec![1, 2]);
        assert_eq!(ask_targets(2, 3), vec![0, 1]);
    }

    #[test]
    fn typing_and_sending_in_a_column_searches_in_all_of_them() {
        // Corre o script das colunas QUE EMBARCA e le o que ele publica pelo
        // parser nativo do produto.
        const CAP: &str = "0123456789abcdef0123456789abcdef";
        let provider = r#"
const box = document.createElement('textarea');
box.value = '  capital da França  ';
const at = (node) => ({ target: node, composedPath() { return [node]; } });
__fire('keydown', Object.assign({ key: 'Enter' }, at(box)));
// o submit/Enter repetido da mesma pergunta nao duplica
__fire('keydown', Object.assign({ key: 'Enter' }, at(box)));
// Shift+Enter e uma quebra de linha
box.value = 'outra coisa';
__fire('keydown', Object.assign({ key: 'Enter', shiftKey: true }, at(box)));
// senha nunca
const pw = document.createElement('input'); pw.type = 'password'; pw.value = 'segredo';
__fire('keydown', Object.assign({ key: 'Enter' }, at(pw)));
// botao de enviar com o texto da ultima caixa focada
const composer = document.createElement('textarea'); composer.value = 'segunda pergunta';
__fire('focusin', at(composer));
const toggle = document.createElement('button'); toggle.setAttribute('aria-label', 'Pesquisar na web');
__fire('click', at(toggle));
const send = document.createElement('button'); send.setAttribute('aria-label', 'Enviar mensagem');
__fire('click', at(send));
// evento sintetico da propria pagina: ignorado
__fire('keydown', Object.assign({ key: 'Enter', isTrusted: false }, at(composer)));
"#;
        let ai_mode_form = r#"
const q = document.createElement('textarea'); q.name = 'q'; q.value = 'nova pergunta';
const form = document.createElement('form'); form.elements = [q];
__fire('submit', { target: form, composedPath() { return [form]; } });
"#;
        let site = r#"
const at = (node) => ({ target: node, composedPath() { return [node]; } });
const q = document.createElement('input'); q.type = 'search'; q.name = 'q'; q.value = 'rust async';
// Enter num site qualquer nao e pergunta a IA
__fire('keydown', Object.assign({ key: 'Enter' }, at(q)));
const lang = document.createElement('input'); lang.type = 'hidden'; lang.name = 'lang'; lang.value = 'pt';
const form = document.createElement('form'); form.setAttribute('action', '/search'); form.elements = [q, lang];
__fire('submit', at(form));
const post = document.createElement('form'); post.setAttribute('method', 'post'); post.elements = [q];
__fire('submit', at(post));
const pw = document.createElement('input'); pw.type = 'password'; pw.name = 'p'; pw.value = 's';
const login = document.createElement('form'); login.elements = [q, pw];
__fire('submit', at(login));
"#;
        let script = COMPARATOR_INJECT_SCRIPT.replace("__NEURALIA_CAP__", CAP);
        let cases: Vec<serde_json::Value> = [
            ("chatgpt", "https://chatgpt.com/c/abc", provider),
            (
                "ai-mode",
                "https://www.google.com/search?q=x&udm=50",
                ai_mode_form,
            ),
            ("site", "https://example.com/artigo", site),
        ]
        .into_iter()
        .map(|(name, href, drive)| {
            serde_json::json!({ "name": name, "href": href, "script": script, "drive": drive })
        })
        .collect();
        let program = format!(
            "const INPUT = {};\n{}",
            serde_json::json!({ "cases": cases }),
            INJECTED_SCRIPT_HARNESS
        );
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&run_node_program(&program)).expect("harness json");
        let actions = |index: usize| -> Vec<IpcAction> {
            results[index]["posted"]
                .as_array()
                .expect("posted")
                .iter()
                .filter_map(|message| parse_ipc_message(message.as_str()?, CAP, 3))
                .filter(|action| matches!(action, IpcAction::Ask { .. } | IpcAction::Link { .. }))
                .collect()
        };
        let ask = |text: &str| IpcAction::Ask {
            col: 0,
            text: text.to_string(),
        };
        assert_eq!(
            actions(0),
            vec![ask("capital da França"), ask("segunda pergunta")],
            "erros: {}",
            results[0]["errors"]
        );
        assert_eq!(
            actions(1),
            vec![ask("nova pergunta")],
            "erros: {}",
            results[1]["errors"]
        );
        assert_eq!(
            actions(2),
            vec![IpcAction::Link {
                col: 0,
                url: "https://example.com/search?q=rust+async&lang=pt".to_string(),
                aside: false,
            }],
            "erros: {}",
            results[2]["errors"]
        );
    }

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
    fn comparator_popup_failure_never_falls_back_to_destroying_all_panels() {
        let source = include_str!("windows_app.rs");
        let body = source
            .split("fn open_in_column")
            .nth(1)
            .and_then(|part| part.split("fn open_everywhere").next())
            .expect("open_in_column body");

        // Fora do comparador, um popup ainda pode abrir como Web normal.
        // Dentro dele, porém, uma falha de load_url deve ficar isolada à
        // coluna. Um segundo self.web(url) reintroduziria o teardown das três
        // colunas por causa de um único clique.
        assert_eq!(body.matches("self.web(url)").count(), 1);
        assert!(body.contains("load_url(valid.as_str())"));
        assert!(body.contains("sem perder a comparação"));
    }

    #[test]
    fn lifecycle_probe_commands_are_deduplicated_by_nonce() {
        LIFECYCLE_LAST_HOME_NONCE.store(0, Ordering::Release);
        LIFECYCLE_LAST_REOPEN_NONCE.store(0, Ordering::Release);

        assert_ne!(LIFECYCLE_LAST_HOME_NONCE.swap(7, Ordering::AcqRel), 7);
        assert_eq!(LIFECYCLE_LAST_HOME_NONCE.swap(7, Ordering::AcqRel), 7);
        assert_ne!(LIFECYCLE_LAST_HOME_NONCE.swap(8, Ordering::AcqRel), 8);

        assert_ne!(LIFECYCLE_LAST_REOPEN_NONCE.swap(9, Ordering::AcqRel), 9);
        assert_eq!(LIFECYCLE_LAST_REOPEN_NONCE.swap(9, Ordering::AcqRel), 9);
        assert_ne!(LIFECYCLE_LAST_REOPEN_NONCE.swap(10, Ordering::AcqRel), 10);
    }

    #[test]
    fn lifecycle_ready_is_published_only_after_returning_to_the_event_loop() {
        let source = include_str!("windows_app.rs");
        let activate = source
            .split("fn activate_comparator")
            .nth(1)
            .and_then(|part| part.split("fn expand_comparator").next())
            .expect("activate_comparator body");
        assert!(
            !activate.contains("LIFECYCLE_COMPARATOR_READY.store(true"),
            "activate_comparator ainda pode estar dentro do pump aninhado do WebView2"
        );

        let relayout = source
            .split("UserEvent::RelayoutComparator =>")
            .nth(1)
            .and_then(|part| part.split("UserEvent::RestoreHomeDecorations =>").next())
            .expect("RelayoutComparator handler");
        assert!(
            !relayout.contains("LIFECYCLE_COMPARATOR_READY.store(true"),
            "timer de relayout nao prova que o pump do WebView2 devolveu o controlo"
        );

        let idle = source
            .split("fn about_to_wait")
            .nth(1)
            .and_then(|part| part.split("fn user_event").next())
            .expect("about_to_wait body");
        let rebind = idle.find("self.ensure_window_subclass()").expect("rebind");
        let ready = idle
            .find("LIFECYCLE_COMPARATOR_READY.store(true")
            .expect("Ready publish");
        assert!(ready > rebind);
    }

    #[test]
    fn webview_teardown_does_not_schedule_home_chrome_while_opening_comparator() {
        let source = include_str!("windows_app.rs");
        let destroy = source
            .split("fn destroy_web_surfaces")
            .nth(1)
            .and_then(|part| part.split("fn schedule_home_restoration").next())
            .expect("destroy_web_surfaces body");
        assert!(!destroy.contains("UserEvent::RestoreHomeDecorations"));

        let home = source
            .split("fn show_home")
            .nth(1)
            .and_then(|part| part.split("fn show_native_error").next())
            .expect("show_home body");
        assert!(home.contains("self.schedule_home_restoration()"));

        let comparator = source
            .split("fn open_comparator")
            .nth(1)
            .and_then(|part| part.split("fn activate_comparator").next())
            .expect("open_comparator body");
        assert!(!comparator.contains("schedule_home_restoration"));

        let idle = source
            .split("fn about_to_wait")
            .nth(1)
            .and_then(|part| part.split("fn user_event").next())
            .expect("about_to_wait body");
        assert!(idle.contains("self.ensure_window_subclass()"));
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
        // Os provedores usam React, popovers e Shadow DOM. O interceptador tem
        // de chegar antes dos handlers de document e descobrir o link real no
        // composed path; depois que assume um link externo, nenhum listener do
        // site pode disparar uma segunda navegacao concorrente.
        assert!(COMPARATOR_INJECT_SCRIPT.contains("listen(window, 'click'"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("listen(window, 'auxclick'"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("event.composedPath"));
        assert!(COMPARATOR_INJECT_SCRIPT.contains("event.stopImmediatePropagation()"));
        let route_link = COMPARATOR_INJECT_SCRIPT
            .split("function routeLink")
            .nth(1)
            .and_then(|part| part.split("listen(window, 'click'").next())
            .expect("routeLink body");
        assert!(!route_link.contains("event.defaultPrevented"));
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
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("neuralia:pending-query:"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("if (host === 'claude.ai') return false"));
        assert!(AI_AUTO_SUBMIT_SCRIPT.contains("storageRemove(pendingKey)"));
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
        assert!(matches!(
            App::split_ipc_event_impl(1, false, IpcAction::Palette { col: 1 }),
            Some(UserEvent::OpenPalette(1))
        ));
        assert!(App::split_ipc_event_impl(1, false, IpcAction::Palette { col: 0 }).is_none());

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
        assert!(
            before.contains("open_split_mode(source_index, url.to_string(), false, true, None)")
        );

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
        assert!(split.contains("let context_id = if private"));
        assert!(split.contains("remember_context_tab("));
        assert!(!split.contains("self.record("));
        let private_split = source
            .split("UserEvent::OpenPrivateSplit { source_index, url } =>")
            .nth(1)
            .and_then(|part| part.split("UserEvent::NewTab").next())
            .expect("OpenPrivateSplit arm");
        assert!(private_split.contains("open_split_mode(source_index, url, false, true, None)"));
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
        assert_eq!(narrow.width, 52.0);
        assert_eq!(narrow.x, 724.0);
        assert!(narrow.width <= 100.0);

        assert!(palette_hint("ChatGPT", false).contains("ChatGPT"));
        assert!(palette_hint("ChatGPT", true).contains("privado"));
    }

    #[test]
    fn duplicate_urls_keep_distinct_tab_identity_across_groups() {
        let mut tabs = vec![
            tab("https://example.com/same", Some(10)),
            tab("https://example.com/same", Some(20)),
        ];
        let first = tabs[0].id;
        let second = tabs[1].id;
        assert_ne!(first, second, "URL repetida nao pode colapsar identidades");

        assert!(
            !active_context_removed_by_scope(&tabs, 0, Some(second), false),
            "mesma URL noutro grupo nao pode ser confundida com a aba ativa do grupo fechado"
        );
        assert!(
            active_context_removed_by_scope(&tabs, 0, Some(first), false),
            "a aba ativa do proprio grupo precisa ser fechada"
        );
        assert!(
            !active_context_removed_by_scope(&tabs, 0, Some(first), true),
            "Fechar outras deve preservar a aba selecionada"
        );

        let mut groups = vec![group(10, false), group(20, false)];
        assert!(close_context_tab_scope(&mut tabs, &mut groups, 0));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, second);
        assert_eq!(tabs[0].url, "https://example.com/same");
        assert_eq!(tabs[0].group, Some(20));
    }

    #[test]
    fn context_limit_prunes_group_orphaned_by_eviction() {
        let mut tabs = vec![tab("https://old.example/", Some(77))];
        let mut groups = vec![group(77, false)];
        let mut next_id = 10_000;
        for index in 0..32 {
            let _ = remember_context_tab(
                &mut tabs,
                &mut groups,
                &mut next_id,
                format!("https://example.com/{index}"),
            );
        }
        assert_eq!(tabs.len(), 32);
        assert!(
            groups.iter().all(|item| item.id != 77),
            "o limite de abas deixou grupo sem membro"
        );
    }

    #[test]
    fn rejected_minimize_keeps_last_visible_panel_state_intact() {
        assert!(!can_minimize_column(&[true, false, true], 3, 1));
        assert!(can_minimize_column(&[false, false, true], 3, 1));
        assert!(!can_minimize_column(&[false, false, true], 3, 9));
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
        assert!(GMAIL_MONITOR_SCRIPT.contains("post(envelope('gmail-state'"));
        assert!(GMAIL_MONITOR_SCRIPT.contains("post(envelope("));
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

    /// Corre `program` no Node (o mesmo motor de JS que os testes de CI dos
    /// scripts injetados usam) e devolve o stdout. Sem Node nao ha gate: falha.
    fn run_node_program(program: &str) -> String {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = Command::new("node")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("node is required to run the injected-script gates");
        child
            .stdin
            .take()
            .expect("node stdin")
            .write_all(program.as_bytes())
            .expect("write program to node");
        let output = child.wait_with_output().expect("node output");
        assert!(
            output.status.success(),
            "node failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("node stdout is utf-8")
    }

    /// Um DOM minimo, criado DENTRO do contexto do vm, para que os literais de
    /// objeto dos scripts herdem do `Object.prototype` que a "pagina" envenena.
    const INJECTED_SCRIPT_HARNESS: &str = r##"
const vm = require('node:vm');
const MOCK = `
var __stolen = [], __posted = [], __errors = [], __listeners = [], __timers = [], __observers = [], __created = [];
class EventTarget {
  addEventListener(type, handler) { __listeners.push({ target: this, type: String(type), handler }); }
  removeEventListener() {}
  dispatchEvent() { return true; }
}
class Node extends EventTarget {
  appendChild(child) { return child; }
  removeChild(child) { return child; }
  insertBefore(child) { return child; }
}
class Element extends Node {
  constructor(tag) {
    super();
    this.tagName = String(tag || 'div').toUpperCase();
    this.style = {}; this.dataset = {}; this.attrs = {}; this.children = [];
    this.classList = { add() {}, remove() {}, toggle() {}, contains() { return false; } };
    this.textContent = ''; this.innerText = 'x'.repeat(40); this.value = '';
  }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  getAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attrs, k) ? this.attrs[k] : null; }
  removeAttribute(k) { delete this.attrs[k]; }
  hasAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attrs, k); }
  querySelector() { return null; }
  querySelectorAll() { return []; }
  closest() { return null; }
  matches() { return false; }
  getBoundingClientRect() { return { x: 0, y: 0, top: 0, left: 0, right: 10, bottom: 10, width: 10, height: 10 }; }
  focus() {} blur() {} select() {} remove() {} click() {} scrollIntoView() {} append() {} prepend() {}
}
class Document extends Node {
  constructor() {
    super();
    this.readyState = 'loading'; this.title = '';
    this.documentElement = new Element('html'); this.body = new Element('body'); this.head = new Element('head');
  }
  getElementById() { return null; }
  createElement(tag) { __created.push(String(tag)); return new Element(tag); }
  createTextNode(text) { return { textContent: String(text) }; }
  querySelector() { return null; }
  querySelectorAll() { return []; }
}
var document = new Document();
var window = new EventTarget();
window.top = window;
window.location = location;
window.history = { back() {}, forward() {} };
window.chrome = { webview: { postMessage(message) { __posted.push(String(message)); } } };
window.__neuralia_col_index = 0;
window.__neuralia_col_name = 'IA';
function __timer(fn) { const t = { fn, done: false }; __timers.push(t); return __timers.length; }
function __cancel(id) { const t = __timers[id - 1]; if (t) t.done = true; }
var setTimeout = __timer, setInterval = __timer, requestAnimationFrame = __timer;
var clearTimeout = __cancel, clearInterval = __cancel, cancelAnimationFrame = __cancel;
var getComputedStyle = () => ({ display: 'block', visibility: 'visible' });
class MutationObserver {
  constructor(callback) { this.callback = callback; __observers.push(this); }
  observe() {} disconnect() {} takeRecords() { return []; }
}
`;
const HELPERS = `
function __event(type, extra) {
  const target = new Element('div');
  return Object.assign({
    type, isTrusted: true, defaultPrevented: false, button: 0, key: '',
    ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, target,
    composedPath() { return [target]; },
    preventDefault() {}, stopPropagation() {}, stopImmediatePropagation() {}
  }, extra || {});
}
function __fire(type, extra) {
  for (const l of __listeners.slice()) {
    if (l.type !== type) continue;
    try {
      const h = l.handler;
      (typeof h === 'function' ? h : h.handleEvent).call(l.target, __event(type, extra));
    } catch (e) { __errors.push(type + ': ' + e.message); }
  }
}
function __drain() {
  for (let round = 0; round < 20; round++) {
    const due = __timers.filter((t) => !t.done);
    if (!due.length) return;
    for (const t of due) {
      t.done = true;
      try { t.fn(); } catch (e) { __errors.push('timer: ' + e.message); }
    }
  }
}
`;
const DEFAULT_DRIVE = `
document.readyState = 'interactive';
__fire('DOMContentLoaded');
__fire('load');
__drain();
__fire('keydown', { key: 'Escape' });
__fire('click');
__fire('dblclick');
__fire('neuralia-agent-rescan');
for (const o of __observers) { try { o.callback([], o); } catch (e) { __errors.push('observer: ' + e.message); } }
__drain();
`;
// O que a pagina corre depois do document-created: um getter de toJSON no
// Object.prototype que guarda qualquer `cap` que lhe passe por `this`.
const PAGE = `
Object.defineProperty(Object.prototype, 'toJSON', {
  configurable: true,
  get() { if (this && typeof this.cap === 'string') __stolen.push(this.cap); return undefined; }
});
`;
const results = [];
for (const c of INPUT.cases) {
  // `steps`: cada passo e uma avaliacao propria e as promessas resolvidas
  // num passo (o clipboard, por exemplo) correm antes do seguinte.
  const context = vm.createContext(
    { location: new URL(c.href), URL },
    c.steps ? { microtaskMode: 'afterEvaluate' } : {}
  );
  vm.runInContext(MOCK, context);
  // `pre`: o resto do "navegador" que um caso precisa, antes do script.
  if (c.pre) vm.runInContext(c.pre, context);
  if (c.child) vm.runInContext('window.top = {};', context);
  vm.runInContext(c.script, context, { filename: c.name });
  vm.runInContext(PAGE, context);
  vm.runInContext(HELPERS, context);
  if (c.steps) {
    // Um passo que falha fica nos erros do caso, que o teste le.
    for (const step of c.steps) {
      try { vm.runInContext(step, context); } catch (e) { context.__errors.push('passo: ' + e.message); }
    }
  } else {
    vm.runInContext(c.drive || DEFAULT_DRIVE, context);
  }
  results.push({
    name: c.name,
    stolen: Array.from(context.__stolen, String),
    posted: Array.from(context.__posted, String),
    errors: Array.from(context.__errors, String),
    created: Array.from(context.__created, String),
    log: Array.from(context.__log || [], String),
  });
}
process.stdout.write(JSON.stringify(results));
"##;

    #[test]
    fn spec_0108_page_cannot_read_the_capability_through_a_to_json_getter() {
        // Um getter de `toJSON` no Object.prototype e chamado pelo
        // JSON.stringify com `this` = cada objeto serializado. Se o envelope
        // com o token passar por la, a pagina fica com o token e forja
        // `clearhistory`. O gate corre os cinco scripts que embarcam, dispara
        // os caminhos que postam e exige: ha mensagens, sao validas, e o
        // getter da pagina nunca viu o token.
        const CAP: &str = "0123456789abcdef0123456789abcdef";
        let cases: Vec<serde_json::Value> = [
            ("keymap", NEURALIA_KEYMAP_SCRIPT, "https://example.com/"),
            ("return", EXTERNAL_RETURN_BUTTON, "https://example.com/"),
            (
                "gmail",
                GMAIL_MONITOR_SCRIPT,
                "https://mail.google.com/mail/u/0/",
            ),
            ("agent", AGENT_OBSERVER_SCRIPT, "https://example.com/"),
            (
                "comparator",
                COMPARATOR_INJECT_SCRIPT,
                "https://example.com/",
            ),
        ]
        .into_iter()
        .map(|(name, script, href)| {
            serde_json::json!({
                "name": name,
                "href": href,
                "script": script.replace("__NEURALIA_CAP__", CAP),
            })
        })
        .collect();
        let program = format!(
            "const INPUT = {};\n{}",
            serde_json::json!({ "cases": cases }),
            INJECTED_SCRIPT_HARNESS
        );
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&run_node_program(&program)).expect("harness json");
        assert_eq!(results.len(), 5);
        for result in &results {
            let name = result["name"].as_str().unwrap_or_default();
            let stolen = result["stolen"].as_array().expect("stolen");
            let posted = result["posted"].as_array().expect("posted");
            assert!(
                !posted.is_empty(),
                "{name}: the harness must reach a signing path; errors: {}",
                result["errors"]
            );
            for message in posted {
                let message = message.as_str().expect("posted string");
                assert!(
                    parse_ipc_message(message, CAP, 3).is_some(),
                    "{name}: posted envelope must stay valid: {message}"
                );
            }
            assert!(
                stolen.is_empty(),
                "{name}: page toJSON getter read the capability {} time(s)",
                stolen.len()
            );
        }
    }

    #[test]
    fn ctrl_r_toggles_auto_scroll_and_reload_stays_on_f5_and_ctrl_shift_r() {
        // Corre o mapa de teclas QUE EMBARCA e le o que ele publica pelo
        // mesmo parser nativo do produto.
        const CAP: &str = "0123456789abcdef0123456789abcdef";
        let drive = r#"
document.readyState = 'interactive';
__fire('DOMContentLoaded');
__drain();
__fire('keydown', { key: 'r', ctrlKey: true });
__fire('keydown', { key: 'R', ctrlKey: true, shiftKey: true });
__fire('keydown', { key: 'F5' });
__fire('keydown', { key: 'F8' });
"#;
        let cases = [serde_json::json!({
            "name": "keymap",
            "href": "https://example.com/",
            "script": NEURALIA_KEYMAP_SCRIPT.replace("__NEURALIA_CAP__", CAP),
            "drive": drive,
        })];
        let program = format!(
            "const INPUT = {};\n{}",
            serde_json::json!({ "cases": cases }),
            INJECTED_SCRIPT_HARNESS
        );
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&run_node_program(&program)).expect("harness json");
        let actions: Vec<IpcAction> = results[0]["posted"]
            .as_array()
            .expect("posted")
            .iter()
            .filter_map(|message| parse_ipc_message(message.as_str()?, CAP, 3))
            .collect();
        assert_eq!(
            actions,
            vec![
                IpcAction::AutoScroll,
                IpcAction::Reload,
                IpcAction::Reload,
                IpcAction::AutoScroll,
            ],
            "erros: {}",
            results[0]["errors"]
        );
    }

    const SELECTION_CAP: &str = "0123456789abcdef0123456789abcdef";

    /// O resto do "navegador" que a barra de selecao usa, sobre o DOM minimo
    /// do harness: arvore com pais, `closest`, shadow root, Selection/Range,
    /// clipboard, `execCommand` e `speechSynthesis`. Corre antes do script,
    /// como o navegador que ele encontra no document-created; os `__` sao
    /// os gestos do utilizador e as leituras do teste.
    const SELECTION_DOM: &str = r##"
var __log = [], __clipboard = [], __exec = [], __spoken = [], __shadows = [], __prevented = [];
var __cancels = 0, __clipboardFails = false, __voices = [];
const __json = JSON.stringify;
class CSSStyleDeclaration {
  setProperty(name, value, priority) {
    this[String(name)] = String(value);
    this['!' + String(name)] = String(priority || '');
  }
}
class CSSStyleSheet { replaceSync(css) { this.css = String(css); } }
class ShadowRoot extends Node {
  constructor(host, mode) { super(); this.host = host; this.mode = mode; this.adoptedStyleSheets = []; }
}
class Text extends Node {
  constructor(data) { super(); this.data = String(data); }
}
Object.defineProperty(Node.prototype, 'nodeType', {
  configurable: true,
  get() { return this instanceof Element ? 1 : this instanceof Text ? 3 : this instanceof Document ? 9 : 11; }
});
Object.defineProperty(Node.prototype, 'textContent', {
  configurable: true,
  get() { return this instanceof Text ? this.data : (this.__text || ''); },
  set(value) { this.__text = String(value); }
});
const __textOf = Object.getOwnPropertyDescriptor(Node.prototype, 'textContent').get;
Node.prototype.appendChild = function (child) {
  const old = child.parentNode;
  if (old && old.childNodes) {
    const at = old.childNodes.indexOf(child);
    if (at >= 0) old.childNodes.splice(at, 1);
  }
  child.parentNode = this;
  if (!this.childNodes) this.childNodes = [];
  this.childNodes.push(child);
  return child;
};
Node.prototype.contains = function (other) {
  for (let node = other; node; node = node.parentNode) { if (node === this) return true; }
  return false;
};
Object.defineProperty(Node.prototype, 'parentElement', {
  configurable: true,
  get() { return this.parentNode instanceof Element ? this.parentNode : null; }
});
Object.defineProperty(Node.prototype, 'isConnected', {
  configurable: true,
  get() {
    for (let node = this; node; node = node.parentNode || node.host) { if (node === document) return true; }
    return false;
  }
});
function __matchOne(el, selector) {
  const s = String(selector).trim();
  let m = /^\[([\w-]+)="([^"]*)"\]$/.exec(s);
  if (m) return el.getAttribute(m[1]) === m[2];
  m = /^#([\w-]+)$/.exec(s);
  if (m) return el.id === m[1] || el.getAttribute('id') === m[1];
  if (/^[a-z]+$/i.test(s)) return el.tagName === s.toUpperCase();
  throw new Error('seletor fora do mock: ' + s);
}
Element.prototype.matches = function (selector) {
  return String(selector).split(',').some((part) => __matchOne(this, part));
};
Element.prototype.closest = function (selector) {
  for (let node = this; node instanceof Element; node = node.parentNode) {
    if (node.matches(selector)) return node;
  }
  return null;
};
Object.defineProperty(Element.prototype, 'isContentEditable', {
  configurable: true,
  get() {
    for (let node = this; node instanceof Element; node = node.parentNode) {
      const value = node.getAttribute('contenteditable');
      if (value === '' || value === 'true' || value === 'plaintext-only') return true;
      if (value === 'false') return false;
    }
    return false;
  }
});
Element.prototype.attachShadow = function (init) {
  const root = new ShadowRoot(this, init && init.mode);
  __shadows.push(root);
  this.__shadow = root;
  if (init && init.mode === 'open') this.shadowRoot = root;
  return root;
};
// A barra mede 300x42 quando esta a vista; escondida nao tem caixa.
const __box = Element.prototype.getBoundingClientRect;
Element.prototype.getBoundingClientRect = function () {
  if (!this.__shadow) return __box.call(this);
  const on = this.style.display !== 'none';
  const w = on ? 300 : 0, h = on ? 42 : 0;
  return { x: 0, y: 0, top: 0, left: 0, right: w, bottom: h, width: w, height: h };
};
const __makeElement = Document.prototype.createElement;
Document.prototype.createElement = function (tag) {
  const el = __makeElement.call(this, tag);
  el.style = new CSSStyleDeclaration();
  return el;
};
document.documentElement.parentNode = document;
document.documentElement.childNodes = [document.head, document.body];
document.head.parentNode = document.documentElement;
document.body.parentNode = document.documentElement;
document.documentElement.lang = '';
// 1000x700 com uma barra de rolagem de 10 px a direita.
document.documentElement.clientWidth = 990;
document.documentElement.clientHeight = 700;
document.activeElement = document.body;
window.innerWidth = 1000;
window.innerHeight = 700;

var __sel = { text: '', anchor: null, focus: null, common: null, rects: [], collapsed: true };
class Range {
  getClientRects() { return __sel.rects.slice(); }
  getBoundingClientRect() {
    const r = __sel.rects;
    if (!r.length) return { top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0 };
    const top = Math.min(...r.map((x) => x.top)), bottom = Math.max(...r.map((x) => x.bottom));
    const left = Math.min(...r.map((x) => x.left)), right = Math.max(...r.map((x) => x.right));
    return { top, bottom, left, right, width: right - left, height: bottom - top };
  }
  get commonAncestorContainer() { return __sel.common; }
}
class Selection {
  toString() { return __sel.text; }
  getRangeAt(index) {
    if (index !== 0 || !__sel.anchor) throw new Error('IndexSizeError');
    return new Range();
  }
  get rangeCount() { return __sel.anchor ? 1 : 0; }
  get isCollapsed() { return __sel.collapsed; }
  get anchorNode() { return __sel.anchor; }
  get focusNode() { return __sel.focus; }
}
const __selection = new Selection();
Document.prototype.getSelection = function () { return __selection; };
window.getSelection = function () { return __selection; };

class Clipboard {
  writeText(text) {
    __clipboard.push(String(text));
    return __clipboardFails ? Promise.reject(new Error('negado')) : Promise.resolve();
  }
}
var navigator = { language: 'en-US', clipboard: new Clipboard() };
Document.prototype.execCommand = function (command) { __exec.push(String(command)); return true; };
class SpeechSynthesisUtterance extends EventTarget {
  constructor(text) { super(); this.text = String(text); this.voice = null; this.lang = ''; }
}
class SpeechSynthesis extends EventTarget {
  speak(utterance) { __spoken.push(utterance); }
  cancel() { __cancels++; }
  getVoices() { return __voices.slice(); }
}
window.speechSynthesis = new SpeechSynthesis();
window.SpeechSynthesisUtterance = SpeechSynthesisUtterance;
var __NUVEM = { name: 'Nuvem', lang: 'pt-BR', localService: false, default: false };
var __MARIA = { name: 'Maria', lang: 'pt-BR', localService: true, default: false };
var __ZIRA = { name: 'Zira', lang: 'en-US', localService: true, default: true };
var __HELENA = { name: 'Helena', lang: 'es-ES', localService: true, default: false };

function __el(tag, attrs, parent) {
  const el = document.createElement(tag);
  for (const key of Object.keys(attrs || {})) el.setAttribute(key, attrs[key]);
  (parent || document.body).appendChild(el);
  return el;
}
function __textIn(parent, value) { return parent.appendChild(new Text(value)); }
var __para = __textIn(__el('p'), 'Um paragrafo da pagina.');
var __line = { top: 300, bottom: 320, left: 100, right: 400, width: 300, height: 20 };
function __select(text, node, extra) {
  const at = node || __para;
  Object.assign(__sel, {
    text: String(text), anchor: at, focus: at, common: at,
    collapsed: String(text) === '', rects: [__line]
  }, extra || {});
}
function __unselect() { Object.assign(__sel, { text: '', collapsed: true, rects: [] }); }
// Um evento so para os ouvintes deste alvo (o __fire do harness chama todos).
function __on(target, type, extra) {
  for (const l of __listeners.slice()) {
    if (l.target !== target || l.type !== type) continue;
    try {
      const h = l.handler;
      (typeof h === 'function' ? h : h.handleEvent).call(target, __event(type, extra));
    } catch (e) { __errors.push(type + ': ' + e.message); }
  }
}
function __up(extra) { __on(window, 'mouseup', Object.assign({ target: document.body }, extra)); }
function __show(text, node, extra) { __select(text, node, extra); __up(); __drain(); }
function __root() { return __shadows[__shadows.length - 1] || null; }
function __kids() {
  const root = __root();
  const bar = root && (root.childNodes || []).find((n) => n.getAttribute && n.getAttribute('class') === 'bar');
  return bar ? bar.childNodes || [] : [];
}
function __button(action) {
  return __kids().find((n) => n.tagName === 'BUTTON' && n.getAttribute('data-action') === action) || null;
}
// Um clique num botao da barra: fora da shadow root o alvo e o host.
function __press(action, extra) {
  const host = __root().host;
  const b = __button(action);
  if (!b) throw new Error('sem o botao ' + action);
  const mark = (where) => ({ preventDefault() { __prevented.push(where); } });
  __on(window, 'mousedown', Object.assign({ target: host }, mark('window'), extra));
  __on(b, 'mousedown', Object.assign({ target: b }, mark('botao'), extra));
  __on(window, 'mouseup', Object.assign({ target: host }, extra));
  __on(b, 'click', Object.assign({ target: b }, extra));
}
function __utterEnd() { __on(__spoken[__spoken.length - 1], 'end'); }
function __voicesChanged() { __on(window.speechSynthesis, 'voiceschanged'); }
function __state(tag) {
  const root = __root();
  const host = root ? root.host : null;
  const kids = __kids();
  const buttons = kids.filter((n) => n.tagName === 'BUTTON');
  const note = kids.find((n) => n.getAttribute && n.getAttribute('role') === 'status');
  __log.push(__json({
    tag: tag,
    shown: !!host && host.style.display === 'block' && host.isConnected,
    mode: root ? root.mode : null,
    sheets: root ? root.adoptedStyleSheets.length : 0,
    position: host ? host.style.position || '' : '',
    zIndex: host ? host.style['z-index'] || '' : '',
    top: host ? parseFloat(host.style.top) : null,
    left: host ? parseFloat(host.style.left) : null,
    buttons: buttons.map((n) => __textOf.call(n)),
    tabindex: buttons.map((n) => n.getAttribute('tabindex')),
    note: note && note.getAttribute('class') === 'msg on' ? __textOf.call(note) : '',
    pending: __timers.filter((t) => !t.done).length,
    clipboard: __clipboard.slice(),
    exec: __exec.slice(),
    spoken: __spoken.map((u) => ({ text: u.text, voice: u.voice ? u.voice.name : null, lang: u.lang })),
    cancels: __cancels,
    prevented: __prevented.slice(),
    posted: __posted.length
  }));
}
"##;

    fn selection_case(
        name: &str,
        script: &str,
        pre_extra: &str,
        steps: &[&str],
    ) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "href": "https://example.com/artigo",
            "pre": format!("{SELECTION_DOM}\n{pre_extra}"),
            "script": script,
            "steps": steps,
        })
    }

    fn run_selection_cases(cases: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
        let program = format!(
            "const INPUT = {};\n{}",
            serde_json::json!({ "cases": cases }),
            INJECTED_SCRIPT_HARNESS
        );
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&run_node_program(&program)).expect("harness json");
        for result in &results {
            let name = &result["name"];
            assert_eq!(
                result["errors"].as_array().map(Vec::len),
                Some(0),
                "{name}: erro na pagina ou num passo do teste: {}",
                result["errors"]
            );
            assert_eq!(
                result["stolen"].as_array().map(Vec::len),
                Some(0),
                "{name}: o toJSON da pagina leu a capability"
            );
        }
        results
    }

    /// Os estados que um caso registou com `__state`, por etiqueta.
    fn selection_states(
        result: &serde_json::Value,
    ) -> std::collections::HashMap<String, serde_json::Value> {
        result["log"]
            .as_array()
            .expect("log")
            .iter()
            .map(|entry| {
                let state: serde_json::Value =
                    serde_json::from_str(entry.as_str().expect("log string")).expect("state json");
                (state["tag"].as_str().expect("tag").to_string(), state)
            })
            .collect()
    }

    fn selection_posted(result: &serde_json::Value) -> Vec<IpcAction> {
        result["posted"]
            .as_array()
            .expect("posted")
            .iter()
            .map(|message| {
                let message = message.as_str().expect("posted string");
                parse_ipc_message(message, SELECTION_CAP, COMPARATOR_COLUMNS)
                    .unwrap_or_else(|| panic!("o parser nativo recusou {message}"))
            })
            .collect()
    }

    #[test]
    fn the_selection_toolbar_offers_three_actions_for_a_trusted_selection() {
        // O mapa de teclas QUE EMBARCA, com a capability e o sinal privado
        // postos pelo mesmo `bind_page_script` dos builders.
        let script = bind_page_script(NEURALIA_KEYMAP_SCRIPT, SELECTION_CAP, false);
        let appears = r#"
__select('  Texto selecionado na pagina.  ');
__up({ isTrusted: false });
__state('sintetico');
__up({ button: 2 });
__state('botao-direito');
__up();
__state('agendada');
__drain();
__state('visivel');
__on(document, 'keydown', { key: 'Escape' });
__state('esc');
__on(document, 'keydown', { key: 'Escape' });
"#;
        let keyboard = r#"
__select('Por teclado.');
__on(window, 'keyup', { key: 'x' });
__state('tecla-x');
__on(window, 'keyup', { key: 'ArrowRight', shiftKey: true, isTrusted: false });
__state('tecla-sintetica');
__on(window, 'keyup', { key: 'ArrowRight', shiftKey: true });
__drain();
__state('shift-seta');
__on(window, 'mousedown', { target: document.body });
__state('clique-fora');
__on(window, 'keyup', { key: 'a', ctrlKey: true });
__drain();
__state('ctrl-a');
"#;
        let hides = r#"
for (const kind of ['scroll', 'resize', 'blur', 'popstate', 'hashchange', 'pagehide']) {
  __show('Texto.');
  __on(window, kind);
  __state('some-' + kind);
}
__show('Texto.');
__unselect();
__on(document, 'selectionchange');
__state('some-colapsada');
__select('Texto.');
__up();
__on(window, 'scroll');
__state('rolou-na-espera');
__drain();
__state('rolou-na-espera-depois');
"#;
        let refuses = r#"
__show('segredo', __el('input', { type: 'password' }));
__state('em-senha');
__show('rascunho', __el('textarea'));
__state('em-textarea');
__show('nota', __textIn(__el('span', {}, __el('div', { contenteditable: 'true' })), 'nota'));
__state('em-editavel');
__show('nota', __textIn(__el('b', {}, __el('div', { contenteditable: '' })), 'nota'));
__state('em-editavel-vazio');
document.activeElement = __el('input', { type: 'text' });
__show('Texto da pagina.');
__state('com-foco-num-campo');
document.activeElement = document.body;
__show('   \n\t  ');
__state('so-espacos');
__show('a'.repeat(5001));
__state('grande-demais');
__show('a'.repeat(5000));
__state('no-limite');
__show('Pesquisar', __root().host);
__state('na-barra');
__show('Texto para os botoes.');
__prevented.length = 0;
__press('copy');
__state('depois-de-premir');
"#;
        let positions = r#"
function __at(tag, rects) {
  __on(window, 'mousedown', { target: document.body });
  __show('Texto.', __para, { rects: rects });
  __state(tag);
}
__at('meio', [{ top: 300, bottom: 320, left: 100, right: 400, width: 300, height: 20 }]);
__at('canto-sup-dir', [{ top: 20, bottom: 40, left: 900, right: 995, width: 95, height: 20 }]);
__at('canto-inf-esq', [{ top: 670, bottom: 690, left: 0, right: 10, width: 10, height: 20 }]);
__at('alta', [{ top: 5, bottom: 695, left: 0, right: 500, width: 500, height: 690 }]);
__at('duas-linhas', [
  { top: 100, bottom: 120, left: 0, right: 900, width: 900, height: 20 },
  { top: 130, bottom: 150, left: 0, right: 200, width: 200, height: 20 },
  { top: 150, bottom: 150, left: 200, right: 200, width: 0, height: 0 }
]);
"#;
        let framed = r#"
__show('Texto num iframe.');
__state('quadro');
"#;
        let mut child = selection_case("quadro", &script, "", &[framed]);
        child["child"] = serde_json::Value::Bool(true);
        let results = run_selection_cases(vec![
            selection_case("aparece", &script, "", &[appears, keyboard, hides, refuses]),
            selection_case("posicao", &script, "", &[positions]),
            child,
        ]);

        let states = selection_states(&results[0]);
        let state = |tag: &str| {
            states
                .get(tag)
                .unwrap_or_else(|| panic!("sem o estado {tag}"))
                .clone()
        };
        let shown = |tag: &str| state(tag)["shown"].as_bool() == Some(true);

        // So um gesto real do utilizador, com o botao principal.
        for tag in ["sintetico", "botao-direito"] {
            assert!(!shown(tag), "{tag}: a barra apareceu");
            assert_eq!(state(tag)["pending"], 0, "{tag}: ficou um relogio");
        }
        // Espera um instante e so depois volta a olhar para a selecao.
        assert!(!shown("agendada"));
        assert_eq!(state("agendada")["pending"], 1);
        assert!(shown("visivel"), "a selecao confiavel nao trouxe a barra");
        let visible = state("visivel");
        assert_eq!(visible["mode"], "closed", "a pagina le a barra");
        assert_eq!(visible["position"], "fixed");
        assert_eq!(visible["zIndex"], "2147483647");
        assert_eq!(visible["sheets"], 1, "a barra ficou sem estilo");
        assert_eq!(
            visible["buttons"],
            serde_json::json!(["🔎 Pesquisar", "📋 Copiar", "🔊 Falar"])
        );
        assert_eq!(visible["tabindex"], serde_json::json!(["-1", "-1", "-1"]));
        // O primeiro Esc so fecha a barra; o segundo volta atras como sempre.
        assert!(!shown("esc"));
        assert_eq!(selection_posted(&results[0]), vec![IpcAction::Back]);

        // Teclado: Shift+setas e Ctrl+A; outra tecla ou evento sintetico nao.
        for tag in ["tecla-x", "tecla-sintetica", "clique-fora"] {
            assert!(!shown(tag), "{tag}: a barra apareceu");
            assert_eq!(state(tag)["pending"], 0, "{tag}: ficou um relogio");
        }
        assert!(shown("shift-seta"));
        assert!(shown("ctrl-a"));

        // Some -- e nao deixa relogio a correr -- com cada um destes.
        for tag in [
            "some-scroll",
            "some-resize",
            "some-blur",
            "some-popstate",
            "some-hashchange",
            "some-pagehide",
            "some-colapsada",
            "rolou-na-espera",
            "rolou-na-espera-depois",
        ] {
            assert!(!shown(tag), "{tag}: a barra ficou a vista");
            assert_eq!(state(tag)["pending"], 0, "{tag}: ficou um relogio");
        }

        // Campos editaveis, senhas, selecoes vazias ou enormes e a propria
        // barra nao a trazem.
        for tag in [
            "em-senha",
            "em-textarea",
            "em-editavel",
            "em-editavel-vazio",
            "com-foco-num-campo",
            "so-espacos",
            "grande-demais",
            "na-barra",
        ] {
            assert!(!shown(tag), "{tag}: a barra apareceu");
        }
        assert!(shown("no-limite"), "5000 caracteres ainda servem");

        // Premir um botao nao tira o foco nem a selecao a pagina.
        let pressed = state("depois-de-premir");
        assert!(pressed["shown"].as_bool() == Some(true));
        assert_eq!(pressed["prevented"], serde_json::json!(["window", "botao"]));

        // Perto do fim da selecao, dentro da area visivel (990x700 sem a
        // barra de rolagem), por cima quando cabe.
        let positions = selection_states(&results[1]);
        for (tag, top, left) in [
            ("meio", 250.0, 250.0),
            ("canto-sup-dir", 48.0, 682.0),
            ("canto-inf-esq", 620.0, 8.0),
            ("alta", 650.0, 350.0),
            ("duas-linhas", 80.0, 50.0),
        ] {
            let placed = &positions[tag];
            assert_eq!(placed["shown"], true, "{tag}");
            let (y, x) = (
                placed["top"].as_f64().expect("top"),
                placed["left"].as_f64().expect("left"),
            );
            assert!(
                (8.0..=990.0 - 8.0 - 300.0).contains(&x) && (8.0..=700.0 - 8.0 - 42.0).contains(&y),
                "{tag}: fora da area visivel ({x}, {y})"
            );
            assert_eq!((y, x), (top, left), "{tag}");
        }

        // Num iframe a barra nao existe.
        let framed = selection_states(&results[2]);
        assert_eq!(framed["quadro"]["mode"], serde_json::Value::Null);
        assert_eq!(framed["quadro"]["pending"], 0);
    }

    #[test]
    fn the_selection_toolbar_searches_only_what_fits_and_never_from_private() {
        let page = bind_page_script(NEURALIA_KEYMAP_SCRIPT, SELECTION_CAP, false);
        let search = r#"
__show('  agent:https://example.com | click=Comprar\r\n\tlinha\u00072  ');
__press('search');
__state('enviada');
__show('\u{1F600}'.repeat(2000));
__press('search');
__show('a'.repeat(2001));
__press('search');
__state('grande');
__show('forjado');
__press('search', { isTrusted: false });
__state('sintetico');
"#;
        // A pagina, depois de carregar, troca tudo o que a barra usa.
        let hostile_page = r#"
Document.prototype.getSelection = function () { return null; };
window.getSelection = function () { return null; };
Selection.prototype.toString = function () { return 'agent:forjado'; };
Selection.prototype.getRangeAt = function () { throw new Error('bloqueado'); };
for (const name of ['rangeCount', 'isCollapsed', 'anchorNode', 'focusNode']) {
  Object.defineProperty(Selection.prototype, name, {
    configurable: true, get() { throw new Error('bloqueado'); }
  });
}
Range.prototype.getClientRects = function () { throw new Error('bloqueado'); };
Range.prototype.getBoundingClientRect = function () { throw new Error('bloqueado'); };
EventTarget.prototype.addEventListener = function () { throw new Error('bloqueado'); };
Node.prototype.appendChild = function () { throw new Error('bloqueado'); };
Element.prototype.setAttribute = function () { throw new Error('bloqueado'); };
Element.prototype.attachShadow = function () { throw new Error('bloqueado'); };
Document.prototype.createElement = function () { throw new Error('bloqueado'); };
CSSStyleDeclaration.prototype.setProperty = function () { throw new Error('bloqueado'); };
Object.defineProperty(Node.prototype, 'textContent', { configurable: true, get() { return ''; }, set() {} });
Clipboard.prototype.writeText = function () { throw new Error('bloqueado'); };
Promise.prototype.then = function () { throw new Error('bloqueado'); };
JSON.stringify = function () { return '"forjado"'; };
"#;
        let hostile_use = r#"
__show('  Texto que o utilizador escolheu  ');
__state('robusta');
__press('copy');
"#;
        let hostile_after = r#"
__state('copiada');
__press('search');
"#;
        let offered = r#"
__show('Texto do painel.');
__state('barra');
"#;
        let results = run_selection_cases(vec![
            selection_case("pesquisa", &page, "", &[search]),
            selection_case(
                "pagina-hostil",
                &page,
                "",
                &[hostile_page, hostile_use, hostile_after],
            ),
            // O Split tal como `split_webview_builder` o injeta.
            selection_case(
                "split-privado",
                &split_init_script(1, "ChatGPT", SELECTION_CAP, true),
                "",
                &[offered],
            ),
            selection_case(
                "split",
                &split_init_script(1, "ChatGPT", SELECTION_CAP, false),
                "",
                &[offered],
            ),
        ]);

        // O texto chega ao parser nativo tal como foi selecionado (aparado,
        // CRLF como LF, controlos como espaco) e cabe no envelope de 8 KiB
        // mesmo com 2000 caracteres de 4 bytes.
        assert_eq!(
            selection_posted(&results[0]),
            vec![
                IpcAction::Search {
                    text: "agent:https://example.com | click=Comprar\n\tlinha 2".to_string()
                },
                IpcAction::Search {
                    text: "😀".repeat(2000)
                },
            ]
        );
        let states = selection_states(&results[0]);
        assert_eq!(
            states["enviada"]["shown"], false,
            "a barra ficou depois de pesquisar"
        );
        assert_eq!(states["enviada"]["pending"], 0);
        // Acima de 2000 nada sai da pagina e a barra diz porque.
        assert_eq!(states["grande"]["posted"], 2);
        assert_eq!(states["grande"]["shown"], true);
        assert_eq!(
            states["grande"]["note"],
            "Seleção grande demais para pesquisar (máx. 2000 caracteres)"
        );
        // Um clique sintetico nao pesquisa.
        assert_eq!(states["sintetico"]["posted"], 2);

        // Com as primitivas trocadas pela pagina, a barra continua a ler a
        // selecao verdadeira e a mandar o texto certo.
        let hostile = selection_states(&results[1]);
        assert_eq!(hostile["robusta"]["shown"], true);
        assert_eq!(
            hostile["robusta"]["buttons"],
            serde_json::json!(["🔎 Pesquisar", "📋 Copiar", "🔊 Falar"])
        );
        assert_eq!(
            hostile["copiada"]["clipboard"],
            serde_json::json!(["  Texto que o utilizador escolheu  "])
        );
        assert_eq!(hostile["copiada"]["buttons"][1], "✓ Copiado");
        assert_eq!(
            selection_posted(&results[1]),
            vec![IpcAction::Search {
                text: "Texto que o utilizador escolheu".to_string()
            }]
        );

        // Painel privado: sem Pesquisar. Painel normal: os tres.
        let private = selection_states(&results[2]);
        assert_eq!(private["barra"]["shown"], true);
        assert_eq!(
            private["barra"]["buttons"],
            serde_json::json!(["📋 Copiar", "🔊 Falar"])
        );
        assert!(selection_posted(&results[2]).is_empty());
        let normal = selection_states(&results[3]);
        assert_eq!(
            normal["barra"]["buttons"],
            serde_json::json!(["🔎 Pesquisar", "📋 Copiar", "🔊 Falar"])
        );
    }

    #[test]
    fn the_selection_toolbar_copies_and_speaks_with_local_voices() {
        let page = bind_page_script(NEURALIA_KEYMAP_SCRIPT, SELECTION_CAP, false);
        let copy = r#"
__show('Texto para copiar');
__press('copy');
__state('premido');
"#;
        let copied = r#"
__state('copiado');
__drain();
__state('volta');
"#;
        let speak = r#"
__voices = [__NUVEM, __MARIA, __ZIRA, __HELENA];
__show('Olá, mundo. Sr. Silva chegou! Tudo bem?\nFim.\n' + 'palavra '.repeat(50).trim() + '.');
__press('speak');
__state('falando');
for (let i = 0; i < 20; i++) {
  const before = __spoken.length;
  __utterEnd();
  if (__spoken.length === before) break;
}
__state('lida');
__press('speak');
__state('de-novo');
__press('speak');
__state('parada');
__utterEnd();
__state('sem-eco');
__press('speak');
__on(document, 'keydown', { key: 'Escape' });
__state('esc');
"#;
        let voices = r#"
function __voz(tag, voices, lang) {
  __voices = voices;
  document.documentElement.lang = lang;
  __show('Uma frase.');
  __press('speak');
  __state(tag);
  __on(document, 'keydown', { key: 'Escape' });
}
__voz('lingua-da-pagina', [__NUVEM, __MARIA, __ZIRA, __HELENA], 'es');
__voz('pt-br', [__NUVEM, __MARIA, __ZIRA, __HELENA], '');
__voz('lingua-do-sistema', [__NUVEM, __ZIRA, __HELENA], 'fr');
__voz('qualquer-local', [__NUVEM, __HELENA], 'fr');
__voz('so-online', [__NUVEM], '');
document.documentElement.lang = '';
__voices = [];
__show('Uma frase.');
__press('speak');
__state('tardia-espera');
__voices = [__MARIA];
__voicesChanged();
__state('tardia');
__on(document, 'keydown', { key: 'Escape' });
__voices = [];
__show('Uma frase.');
__press('speak');
__drain();
__state('sem-vozes');
"#;
        let results = run_selection_cases(vec![
            selection_case("copiar", &page, "", &[copy, copied]),
            selection_case(
                "copiar-sem-api",
                &page,
                "navigator.clipboard = undefined;",
                &[copy],
            ),
            selection_case(
                "copiar-negado",
                &page,
                "__clipboardFails = true;",
                &[copy, copied],
            ),
            selection_case("falar", &page, "", &[speak]),
            selection_case("vozes", &page, "", &[voices]),
        ]);
        // Copiar e falar ficam na pagina: nenhuma mensagem ao nativo.
        for result in &results {
            assert!(
                selection_posted(result).is_empty(),
                "{}: postou {}",
                result["name"],
                result["posted"]
            );
        }

        let copy = selection_states(&results[0]);
        assert_eq!(
            copy["premido"]["clipboard"],
            serde_json::json!(["Texto para copiar"])
        );
        assert_eq!(copy["premido"]["exec"], serde_json::json!([]));
        assert_eq!(copy["copiado"]["buttons"][1], "✓ Copiado");
        assert_eq!(copy["copiado"]["shown"], true);
        assert_eq!(copy["volta"]["buttons"][1], "📋 Copiar");
        assert_eq!(copy["volta"]["pending"], 0);
        // Sem a API do clipboard, ou com ela a recusar, copia pela selecao.
        let fallback = selection_states(&results[1]);
        assert_eq!(fallback["premido"]["exec"], serde_json::json!(["copy"]));
        assert_eq!(fallback["premido"]["buttons"][1], "✓ Copiado");
        let refused = selection_states(&results[2]);
        assert_eq!(refused["copiado"]["exec"], serde_json::json!(["copy"]));
        assert_eq!(refused["copiado"]["buttons"][1], "✓ Copiado");

        let speech = selection_states(&results[3]);
        let spoken = |tag: &str| -> Vec<(String, String)> {
            speech[tag]["spoken"]
                .as_array()
                .expect("spoken")
                .iter()
                .map(|u| {
                    (
                        u["text"].as_str().unwrap_or_default().to_string(),
                        u["voice"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        };
        assert_eq!(speech["falando"]["buttons"][2], "⏹ Parar");
        assert_eq!(
            spoken("falando"),
            vec![("Olá, mundo.".to_string(), "Maria".to_string())]
        );
        // Uma frase por fala; a abreviatura cola-se a seguinte; a frase
        // enorme parte em palavras, sem passar de 200 caracteres.
        let words = |n: usize| vec!["palavra"; n].join(" ");
        let read: Vec<String> = spoken("lida").into_iter().map(|(text, _)| text).collect();
        assert_eq!(
            read,
            vec![
                "Olá, mundo.".to_string(),
                "Sr. Silva chegou!".to_string(),
                "Tudo bem?".to_string(),
                "Fim.".to_string(),
                words(25),
                format!("{}.", words(25)),
            ]
        );
        assert!(read.iter().all(|part| part.chars().count() <= 200));
        assert!(spoken("lida").iter().all(|(_, voice)| voice == "Maria"));
        assert_eq!(speech["lida"]["buttons"][2], "🔊 Falar");
        // Segundo clique cala; a fala cancelada nao puxa a frase seguinte.
        assert_eq!(speech["de-novo"]["buttons"][2], "⏹ Parar");
        let cancels = speech["de-novo"]["cancels"].as_u64().expect("cancels");
        assert_eq!(speech["parada"]["buttons"][2], "🔊 Falar");
        assert_eq!(speech["parada"]["cancels"], cancels + 1);
        assert_eq!(spoken("sem-eco").len(), spoken("de-novo").len());
        // Esc tambem cala, fecha a barra e nao volta atras.
        assert_eq!(speech["esc"]["shown"], false);
        assert_eq!(
            speech["esc"]["cancels"].as_u64(),
            Some(cancels + 3),
            "o Esc nao calou a leitura"
        );

        // A voz: local, na lingua da pagina; senao pt-BR; senao a do sistema;
        // senao qualquer local. Nunca a online.
        let voices = selection_states(&results[4]);
        let last_voice = |tag: &str| {
            voices[tag]["spoken"]
                .as_array()
                .and_then(|spoken| spoken.last())
                .map(|u| u["voice"].clone())
                .unwrap_or_default()
        };
        assert_eq!(last_voice("lingua-da-pagina"), "Helena");
        assert_eq!(last_voice("pt-br"), "Maria");
        assert_eq!(last_voice("lingua-do-sistema"), "Zira");
        assert_eq!(last_voice("qualquer-local"), "Helena");
        assert_eq!(
            voices["so-online"]["spoken"].as_array().map(Vec::len),
            voices["qualquer-local"]["spoken"].as_array().map(Vec::len),
            "falou com uma voz online"
        );
        assert_eq!(voices["so-online"]["note"], "Nenhuma voz local disponível");
        // As vozes chegam depois: espera pelo 'voiceschanged' e nao deixa o
        // relogio da espera a correr.
        assert_eq!(voices["tardia-espera"]["pending"], 1);
        assert_eq!(
            voices["tardia-espera"]["spoken"].as_array().map(Vec::len),
            voices["so-online"]["spoken"].as_array().map(Vec::len)
        );
        assert_eq!(last_voice("tardia"), "Maria");
        assert_eq!(voices["tardia"]["pending"], 0);
        assert_eq!(voices["sem-vozes"]["note"], "Nenhuma voz local disponível");
        assert_eq!(voices["sem-vozes"]["buttons"][2], "🔊 Falar");
    }

    #[test]
    fn a_selected_search_reaches_the_comparator_from_every_surface_but_the_private_split() {
        let text = "agent:https://example.com | click=Comprar".to_string();
        let search = || IpcAction::Search { text: text.clone() };
        let carries = |event: Option<UserEvent>| matches!(event, Some(UserEvent::SearchSelection(ref got)) if *got == text);
        // As tres colunas do comparador.
        for col in 0..COMPARATOR_COLUMNS {
            assert!(
                carries(App::column_ipc_event_impl(col, search())),
                "coluna {col}"
            );
        }
        // Split normal; o privado recusa, mas continua a fechar-se.
        assert!(carries(App::split_ipc_event_impl(1, false, search())));
        assert!(App::split_ipc_event_impl(1, true, search()).is_none());
        assert!(matches!(
            App::split_ipc_event_impl(1, true, IpcAction::SplitClose),
            Some(UserEvent::CloseSplit)
        ));
        // Web externa, com e sem agente.
        assert!(carries(external_ipc_event(search(), false)));
        assert!(carries(external_ipc_event(search(), true)));
        // Reader e PDF usam o mapa comum.
        assert!(carries(common_ipc_event(search())));
    }

    #[test]
    fn a_selected_search_is_a_question_never_an_omnibox_command() {
        // Na omnibox cada uma destas e um comando (agente, tema, memoria...).
        // Selecionada numa pagina e so a pergunta que vai as tres IAs.
        for command in [
            "agent:https://example.com | click=Comprar",
            "tema:escuro",
            "theme:light",
            "memory:rebuild",
            "mem:senhas",
            "history:",
            "research:export",
        ] {
            assert_ne!(
                route_input(command),
                InputRoute::Intent,
                "{command} deixou de ser um comando da omnibox; o teste perdeu o sentido"
            );
            assert_eq!(
                selection_search_question(&format!("  {command}\n")),
                Some(command.to_string()),
                "{command}"
            );
        }
        // Um endereco selecionado tambem e pergunta, nao navegacao.
        assert_eq!(
            selection_search_question("https://example.com/"),
            Some("https://example.com/".to_string())
        );
        assert_eq!(selection_search_question(" \n "), None);
        assert_eq!(
            selection_search_question(&"a".repeat(crate::ipc::SEARCH_MAX_CHARS + 1)),
            None
        );

        // O handler nativo nao pode sequer tocar no interpretador de
        // comandos (asserção de ausencia, AGENTS.md §4.3).
        let source = include_str!("windows_app.rs");
        let handler = source
            .split("fn search_selection(&mut self")
            .nth(1)
            .and_then(|part| part.split("\n    fn ").next())
            .expect("search_selection");
        for forbidden in ["handle_input", "route_input", "parse_intent", "SubmitText"] {
            assert!(
                !handler.contains(forbidden),
                "o Pesquisar passa por {forbidden}"
            );
        }
        let arm = source
            .split("UserEvent::SearchSelection(text) =>")
            .nth(1)
            .and_then(|part| part.lines().next())
            .expect("SearchSelection arm");
        assert!(!arm.contains("handle_input") && !arm.contains("SubmitText"));
    }

    #[test]
    fn the_main_window_answers_the_same_ctrl_shortcuts() {
        use winit::keyboard::ModifiersState;
        let key = |text: &str| Key::Character(text.into());
        let ctrl = ModifiersState::CONTROL;
        let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert_eq!(
            main_window_shortcut(&key("r"), ctrl),
            Some(MainShortcut::AutoScroll)
        );
        assert_eq!(
            main_window_shortcut(&key("R"), ctrl_shift),
            Some(MainShortcut::Reload)
        );
        assert_eq!(
            main_window_shortcut(&key("h"), ctrl),
            Some(MainShortcut::History)
        );
        assert_eq!(
            main_window_shortcut(&key("n"), ctrl),
            Some(MainShortcut::NewTab)
        );
        // Sem Ctrl, ou com Alt (AltGr no teclado portugues), a tecla e texto.
        assert_eq!(
            main_window_shortcut(&key("r"), ModifiersState::empty()),
            None
        );
        assert_eq!(
            main_window_shortcut(&key("r"), ctrl | ModifiersState::ALT),
            None
        );
    }

    #[test]
    fn clearing_all_history_needs_an_explicit_yes() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IDCANCEL, IDNO};
        assert!(clear_history_confirmed(IDYES));
        for answer in [IDNO, IDCANCEL, 0] {
            assert!(
                !clear_history_confirmed(answer),
                "resposta {answer} apagou tudo"
            );
        }
        assert!(auto_scroll_message(false).contains("desligada"));
        assert!(auto_scroll_message(true).contains("Ctrl+R desliga"));
    }

    #[test]
    fn comparator_ctrl_click_on_a_google_search_link_keeps_the_real_url() {
        // `q` numa pesquisa do Google e um termo, nao uma URL. Resolvido
        // contra a origem da coluna virava https://gemini.google.com/app/rust.
        const CAP: &str = "0123456789abcdef0123456789abcdef";
        let search = "https://www.google.com/search?q=rust";
        let cases = [
            ("https://gemini.google.com/app/abc", search, search),
            ("https://chatgpt.com/c/abc", search, search),
            ("https://www.google.com/search?q=x&udm=50", search, search),
            (
                "https://gemini.google.com/app/abc",
                "https://maps.google.com/?q=Paris",
                "https://maps.google.com/?q=Paris",
            ),
            // O embrulho real do Google continua a ser desembrulhado.
            (
                "https://gemini.google.com/app/abc",
                "https://www.google.com/url?q=https%3A%2F%2Fexample.com%2F",
                "https://example.com/",
            ),
        ];
        let inputs: Vec<serde_json::Value> = cases
            .iter()
            .map(|(location, href, _)| {
                serde_json::json!({
                    "name": format!("{location} -> {href}"),
                    "href": location,
                    "script": COMPARATOR_INJECT_SCRIPT.replace("__NEURALIA_CAP__", CAP),
                    "drive": format!(
                        "const anchor = new Element('a');\n\
                         anchor.href = {};\n\
                         anchor.matches = () => true;\n\
                         __fire('click', {{ ctrlKey: true, target: anchor, \
                         composedPath() {{ return [anchor]; }} }});\n",
                        serde_json::Value::from(*href)
                    ),
                })
            })
            .collect();
        let program = format!(
            "const INPUT = {};\n{}",
            serde_json::json!({ "cases": inputs }),
            INJECTED_SCRIPT_HARNESS
        );
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&run_node_program(&program)).expect("harness json");
        assert_eq!(results.len(), cases.len());
        for (result, (_, _, expected)) in results.iter().zip(cases) {
            let name = result["name"].as_str().unwrap_or_default();
            let posted = result["posted"].as_array().expect("posted");
            assert_eq!(posted.len(), 1, "{name}: errors {}", result["errors"]);
            let message = posted[0].as_str().expect("posted string");
            match parse_ipc_message(message, CAP, 3) {
                Some(IpcAction::Link { url, aside, .. }) => {
                    assert!(aside, "{name}");
                    assert_eq!(url, expected, "{name}");
                }
                other => panic!("{name}: expected a link action, got {other:?}"),
            }
        }
    }

    #[test]
    fn split_scroll_rail_is_top_frame_only() {
        // WebView2 corre os initialization scripts tambem nos iframes. O rail
        // (e o CSS que esconde as barras de rolagem) so pertence ao documento
        // principal; num iframe cobria e engolia cliques do conteudo dele.
        let inputs: Vec<serde_json::Value> = [false, true]
            .into_iter()
            .map(|child| {
                serde_json::json!({
                    "name": if child { "child frame" } else { "top frame" },
                    "href": "https://example.com/",
                    "child": child,
                    "script": SPLIT_SCROLL_RAIL_SCRIPT,
                })
            })
            .collect();
        let program = format!(
            "const INPUT = {};
{}",
            serde_json::json!({ "cases": inputs }),
            INJECTED_SCRIPT_HARNESS
        );
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&run_node_program(&program)).expect("harness json");
        let created = |index: usize| -> Vec<String> {
            results[index]["created"]
                .as_array()
                .expect("created")
                .iter()
                .map(|tag| tag.as_str().unwrap_or_default().to_string())
                .collect()
        };
        assert!(
            created(0).iter().any(|tag| tag == "style"),
            "top frame must still mount the rail: {:?}",
            results[0]["errors"]
        );
        assert!(
            created(1).is_empty(),
            "child frame mounted rail elements: {:?}",
            created(1)
        );
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
    fn clearing_memory_drops_the_live_research_session() {
        // Depois de "Apagar historico" a sessao viva nao pode continuar a
        // receber fontes: o proximo save_session reescrevia no disco a
        // pergunta que o utilizador acabou de apagar.
        let (tx, rx) = sync_channel::<MemoryCommand>(4);
        let worker = MemoryWorker { tx };
        let mut current_research = Some(ResearchSession::new("pergunta secreta"));
        worker.clear(&mut current_research);
        assert!(matches!(rx.try_recv(), Ok(MemoryCommand::Clear)));
        assert!(
            current_research.is_none(),
            "the cleared research session is still alive and will be saved again"
        );
    }

    #[test]
    fn a_private_split_request_after_leaving_the_comparator_is_dropped() {
        // Fora do comparador um pedido privado nunca cai em web(): isso
        // gravava a URL privada no historico e na memoria semantica.
        for surface in [
            Surface::Home,
            Surface::Reader,
            Surface::External,
            Surface::Pdf,
        ] {
            assert_eq!(
                App::split_request_fallback(surface, true),
                SplitFallback::Ignore,
                "{surface:?}"
            );
            assert_eq!(
                App::split_request_fallback(surface, false),
                SplitFallback::OpenWeb,
                "{surface:?}"
            );
        }
        for private in [false, true] {
            assert_eq!(
                App::split_request_fallback(Surface::Comparator, private),
                SplitFallback::OpenSplit
            );
        }
    }

    #[test]
    fn full_web_new_window_about_blank_does_not_replace_the_page() {
        for blank in ["about:blank", "ABOUT:BLANK"] {
            assert!(
                external_new_window_event(blank.to_string(), None).is_none(),
                "{blank} must be denied, not opened as OpenExternal"
            );
        }
        assert!(matches!(
            external_new_window_event("https://example.com/".to_string(), None),
            Some(UserEvent::OpenExternal(url)) if url == "https://example.com/"
        ));
        assert!(external_new_window_event("http://192.168.0.1/".to_string(), None).is_none());
    }

    #[test]
    fn reopening_a_context_tab_does_not_rebuild_or_duplicate_the_source() {
        assert!(split_open_records_source(None, false));
        assert!(!split_open_records_source(None, true));
        assert!(
            !split_open_records_source(Some(7), false),
            "a reopened context tab must not add another session source"
        );
        assert!(
            context_tab_click_is_noop(Some((0, Some(1))), 0, 1),
            "clicking the active context tab must not rebuild the split"
        );
        assert!(!context_tab_click_is_noop(Some((0, Some(1))), 0, 2));
        assert!(!context_tab_click_is_noop(Some((1, Some(1))), 0, 1));
        assert!(!context_tab_click_is_noop(Some((0, None)), 0, 1));
        assert!(!context_tab_click_is_noop(None, 0, 1));
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
            panel_width: 0.0,
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
            panel_width: 0.0,
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
    fn native_controls_follow_the_effective_hwnd_after_decoration_changes() {
        let source = include_str!("windows_app.rs");
        let body = source
            .split("fn ensure_window_subclass")
            .nth(1)
            .and_then(|part| part.split("fn create_omnibox").next())
            .expect("ensure_window_subclass body");
        assert!(body.contains("GetParent(child) != parent"));
        assert!(body.contains("SetParent(child, parent)"));
        assert!(body.contains("self.omnibox"));
        assert!(body.contains("self.home_button"));
    }

    #[test]
    fn expanded_column_keeps_window_chrome_and_content_offset() {
        let source = include_str!("windows_app.rs");
        let expand = source
            .split("fn expand_comparator")
            .nth(1)
            .and_then(|part| part.split("fn minimize_comparator").next())
            .expect("expand_comparator body");
        assert!(
            !expand.contains("set_fullscreen(Some"),
            "expandir uma IA nao pode esconder os controles da janela"
        );

        let layout = source
            .split("fn update_comparator_layout")
            .nth(1)
            .and_then(|part| part.split("fn column_ipc_event_impl").next())
            .expect("layout body");
        assert!(layout.contains("LogicalPosition::new(0.0, content_y)"));
        assert!(layout.contains("LogicalSize::new(logical_w, content_h)"));

        let bar = BarLayout::new(1600.0, 1.0, true, 3);
        assert!(bar.window_minimize.width > 0.0);
        assert!(bar.window_maximize.width > 0.0);
        assert!(bar.window_close.width > 0.0);
    }

    #[test]
    fn comparator_uses_palette_instead_of_the_home_omnibox() {
        assert!(surface_accepts_omnibox_submit(Surface::Home));
        assert!(!surface_accepts_omnibox_submit(Surface::Comparator));
        assert!(matches!(
            App::column_ipc_event_impl(0, IpcAction::Omnibox),
            Some(UserEvent::OpenPalette(0))
        ));
        assert!(matches!(
            App::column_ipc_event_impl(2, IpcAction::Omnibox),
            Some(UserEvent::OpenPalette(2))
        ));

        assert!(matches!(
            App::column_ipc_event_impl(1, IpcAction::Expand { col: 1 }),
            Some(UserEvent::ExpandComparator(1))
        ));
        assert!(
            App::column_ipc_event_impl(0, IpcAction::Expand { col: 1 }).is_none(),
            "uma coluna nao pode comandar a expansao de outra"
        );
    }

    #[test]
    fn comparator_disables_the_offscreen_home_omnibox() {
        unsafe {
            let parent = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                WS_POPUP,
                0,
                0,
                200,
                80,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!parent.is_null());

            let edit = CreateWindowExW(
                0,
                windows_sys::w!("EDIT"),
                windows_sys::w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                0,
                0,
                100,
                30,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!edit.is_null());

            apply_omnibox_interactivity(edit, Surface::Comparator);
            assert_eq!(
                windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(edit),
                0,
                "omnibox invisivel nao pode receber foco"
            );

            apply_omnibox_interactivity(edit, Surface::Home);
            assert_ne!(
                windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(edit),
                0,
                "Home precisa reativar a omnibox"
            );

            DestroyWindow(parent);
        }
    }

    #[test]
    fn comparator_titlebar_does_not_reserve_an_omnibox_slot() {
        let layout = BarLayout::with_contexts(
            1600.0,
            1.0,
            true,
            BarColumns::even(COMPARATOR_COLUMNS),
            [1, 0, 0],
        );
        assert_eq!(layout.context_tab_counts[0], 1);
        assert!(
            layout.context_tabs[0][0].x <= 100.0,
            "a primeira aba foi empurrada por controle extra: {:?}",
            layout.context_tabs[0][0]
        );
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
        static NEXT_TEST_TAB_ID: AtomicU64 = AtomicU64::new(1);
        ContextTab {
            id: NEXT_TEST_TAB_ID.fetch_add(1, Ordering::Relaxed),
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
    fn closing_other_tabs_only_touches_the_selected_group() {
        let mut tabs = vec![
            tab("https://g1.example/keep", Some(1)),
            tab("https://g1.example/drop", Some(1)),
            tab("https://g2.example/a", Some(2)),
            tab("https://loose.example/a", None),
        ];
        let mut groups = vec![group(1, false), group(2, false)];
        assert!(close_other_context_tabs_in_scope(&mut tabs, &mut groups, 0));
        assert_eq!(
            tabs.iter().map(|tab| tab.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://g1.example/keep",
                "https://g2.example/a",
                "https://loose.example/a"
            ]
        );
        assert_eq!(groups.len(), 2, "outro grupo deve permanecer intacto");
    }

    #[test]
    fn closing_all_tabs_only_removes_the_selected_group_and_prunes_its_pill() {
        let mut tabs = vec![
            tab("https://g1.example/a", Some(1)),
            tab("https://g1.example/b", Some(1)),
            tab("https://g2.example/a", Some(2)),
            tab("https://loose.example/a", None),
        ];
        let mut groups = vec![group(1, false), group(2, false)];
        assert!(close_context_tab_scope(&mut tabs, &mut groups, 1));
        assert_eq!(
            tabs.iter().map(|tab| tab.url.as_str()).collect::<Vec<_>>(),
            vec!["https://g2.example/a", "https://loose.example/a"]
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, 2);
    }

    #[test]
    fn regrouping_the_last_tab_prunes_the_previous_empty_group() {
        let mut tabs = vec![
            tab("https://old.example/a", Some(7)),
            tab("https://other.example/a", Some(9)),
        ];
        let mut groups = vec![group(7, false), group(9, false)];
        let mut next_id = 10;
        let created =
            regroup_context_tab(&mut tabs, &mut groups, &mut next_id, 0).expect("aba existe");
        assert_eq!(tabs[0].group, Some(groups[created].id));
        assert!(
            groups.iter().all(|group| group.id != 7),
            "grupo anterior vazio nao pode continuar na barra"
        );
        assert!(groups.iter().any(|group| group.id == 9));
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
    fn a_group_larger_than_the_window_keeps_its_pill_and_recent_tabs() {
        // Quatro abas abertas num grupo: o corte cai dentro do grupo e nao ha
        // aba solta depois dele. A linha nao pode ficar vazia -- a pilula e o
        // unico caminho para fechar ou reabrir o grupo.
        let mut tabs = vec![
            tab("https://a.example/1", Some(1)),
            tab("https://b.example/2", Some(1)),
            tab("https://c.example/3", Some(1)),
            tab("https://d.example/4", None),
        ];
        let groups = vec![group(1, false)];
        join_context_group(&mut tabs, 1, 3);
        let row = plan_tab_row(&tabs, &groups);
        assert_no_orphans(&row, &tabs, &groups);
        assert_eq!(
            row.visible(),
            &[
                TabSlot::Group(0),
                TabSlot::Tab(1),
                TabSlot::Tab(2),
                TabSlot::Tab(3)
            ]
        );
        let mut rows = [TabRow::empty(); COMPARATOR_COLUMNS];
        rows[0] = row;
        let layout = BarLayout::with_rows(1600.0, 1.0, true, BarColumns::even(3), rows);
        assert_eq!(layout.group_pill_counts[0], 1);
        assert_eq!(layout.context_tab_counts[0], 3);
        let pill = layout.group_pills[0][0];
        assert_eq!(
            layout.hit(pill.x + pill.width / 2.0, pill.y + pill.height / 2.0),
            Some(BarHit::ContextGroup {
                source_index: 0,
                group_index: 0
            })
        );

        // Um grupo fechado antes dele nao perde a sua pilula.
        let tabs = vec![
            tab("https://x.example/0", Some(0)),
            tab("https://a.example/1", Some(1)),
            tab("https://b.example/2", Some(1)),
            tab("https://c.example/3", Some(1)),
            tab("https://d.example/4", Some(1)),
        ];
        let groups = vec![group(0, true), group(1, false)];
        let row = plan_tab_row(&tabs, &groups);
        assert_no_orphans(&row, &tabs, &groups);
        assert_eq!(
            row.visible(),
            &[
                TabSlot::Group(0),
                TabSlot::Group(1),
                TabSlot::Tab(2),
                TabSlot::Tab(3),
                TabSlot::Tab(4)
            ]
        );
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

    #[test]
    fn ui_rect_edges_are_half_open_so_adjacent_controls_never_share_a_click() {
        let left = UiRect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        let right = UiRect {
            x: 10.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        assert!(left.contains(9.999, 5.0));
        assert!(!left.contains(10.0, 5.0));
        assert!(right.contains(10.0, 5.0));
        assert!(!right.contains(20.0, 5.0));

        let bar = BarLayout::new(1120.0, 1.0, true, 3);
        let boundary = bar.window_close.x;
        let y = bar.window_close.y + bar.window_close.height / 2.0;
        assert!(
            !bar.window_maximize.contains(boundary, y),
            "a borda do fechar nao pode pertencer tambem ao maximizar"
        );
        assert_eq!(bar.hit(boundary, y), Some(BarHit::WindowClose));
        assert_eq!(bar.hit(boundary - 0.001, y), Some(BarHit::WindowMaximize));
    }

    #[test]
    fn keyboard_shortcuts_keep_page_identity_and_global_digit_targets() {
        for source in 0..COMPARATOR_COLUMNS {
            assert!(matches!(
                App::column_ipc_event_impl(source, IpcAction::Fullscreen),
                Some(UserEvent::ExpandComparator(index)) if index == source
            ));
            assert!(matches!(
                App::column_ipc_event_impl(source, IpcAction::Reload),
                Some(UserEvent::ReloadTarget(PageTarget::Column(index))) if index == source
            ));
            assert!(matches!(
                App::column_ipc_event_impl(source, IpcAction::Print),
                Some(UserEvent::PrintTarget(PageTarget::Column(index))) if index == source
            ));
            assert!(matches!(
                App::column_ipc_event_impl(source, IpcAction::DevTools),
                Some(UserEvent::OpenDevToolsTarget(PageTarget::Column(index))) if index == source
            ));
            assert!(matches!(
                App::column_ipc_event_impl(source, IpcAction::ViewSource),
                Some(UserEvent::ViewSourceTarget(PageTarget::Column(index))) if index == source
            ));

            // 1/2/3 sao atalhos globais: a coluna com foco nao limita o alvo.
            for target in 0..COMPARATOR_COLUMNS {
                assert!(matches!(
                    App::column_ipc_event_impl(
                        source,
                        IpcAction::ShortcutExpand { col: target }
                    ),
                    Some(UserEvent::ExpandComparator(index)) if index == target
                ));
            }
        }

        // O botao/DOM normal continua preso a propria coluna.
        assert!(matches!(
            App::column_ipc_event_impl(1, IpcAction::Expand { col: 1 }),
            Some(UserEvent::ExpandComparator(1))
        ));
        assert!(App::column_ipc_event_impl(0, IpcAction::Expand { col: 1 }).is_none());

        assert!(matches!(
            App::split_ipc_event_impl(2, false, IpcAction::Fullscreen),
            Some(UserEvent::ToggleSplitFullscreen)
        ));
        assert!(matches!(
            App::split_ipc_event_impl(2, false, IpcAction::Omnibox),
            Some(UserEvent::OpenPalette(2))
        ));
        assert!(matches!(
            App::split_ipc_event_impl(2, false, IpcAction::Print),
            Some(UserEvent::PrintTarget(PageTarget::Split))
        ));
        assert!(matches!(
            App::split_ipc_event_impl(2, false, IpcAction::ShortcutExpand { col: 0 }),
            Some(UserEvent::ExpandComparator(0))
        ));
    }

    #[test]
    fn stale_split_builds_are_discarded_after_navigation_changes() {
        assert!(split_build_is_current(7, 7, Surface::Comparator));
        assert!(!split_build_is_current(7, 8, Surface::Comparator));
        assert!(!split_build_is_current(7, 7, Surface::Home));
        assert!(!split_build_is_current(7, 7, Surface::External));
    }

    #[test]
    fn failed_split_build_preserves_the_previous_split_and_expansion() {
        let mut split = Some(41u32);
        let mut expanded = Some(2usize);

        let failed: Result<u32, &str> = Err("webview failed");
        assert_eq!(
            commit_split_build(failed, &mut split, &mut expanded),
            Err("webview failed")
        );
        assert_eq!(split, Some(41), "falha nao pode destruir o Split anterior");
        assert_eq!(
            expanded,
            Some(2),
            "falha nao pode sair da expansao que ja estava visivel"
        );

        let committed = commit_split_build(Ok::<u32, &str>(99), &mut split, &mut expanded)
            .expect("build valido");
        assert_eq!(committed, (99, Some(41)));
        assert_eq!(split, None);
        assert_eq!(expanded, None);
    }

    #[test]
    fn native_buttons_only_activate_when_press_and_release_match() {
        assert_eq!(native_button_index(ninety_for_test(), 0), Some(0));
        assert_eq!(native_button_index(ninety_for_test(), 29), Some(0));
        assert_eq!(native_button_index(ninety_for_test(), 30), Some(1));
        assert_eq!(native_button_index(ninety_for_test(), 59), Some(1));
        assert_eq!(native_button_index(ninety_for_test(), 60), Some(2));
        assert_eq!(native_button_index(ninety_for_test(), 89), Some(2));
        assert_eq!(native_button_index(ninety_for_test(), -1), None);
        assert_eq!(native_button_index(ninety_for_test(), 90), None);

        assert_eq!(native_release_matches(Some(0), Some(0)), Some(0));
        assert_eq!(native_release_matches(Some(0), Some(2)), None);
        assert_eq!(native_release_matches(None, Some(2)), None);
        assert_eq!(
            native_caption_release(Some(1), true, 90, 30, 45, 15),
            Some(1)
        );
        assert_eq!(native_caption_release(Some(1), true, 90, 30, 45, 30), None);
        assert_eq!(native_caption_release(Some(1), true, 90, 30, 45, -1), None);
        assert_eq!(native_caption_release(Some(1), false, 90, 30, 45, 15), None);
        assert!(point_inside_client(100, 30, 99, 29));
        assert!(!point_inside_client(100, 30, 100, 29));
        assert!(!point_inside_client(100, 30, 99, 30));

        let pressed = AtomicUsize::new(1);
        assert_eq!(
            take_native_pressed_button(&pressed, || {
                // WM_CAPTURECHANGED chega sincronamente durante ReleaseCapture.
                pressed.store(NATIVE_BUTTON_NONE, Ordering::Release);
            }),
            Some(1)
        );
        assert_eq!(pressed.load(Ordering::Acquire), NATIVE_BUTTON_NONE);
        assert_eq!(take_native_pressed_button(&pressed, || {}), None);
    }

    fn ninety_for_test() -> i32 {
        90
    }

    #[test]
    fn current_group_is_not_offered_as_a_join_target() {
        let groups = vec![group(1, false), group(2, false), group(3, false)];
        let joinable = App::joinable_context_groups(&groups, Some(2));
        assert_eq!(joinable, vec![(0, "G1".to_string()), (2, "G3".to_string())]);
        assert_eq!(
            App::joinable_context_groups(&groups, None),
            vec![
                (0, "G1".to_string()),
                (1, "G2".to_string()),
                (2, "G3".to_string())
            ]
        );
    }

    #[test]
    fn ui_100_interaction_matrix_keeps_click_targets_unambiguous() {
        let logical_widths = [700.0, 760.0, 900.0, 1120.0, 1600.0];
        let scales = [1.0, 1.25, 1.5, 2.0];
        let topologies = [
            ([false, false, false], false),
            ([true, false, false], false),
            ([false, true, false], false),
            ([false, false, true], false),
            ([false, false, false], true),
        ];
        let center = |rect: UiRect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let mut scenarios = 0usize;

        for logical_width in logical_widths {
            for scale in scales {
                let client_width = logical_width * scale;
                for (minimized, split_active) in topologies {
                    scenarios += 1;
                    let columns = BarColumns {
                        count: COMPARATOR_COLUMNS,
                        weights: [1.0; COMPARATOR_COLUMNS],
                        minimized,
                        split_active,
                        panel_width: 0.0,
                    };
                    let layout =
                        BarLayout::with_contexts(client_width, scale, true, columns, [3, 3, 3]);

                    for (rect, expected) in [
                        (layout.window_minimize, BarHit::WindowMinimize),
                        (layout.window_maximize, BarHit::WindowMaximize),
                        (layout.window_close, BarHit::WindowClose),
                        (layout.home, BarHit::Home),
                    ] {
                        let (x, y) = center(rect);
                        assert_eq!(
                            layout.hit(x, y),
                            Some(expected),
                            "alvo errado em {logical_width}px @{scale}x"
                        );
                    }

                    let controls = right_controls(client_width, scale, split_active);
                    let (px, py) = center(controls.private);
                    assert_eq!(right_controls_hit(controls, px, py), Some(BarHit::Private));
                    assert_eq!(
                        layout.hit(px, py),
                        None,
                        "controle Privado sobrepoe alvo da barra em {logical_width}px @{scale}x"
                    );

                    if let Some((_label, expand, close)) = controls.split {
                        for (rect, expected) in
                            [(expand, BarHit::SplitExpand), (close, BarHit::SplitClose)]
                        {
                            let (x, y) = center(rect);
                            assert_eq!(right_controls_hit(controls, x, y), Some(expected));
                            assert_eq!(
                                layout.hit(x, y),
                                None,
                                "controle do Split sobrepoe alvo da barra em {logical_width}px @{scale}x"
                            );
                        }
                    }

                    for index in 0..COMPARATOR_COLUMNS {
                        let provider = layout.columns[index];
                        if provider.width > 0.0 {
                            let (x, y) = center(provider);
                            assert_eq!(
                                layout.hit(x, y),
                                Some(BarHit::Column(index)),
                                "provedor {index} perdeu o clique em {logical_width}px @{scale}x"
                            );
                        }

                        let add = layout.add_tabs[index];
                        if add.width > 0.0 {
                            let (x, y) = center(add);
                            assert_eq!(
                                layout.hit(x, y),
                                Some(BarHit::AddTab(index)),
                                "+ da coluna {index} perdeu o clique em {logical_width}px @{scale}x"
                            );
                        }

                        for visual in 0..layout.context_tab_counts[index] {
                            let tab_rect = layout.context_tabs[index][visual];
                            let (x, y) = center(tab_rect);
                            assert_eq!(
                                layout.hit(x, y),
                                Some(BarHit::ContextTab {
                                    source_index: index,
                                    context_index: layout.context_indices[index][visual],
                                }),
                                "aba da coluna {index} perdeu o clique em {logical_width}px @{scale}x"
                            );
                        }
                    }
                }
            }
        }

        assert_eq!(
            scenarios, 100,
            "o gate precisa exercitar exatamente cem combinacoes de tela/estado"
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

/// O tema que o utilizador escolheu: acompanhar o Windows (o padrao) ou
/// forcar claro ou escuro. Guarda-se em `<data_dir>/theme`, uma palavra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeChoice {
    System,
    Light,
    Dark,
}

/// Indice em `ThemeChoice::ALL` da escolha em vigor.
static THEME_CHOICE: AtomicUsize = AtomicUsize::new(0);

impl ThemeChoice {
    const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    fn label(self) -> &'static str {
        match self {
            Self::System => "Tema do sistema",
            Self::Light => "Tema claro",
            Self::Dark => "Tema escuro",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::System => "sistema",
            Self::Light => "claro",
            Self::Dark => "escuro",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "sistema" | "system" | "auto" => Some(Self::System),
            "claro" | "light" => Some(Self::Light),
            "escuro" | "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn current() -> Self {
        Self::ALL
            .get(THEME_CHOICE.load(Ordering::Acquire))
            .copied()
            .unwrap_or(Self::System)
    }

    /// Escuro ou claro, dado o que o Windows diz agora.
    fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Self::System => system_dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }

    /// O `prefers-color-scheme` das paginas no WebView2.
    fn webview_theme(self) -> wry::Theme {
        match self {
            Self::System => wry::Theme::Auto,
            Self::Light => wry::Theme::Light,
            Self::Dark => wry::Theme::Dark,
        }
    }

    /// Sem ficheiro, ou com lixo dentro, vale o padrao: acompanhar o Windows.
    fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| Self::parse(&text))
            .unwrap_or(Self::System)
    }

    /// Escreve num temporario ao lado e renomeia: um arranque a meio de uma
    /// escrita nunca le meia palavra.
    fn save(self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, self.word())?;
        std::fs::rename(&temp, path)
    }

    /// Passa a valer ja: a proxima leitura do tema usa esta escolha.
    fn apply(self) {
        let index = Self::ALL
            .iter()
            .position(|choice| *choice == self)
            .unwrap_or(0);
        THEME_CHOICE.store(index, Ordering::Release);
        Theme::invalidate();
    }
}

/// Um WebViewBuilder que ja nasce com o tema escolhido nas paginas.
fn themed_webview_builder<'a>() -> WebViewBuilder<'a> {
    use wry::WebViewBuilderExtWindows;
    WebViewBuilder::new().with_theme(ThemeChoice::current().webview_theme())
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
        Self::read_for(ThemeChoice::current())
    }

    /// O tema que `choice` da agora: a escolha manda, o Windows so desempata
    /// em `ThemeChoice::System`.
    fn read_for(choice: ThemeChoice) -> Self {
        let accent = system_accent();
        if choice.is_dark(system_dark_mode()) {
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
    let pixels = pill_pixels(width, height, radius, &|_| fill, border, background);
    blit_bgrx(
        hdc,
        &pixels,
        rect.x.round() as i32,
        rect.y.round() as i32,
        width,
        height,
    );
}

/// Pixels BGRX de uma pilula com contorno suave. `fill_at(t)` da a cor do
/// corpo na fraccao `t` da largura (0 a esquerda, 1 a direita): cor unica nos
/// botoes normais, degradê no "Ir" sob o rato. Sem borda, o fio tem a cor do
/// proprio corpo.
fn pill_pixels(
    width: i32,
    height: i32,
    radius: f64,
    fill_at: &dyn Fn(f32) -> Rgb,
    border: Option<(Rgb, f64)>,
    background: Rgb,
) -> Vec<u8> {
    let border_width = border.map_or(0.0, |(_, width)| width) as f32;
    let radius = radius.min(width.min(height) as f64 / 2.0).max(0.0) as f32;

    let mut pixels = Vec::with_capacity((width.max(0) * height.max(0) * 4) as usize);
    for py in 0..height {
        for px in 0..width {
            let fill = fill_at((px as f32 + 0.5) / width as f32);
            let border_color = border.map_or(fill, |(color, _)| color);
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
    pixels
}

/// Fim do degradê do "Ir" sob o rato; o inicio e a cor de destaque do tema.
const GO_GRADIENT_END: Rgb = (124, 58, 237);
/// O que o aviso do meio da janela diz quando a rolagem muda.
fn auto_scroll_message(on: bool) -> String {
    if on {
        format!("Rolagem automática ligada — {AUTO_SCROLL_SECONDS}s  ·  Ctrl+R desliga")
    } else {
        "Rolagem automática desligada  ·  Ctrl+R liga".to_string()
    }
}

/// Apagar TUDO so com um "Sim" explicito. Fechar a caixa, "Nao" ou uma caixa
/// que nem abriu (0) deixam o historico como estava.
fn clear_history_confirmed(answer: i32) -> bool {
    answer == IDYES
}

/// Uma volta completa do degradê a deslizar.
const GO_GRADIENT_PERIOD_MS: u64 = 2400;

/// Cor do degradê do "Ir" na fraccao `t` da largura. A `phase` (0..1) faz a
/// onda deslizar com o tempo: o botao "respira" enquanto o rato esta nele.
fn go_gradient_color(from: Rgb, to: Rgb, t: f32, phase: f32) -> Rgb {
    let wave = 0.5 - 0.5 * (std::f32::consts::TAU * (t * 0.5 + phase)).cos();
    mix(from, to, wave)
}

/// O rato esta sobre o "Ir"? So na Home; noutras superficies o rect da Home
/// nao existe e o botao nao se acende por baixo das colunas.
fn home_go_hovered(surface: Surface, size: (f64, f64), scale: f64, cursor: (f64, f64)) -> bool {
    surface == Surface::Home
        && HomeLayout::new(size.0, size.1, scale)
            .go
            .contains(cursor.0, cursor.1)
}

/// O "Ir" em degradê, texto legivel sobre o meio do degradê.
unsafe fn draw_go_gradient(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    phase: f32,
    font: *mut core::ffi::c_void,
    theme: &Theme,
) {
    let width = rect.width.round() as i32;
    let height = rect.height.round() as i32;
    if width <= 0 || height <= 0 {
        return;
    }
    let (from, to) = (theme.accent, GO_GRADIENT_END);
    let pixels = pill_pixels(
        width,
        height,
        rect.height / 2.0,
        &|t| go_gradient_color(from, to, t, phase),
        None,
        theme.page_bg,
    );
    blit_bgrx(
        hdc,
        &pixels,
        rect.x.round() as i32,
        rect.y.round() as i32,
        width,
        height,
    );
    SelectObject(hdc, font as _);
    SetTextColor(hdc, rgb3(on_color(mix(from, to, 0.5))));
    SetBkMode(hdc, TRANSPARENT as i32);
    let mut text_rect = RECT {
        left: rect.x.round() as i32,
        top: rect.y as i32,
        right: (rect.x + rect.width) as i32,
        bottom: (rect.y + rect.height) as i32,
    };
    draw_text(
        hdc,
        "Ir",
        &mut text_rect,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
    );
}

/// Slot 0..2 = icones das IAs, slot 3 = glifo da casa (pintado com a cor do tema).
const ICON_SLOT_HOME: usize = COMPARATOR_COLUMNS;
/// Icones dos botoes do canto direito (gerados por scripts/gen-ai-icons.py).
const ICON_SLOT_VIDEO: usize = COMPARATOR_COLUMNS + 1;
const ICON_SLOT_WHATSAPP: usize = COMPARATOR_COLUMNS + 2;
const ICON_SLOT_YOUTUBE: usize = COMPARATOR_COLUMNS + 3;
const ICON_SLOT_MAIL: usize = COMPARATOR_COLUMNS + 4;
const ICON_SLOT_INCOGNITO: usize = COMPARATOR_COLUMNS + 5;
static EXTRA_ICON_IMAGES: [OnceLock<RgbaImage>; 5] = [const { OnceLock::new() }; 5];

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

fn extra_icon(slot: usize) -> &'static RgbaImage {
    let index = slot
        .saturating_sub(ICON_SLOT_VIDEO)
        .min(EXTRA_ICON_IMAGES.len() - 1);
    EXTRA_ICON_IMAGES[index].get_or_init(|| {
        let raw: &[u8] = match slot {
            ICON_SLOT_VIDEO => include_bytes!("../../../assets/ai/video.png"),
            ICON_SLOT_WHATSAPP => include_bytes!("../../../assets/ai/whatsapp.png"),
            ICON_SLOT_YOUTUBE => include_bytes!("../../../assets/ai/youtube.png"),
            ICON_SLOT_MAIL => include_bytes!("../../../assets/ai/mail.png"),
            _ => include_bytes!("../../../assets/ai/incognito.png"),
        };
        image::load_from_memory(raw)
            .expect("assets/ai/*.png must be valid PNG")
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
    } else if slot >= ICON_SLOT_VIDEO {
        extra_icon(slot)
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
    let mut text_right = rect.x + rect.width - padding * 0.6;
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
        // Texto centrado usa a pilula inteira. Com a margem de 11 px dos dois
        // lados, um botao redondo de ~30 px ficava com ~10 px para o "+" e o
        // DT_END_ELLIPSIS desenhava "-." no lugar dele.
        None => {
            text_left = rect.x;
            text_right = rect.x + rect.width;
            format |= DT_CENTER;
        }
    }

    SelectObject(hdc, font as _);
    SetTextColor(hdc, rgb3(style.text));
    // O DC de um BeginPaint nasce OPAQUE com fundo branco. Quem chamava sem
    // SetBkMode (botao Home, botoes -/□/x) pintava o texto num rectangulo
    // branco por cima da pilula -- os "icones" viravam quadrados brancos.
    SetBkMode(hdc, TRANSPARENT as i32);
    let mut text_rect = RECT {
        left: text_left.round() as i32,
        top: rect.y as i32,
        right: text_right as i32,
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
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
  const colIndex = window.__neuralia_col_index;
  function act(action, args) {
    post(envelope(action, args));
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

  // Barra de selecao (pedido do dono: "quando eu selecionar um texto, tem
  // que aparecer a pergunta mandar para pesquisa ? ou copiar ? ou falar ?
  // 3 botoes"). Vive neste script porque e o que todas as paginas recebem.
  // Se faltar uma primitiva, a barra fica desligada e os atalhos seguem.
  let selectionBar = null;
  try { selectionBar = createSelectionBar(); } catch (err) { selectionBar = null; }

  function createSelectionBar() {
    // Capturas no document-created, antes de a pagina correr: trocar depois
    // getSelection, Selection.prototype, Range.prototype ou EventTarget nao
    // desliga a barra nem lhe muda o texto.
    function uncurry(fn) {
      if (typeof fn !== 'function') throw new TypeError('primitiva em falta');
      return Function.prototype.call.bind(fn);
    }
    function getterOf(proto, name) {
      const found = Object.getOwnPropertyDescriptor(proto, name);
      return uncurry(found && found.get);
    }
    function setterOf(proto, name) {
      const found = Object.getOwnPropertyDescriptor(proto, name);
      return uncurry(found && found.set);
    }
    const listen = uncurry(EventTarget.prototype.addEventListener);
    const setAttr = uncurry(Element.prototype.setAttribute);
    const setText = setterOf(Node.prototype, 'textContent');
    const setStyle = uncurry(CSSStyleDeclaration.prototype.setProperty);
    const connected = getterOf(Node.prototype, 'isConnected');
    const later = setTimeout;
    const cancelLater = clearTimeout;
    const thenOf = uncurry(Promise.prototype.then);
    const docSelection = uncurry(Document.prototype.getSelection);
    const selText = uncurry(Selection.prototype.toString);
    const selRange = uncurry(Selection.prototype.getRangeAt);
    const selCount = getterOf(Selection.prototype, 'rangeCount');
    const selCollapsed = getterOf(Selection.prototype, 'isCollapsed');
    const selAnchor = getterOf(Selection.prototype, 'anchorNode');
    const selFocus = getterOf(Selection.prototype, 'focusNode');
    const rangeRects = uncurry(Range.prototype.getClientRects);
    const rangeBox = uncurry(Range.prototype.getBoundingClientRect);
    const rangeCommon = getterOf(Range.prototype, 'commonAncestorContainer');
    const makeElement = uncurry(Document.prototype.createElement);
    const appendTo = uncurry(Node.prototype.appendChild);
    const holds = uncurry(Node.prototype.contains);
    const closestOf = uncurry(Element.prototype.closest);
    const shadowOf = uncurry(Element.prototype.attachShadow);
    const boxOf = uncurry(Element.prototype.getBoundingClientRect);
    const execCommand = typeof Document.prototype.execCommand === 'function'
      ? uncurry(Document.prototype.execCommand) : null;
    const clip = typeof navigator !== 'undefined' ? navigator.clipboard : null;
    const writeText = clip && typeof Clipboard === 'function'
      && typeof Clipboard.prototype.writeText === 'function'
      ? uncurry(Clipboard.prototype.writeText) : null;
    const navLang = typeof navigator !== 'undefined' ? String(navigator.language || '') : '';
    const synth = window.speechSynthesis || null;
    const Utterance = window.SpeechSynthesisUtterance || null;
    const speakNow = synth && Utterance ? uncurry(synth.speak) : null;
    const cancelSpeech = speakNow ? uncurry(synth.cancel) : null;
    const listVoices = speakNow ? uncurry(synth.getVoices) : null;
    const Sheet = typeof CSSStyleSheet === 'function' ? CSSStyleSheet : null;

    // Painel privado: o texto nunca sai para o comparador (historico e
    // memoria). O nativo tambem recusa o `search` destes WebViews.
    const searchAllowed = '__NEURALIA_PRIVATE__' === 'false';
    const SHOW_MAX = 5000;
    const SEARCH_MAX = 2000;
    const SHOW_DELAY_MS = 200;
    const FEEDBACK_MS = 1000;
    const VOICE_WAIT_MS = 1500;
    const SPEECH_CHUNK = 200;
    const SPEECH_ABBREVIATION = 5;
    const MARGIN = 8;
    const TOO_LONG = 'Seleção grande demais para pesquisar (máx. 2000 caracteres)';
    const LABELS = {
      search: '\u{1F50E} Pesquisar',
      copy: '\u{1F4CB} Copiar',
      copied: '✓ Copiado',
      speak: '\u{1F50A} Falar',
      stop: '⏹ Parar'
    };
    const CSS = [
      '.bar{display:flex;flex-wrap:wrap;align-items:center;gap:4px;padding:4px;',
      'border-radius:22px;background:#ffffff;color:#111314;',
      'border:1px solid rgba(0,0,0,.14);box-shadow:0 8px 28px rgba(0,0,0,.22);',
      'font:600 13px "Segoe UI",system-ui,sans-serif;max-width:420px;',
      'user-select:none;-webkit-user-select:none;cursor:default}',
      'button{all:unset;box-sizing:border-box;display:inline-flex;align-items:center;',
      'gap:6px;min-height:34px;padding:0 14px;border-radius:999px;cursor:pointer;',
      'color:inherit;font:inherit;white-space:nowrap}',
      'button:hover{background:rgba(0,0,0,.08)}',
      '.msg{display:none;flex-basis:100%;padding:6px 12px;font-weight:500;line-height:1.35}',
      '.msg.on{display:block}',
      '@media (prefers-color-scheme: dark){',
      '.bar{background:#1c1f22;color:#f1f3f4;border-color:rgba(255,255,255,.16);',
      'box-shadow:0 8px 28px rgba(0,0,0,.55)}',
      'button:hover{background:rgba(255,255,255,.12)}}'
    ].join('');

    let host = null;
    let bar = null;
    let note = null;
    const buttons = Object.create(null);
    let visible = false;
    let text = '';
    let shownAt = null;
    let showTimer = 0;
    let feedbackTimer = 0;
    let voiceTimer = 0;
    let waitingVoices = null;
    let voicesHooked = false;
    let speaking = false;
    let speechRun = 0;
    // A fala em curso fica referenciada: o Chromium recolhe uma
    // SpeechSynthesisUtterance sem dono e o 'end' dela nunca chega.
    let speechHold = null;

    function guard(fn) {
      return function (event) {
        try { fn(event); } catch (err) {}
      };
    }

    function important(node, name, value) {
      setStyle(node.style, name, value, 'important');
    }

    function elementOf(node) {
      if (!node) return null;
      return node.nodeType === 1 ? node : node.parentElement || null;
    }

    function editable(node) {
      const el = elementOf(node);
      if (!el) return false;
      const tag = String(el.tagName || '').toUpperCase();
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
      if (el.isContentEditable) return true;
      return !!closestOf(el,
        'input,textarea,select,[contenteditable=""],[contenteditable="true"],[contenteditable="plaintext-only"]');
    }

    function focusedDeep() {
      let el = document.activeElement;
      while (el && el.shadowRoot && el.shadowRoot.activeElement) {
        el = el.shadowRoot.activeElement;
      }
      return el;
    }

    function ours(node) {
      return !!host && !!node && (node === host || holds(host, node));
    }

    function codePoints(value) {
      let count = 0;
      for (let i = 0; i < value.length; i++) {
        const high = value.charCodeAt(i);
        if (high >= 0xD800 && high <= 0xDBFF && i + 1 < value.length) {
          const low = value.charCodeAt(i + 1);
          if (low >= 0xDC00 && low <= 0xDFFF) i++;
        }
        count++;
      }
      return count;
    }

    // O texto que vai para a pesquisa: o que o parser nativo aceita (sem
    // caracteres de controlo alem de \n e \t, UTF-16 bem formado).
    function searchable(value) {
      return String(value)
        .replace(/\r\n?/g, '\n')
        .replace(/[\u0000-\u0008\u000B-\u001F\u007F-\u009F]/g, ' ')
        .replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, '�')
        .trim();
    }

    function endRect(range) {
      const rects = rangeRects(range);
      for (let i = rects.length - 1; i >= 0; i--) {
        const r = rects[i];
        if (r && (r.width > 0 || r.height > 0)) return r;
      }
      const whole = rangeBox(range);
      return whole && (whole.width > 0 || whole.height > 0) ? whole : null;
    }

    // A selecao do utilizador, se for uma que a barra serve; null se nao.
    function snapshot() {
      const sel = docSelection(document);
      if (!sel || selCount(sel) < 1 || selCollapsed(sel)) return null;
      const raw = String(selText(sel));
      const size = codePoints(raw.trim());
      if (size < 1 || size > SHOW_MAX) return null;
      const range = selRange(sel, 0);
      const anchor = selAnchor(sel);
      const focus = selFocus(sel);
      if (ours(anchor) || ours(focus)) return null;
      if (editable(anchor) || editable(focus) || editable(rangeCommon(range))
          || editable(focusedDeep())) return null;
      const rect = endRect(range);
      return rect ? { text: raw, rect: rect } : null;
    }

    function selected() {
      const sel = docSelection(document);
      return !!sel && selCount(sel) > 0 && !selCollapsed(sel) ? sel : null;
    }

    function stillSelected() {
      const sel = selected();
      return !!sel && String(selText(sel)) === text;
    }

    function setLabel(name, label) {
      if (buttons[name]) setText(buttons[name], label);
    }

    // Mensagem dentro da barra; a barra cresce e volta a caber no ecra.
    function say(message) {
      if (!note) return;
      setText(note, message);
      setAttr(note, 'class', message ? 'msg on' : 'msg');
      if (visible && shownAt) place(shownAt);
    }

    function clearFeedback() {
      if (feedbackTimer) { cancelLater(feedbackTimer); feedbackTimer = 0; }
      setLabel('copy', LABELS.copy);
    }

    function clearVoiceWait() {
      waitingVoices = null;
      if (voiceTimer) { cancelLater(voiceTimer); voiceTimer = 0; }
    }

    function button(name, label, run) {
      const b = makeElement(document, 'button');
      setAttr(b, 'type', 'button');
      // Fora da ordem do Tab e sem foco ao clicar: o foco fica na pagina.
      setAttr(b, 'tabindex', '-1');
      setAttr(b, 'data-action', name);
      setText(b, label);
      // mousedown sem efeito por omissao: o foco e a selecao ficam na pagina.
      listen(b, 'mousedown', guard(function (e) { e.preventDefault(); }));
      listen(b, 'click', guard(function (e) {
        if (!e.isTrusted) { return; }
        e.preventDefault();
        e.stopPropagation();
        run();
      }));
      buttons[name] = b;
      appendTo(bar, b);
    }

    function build() {
      if (host) {
        if (!connected(host)) appendTo(document.documentElement, host);
        return;
      }
      host = makeElement(document, 'div');
      important(host, 'all', 'initial');
      important(host, 'position', 'fixed');
      important(host, 'z-index', '2147483647');
      important(host, 'display', 'none');
      important(host, 'top', '0px');
      important(host, 'left', '0px');
      // Fechada: CSS e JS da pagina nao leem nem restilizam o que ha dentro.
      const root = shadowOf(host, { mode: 'closed' });
      // Folha construida primeiro: um CSP de style-src sem 'unsafe-inline'
      // bloqueia um <style>, nao uma CSSStyleSheet adotada.
      let styled = false;
      if (Sheet) {
        try {
          const sheet = new Sheet();
          sheet.replaceSync(CSS);
          root.adoptedStyleSheets = [sheet];
          styled = true;
        } catch (err) { styled = false; }
      }
      if (!styled) {
        const style = makeElement(document, 'style');
        setText(style, CSS);
        appendTo(root, style);
      }
      bar = makeElement(document, 'div');
      setAttr(bar, 'class', 'bar');
      setAttr(bar, 'role', 'toolbar');
      setAttr(bar, 'aria-label', 'Texto selecionado');
      appendTo(root, bar);
      if (searchAllowed) button('search', LABELS.search, search);
      button('copy', LABELS.copy, copy);
      if (speakNow) button('speak', LABELS.speak, toggleSpeech);
      note = makeElement(document, 'div');
      setAttr(note, 'class', 'msg');
      setAttr(note, 'role', 'status');
      appendTo(bar, note);
      appendTo(document.documentElement, host);
    }

    // Perto do fim da selecao: por cima se couber, senao por baixo; sempre
    // dentro da area visivel (sem a barra de rolagem).
    function place(rect) {
      const root = document.documentElement;
      let vw = window.innerWidth || 0;
      let vh = window.innerHeight || 0;
      if (root && root.clientWidth > 0 && root.clientWidth < vw) vw = root.clientWidth;
      if (root && root.clientHeight > 0 && root.clientHeight < vh) vh = root.clientHeight;
      const box = boxOf(host);
      const w = box && box.width > 0 ? box.width : 320;
      const h = box && box.height > 0 ? box.height : 44;
      let top = rect.top - h - MARGIN;
      if (top < MARGIN) top = rect.bottom + MARGIN;
      if (top + h > vh - MARGIN) top = vh - h - MARGIN;
      if (top < MARGIN) top = MARGIN;
      let left = rect.right - w / 2;
      if (left + w > vw - MARGIN) left = vw - w - MARGIN;
      if (left < MARGIN) left = MARGIN;
      important(host, 'top', Math.round(top) + 'px');
      important(host, 'left', Math.round(left) + 'px');
    }

    function show(snap) {
      build();
      text = snap.text;
      shownAt = snap.rect;
      clearFeedback();
      say('');
      important(host, 'visibility', 'hidden');
      important(host, 'display', 'block');
      place(shownAt);
      important(host, 'visibility', 'visible');
      visible = true;
    }

    // Escondida, a barra nao deixa relogio nenhum a correr.
    function hide() {
      if (showTimer) { cancelLater(showTimer); showTimer = 0; }
      clearFeedback();
      if (host) important(host, 'display', 'none');
      visible = false;
      text = '';
      shownAt = null;
    }

    function check() {
      showTimer = 0;
      if (speaking) return;
      const snap = snapshot();
      if (snap) show(snap); else hide();
    }

    // Um clique sem selecao nao deixa relogio nenhum a correr.
    function schedule() {
      if (speaking) return;
      if (!selected()) { hide(); return; }
      if (showTimer) cancelLater(showTimer);
      showTimer = later(guard(check), SHOW_DELAY_MS);
    }

    // Esc: fecha a barra (e cala a leitura) em vez de voltar atras. Sem
    // barra a vista, o Esc segue para o 'back' de sempre.
    function dismiss() {
      const busy = visible || speaking;
      stopSpeech();
      hide();
      return busy;
    }

    // A pergunta vai inteira ou nao vai: acima do tecto nada sai da pagina
    // e a barra diz porque.
    function search() {
      const question = searchable(text);
      if (!question) { hide(); return; }
      if (codePoints(question) > SEARCH_MAX) { say(TOO_LONG); return; }
      stopSpeech();
      // Envelope montado so com strings: o serializador nunca ve um objeto
      // em que a pagina possa pendurar um toJSON.
      post('{"v":1,"cap":"' + capability + '","action":"search","args":{"text":'
        + stringify(question) + '}}');
      hide();
    }

    function copied(ok) {
      if (!visible) return;
      if (!ok) { say('Não foi possível copiar'); return; }
      setLabel('copy', LABELS.copied);
      if (feedbackTimer) cancelLater(feedbackTimer);
      feedbackTimer = later(guard(function () {
        feedbackTimer = 0;
        setLabel('copy', LABELS.copy);
      }), FEEDBACK_MS);
    }

    function copyByCommand() {
      let ok = false;
      try { ok = !!(execCommand && execCommand(document, 'copy')); } catch (err) { ok = false; }
      copied(ok);
    }

    function copy() {
      if (!writeText) { copyByCommand(); return; }
      try {
        thenOf(writeText(clip, text), guard(function () { copied(true); }), guard(copyByCommand));
      } catch (err) {
        copyByCommand();
      }
    }

    function langTag(value) {
      return String(value || '').replace(/_/g, '-').toLowerCase();
    }

    // Voz local (offline) na lingua da pagina; senao pt-BR; senao a do
    // sistema; senao qualquer voz local. Nunca uma voz online por omissao.
    function chooseVoice(voices) {
      const local = [];
      for (let i = 0; i < voices.length; i++) {
        if (voices[i] && voices[i].localService !== false) local.push(voices[i]);
      }
      let pageLang = '';
      try { pageLang = document.documentElement.lang; } catch (err) { pageLang = ''; }
      const wanted = [pageLang, 'pt-BR', navLang];
      for (let w = 0; w < wanted.length; w++) {
        const tag = langTag(wanted[w]);
        if (!tag) continue;
        const base = tag.split('-')[0];
        let loose = null;
        for (let i = 0; i < local.length; i++) {
          const have = langTag(local[i].lang);
          if (have === tag) return local[i];
          if (!loose && have.split('-')[0] === base) loose = local[i];
        }
        if (loose) return loose;
      }
      for (let i = 0; i < local.length; i++) {
        if (local[i].default) return local[i];
      }
      return local.length ? local[0] : null;
    }

    // O Chromium corta falas longas: uma frase por fala. Uma abreviatura
    // solta ("Sr.", "Fig.") cola-se a frase seguinte; uma frase enorme parte
    // em palavras.
    function sentences(value) {
      const out = [];
      let current = '';
      const pieces = String(value).split(/\n+|(?<=[.!?…;:])\s+/);
      for (let i = 0; i < pieces.length; i++) {
        let piece = String(pieces[i] || '').replace(/\s+/g, ' ').trim();
        while (piece.length > SPEECH_CHUNK) {
          let cut = piece.lastIndexOf(' ', SPEECH_CHUNK);
          if (cut < SPEECH_CHUNK / 2) {
            // Sem espaco: corte seco, mas nunca a meio de um par UTF-16.
            cut = SPEECH_CHUNK;
            const high = piece.charCodeAt(cut - 1);
            if (high >= 0xD800 && high <= 0xDBFF) cut--;
          }
          if (current) { out.push(current); current = ''; }
          out.push(piece.slice(0, cut).trim());
          piece = piece.slice(cut).trim();
        }
        if (!piece) continue;
        if (current && current.length <= SPEECH_ABBREVIATION && current.indexOf(' ') < 0
            && current.length + 1 + piece.length <= SPEECH_CHUNK) {
          current = current + ' ' + piece;
        } else {
          if (current) out.push(current);
          current = piece;
        }
      }
      if (current) out.push(current);
      return out;
    }

    function withVoice(run, done) {
      if (listVoices(synth).length) { done(chooseVoice(listVoices(synth))); return; }
      // As vozes chegam depois ('voiceschanged'); a primeira lista vem vazia.
      waitingVoices = function () {
        if (run !== speechRun) return;
        clearVoiceWait();
        done(chooseVoice(listVoices(synth)));
      };
      if (!voicesHooked) {
        voicesHooked = true;
        listen(synth, 'voiceschanged', guard(function () {
          if (waitingVoices) waitingVoices();
        }));
      }
      voiceTimer = later(guard(function () {
        voiceTimer = 0;
        if (waitingVoices) waitingVoices();
      }), VOICE_WAIT_MS);
    }

    function stopSpeech() {
      speechRun++;
      clearVoiceWait();
      speechHold = null;
      if (speaking) {
        speaking = false;
        try { cancelSpeech(synth); } catch (err) {}
      }
      setLabel('speak', LABELS.speak);
    }

    function spoken(run) {
      if (run !== speechRun) return;
      speaking = false;
      speechHold = null;
      setLabel('speak', LABELS.speak);
      if (!stillSelected()) hide();
    }

    function toggleSpeech() {
      if (speaking) { stopSpeech(); return; }
      const parts = sentences(text);
      if (!parts.length) return;
      speechRun++;
      const run = speechRun;
      speaking = true;
      setLabel('speak', LABELS.stop);
      withVoice(run, function (voice) {
        if (run !== speechRun) return;
        if (!voice) {
          speaking = false;
          setLabel('speak', LABELS.speak);
          say('Nenhuma voz local disponível');
          return;
        }
        try { cancelSpeech(synth); } catch (err) {}
        let next = 0;
        function sayNext() {
          if (run !== speechRun) return;
          if (next >= parts.length) { spoken(run); return; }
          const utterance = new Utterance(parts[next++]);
          utterance.voice = voice;
          utterance.lang = voice.lang;
          listen(utterance, 'end', guard(sayNext));
          listen(utterance, 'error', guard(function () { spoken(run); }));
          speechHold = utterance;
          speakNow(synth, utterance);
        }
        sayNext();
      });
    }

    listen(window, 'mouseup', guard(function (e) {
      if (!e.isTrusted || e.button !== 0 || ours(e.target)) return;
      schedule();
    }), true);

    listen(window, 'keyup', guard(function (e) {
      if (!e.isTrusted) return;
      const key = String(e.key || '').toLowerCase();
      if (e.shiftKey || key === 'shift' || key === 'control' || key === 'meta'
          || key.indexOf('arrow') === 0 || key === 'home' || key === 'end'
          || key === 'pageup' || key === 'pagedown'
          || ((e.ctrlKey || e.metaKey) && key === 'a')) {
        schedule();
      }
    }), true);

    listen(window, 'mousedown', guard(function (e) {
      if (ours(e.target)) { e.preventDefault(); return; }
      if (!speaking) hide();
    }), true);

    // Duplo clique nos botoes nao chega a pagina (no comparador expandia a
    // coluna).
    listen(window, 'dblclick', guard(function (e) {
      if (ours(e.target)) { e.stopImmediatePropagation(); }
    }), true);

    listen(document, 'selectionchange', guard(function () {
      if (visible && !speaking && !stillSelected()) hide();
    }));

    const quietHide = guard(function () { if (!speaking) hide(); });
    listen(window, 'scroll', quietHide, true);
    listen(window, 'resize', quietHide);
    listen(window, 'popstate', quietHide);
    listen(window, 'hashchange', quietHide);
    listen(window, 'blur', guard(function () { if (!speaking) hide(); }));
    listen(window, 'pagehide', guard(function () { stopSpeech(); hide(); }));

    return {
      dismiss: function () {
        try { return dismiss(); } catch (err) { return false; }
      }
    };
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
        // Ctrl+R liga/desliga a rolagem automatica (pedido do dono);
        // recarregar fica no F5 e no Ctrl+Shift+R.
        case 'r': e.preventDefault(); act('autoscroll'); return;
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
    // Com a barra de selecao aberta (ou a ler), o Esc fecha-a e fica por ai.
    if (key === 'escape' && selectionBar && selectionBar.dismiss()) {
      e.preventDefault(); e.stopPropagation(); return;
    }
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
        act('shortcut-expand', { col:(parseInt(key, 10) - 1) });
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
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
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
      post(envelope('home', {}));
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
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
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

    post(envelope('gmail-state', {
      count, sender:first.sender, subject:first.subject, key:first.key
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

  // Claude pode redireccionar /new?q=... antes de o compositor ficar pronto.
  // Guardamos a consulta no sessionStorage no primeiro documento e retomamos
  // no seguinte. Assim o auto-submit nao depende de o fornecedor preservar o
  // parametro q durante toda a montagem da SPA.
  function storageRead(key) {
    try { return String(sessionStorage.getItem(key) || ''); } catch (_) { return ''; }
  }
  function storageWrite(key, value) {
    try { sessionStorage.setItem(key, String(value)); } catch (_) {}
  }
  function storageRemove(key) {
    try { sessionStorage.removeItem(key); } catch (_) {}
  }
  function stampRead(key) {
    return Number(storageRead(key) || '0');
  }
  function stampWrite(key, value) {
    storageWrite(key, value);
  }

  let urlQuery = '';
  try { urlQuery = String(new URL(location.href).searchParams.get('q') || '').trim(); } catch (_) {}
  const pendingKey = 'neuralia:pending-query:' + host;
  if (urlQuery) storageWrite(pendingKey, urlQuery);
  const query = (urlQuery || storageRead(pendingKey)).trim();
  if (!query) return;

  const stampKey = 'neuralia:auto-submit:' + host + ':' + query;
  if (Date.now() - stampRead(stampKey) < 10000) {
    storageRemove(pendingKey);
    return;
  }

  function finish() {
    stampWrite(stampKey, Date.now());
    storageRemove(pendingKey);
  }

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
      : ['button[data-testid="send-button"]', 'button[aria-label*="Send"]', 'button[aria-label*="Enviar"]', 'button[data-testid*="send"]', 'form button[type="submit"]'];
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
    // No ChatGPT o desaparecimento de q e um sinal real de submissao. No
    // Claude e apenas parte do redirect de /new para a SPA, portanto nao pode
    // encerrar o auto-submit antes de o compositor sequer existir.
    if (host === 'claude.ai') return false;
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
      finish();
      return;
    }

    const el = editor();

    // Depois de uma tentativa, o compositor vazio e o melhor reconhecimento
    // transversal de que o site aceitou a pergunta. Nao ha novo clique.
    if (lastSubmitAt && el && !textOf(el)) {
      finish();
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
    storageRemove(pendingKey);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => setTimeout(submitWhenReady, 100), { once:true });
  } else {
    setTimeout(submitWhenReady, 100);
  }
})();
"#;

const AGENT_OBSERVER_SCRIPT: &str = concat!(
    r#"
(function () {
  if (window.top !== window) return;
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const defer = setTimeout;
  let generation = 0;
  let lastMaterial = '';
  let timer = 0;
  // Um id por ELEMENTO, dado uma vez e nunca renomeado. Os ids por geracao
  // mudavam a cada observacao: o proprio setAttribute disparava o
  // MutationObserver, a observacao seguinte trazia ids novos e o material
  // nunca repetia, por isso os ids mudavam a cada ~700 ms. Um clique aprovado
  // depois de o utilizador ler o dialogo procurava um id que ja nao existia e
  // nao fazia nada. Um clone copia o atributo mas nao a entrada do mapa, e
  // recebe um id seu.
  const agentIds = new WeakMap();
  let nextAgentId = 0;
"#,
    agent_element_identity_js!(),
    r#"
  // Bytes UTF-8 que uma unidade UTF-16 ocupa depois de serializada em JSON, no
  // pior caso: controlo e surrogate viram \uXXXX (6), aspas e barra levam
  // escape (2).
  function unitCost(code) {
    if (code < 0x20 || (code >= 0xd800 && code <= 0xdfff)) return 6;
    if (code === 0x22 || code === 0x5c) return 2;
    if (code < 0x80) return 1;
    return code < 0x800 ? 2 : 3;
  }

  function jsonCost(value) {
    let total = 0;
    for (let i = 0; i < value.length; i++) total += unitCost(value.charCodeAt(i));
    return total;
  }

  // O prefixo mais longo que cabe em `budget`, sem deixar um surrogate alto
  // sozinho no fim.
  function fitJson(value, budget) {
    let total = 0;
    let end = 0;
    for (; end < value.length; end++) {
      const cost = unitCost(value.charCodeAt(end));
      if (total + cost > budget) break;
      total += cost;
    }
    const code = end > 0 ? value.charCodeAt(end - 1) : 0;
    return value.slice(0, code >= 0xd800 && code <= 0xdbff ? end - 1 : end);
  }

  function observe() {
    timer = 0;
    const root = document.querySelector('main,[role="main"]') || document.body || document.documentElement;
    const pageText = clean(root ? (root.innerText || root.textContent) : '', 1600);
    const candidates = document.querySelectorAll(
      'input,textarea,select,button,a[href],[role="button"],[role="textbox"],[role="combobox"]'
    );
    const rows = [];
    for (const el of candidates) {
      if (rows.length >= 32) break;
      const rect = el.getBoundingClientRect();
      const css = getComputedStyle(el);
      if (rect.width <= 0 || rect.height <= 0 || css.display === 'none' || css.visibility === 'hidden') continue;
      let id = agentIds.get(el);
      if (!id) {
        nextAgentId += 1;
        id = 'n' + nextAgentId;
        agentIds.set(el, id);
      }
      if (el.getAttribute('data-neuralia-agent-id') !== id) el.setAttribute('data-neuralia-agent-id', id);
      rows.push([id, fieldRole(el), elementName(el), clean(el.tagName, 20), el.disabled ? '0' : '1'].join('\t'));
    }

    const material = [location.href, document.title || '', pageText, rows.join('\n')].join('\n');
    if (material === lastMaterial) return;
    lastMaterial = material;
    generation += 1;

    // O envelope nativo aceita no maximo 8 KiB. Antes cortava-se a string
    // inteira a 1200 unidades, e as linhas de elementos vinham no fim: numa
    // pagina com mais de ~1.1K caracteres de texto o agente deixava de ver
    // qualquer controlo, e um URL longo levava ate a linha do titulo. Agora
    // cada parte paga o seu custo em bytes do JSON no pior caso: o cabecalho
    // vai inteiro, as linhas entram antes do texto (com uma reserva para ele)
    // e o texto fica com o que sobra. 7000 bytes deixam >1 KiB para
    // cap/action/args.
    const head = [String(generation), clean(location.href, 1200), clean(document.title, 256)].join('\n');
    let left = 7000 - jsonCost(head) - jsonCost('\n');
    const textReserve = Math.min(jsonCost(pageText), 1500);
    const kept = [];
    for (const row of rows) {
      const rowCost = jsonCost('\n' + row);
      if (rowCost > left - textReserve) break;
      kept.push(row);
      left -= rowCost;
    }
    const payload = [head, fitJson(pageText, left)].concat(kept).join('\n');
    post(envelope('agent-observation', { data:payload }));
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
"#
);

const SPLIT_SCROLL_RAIL_SCRIPT: &str = r#"
(function () {
  // WRY/WebView2 injeta initialization scripts em child frames no Windows:
  // o rail e o CSS que esconde as barras so pertencem ao documento principal.
  if (window.top !== window) return;
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
})();
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
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
  const defer = setTimeout;
  const cancelDefer = clearTimeout;
  function act(action, args) {
    post(envelope(action, args));
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
  const LINK_SELECTOR = 'a[href],area[href],[role="link"],[data-href],[data-url]';

  function linkNodeFromEvent(event) {
    // React/Shadow DOM pode retargetear event.target para um host que nao e o
    // <a> real. composedPath devolve o caminho original atraves das sombras.
    const path = typeof event.composedPath === 'function'
      ? event.composedPath()
      : [event.target];

    for (const candidate of path) {
      if (!candidate || candidate === window || candidate === document) continue;
      if (candidate.matches && candidate.matches(LINK_SELECTOR)) return candidate;
      if (candidate.closest) {
        const found = candidate.closest(LINK_SELECTOR);
        if (found) return found;
      }
    }
    return null;
  }

  function neuraliaControlFromEvent(event) {
    const path = typeof event.composedPath === 'function'
      ? event.composedPath()
      : [event.target];
    return path.some((candidate) => candidate && candidate.closest
      && candidate.closest('#neuralia-comp-controls,#neuralia-palette'));
  }

  function linkUrl(anchor) {
    if (!anchor) return null;

    // href absoluto do DOM ganha de getAttribute: sites React podem montar a
    // URL relativa e trocar <base>. data-* cobre chips de fonte sem <a>.
    const raw = (typeof anchor.href === 'string' && anchor.href)
      || anchor.getAttribute('href')
      || anchor.getAttribute('data-href')
      || anchor.getAttribute('data-url');
    if (!raw) return null;

    let target;
    try { target = new URL(raw, location.href); } catch (_) { return null; }
    if (target.protocol !== 'http:' && target.protocol !== 'https:') return null;

    // O Google embrulha as fontes em /url, /imgres, /aclk etc. O parametro
    // revela o destino real sem depender de um path especifico.
    const host = target.hostname.toLowerCase();
    if (host === 'google.com' || host.endsWith('.google.com')) {
      for (const name of GOOGLE_REDIRECT_PARAMS) {
        const actual = target.searchParams.get(name);
        if (!actual) continue;
        try {
          // So URL absoluta: em /search o `q` e um termo, e resolvido contra a
          // coluna virava uma URL falsa na origem da IA.
          const unwrapped = new URL(actual);
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
    if (!event.isTrusted) return false;
    // Alt e Shift continuam reservados ao comportamento nativo do navegador.
    if (event.altKey || event.shiftKey) return false;
    if (neuraliaControlFromEvent(event)) return false;

    const anchor = linkNodeFromEvent(event);
    const target = linkUrl(anchor);
    if (!target) return false;

    // Navegacao interna da propria IA continua com a SPA para nao perder a
    // conversa. Ctrl/meta/meio e links externos sao assumidos pelo NeuralIA.
    if (!aside && target.origin === location.origin) return false;

    // Capturamos no WINDOW, antes de handlers de document/React. Depois que o
    // NeuralIA assume o clique, nenhum listener concorrente pode navegar a
    // coluna ao mesmo tempo e criar click duplo/race com o IPC.
    event.preventDefault();
    event.stopImmediatePropagation();
    act('link', { col:colIndex, url:target.href, aside:aside });
    return true;
  }

  // UM so listener de clique no window (o gate scripts/test-link-routing.mjs
  // exige-o): primeiro o link; se nao era link, o botao de enviar da IA.
  listen(window, 'click', (event) => {
    if (event.button !== 0) return;
    if (routeLink(event, !!(event.ctrlKey || event.metaKey))) return;
    askFromSendButton(event);
  }, true);

  // O botao do meio usa auxclick. Captura no window pela mesma razao: sites de
  // IA costumam instalar handlers de document que chamam window.open primeiro.
  listen(window, 'auxclick', (event) => {
    if (event.button !== 1) return;
    routeLink(event, true);
  }, true);

  // Pergunta escrita numa coluna tambem pesquisa nas outras (pedido do
  // dono: "escrever pesquisar, tem que pesquisar em todos tambem").
  //  - Na pagina da propria IA (Google IA, ChatGPT, Claude, Gemini): o texto
  //    enviado com Enter ou com o botao de enviar vai as OUTRAS colunas
  //    ('ask'); esta segue a conversa dela.
  //  - Num site aberto por um link (as colunas no mesmo site): a pesquisa GET
  //    desse site abre o resultado em todas ('link'), tal como o clique.
  //  POST, senhas, e-mails e eventos sinteticos nunca sao replicados.
  const ASK_MAX = 2000;
  const ASK_REPEAT_MS = 2000;
  const SEND_LABEL = /\bsend\b|enviar|submit/i;
  const clock = Date.now;
  const toArray = Array.from;
  let lastAsk = { text: '', at: 0 };
  let lastComposer = null;

  function onProviderPage() {
    let here;
    try { here = new URL(location.href); } catch (_) { return false; }
    const host = here.hostname.toLowerCase();
    if (host === 'chatgpt.com' || host.endsWith('.chatgpt.com') || host === 'chat.openai.com') return true;
    if (host === 'claude.ai' || host.endsWith('.claude.ai')) return true;
    if (host === 'gemini.google.com') return true;
    return (host === 'google.com' || host.endsWith('.google.com'))
      && here.searchParams.get('udm') === '50';
  }

  // Texto de uma caixa onde se escreve uma pergunta; null para tudo o resto
  // (senhas, e-mails, codigos, botoes...).
  function composerText(node) {
    if (!node || !node.tagName) return null;
    const tag = String(node.tagName).toUpperCase();
    if (tag === 'TEXTAREA') return String(node.value || '');
    if (tag === 'INPUT') {
      const type = String(node.type || 'text').toLowerCase();
      if (type !== 'text' && type !== 'search') return null;
      const auto = String((node.getAttribute && node.getAttribute('autocomplete')) || '').toLowerCase();
      if (/user|mail|pass|code|tel|cc-/.test(auto)) return null;
      return String(node.value || '');
    }
    if (node.isContentEditable) return String(node.innerText || node.textContent || '');
    return null;
  }

  function pathOf(event) {
    return typeof event.composedPath === 'function' ? event.composedPath() : [event.target];
  }

  function composerFromEvent(event) {
    for (const candidate of pathOf(event)) {
      if (composerText(candidate) !== null) return candidate;
    }
    return null;
  }

  function sendAsk(text) {
    const clean = String(text || '').trim();
    if (!clean || clean.length > ASK_MAX) return;
    const at = clock();
    // Enter e o submit do mesmo formulario chegam os dois: uma pergunta so.
    if (clean === lastAsk.text && at - lastAsk.at < ASK_REPEAT_MS) return;
    lastAsk = { text: clean, at: at };
    act('ask', { col:colIndex, text:clean });
  }

  // Um GET de um formulario com texto escrito -> a URL que ele abriria.
  function formSearchUrl(form, submitter) {
    const method = String((form.getAttribute && form.getAttribute('method')) || 'get').toLowerCase();
    if (method !== 'get') return null;
    let target;
    try {
      target = new URL((form.getAttribute && form.getAttribute('action')) || location.href, location.href);
    } catch (_) { return null; }
    if (target.protocol !== 'http:' && target.protocol !== 'https:') return null;
    target.search = '';
    let typed = false;
    for (const field of toArray(form.elements || [])) {
      if (!field || !field.name || field.disabled) continue;
      const type = String(field.type || '').toLowerCase();
      if (type === 'password' || type === 'file' || type === 'email') return null;
      if ((type === 'checkbox' || type === 'radio') && !field.checked) continue;
      if ((type === 'submit' || type === 'button' || type === 'image' || type === 'reset')
          && field !== submitter) continue;
      const value = String(field.value == null ? '' : field.value);
      if ((type === 'search' || type === 'text' || type === 'textarea') && value.trim()) typed = true;
      target.searchParams.append(String(field.name), value);
    }
    return typed ? target : null;
  }

  listen(window, 'focusin', (event) => {
    if (!event.isTrusted) return;
    const node = composerFromEvent(event);
    if (node) lastComposer = node;
  }, true);

  listen(window, 'keydown', (event) => {
    if (!event.isTrusted || event.key !== 'Enter') return;
    // Shift+Enter e quebra de linha; durante a composicao (acentos, IME) o
    // Enter ainda nao e envio.
    if (event.shiftKey || event.ctrlKey || event.altKey || event.metaKey || event.isComposing) return;
    if (!onProviderPage() || neuraliaControlFromEvent(event)) return;
    const node = composerFromEvent(event);
    if (node) sendAsk(composerText(node));
  }, true);

  // Chamado pelo listener de clique do window, depois do encaminhamento de
  // links.
  function askFromSendButton(event) {
    if (!event.isTrusted || event.button !== 0 || !onProviderPage()) return;
    if (neuraliaControlFromEvent(event) || !lastComposer) return;
    const button = pathOf(event).find((node) => node && node.tagName
      && (String(node.tagName).toUpperCase() === 'BUTTON'
        || (node.getAttribute && node.getAttribute('role') === 'button')));
    if (!button || !button.getAttribute) return;
    const label = [
      button.getAttribute('aria-label'),
      button.getAttribute('data-testid'),
      button.getAttribute('title')
    ].join(' ');
    if (SEND_LABEL.test(label)) sendAsk(composerText(lastComposer));
  }

  listen(window, 'submit', (event) => {
    if (!event.isTrusted) return;
    const form = event.target;
    if (!form || !form.tagName || String(form.tagName).toUpperCase() !== 'FORM') return;
    if (onProviderPage()) {
      for (const field of toArray(form.elements || [])) {
        const text = composerText(field);
        if (text && text.trim()) { sendAsk(text); return; }
      }
      return;
    }
    const target = formSearchUrl(form, event.submitter || null);
    if (!target) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    act('link', { col:colIndex, url:target.href, aside:false });
  }, true);

  listen(document, 'dblclick', (event) => {
    if (!event.isTrusted || event.defaultPrevented) return;
    if (event.target && event.target.closest
        && event.target.closest(
          '#neuralia-comp-controls,#neuralia-palette,a[href],button,input,textarea,select,option,label,summary,[role="button"],[role="link"],[contenteditable="true"]'
        )) return;
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
