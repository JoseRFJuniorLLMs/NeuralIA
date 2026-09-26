#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    cell::Cell,
    collections::BinaryHeap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{Sender, SyncSender, channel, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

use image::RgbaImage;

use crate::epub_app::{EPUB_SCHEME, EpubJob, EpubNotice, EpubRuntime, EpubUiRequest};
use crate::gemini_live::{LiveIndicator, LiveMessage, LivePanel, live_theme_script};
#[cfg(test)]
use crate::ipc::constant_time_eq;
use crate::ipc::{
    ColumnHint, IpcAction, NoteVia, SEARCH_MAX_CHARS, SearchIntent, parse_ipc_message,
};
use crate::panel_chrome::{
    Area, CAPTION_HOT_MARGIN, CaptionReveal, PANEL_WIDTHS_FILE, PanelKind, PanelWidths,
    PanelWindowFullscreen, RevealStep, ScreenRect, ServiceBadge, ServiceInput, ServicePanelState,
    StripButton, WheelRoute, caption_hot_zone, is_escape_down, panel_area, panel_width,
    strip_buttons, strip_hit, wheel_message_params, wheel_route,
};
use crate::pomodoro_ui::{PomodoroController, TickSchedule, TickScheduler, phase_color};
use crate::read_aloud::READ_ALOUD_SCRIPT;
use crate::secrets::redact_debug_secrets;
use crate::tab_session::{self, Loaded, SessionColumn, SessionGroup, SessionTab, TabSession};
use neural_core::json_store::StoreRegistry;
use neural_core::{
    ActionRisk, AgentAction, AgentElement, AgentPermissionPolicy, AgentRuntimeConfig,
    AgentSecurityAction, CoreConfig, FieldKind, HistoryEntry, HistoryKind, HistoryStore, Intent,
    MemoryDocument, MemoryHit, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore, Note,
    ObservedPage, Phase, ReaderArticle, ReaderClient, ResearchItemKind, ResearchSession,
    ZettelError, ZettelStore, chatgpt_search_url, claude_search_url, google_ai_url,
    is_local_network_target, is_pdf_url, redact_sensitive_text, tissue,
    zettel::{self, is_valid_note_id},
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        BeginPaint, BitBlt, CLEARTYPE_QUALITY, ClientToScreen, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen, CreateRoundRectRgn,
        CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER,
        DT_EDITCONTROL, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK,
        DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FW_BOLD, FW_NORMAL, FillRect, GetDC,
        GetStockObject, InvalidateRect, LineTo, MoveToEx, NULL_BRUSH, OUT_DEFAULT_PRECIS,
        PAINTSTRUCT, PS_SOLID, ReleaseDC, SRCCOPY, ScreenToClient, SelectObject, SetBkColor,
        SetBkMode, SetTextColor, SetWindowRgn, StretchDIBits, TRANSPARENT,
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
            GetWindowThreadProcessId, IDYES, IsZoomed, MB_ICONINFORMATION, MB_OK, MB_YESNO,
            MF_SEPARATOR, MF_STRING, MessageBoxW, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
            SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetParent, SetWindowPos, ShowWindow,
            TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_CANCELMODE, WM_CAPTURECHANGED,
            WM_KEYDOWN, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_TABSTOP,
            WS_VISIBLE,
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
    window::{CursorIcon, Icon, Window, WindowId},
};
use wry::{NewWindowResponse, PermissionKind, PermissionResponse, WebView, WebViewBuilder};

use side_panel::PanelExit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum PageTarget {
    Column(usize),
    Split,
}

#[derive(Debug)]
pub(in crate::windows_app) enum UserEvent {
    /// O tema (`theme.rs`): a unica variante do modulo, com o enum dele
    /// dentro. E o padrao de cada feature: uma variante aqui, o resto la.
    Theme(ThemeEvent),
    /// O pedido de chave nativo (`secret_prompt.rs`): Enter com uma chave
    /// com a forma do slot, "Esquecer chave" ou cancelar.
    Keys(KeyEvent),
    /// Os ganchos das WebViews (`webview_hooks.rs`): o que cada WebView
    /// avisa, com o hospedeiro de onde veio.
    WebView(WebViewEvent),
    /// O bloqueio de anuncios (`adblock.rs`): os itens do menu e as threads
    /// que leem e baixam a lista.
    Adblock(AdblockEvent),
    /// Pedido da pagina local do painel lateral (canal proprio), com o
    /// numero da pagina que o mandou.
    Panel(side_panel::PanelPost),
    /// Resposta do worker das notas.
    NotesReady {
        origin: NotesOrigin,
        reply: NotesReply,
    },
    /// Ctrl+Shift+Z ou o "Salvar nota" da barra numa pagina: a nota DESSA
    /// WebView. `target` `None` e a WebView unica da web externa, do Leitor e
    /// do PDF; `via` diz qual dos dois gestos foi (e, no Salvar nota, traz o
    /// texto que a barra mostrava).
    NoteRequested {
        target: Option<PageTarget>,
        via: NoteVia,
    },
    /// Ctrl+Shift+Z no Split privado: recusado sem ler a pagina.
    NoteRefusedPrivate,
    /// O que a pagina devolveu ao `NOTE_CAPTURE_SCRIPT` (JSON, dado dela)
    /// num Ctrl+Shift+Z, e a fonte que o lado nativo conhece (o artigo do
    /// Leitor, o PDF). O Salvar nota nunca passa por aqui: o texto dele veio
    /// no pedido.
    NoteCaptured {
        raw: String,
        source: Option<String>,
    },
    /// Ctrl+Shift+Z na Home ou com o teclado na barra: nota nova em branco.
    NewNote,
    /// Pedido da pagina do painel do Gemini Live (canal proprio, lista
    /// fechada em `gemini_live::parse_live_message`).
    Live(LiveMessage),
    /// O aviso do canto (`toast.rs`, centro de avisos `crate::notify`):
    /// um clique num botao dele ou o fim do prazo.
    Notify(NotifyEvent),
    HomeRequested,
    /// Voltar um nivel: de ecra completo para tres colunas, de la para a Home.
    BackRequested,
    /// Outra janela ficou com o rato a meio do gesto numero N na fila de
    /// abas (WM_CAPTURECHANGED): o arrasto desse gesto cancela-se.
    TabCaptureLost(u64),
    ToggleAutoScroll,
    /// Clique no botao `index` da pergunta do meio da janela
    /// (`SplashQuestion`), de quem a fez.
    SplashAnswer {
        asker: SplashAsker,
        index: usize,
    },
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
    /// Tique do Pomodoro; so conta o da cadeia viva
    /// (`PomodoroController::tick`).
    PomodoroTick(u64),
    HideSplash(u64),
    GmailProbe(u64),
    GmailInboxState {
        unread: u32,
        sender: String,
        subject: String,
        key: String,
    },
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
    /// "Mandar para IA" ou "Traduzir" da barra de selecao. So PEDE: o texto
    /// aparece no cartao nativo ("Mandar para as 3 IAs?", "Traduzir nas 3
    /// IAs?") e so o clique nativo no botao de confirmar o leva as tres IAs
    /// (`App::compare`), como pergunta (ou dentro do pedido fixo de
    /// traducao) e nunca como comando da omnibox.
    SearchSelection {
        text: String,
        intent: SearchIntent,
    },
    /// Clique nativo num botao do cartao; `token` e o do cartao que estava
    /// pintado quando o botao foi solto, e `shown` quantos caracteres do
    /// texto essa pintura mostrou.
    SearchCardAnswer {
        token: u64,
        button: SearchCardButton,
        shown: usize,
    },
    /// Os segundos do cartao `token` passaram sem resposta: conta como
    /// Cancelar.
    SearchCardExpired(u64),
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
    /// Link que a propria fonte aberta ao lado mandou abrir noutra aba: a aba
    /// nova herda o grupo da aba de onde saiu.
    OpenSplitFromSplit {
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
    /// Voltar a olhar para os botoes da janela na Home: o rato saiu deles,
    /// ou chegou o prazo de os esconder.
    CaptionReveal,
    /// A pega da borda do painel da direita foi arrastada; a posicao vem de
    /// `PANEL_RESIZE_X` (os movimentos em fila substituem-se).
    ResizePanel,
    /// Largou a pega: gravar a largura.
    PanelResizeDone,
    /// A pagina do painel de servicos numero `generation` entrou (ou saiu)
    /// de tela cheia -- o botao de tela cheia do YouTube.
    ServiceFullscreen {
        generation: u64,
        on: bool,
    },
    /// A pagina do painel de servicos comecou (ou parou) de tocar som.
    ServiceAudio {
        generation: u64,
        playing: bool,
    },
    /// Esc no painel de servicos (AcceleratorKeyPressed do WebView2).
    ServiceEscape(u64),
    /// O rato entrou (ou saiu) do "−" ou do "⛶ <IA>" injetados na coluna:
    /// a dica centrada do app, com o texto escolhido aqui.
    ColumnHint {
        col: usize,
        hint: ColumnHint,
    },
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
    /// Grava as abas e os grupos, se este ainda for o ultimo pedido agendado.
    SaveTabSession(u64),
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
    /// Resultado do worker da biblioteca de livros (adicionados, removidos,
    /// marcador gravado, erro).
    EpubNotice(EpubNotice),
    /// Pedido das paginas EPUB que so a thread da interface pode atender
    /// (dialogo de arquivos, link externo, voltar a Home).
    EpubUi(EpubUiRequest),
    /// Arquivos largados sobre o WebView da biblioteca/leitor.
    EpubDropped(Vec<PathBuf>),
    /// O dialogo "Adicionar livros EPUB" (o comando `OpenEpub`: Ctrl+O na
    /// janela e na omnibox).
    OpenEpubDialog,
    /// Um atalho do mapa de teclas (`keymap.rs`): o comando `key`, a correr
    /// contra a origem de onde a tecla veio -- o hospedeiro da WebView que a
    /// recebeu, a janela ou a omnibox --, nunca contra nada da pagina.
    /// So `accelerator_decision` o constroi.
    RunCommandKey {
        key: CommandId,
        origin: CommandOrigin,
    },
    /// Uma linha do condutor do spike de aceleradores (so no build de CI
    /// com `--features accel-spike`; ver `accel_spike_app.rs`).
    #[cfg(feature = "accel-spike")]
    AccelSpike(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    Home,
    Reader,
    External,
    Comparator,
    /// O nosso visualizador de PDF (PDF.js embutido), numa origem nossa.
    Pdf,
    /// A biblioteca e o leitor de EPUB, na origem `neuralia-epub`.
    Epub,
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
/// Tecto, em chars, de cada campo que o monitor do Gmail nos envia. O script
/// ja corta a 180, mas o script corre numa pagina remota: o lado nativo nao
/// pode confiar nesse corte e repete-o antes de guardar ou pintar.
const GMAIL_FIELD_MAX_CHARS: usize = 180;

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
const SEARCH_CARD_SUBCLASS_ID: usize = 0x4E71;
/// Cartao de confirmacao da barra de selecao ("Mandar para as 3 IAs?",
/// "Traduzir nas 3 IAs?"), em pixeis logicos, centrado na janela. A caixa do
/// texto leva umas oito linhas: o que nao cabe nao vai.
const SEARCH_CARD_WIDTH: f64 = 600.0;
const SEARCH_CARD_HEIGHT: f64 = 340.0;
/// Sem resposta, o cartao some e conta como Cancelar.
const SEARCH_CARD_SECONDS: u64 = 12;
/// Um confirmar que chega antes disto, contado desde que o cartao (ou o texto
/// que o trocou) apareceu, nao conta: um duplo clique que a pagina pediu no
/// sitio onde o cartao ia nascer nao o confirma.
/// O cartao arma pelo `NATIVE_CARD_ARM` dos cartoes nativos; os gates do
/// cartao da barra leem-no por este nome.
#[cfg(test)]
const SEARCH_CARD_ARM: Duration = NATIVE_CARD_ARM;
/// O pedido fixo do Traduzir, escrito pelo nativo: as tres IAs recebem isto,
/// uma linha em branco e o texto que o cartao pintou.
const TRANSLATE_PROMPT: &str = "Traduza para o português do Brasil (se o texto já estiver em português, traduza para o inglês):";

/// O cartao a mostrar: (token, o botao da barra que o pediu -- que da o
/// titulo e o botao de confirmar --, a pergunta ja limpa por
/// `selection_question`).
static SEARCH_CARD_VIEW: Mutex<Option<(u64, SearchIntent, String)>> = Mutex::new(None);
/// O que o cartao pintou por ultimo: (token, caracteres da pergunta que
/// couberam e se viram). Um clique leva ISTO: o texto que o utilizador viu, e
/// nao um que o trocou e ainda nao foi pintado, nem o que ficou de fora.
static SEARCH_CARD_PAINTED: Mutex<(u64, usize)> = Mutex::new((0, 0));
/// O botao do cartao onde o rato desceu (indice de `SearchCardButton`).
static SEARCH_CARD_PRESSED: AtomicUsize = AtomicUsize::new(NATIVE_BUTTON_NONE);
/// Legenda da palette nativa (para onde vao URL e texto). Fora do App pela
/// mesma razao que SPLASH_TEXT: quem a pinta e o procedimento de janela.
static PALETTE_HINT: Mutex<String> = Mutex::new(String::new());

/// O que esta debaixo do rato no chrome nativo do comparador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum BarHit {
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
    /// O x de fechar dentro de uma aba (16 px, como no Chrome).
    CloseTab {
        source_index: usize,
        context_index: usize,
    },
    /// A pilula de um grupo. O indice e o do grupo dentro da coluna.
    ContextGroup {
        source_index: usize,
        group_index: usize,
    },
    /// O "‹N" de uma coluna: a lista de todas as abas dela, as que a barra
    /// nao mostra incluidas.
    TabOverflow(usize),
    SplitExpand,
    SplitClose,
    Private,
    /// A faixa por cima do painel de servicos: minimizar, tela cheia, fechar.
    ServiceStrip(StripButton),
    /// Icones do canto direito: servicos no painel e avisos do Gmail.
    Service(Service),
    GmailToggle,
    /// Ferramentas (Pomodoro, Notas, Respiracao), a esquerda do Gemini Live.
    Tool(Tool),
    /// O olho: liga e desliga o Gemini Live (tela, camera e microfone).
    GeminiLive,
    WindowMinimize,
    WindowMaximize,
    WindowClose,
}

/// O menu que o botao direito abre na barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarMenu {
    Tab {
        source_index: usize,
        context_index: usize,
    },
    Group {
        source_index: usize,
        group_index: usize,
    },
    /// A pilula de uma IA: o item da rolagem automatica (Ctrl+R).
    Column(usize),
}

/// Que menu o botao direito abre, pelo que esta debaixo do rato: o da aba
/// (tambem sobre o x dela), o do grupo (a pilula dele) ou o da IA (a pilula
/// da coluna). O resto da barra nao tem menu.
fn bar_menu_for(hit: Option<BarHit>) -> Option<BarMenu> {
    match hit? {
        BarHit::ContextTab {
            source_index,
            context_index,
        }
        | BarHit::CloseTab {
            source_index,
            context_index,
        } => Some(BarMenu::Tab {
            source_index,
            context_index,
        }),
        BarHit::ContextGroup {
            source_index,
            group_index,
        } => Some(BarMenu::Group {
            source_index,
            group_index,
        }),
        BarHit::Column(col_index) => Some(BarMenu::Column(col_index)),
        _ => None,
    }
}

/// Uma coluna do comparador. Generica na vista para os gates correrem sem
/// WebView (`note_read_view`); no app e a `WebView`.
pub(in crate::windows_app) struct ComparatorView<V = WebView> {
    pub(in crate::windows_app) webview: V,
    pub(in crate::windows_app) name: &'static str,
}

pub(in crate::windows_app) struct ComparatorState {
    pub(in crate::windows_app) views: Vec<ComparatorView>,
    pub(in crate::windows_app) expanded: Option<usize>,
    pub(in crate::windows_app) minimized: [bool; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) weights: [f64; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) split: Option<SplitView>,
    /// Abas/fontes agrupadas automaticamente pela IA que abriu cada link.
    pub(in crate::windows_app) contexts: [Vec<ContextTab>; COMPARATOR_COLUMNS],
    /// Grupos por coluna, na ordem em que aparecem na barra.
    pub(in crate::windows_app) groups: [Vec<ContextGroup>; COMPARATOR_COLUMNS],
    /// Contador dos ids de grupo. Nunca reutiliza.
    pub(in crate::windows_app) next_group_id: u64,
    /// Contador das identidades de abas. Nunca reutiliza durante a sessao.
    pub(in crate::windows_app) next_context_id: u64,
    /// Por coluna, a aba em que o dono acabou de mexer (largou-a, juntou-a a
    /// um grupo, mandou ver o grupo dela, abriu-a pela lista "‹N"): a barra
    /// mostra-a mesmo longe das mais recentes. Nao vai para o disco.
    pub(in crate::windows_app) bar_focus: [Option<u64>; COMPARATOR_COLUMNS],
    /// Largura logica ocupada a direita por um painel lateral aberto (0 sem
    /// painel): as colunas repartem so o que sobra, como no Chrome.
    pub(in crate::windows_app) panel_width: f64,
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
///
/// A etiqueta do Pomodoro nao e estado do comparador, mas muda a largura dos
/// controlos da direita; entra aqui para desenho e hit-testing a receberem
/// pelo mesmo caminho (ver `App::pomodoro_bar_label`).
fn bar_columns(comp: &ComparatorState, pomodoro_label: Option<BarLabel>) -> BarColumns {
    // Em ecra completo ou com a gaveta aberta o conteudo ja nao esta em
    // faixas por peso, por isso a barra tambem nao finge que esta: reparte-se
    // em partes iguais e nenhuma coluna vira chip.
    if comp.expanded.is_some() || comp.split.is_some() {
        return BarColumns {
            split_active: comp.split.is_some(),
            panel_width: comp.panel_width,
            pomodoro_label,
            ..BarColumns::even(comp.views.len())
        };
    }
    BarColumns {
        count: comp.views.len(),
        weights: comp.weights,
        minimized: comp.minimized,
        split_active: false,
        panel_width: comp.panel_width,
        pomodoro_label,
    }
}

/// As filas de abas das colunas, cada uma com a sua aba aberta ao lado (se a
/// gaveta for dela). Desenho, hit-testing e arrasto chamam isto -- nunca
/// montam as suas, senao o rato acertava noutro sitio que nao o desenhado.
#[cfg(test)]
fn tab_rows(
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    active: Option<(usize, Option<u64>)>,
) -> [TabRow; COMPARATOR_COLUMNS] {
    tab_rows_focused(contexts, groups, active, [None; COMPARATOR_COLUMNS])
}

/// `focus` e, por coluna, a aba em que o dono acabou de mexer
/// (`ComparatorState::bar_focus`).
fn tab_rows_focused(
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    active: Option<(usize, Option<u64>)>,
    focus: [Option<u64>; COMPARATOR_COLUMNS],
) -> [TabRow; COMPARATOR_COLUMNS] {
    std::array::from_fn(|index| {
        let open = active
            .filter(|(source, _)| *source == index)
            .and_then(|(_, id)| id);
        plan_tab_row_focused(&contexts[index], &groups[index], open, focus[index])
    })
}

/// A aba que fica na barra depois de largar `item`: a propria aba, ou a
/// primeira do grupo arrastado (a pilula vem antes dela).
fn drag_focus(tabs: &[ContextTab], item: DragItem) -> Option<u64> {
    match item {
        DragItem::Tab(id) => tabs.iter().any(|tab| tab.id == id).then_some(id),
        DragItem::Group(id) => tabs
            .iter()
            .find(|tab| tab.group == Some(id))
            .map(|tab| tab.id),
    }
}

/// A aba aberta ao lado: (coluna de onde veio, identidade da aba).
fn active_context(comp: &ComparatorState) -> Option<(usize, Option<u64>)> {
    comp.split
        .as_ref()
        .map(|split| (split.source_index, split.context_id))
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
pub(in crate::windows_app) fn visible_column_spans(
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
pub(in crate::windows_app) fn resized_weights(
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

/// O cartao "Mandar para as 3 IAs?" / "Traduzir nas 3 IAs?". Nativo e owned
/// pela janela principal: a pagina que escolheu o texto nao o tapa, nao o
/// move, nao o pinta e nao lhe manda cliques. Nao se ativa (como o aviso do Gmail): o foco fica onde
/// estava, por isso Enter e Esc nao lhe chegam -- responde-se com o rato.
unsafe extern "system" fn search_card_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    // Os cliques sao do cartao (um STATIC devolve HTTRANSPARENT e iam para
    // a pagina) e nao o ativam (`popup_no_activate_message`).
    if let Some(result) = popup_no_activate_message(message) {
        return result;
    }
    match message {
        WM_LBUTTONDOWN => {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
                if let Some(button) = search_card_hit(&client, search_card_scale(&client), x, y) {
                    SEARCH_CARD_PRESSED.store(button.index(), Ordering::Release);
                    SetCapture(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let captured = GetCapture() == hwnd;
            let pressed = take_native_pressed_button(&SEARCH_CARD_PRESSED, || {
                if captured {
                    ReleaseCapture();
                }
            });
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) == 0 || reference_data == 0 {
                return 0;
            }
            let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
            if let Some(button) = search_card_release(pressed, captured, &client, x, y) {
                let (token, shown) = SEARCH_CARD_PAINTED
                    .lock()
                    .map(|painted| *painted)
                    .unwrap_or((0, 0));
                let sink = &*(reference_data as *const SearchCardSink);
                sink(UserEvent::SearchCardAnswer {
                    token,
                    button,
                    shown,
                });
            }
            0
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            SEARCH_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
            0
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let (token, intent, text) = SEARCH_CARD_VIEW
                        .lock()
                        .ok()
                        .and_then(|view| view.clone())
                        .unwrap_or((0, SearchIntent::Ask, String::new()));
                    let shown = paint_search_card(hdc, &client, intent, &text);
                    // So agora o utilizador ve este texto: e ele -- e so a
                    // parte que coube -- que um clique a seguir confirma.
                    if let Ok(mut painted) = SEARCH_CARD_PAINTED.lock() {
                        *painted = (token, shown);
                    }
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

/// Pinta o cartao de `intent` (titulo e botao de confirmar) e devolve quantos
/// caracteres de `text` mostrou: todos, ou o inicio que coube na caixa (com
/// "…" e a conta do que ficou de fora).
unsafe fn paint_search_card(
    hdc: *mut core::ffi::c_void,
    client: &RECT,
    intent: SearchIntent,
    text: &str,
) -> usize {
    let theme = Theme::system();
    let background = CreateSolidBrush(rgb3(theme.surface));
    FillRect(hdc, client, background);
    DeleteObject(background as _);

    let scale = search_card_scale(client);
    let layout = search_card_layout(client, scale);
    let title_font = create_font((-22.0 * scale) as i32, FW_BOLD as i32);
    let text_font = create_font((-18.0 * scale) as i32, FW_NORMAL as i32);
    let button_font = create_font((-16.0 * scale) as i32, FW_BOLD as i32);
    let note_font = create_font((-14.0 * scale) as i32, FW_NORMAL as i32);
    let old_font = SelectObject(hdc, title_font as _);
    SetBkMode(hdc, TRANSPARENT as i32);

    SetTextColor(hdc, rgb3(theme.accent));
    let mut title = layout.title;
    draw_text(
        hdc,
        search_card_title(intent),
        &mut title,
        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    // O texto numa caixa propria, para se ver onde comeca e acaba. Mede-se
    // antes com a mesma fonte: o que nao cabe nao se pinta cortado, sai --
    // e a confirmacao so leva o que ficou (`search_card_shown`).
    let quote = CreateSolidBrush(rgb3(theme.page_bg));
    FillRect(hdc, &layout.quote, quote);
    DeleteObject(quote as _);
    SelectObject(hdc, text_font as _);
    SetTextColor(hdc, rgb3(theme.fg));
    let mut body = layout.text;
    let shown = search_card_fit(hdc, text, body.right - body.left, body.bottom - body.top);
    draw_text(
        hdc,
        &search_card_body(text, shown),
        &mut body,
        SEARCH_CARD_TEXT_FORMAT,
    );
    let left_out = text.chars().count() - search_card_shown(text, shown).chars().count();
    if left_out > 0 {
        SelectObject(hdc, note_font as _);
        SetTextColor(hdc, rgb3(theme.fg_muted));
        let mut note = layout.note;
        draw_text(
            hdc,
            &search_card_left_out(left_out),
            &mut note,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }

    for (rect, button) in [
        (layout.search, SearchCardButton::Confirm),
        (layout.cancel, SearchCardButton::Cancel),
    ] {
        let pill = UiRect {
            x: rect.left as f64,
            y: rect.top as f64,
            width: (rect.right - rect.left) as f64,
            height: (rect.bottom - rect.top) as f64,
        };
        let style = match button {
            SearchCardButton::Confirm => {
                PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
            }
            SearchCardButton::Cancel => {
                PillStyle::new(theme.surface_line, theme.surface_line, theme.fg)
            }
        };
        draw_pill(
            hdc,
            pill,
            button.label(intent),
            style,
            scale,
            button_font,
            theme.surface,
        );
    }

    SelectObject(hdc, old_font);
    DeleteObject(title_font as _);
    DeleteObject(text_font as _);
    DeleteObject(button_font as _);
    DeleteObject(note_font as _);
    shown
}

// Popups auxiliares owned pela janela principal -- divisores do comparador,
// botao de saida, splash e aviso do Gmail. Nascem invisiveis: WS_VISIBLE no
// CreateWindowExW mostra-os com SW_SHOW, que os ativa.
pub(in crate::windows_app) const AUX_POPUP_EX_STYLE: u32 = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
pub(in crate::windows_app) const AUX_POPUP_STYLE: u32 = WS_POPUP;

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
pub(in crate::windows_app) fn bar_tooltip_label(
    hit: BarHit,
    state: &BarState,
    provider: &str,
    tab_url: Option<&str>,
    group: Option<(&str, bool)>,
) -> Option<String> {
    let maximized = state.maximized;
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
                "{}\nClique: abrir ao lado · Arraste: mover · Botão direito: fechar e grupos",
                tab_url?
            )
        }
        BarHit::CloseTab { .. } => "Fechar aba".to_string(),
        BarHit::ContextGroup { .. } => {
            let (name, shows) = group?;
            let action = if shows { "mostrar as abas" } else { "recolher" };
            format!(
                "Grupo \"{name}\": clique para {action} · Arraste: mover o grupo · Botão direito: cor e opções"
            )
        }
        BarHit::TabOverflow(_) => {
            format!("Todas as abas do {provider}, também as que não cabem na barra")
        }
        BarHit::SplitExpand => "Expandir ou reduzir a fonte aberta ao lado".to_string(),
        BarHit::SplitClose => "Fechar a fonte aberta ao lado".to_string(),
        BarHit::Private => {
            "Painel privado: abre ao lado sem gravar histórico nem memória".to_string()
        }
        BarHit::Service(service) => format!("{} no painel ao lado", service.label()),
        BarHit::ServiceStrip(button) => button.hint("o serviço"),
        BarHit::GmailToggle => if GMAIL_NOTIFICATIONS.load(Ordering::Acquire) {
            "Avisos do Gmail: ligados · clique para desligar"
        } else {
            "Avisos do Gmail: desligados · clique para ligar"
        }
        .to_string(),
        BarHit::Tool(tool) => tool.tooltip().to_string(),
        BarHit::GeminiLive => LIVE_TOOLTIP.to_string(),
        BarHit::WindowMinimize => caption_tooltip_label(0, maximized).to_string(),
        BarHit::WindowMaximize => caption_tooltip_label(1, maximized).to_string(),
        BarHit::WindowClose => caption_tooltip_label(2, maximized).to_string(),
    })
}

/// Os botoes da janela do proprio app aparecem sempre que a moldura do Windows
/// nao esta la: na Home e no comparador (com a barra).
/// Os tres botoes da janela (minimizar, maximizar, fechar) juntos, em pixels
/// do cliente, tal como a barra os desenha.
fn caption_area(layout: &BarLayout) -> Area {
    let left = layout.window_minimize.x;
    let right = layout.window_close.x + layout.window_close.width;
    Area {
        x: left,
        y: 0.0,
        width: (right - left).max(1.0),
        height: layout.window_close.height.max(1.0),
    }
}

/// Os botoes da janela na Home: sem barra do comparador, mas no mesmo sitio
/// que nela -- a geometria so depende da largura e da escala.
fn home_caption_rect(client_width: f64, scale: f64) -> Area {
    caption_area(&BarLayout::with_rows(
        client_width,
        scale,
        true,
        BarColumns::even(COMPARATOR_COLUMNS),
        [TabRow::empty(); COMPARATOR_COLUMNS],
    ))
}

fn caption_buttons_wanted(surface: Surface, bar_visible: bool) -> bool {
    match surface {
        Surface::Home => true,
        Surface::Comparator => bar_visible,
        _ => false,
    }
}

/// Se os botoes da janela se veem agora. No comparador fazem parte da barra e
/// estao sempre la; na Home so com o rato perto (pedido do dono: "nao quero
/// os botoes visiveis so quando mover o mouse na direcao deles"). Com o
/// painel de servicos a cobrir a janela (tela cheia do YouTube, "Tela cheia"
/// da faixa) nao se veem em lado nenhum: ficavam no canto do video e o x
/// apanhava o clique de quem queria o video -- e fechava a NeuralIA.
fn caption_buttons_visible(surface: Surface, revealed: bool, panel_covers_window: bool) -> bool {
    if panel_covers_window {
        return false;
    }
    match surface {
        Surface::Home => revealed,
        _ => true,
    }
}

/// Poe o controlo dos botoes da janela no sitio. A vista vai para o topo dos
/// irmaos (por cima das WebViews, que nascem depois dele); escondido nao muda
/// de lugar na ordem Z -- sem o SWP_NOZORDER, cada Resized ou regresso do
/// foco punha-o por cima do painel em tela cheia.
fn place_caption_buttons(buttons: HWND, x: i32, width: i32, height: i32, visible: bool) {
    let z = if visible { 0 } else { SWP_NOZORDER };
    unsafe {
        SetWindowPos(
            buttons,
            std::ptr::null_mut(),
            x,
            0,
            width,
            height,
            SWP_NOACTIVATE | z,
        );
        // SW_SHOWNOACTIVATE: mostrar os botoes nunca rouba o foco a quem
        // esta a escrever.
        ShowWindow(buttons, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        InvalidateRect(buttons, std::ptr::null(), 1);
    }
}

/// A faixa de cima da Home, onde se agarra a janela sem moldura.
fn home_drag_strip(y: f64, scale: f64) -> bool {
    y >= 0.0 && y <= TITLE_TAB_HEIGHT * scale.max(1.0)
}

/// As ferramentas na faixa de cima da Home, encostadas aos botoes da janela
/// (os tres de 46 px que `sync_caption_buttons` poe no canto): a mesma
/// ordem e o mesmo desenho da barra do comparador, so um pouco mais baixos
/// para caberem na faixa de 32 px.
fn home_tool_buttons(
    client_width: f64,
    scale: f64,
    pomodoro_label: Option<BarLabel>,
) -> [UiRect; 3] {
    let scale = scale.max(1.0);
    let caption_left = client_width - 3.0 * 46.0 * scale;
    let size = (TITLE_TAB_HEIGHT - 6.0) * scale;
    tool_button_row(
        caption_left - 8.0 * scale,
        3.0 * scale,
        size,
        4.0 * scale,
        pomodoro_label.map_or(0.0, |label| label.width()) * scale,
    )
}

fn home_tool_hit(
    client_width: f64,
    scale: f64,
    pomodoro_label: Option<BarLabel>,
    x: f64,
    y: f64,
) -> Option<Tool> {
    home_tool_buttons(client_width, scale, pomodoro_label)
        .iter()
        .zip(Tool::ALL)
        .find_map(|(rect, tool)| rect.contains(x, y).then_some(tool))
}

/// O que um clique na Home apanha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HomeClick {
    Tool(Tool),
    /// O botao "Ir" da omnibox.
    Go,
    /// A faixa de cima fora dos botoes: arrasta a janela.
    Drag,
    Nothing,
}

/// A unica decisao do clique na Home. As ferramentas ficam DENTRO da faixa
/// de arrastar, por isso sao vistas primeiro: sem isso o clique nelas
/// arrastava a janela em vez de carregar no botao.
fn home_click_target(
    size: (f64, f64),
    scale: f64,
    pomodoro_label: Option<BarLabel>,
    x: f64,
    y: f64,
) -> HomeClick {
    if let Some(tool) = home_tool_hit(size.0, scale, pomodoro_label, x, y) {
        return HomeClick::Tool(tool);
    }
    if HomeLayout::new(size.0, size.1, scale).go.contains(x, y) {
        return HomeClick::Go;
    }
    if home_drag_strip(y, scale) {
        return HomeClick::Drag;
    }
    HomeClick::Nothing
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

/// O que a dica centrada diz sobre um controlo injetado numa coluna. Vazio
/// quando o rato saiu dele: a dica some.
fn column_hint_text(hint: ColumnHint, provider: &str) -> String {
    match hint {
        ColumnHint::Minimize => format!("Minimizar {provider}"),
        ColumnHint::Expand => format!("Expandir {provider}"),
        ColumnHint::None => String::new(),
    }
}

/// A dica centrada que um controlo injetado pediu: da coluna `col`, e o
/// numero do pedido de dica (`hover_tooltip`) que a agendou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColumnHintOwner {
    col: usize,
    request: u64,
}

/// O que fazer com uma dica pedida por uma coluna.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnHintStep {
    /// O rato entrou num controlo: mostrar a dica dele.
    Show,
    /// O rato saiu e a dica a vista ainda e a dele: apaga-la.
    Clear,
    /// O "none" chegou atrasado: a dica a vista ja e de outro (a barra, a
    /// pega do painel, outra coluna). Fica.
    Keep,
}

/// O "none" (o rato saiu do controlo) atravessa pagina, browser e anfitriao
/// e chega muitas vezes DEPOIS do WM_MOUSEMOVE da janela, quando o rato vai
/// do "−"/"⛶" direto para a barra ou para a pega. Apagar sempre apagava a
/// dica da barra, que so e pedida quando o alvo muda e ja nao voltava. So se
/// apaga a dica que ainda e desta coluna e que ninguem substituiu desde
/// entao (`latest_request` e o numero do ultimo pedido de dica).
fn column_hint_step(
    owner: Option<ColumnHintOwner>,
    col: usize,
    hint: ColumnHint,
    latest_request: u64,
) -> ColumnHintStep {
    match hint {
        ColumnHint::Minimize | ColumnHint::Expand => ColumnHintStep::Show,
        ColumnHint::None => {
            if owner
                == Some(ColumnHintOwner {
                    col,
                    request: latest_request,
                })
            {
                ColumnHintStep::Clear
            } else {
                ColumnHintStep::Keep
            }
        }
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

/// Quantas vezes `hover_tooltip` foi chamada: cada chamada substitui a dica
/// anterior, seja de quem for.
static TOOLTIP_REQUESTS: AtomicU64 = AtomicU64::new(0);

fn latest_tooltip_request() -> u64 {
    TOOLTIP_REQUESTS.load(Ordering::Acquire)
}

/// O rato entrou num alvo com dica `text`, ou saiu de todos (""). Esconde a
/// dica que estiver a vista e, se houver texto, agenda a nova. Devolve o
/// numero deste pedido.
pub(in crate::windows_app) fn hover_tooltip(window: HWND, text: &str) -> u64 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, KillTimer, SetTimer};
    let request = TOOLTIP_REQUESTS.fetch_add(1, Ordering::AcqRel) + 1;
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
        return request;
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
    request
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

/// A dica a vista -- ou a espera do atraso -- passa a dizer `text`, sem se
/// esconder nem voltar a esperar: o tempo do Pomodoro muda a cada segundo
/// com o rato parado em cima do botao. Escondida, fica escondida.
pub(in crate::windows_app) fn refresh_hint_text(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GW_OWNER, GetWindow, IsWindowVisible};
    {
        let mut pending = TOOLTIP_PENDING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, waiting)) = pending.as_mut() {
            *waiting = text.to_string();
            return;
        }
    }
    let hint = HINT_HWND.load(Ordering::Acquire) as HWND;
    if hint.is_null() || unsafe { IsWindowVisible(hint) } == 0 {
        return;
    }
    let unchanged = *HINT_TEXT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        == text;
    if unchanged {
        return;
    }
    let root = unsafe { GetWindow(hint, GW_OWNER) };
    *TOOLTIP_PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((root as usize, text.to_string()));
    // Volta a centrar e a dimensionar pelo texto novo; `show_popup_without_activation`
    // la dentro, como sempre: a dica nunca rouba o foco.
    show_pending_tooltip();
}

/// Pixeis BGRA de um disco da cor `color` com a borda suave, com o alfa ja
/// multiplicado nos canais -- o que o menu espera de um bitmap de 32 bits.
fn swatch_pixels(color: Rgb, size: i32) -> Vec<u8> {
    let size = size.max(1);
    let center = size as f32 / 2.0;
    let radius = center - 1.0;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let alpha = (coverage * 255.0).round() as u32;
            let channel = |value: u8| ((value as u32 * alpha + 127) / 255) as u8;
            pixels.extend_from_slice(&[
                channel(color.2),
                channel(color.1),
                channel(color.0),
                alpha as u8,
            ]);
        }
    }
    pixels
}

/// A amostra redonda de uma cor para um item de menu. Quem a pede apaga-a
/// com `DeleteObject` depois do `DestroyMenu`: o menu nao e dono dos
/// bitmaps dos seus itens. Nulo se o GDI recusar -- o item fica so com texto.
unsafe fn color_swatch_bitmap(color: Rgb, size: i32) -> *mut core::ffi::c_void {
    let size = size.max(8);
    let screen = GetDC(std::ptr::null_mut());
    if screen.is_null() {
        return std::ptr::null_mut();
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
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
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(
        screen,
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    ReleaseDC(std::ptr::null_mut(), screen);
    if bitmap.is_null() || bits.is_null() {
        if !bitmap.is_null() {
            DeleteObject(bitmap as _);
        }
        return std::ptr::null_mut();
    }
    let pixels = swatch_pixels(color, size);
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
    bitmap as _
}

/// Acrescenta ao menu um item com texto e, a esquerda, a amostra `swatch`
/// (pode ser nula). `checked` marca-o como a escolha em vigor; `disabled`
/// deixa-o cinzento e sem clique, como o `MF_GRAYED` de um item sem icone.
unsafe fn append_swatch_item(
    menu: *mut core::ffi::c_void,
    id: usize,
    label: &[u16],
    swatch: *mut core::ffi::c_void,
    checked: bool,
    disabled: bool,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, InsertMenuItemW, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED,
        MFT_RADIOCHECK, MFT_STRING, MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING,
    };
    let mut state = 0;
    if checked {
        state |= MFS_CHECKED;
    }
    if disabled {
        // MFS_DISABLED e MFS_GRAYED sao o mesmo valor: cinzento e sem clique.
        state |= MFS_DISABLED;
    }
    let info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID
            | MIIM_STRING
            | MIIM_FTYPE
            | MIIM_STATE
            | if swatch.is_null() { 0 } else { MIIM_BITMAP },
        fType: MFT_STRING | if checked { MFT_RADIOCHECK } else { 0 },
        fState: state,
        wID: id as u32,
        hSubMenu: std::ptr::null_mut(),
        hbmpChecked: std::ptr::null_mut(),
        hbmpUnchecked: std::ptr::null_mut(),
        dwItemData: 0,
        dwTypeData: label.as_ptr() as *mut u16,
        cch: label.len().saturating_sub(1) as u32,
        hbmpItem: swatch as _,
    };
    InsertMenuItemW(menu, GetMenuItemCount(menu).max(0) as u32, 1, &info);
}

/// Um submenu com a amostra de cor a esquerda do texto -- o grupo na lista de
/// abas. O submenu passa a ser do menu e morre com ele.
unsafe fn append_swatch_submenu(
    menu: *mut core::ffi::c_void,
    submenu: *mut core::ffi::c_void,
    label: &[u16],
    swatch: *mut core::ffi::c_void,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, InsertMenuItemW, MENUITEMINFOW, MFT_STRING, MIIM_BITMAP, MIIM_FTYPE,
        MIIM_STRING, MIIM_SUBMENU,
    };
    let info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_SUBMENU
            | MIIM_STRING
            | MIIM_FTYPE
            | if swatch.is_null() { 0 } else { MIIM_BITMAP },
        fType: MFT_STRING,
        fState: 0,
        wID: 0,
        hSubMenu: submenu as _,
        hbmpChecked: std::ptr::null_mut(),
        hbmpUnchecked: std::ptr::null_mut(),
        dwItemData: 0,
        dwTypeData: label.as_ptr() as *mut u16,
        cch: label.len().saturating_sub(1) as u32,
        hbmpItem: swatch as _,
    };
    InsertMenuItemW(menu, GetMenuItemCount(menu).max(0) as u32, 1, &info);
}

/// O botao principal do rato esta em baixo, tal como a fila de mensagens o
/// ve (botoes trocados nas Definicoes incluidos: e o estado logico).
fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_LBUTTON};
    unsafe { GetKeyState(VK_LBUTTON as i32) < 0 }
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
    reference_data: usize,
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
            // Saiu dos botoes -- talvez para fora da janela, onde a janela
            // principal ja nao recebe movimento: o prazo de os esconder tem de
            // comecar daqui.
            if reference_data != 0 {
                let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                let _ = proxy.send_event(UserEvent::CaptionReveal);
            }
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
                let _ = proxy.send_event(UserEvent::Theme(ThemeEvent::Chosen(choice)));
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

/// Botao de sair do ecra completo. Tem de ser uma janela de topo propria: o
/// WebView2 e uma janela filha que cobre o cliente todo, por isso nada pintado
/// pela janela principal apareceria por cima dele. Tambem nao pode depender de
/// nada injetado na pagina -- o YouTube reescreve o seu proprio DOM e o botao
/// injetado desaparece, que foi exatamente o que aconteceu.
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
pub(in crate::windows_app) struct Timers {
    shared: Arc<(Mutex<TimerQueue<UserEvent>>, Condvar)>,
    /// So para o fallback: a thread de servico tem a sua propria copia.
    proxy: EventLoopProxy<UserEvent>,
    alive: bool,
}

/// Os tiques do Pomodoro voltam ao event loop como `PomodoroTick(token)`.
impl TickScheduler for Timers {
    fn schedule(&self, tick: TickSchedule) {
        self.after(tick.delay, UserEvent::PomodoroTick(tick.token));
    }
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::windows_app) struct UiRect {
    pub(in crate::windows_app) x: f64,
    pub(in crate::windows_app) y: f64,
    pub(in crate::windows_app) width: f64,
    pub(in crate::windows_app) height: f64,
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

pub(in crate::windows_app) struct App {
    pub(in crate::windows_app) proxy: EventLoopProxy<UserEvent>,
    pub(in crate::windows_app) window: Option<Window>,
    pub(in crate::windows_app) webview: Option<WebView>,
    pub(in crate::windows_app) comparator: Option<ComparatorState>,
    pub(in crate::windows_app) omnibox: Option<HWND>,
    pub(in crate::windows_app) bar_hover: Option<BarHit>,
    /// Botao esquerdo em baixo sobre uma aba, o x dela ou a pilula de um
    /// grupo: o que acontece so se decide ao largar (ou ao arrastar).
    pub(in crate::windows_app) tab_press: Option<TabPress>,
    /// Numero do ultimo gesto na fila de abas; o proximo e este mais um.
    pub(in crate::windows_app) tab_gesture_count: u64,
    /// Ctrl/Shift/Alt no teclado da janela principal (a barra com o foco).
    pub(in crate::windows_app) modifiers: winit::keyboard::ModifiersState,
    /// O rato esta em cima do "Ir" da Home: pinta-se em degradê.
    pub(in crate::windows_app) home_go_hover: bool,
    pub(in crate::windows_app) exit_button: Option<HWND>,
    pub(in crate::windows_app) home_button: Option<HWND>,
    pub(in crate::windows_app) caption_buttons: Option<HWND>,
    /// Na Home os botoes da janela so se veem com o rato perto deles.
    pub(in crate::windows_app) caption_reveal: CaptionReveal,
    pub(in crate::windows_app) splitters: [Option<HWND>; COMPARATOR_COLUMNS - 1],
    /// Partilhado com o menu do botao direito de cada coluna: o WebView2 monta
    /// esse menu num callback fora do `&mut App`, e o rotulo tem de dizer o
    /// estado de AGORA, mudado pelo Ctrl+R, pela pergunta ou pelo proprio menu.
    pub(in crate::windows_app) auto_scroll: SharedFlag,
    pub(in crate::windows_app) auto_scroll_answered: bool,
    pub(in crate::windows_app) auto_scroll_token: u64,
    pub(in crate::windows_app) zoom: f64,
    /// O visualizador de PDF nao aceita script do host: rola-se por tecla.
    pub(in crate::windows_app) reading_pdf: bool,
    pub(in crate::windows_app) splash: Option<HWND>,
    pub(in crate::windows_app) splash_board: SplashBoard,
    /// A janela do aviso do canto (`toast.rs`), enquanto existe.
    pub(in crate::windows_app) toast: Option<HWND>,
    /// O centro de avisos: o aviso a vista, o token dele e a fila.
    pub(in crate::windows_app) notify: crate::notify::NotifyCentre,
    /// O pedido da barra (Mandar para IA, Traduzir) a espera do clique no
    /// cartao nativo.
    pub(in crate::windows_app) search_card: SearchCard,
    /// Os "Salvar nota" gravados ha menos de 2 s: o mesmo texto nao e outra
    /// nota.
    pub(in crate::windows_app) bar_notes: BarNoteGuard,
    /// O mesmo para o Ctrl+Shift+Z (a tecla presa repete o keydown).
    pub(in crate::windows_app) shortcut_notes: BarNoteGuard,
    pub(in crate::windows_app) search_card_popup: Option<HWND>,
    pub(in crate::windows_app) search_card_sink: Box<SearchCardSink>,
    pub(in crate::windows_app) gmail_monitor: Option<WebView>,
    pub(in crate::windows_app) gmail_probe_token: u64,
    pub(in crate::windows_app) gmail_last_unread: Option<u32>,
    pub(in crate::windows_app) gmail_last_key: Option<String>,
    /// O Win32 nao apaga o fundo por nos e uma janela filha destruida deixa os
    /// ultimos pixeis onde estava. Sem isto viam-se barras e texto fantasma.
    pub(in crate::windows_app) needs_clear: bool,
    pub(in crate::windows_app) omnibox_font: Option<*mut core::ffi::c_void>,
    pub(in crate::windows_app) omnibox_font_height: i32,
    pub(in crate::windows_app) omnibox_proxy: Box<EventLoopProxy<UserEvent>>,
    /// Popup nativo da palette (Ctrl+K/T ou +), so enquanto esta aberta.
    pub(in crate::windows_app) palette: Option<PaletteWindow>,
    /// Estado nativo lido pela subclasse do EDIT da palette.
    pub(in crate::windows_app) palette_host: Box<PaletteHost>,
    pub(in crate::windows_app) config: CoreConfig,
    pub(in crate::windows_app) history: HistoryWriter,
    pub(in crate::windows_app) memory: MemoryWorker,
    /// Todos os prazos da interface (avisos, sondas, rolagem, barra) passam
    /// por aqui: uma thread para a aplicacao inteira.
    pub(in crate::windows_app) timers: Timers,
    pub(in crate::windows_app) current_research: Option<ResearchSession>,
    pub(in crate::windows_app) active_agent: Option<BrowserAgentState>,
    pub(in crate::windows_app) reader: ReaderWorker,
    /// Worker unico para documentos binarios. Um pedido novo substitui o
    /// pendente, evitando uma thread/socket de 90 s por clique em PDF.
    pub(in crate::windows_app) document: DocumentWorker,
    /// Os bytes do PDF aberto, servidos ao visualizador pela origem propria.
    pub(in crate::windows_app) pdf_bytes: Arc<Mutex<Vec<u8>>>,
    pub(in crate::windows_app) surface: Surface,
    pub(in crate::windows_app) navigation_generation: Arc<AtomicU64>,
    pub(in crate::windows_app) status: Option<String>,
    pub(in crate::windows_app) cursor: (f64, f64),
    /// Proximo frame da rede neural nativa da Home. Nao existe WebView nem
    /// rede por tras do efeito: e apenas GDI, limitado a ~15 FPS.
    pub(in crate::windows_app) next_home_frame: Instant,
    /// Janela inteiramente tapada por outra (ou minimizada), segundo o
    /// `WindowEvent::Occluded`. Animar nesse estado e gastar bateria a pintar
    /// pixeis que ninguem chega a ver.
    pub(in crate::windows_app) home_occluded: bool,
    /// Janela com o foco do teclado (`WindowEvent::Focused`). Em segundo plano
    /// a animacao continua, mas devagar.
    pub(in crate::windows_app) home_focused: bool,
    /// Painel lateral do historico inteligente e das notas (Ctrl+H). Sai por
    /// `close_side_panel`, que grava primeiro o que o editor tinha por
    /// salvar e devolve o teclado (`side_panel::SidePanel::dismiss`);
    /// largado de outra forma, o `Drop` dele ainda grava o rascunho.
    pub(in crate::windows_app) side_panel: side_panel::SidePanel<WebView, ZettelWorker>,
    /// A consulta de memoria que alimenta as sugestoes do painel.
    pub(in crate::windows_app) panel_suggestion_query: Option<String>,
    /// Servico aberto no painel lateral (WhatsApp, Meet, YouTube, Gmail e o
    /// video da respiracao, este em InPrivate).
    pub(in crate::windows_app) service_panel: Option<ServicePanel>,
    /// Numero do ultimo painel de servicos aberto: os avisos do WebView2 de
    /// um painel ja fechado chegam com o numero dele e caem.
    pub(in crate::windows_app) service_generation: u64,
    /// A tela cheia da janela pedida pelo painel de servicos, e se foi ele
    /// que a pos (so entao a devolve ao sair).
    pub(in crate::windows_app) panel_window_fullscreen: PanelWindowFullscreen,
    /// De que coluna e a dica centrada pedida por um controlo injetado (o
    /// "none" atrasado de uma coluna so apaga a dica dela).
    pub(in crate::windows_app) column_hint: Option<ColumnHintOwner>,
    /// Larguras escolhidas para os paineis da direita, gravadas em
    /// `<data_dir>/panel-width.json`.
    pub(in crate::windows_app) panel_widths: PanelWidths,
    /// A pega de arrastar a borda esquerda do painel aberto.
    pub(in crate::windows_app) panel_handle: Option<HWND>,
    /// Gravacao das abas e grupos do comparador em `tabs.json`. Aberta no
    /// arranque: a primeira janela do NeuralIA fica com o `tabs.lock`.
    pub(in crate::windows_app) tab_session: TabPersistence,
    /// Ferramentas: o botao da Home sob o rato (a barra usa `bar_hover`).
    pub(in crate::windows_app) home_tool_hover: Option<Tool>,
    /// Notas (Zettelkasten) em `<data_dir>/zettel`, lidas e gravadas fora do
    /// event loop.
    pub(in crate::windows_app) notes: ZettelWorker,
    /// O endereco verdadeiro da pagina da WebView unica quando o dela nao o
    /// e: o artigo do Leitor (o HTML e local) e o PDF (o visualizador e
    /// nosso). E a fonte das notas feitas ali.
    pub(in crate::windows_app) page_source: Option<String>,
    /// O Pomodoro do botao da barra e da Home, com a cadeia de tiques viva.
    /// As duracoes vivem em `<data_dir>/pomodoro`.
    pub(in crate::windows_app) pomodoro: PomodoroController,
    /// Biblioteca de livros (worker) e servidor da origem `neuralia-epub`.
    /// Nascem na primeira vez que se abre um livro e vivem com a app.
    pub(in crate::windows_app) epub: Option<EpubRuntime>,
    /// Arquivos largados na janela neste lote de eventos. O winit entrega um
    /// `DroppedFile` por arquivo; o lote segue inteiro no `about_to_wait`.
    pub(in crate::windows_app) pending_drops: Vec<PathBuf>,
    /// Painel do Gemini Live, com o estado do olho da barra. Existir e estar
    /// ligado: fecha-lo desliga tudo.
    pub(in crate::windows_app) live_panel: LivePanel<WebView>,
    /// O registo das lojas (`neural_core::json_store`), cunhado aqui -- a
    /// unica cunhagem do produto. So ele passa os grants que abrem as lojas;
    /// o infra-privacy-guard muda-o para o `PrivacyGuard`. `None` so se o
    /// processo ja o tivesse cunhado, o que nao acontece: ha um `App` por
    /// processo.
    pub(in crate::windows_app) stores: Option<StoreRegistry>,
    /// O pedido de chave nativo e o cofre das chaves (`secret_prompt.rs`).
    pub(in crate::windows_app) keys: KeysState,
    /// O bloqueio de anuncios (`adblock.rs`): a escolha, a lista e o que os
    /// handlers do WebView2 leem.
    pub(in crate::windows_app) adblock: AdblockState,
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
        let pomodoro = PomodoroController::new(crate::pomodoro_ui::load_settings(
            &config.data_dir.join("pomodoro"),
        ));
        let history = HistoryWriter::new(history_store, proxy.clone());
        let memory = MemoryWorker::new(config.data_dir.join("memory"), proxy.clone());
        let notes = ZettelWorker::new(config.data_dir.join("zettel"), proxy.clone());
        let timers = Timers::new(proxy.clone());
        let reader_client = ReaderClient::new(config.reader_timeout_secs, config.reader_max_bytes);
        let navigation_generation = Arc::new(AtomicU64::new(0));
        let reader = ReaderWorker::new(
            reader_client,
            proxy.clone(),
            Arc::clone(&navigation_generation),
        );
        let omnibox_proxy = Box::new(proxy.clone());
        let panel_widths = PanelWidths::load(&config.data_dir.join(PANEL_WIDTHS_FILE));
        let search_card_sink: Box<SearchCardSink> = {
            let proxy = proxy.clone();
            Box::new(Box::new(move |event| {
                let _ = proxy.send_event(event);
            }))
        };
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
        let tab_session = TabPersistence::open(&config.data_dir);
        // A unica cunhagem do registo das lojas no produto. Nao toca no
        // disco: so os grants, pedidos depois, dizem onde cada loja vive.
        let stores = StoreRegistry::mint(&config.data_dir).ok();
        let keys = KeysState::new(proxy.clone());
        // Desligado (quem nunca clicou em "Ativar"), so le a escolha.
        let adblock = AdblockState::open(stores.as_ref(), &proxy);
        Self {
            document,
            pdf_bytes: Arc::new(Mutex::new(Vec::new())),
            proxy,
            window: None,
            webview: None,
            comparator: None,
            omnibox: None,
            bar_hover: None,
            tab_press: None,
            tab_gesture_count: 0,
            modifiers: winit::keyboard::ModifiersState::empty(),
            home_go_hover: false,
            exit_button: None,
            home_button: None,
            caption_buttons: None,
            caption_reveal: CaptionReveal::default(),
            splitters: [None; COMPARATOR_COLUMNS - 1],
            // Ligada por omissao: a aplicacao serve para ler.
            // Nada rola sem o utilizador dizer que sim.
            auto_scroll: SharedFlag::default(),
            auto_scroll_answered: false,
            auto_scroll_token: 0,
            zoom: 1.0,
            reading_pdf: false,
            splash: None,
            splash_board: SplashBoard::default(),
            toast: None,
            notify: crate::notify::NotifyCentre::default(),
            search_card: SearchCard::default(),
            bar_notes: BarNoteGuard::default(),
            shortcut_notes: BarNoteGuard::default(),
            search_card_popup: None,
            search_card_sink,
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
            side_panel: side_panel::SidePanel::closed(notes.clone()),
            panel_suggestion_query: None,
            service_panel: None,
            service_generation: 0,
            panel_window_fullscreen: PanelWindowFullscreen::default(),
            column_hint: None,
            panel_widths,
            panel_handle: None,
            tab_session,
            home_tool_hover: None,
            notes,
            page_source: None,
            pomodoro,
            epub: None,
            pending_drops: Vec::new(),
            live_panel: LivePanel::off(),
            stores,
            keys,
            adblock,
        }
    }
}

/// Ligacao do agente (AGENTS.md §7). Fica na raiz ate o dono aprovar
/// `app/agent.rs` (OQ2); os metodos seguem contiguos para os anchors.
impl App {
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

        let Some(window) = &self.window else {
            self.active_agent = None;
            return;
        };
        let builder = self
            .external_webview_builder(None, true)
            .with_url(valid.as_str());
        let hooked = self.hooked_builder(builder, WebViewHost::External, None);
        let result = hooked.build_hooked(window);

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

// ---------------------------------------------------------------------------
// Abas e grupos entre sessoes (`<data_dir>/tabs.json`). O formato, a leitura
// tolerante e a escrita atomica vivem em `tab_session.rs`, portatil; aqui fica
// so a ponte com o modelo da barra.

const _: () = assert!(COMPARATOR_COLUMNS == tab_session::COLUMNS);

/// Quanto se espera, depois da ultima mudanca nas abas, para gravar.
const TAB_SESSION_DEBOUNCE: Duration = Duration::from_millis(1500);

impl GroupColor {
    /// Chave estavel no ficheiro. O nome da variante nao serve: renomear uma
    /// cor no codigo nao pode apagar a cor dos grupos ja guardados.
    fn key(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Amber => "amber",
            Self::Pink => "pink",
            Self::Purple => "purple",
            Self::Slate => "slate",
            Self::Red => "red",
            Self::Cyan => "cyan",
            Self::Orange => "orange",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.key() == key)
    }
}

/// O que a gravacao das abas ja viu e o que ja esta no disco.
#[derive(Debug, Default)]
struct TabSessionSync {
    /// Sobe a cada mudanca: so o ultimo `SaveTabSession` agendado grava.
    token: u64,
    /// Impressao digital do modelo da ultima vez que se olhou para ele.
    seen: Option<u64>,
    /// Impressao digital do que esta no disco.
    saved: Option<u64>,
}

impl TabSessionSync {
    /// O modelo depois de um lote de eventos. Se mudou desde a ultima vez,
    /// devolve o bilhete da gravacao a agendar (o anterior deixa de valer);
    /// se nao mudou, nada. Um arrasto so muda o modelo ao largar -- a meio,
    /// a barra desenha uma copia --, e por isso so o largar agenda.
    fn observe(&mut self, fingerprint: u64) -> Option<u64> {
        if self.seen == Some(fingerprint) {
            return None;
        }
        self.seen = Some(fingerprint);
        self.token = self.token.wrapping_add(1);
        Some(self.token)
    }
}

/// O que uma gravacao das abas fez, para o App dizer ao dono.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabSave {
    /// O disco ja tinha isto: nada escrito.
    Unchanged,
    Written,
    /// Outra janela do NeuralIA guarda as abas; esta nao escreve.
    NotWriter,
    /// O historico foi apagado noutra janela: as abas desta, de antes disso,
    /// foram largadas em vez de voltarem ao disco.
    ClearedElsewhere,
}

/// A ponte entre o modelo da barra e o `tabs.json` de um `data_dir`: tudo o
/// que o App faz com as abas guardadas -- ler ao abrir o comparador, gravar
/// no fim do atraso (`SaveTabSession`), ao destruir o comparador e ao sair, e
/// apagar em "Apagar historico". Nada aqui toca em janelas: os gates
/// conduzem isto contra um diretorio temporario, pelo mesmo caminho que o App
/// usa. O App so passa o seu modelo e mostra o aviso.
struct TabPersistence {
    store: tab_session::SessionStore,
    sync: TabSessionSync,
}

impl TabPersistence {
    fn open(data_dir: &std::path::Path) -> Self {
        Self {
            store: tab_session::SessionStore::open(data_dir),
            sync: TabSessionSync::default(),
        }
    }

    /// O disco e o modelo concordam: nada a gravar ate o modelo mudar.
    fn synced(
        &mut self,
        contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) {
        let fingerprint = tab_session_fingerprint(contexts, groups, split);
        self.sync.seen = Some(fingerprint);
        self.sync.saved = Some(fingerprint);
    }

    /// As abas da sessao anterior, ja como uma pesquisa nova as ve
    /// (`start_new_search`), e o aviso para o dono, se houver. Um ficheiro
    /// estragado nao impede o comparador de abrir: fica em `tabs.json.bak` e
    /// a sessao comeca limpa; um que nao se leu fica copiado antes da
    /// primeira gravacao (`SessionStore`).
    fn restore(&mut self) -> (RestoredTabs, Option<String>) {
        let empty = TabSession::default();
        let (mut restored, notice) = match self.store.load() {
            Loaded::Restored(session) => (restore_tab_session(&session), None),
            Loaded::Missing => (restore_tab_session(&empty), None),
            Loaded::Quarantined(error) => {
                debug_log(format_args!("tabs.json recusado: {error:?}"));
                (
                    restore_tab_session(&empty),
                    Some(format!(
                        "Abas anteriores não restauradas: {error}. Cópia em tabs.json.bak."
                    )),
                )
            }
            Loaded::Unreadable(error) => {
                debug_log(format_args!("tabs.json ilegivel: {error}"));
                (
                    restore_tab_session(&empty),
                    Some(
                        "Não foi possível ler as abas da sessão anterior. Antes de salvar \
                         por cima, o arquivo é copiado para tabs.json.unread."
                            .to_string(),
                    ),
                )
            }
        };
        start_new_search(
            &mut restored.contexts,
            &mut restored.groups,
            &mut restored.next_context_id,
            &mut restored.next_group_id,
            &mut restored.active,
        );
        self.synced(&restored.contexts, &restored.groups, None);
        let notice = if self.store.is_writer() {
            notice
        } else {
            Some(
                "Outra janela do NeuralIA já salva as abas: as abas desta janela não serão salvas."
                    .to_string(),
            )
        };
        (restored, notice)
    }

    /// O modelo depois de um lote de eventos: o bilhete da gravacao a
    /// agendar, se mudou.
    fn observe(
        &mut self,
        contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> Option<u64> {
        self.sync
            .observe(tab_session_fingerprint(contexts, groups, split))
    }

    /// Grava ja, se o disco estiver atrasado em relacao a barra: ao destruir
    /// o comparador e ao sair -- uma mudanca feita dentro do atraso de
    /// `TAB_SESSION_DEBOUNCE` nao se perde. Se o historico foi apagado noutra
    /// janela, as abas de antes sao largadas do modelo (como o "Apagar
    /// historico" faz na propria janela) em vez de voltarem ao disco.
    fn save_now(
        &mut self,
        contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> std::io::Result<TabSave> {
        let fingerprint = tab_session_fingerprint(contexts, groups, split);
        if self.sync.saved == Some(fingerprint) {
            return Ok(TabSave::Unchanged);
        }
        let session = snapshot_tab_session(contexts, groups, split);
        match self.store.save(&session) {
            Ok(tab_session::SaveOutcome::Written) => {
                self.sync.saved = Some(fingerprint);
                Ok(TabSave::Written)
            }
            Ok(tab_session::SaveOutcome::NotWriter) => {
                // Nada a tentar outra vez ate o modelo mudar.
                self.sync.saved = Some(fingerprint);
                Ok(TabSave::NotWriter)
            }
            Ok(tab_session::SaveOutcome::ClearedElsewhere) => {
                contexts.iter_mut().for_each(Vec::clear);
                groups.iter_mut().for_each(Vec::clear);
                self.store.acknowledge_clear();
                self.synced(contexts, groups, split);
                Ok(TabSave::ClearedElsewhere)
            }
            Err(error) => {
                self.sync.saved = None;
                debug_log(format_args!("tabs.json nao gravado: {error}"));
                Err(error)
            }
        }
    }

    /// O `SaveTabSession(token)` que o atraso entrega: so o ultimo agendado
    /// grava (`None` para um bilhete ultrapassado por outra mudanca).
    fn save_due(
        &mut self,
        token: u64,
        contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> Option<std::io::Result<TabSave>> {
        (token == self.sync.token).then(|| self.save_now(contexts, groups, split))
    }

    /// Parte de "Apagar historico": o modelo vivo esvazia antes -- senao a
    /// gravacao seguinte, ou o fecho do comparador, escrevia de volta o que se
    /// acabou de apagar -- e saem o ficheiro, a copia de um ficheiro recusado
    /// ou nao lido e os temporarios; a geracao sobe para outra janela aberta
    /// nao os escrever de volta.
    fn forget(
        &mut self,
        contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> std::io::Result<()> {
        contexts.iter_mut().for_each(Vec::clear);
        groups.iter_mut().for_each(Vec::clear);
        let result = self.store.forget();
        self.synced(contexts, groups, split);
        result
    }
}

/// O aviso de uma gravacao, para o splash (`None`: nada a dizer).
fn tab_save_notice(result: &std::io::Result<TabSave>) -> Option<String> {
    match result {
        Ok(TabSave::ClearedElsewhere) => Some(
            "O histórico foi apagado em outra janela do NeuralIA: as abas desta janela também."
                .to_string(),
        ),
        Ok(_) => None,
        Err(error) => Some(format!("As abas não foram salvas: {error}")),
    }
}

/// A aba aberta ao lado, tal como a gravacao a ve: (coluna, aba). `split` e
/// `(coluna, aba, privado)`. Uma fonte privada nunca conta -- e nem tem aba na
/// lista, ver `record_split_context`.
fn persisted_active(split: Option<(usize, Option<u64>, bool)>) -> Option<(usize, u64)> {
    let (column, context_id, private) = split?;
    if private {
        return None;
    }
    context_id.map(|id| (column, id))
}

/// O modelo da barra como vai para o disco. So abas da lista -- onde uma
/// fonte privada nunca entra -- e a aba aberta ao lado so quando nao e
/// privada.
fn snapshot_tab_session(
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    split: Option<(usize, Option<u64>, bool)>,
) -> TabSession {
    let active = persisted_active(split);
    TabSession {
        columns: std::array::from_fn(|index| SessionColumn {
            tabs: contexts[index]
                .iter()
                .map(|tab| SessionTab {
                    id: tab.id,
                    url: tab.url.clone(),
                    title: context_tab_label(&tab.url),
                    group: tab.group,
                })
                .collect(),
            groups: groups[index]
                .iter()
                .map(|group| SessionGroup {
                    id: group.id,
                    name: group.name.clone(),
                    color: group.color.key().to_string(),
                    collapsed: group.collapsed,
                })
                .collect(),
            active: active
                .filter(|(column, _)| *column == index)
                .and_then(|(_, id)| contexts[index].iter().position(|tab| tab.id == id)),
        }),
    }
}

/// Impressao digital de tudo o que `snapshot_tab_session` grava. Barata o
/// bastante para correr depois de cada lote de eventos.
fn tab_session_fingerprint(
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    split: Option<(usize, Option<u64>, bool)>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    for (tabs, column_groups) in contexts.iter().zip(groups) {
        tabs.len().hash(&mut hasher);
        for tab in tabs {
            tab.id.hash(&mut hasher);
            tab.url.hash(&mut hasher);
            tab.group.hash(&mut hasher);
        }
        column_groups.len().hash(&mut hasher);
        for group in column_groups {
            group.id.hash(&mut hasher);
            group.name.hash(&mut hasher);
            group.color.key().hash(&mut hasher);
            group.collapsed.hash(&mut hasher);
        }
    }
    persisted_active(split).hash(&mut hasher);
    hasher.finish()
}

/// O modelo da barra reconstruido a partir do ficheiro.
struct RestoredTabs {
    contexts: [Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: [Vec<ContextGroup>; COMPARATOR_COLUMNS],
    next_context_id: u64,
    next_group_id: u64,
    /// A aba que estava aberta ao lado em cada coluna: a unica que teria de
    /// carregar. As outras voltam so como entradas na barra e carregam quando
    /// forem escolhidas.
    active: [Option<u64>; COMPARATOR_COLUMNS],
}

/// As identidades sao renumeradas: o ficheiro nao escolhe ids da sessao viva
/// (um id perto de `u64::MAX` dava a volta ao contador e colidia). Os membros
/// de um grupo voltam juntos, a seguir ao primeiro -- a invariante que
/// `join_context_group` mantem na sessao viva.
fn restore_tab_session(session: &TabSession) -> RestoredTabs {
    let session = tab_session::sanitize(session);
    let mut restored = RestoredTabs {
        contexts: std::array::from_fn(|_| Vec::new()),
        groups: std::array::from_fn(|_| Vec::new()),
        next_context_id: 1,
        next_group_id: 1,
        active: [None; COMPARATOR_COLUMNS],
    };
    for (index, column) in session.columns.iter().enumerate() {
        let mut group_ids: Vec<(u64, u64)> = Vec::new();
        let mut used: Vec<GroupColor> = Vec::new();
        for group in &column.groups {
            let id = restored.next_group_id;
            restored.next_group_id += 1;
            group_ids.push((group.id, id));
            let color =
                GroupColor::from_key(&group.color).unwrap_or_else(|| GroupColor::next(&used));
            used.push(color);
            restored.groups[index].push(ContextGroup {
                id,
                name: group.name.clone(),
                color,
                collapsed: group.collapsed,
            });
        }

        // As identidades novas guardam a ordem de idade das do ficheiro: os
        // limites podam pela identidade (a mais baixa e a mais antiga), e uma
        // aba arrastada para a frente antes de fechar nao pode passar a ser a
        // mais antiga so por ter reiniciado.
        let mut by_age: Vec<usize> = (0..column.tabs.len()).collect();
        by_age.sort_by_key(|position| (column.tabs[*position].id, *position));
        let mut new_ids = vec![0u64; column.tabs.len()];
        for (rank, position) in by_age.into_iter().enumerate() {
            new_ids[position] = restored.next_context_id + rank as u64;
        }
        restored.next_context_id += column.tabs.len() as u64;

        let mut tabs: Vec<ContextTab> = Vec::with_capacity(column.tabs.len());
        for (position, tab) in column.tabs.iter().enumerate() {
            let id = new_ids[position];
            let group = tab.group.and_then(|old| {
                group_ids
                    .iter()
                    .find(|(from, _)| *from == old)
                    .map(|(_, to)| *to)
            });
            if column.active == Some(position) {
                restored.active[index] = Some(id);
            }
            tabs.push(ContextTab {
                id,
                url: tab.url.clone(),
                group,
            });
        }
        // Membros de um grupo seguidos, a partir do primeiro, pela ordem em
        // que aparecem.
        let mut ordered: Vec<ContextTab> = Vec::with_capacity(tabs.len());
        let mut placed = vec![false; tabs.len()];
        for (slot, tab) in tabs.iter().enumerate() {
            if placed[slot] {
                continue;
            }
            placed[slot] = true;
            ordered.push(tab.clone());
            if tab.group.is_none() {
                continue;
            }
            for (later, other) in tabs.iter().enumerate().skip(slot + 1) {
                if !placed[later] && other.group == tab.group {
                    placed[later] = true;
                    ordered.push(other.clone());
                }
            }
        }
        restored.contexts[index] = ordered;
        prune_empty_groups(&restored.contexts[index], &mut restored.groups[index]);
    }
    restored
}

/// Uma pesquisa nova no comparador. Como a omnibox do Chrome: a pergunta
/// substitui a pagina da propria IA -- a aba inicial de cada coluna -- e passa
/// a ser o contexto ativo; as abas de contexto e os grupos ficam como
/// estavam, e a que estava aberta ao lado fica na barra, por carregar, ate ser
/// escolhida. Os contadores nunca recuam: um grupo novo nao pode herdar o id
/// de um grupo que sobreviveu, senao as abas dos dois fundiam-se.
fn start_new_search(
    contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
    next_context_id: &mut u64,
    next_group_id: &mut u64,
    active: &mut [Option<u64>; COMPARATOR_COLUMNS],
) {
    // Os mesmos limites de quando uma aba nasce: saem as soltas mais antigas,
    // nunca a que fica aberta ao lado nem, antes delas, uma agrupada.
    for ((tabs, column_groups), open) in contexts.iter_mut().zip(groups.iter_mut()).zip(*active) {
        let protected: Vec<u64> = open.into_iter().collect();
        let _ = prune_context_tabs(tabs, column_groups, &protected);
        prune_empty_groups(tabs, column_groups);
    }
    let last_tab = contexts.iter().flatten().map(|tab| tab.id).max();
    let last_group = groups.iter().flatten().map(|group| group.id).max();
    *next_context_id = (*next_context_id).max(last_tab.map_or(1, |id| id.saturating_add(1)));
    *next_group_id = (*next_group_id).max(last_group.map_or(1, |id| id.saturating_add(1)));
    *active = [None; COMPARATOR_COLUMNS];
}

/// Gates da persistencia de abas sobre as funcoes que o comparador chama:
/// `record_split_context` (open_split_mode), `start_new_search`
/// (open_comparator), `snapshot_tab_session`/`restore_tab_session` (gravacao
/// e arranque) e `TabPersistence` -- o que o App faz ao abrir o comparador,
/// no `SaveTabSession`, ao destruir o comparador, ao sair e em "Apagar
/// historico", contra um `data_dir` temporario.
#[cfg(test)]
mod tab_session_gates {
    use super::*;
    use std::path::PathBuf;

    type Columns<T> = [Vec<T>; COMPARATOR_COLUMNS];

    fn empty<T>() -> Columns<T> {
        std::array::from_fn(|_| Vec::new())
    }

    fn temp_dir(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "neuralia-tab-gates-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn open(
        contexts: &mut Columns<ContextTab>,
        groups: &mut Columns<ContextGroup>,
        next_context_id: &mut u64,
        column: usize,
        url: &str,
    ) -> u64 {
        record_split_context(
            &mut contexts[column],
            &mut groups[column],
            next_context_id,
            url.to_string(),
            false,
            None,
            None,
        )
        .expect("a public source becomes a tab")
    }

    /// Uma aba como a barra a mostra: URL e (nome, cor, fechado) do grupo.
    type VisibleTab = (String, Option<(String, &'static str, bool)>);

    /// O que a barra mostra, sem as identidades (que o restauro renumera):
    /// por coluna, cada aba com o nome, a cor e o estado do seu grupo.
    fn visible(
        contexts: &Columns<ContextTab>,
        groups: &Columns<ContextGroup>,
    ) -> Vec<Vec<VisibleTab>> {
        contexts
            .iter()
            .zip(groups)
            .map(|(tabs, column_groups)| {
                tabs.iter()
                    .map(|tab| {
                        let group = tab.group.map(|id| {
                            let group = column_groups
                                .iter()
                                .find(|group| group.id == id)
                                .expect("a grouped tab without its group");
                            (group.name.clone(), group.color.key(), group.collapsed)
                        });
                        (tab.url.clone(), group)
                    })
                    .collect()
            })
            .collect()
    }

    /// Tres colunas como o utilizador as deixa: grupos com cor escolhida, um
    /// fechado, abas soltas e uma aba aberta ao lado.
    fn a_working_session() -> (Columns<ContextTab>, Columns<ContextGroup>, u64, u64, u64) {
        let mut contexts = empty();
        let mut groups = empty();
        let mut next_context_id = 1;
        let mut next_group_id = 1;
        let a = open(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            0,
            "https://a.example/",
        );
        let b = open(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            0,
            "https://b.example/x",
        );
        let c = open(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            0,
            "https://c.example/",
        );
        let first = regroup_context_tab(&mut contexts[0], &mut groups[0], &mut next_group_id, 0)
            .expect("group");
        groups[0][first].color = GroupColor::Pink;
        groups[0][first].name = "Leitura".into();
        let first_id = groups[0][first].id;
        join_context_group(&mut contexts[0], first_id, 2);
        let _ = open(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            2,
            "https://d.example/",
        );
        let _ = open(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            2,
            "https://e.example/",
        );
        let second = regroup_context_tab(&mut contexts[2], &mut groups[2], &mut next_group_id, 1)
            .expect("group");
        groups[2][second].collapsed = true;
        // Coluna 0: [a, c] no grupo "Leitura" e b solta. A aba aberta ao lado
        // e a do meio -- nem a primeira nem a ultima, que um restauro errado
        // acertaria por acaso.
        let order: Vec<u64> = contexts[0].iter().map(|tab| tab.id).collect();
        assert_eq!(order, vec![a, c, b]);
        (contexts, groups, next_context_id, next_group_id, c)
    }

    #[test]
    fn a_new_search_keeps_tabs_and_groups_and_never_reuses_their_ids() {
        let (mut contexts, mut groups, mut next_context_id, mut next_group_id, open_tab) =
            a_working_session();
        let before = visible(&contexts, &groups);
        let mut active = [Some(open_tab), None, None];

        start_new_search(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            &mut next_group_id,
            &mut active,
        );
        assert_eq!(
            visible(&contexts, &groups),
            before,
            "a new search lost tabs or groups"
        );
        assert_eq!(
            active, [None; COMPARATOR_COLUMNS],
            "the query must become the active context of every column"
        );

        // Um grupo e uma aba criados depois da pesquisa nunca herdam a
        // identidade de quem sobreviveu -- senao as abas dos dois grupos
        // fundiam-se na barra.
        let _ = open(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            1,
            "https://f.example/",
        );
        let fresh = regroup_context_tab(&mut contexts[1], &mut groups[1], &mut next_group_id, 0)
            .expect("group");
        let fresh_id = groups[1][fresh].id;
        let survivors = groups[0].iter().chain(&groups[2]);
        assert!(
            survivors.clone().all(|group| group.id != fresh_id),
            "new group {fresh_id} reuses the id of a surviving group"
        );
        let mut ids: Vec<u64> = contexts.iter().flatten().map(|tab| tab.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "a new tab reused a surviving tab id");

        // Contadores atrasados (um estado vindo de outro sitio) sao
        // corrigidos pela propria pesquisa.
        let mut stale_context = 1;
        let mut stale_group = 1;
        start_new_search(
            &mut contexts,
            &mut groups,
            &mut stale_context,
            &mut stale_group,
            &mut active,
        );
        assert!(contexts.iter().flatten().all(|tab| tab.id < stale_context));
        assert!(groups.iter().flatten().all(|group| group.id < stale_group));
    }

    #[test]
    fn a_private_split_never_reaches_the_tab_session_file() {
        let dir = temp_dir("private");
        let path = tab_session::path_in(&dir);
        let mut contexts = empty();
        let mut groups = empty();
        let mut next_context_id = 1;
        let public = open(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            1,
            "https://public.example/page",
        );
        let private = record_split_context(
            &mut contexts[1],
            &mut groups[1],
            &mut next_context_id,
            "https://secret.example/private".to_string(),
            true,
            None,
            None,
        );
        assert_eq!(private, None, "a private source became a tab");

        // O painel privado e o que esta aberto ao lado da coluna 1.
        let session = snapshot_tab_session(&contexts, &groups, Some((1, private, true)));
        tab_session::save(&path, &session).expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("public.example"));
        assert!(
            !text.contains("secret.example"),
            "a private source was written to tabs.json:\n{text}"
        );
        assert_eq!(session.columns[1].active, None);

        // Nem uma fonte privada que traga a identidade de uma aba e marcada
        // como a aba aberta.
        let session = snapshot_tab_session(&contexts, &groups, Some((1, Some(public), true)));
        assert_eq!(
            session.columns[1].active, None,
            "a private split was saved as active"
        );
        let session = snapshot_tab_session(&contexts, &groups, Some((1, Some(public), false)));
        assert_eq!(session.columns[1].active, Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_marks_only_the_saved_active_tab_and_a_search_keeps_every_tab_lazy() {
        let dir = temp_dir("restore");
        let path = tab_session::path_in(&dir);
        let (contexts, groups, _, _, open_tab) = a_working_session();
        let open_position = contexts[0]
            .iter()
            .position(|tab| tab.id == open_tab)
            .expect("open tab");
        assert!(
            open_position != 0 && open_position + 1 != contexts[0].len(),
            "the test needs a tab that is neither first nor last"
        );

        tab_session::save(
            &path,
            &snapshot_tab_session(&contexts, &groups, Some((0, Some(open_tab), false))),
        )
        .expect("save");
        let Loaded::Restored(session) = tab_session::load(&path) else {
            panic!("the saved session did not load back");
        };
        let mut restored = restore_tab_session(&session);

        assert_eq!(
            visible(&restored.contexts, &restored.groups),
            visible(&contexts, &groups),
            "tabs, group membership, names, colours or collapsed state changed on disk"
        );
        // So a aba que estava aberta fica marcada; nenhuma outra coluna
        // carrega nada.
        assert_eq!(
            restored.active,
            [Some(restored.contexts[0][open_position].id), None, None]
        );
        let mut ids: Vec<u64> = restored
            .contexts
            .iter()
            .flatten()
            .map(|tab| tab.id)
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "restored tab ids collide");
        assert!(ids.iter().all(|id| *id < restored.next_context_id));
        assert!(
            restored
                .groups
                .iter()
                .flatten()
                .all(|group| group.id < restored.next_group_id)
        );

        // O comparador so restaura ao abrir uma pesquisa: a pergunta passa a
        // ser o contexto ativo e nenhuma aba restaurada carrega ate ser
        // escolhida. As abas continuam todas na barra.
        start_new_search(
            &mut restored.contexts,
            &mut restored.groups,
            &mut restored.next_context_id,
            &mut restored.next_group_id,
            &mut restored.active,
        );
        assert_eq!(restored.active, [None; COMPARATOR_COLUMNS]);
        assert_eq!(
            visible(&restored.contexts, &restored.groups),
            visible(&contexts, &groups)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restored_group_comes_back_together_with_its_colour() {
        for color in GroupColor::ALL {
            assert_eq!(GroupColor::from_key(color.key()), Some(color));
        }
        let session = TabSession {
            columns: [
                SessionColumn {
                    tabs: vec![
                        SessionTab {
                            id: 1,
                            url: "https://a.example/".into(),
                            title: String::new(),
                            group: Some(9),
                        },
                        SessionTab {
                            id: 2,
                            url: "https://solta.example/".into(),
                            title: String::new(),
                            group: None,
                        },
                        SessionTab {
                            id: 3,
                            url: "https://c.example/".into(),
                            title: String::new(),
                            group: Some(9),
                        },
                    ],
                    groups: vec![SessionGroup {
                        id: 9,
                        name: "Fontes".into(),
                        color: "cor-de-uma-versao-futura".into(),
                        collapsed: false,
                    }],
                    active: Some(2),
                },
                SessionColumn::default(),
                SessionColumn::default(),
            ],
        };
        let restored = restore_tab_session(&session);
        let urls: Vec<&str> = restored.contexts[0]
            .iter()
            .map(|tab| tab.url.as_str())
            .collect();
        // Os membros de um grupo ficam seguidos, senao a pilula rotulava a
        // aba solta que ficou no meio.
        assert_eq!(
            urls,
            vec![
                "https://a.example/",
                "https://c.example/",
                "https://solta.example/"
            ]
        );
        assert_eq!(restored.groups[0].len(), 1);
        assert_eq!(restored.groups[0][0].color, GroupColor::Blue);
        // A ativa segue a aba, nao a posicao.
        assert_eq!(restored.active[0], Some(restored.contexts[0][1].id));
    }

    #[test]
    fn the_tab_fingerprint_sees_every_change_the_file_records() {
        let (contexts, groups, _, _, open_tab) = a_working_session();
        let split = Some((0, Some(open_tab), false));
        let base = tab_session_fingerprint(&contexts, &groups, split);
        assert_eq!(base, tab_session_fingerprint(&contexts, &groups, split));

        let mut changes: Vec<(&str, u64)> = Vec::new();
        let mut recolored = groups.clone();
        recolored[0][0].color = GroupColor::Green;
        changes.push((
            "colour",
            tab_session_fingerprint(&contexts, &recolored, split),
        ));
        let mut collapsed = groups.clone();
        collapsed[0][0].collapsed = !collapsed[0][0].collapsed;
        changes.push((
            "collapsed",
            tab_session_fingerprint(&contexts, &collapsed, split),
        ));
        let mut renamed = groups.clone();
        renamed[0][0].name.push('!');
        changes.push(("name", tab_session_fingerprint(&contexts, &renamed, split)));
        let mut moved = contexts.clone();
        moved[0].swap(0, 1);
        changes.push(("order", tab_session_fingerprint(&moved, &groups, split)));
        let mut ungrouped = contexts.clone();
        ungrouped[0][0].group = None;
        changes.push((
            "membership",
            tab_session_fingerprint(&ungrouped, &groups, split),
        ));
        let mut closed = contexts.clone();
        closed[2].pop();
        changes.push(("closed", tab_session_fingerprint(&closed, &groups, split)));
        changes.push(("active", tab_session_fingerprint(&contexts, &groups, None)));
        for (label, fingerprint) in changes {
            assert_ne!(
                fingerprint, base,
                "{label} would never be saved before exit"
            );
        }
        // Um painel privado aberto nao e uma mudanca a gravar.
        assert_eq!(
            tab_session_fingerprint(&contexts, &groups, Some((0, None, true))),
            tab_session_fingerprint(&contexts, &groups, None)
        );
    }

    #[test]
    fn clearing_history_also_forgets_the_saved_tabs() {
        let dir = temp_dir("clear");
        // O nome escrito a mao, nao `path_in`: o gate tem de apanhar tambem
        // um `tabs.json` que o App passasse a procurar noutro sitio.
        let path = dir.join("tabs.json");
        let (mut contexts, mut groups, _, _, open_tab) = a_working_session();
        let split = Some((0, Some(open_tab), false));
        let mut app = TabPersistence::open(&dir);
        assert_eq!(
            app.save_now(&mut contexts, &mut groups, split)
                .expect("save"),
            TabSave::Written
        );
        std::fs::write(tab_session::backup_path(&path), b"old bad file").expect("bak");
        assert!(path.exists());

        app.forget(&mut contexts, &mut groups, split)
            .expect("forget");
        assert!(!path.exists(), "tabs.json survived \"Apagar histórico\"");
        assert!(
            !tab_session::backup_path(&path).exists(),
            "tabs.json.bak survived \"Apagar histórico\""
        );
        assert!(
            contexts.iter().all(Vec::is_empty) && groups.iter().all(Vec::is_empty),
            "the live tabs survived and would be written back"
        );

        // O fecho do comparador a seguir grava o que ficou -- nada -- e nao
        // recria o ficheiro.
        let _ = app.save_now(&mut contexts, &mut groups, split);
        assert_eq!(tab_session::load(&path), Loaded::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Uma aba nova na coluna, pelo caminho do split.
    fn link(
        contexts: &mut Columns<ContextTab>,
        groups: &mut Columns<ContextGroup>,
        next_context_id: &mut u64,
        url: &str,
    ) -> Option<u64> {
        record_split_context(
            &mut contexts[0],
            &mut groups[0],
            next_context_id,
            url.to_string(),
            false,
            None,
            None,
        )
    }

    fn file_urls(path: &std::path::Path) -> Vec<String> {
        match tab_session::load(path) {
            Loaded::Restored(session) => session
                .columns
                .iter()
                .flat_map(|column| column.tabs.iter().map(|tab| tab.url.clone()))
                .collect(),
            Loaded::Missing => Vec::new(),
            other => panic!("tabs.json ilegivel: {other:?}"),
        }
    }

    /// data-4: o que o App faz com o `tabs.json`, pelo `TabPersistence` que
    /// ele usa -- abrir o comparador le o ficheiro de verdade; o
    /// `SaveTabSession` so grava com o bilhete do ultimo agendado; sair (ou
    /// destruir o comparador) dentro do atraso grava a mudanca; "Apagar
    /// historico" tira o ficheiro com o nome certo. Antes so havia gates das
    /// pecas: desligar qualquer um destes caminhos deixava tudo verde.
    #[test]
    fn the_app_path_saves_restores_and_forgets_the_real_tabs_json() {
        let dir = temp_dir("app-path");
        let path = dir.join("tabs.json");
        let mut app = TabPersistence::open(&dir);
        let (mut restored, notice) = app.restore();
        assert_eq!(notice, None);
        assert!(restored.contexts.iter().all(Vec::is_empty));
        let RestoredTabs {
            contexts,
            groups,
            next_context_id,
            ..
        } = &mut restored;

        // Duas mudancas seguidas: dois bilhetes; so o ultimo grava.
        link(contexts, groups, next_context_id, "https://um.example/");
        let first = app.observe(contexts, groups, None).expect("mudou");
        link(contexts, groups, next_context_id, "https://dois.example/");
        let second = app.observe(contexts, groups, None).expect("mudou");
        assert_eq!(app.observe(contexts, groups, None), None, "nada mudou");
        assert!(
            app.save_due(first, contexts, groups, None).is_none(),
            "um bilhete ultrapassado gravou"
        );
        assert!(!path.exists());
        assert_eq!(
            app.save_due(second, contexts, groups, None)
                .expect("o ultimo bilhete grava")
                .expect("gravado"),
            TabSave::Written
        );
        assert_eq!(
            file_urls(&path),
            ["https://um.example/", "https://dois.example/"]
        );

        // Uma aba aberta e a janela fechada antes do fim do atraso: sair
        // grava-a na mesma (o bilhete dela nunca chegou a disparar).
        link(contexts, groups, next_context_id, "https://tres.example/");
        let _pending = app.observe(contexts, groups, None).expect("mudou");
        assert_eq!(
            app.save_now(contexts, groups, None).expect("sair"),
            TabSave::Written
        );
        assert_eq!(file_urls(&path).len(), 3, "a ultima aba perdeu-se ao sair");
        assert_eq!(
            app.save_now(contexts, groups, None).expect("sair"),
            TabSave::Unchanged
        );

        // O proximo arranque le o mesmo ficheiro.
        drop(app);
        let mut app = TabPersistence::open(&dir);
        let (mut again, notice) = app.restore();
        assert_eq!(notice, None);
        let urls: Vec<&str> = again.contexts[0]
            .iter()
            .map(|tab| tab.url.as_str())
            .collect();
        assert_eq!(
            urls,
            [
                "https://um.example/",
                "https://dois.example/",
                "https://tres.example/"
            ]
        );
        assert_eq!(
            app.save_now(&mut again.contexts, &mut again.groups, None)
                .expect("nada"),
            TabSave::Unchanged,
            "o que acabou de ser lido nao se regrava"
        );

        // "Apagar historico": o ficheiro de verdade sai, e o modelo tambem.
        app.forget(&mut again.contexts, &mut again.groups, None)
            .expect("apagar");
        assert!(!path.exists(), "\"Apagar histórico\" deixou o tabs.json");
        assert!(again.contexts.iter().all(Vec::is_empty));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// data-3 pelo caminho do App: duas janelas no mesmo `data_dir`. A
    /// segunda avisa que nao grava (e nao grava); o "Apagar historico" dela
    /// faz a primeira largar as abas de antes em vez de as escrever de volta.
    #[test]
    fn a_second_window_never_overwrites_the_tabs_or_resurrects_a_clear() {
        let dir = temp_dir("two-windows");
        let path = dir.join("tabs.json");
        tab_session::save(&path, &{
            let mut session = TabSession::default();
            session.columns[0].tabs.push(SessionTab {
                id: 1,
                url: "https://old-secret.example/".into(),
                title: String::new(),
                group: None,
            });
            session
        })
        .expect("sessao anterior");

        let mut first = TabPersistence::open(&dir);
        let mut second = TabPersistence::open(&dir);
        let (mut a, notice_a) = first.restore();
        let (mut b, notice_b) = second.restore();
        assert_eq!(notice_a, None);
        assert!(
            notice_b.is_some_and(|text| text.contains("Outra janela")),
            "a segunda janela tem de dizer que nao salva"
        );

        link(
            &mut a.contexts,
            &mut a.groups,
            &mut a.next_context_id,
            "https://from-a.example/",
        );
        link(
            &mut b.contexts,
            &mut b.groups,
            &mut b.next_context_id,
            "https://from-b.example/",
        );
        assert_eq!(
            first
                .save_now(&mut a.contexts, &mut a.groups, None)
                .expect("a"),
            TabSave::Written
        );
        assert_eq!(
            second
                .save_now(&mut b.contexts, &mut b.groups, None)
                .expect("b"),
            TabSave::NotWriter
        );
        assert!(
            file_urls(&path).contains(&"https://from-a.example/".to_string()),
            "a aba aberta na primeira janela foi apagada pela segunda"
        );

        second
            .forget(&mut b.contexts, &mut b.groups, None)
            .expect("apagar");
        link(
            &mut a.contexts,
            &mut a.groups,
            &mut a.next_context_id,
            "https://depois.example/",
        );
        let result = first.save_now(&mut a.contexts, &mut a.groups, None);
        assert!(
            tab_save_notice(&result).is_some_and(|text| text.contains("apagado")),
            "o dono tem de saber porque as abas sumiram"
        );
        assert_eq!(result.expect("a"), TabSave::ClearedElsewhere);
        assert!(!path.exists(), "o que o dono apagou voltou ao disco");
        assert!(a.contexts.iter().all(Vec::is_empty));

        // A partir dai, a primeira volta a guardar o que nasce.
        link(
            &mut a.contexts,
            &mut a.groups,
            &mut a.next_context_id,
            "https://novo.example/",
        );
        assert_eq!(
            first
                .save_now(&mut a.contexts, &mut a.groups, None)
                .expect("a"),
            TabSave::Written
        );
        assert_eq!(file_urls(&path), ["https://novo.example/"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// data-1: com as abas a sobreviver a pesquisas e reinicios, o limite de
    /// abas passou a ser rotina -- e podava pela esquerda, onde ficam os
    /// grupos feitos primeiro. Um grupo recolhido do dono sobrevive a quatro
    /// rondas de pesquisa + gravacao + reinicio + 10 links; e uma aba posta a
    /// frente nao passa a ser a primeira a sair (nem depois de reiniciar).
    #[test]
    fn a_saved_group_survives_searches_restarts_and_many_new_links() {
        let dir = temp_dir("group-survives");
        let path = dir.join("tabs.json");
        {
            let mut app = TabPersistence::open(&dir);
            let (mut model, _) = app.restore();
            let RestoredTabs {
                contexts,
                groups,
                next_context_id,
                next_group_id,
                ..
            } = &mut model;
            link(contexts, groups, next_context_id, "https://keep.example/1");
            link(contexts, groups, next_context_id, "https://keep.example/2");
            let created = regroup_context_tab(&mut contexts[0], &mut groups[0], next_group_id, 0)
                .expect("grupo");
            groups[0][created].name = "Pesquisa".into();
            groups[0][created].collapsed = true;
            let id = groups[0][created].id;
            join_context_group(&mut contexts[0], id, 1);
            app.save_now(contexts, groups, None).expect("gravar");
        }
        let saved_groups = || match tab_session::load(&path) {
            Loaded::Restored(session) => session.columns[0]
                .groups
                .iter()
                .map(|group| group.name.clone())
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        for round in 0..4 {
            let mut app = TabPersistence::open(&dir);
            let (mut model, _) = app.restore();
            for index in 0..10 {
                link(
                    &mut model.contexts,
                    &mut model.groups,
                    &mut model.next_context_id,
                    &format!("https://r{round}.example/{index}"),
                );
            }
            app.save_now(&mut model.contexts, &mut model.groups, None)
                .expect("gravar");
            assert_eq!(
                saved_groups(),
                ["Pesquisa"],
                "ronda {round}: o grupo do dono sumiu do tabs.json"
            );
        }

        // A coluna esta no limite de soltas. A mais nova vai para a frente
        // (como num arrasto), grava-se e reinicia-se: o link seguinte tira a
        // solta MAIS ANTIGA, nao a que esta a frente.
        let mut app = TabPersistence::open(&dir);
        let (mut model, _) = app.restore();
        let loose = model.contexts[0]
            .iter()
            .filter(|tab| tab.group.is_none())
            .count();
        assert_eq!(loose, tab_session::MAX_TABS_PER_COLUMN);
        let newest = model.contexts[0].pop().expect("aba");
        let moved_url = newest.url.clone();
        model.contexts[0].insert(0, newest);
        app.save_now(&mut model.contexts, &mut model.groups, None)
            .expect("gravar");
        drop(app);
        let mut app = TabPersistence::open(&dir);
        let (mut model, _) = app.restore();
        let oldest_loose = model.contexts[0]
            .iter()
            .filter(|tab| tab.group.is_none())
            .min_by_key(|tab| tab.id)
            .map(|tab| tab.url.clone())
            .expect("soltas");
        assert_ne!(oldest_loose, moved_url, "o reinicio baralhou as idades");
        link(
            &mut model.contexts,
            &mut model.groups,
            &mut model.next_context_id,
            "https://final.example/",
        );
        let urls: Vec<&str> = model.contexts[0]
            .iter()
            .map(|tab| tab.url.as_str())
            .collect();
        assert!(
            urls.contains(&moved_url.as_str()),
            "a aba posta a frente saiu"
        );
        assert!(
            !urls.contains(&oldest_loose.as_str()),
            "a mais antiga ficou"
        );
        assert_eq!(model.groups[0].len(), 1, "o limite tirou o grupo do dono");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// data-1: so o tecto de tudo tira uma aba agrupada, e nunca em
    /// silencio.
    #[test]
    fn only_the_hard_cap_closes_a_grouped_tab_and_the_owner_is_told() {
        // Uma coluna no tecto, toda agrupada.
        let mut contexts: Columns<ContextTab> = empty();
        let mut groups: Columns<ContextGroup> = empty();
        groups[0].push(ContextGroup {
            id: 1,
            name: "Pesquisa".into(),
            color: GroupColor::Blue,
            collapsed: false,
        });
        let cap = tab_session::MAX_KEPT_TABS_PER_COLUMN as u64;
        for id in 1..=cap {
            contexts[0].push(ContextTab {
                id,
                url: format!("https://g.example/{id}"),
                group: Some(1),
            });
        }
        let mut next_context_id = cap + 1;
        let oldest = 1;

        let before = grouped_tab_ids(&contexts[0]);
        let opened = link(
            &mut contexts,
            &mut groups,
            &mut next_context_id,
            "https://solta.example/",
        )
        .expect("aba nova");
        let lost = lost_grouped_tabs(&before, &contexts[0]);
        assert_eq!(lost, 1);
        assert!(
            contexts[0].iter().any(|tab| tab.id == opened),
            "a aba nova saiu"
        );
        assert!(
            contexts[0].iter().all(|tab| tab.id != oldest),
            "saiu outra que nao a agrupada mais antiga"
        );
        assert_eq!(contexts[0].len(), tab_session::MAX_KEPT_TABS_PER_COLUMN);
        assert!(lost_grouped_notice(lost).is_some_and(|text| text.contains("agrupada")));
        assert_eq!(lost_grouped_notice(0), None);
    }

    /// data-6: um endereco que o `tabs.json` recusaria nao vira aba (a barra
    /// e o ficheiro nunca discordam), e um link comprido do Google com
    /// `#:~:text=` -- mais de 2 KiB -- vira aba e sobrevive ao reinicio.
    #[test]
    fn a_long_link_survives_a_restart_and_an_unstorable_one_never_becomes_a_tab() {
        let dir = temp_dir("long-url");
        let path = dir.join("tabs.json");
        let long = format!(
            "https://www.google.com/search?q=neuralia#:~:text={}",
            "palavra%20".repeat(400)
        );
        assert!(long.len() > 2048 && long.len() <= tab_session::MAX_URL_BYTES);
        let huge = format!(
            "https://example.com/{}",
            "a".repeat(tab_session::MAX_URL_BYTES)
        );

        let mut contexts: Columns<ContextTab> = empty();
        let mut groups: Columns<ContextGroup> = empty();
        let mut next_context_id = 1;
        let mut next_group_id = 1;
        link(&mut contexts, &mut groups, &mut next_context_id, &long).expect("aba");
        let created = regroup_context_tab(&mut contexts[0], &mut groups[0], &mut next_group_id, 0)
            .expect("grupo so dela");
        assert_eq!(
            link(&mut contexts, &mut groups, &mut next_context_id, &huge),
            None,
            "um endereco que o ficheiro deitaria fora virou aba"
        );
        assert_eq!(contexts[0].len(), 1);

        let mut app = TabPersistence::open(&dir);
        app.save_now(&mut contexts, &mut groups, None)
            .expect("gravar");
        drop(app);
        let mut app = TabPersistence::open(&dir);
        let (restored, _) = app.restore();
        assert_eq!(restored.contexts[0].len(), 1, "a aba comprida sumiu");
        assert_eq!(restored.groups[0].len(), 1, "o grupo so dela sumiu");
        assert_eq!(restored.groups[0][0].name, groups[0][created].name);
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
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

/// Traduz um `neuralia:<accao>` num evento. E o unico sitio onde a lista de
/// atalhos existe do lado nativo: as paginas so sabem escrever o nome.
pub(in crate::windows_app) fn neuralia_action(target: &str) -> Option<UserEvent> {
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

pub(in crate::windows_app) fn common_ipc_event(action: IpcAction) -> Option<UserEvent> {
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
        // A WebView unica (web externa, Leitor, PDF). As colunas e o Split
        // tratam o `note` antes de chegar aqui.
        IpcAction::Note { via } => UserEvent::NoteRequested { target: None, via },
        // Colunas, Split normal, Web externa, Reader e PDF. O Split privado
        // recusa antes de chegar aqui (`split_ipc_event_impl`).
        IpcAction::Search { text, intent } => UserEvent::SearchSelection { text, intent },
        _ => return None,
    })
}

/// O que o "Mandar para IA" e o "Traduzir" da barra de selecao fazem com o
/// texto. Ha uma so saida: a comparacao normal das tres IAs com o texto como
/// pergunta (no Traduzir, dentro do pedido fixo de traducao).
#[derive(Debug, PartialEq)]
enum SelectionSearch {
    Compare(String),
}

/// A decisao do "Mandar para IA" / "Traduzir": o texto selecionado, limpo por
/// `selection_question`, e mais nada. Nao passa pelo `route_input`, pelo
/// `parse_intent` nem pela palette. Quem seleciona "agent:https://x",
/// "tema:escuro" ou um endereco numa pagina quer saber o que aquilo e, nao
/// correr um agente, mudar o tema ou navegar -- e a pagina, que escolhe o
/// texto, nunca pode dar ordens ao navegador por esta via.
fn selection_search(text: &str) -> Option<SelectionSearch> {
    let question = selection_question(text);
    (!question.is_empty() && question.chars().count() <= SEARCH_MAX_CHARS)
        .then_some(SelectionSearch::Compare(question))
}

/// O que chega ao cartao de confirmacao.
#[derive(Debug, PartialEq)]
enum SearchCardInput {
    /// `search` de uma barra de selecao (o Split privado ja o recusou), com
    /// o botao que o pediu.
    Request { text: String, intent: SearchIntent },
    /// Clique nativo num botao do cartao que tinha `token` pintado, com
    /// `shown` caracteres do texto a vista.
    Answer {
        token: u64,
        button: SearchCardButton,
        shown: usize,
    },
    /// Os segundos do cartao `token` passaram.
    Expire(u64),
}

/// O que o cartao faz com uma entrada.
#[derive(Debug, Clone, PartialEq)]
enum SearchCardOutcome {
    /// Mostrar o cartao de `intent` com `text` (a pergunta ja limpa);
    /// `replaced` quando troca um que ainda esperava.
    Show {
        token: u64,
        intent: SearchIntent,
        text: String,
        replaced: bool,
    },
    /// O clique em confirmar: a unica saida que leva texto as tres IAs -- e
    /// so o que o cartao mostrou, ja com o pedido de traducao quando foi o
    /// Traduzir (`CompareRequest::selection`).
    Confirmed(CompareRequest),
    Cancelled,
    Expired,
    /// Nada muda: pedido invalido, token de um cartao que ja nao esta la,
    /// confirmar cedo demais.
    Ignored,
}

/// O pedido do cartao da barra (`SearchCard`, um `NativeCard`): o botao que
/// o pediu e a pergunta ja limpa. O token e a hora sao do `NativeCard`.
struct PendingSearch {
    intent: SearchIntent,
    question: String,
}

/// O mapa de teclas (e a barra de selecao que vive nele) com a capability e
/// o sinal de superficie privada postos. Um painel privado nao mostra o
/// "Mandar para IA" nem o "Traduzir": o texto dele nao pode ir parar ao
/// historico nem a memoria.
pub(in crate::windows_app) fn bind_page_script(
    script: &str,
    capability: &str,
    private: bool,
) -> String {
    script.replace("__NEURALIA_CAP__", capability).replace(
        "__NEURALIA_PRIVATE__",
        if private { "true" } else { "false" },
    )
}

/// Os scripts de uma coluna do comparador, pela ordem em que
/// `comparator_webview_builder` os injeta (um por chamada).
fn comparator_init_scripts(col_index: usize, col_name: &str, capability: &str) -> [String; 4] {
    [
        format!(
            "window.__neuralia_col_index = {col_index}; window.__neuralia_col_name = '{col_name}';"
        ),
        bind_page_script(NEURALIA_KEYMAP_SCRIPT, capability, false),
        AI_AUTO_SUBMIT_SCRIPT.replace("__NEURALIA_CAP__", capability),
        COMPARATOR_INJECT_SCRIPT.replace("__NEURALIA_CAP__", capability),
    ]
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

pub(in crate::windows_app) fn remote_capability() -> String {
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
pub(in crate::windows_app) fn local_origin_of(url: &Url) -> Option<String> {
    is_local_network_target(url).then(|| url.origin().ascii_serialization())
}

pub(in crate::windows_app) fn web_media_permission(
    kind: PermissionKind,
    user_visible: bool,
) -> PermissionResponse {
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

pub(in crate::windows_app) fn remote_web_target(target: &str, local_origin: Option<&str>) -> bool {
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
pub(in crate::windows_app) fn is_view_source_target(
    target: &str,
    local_origin: Option<&str>,
) -> bool {
    let Some(rest) = target.strip_prefix("view-source:") else {
        return false;
    };
    neural_core::validate_web_url(rest).is_ok_and(|url| {
        !is_local_network_target(&url)
            || local_origin.is_some_and(|allowed| url.origin().ascii_serialization() == allowed)
    })
}

pub(in crate::windows_app) fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
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

fn draw_home(
    window: &Window,
    status: Option<&str>,
    go_hover: bool,
    tool_hover: Option<Tool>,
    pomodoro_label: Option<BarLabel>,
) {
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

        // Ferramentas no canto de cima, a esquerda dos botoes da janela.
        let tools = home_tool_buttons(width, scale, pomodoro_label);
        let labels = [pomodoro_label, None, None];
        for ((rect, tool), label) in tools.iter().zip(Tool::ALL).zip(labels) {
            draw_tool_button(
                target,
                *rect,
                tool,
                label.as_ref().map(BarLabel::as_str),
                label.and_then(|label| label.phase),
                tool_hover == Some(tool),
                scale,
                small_font,
                &theme,
                theme.page_bg,
            );
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

// O arrasto das abas (2.1.7), a etiqueta do Pomodoro e o resto do estado
// chegam da `App` num `BarState` so (`App::bar_state`).
fn draw_comparator_bar<W>(
    window: &Window,
    comp: &ComparatorState,
    state: BarState,
    live: &LivePanel<W>,
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
            bar_columns(comp, state.pomodoro_label),
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
            comp.bar_focus,
            state,
            live,
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
#[cfg(test)]
unsafe fn paint_comparator_bar<W>(
    target: *mut core::ffi::c_void,
    width: i32,
    scale: f64,
    names: &[&str],
    state: BarState,
    live: &LivePanel<W>,
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
        [None; COMPARATOR_COLUMNS],
        state,
        live,
        theme,
    );
}

/// `live` e o proprio painel do Gemini Live: o olho pinta-se do estado dele,
/// nao de um booleano que o chamador possa trocar por `false`.
#[allow(clippy::too_many_arguments)]
unsafe fn paint_comparator_bar_with_contexts<W>(
    target: *mut core::ffi::c_void,
    width: i32,
    scale: f64,
    names: &[&str],
    columns: BarColumns,
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    active_context: Option<(usize, Option<u64>, bool, bool)>,
    focus: [Option<u64>; COMPARATOR_COLUMNS],
    state: BarState,
    live: &LivePanel<W>,
    theme: &Theme,
) {
    let BarState {
        hover,
        visible,
        auto_scroll,
        drag,
        ..
    } = state;
    // A meio de um arrasto a fila da coluna desenha-se ja como ficara se o
    // botao subir agora: as outras abas abrem lugar ao que se arrasta -- e o
    // que se arrasta fica sempre na fila (e a ancora da coluna), como no
    // Chrome, em vez de sumir quando cai longe das abas recentes.
    let preview = drag.and_then(|drag| drag_preview_model(contexts, groups, focus, drag));
    let (contexts, groups, focus) = match &preview {
        Some((tabs, column_groups, focus)) => (tabs, column_groups, *focus),
        None => (contexts, groups, focus),
    };
    let layout = BarLayout::with_rows(
        width as f64,
        scale,
        visible,
        columns,
        tab_rows_focused(
            contexts,
            groups,
            active_context.map(|(source, id, _, _)| (source, id)),
            focus,
        ),
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
    let chip_font = create_font((-10.0 * scale) as i32, FW_BOLD as i32);
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
        // "‹N": ha N abas desta coluna que a barra nao mostra; o clique lista
        // todas.
        if layout.tab_overflow_counts[index] > 0 {
            let hovered = hover == Some(BarHit::TabOverflow(index));
            let fill = mix(theme.bar_bg, brand, if hovered { 0.30 } else { 0.12 });
            draw_pill(
                target,
                layout.tab_overflow[index],
                &format!("\u{2039}{}", layout.tab_overflow_counts[index]),
                PillStyle::new(fill, mix(theme.bar_bg, brand, 0.30), theme.fg),
                scale,
                tab_font,
                theme.bar_bg,
            );
        }
        let dragging = drag.filter(|drag| drag.source_index == index);
        // O que se arrasta segue o rato por cima da fila ja reordenada, e por
        // isso e desenhado por ultimo; fora da faixa onde se larga volta ao
        // seu lugar, esbatido.
        let float_dx = dragging.and_then(|drag| {
            drag.float_left.and_then(|left| {
                drag_float_offset(
                    &layout,
                    index,
                    source_contexts,
                    &groups[index],
                    drag.item,
                    left,
                )
            })
        });
        let is_dragged = |kind: RowKind| {
            dragging.is_some_and(|drag| {
                row_item_is_dragged(kind, drag.item, source_contexts, &groups[index])
            })
        };
        let shifted = |rect: UiRect, dx: f64| UiRect {
            x: rect.x + dx,
            ..rect
        };
        for floating in [false, true] {
            let Some(dx) = (if floating { float_dx } else { Some(0.0) }) else {
                break;
            };
            // A pilula do grupo, como no Chrome: cheia da cor do grupo, com o
            // nome legivel por cima, e um fio da mesma cor por baixo das abas
            // do grupo -- e isso que mostra, de relance, que abas estao
            // dentro dele.
            for visual in 0..layout.group_pill_counts[index] {
                let group_index = layout.group_pill_indices[index][visual];
                let Some(group) = groups[index].get(group_index) else {
                    continue;
                };
                let dragged = is_dragged(RowKind::Chip(group_index));
                if (dragged && float_dx.is_some()) != floating {
                    continue;
                }
                let color = group.color.rgb();
                let hovered = hover
                    == Some(BarHit::ContextGroup {
                        source_index: index,
                        group_index,
                    });
                let fill = if dragged && !floating {
                    mix(theme.bar_bg, color, 0.45)
                } else if hovered {
                    mix(color, theme.bar_bg, 0.18)
                } else {
                    color
                };
                draw_pill(
                    target,
                    shifted(layout.group_pills[index][visual], dx),
                    &group.name,
                    PillStyle::new(fill, fill, on_color(fill)),
                    scale,
                    chip_font,
                    theme.bar_bg,
                );
                let line = shifted(layout.group_lines[index][visual], dx);
                if line.width > 0.0 {
                    fill_pill(target, line, line.height / 2.0, color, None, theme.bar_bg);
                }
            }
            for visual in 0..layout.context_tab_counts[index] {
                let context_index = layout.context_indices[index][visual];
                let Some(tab) = source_contexts.get(context_index) else {
                    continue;
                };
                let owner = layout.tab_owners[index][visual];
                let dragged = is_dragged(RowKind::Tab {
                    context: context_index,
                    owner,
                });
                if (dragged && float_dx.is_some()) != floating {
                    continue;
                }
                let url = tab.url.as_str();
                // Uma aba agrupada veste a cor do grupo, nao a do provedor: e
                // assim que se ve de relance onde acaba um grupo e comeca o
                // outro. A arrastada ja veste a do grupo onde vai cair.
                let group_color = owner
                    .and_then(|owner| groups[index].get(owner))
                    .map(|group| group.color.rgb());
                let tint = group_color.unwrap_or(brand);
                let active = active_context.is_some_and(|(source, active_id, _, _)| {
                    source == index && active_id == Some(tab.id)
                });
                let close_hovered = hover
                    == Some(BarHit::CloseTab {
                        source_index: index,
                        context_index,
                    });
                let hovered = close_hovered
                    || hover
                        == Some(BarHit::ContextTab {
                            source_index: index,
                            context_index,
                        });
                // A que vai na mao do rato fica levantada, como sob o rato.
                let mut fill = if active {
                    mix(theme.bar_bg, tint, 0.48)
                } else if hovered || floating {
                    mix(theme.bar_bg, tint, 0.30)
                } else {
                    mix(theme.bar_bg, tint, 0.12)
                };
                if dragged && !floating {
                    fill = mix(fill, theme.bar_bg, 0.5);
                }
                // A aba aberta de um grupo leva o contorno na cor do grupo,
                // como a aba ativa de um grupo no Chrome.
                let border = match (active, group_color) {
                    (true, Some(color)) => (color, 2.0 * scale),
                    _ => (mix(theme.bar_bg, tint, 0.30), scale),
                };
                let close_slot = shifted(layout.tab_closes[index][visual], dx);
                let close = (close_slot.width > 0.0 && !dragged)
                    .then(|| tab_close_style(close_hovered, fill, theme));
                draw_context_tab(
                    target,
                    shifted(layout.context_tabs[index][visual], dx),
                    &context_tab_label(url),
                    fill,
                    border,
                    if active { theme.fg } else { theme.fg_muted },
                    close_slot,
                    close,
                    scale,
                    (tab_font, font),
                    theme.bar_bg,
                );
            }
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
        for button in ColumnButton::ALL {
            pairs.push((
                layout.column_button(index, button),
                button.glyph(),
                button.hit(index),
            ));
        }
    }
    for (rect, label, hit) in pairs {
        if rect.width > 0.0 {
            draw_button(target, rect, label, hover == Some(hit), scale, font, theme);
        }
    }

    // Os mesmos rectangulos que o hit-testing usa; ver `right_controls`.
    let controls = right_controls(
        width as f64,
        scale,
        active_context.is_some(),
        columns.pomodoro_label,
    );
    // Ferramentas, num grupo a esquerda do Gemini Live. O Pomodoro leva o tempo
    // ao lado do icone quando `right_controls` lhe deu largura para isso.
    let labels = [columns.pomodoro_label, None, None];
    for ((rect, tool), label) in controls.tools.iter().zip(Tool::ALL).zip(labels) {
        draw_tool_button(
            target,
            *rect,
            tool,
            label.as_ref().map(BarLabel::as_str),
            label.and_then(|label| label.phase),
            hover == Some(BarHit::Tool(tool)),
            scale,
            font,
            theme,
            theme.bar_bg,
        );
    }
    // O canto direito pela ordem do registo (`RIGHT_CLUSTER`): o olho do
    // Gemini Live pinta-se do estado do painel; os outros sao icones, com
    // a cor de cada um -- o envelope do Gmail apaga-se com os avisos
    // desligados; o Privado e o chapeu e os oculos, sem nome (pedido do
    // dono). Os lugares nao se tocam, por isso a ordem de pintura e a do
    // registo.
    let gmail_tint = if GMAIL_NOTIFICATIONS.load(Ordering::Acquire) {
        theme.fg
    } else {
        theme.fg_muted
    };
    for (slot, rect) in RIGHT_CLUSTER.iter().zip(controls.cluster()) {
        let hovered = hover == Some(slot.hit);
        match slot.hit {
            BarHit::GeminiLive => {
                draw_live_button(target, rect, live.indicator(), hovered, scale, theme);
            }
            hit => {
                let tint = match hit {
                    BarHit::Service(Service::Meet) | BarHit::Private => Some(theme.fg),
                    BarHit::GmailToggle => Some(gmail_tint),
                    _ => None,
                };
                draw_icon_button(target, rect, slot.icon, tint, hovered, scale, theme);
            }
        }
    }

    if let (Some((source_index, _url, fullscreen, private_split)), Some((label, expand, close))) =
        (active_context, controls.split)
    {
        let source = names.get(source_index).copied().unwrap_or("IA");
        // Numa janela estreita o rotulo cede lugar (ver `right_controls_flex`)
        // e pode nem existir: sem largura nao se escreve nada.
        if label.width > 0.0 {
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
        }
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
    DeleteObject(chip_font as _);
}

/// Uma aba da barra: a pilula, o titulo centrado no espaco que o x deixa e,
/// se `close` vier, o x redondo a direita. O lugar do x fica reservado mesmo
/// escondido, para o titulo nao saltar quando o rato entra na aba.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_context_tab(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    label: &str,
    fill: Rgb,
    border: (Rgb, f64),
    text: Rgb,
    close_slot: UiRect,
    close: Option<PillStyle>,
    scale: f64,
    (label_font, glyph_font): (*mut core::ffi::c_void, *mut core::ffi::c_void),
    background: Rgb,
) {
    fill_pill(hdc, rect, rect.height / 2.0, fill, Some(border), background);
    let right_edge = rect.x + rect.width;
    let inset = if close_slot.width > 0.0 {
        right_edge - close_slot.x + 2.0 * scale
    } else {
        8.0 * scale
    };
    SelectObject(hdc, label_font as _);
    SetTextColor(hdc, rgb3(text));
    SetBkMode(hdc, TRANSPARENT as i32);
    let mut text_rect = RECT {
        left: (rect.x + inset).round() as i32,
        top: rect.y as i32,
        right: (right_edge - inset).round() as i32,
        bottom: (rect.y + rect.height) as i32,
    };
    draw_text(
        hdc,
        label,
        &mut text_rect,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    if let Some(style) = close {
        fill_pill(
            hdc,
            close_slot,
            close_slot.height / 2.0,
            style.fill,
            Some((style.border, scale)),
            fill,
        );
        SelectObject(hdc, glyph_font as _);
        SetTextColor(hdc, rgb3(style.text));
        let mut glyph = RECT {
            left: close_slot.x.round() as i32,
            top: close_slot.y.round() as i32,
            right: (close_slot.x + close_slot.width).round() as i32,
            bottom: (close_slot.y + close_slot.height).round() as i32,
        };
        draw_text(
            hdc,
            "\u{00D7}",
            &mut glyph,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
    }
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

pub(in crate::windows_app) unsafe fn create_font(
    height: i32,
    weight: i32,
) -> *mut core::ffi::c_void {
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

pub(in crate::windows_app) const PDF_VIEWER_JS: &[u8] =
    include_bytes!("../../../assets/pdfjs/viewer.mjs");
pub(in crate::windows_app) const PDFJS_CORE: &[u8] =
    include_bytes!("../../../assets/pdfjs/pdf.mjs");
pub(in crate::windows_app) const PDFJS_WORKER: &[u8] =
    include_bytes!("../../../assets/pdfjs/pdf.worker.mjs");
/// No Windows um esquema personalizado `neuralia-pdf` aparece a pagina como
/// `http://neuralia-pdf.<host>`; o wry intercepta tudo o que comece assim.
pub(in crate::windows_app) const PDF_ORIGIN: &str = "http://neuralia-pdf.localhost";
/// A mesma politica do `<meta>` do viewer.html, servida em cabecalho para
/// valer antes de o HTML ser lido; um teste garante que as duas nao divergem.
pub(in crate::windows_app) const PDF_VIEWER_CSP: &str = "default-src 'none'; script-src 'self' blob: 'wasm-unsafe-eval'; worker-src 'self' blob:; connect-src 'self'; img-src 'self' blob: data:; style-src 'unsafe-inline'; font-src 'self' data:; object-src 'none'; base-uri 'none'; form-action 'none'";
/// Limite para um documento; o do Reader (2 MiB) e para HTML.
const PDF_MAX_BYTES: usize = 32 * 1024 * 1024;
const PDF_TIMEOUT_SECS: u64 = 90;

static BRAND_IMAGE: OnceLock<RgbaImage> = OnceLock::new();
static SPLASH_CACHE: Mutex<SplashCache> = Mutex::new(None);

/// Arte da marca mostrada na tela inicial: e a unica coisa la, com a barra.
fn get_brand_image() -> &'static RgbaImage {
    BRAND_IMAGE.get_or_init(|| {
        let raw = include_bytes!("../../../assets/neuralia-home.png");
        image::load_from_memory(raw)
            .expect("assets/neuralia-home.png must be valid PNG")
            .to_rgba8()
    })
}

/// O icone do projeto e o grupo 1 dos recursos do proprio executavel: o
/// `build.rs` compila la o `assets/logo.ico` (tests/brand_assets.rs confere-o
/// no NeuralIA.exe). O mesmo que o Explorador e o atalho mostram.
const APP_ICON_RESOURCE: u16 = 1;

/// (pequeno, grande): o da barra de titulo e do Alt+Tab, e o da barra de
/// tarefas. Cada um pedido no tamanho que o sistema usa a esta escala, para o
/// Windows escolher a entrada certa do icone em vez de esticar uma so -- o
/// icone anterior era uma imagem de 64 px com os cantos arredondados a mao,
/// reduzida ou ampliada a tudo.
fn app_icons() -> (Option<Icon>, Option<Icon>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXICON, SM_CXSMICON};
    use winit::{dpi::PhysicalSize, platform::windows::IconExtWindows};

    let load = |side: i32| {
        let side = side.clamp(16, 256) as u32;
        Icon::from_resource(APP_ICON_RESOURCE, Some(PhysicalSize::new(side, side))).ok()
    };
    let (small, big) = unsafe { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CXICON)) };
    (load(small), load(big))
}

/// A janela principal, tal como o `resumed` a cria: o icone do projeto na
/// barra de titulo/Alt+Tab (pequeno) e na barra de tarefas (grande).
fn main_window_attributes() -> winit::window::WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows;
    let (small_icon, big_icon) = app_icons();
    Window::default_attributes()
        .with_title("NeuralIA")
        // Maximizada a abrir: e um browser, e o comparador de tres colunas nao
        // cabe com folga em 1120 px. O `inner_size` fica como o tamanho de
        // restauro, para quem carregar no botao do meio.
        .with_maximized(true)
        // Sem a barra do Windows em lado nenhum: a Home e o comparador
        // desenham os seus proprios botoes da janela (pedido do dono).
        .with_decorations(false)
        .with_inner_size(LogicalSize::new(1120.0, 760.0))
        .with_min_inner_size(LogicalSize::new(700.0, 500.0))
        .with_window_icon(small_icon)
        .with_taskbar_icon(big_icon)
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

/// Cada ficheiro de `src/windows_app/` com o seu texto, para os gates que
/// leem o codigo-fonte (`all_sources`, `shipped_source` em `tests.rs`):
/// primeiro os `impl App` de `app/`, depois os modulos-folha, por fim os
/// testes. `all_sources_lists_every_module` compara a lista com o disco.
#[cfg(test)]
pub(super) const ALL_MODULES: &[(&str, &str)] = &[
    ("app/mod.rs", include_str!("windows_app/app/mod.rs")),
    (
        "app/navigation.rs",
        include_str!("windows_app/app/navigation.rs"),
    ),
    ("app/panels.rs", include_str!("windows_app/app/panels.rs")),
    ("app/gmail.rs", include_str!("windows_app/app/gmail.rs")),
    ("app/tools.rs", include_str!("windows_app/app/tools.rs")),
    ("app/split.rs", include_str!("windows_app/app/split.rs")),
    ("app/pages.rs", include_str!("windows_app/app/pages.rs")),
    ("app/chrome.rs", include_str!("windows_app/app/chrome.rs")),
    ("app/compare.rs", include_str!("windows_app/app/compare.rs")),
    ("app/tabs.rs", include_str!("windows_app/app/tabs.rs")),
    (
        "app/event_loop.rs",
        include_str!("windows_app/app/event_loop.rs"),
    ),
    ("theme.rs", include_str!("windows_app/theme.rs")),
    ("icons.rs", include_str!("windows_app/icons.rs")),
    ("bar_layout.rs", include_str!("windows_app/bar_layout.rs")),
    ("tab_row.rs", include_str!("windows_app/tab_row.rs")),
    ("native.rs", include_str!("windows_app/native.rs")),
    ("splash.rs", include_str!("windows_app/splash.rs")),
    (
        "page_scripts.rs",
        include_str!("windows_app/page_scripts.rs"),
    ),
    ("notes.rs", include_str!("windows_app/notes.rs")),
    ("side_panel.rs", include_str!("windows_app/side_panel.rs")),
    (
        "clear_history.rs",
        include_str!("windows_app/clear_history.rs"),
    ),
    ("services.rs", include_str!("windows_app/services.rs")),
    ("search_card.rs", include_str!("windows_app/search_card.rs")),
    (
        "secret_prompt.rs",
        include_str!("windows_app/secret_prompt.rs"),
    ),
    ("toast.rs", include_str!("windows_app/toast.rs")),
    ("popup_menu.rs", include_str!("windows_app/popup_menu.rs")),
    ("native_card.rs", include_str!("windows_app/native_card.rs")),
    ("page_eval.rs", include_str!("windows_app/page_eval.rs")),
    (
        "webview_hooks.rs",
        include_str!("windows_app/webview_hooks.rs"),
    ),
    ("commands.rs", include_str!("windows_app/commands.rs")),
    ("keymap.rs", include_str!("windows_app/keymap.rs")),
    ("adblock.rs", include_str!("windows_app/adblock.rs")),
    ("tests.rs", include_str!("windows_app/tests.rs")),
];

/// A raiz e todos os modulos de `ALL_MODULES` (com `tests.rs`), LF.
#[cfg(test)]
pub(super) fn all_sources() -> String {
    let mut out = include_str!("windows_app.rs").replace("\r\n", "\n");
    for (_, content) in ALL_MODULES {
        out.push('\n');
        out.push_str(&content.replace("\r\n", "\n"));
    }
    out
}

#[cfg(test)]
pub(super) use crate::pomodoro_ui::{POMODORO_COMMAND_HELP, PomodoroCommand};
#[cfg(test)]
pub(super) use neural_core::parse_intent;
// Nomes que so os testes usam por `use super::*` desde que o codigo que os
// usava saiu da raiz (split-c): no binario ficavam como import nao usado.
#[cfg(test)]
pub(super) use neural_core::ReaderBlock;
#[cfg(test)]
pub(super) use std::borrow::Cow;
#[cfg(test)]
pub(super) use wry::http::{Request, Response as HttpResponse};

#[cfg(test)]
mod tests;

pub(in crate::windows_app) mod theme;
pub(in crate::windows_app) use theme::*;

pub(in crate::windows_app) mod icons;
pub(in crate::windows_app) use icons::*;

pub(in crate::windows_app) mod bar_layout;
pub(in crate::windows_app) use bar_layout::*;

pub(in crate::windows_app) mod tab_row;
pub(in crate::windows_app) use tab_row::*;

pub(in crate::windows_app) mod native;
pub(in crate::windows_app) use native::*;

pub(in crate::windows_app) mod splash;
pub(in crate::windows_app) use splash::*;

pub(in crate::windows_app) mod page_scripts;
pub(in crate::windows_app) use page_scripts::*;

pub(in crate::windows_app) mod notes;
pub(in crate::windows_app) use notes::*;

pub(in crate::windows_app) mod side_panel;
pub(in crate::windows_app) use side_panel::*;
pub(in crate::windows_app) mod clear_history;
#[allow(unused_imports)]
pub(in crate::windows_app) use clear_history::*;
pub(in crate::windows_app) mod services;
pub(in crate::windows_app) use services::*;
pub(in crate::windows_app) mod search_card;
pub(in crate::windows_app) use search_card::*;
pub(in crate::windows_app) mod secret_prompt;
pub(in crate::windows_app) use secret_prompt::*;
pub(in crate::windows_app) mod toast;
pub(in crate::windows_app) use toast::*;
pub(in crate::windows_app) mod popup_menu;
pub(in crate::windows_app) use popup_menu::*;
pub(in crate::windows_app) mod native_card;
pub(in crate::windows_app) use native_card::*;
// Leitura de paginas por script so-leitura (infra-llm-untrusted, plano 2.3):
// os consumidores (Traducao, Consenso, Copiloto, Escudo) chegam nas ondas
// seguintes; ate la so corre nos testes.
#[cfg_attr(not(test), allow(dead_code))]
pub(in crate::windows_app) mod page_eval;
#[cfg_attr(not(test), allow(unused_imports))]
pub(in crate::windows_app) use page_eval::*;
pub(in crate::windows_app) mod webview_hooks;
pub(in crate::windows_app) use webview_hooks::*;
pub(in crate::windows_app) mod commands;
pub(in crate::windows_app) use commands::*;
pub(in crate::windows_app) mod keymap;
pub(in crate::windows_app) use keymap::*;
pub(in crate::windows_app) mod adblock;
pub(in crate::windows_app) use adblock::*;

pub(in crate::windows_app) mod app;
#[allow(unused_imports)]
pub(in crate::windows_app) use app::*;

// Spike do AcceleratorKeyPressed (infra-accel-spike, plano 2.3), so no build
// de CI com `--features accel-spike`. Fora de `src/windows_app/` de
// proposito: nao embarca (o exe publicado e compilado sem a feature), por
// isso nao e `ALL_MODULES` nem `shipped_source()`.
#[cfg(feature = "accel-spike")]
#[path = "accel_spike_app.rs"]
pub(in crate::windows_app) mod accel_spike_app;

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

/// Um interruptor partilhado entre a aplicacao e callbacks nativos que correm
/// fora do `&mut App` -- o menu do botao direito das colunas, que o WebView2
/// monta no instante do clique. Clonar partilha a MESMA celula: uma copia
/// envelheceria, e o menu ofereceria "Ativar" com a rolagem ja ligada pelo
/// Ctrl+R. Tudo corre na thread da interface (o WebView2 chama os handlers
/// nela), por isso `Rc<Cell>` e nao atomicos.
#[derive(Clone, Default)]
struct SharedFlag(Rc<Cell<bool>>);

impl SharedFlag {
    fn get(&self) -> bool {
        self.0.get()
    }

    fn set(&self, on: bool) {
        self.0.set(on);
    }

    /// Inverte e devolve o estado novo.
    fn toggle(&self) -> bool {
        let on = !self.0.get();
        self.0.set(on);
        on
    }
}

/// O item de rolagem dos menus de coluna diz o que o clique FAZ: com a
/// rolagem ligada oferece desativar, desligada oferece ativar.
fn auto_scroll_menu_label(on: bool) -> &'static str {
    if on {
        "Desativar rolagem automática (Ctrl+R)"
    } else {
        "Ativar rolagem automática (Ctrl+R)"
    }
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

/// Rotulos do botao injetado no comparador. Em tela cheia a barra nativa some,
/// por isso este botao tem de anunciar a saida.
const COMPARATOR_BUTTON_EXPANDED: &str = "(function(){var b=document.querySelector('#neuralia-comp-expand');if(b){b.style.display='none';}var m=document.querySelector('#neuralia-comp-minimize');if(m){m.style.display='none';}})();";
const COMPARATOR_BUTTON_COLLAPSED: &str = "(function(){var b=document.querySelector('#neuralia-comp-expand');if(b){b.style.display='block';b.textContent='\u{26F6} ' + (window.__neuralia_col_name || 'IA');}var m=document.querySelector('#neuralia-comp-minimize');if(m){m.style.display='block';}})();";

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
