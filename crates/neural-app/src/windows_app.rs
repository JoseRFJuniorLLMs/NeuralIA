#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    borrow::Cow,
    cell::Cell,
    collections::BinaryHeap,
    ffi::OsString,
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

use crate::epub_app::{
    EPUB_SCHEME, EpubJob, EpubNotice, EpubResponse, EpubRuntime, EpubUiRequest, ServeJob,
    dispatch_epub_request, epub_dialog_filter, epub_drop_job, epub_navigation_allowed,
    epub_request_target, handle_epub_ipc, is_epub_path, library_url, notice_script,
    parse_dialog_selection, reader_url,
};
use crate::gemini_live::{
    LIVE_PROTOCOL, LiveAction, LiveIndicator, LiveKeyStore, LiveMessage, LivePanel,
    live_ipc_message, live_page_url, live_panel_navigation, live_step, live_theme_script,
    redact_debug_secrets, serve_live_asset,
};
#[cfg(test)]
use crate::ipc::constant_time_eq;
use crate::ipc::{
    ColumnHint, IpcAction, NoteVia, SEARCH_MAX_CHARS, SearchIntent, parse_ipc_message,
};
use crate::panel_chrome::{
    Area, CAPTION_HOT_MARGIN, CaptionReveal, EXIT_PAGE_FULLSCREEN_SCRIPT, PANEL_HANDLE_WIDTH,
    PANEL_WIDTHS_FILE, PanelKind, PanelWidths, PanelWindowFullscreen, RevealStep,
    SERVICE_STRIP_HEIGHT, ScreenRect, ServiceBadge, ServiceEffect, ServiceFrame, ServiceInput,
    ServicePanelState, StripButton, WheelRoute, caption_hot_zone, is_escape_down, panel_area,
    panel_handle_area, panel_width, panel_width_from_drag, strip_buttons, strip_hit,
    wheel_message_params, wheel_route,
};
use crate::pomodoro_ui::{
    POMODORO_COMMAND_HELP, PomodoroCommand, PomodoroController, PomodoroHost, PomodoroMenuItem,
    TickSchedule, TickScheduler, WindowAttention, parse_pomodoro_command, phase_color,
    pomodoro_menu_command,
};
use crate::read_aloud::READ_ALOUD_SCRIPT;
use crate::tab_session::{self, Loaded, SessionColumn, SessionGroup, SessionTab, TabSession};
use neural_core::{
    ActionRisk, AgentAction, AgentElement, AgentPermissionPolicy, AgentRuntimeConfig,
    AgentSecurityAction, CoreConfig, FieldKind, HistoryEntry, HistoryKind, HistoryStore, Intent,
    MemoryDocument, MemoryHit, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore, Note,
    ObservedPage, Phase, ReaderArticle, ReaderBlock, ReaderClient, ResearchItemKind,
    ResearchSession, ZettelError, ZettelStore, chatgpt_search_url, claude_search_url,
    google_ai_url, is_local_network_target, is_pdf_url, parse_intent, reader_html,
    redact_sensitive_text, tissue,
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

use side_panel::PanelExit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageTarget {
    Column(usize),
    Split,
}

#[derive(Debug)]
enum UserEvent {
    /// Escolha de tema feita no menu do botao Home.
    ThemeChosen(ThemeChoice),
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
    /// "Abrir?" do aviso do Gmail: Sim (true) ou Nao.
    GmailAnswer(bool),
    HomeRequested,
    /// Voltar um nivel: de ecra completo para tres colunas, de la para a Home.
    BackRequested,
    /// Outra janela ficou com o rato a meio do gesto numero N na fila de
    /// abas (WM_CAPTURECHANGED): o arrasto desse gesto cancela-se.
    TabCaptureLost(u64),
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
    /// Ctrl+O na omnibox da Home: o dialogo de livros.
    OpenEpubDialog,
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
/// Quanto tempo o aviso de correio novo fica no canto.
/// Com a pergunta "Abrir?" o aviso fica mais tempo a vista.
const GMAIL_TOAST_SECONDS: u64 = 12;
/// Quantas entradas do historico a caixa "history:" mostra.
const HISTORY_RECENT_LIMIT: usize = 20;
/// Tecto, em chars, de cada campo que o monitor do Gmail nos envia. O script
/// ja corta a 180, mas o script corre numa pagina remota: o lado nativo nao
/// pode confiar nesse corte e repete-o antes de guardar ou pintar.
const GMAIL_FIELD_MAX_CHARS: usize = 180;

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
/// Quanto ficam no ecra os avisos do Pomodoro: os dos comandos, e os do fim
/// de uma fase (mais tempo: quem estava concentrado pode nao estar a olhar).
const POMODORO_NOTICE_SECONDS: u64 = 3;
const POMODORO_PHASE_END_SECONDS: u64 = 8;
/// A ajuda do `tema:` com uma palavra desconhecida (omnibox e palette).
const THEME_COMMAND_HELP: &str = "Use tema:sistema, tema:claro ou tema:escuro.";
const GMAIL_TOAST_WIDTH: f64 = 390.0;
const GMAIL_TOAST_HEIGHT: f64 = 68.0;
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
const SEARCH_CARD_ARM: Duration = Duration::from_millis(600);
/// O pedido fixo do Traduzir, escrito pelo nativo: as tres IAs recebem isto,
/// uma linha em branco e o texto que o cartao pintou.
const TRANSLATE_PROMPT: &str = "Traduza para o português do Brasil (se o texto já estiver em português, traduza para o inglês):";

static GMAIL_TOAST_TEXT: Mutex<String> = Mutex::new(String::new());
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

/// As ferramentas da barra e da Home, na ordem em que aparecem (da esquerda
/// para a direita): pedidas pelo dono como botoes, ao lado dos servicos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Pomodoro,
    /// Zettelkasten: as notas vivem no painel do Ctrl+H.
    Notes,
    /// Respiracao guiada (metodo Wim Hof): o video no painel anonimo.
    Breath,
}

impl Tool {
    const ALL: [Tool; 3] = [Tool::Pomodoro, Tool::Notes, Tool::Breath];

    fn icon_slot(self) -> usize {
        match self {
            Self::Pomodoro => ICON_SLOT_POMODORO,
            Self::Notes => ICON_SLOT_NOTES,
            Self::Breath => ICON_SLOT_BREATH,
        }
    }

    /// O tomate tem cor propria; as outras duas marcas sao brancas e seguem
    /// o tema, como a videochamada e o envelope.
    fn icon_tint(self, theme: &Theme) -> Option<Rgb> {
        match self {
            Self::Pomodoro => None,
            Self::Notes | Self::Breath => Some(theme.fg),
        }
    }

    /// A dica: o que o clique FAZ, como as outras dicas da barra.
    fn tooltip(self) -> &'static str {
        match self {
            Self::Pomodoro => {
                "Pomodoro: foco e pausas (clique inicia/pausa; botão direito: opções)"
            }
            Self::Notes => "Notas (Zettelkasten) — Ctrl+Shift+Z cria nota da seleção",
            Self::Breath => "Respiração guiada — método Wim Hof (vídeo em modo anônimo)",
        }
    }
}

/// A dica de uma ferramenta, na barra e na Home. A do Pomodoro, com uma
/// sessao em curso, diz a fase, o que falta e os focos feitos
/// (`PomodoroController::hint`); parado, e nas outras duas, a fixa.
fn tool_hint(tool: Tool, pomodoro: Option<&str>) -> String {
    match (tool, pomodoro) {
        (Tool::Pomodoro, Some(session)) => session.to_string(),
        _ => tool.tooltip().to_string(),
    }
}

/// A dica de uma ferramenta em `now`, com o Pomodoro da app: e o que a Home
/// (`update_home_tool_hover`), a barra (`bar_hint`) e o refresco de cada
/// segundo (`pomodoro_changed`) mostram.
fn tool_hint_at(tool: Tool, pomodoro: &PomodoroController, now: Instant) -> String {
    let session = match tool {
        Tool::Pomodoro => pomodoro.hint(now),
        Tool::Notes | Tool::Breath => None,
    };
    tool_hint(tool, session.as_deref())
}

/// A dica de um alvo da barra, como `App::bar_tooltip_text` a mostra: as
/// ferramentas pela `tool_hint_at` (a do Pomodoro diz a sessao), o resto
/// pela `bar_tooltip_label`.
fn bar_hint(
    hit: BarHit,
    pomodoro: &PomodoroController,
    now: Instant,
    provider: &str,
    maximized: bool,
    tab_url: Option<&str>,
    group: Option<(&str, bool)>,
) -> Option<String> {
    if let BarHit::Tool(tool) = hit {
        return Some(tool_hint_at(tool, pomodoro, now));
    }
    bar_tooltip_label(hit, provider, maximized, tab_url, group)
}

/// Uma coluna do comparador. Generica na vista para os gates correrem sem
/// WebView (`note_read_view`); no app e a `WebView`.
struct ComparatorView<V = WebView> {
    webview: V,
    name: &'static str,
}

/// A fonte aberta ao lado (Split). Generica na vista como a
/// `ComparatorView`: quem decide se um Split privado pode ser lido recebe o
/// Split inteiro, com o `private` dele, e nao um booleano copiado a parte.
struct SplitView<V = WebView> {
    webview: V,
    source_index: usize,
    /// Identidade da aba que originou este Split. URL nao e identidade:
    /// a mesma fonte pode existir em dois grupos diferentes.
    context_id: Option<u64>,
    fullscreen: bool,
    private: bool,
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
    /// Por coluna, a aba em que o dono acabou de mexer (largou-a, juntou-a a
    /// um grupo, mandou ver o grupo dela, abriu-a pela lista "‹N"): a barra
    /// mostra-a mesmo longe das mais recentes. Nao vai para o disco.
    bar_focus: [Option<u64>; COMPARATOR_COLUMNS],
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
    match message {
        // Um STATIC devolve HTTRANSPARENT e os cliques iam para a pagina.
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
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
const AUX_POPUP_EX_STYLE: u32 = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
const AUX_POPUP_STYLE: u32 = WS_POPUP;

/// Para onde o cartao manda a resposta: o proxy do event loop no app, um
/// registo nos gates. Em caixa dupla: o `reference_data` da subclasse e um
/// ponteiro fino.
type SearchCardSink = Box<dyn Fn(UserEvent)>;

/// Os dois botoes do cartao: o de confirmar ("Mandar", "Traduzir") e o
/// Cancelar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchCardButton {
    Confirm,
    Cancel,
}

impl SearchCardButton {
    fn index(self) -> usize {
        match self {
            Self::Confirm => 0,
            Self::Cancel => 1,
        }
    }

    fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Confirm),
            1 => Some(Self::Cancel),
            _ => None,
        }
    }

    /// O rotulo no cartao do botao da barra que o pediu.
    fn label(self, intent: SearchIntent) -> &'static str {
        match (self, intent) {
            (Self::Confirm, SearchIntent::Ask) => "Mandar",
            (Self::Confirm, SearchIntent::Translate) => "Traduzir",
            (Self::Cancel, _) => "Cancelar",
        }
    }
}

/// O titulo do cartao, pelo botao da barra que o pediu.
fn search_card_title(intent: SearchIntent) -> &'static str {
    match intent {
        SearchIntent::Ask => "Mandar para as 3 IAs?",
        SearchIntent::Translate => "Traduzir nas 3 IAs?",
    }
}

/// O que as tres IAs recebem depois do clique em confirmar, a partir do que
/// o cartao pintou (`seen`): a pergunta tal e qual, ou o pedido fixo de
/// traducao, uma linha em branco e o texto. O pedido e sempre este, escrito
/// aqui: a pagina so escolhe o botao.
fn selection_prompt(intent: SearchIntent, seen: &str) -> String {
    match intent {
        SearchIntent::Ask => seen.to_string(),
        SearchIntent::Translate => format!("{TRANSLATE_PROMPT}\n\n{seen}"),
    }
}

/// O comando da omnibox que traduz nas tres IAs: e o que o Historico guarda
/// de um Traduzir, e o clique la refaz o pedido (`route_input`).
const TRANSLATE_COMMAND: &str = "traduzir:";

/// Uma comparacao nas tres IAs: o que elas recebem (`prompt`) e como fica no
/// Historico, na memoria e na sessao de pesquisa. Numa pergunta e tudo o
/// mesmo texto. No Traduzir as IAs recebem o pedido fixo, mas o nome e o do
/// texto de quem le ("Traduzir: <texto>") -- o pedido fixo a frente deixava
/// todas as traducoes com o mesmo titulo -- e o Historico guarda
/// `traduzir:<texto>`, que reabre refazendo o pedido (com o pedido inteiro,
/// um texto longo passava do tecto do painel e ja nao reabria).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompareRequest {
    /// O que as tres IAs recebem.
    prompt: String,
    /// O nome da sessao de pesquisa e da memoria.
    label: String,
    /// O que o Historico guarda e o clique la volta a abrir (`handle_input`).
    reopen: String,
}

impl CompareRequest {
    /// Uma pergunta: o mesmo texto para as IAs, o nome e o Historico.
    fn ask(query: String) -> Self {
        Self {
            label: query.clone(),
            reopen: format!("compare:{query}"),
            prompt: query,
        }
    }

    /// O Traduzir de `text` (`selection_prompt`).
    fn translate(text: &str) -> Self {
        Self {
            prompt: selection_prompt(SearchIntent::Translate, text),
            label: format!("Traduzir: {text}"),
            reopen: format!("{TRANSLATE_COMMAND}{text}"),
        }
    }

    /// O que o clique em confirmar no cartao manda: o texto que ele pintou,
    /// pelo botao da barra que o pediu.
    fn selection(intent: SearchIntent, seen: &str) -> Self {
        match intent {
            SearchIntent::Ask => Self::ask(seen.to_string()),
            SearchIntent::Translate => Self::translate(seen),
        }
    }
}

/// O que um `compare` deixa, sem janela: a sessao de pesquisa (com o nome
/// de `label`), a memoria da pergunta e a entrada do Historico.
fn compare_records(request: &CompareRequest) -> (ResearchSession, MemoryDocument, String) {
    let session = ResearchSession::new(request.prompt.clone()).titled(&request.label);
    let memory = MemoryDocument::new(
        MemoryKind::ResearchResult,
        MemorySourceKind::Note,
        format!("Pesquisa · {}", session.title),
        None,
        request.prompt.clone(),
    )
    .session(session.id.clone());
    (session, memory, request.reopen.clone())
}

/// O texto do cartao e texto simples: DrawTextW com DT_NOPREFIX (um "&" da
/// pagina e um "&", nao um sublinhado), quebra por palavras e, numa palavra
/// maior que a linha, por caracteres. Sem DT_END_ELLIPSIS: o corte e o de
/// `search_card_fit`, medido, nunca um que o GDI faca em silencio.
const SEARCH_CARD_TEXT_FORMAT: u32 = DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX;

/// Onde fica cada coisa no cartao, em pixeis do cliente. Uma so funcao para o
/// desenho e o clique concordarem sempre.
struct SearchCardLayout {
    title: RECT,
    /// A caixa do texto e, dentro dela, o texto.
    quote: RECT,
    text: RECT,
    /// A conta do que ficou de fora, a esquerda dos botoes.
    note: RECT,
    search: RECT,
    cancel: RECT,
}

fn search_card_scale(client: &RECT) -> f64 {
    ((client.bottom - client.top) as f64 / SEARCH_CARD_HEIGHT).max(1.0)
}

fn search_card_layout(client: &RECT, scale: f64) -> SearchCardLayout {
    let px = |value: f64| (value * scale).round() as i32;
    let pad = px(24.0);
    let right = client.right - pad;
    let bottom = client.bottom - px(20.0);
    let top = bottom - px(40.0);
    let cancel = RECT {
        left: right - px(124.0),
        top,
        right,
        bottom,
    };
    let search = RECT {
        left: cancel.left - px(12.0) - px(140.0),
        top,
        right: cancel.left - px(12.0),
        bottom,
    };
    let title = RECT {
        left: client.left + pad,
        top: client.top + px(16.0),
        right,
        bottom: client.top + px(50.0),
    };
    let quote = RECT {
        left: title.left,
        top: title.bottom + px(6.0),
        right,
        bottom: top - px(14.0),
    };
    let text = RECT {
        left: quote.left + px(12.0),
        top: quote.top + px(8.0),
        right: quote.right - px(12.0),
        bottom: quote.bottom - px(8.0),
    };
    let note = RECT {
        left: title.left,
        top,
        right: search.left - px(12.0),
        bottom,
    };
    SearchCardLayout {
        title,
        quote,
        text,
        note,
        search,
        cancel,
    }
}

/// O botao do cartao debaixo de (x, y); bordas semiabertas, como o resto da UI.
fn search_card_hit(client: &RECT, scale: f64, x: i32, y: i32) -> Option<SearchCardButton> {
    let layout = search_card_layout(client, scale);
    let inside = |rect: &RECT| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
    if inside(&layout.search) {
        Some(SearchCardButton::Confirm)
    } else if inside(&layout.cancel) {
        Some(SearchCardButton::Cancel)
    } else {
        None
    }
}

/// O clique que o cartao aceita: o botao desceu E subiu no mesmo botao, com o
/// cartao a segurar o rato desde que desceu. Arrastar de fora para cima do
/// confirmar, ou premir e sair, nao responde nada.
fn search_card_release(
    pressed: Option<usize>,
    captured: bool,
    client: &RECT,
    x: i32,
    y: i32,
) -> Option<SearchCardButton> {
    if !captured {
        return None;
    }
    let released = search_card_hit(client, search_card_scale(client), x, y);
    native_release_matches(pressed, released.map(SearchCardButton::index))
        .and_then(SearchCardButton::from_index)
}

/// Caracteres que o cartao pintaria como nada (ou que mudam a ordem do que
/// pinta) e que as IAs leem na mesma: os Default_Ignorable_Code_Point do
/// Unicode -- tags U+E0000.. ("ASCII smuggling"), seletores de variacao,
/// ZWSP/ZWJ, marcas e controlos bidi, soft hyphen, BOM, preenchimentos
/// Hangul --, o braille vazio, as ancoras de anotacao e o U+FFFC, a area
/// privada e os nao-caracteres. A pagina escolhe o texto; nao escolhe mandar
/// as IAs uma coisa que o cartao nao mostra.
fn invisible_in_card(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{E000}'..='\u{F8FF}'
            | '\u{FDD0}'..='\u{FDEF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFFC}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
            | '\u{F0000}'..='\u{10FFFF}'
    ) || (c as u32 & 0xFFFE) == 0xFFFE
}

/// A pergunta que o "Mandar para IA" (ou o texto que o "Traduzir") leva, tal
/// como o cartao a mostra: sem os
/// caracteres invisiveis, com quebras de linha, tabs e outros espacos ou
/// controlos reduzidos a um espaco, aparada. E ESTE texto -- nao o que a
/// pagina mandou -- que o cartao pinta e que a confirmacao leva.
fn selection_question(text: &str) -> String {
    let mut question = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if invisible_in_card(c) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            gap = !question.is_empty();
            continue;
        }
        if gap {
            question.push(' ');
            gap = false;
        }
        question.push(c);
    }
    question
}

/// Os primeiros `shown` caracteres de `text`, sem o espaco do fim: o que o
/// cartao pintou e, portanto, tudo o que um clique em confirmar leva.
fn search_card_shown(text: &str, shown: usize) -> &str {
    let end = text
        .char_indices()
        .nth(shown)
        .map_or(text.len(), |(at, _)| at);
    text[..end].trim_end()
}

/// O que se pinta na caixa: o texto inteiro, ou o inicio que coube e "…".
fn search_card_body(text: &str, shown: usize) -> String {
    let part = search_card_shown(text, shown);
    if part == text.trim_end() {
        part.to_string()
    } else {
        format!("{part}…")
    }
}

/// A conta, debaixo da caixa, do que nao coube (e por isso nao vai).
fn search_card_left_out(chars: usize) -> String {
    if chars == 1 {
        "+1 caractere fica de fora".to_string()
    } else {
        format!("+{chars} caracteres ficam de fora")
    }
}

/// Quantos caracteres de `text` cabem na caixa `width` x `height`, medidos
/// com DT_CALCRECT no `hdc` (com a fonte do texto ja escolhida) e o mesmo
/// formato do desenho: todos, se cabem; senao o maior inicio que cabe com o
/// "…" (cortado no ultimo espaco, se estiver perto). Uma linha mais larga que
/// a caixa (uma palavra que o GDI nao partisse) conta como nao caber.
unsafe fn search_card_fit(
    hdc: *mut core::ffi::c_void,
    text: &str,
    width: i32,
    height: i32,
) -> usize {
    let fits = |shown: usize| {
        let wide: Vec<u16> = search_card_body(text, shown).encode_utf16().collect();
        if wide.is_empty() {
            return true;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: 0,
        };
        let height_used = DrawTextW(
            hdc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            SEARCH_CARD_TEXT_FORMAT | DT_CALCRECT,
        );
        height_used > 0 && rect.right - rect.left <= width && rect.bottom - rect.top <= height
    };
    // Mede-se de inicios cada vez maiores, nunca o texto todo de uma vez: o
    // GDI parte palavras enormes (e CJK) devagar, e 2000 caracteres assim
    // custavam centenas de ms numa pintura. fits(low) e !fits(high); o "…"
    // sozinho cabe em qualquer cartao.
    let total = text.chars().count();
    let (mut low, mut high) = (0, total.min(128));
    while fits(high) {
        if high == total {
            return total;
        }
        low = high;
        high = (high * 2).min(total);
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    let prefix: Vec<char> = text.chars().take(low).collect();
    match prefix.iter().rposition(|c| *c == ' ') {
        Some(space) if low - space <= 24 => space,
        _ => low,
    }
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
        // A linha passa pela redacao antes do disco: se alguma um dia levar
        // a chave do Gemini Live, sai com um marcador no lugar dela.
        let line = event.to_string();
        let _ = writeln!(file, "{elapsed_ms:>8} ms  {}", redact_debug_secrets(&line));
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

/// A dica do icone do servico aberto: minimizado, diz que volta ao clique (e
/// se continua a tocar); aberto, que fecha.
fn service_icon_hint(label: &str, badge: Option<ServiceBadge>) -> String {
    match badge {
        Some(ServiceBadge::Playing) => {
            format!("{label} minimizado, a tocar · clique para voltar ao painel")
        }
        Some(ServiceBadge::Minimized) => {
            format!("{label} minimizado · clique para voltar ao painel")
        }
        None => format!("{label} aberto ao lado · clique para fechar"),
    }
}

/// A dica que o painel de servicos aberto (`service`, no modo `badge`) da
/// ao alvo `hit` da barra: a faixa dele e o botao que o abriu -- o icone do
/// servico ou, na Respiracao, o botao dela nas ferramentas, onde o ponto de
/// minimizado fica (`service_icon_rect`) e que o clique restaura ou fecha
/// (`open_service_panel`). `None`: o alvo nao e do painel, e a dica e a de
/// sempre.
fn service_panel_hint(
    hit: BarHit,
    service: Service,
    badge: Option<ServiceBadge>,
) -> Option<String> {
    match hit {
        BarHit::ServiceStrip(button) => Some(button.hint(service.label())),
        BarHit::Service(hit_service) if hit_service == service => {
            Some(service_icon_hint(service.label(), badge))
        }
        BarHit::Tool(Tool::Breath) if service == Service::Breath => {
            Some(service_icon_hint(service.label(), badge))
        }
        _ => None,
    }
}

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

/// Onde comecam os paineis da direita (Ctrl+H, servicos, Gemini Live), em
/// pixels logicos. No comparador, abaixo da barra. Na Home, abaixo da fila dos
/// botoes da janela: com o painel a comecar no topo, a WebView dele tapava a
/// zona que os acorda, a janela principal nunca via o rato la e minimizar,
/// maximizar e fechar ficavam impossiveis de encontrar com o painel aberto.
fn right_panel_top(surface: Surface) -> f64 {
    match surface {
        Surface::Comparator => COMPARATOR_CHROME_HEIGHT,
        Surface::Home => TITLE_TAB_HEIGHT,
        _ => 0.0,
    }
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
fn hover_tooltip(window: HWND, text: &str) -> u64 {
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
fn refresh_hint_text(text: &str) {
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

/// Som curto do sistema no fim de uma fase do Pomodoro (o "Asterisco" do
/// esquema de sons do Windows; sem som configurado, nada).
fn pomodoro_sound() {
    use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;
    unsafe {
        MessageBeep(MB_ICONASTERISK);
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
/// (pode ser nula). `checked` marca-o como a escolha em vigor.
unsafe fn append_swatch_item(
    menu: *mut core::ffi::c_void,
    id: usize,
    label: &[u16],
    swatch: *mut core::ffi::c_void,
    checked: bool,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, InsertMenuItemW, MENUITEMINFOW, MFS_CHECKED, MFT_RADIOCHECK, MFT_STRING,
        MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING,
    };
    let info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID
            | MIIM_STRING
            | MIIM_FTYPE
            | MIIM_STATE
            | if swatch.is_null() { 0 } else { MIIM_BITMAP },
        fType: MFT_STRING | if checked { MFT_RADIOCHECK } else { 0 },
        fState: if checked { MFS_CHECKED } else { 0 },
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

/// Menu do botao direito do Pomodoro no cursor, feito das linhas de
/// `PomodoroController::menu_items` (o modelo e o `pick_theme_from_menu`).
/// Devolve o id escolhido; 0 se o menu fechou sem escolha.
fn pick_pomodoro_from_menu(hwnd: HWND, items: &[PomodoroMenuItem]) -> usize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, MF_CHECKED, MF_GRAYED, SetForegroundWindow,
    };
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        for item in items {
            match *item {
                PomodoroMenuItem::Separator => {
                    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                }
                PomodoroMenuItem::Command {
                    id,
                    label,
                    enabled,
                    checked,
                    ..
                } => {
                    let mut flags = MF_STRING;
                    if checked {
                        flags |= MF_CHECKED;
                    }
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    let text: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
                    AppendMenuW(menu, flags, id, text.as_ptr());
                }
            }
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
        usize::try_from(picked).unwrap_or(0)
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

/// Ctrl+O na omnibox da Home: o diálogo "Adicionar livros EPUB". Só nativo
/// (a janela principal responde o mesmo em `main_window_shortcut`); o mapa de
/// teclas das páginas (`NEURALIA_KEYMAP_SCRIPT`) não o conhece, para não
/// nascer uma ação IPC nova no canal das páginas remotas. Nas páginas de
/// livros quem o trata é a própria página, pelo IPC fechado delas.
fn omnibox_opens_epub_dialog(virtual_key: u32, ctrl: bool, shift: bool) -> bool {
    virtual_key == u32::from(b'O') && ctrl && !shift
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
            // Ctrl+O: adicionar e abrir livros EPUB.
            key if omnibox_opens_epub_dialog(key, ctrl, shift) => {
                let _ = proxy.send_event(UserEvent::OpenEpubDialog);
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
            // Ctrl+Shift+Z (Z = 0x5A): nota nova no painel. Na omnibox nao
            // ha pagina com selecao; o Ctrl+Z sozinho continua a desfazer.
            0x5A if ctrl && shift => {
                let _ = proxy.send_event(UserEvent::NewNote);
                return 0;
            }
            _ => {}
        }
    }
    // O Ctrl+Shift+Z acima ja foi tratado: o carater 0x1A que o
    // TranslateMessage gera a seguir seria o "desfazer" do EDIT.
    if message == WM_CHAR
        && wparam == 0x1A
        && (GetAsyncKeyState(VK_SHIFT as i32) as u16 & 0x8000) != 0
    {
        return 0;
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
    /// Botao esquerdo em baixo sobre uma aba, o x dela ou a pilula de um
    /// grupo: o que acontece so se decide ao largar (ou ao arrastar).
    tab_press: Option<TabPress>,
    /// Numero do ultimo gesto na fila de abas; o proximo e este mais um.
    tab_gesture_count: u64,
    /// Ctrl/Shift/Alt no teclado da janela principal (a barra com o foco).
    modifiers: winit::keyboard::ModifiersState,
    /// O rato esta em cima do "Ir" da Home: pinta-se em degradê.
    home_go_hover: bool,
    exit_button: Option<HWND>,
    home_button: Option<HWND>,
    caption_buttons: Option<HWND>,
    /// Na Home os botoes da janela so se veem com o rato perto deles.
    caption_reveal: CaptionReveal,
    splitters: [Option<HWND>; COMPARATOR_COLUMNS - 1],
    /// Partilhado com o menu do botao direito de cada coluna: o WebView2 monta
    /// esse menu num callback fora do `&mut App`, e o rotulo tem de dizer o
    /// estado de AGORA, mudado pelo Ctrl+R, pela pergunta ou pelo proprio menu.
    auto_scroll: SharedFlag,
    auto_scroll_answered: bool,
    auto_scroll_token: u64,
    zoom: f64,
    /// O visualizador de PDF nao aceita script do host: rola-se por tecla.
    reading_pdf: bool,
    splash: Option<HWND>,
    splash_board: SplashBoard,
    gmail_toast: Option<HWND>,
    gmail_toast_token: u64,
    /// O pedido da barra (Mandar para IA, Traduzir) a espera do clique no
    /// cartao nativo.
    search_card: SearchCard,
    /// Os "Salvar nota" gravados ha menos de 2 s: o mesmo texto nao e outra
    /// nota.
    bar_notes: BarNoteGuard,
    /// O mesmo para o Ctrl+Shift+Z (a tecla presa repete o keydown).
    shortcut_notes: BarNoteGuard,
    search_card_popup: Option<HWND>,
    search_card_sink: Box<SearchCardSink>,
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
    /// Painel lateral do historico inteligente e das notas (Ctrl+H). Sai por
    /// `close_side_panel`, que grava primeiro o que o editor tinha por
    /// salvar e devolve o teclado (`side_panel::SidePanel::dismiss`);
    /// largado de outra forma, o `Drop` dele ainda grava o rascunho.
    side_panel: side_panel::SidePanel<WebView, ZettelWorker>,
    /// A consulta de memoria que alimenta as sugestoes do painel.
    panel_suggestion_query: Option<String>,
    /// Servico aberto no painel lateral (WhatsApp, Meet, YouTube, Gmail e o
    /// video da respiracao, este em InPrivate).
    service_panel: Option<ServicePanel>,
    /// Numero do ultimo painel de servicos aberto: os avisos do WebView2 de
    /// um painel ja fechado chegam com o numero dele e caem.
    service_generation: u64,
    /// A tela cheia da janela pedida pelo painel de servicos, e se foi ele
    /// que a pos (so entao a devolve ao sair).
    panel_window_fullscreen: PanelWindowFullscreen,
    /// De que coluna e a dica centrada pedida por um controlo injetado (o
    /// "none" atrasado de uma coluna so apaga a dica dela).
    column_hint: Option<ColumnHintOwner>,
    /// Larguras escolhidas para os paineis da direita, gravadas em
    /// `<data_dir>/panel-width.json`.
    panel_widths: PanelWidths,
    /// A pega de arrastar a borda esquerda do painel aberto.
    panel_handle: Option<HWND>,
    /// Gravacao das abas e grupos do comparador em `tabs.json`. Aberta no
    /// arranque: a primeira janela do NeuralIA fica com o `tabs.lock`.
    tab_session: TabPersistence,
    /// Ferramentas: o botao da Home sob o rato (a barra usa `bar_hover`).
    home_tool_hover: Option<Tool>,
    /// Notas (Zettelkasten) em `<data_dir>/zettel`, lidas e gravadas fora do
    /// event loop.
    notes: ZettelWorker,
    /// O endereco verdadeiro da pagina da WebView unica quando o dela nao o
    /// e: o artigo do Leitor (o HTML e local) e o PDF (o visualizador e
    /// nosso). E a fonte das notas feitas ali.
    page_source: Option<String>,
    /// O Pomodoro do botao da barra e da Home, com a cadeia de tiques viva.
    /// As duracoes vivem em `<data_dir>/pomodoro`.
    pomodoro: PomodoroController,
    /// Biblioteca de livros (worker) e servidor da origem `neuralia-epub`.
    /// Nascem na primeira vez que se abre um livro e vivem com a app.
    epub: Option<EpubRuntime>,
    /// Arquivos largados na janela neste lote de eventos. O winit entrega um
    /// `DroppedFile` por arquivo; o lote segue inteiro no `about_to_wait`.
    pending_drops: Vec<PathBuf>,
    /// Painel do Gemini Live, com o estado do olho da barra. Existir e estar
    /// ligado: fecha-lo desliga tudo.
    live_panel: LivePanel<WebView>,
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
            gmail_toast: None,
            gmail_toast_token: 0,
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
            self.show_unseen_phase_end();
            self.resume_home_animation();
            // Os popups owned reaparecem com o dono, mas a geometria pode ter
            // mudado enquanto estivemos fora (outro ecra, outro DPI, outra
            // maximizacao), por isso recalcula-se em vez de se confiar nela.
            self.sync_comparator_splitters();
            self.sync_exit_button();
            self.sync_caption_buttons();
            self.sync_panel_handle();
            return;
        }
        // Sem foco nao ha o que arrastar nem de onde sair: as auxiliares que
        // so servem o rato saem da frente ate a janela voltar.
        self.hide_comparator_splitters();
        self.hide_exit_button();
        self.hide_panel_handle();
    }

    /// A janela ficou inteiramente tapada (ou deixou de estar). Enquanto esta
    /// tapada nao se pinta nada.
    fn on_occluded_changed(&mut self, occluded: bool) {
        if self.home_occluded == occluded {
            return;
        }
        self.home_occluded = occluded;
        if !occluded {
            // Restaurada da barra de tarefas: o foco pode ir direto para a
            // WebView e o `Focused` da janela nunca chegar.
            self.show_unseen_phase_end();
            self.resume_home_animation();
        }
    }

    /// Um fim de fase do Pomodoro que a janela nao viu (estava minimizada ou
    /// atras de outra) aparece quando ela volta -- uma vez.
    fn show_unseen_phase_end(&mut self) {
        if let Some(message) = self.pomodoro.window_back() {
            self.show_background_splash(message, POMODORO_PHASE_END_SECONDS);
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
        //
        // As abas e os grupos morrem com o comparador: gravam-se antes, para
        // a proxima pesquisa (ou o proximo arranque) os trazer de volta.
        let _ = self.save_tab_session();
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
        // Os paineis da direita tambem sao superficies web e nao sobrevivem a
        // esta saida. O do Gemini Live em especial: so escondido pelo
        // `hide_orphaned_wry_hosts` la em baixo, continuava vivo a mandar a
        // tela, a camera e o microfone ao Google, sem o olho vermelho (a barra
        // so se pinta no comparador) e sem o botao Desligar -- bastava um erro
        // nativo (`show_native_error`) ou um link para a Web completa.
        self.close_live_panel();
        self.close_service_panel();
        // O do Ctrl+H pela saida unica: o texto de uma nota a meio vai para
        // o disco antes de a pagina sair (gate
        // `every_way_out_of_the_side_panel_saves_the_note_being_typed_once`).
        // `SurfaceChange` nao mexe no teclado: esta troca trata dele.
        self.close_side_panel(PanelExit::SurfaceChange);
        // Sem painel: a pega some e o gancho da roda sai.
        self.after_panel_change();

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
        self.page_source = None;
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
        self.caption_reveal.reset();
        self.close_side_panel(PanelExit::Home);
        self.close_service_panel();
        self.close_live_panel();
        self.next_generation();
        self.surface = Surface::Home;

        // Home é uma fronteira de ciclo de vida real. Destruir os controllers
        // aqui garante que nenhum host WRY_WEBVIEW sobreviva oculto/reparentado
        // entre pesquisas. Reuso dentro do próprio comparador continua possível,
        // mas sair para Home sempre encerra as superfícies web.
        self.destroy_web_surfaces();
        self.schedule_home_restoration();

        self.bar_hover = None;
        self.forget_tab_gesture();
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
            "Apagar TODO o histórico e a memória local da NeuralIA?\n\nNos livros, some o registro de quando cada um foi aberto; a posição de leitura e os marcadores ficam.\n\nIsto não pode ser desfeito.",
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
            InputRoute::Theme(None) => self.show_splash(THEME_COMMAND_HELP.to_string(), 3),
            InputRoute::Pomodoro(Some(command)) => self.pomodoro_command(command),
            InputRoute::Pomodoro(None) => {
                self.show_splash(POMODORO_COMMAND_HELP.to_string(), 4);
            }
            InputRoute::ResearchCompare => self.compare_current_research(),
            InputRoute::ResearchSynthesize => self.synthesize_current_research(),
            InputRoute::ResearchExport => self.export_current_research(),
            InputRoute::Translate(Some(text)) => self.compare(CompareRequest::translate(&text)),
            InputRoute::Translate(None) => {
                self.show_splash(TRANSLATE_COMMAND_HELP.to_string(), 3);
            }
            InputRoute::Library => self.open_library(),
            InputRoute::OpenEpub(None) => self.open_epub_dialog(true),
            InputRoute::OpenEpub(Some(path)) => self.open_epub(path),
            InputRoute::Intent => match parse_intent(&input) {
                Ok(Intent::Home) => self.show_home(),
                Ok(Intent::Ask(query)) => self.ask(query),
                Ok(Intent::Compare(query)) => self.compare(CompareRequest::ask(query)),
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

    /// O cartao "Mandar para as 3 IAs?" / "Traduzir nas 3 IAs?": a decisao e
    /// a de `SearchCard::step`
    /// (pura, testada) e o efeito o de `apply_search_card`; aqui so se junta
    /// o relogio. O `compare` so corre para um `Confirmed`.
    fn search_card_event(&mut self, input: SearchCardInput) {
        let outcome = self.search_card.step(input, Instant::now());
        apply_search_card(self, outcome);
    }

    /// Destino normal de uma pergunta: a mesma consulta segue em simultaneo
    /// para o Google AI Mode, o ChatGPT e o Claude, lado a lado. O nome e a
    /// entrada do Historico sao os de `compare_records`.
    fn compare(&mut self, request: CompareRequest) {
        self.next_generation();

        let (session, question_memory, reopen) = compare_records(&request);
        self.memory.capture(question_memory);
        self.memory.save_session(session.clone());
        self.current_research = Some(session);

        self.record(HistoryKind::Ask, reopen, "comparator-3col".to_string());
        self.open_comparator(&request.prompt);
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
    fn ensure_epub(&mut self) -> bool {
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
    fn open_library(&mut self) {
        self.open_epub_page(library_url());
    }

    /// Acrescenta o EPUB à biblioteca (numa thread própria: um livro grande
    /// pode demorar) e abre-o no leitor quando estiver lá.
    fn open_epub(&mut self, path: PathBuf) {
        self.submit_epub_job(EpubJob::Add {
            paths: vec![path],
            open: true,
        });
    }

    fn submit_epub_job(&mut self, job: EpubJob) {
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
    fn route_dropped_files(&mut self, paths: Vec<PathBuf>) {
        if let Some(job) = epub_drop_job(paths) {
            self.submit_epub_job(job);
        }
    }

    /// Ctrl+O e `epub:`: o diálogo "Abrir" do Windows, só `*.epub`.
    fn open_epub_dialog(&mut self, open: bool) {
        let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        let paths = pick_epub_files(owner);
        if !paths.is_empty() {
            self.submit_epub_job(EpubJob::Add { paths, open });
        }
    }

    fn open_epub_reader(&mut self, id: &str) {
        if let Some(url) = reader_url(id) {
            self.open_epub_page(url);
        }
    }

    fn open_epub_page(&mut self, url: String) {
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
    fn epub_webview_builder(&self, runtime: &EpubRuntime) -> WebViewBuilder<'static> {
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

    fn handle_epub_notice(&mut self, notice: EpubNotice) {
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

    fn handle_epub_ui(&mut self, request: EpubUiRequest) {
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
    fn reader_init_script(capability: &str) -> String {
        bind_page_script(
            &format!("{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}\n{READ_ALOUD_SCRIPT}"),
            capability,
            false,
        )
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
                self.page_source = Some(article.source_url.clone());
                self.begin_reading_session(false);
            }
            Err(error) => {
                self.show_native_error(format!("WebView2 não pôde exibir o Reader: {error}"));
            }
        }
    }

    fn open_comparator(&mut self, query: &str) {
        self.close_side_panel(PanelExit::NewSearch);
        self.close_service_panel();
        self.close_live_panel();
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
                comparator.bar_focus = [None; COMPARATOR_COLUMNS];
                // A pesquisa nova nao apaga abas nem grupos: passa a ser o
                // contexto ativo de cada coluna, e a fonte que estava aberta
                // ao lado fica na barra, por carregar. Quem decide o que
                // continua aberto e `start_new_search`; aqui so se obedece.
                let mut active = [None; COMPARATOR_COLUMNS];
                if let Some((column, id)) = persisted_active(comparator_split_key(comparator)) {
                    active[column] = Some(id);
                }
                let ComparatorState {
                    contexts,
                    groups,
                    next_context_id,
                    next_group_id,
                    split,
                    ..
                } = comparator;
                start_new_search(
                    contexts,
                    groups,
                    next_context_id,
                    next_group_id,
                    &mut active,
                );
                let split_stays = split.as_ref().is_some_and(|split| {
                    persisted_active(Some((split.source_index, split.context_id, split.private)))
                        .is_some_and(|(column, id)| active[column] == Some(id))
                });
                if !split_stays && let Some(split) = split.take() {
                    let _ = split.webview.set_visible(false);
                    let _ = split.webview.focus_parent();
                    drop(split);
                }
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

        // As abas e os grupos da sessao anterior voltam a barra. Nenhuma
        // carrega agora: a pergunta e o contexto ativo de cada coluna, e cada
        // aba restaurada so abre quando for escolhida. O que esta no disco e
        // o que acabou de ser lido: nada a regravar ate alguma aba mudar.
        let (restored, tabs_notice) = self.tab_session.restore();

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
                    self.install_context_menu(&wv, WebViewHost::Column(i));
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
            contexts: restored.contexts,
            groups: restored.groups,
            next_group_id: restored.next_group_id,
            next_context_id: restored.next_context_id,
            bar_focus: [None; COMPARATOR_COLUMNS],
            panel_width: 0.0,
        });
        self.activate_comparator(true);
        if let Some(notice) = tabs_notice {
            self.show_splash(notice, 5);
        }
    }

    /// Corre depois de cada lote de eventos: se as abas ou os grupos mudaram,
    /// agenda a gravacao. Olhar aqui, e nao em cada sitio que mexe nas abas,
    /// apanha tambem os gestos da barra que ainda vao nascer.
    fn observe_tab_session(&mut self) {
        let Some(comp) = &self.comparator else {
            return;
        };
        if let Some(token) =
            self.tab_session
                .observe(&comp.contexts, &comp.groups, comparator_split_key(comp))
        {
            self.timers
                .after(TAB_SESSION_DEBOUNCE, UserEvent::SaveTabSession(token));
        }
    }

    /// Grava ja, se o disco estiver atrasado em relacao a barra: antes de o
    /// comparador ser destruido e ao sair (`TabPersistence::save_now`).
    fn save_tab_session(&mut self) -> std::io::Result<TabSave> {
        let Some(comp) = &mut self.comparator else {
            return Ok(TabSave::Unchanged);
        };
        let split = comparator_split_key(comp);
        self.tab_session
            .save_now(&mut comp.contexts, &mut comp.groups, split)
    }

    /// O `SaveTabSession(token)` do fim do atraso: grava se ainda for o
    /// ultimo agendado (`TabPersistence::save_due`) e diz ao dono o que correu
    /// mal ou o que mudou.
    fn save_due_tab_session(&mut self, token: u64) {
        let result = self.comparator.as_mut().and_then(|comp| {
            let split = comparator_split_key(comp);
            self.tab_session
                .save_due(token, &mut comp.contexts, &mut comp.groups, split)
        });
        if let Some(notice) = result.as_ref().and_then(tab_save_notice) {
            self.request_redraw();
            self.show_splash(notice, 4);
        }
    }

    /// Parte de "Apagar historico": o modelo vivo, o ficheiro e as copias,
    /// tudo de uma vez (`TabPersistence::forget`).
    fn forget_tab_session(&mut self) {
        let result = match &mut self.comparator {
            Some(comp) => {
                let split = comparator_split_key(comp);
                self.tab_session
                    .forget(&mut comp.contexts, &mut comp.groups, split)
            }
            None => self.tab_session.forget(
                &mut std::array::from_fn(|_| Vec::new()),
                &mut std::array::from_fn(|_| Vec::new()),
                None,
            ),
        };
        self.request_redraw();
        if let Err(error) = result {
            self.show_splash(
                format!("Não foi possível apagar as abas salvas: {error}"),
                4,
            );
        }
    }

    fn activate_comparator(&mut self, sync_remote_buttons: bool) {
        self.bar_hover = None;
        self.forget_tab_gesture();
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
            self.forget_tab_gesture();
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
        self.forget_tab_gesture();
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
        self.forget_tab_gesture();
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
        let logical_w = comparator_logical_width(size.width as f64 / scale, comp.panel_width);
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
            IpcAction::Hint { col, hint } if col == col_index => {
                Some(UserEvent::ColumnHint { col, hint })
            }
            // Ctrl+Shift+Z ou Salvar nota: a selecao e lida DESTA coluna,
            // pelo lado nativo.
            IpcAction::Note { via } => Some(UserEvent::NoteRequested {
                target: Some(PageTarget::Column(col_index)),
                via,
            }),
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
        let [prelude, keymap, auto_submit, inject] =
            comparator_init_scripts(col_index, col_name, &capability);

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

    /// A unica porta para mexer no menu do botao direito de uma WebView: cada
    /// uma que o comparador constroi passa aqui com o que e, e so as colunas
    /// das IAs ganham o item de rolagem. Um runtime WebView2 sem o evento
    /// ContextMenuRequested deixa a coluna com o menu nativo e fica no log.
    fn install_context_menu(&self, webview: &WebView, host: WebViewHost) {
        let missing = install_column_menu(host, |col_index| {
            register_column_context_menu(
                webview,
                col_index,
                self.auto_scroll.clone(),
                self.proxy.clone(),
            )
        });
        if let Some(line) = missing {
            debug_log(format_args!("{line}"));
        }
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
            // O Ctrl+Shift+Z no Split privado nao faz notas: a pagina nem
            // chega a ser lida. O "Salvar nota" da barra e um pedido
            // explicito de quem le, e grava (so nas notas; o aviso diz que
            // foi no modo privado).
            IpcAction::Note {
                via: NoteVia::Shortcut,
            } if private => Some(UserEvent::NoteRefusedPrivate),
            IpcAction::Note { via } => Some(UserEvent::NoteRequested {
                target: Some(PageTarget::Split),
                via,
            }),
            // Defesa em profundidade: a barra de um painel privado nem mostra
            // o "Mandar para IA" nem o "Traduzir", e mesmo que uma mensagem
            // chegasse o texto nao pode sair para o comparador (historico e
            // memoria).
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

    /// O WebView do Split: o `WebViewBuilder` do tema, configurado por
    /// `configure_split_webview` com o `SplitBuild` que `split_open_plan`
    /// decidiu. Nada mais se lhe acrescenta aqui.
    fn split_webview_builder(&self, build: &SplitBuild) -> WebViewBuilder<'static> {
        let proxy = self.proxy.clone();
        configure_split_webview(themed_webview_builder(), build, move |event| {
            let _ = proxy.send_event(event);
        })
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
        self.open_split_opened_by(
            source_index,
            url,
            allow_local,
            private,
            existing_context_id,
            None,
        )
    }

    /// Um link que a fonte aberta ao lado mandou abrir noutra aba: a aba nova
    /// nasce no grupo da aba de onde saiu, como no Chrome.
    fn open_split_from_split(&mut self, source_index: usize, url: String) {
        let opener = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .filter(|split| split.source_index == source_index && !split.private)
            .and_then(|split| split.context_id);
        let _ = self.open_split_opened_by(source_index, url, false, false, None, opener);
    }

    /// `opener`: a aba de onde o link saiu, quando saiu de uma aba. Se ela
    /// estiver num grupo, a aba nova entra no fim do troco desse grupo.
    fn open_split_opened_by(
        &mut self,
        source_index: usize,
        url: String,
        allow_local: bool,
        private: bool,
        existing_context_id: Option<u64>,
        opener: Option<u64>,
    ) -> bool {
        let column_name = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(source_index))
            .map(|view| view.name);
        // Tudo o que depende do `private` sai daqui (ver `split_open_plan`).
        let build = match split_open_plan(
            self.surface,
            column_name,
            source_index,
            url,
            allow_local,
            private,
            remote_capability,
        ) {
            SplitOpenPlan::Web(url) => {
                self.web(url);
                return true;
            }
            SplitOpenPlan::Ignore => return false,
            SplitOpenPlan::Refuse { message, seconds } => {
                self.show_splash(message.to_string(), seconds);
                return false;
            }
            SplitOpenPlan::Build(build) => build,
        };
        // Memoria, sessao, aba de contexto e o SplitView leem o mesmo valor
        // que o builder recebeu.
        let private = build.incognito;
        let valid = build.url.clone();
        let source_name = build.source_name;

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
            .split_webview_builder(&build)
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
                self.install_context_menu(&webview, WebViewHost::Split(source_index));
                let mut lost_grouped = 0usize;
                if let Some(comp) = &mut self.comparator {
                    let ComparatorState {
                        contexts,
                        groups,
                        next_context_id,
                        ..
                    } = comp;
                    let grouped_before = grouped_tab_ids(&contexts[source_index]);
                    let context_id = record_split_context(
                        &mut contexts[source_index],
                        &mut groups[source_index],
                        next_context_id,
                        valid.to_string(),
                        private,
                        existing_context_id,
                        opener,
                    );
                    lost_grouped = lost_grouped_tabs(&grouped_before, &contexts[source_index]);
                    // A aba que se abriu e aquela para onde se olha: nasce a
                    // meio da fila (no fim do grupo de quem a abriu) e fica
                    // a vista.
                    if context_id.is_some() {
                        comp.bar_focus[source_index] = context_id;
                    }
                    comp.split = Some(SplitView {
                        webview,
                        source_index,
                        context_id,
                        fullscreen: false,
                        private,
                    });
                }
                if let Some(notice) = lost_grouped_notice(lost_grouped) {
                    self.show_splash(notice, 5);
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
            PaletteRoute::Pomodoro(Some(command)) => self.pomodoro_command(command),
            PaletteRoute::Pomodoro(None) => {
                self.show_splash(POMODORO_COMMAND_HELP.to_string(), 4);
            }
            PaletteRoute::Theme(Some(choice)) => self.choose_theme(choice),
            PaletteRoute::Theme(None) => self.show_splash(THEME_COMMAND_HELP.to_string(), 3),
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
        // O F8 responde a pergunta que estiver a vista.
        self.splash_board.answered();
        self.auto_scroll_answered = true;
        let on = self.auto_scroll.toggle();
        self.auto_scroll_token = self.auto_scroll_token.wrapping_add(1);

        if on {
            self.schedule_auto_scroll();
        }

        // A mensagem e a do meio da janela, como as outras dicas: o aviso
        // dentro da pagina ficava no fundo e so aparecia nas colunas.
        self.show_splash(auto_scroll_message(on), 3);
        self.request_redraw();
    }

    /// Aviso flutuante, centrado na janela, que se apaga sozinho: a resposta
    /// a um gesto do utilizador (`SplashKind::Notice`).
    fn show_splash(&mut self, text: String, seconds: u64) {
        if let Some(frame) = self.splash_board.show(text, seconds, SplashKind::Notice) {
            self.present_splash(frame);
        }
    }

    /// Aviso que chega sozinho, sem gesto nenhum (o fim de uma fase do
    /// Pomodoro): com a pergunta da rolagem a vista, espera por ela.
    fn show_background_splash(&mut self, text: String, seconds: u64) {
        if let Some(frame) = self
            .splash_board
            .show(text, seconds, SplashKind::Background)
        {
            self.present_splash(frame);
        }
    }

    /// Poe `frame` no popup (criando-o se preciso) e agenda o fim dele.
    fn present_splash(&mut self, frame: SplashFrame) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SPLASH_WIDTH * scale).round() as i32;
        let height = (SPLASH_HEIGHT * scale).round() as i32;

        // A dica do rato e o aviso nascem os dois no centro da janela: com a
        // dica viva do Pomodoro por baixo (refrescada a cada segundo), um
        // fim de fase empilhava duas mensagens no mesmo sitio. A dica sai;
        // escondida, `refresh_hint_text` ja nao a traz de volta.
        hover_tooltip(std::ptr::null_mut(), "");
        let SplashFrame {
            text,
            asks,
            seconds,
            token,
        } = frame;
        SPLASH_ASKS.store(asks, Ordering::SeqCst);
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

        self.timers
            .after(Duration::from_secs(seconds), UserEvent::HideSplash(token));
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
        let SplashHide::Hide {
            question_expired,
            next,
        } = self.splash_board.hide(token)
        else {
            return;
        };
        SPLASH_ASKS.store(false, Ordering::SeqCst);
        // So o fim do quadro da PROPRIA pergunta e um "nao" (ver
        // `SplashBoard`); um aviso que a substituiu nao responde nada.
        if question_expired {
            self.auto_scroll_answered = true;
            self.auto_scroll.set(false);
        }
        if let Some(splash) = self.splash.take() {
            unsafe {
                DestroyWindow(splash);
            }
        }
        if let Some(frame) = next {
            self.present_splash(frame);
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

    /// Centra o cartao de pesquisa na janela. Em coordenadas de ECRA, como o
    /// splash: refaz-se quando a janela se mexe.
    fn position_search_card(&self) {
        let (Some(window), Some(card)) = (&self.window, self.search_card_popup) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SEARCH_CARD_WIDTH * scale).round() as i32;
        let height = (SEARCH_CARD_HEIGHT * scale).round() as i32;
        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let (x, y) = splash_origin(client.right, client.bottom, width, height);
            SetWindowPos(
                card,
                std::ptr::null_mut(),
                origin.x + x,
                origin.y + y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(card);
            InvalidateRect(card, std::ptr::null(), 1);
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

        if self.auto_scroll.get() {
            self.auto_scroll_token = self.auto_scroll_token.wrapping_add(1);
            self.schedule_auto_scroll();
            self.show_splash(auto_scroll_message(true), 4);
        }
    }

    fn ask_auto_scroll(&mut self) {
        if let Some(frame) = self.splash_board.show(
            format!("Rolar a página sozinho a cada {AUTO_SCROLL_SECONDS}s?"),
            AUTO_SCROLL_PROMPT_SECONDS,
            SplashKind::Question,
        ) {
            self.present_splash(frame);
        }
    }

    /// Sem resposta nao se mexe: se a pergunta desaparecer sozinha, fica "nao"
    /// ate a pessoa carregar em F8.
    fn answer_auto_scroll(&mut self, yes: bool) {
        self.splash_board.answered();
        self.auto_scroll_answered = true;
        self.auto_scroll.set(yes);

        if yes {
            self.auto_scroll_token = self.auto_scroll_token.wrapping_add(1);
            self.schedule_auto_scroll();
            // Substitui a pergunta; um aviso que esperava por ela aparece
            // quando este sair.
            self.show_splash(auto_scroll_message(true), 4);
        } else {
            // Sai ja; o aviso que esperava (se houver) aparece agora.
            self.hide_splash(self.splash_board.current());
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
        if !self.auto_scroll.get() || token != self.auto_scroll_token {
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
            // O leitor de livros vira as proprias paginas; a biblioteca nao rola.
            Surface::Home | Surface::Epub => {}
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
            bar_columns(comp, self.pomodoro_bar_label()),
            tab_rows_focused(
                &comp.contexts,
                &comp.groups,
                active_context(comp),
                comp.bar_focus,
            ),
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
        let area = match self.bar_layout() {
            Some(layout) if self.surface == Surface::Comparator => caption_area(&layout),
            _ => home_caption_rect(window.inner_size().width as f64, window.scale_factor()),
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let left = area.x;
        let width = area.width;
        let height = area.height;

        if let Some(buttons) = self.caption_buttons
            && unsafe { GetParent(buttons) } != owner
        {
            unsafe {
                DestroyWindow(buttons);
            }
            self.caption_buttons = None;
        }

        let visible = caption_buttons_visible(
            self.surface,
            self.caption_reveal.shown(),
            self.service_covers_window(),
        );
        if self.caption_buttons.is_none() {
            unsafe {
                // Nasce escondida na Home: aparecer e desaparecer logo a
                // seguir era um piscar no canto a cada regresso a Home.
                let created = CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("NeuralIA.CaptionControls"),
                    if visible {
                        WS_CHILD | WS_VISIBLE
                    } else {
                        WS_CHILD
                    },
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
                let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                if SetWindowSubclass(
                    created,
                    Some(caption_buttons_subclass),
                    CAPTION_BUTTONS_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                self.caption_buttons = Some(created);
            }
        }

        if let Some(buttons) = self.caption_buttons {
            place_caption_buttons(
                buttons,
                left.round() as i32,
                width.round() as i32,
                height.round() as i32,
                visible,
            );
        }
    }

    /// Onde estao os tres botoes da janela na Home, em pixels do cliente.
    fn home_caption_area(&self) -> Option<Area> {
        let window = self.window.as_ref()?;
        Some(home_caption_rect(
            window.inner_size().width as f64,
            window.scale_factor(),
        ))
    }

    /// Na Home: os botoes aparecem quando o rato entra na zona deles e
    /// somem 300 ms depois de ele sair (CaptionReveal). A posicao do rato e
    /// lida ao Windows, nao ao ultimo CursorMoved: por cima dos proprios
    /// botoes (outra janela) ou fora da janela a janela principal nao ve
    /// movimento nenhum.
    fn refresh_caption_reveal(&mut self) {
        if self.surface != Surface::Home {
            return;
        }
        let (Some(window), Some(area)) = (&self.window, self.home_caption_area()) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let mut point = POINT { x: 0, y: 0 };
        let inside =
            unsafe { GetCursorPos(&mut point) != 0 && ScreenToClient(owner, &mut point) != 0 }
                && caption_hot_zone(area, CAPTION_HOT_MARGIN * scale)
                    .contains(point.x as f64, point.y as f64);
        match self.caption_reveal.observe(inside, now_ms()) {
            RevealStep::Show | RevealStep::Hide => self.sync_caption_buttons(),
            RevealStep::ScheduleHide(delay) => self
                .timers
                .after(Duration::from_millis(delay), UserEvent::CaptionReveal),
            RevealStep::Nothing => {}
        }
    }

    /// Cria/mostra/esconde o botao flutuante de saida. Existe apenas enquanto
    /// houver uma coluna em ecra completo -- e a unica saida sempre visivel,
    /// porque a barra de titulo desapareceu e a barra da app auto-esconde-se.
    fn sync_exit_button(&mut self) {
        // Em fullscreen e o controlo nativo permanente de saida. Nao depende
        // de hover nem de redimensionar o WebView.
        let wanted = (self.surface == Surface::Comparator && self.is_fullscreen_column())
            || self.service_frame().is_some_and(|frame| frame.exit_button);

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
            let logical_w = comparator_logical_width(size.width as f64 / scale, comp.panel_width);
            let logical_h = size.height as f64 / scale;
            // Com o painel de servicos em tela cheia por cima de tudo, os
            // divisores (popups, acima das WebViews) ficavam a flutuar sobre
            // o video.
            let show = self.surface == Surface::Comparator
                && comp.split.is_none()
                && comp.expanded.is_none()
                && !self.service_covers_window();
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
        let logical_w =
            comparator_logical_width(window.inner_size().width as f64 / scale, comp.panel_width);
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
            self.pomodoro_bar_label(),
        ))
    }

    /// A faixa do painel de servicos, em pixels do cliente (os do rato).
    fn service_strip_physical(&self) -> Option<Area> {
        let strip = self.service_frame()?.strip?;
        let scale = self.window.as_ref()?.scale_factor().max(1.0);
        Some(Area {
            x: strip.x * scale,
            y: strip.y * scale,
            width: strip.width * scale,
            height: strip.height * scale,
        })
    }

    fn comparator_bar_hit(&self) -> Option<BarHit> {
        if let Some(strip) = self.service_strip_physical() {
            let scale = self
                .window
                .as_ref()
                .map_or(1.0, |window| window.scale_factor().max(1.0));
            if let Some(button) = strip_hit(strip, scale, self.cursor.0, self.cursor.1) {
                return Some(BarHit::ServiceStrip(button));
            }
        }
        bar_hit_at(
            self.right_controls(),
            self.bar_layout(),
            self.cursor.0,
            self.cursor.1,
        )
    }

    /// Ferramentas da Home sob o rato: realce e dica, como na barra.
    fn update_home_tool_hover(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        if self.surface != Surface::Home {
            // Fora da Home a dica e da barra (`update_bar_hover` correu antes
            // neste mesmo movimento): so se esquece o realce, sem a apagar.
            self.home_tool_hover = None;
            return;
        }
        let next = home_tool_hit(
            window.inner_size().width as f64,
            window.scale_factor(),
            self.pomodoro_bar_label(),
            self.cursor.0,
            self.cursor.1,
        );
        if next == self.home_tool_hover {
            return;
        }
        self.home_tool_hover = next;
        if let Some(owner) = window_hwnd(window) {
            let text = next.map(|tool| tool_hint_at(tool, &self.pomodoro, Instant::now()));
            hover_tooltip(owner, text.as_deref().unwrap_or(""));
        }
        self.request_redraw();
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

    /// A dica centrada de um controlo injetado numa coluna. O texto e o nome
    /// da IA saem daqui; a pagina so disse qual dos controlos tem o rato.
    fn show_column_hint(&mut self, col: usize, hint: ColumnHint) {
        let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        match column_hint_step(self.column_hint, col, hint, latest_tooltip_request()) {
            ColumnHintStep::Show => {
                let Some(provider) = self
                    .comparator
                    .as_ref()
                    .filter(|_| self.surface == Surface::Comparator)
                    .and_then(|comp| comp.views.get(col))
                    .map(|view| view.name)
                else {
                    return;
                };
                let request = hover_tooltip(owner, &column_hint_text(hint, provider));
                self.column_hint = Some(ColumnHintOwner { col, request });
            }
            ColumnHintStep::Clear => {
                hover_tooltip(owner, "");
                self.column_hint = None;
            }
            ColumnHintStep::Keep => {}
        }
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

    /// Os icones da barra: o servico abre no painel ao lado; de novo, fecha
    /// -- ou, minimizado, volta (`ServicePanelState`).
    fn open_service_panel(&mut self, service: Service) {
        if let Some(panel) = self
            .service_panel
            .as_ref()
            .filter(|panel| panel.service == service)
        {
            // Voltar do minimizado ocupa o lugar do painel que estiver aberto.
            if panel.state.minimized() {
                self.close_side_panel(PanelExit::OtherPanel);
                self.close_live_panel();
            }
            self.service_input(ServiceInput::IconClick);
            return;
        }
        self.close_service_panel();
        // Um painel de cada vez.
        self.close_side_panel(PanelExit::OtherPanel);
        self.close_live_panel();
        let Some(area) = self
            .service_frame_for(ServicePanelState::default())
            .and_then(|frame| frame.panel)
        else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        // Sem IPC, sem scripts injetados, sem `record`/`capture`: nada do que
        // corre num painel de servico chega ao historico ou a memoria do
        // NeuralIA. O privado (Respiracao) tambem nao deixa nada no perfil
        // do WebView2: e InPrivate.
        let built = themed_webview_builder()
            .with_incognito(service.private())
            .with_url(service.url())
            .with_bounds(logical_rect(area))
            .with_navigation_handler(move |target| service_panel_navigation(service, &target))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            // Caminho A do WebRTC: camera e microfone pelo aviso do WebView2
            // -- salvo no painel privado, onde sao recusados.
            .with_permission_handler(move |kind| service_panel_permission(service, kind))
            .build_as_child(window);
        match built {
            Ok(panel) => {
                let _ = panel.focus();
                self.install_context_menu(&panel, WebViewHost::Service);
                self.service_generation = self.service_generation.wrapping_add(1);
                let generation = self.service_generation;
                // Sem estes avisos o painel abre na mesma; so a tela cheia da
                // pagina, o ponto "a tocar" e o Esc ficam de fora.
                if let Err(error) =
                    register_service_panel_events(&panel, generation, self.proxy.clone())
                {
                    debug_log(format_args!("service panel: sem avisos ({error})"));
                }
                debug_log(format_args!("service panel: {service:?}"));
                self.service_panel = Some(ServicePanel {
                    service,
                    webview: panel,
                    state: ServicePanelState::default(),
                    audio: false,
                    generation,
                });
                self.apply_service_frame();
            }
            Err(error) => {
                self.show_splash(
                    format!("Não foi possível abrir {}: {error}", service.label()),
                    3,
                );
            }
        }
    }

    /// Fecha o painel de servicos so se ele estiver a vista.
    fn close_docked_service_panel(&mut self) {
        if self
            .service_panel
            .as_ref()
            .is_some_and(|panel| !panel.state.minimized())
        {
            self.close_service_panel();
        }
    }

    fn close_service_panel(&mut self) {
        // O teclado volta a omnibox (Home) ou a janela: `release_panel`
        // (gate `closing_a_panel_gives_the_keyboard_back`).
        if !close_service_panel_in(&mut self.service_panel, self.surface, self.omnibox) {
            return;
        }
        debug_log(format_args!("service panel: fechado"));
        let window_fullscreen = self
            .window
            .as_ref()
            .is_some_and(|window| window.fullscreen().is_some());
        if let Some(on) = self.panel_window_fullscreen.step(false, window_fullscreen)
            && let Some(window) = &self.window
        {
            window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        }
        self.fit_comparator_to_panel();
        self.sync_comparator_splitters();
        self.sync_exit_button();
        self.sync_caption_buttons();
        self.after_panel_change();
        self.needs_clear = true;
        self.request_redraw();
    }

    /// A janela em pixels logicos e o topo dos paineis da direita
    /// (`right_panel_top`: abaixo da barra no comparador, abaixo dos botoes
    /// da janela na Home).
    fn panel_space(&self) -> Option<(f64, f64, f64, f64)> {
        let window = self.window.as_ref()?;
        let scale = window.scale_factor().max(1.0);
        let size = window.inner_size();
        Some((
            size.width as f64 / scale,
            size.height as f64 / scale,
            right_panel_top(self.surface),
            scale,
        ))
    }

    fn chosen_panel_width(&self, kind: PanelKind, logical_w: f64) -> f64 {
        panel_width(kind, self.panel_widths.get(kind), logical_w)
    }

    /// O que o painel de servicos, no modo `state`, pede a janela agora.
    fn service_frame_for(&self, state: ServicePanelState) -> Option<ServiceFrame> {
        let (logical_w, logical_h, top, _) = self.panel_space()?;
        // A faixa de controlos so existe no comparador, que e onde ha barra.
        let strip = if self.surface == Surface::Comparator {
            SERVICE_STRIP_HEIGHT
        } else {
            0.0
        };
        Some(state.frame(
            self.chosen_panel_width(PanelKind::Service, logical_w),
            logical_w,
            logical_h,
            top,
            strip,
        ))
    }

    fn service_frame(&self) -> Option<ServiceFrame> {
        self.service_frame_for(self.service_panel.as_ref()?.state)
    }

    fn service_covers_window(&self) -> bool {
        self.service_frame()
            .is_some_and(|frame| frame.window_fullscreen)
    }

    fn service_event_is_current(&self, generation: u64) -> bool {
        service_event_is_current(
            self.service_panel.as_ref().map(|panel| panel.generation),
            generation,
        )
    }

    fn position_service_panel(&self) {
        let (Some(panel), Some(frame)) = (&self.service_panel, self.service_frame()) else {
            return;
        };
        match frame.panel {
            Some(area) => {
                let _ = panel.webview.set_bounds(logical_rect(area));
                let _ = panel.webview.set_visible(true);
                if frame.window_fullscreen {
                    // Por cima das colunas e de todos os filhos da janela.
                    raise_webview_host(&panel.webview);
                }
            }
            // Minimizado: sai da frente, a pagina continua viva (o som
            // continua, como numa aba em segundo plano), e o teclado nao fica
            // preso nela -- uma tecla perdida pausava o video.
            None => {
                let _ = panel.webview.set_visible(false);
                let _ = panel.webview.focus_parent();
            }
        }
    }

    /// Um passo do painel de servicos: faixa, icone, a propria pagina, Esc.
    fn service_input(&mut self, input: ServiceInput) {
        let Some(panel) = self.service_panel.as_mut() else {
            return;
        };
        match panel.state.step(input) {
            ServiceEffect::Relayout => self.apply_service_frame(),
            ServiceEffect::ExitPageFullscreen => {
                let _ = panel.webview.evaluate_script(EXIT_PAGE_FULLSCREEN_SCRIPT);
            }
            ServiceEffect::Close => self.close_service_panel(),
            ServiceEffect::Nothing => {}
        }
    }

    /// Aplica o modo do painel de servicos a janela inteira: tela cheia da
    /// janela, painel, colunas, divisores, "Sair", pega e roda.
    fn apply_service_frame(&mut self) {
        let fullscreen = self.service_covers_window();
        let entering = fullscreen && !self.panel_window_fullscreen.active();
        let window_fullscreen = self
            .window
            .as_ref()
            .is_some_and(|window| window.fullscreen().is_some());
        // So se desfaz o que o painel fez: com o split ja em tela cheia, sair
        // do YouTube deixa a janela como o split a quer.
        if let Some(on) = self
            .panel_window_fullscreen
            .step(fullscreen, window_fullscreen)
            && let Some(window) = &self.window
        {
            window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        }
        // Em tela cheia o teclado vai para o painel: o Esc (e as teclas do
        // proprio video) chegam a ele e nao a uma coluna escondida.
        if entering && let Some(panel) = &self.service_panel {
            let _ = panel.webview.focus();
        }
        self.position_service_panel();
        self.fit_comparator_to_panel();
        self.sync_comparator_splitters();
        self.sync_exit_button();
        // Os botoes da janela saem da frente do painel em tela cheia e voltam
        // ao sair -- tambem quando a janela ja estava em tela cheia e nao ha
        // Resized nenhum a sincroniza-los.
        self.sync_caption_buttons();
        self.after_panel_change();
        self.needs_clear = true;
        self.request_redraw();
    }

    /// O painel da direita a vista e encostado (o que tem pega), em pixels
    /// logicos, e o tipo de largura que ele usa.
    fn docked_right_panel(&self) -> Option<(PanelKind, Area)> {
        let (logical_w, logical_h, top, _) = self.panel_space()?;
        // O de servicos minimizado nao esta a vista: conta o outro painel.
        if let Some(frame) = self.service_frame().filter(|frame| frame.panel.is_some()) {
            return frame
                .resize_handle
                .then_some(frame.panel)
                .flatten()
                .map(|area| (PanelKind::Service, area));
        }
        let kind = if self.live_panel.is_open() {
            PanelKind::Service
        } else if self.side_panel.is_open() {
            PanelKind::History
        } else {
            return None;
        };
        Some((
            kind,
            panel_area(
                self.chosen_panel_width(kind, logical_w),
                logical_w,
                logical_h,
                top,
            ),
        ))
    }

    /// O painel da direita a vista (encostado ou em tela cheia) e a janela
    /// hospedeira dele, para a roda do rato.
    fn visible_right_panel(&self) -> Option<(Area, HWND)> {
        use wry::WebViewExtWindows;
        if let Some(panel) = &self.service_panel
            && let Some(area) = self.service_frame().and_then(|frame| frame.panel)
        {
            return Some((area, panel.webview.hwnd().0 as HWND));
        }
        let (_, area) = self.docked_right_panel()?;
        let host = self.live_panel.view().or(self.side_panel.view())?.hwnd().0 as HWND;
        Some((area, host))
    }

    /// Depois de qualquer mudanca nos paineis da direita: a pega e a roda.
    fn after_panel_change(&mut self) {
        self.sync_panel_handle();
        self.sync_wheel_route();
    }

    /// A roda sobre o painel da direita vai para o painel (`wheel_hook`).
    fn sync_wheel_route(&self) {
        let target = self.visible_right_panel().and_then(|(area, host)| {
            let window = self.window.as_ref()?;
            let owner = window_hwnd(window)?;
            let scale = window.scale_factor().max(1.0);
            let mut origin = POINT { x: 0, y: 0 };
            unsafe {
                ClientToScreen(owner, &mut origin);
            }
            let rect = ScreenRect {
                left: origin.x + (area.x * scale).round() as i32,
                top: origin.y + (area.y * scale).round() as i32,
                right: origin.x + ((area.x + area.width) * scale).round() as i32,
                bottom: origin.y + ((area.y + area.height) * scale).round() as i32,
            };
            Some((rect, host, owner))
        });
        match target {
            Some((rect, host, owner)) => {
                WHEEL_PANEL_LEFT.store(rect.left, Ordering::Release);
                WHEEL_PANEL_TOP.store(rect.top, Ordering::Release);
                WHEEL_PANEL_RIGHT.store(rect.right, Ordering::Release);
                WHEEL_PANEL_BOTTOM.store(rect.bottom, Ordering::Release);
                WHEEL_PANEL_HOST.store(host as usize, Ordering::Release);
                WHEEL_APP_HWND.store(owner as usize, Ordering::Release);
                WHEEL_PANEL_ACTIVE.store(true, Ordering::Release);
                install_wheel_hook();
            }
            None => {
                WHEEL_PANEL_ACTIVE.store(false, Ordering::Release);
                uninstall_wheel_hook();
            }
        }
    }

    /// A pega da borda esquerda do painel: popup owned como os divisores das
    /// colunas, nunca ativa, so com o painel encostado.
    fn sync_panel_handle(&mut self) {
        let wanted = self.docked_right_panel();
        let (Some((_, panel)), Some(window)) = (wanted, &self.window) else {
            if let Some(handle) = self.panel_handle {
                unsafe {
                    ShowWindow(handle, SW_HIDE);
                }
            }
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let area = panel_handle_area(panel, PANEL_HANDLE_WIDTH);

        if let Some(handle) = self.panel_handle
            && unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindow(handle, 4) } != owner
        {
            unsafe {
                DestroyWindow(handle);
            }
            self.panel_handle = None;
        }
        let handle = match self.panel_handle {
            Some(handle) => handle,
            None => unsafe {
                let created = CreateWindowExW(
                    AUX_POPUP_EX_STYLE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("NeuralIA.PanelResize"),
                    AUX_POPUP_STYLE,
                    0,
                    0,
                    1,
                    1,
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
                    Some(panel_handle_subclass),
                    PANEL_HANDLE_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                self.panel_handle = Some(created);
                created
            },
        };
        let mut origin = POINT { x: 0, y: 0 };
        unsafe {
            ClientToScreen(owner, &mut origin);
            SetWindowPos(
                handle,
                std::ptr::null_mut(),
                origin.x + (area.x * scale).round() as i32,
                origin.y + (area.y * scale).round() as i32,
                (area.width * scale).round().max(3.0) as i32,
                (area.height * scale).round().max(1.0) as i32,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(handle);
            InvalidateRect(handle, std::ptr::null(), 1);
        }
    }

    fn hide_panel_handle(&self) {
        if let Some(handle) = self.panel_handle {
            unsafe {
                ShowWindow(handle, SW_HIDE);
            }
        }
    }

    /// A pega foi arrastada: a borda do painel segue o rato, presa a
    /// [300 px, 60% da janela], e as colunas refluem para o lado.
    fn resize_panel(&mut self) {
        // Limpar a marca ANTES de ler, como no divisor das colunas.
        PANEL_RESIZE_PENDING.store(false, Ordering::Release);
        let Some((kind, _)) = self.docked_right_panel() else {
            return;
        };
        let (Some(window), Some((logical_w, _, _, scale))) = (&self.window, self.panel_space())
        else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let mut point = POINT {
            x: PANEL_RESIZE_X.load(Ordering::Acquire),
            y: 0,
        };
        unsafe {
            ScreenToClient(owner, &mut point);
        }
        let width = panel_width_from_drag(point.x as f64 / scale, logical_w);
        self.panel_widths.set(kind, width);
        self.relayout_right_panels();
    }

    /// Recoloca os paineis da direita e as colunas depois de mudar a largura.
    fn relayout_right_panels(&mut self) {
        self.position_side_panel();
        self.position_service_panel();
        self.position_live_panel();
        self.fit_comparator_to_panel();
        self.after_panel_change();
        self.needs_clear = true;
        self.request_redraw();
    }

    /// Largou a pega: a largura fica gravada (escrita atomica).
    fn save_panel_widths(&mut self) {
        let path = self.config.data_dir.join(PANEL_WIDTHS_FILE);
        if let Err(error) = self.panel_widths.save(&path) {
            self.show_splash(format!("A largura do painel não foi gravada: {error}"), 3);
        }
    }

    /// O olho da barra: liga o Gemini Live (abre o painel, que pede a chave
    /// na primeira vez e depois liga tela, camera e microfone); de novo,
    /// desliga -- fechar o painel destroi a pagina e com ela tudo o que
    /// estava a ser capturado.
    fn toggle_live_panel(&mut self) {
        if self.live_panel.is_open() {
            self.close_live_panel();
        } else {
            self.open_live_panel();
        }
    }

    fn live_panel_rect(&self) -> Option<wry::Rect> {
        let (logical_w, logical_h, top, _) = self.panel_space()?;
        let (x, y, width, height) = panel_bounds(
            PanelKind::Service,
            self.panel_widths.get(PanelKind::Service),
            logical_w,
            logical_h,
            top,
        );
        Some(wry::Rect {
            position: LogicalPosition::new(x, y).into(),
            size: LogicalSize::new(width, height).into(),
        })
    }

    fn open_live_panel(&mut self) {
        // Um painel de cada vez -- o de servicos minimizado nao esta a vista e
        // continua a tocar.
        self.close_docked_service_panel();
        self.close_side_panel(PanelExit::OtherPanel);
        let Some(bounds) = self.live_panel_rect() else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let proxy = self.proxy.clone();
        let built = themed_webview_builder()
            // Origem propria: `http://neuralia-live.localhost` e contexto
            // seguro, e sem isso nao ha getUserMedia nem getDisplayMedia.
            .with_custom_protocol(LIVE_PROTOCOL.to_string(), move |_id, request| {
                serve_live_asset(&request)
            })
            .with_url(live_page_url())
            .with_bounds(bounds)
            .with_ipc_handler(live_panel_ipc_handler(move |event| {
                let _ = proxy.send_event(event);
            }))
            .with_navigation_handler(live_panel_navigation)
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_permission_handler(live_panel_permission)
            .build_as_child(window);
        match built {
            Ok(panel) => {
                let _ = panel.focus();
                debug_log(format_args!("live panel: ligado"));
                self.live_panel.open(panel);
                self.fit_comparator_to_panel();
                self.after_panel_change();
                self.request_redraw();
            }
            Err(error) => {
                self.show_splash(format!("Não foi possível abrir o Gemini Live: {error}"), 3);
            }
        }
    }

    fn close_live_panel(&mut self) {
        let Some(panel) = self.live_panel.close() else {
            return;
        };
        let _ = panel.set_visible(false);
        let _ = panel.focus_parent();
        drop(panel);
        debug_log(format_args!("live panel: desligado"));
        self.fit_comparator_to_panel();
        self.after_panel_change();
        self.request_redraw();
    }

    fn position_live_panel(&self) {
        if let (Some(panel), Some(bounds)) = (self.live_panel.view(), self.live_panel_rect()) {
            let _ = panel.set_bounds(bounds);
        }
    }

    fn live_eval(&self, script: &str) {
        if let Some(panel) = self.live_panel.view() {
            let _ = panel.evaluate_script(script);
        }
    }

    fn handle_live_message(&mut self, message: LiveMessage) {
        let store = LiveKeyStore::in_dir(&self.config.data_dir);
        let step = live_step(message, &store, &panel_theme_vars(&Theme::system()));
        // O script vem do painel depois de o olho mudar: vermelho antes de a
        // captura poder comecar.
        match self.live_panel.follow(step) {
            LiveAction::Run(script) => self.live_eval(&script),
            LiveAction::Close => self.close_live_panel(),
            LiveAction::Nothing => {}
        }
        self.request_redraw();
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
        let scale = window.scale_factor().max(1.0);
        open_panel_width_for(
            self.surface,
            self.service_panel.as_ref().map(|panel| panel.state),
            self.live_panel.is_open(),
            self.side_panel.is_open(),
            window.inner_size().width as f64 / scale,
            self.panel_widths,
        )
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
        if self.side_panel.is_open() {
            self.close_side_panel(PanelExit::CtrlH);
        } else {
            self.open_side_panel();
        }
    }

    fn side_panel_rect(&self) -> Option<wry::Rect> {
        let (logical_w, logical_h, top, _) = self.panel_space()?;
        let (x, y, width, height) = panel_bounds(
            PanelKind::History,
            self.panel_widths.get(PanelKind::History),
            logical_w,
            logical_h,
            top,
        );
        Some(wry::Rect {
            position: LogicalPosition::new(x, y).into(),
            size: LogicalSize::new(width, height).into(),
        })
    }

    fn open_side_panel(&mut self) {
        // Um painel de cada vez -- o de servicos minimizado nao esta a vista e
        // continua a tocar.
        self.close_docked_service_panel();
        self.close_live_panel();
        let Some(bounds) = self.side_panel_rect() else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let proxy = self.proxy.clone();
        // O numero desta pagina vai no canal dela: um pedido que chegue
        // depois de ela sair nao se confunde com o da seguinte.
        let ticket = self.side_panel.ticket();
        // Criado por ultimo, fica por cima das outras WebViews.
        let built = themed_webview_builder()
            .with_html(panel_html(&Theme::system()))
            .with_bounds(bounds)
            .with_ipc_handler(move |request| {
                if let Some(post) = side_panel::PanelPost::parse(ticket, request.body()) {
                    let _ = proxy.send_event(UserEvent::Panel(post));
                }
            })
            .with_navigation_handler(|target| panel_allows_navigation(&target))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .build_as_child(window);
        match built {
            Ok(panel) => {
                let _ = panel.focus();
                self.install_context_menu(&panel, WebViewHost::SidePanel);
                // So com o painel fechado se chega aqui; um aberto nunca e
                // largado sem `close_side_panel`.
                if let Err(extra) = self.side_panel.open(ticket, panel) {
                    drop(extra);
                    return;
                }
                self.fit_comparator_to_panel();
                self.after_panel_change();
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

    /// A saida unica do painel do Ctrl+H, venha de onde vier: o X, o Ctrl+H,
    /// outro painel, a Home, uma pesquisa nova, `destroy_web_surfaces` (o
    /// ecra de erro, a Web completa, o Leitor, o PDF, um link externo) e a
    /// saida da app. `SidePanel::dismiss` grava primeiro o que o editor
    /// tinha por salvar e so depois larga a WebView, devolvendo o teclado
    /// (gate `every_way_out_of_the_side_panel_saves_the_note_being_typed_once`).
    fn close_side_panel(&mut self, exit: PanelExit) {
        let Some(closed) = self.side_panel.dismiss(exit, self.surface, self.omnibox) else {
            return;
        };
        self.panel_suggestion_query = None;
        debug_log(format_args!("side panel: fechado ({exit:?})"));
        if let Err(error) = closed.saved {
            self.show_splash(error, 6);
        }
        self.fit_comparator_to_panel();
        self.after_panel_change();
        // Na Home, o sitio do painel volta a ser a Home.
        if self.surface == Surface::Home {
            self.needs_clear = true;
            self.request_redraw();
        }
    }

    fn position_side_panel(&self) {
        if let (Some(panel), Some(bounds)) = (self.side_panel.view(), self.side_panel_rect()) {
            let _ = panel.set_bounds(bounds);
        }
    }

    fn panel_eval(&self, script: &str) {
        if let Some(panel) = self.side_panel.view() {
            let _ = panel.evaluate_script(script);
        }
    }

    /// Corre `script` no painel, ou guarda-o para o "ready" se a pagina
    /// ainda nao correu o script dela. Sem painel, nao faz nada.
    fn panel_run(&mut self, script: String) {
        if let Some(script) = self.side_panel.run(script) {
            self.panel_eval(&script);
        }
    }

    /// Abre (se preciso -- nunca fecha) o painel ja nas Notas e corre la os
    /// `scripts`, pela ordem.
    fn show_notes_panel(&mut self, scripts: Vec<String>) {
        if !self.side_panel.is_open() {
            self.open_side_panel();
        }
        if !self.side_panel.is_open() {
            return;
        }
        self.panel_run(PANEL_SHOW_NOTES_SCRIPT.to_string());
        for script in scripts {
            self.panel_run(script);
        }
    }

    fn submit_notes(&mut self, command: NotesCommand, origin: NotesOrigin) {
        if let Err(error) = self.notes.submit(command, origin) {
            match origin {
                NotesOrigin::Panel => {
                    self.panel_run(notes_reply_script(&NotesReply::Failed(error)))
                }
                NotesOrigin::Selection | NotesOrigin::Closed | NotesOrigin::Bar { .. } => {
                    self.show_splash(error, 3)
                }
            }
        }
    }

    /// A app vai sair: o que o editor do painel tinha por salvar -- e o que
    /// fechos anteriores ainda tinham na fila -- chega ao disco antes de o
    /// processo acabar (`SidePanel::exit`).
    fn save_notes_draft_before_exit(&mut self) {
        let _ = self.side_panel.exit(NOTES_EXIT_WAIT);
    }

    /// Resposta do worker das notas. A de uma selecao abre o painel na nota
    /// criada; a do painel so vai para o painel que ainda estiver aberto.
    fn notes_ready(&mut self, origin: NotesOrigin, reply: NotesReply) {
        match origin {
            NotesOrigin::Panel => self.panel_run(notes_reply_script(&reply)),
            NotesOrigin::Selection => match &reply {
                NotesReply::Opened { .. } => {
                    self.show_notes_panel(vec![notes_reply_script(&reply)]);
                    self.show_splash("Nota criada".to_string(), 2);
                }
                NotesReply::Failed(error) => self.show_splash(error.clone(), 4),
                NotesReply::Listed { .. }
                | NotesReply::Deleted { .. }
                | NotesReply::Missing { .. }
                | NotesReply::Conflict { .. } => {}
            },
            // O "Salvar nota": so o aviso, sem abrir o painel.
            NotesOrigin::Bar { private } => match &reply {
                NotesReply::Opened { note, .. } => {
                    self.show_splash(bar_note_notice(private, &note.title), 3);
                }
                NotesReply::Failed(error) => self.show_splash(error.clone(), 4),
                NotesReply::Listed { .. }
                | NotesReply::Deleted { .. }
                | NotesReply::Missing { .. }
                | NotesReply::Conflict { .. } => {}
            },
            NotesOrigin::Closed => match &reply {
                NotesReply::Opened { note, .. } => {
                    self.show_splash(format!("Nota salva: {}", note.title), 3);
                }
                NotesReply::Conflict { note, .. } => {
                    self.show_splash(
                        format!(
                            "A nota mudou fora do NeuralIA; o texto ficou em: {}",
                            note.title
                        ),
                        6,
                    );
                }
                NotesReply::Failed(error) => self.show_splash(error.clone(), 6),
                NotesReply::Listed { .. }
                | NotesReply::Deleted { .. }
                | NotesReply::Missing { .. } => {}
            },
        }
    }

    /// Ctrl+Shift+Z ou "Salvar nota" numa pagina: a nota da WebView
    /// `target`. O Ctrl+Shift+Z le a selecao dela e nunca le o Split privado;
    /// o Salvar nota traz o texto no pedido e so tira dela a fonte.
    fn request_note_from_page(&mut self, target: Option<PageTarget>, via: NoteVia) {
        // Qual WebView e se o Split privado recusa: `note_read_view`, com as
        // colunas e o Split do proprio comparador (gate
        // `a_note_request_reads_its_own_webview_and_never_the_private_split`).
        let comp = self.comparator.as_ref();
        // So o "Salvar nota" chega ao Split privado; `private` vem da mesma
        // decisao, e o aviso di-lo-a.
        let NoteRead {
            view: webview,
            private,
        } = match note_read_view(
            target,
            &via,
            comp.map_or(&[][..], |comp| comp.views.as_slice()),
            comp.and_then(|comp| comp.split.as_ref()),
            self.webview.as_ref(),
        ) {
            Ok(read) => read,
            Err(NoteCapture::RefusePrivate) => {
                self.show_splash(NOTE_PRIVATE_REFUSAL.to_string(), 3);
                return;
            }
            Err(NoteCapture::Read | NoteCapture::NoPage) => return,
        };
        let source = note_capture_source(
            &via,
            target,
            self.surface,
            self.page_source.as_deref(),
            || webview.url().ok(),
        );
        match via {
            // O texto e o que a barra mostrava, e veio no pedido: nada se
            // volta a ler da pagina (que podia ter trocado o getSelection).
            NoteVia::Bar { text } => self.save_bar_note(&text, source.as_deref(), private),
            NoteVia::Shortcut => {
                let proxy = self.proxy.clone();
                let asked =
                    webview.evaluate_script_with_callback(NOTE_CAPTURE_SCRIPT, move |raw| {
                        let _ = proxy.send_event(UserEvent::NoteCaptured {
                            raw,
                            source: source.clone(),
                        });
                    });
                if asked.is_err() {
                    self.show_splash(
                        "Não foi possível ler a seleção desta página.".to_string(),
                        3,
                    );
                }
            }
        }
    }

    /// O "Salvar nota": a fonte e a que o nativo conhece da WebView, o mesmo
    /// texto em menos de 2 s nao e outra nota, e a resposta so aparece no
    /// aviso do meio (`NotesOrigin::Bar`). Nada disto passa pelo historico
    /// nem pela memoria.
    fn save_bar_note(&mut self, text: &str, source: Option<&str>, private: bool) {
        match bar_note_step(&mut self.bar_notes, text, source, Instant::now()) {
            BarNoteStep::Save(draft) => {
                self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Bar { private });
            }
            BarNoteStep::Repeated => {}
            BarNoteStep::Refused(error) => self.note_refused(error),
        }
    }

    /// A resposta da pagina a um Ctrl+Shift+Z.
    fn note_captured(&mut self, raw: &str, source: Option<&str>) {
        match shortcut_note_step(&mut self.shortcut_notes, raw, source, Instant::now()) {
            BarNoteStep::Save(draft) => {
                self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Selection);
            }
            BarNoteStep::Repeated => {}
            BarNoteStep::Refused(error) => self.note_refused(error),
        }
    }

    fn note_refused(&mut self, error: NoteCaptureError) {
        match error {
            NoteCaptureError::EmptySelection => {
                self.show_splash("Selecione um texto para criar a nota".to_string(), 3);
            }
            NoteCaptureError::Unreadable => {
                self.show_splash(
                    "Não foi possível ler a seleção desta página.".to_string(),
                    3,
                );
            }
        }
    }

    /// Ctrl+Shift+Z na Home ou na barra: o painel nas Notas, com uma nota
    /// nova em branco no editor.
    fn new_note_in_panel(&mut self) {
        self.show_notes_panel(vec![PANEL_NEW_NOTE_SCRIPT.to_string()]);
    }

    /// Botao Notas (Zettelkasten): abre o painel do Ctrl+H ja na secao das
    /// notas. Com o painel aberto quem decide e a pagina
    /// (`PANEL_NOTES_BUTTON_SCRIPT`): nas Notas fecha -- salvando antes o
    /// que o editor tinha, como o X --, no Historico passa para as Notas.
    ///
    /// A pagina do painel acabou de nascer e ainda nao correu o script dela:
    /// um `evaluate_script` agora corria no documento vazio e perdia-se. Por
    /// isso `show_notes_panel` guarda o `PANEL_SHOW_NOTES_SCRIPT` em
    /// `panel_pending`, e o `PanelMessage::Ready` (o "pronto" que a pagina
    /// manda no fim do script) corre-o.
    fn open_notes(&mut self) {
        if self.side_panel.is_open() {
            self.panel_run(PANEL_NOTES_BUTTON_SCRIPT.to_string());
            return;
        }
        self.show_notes_panel(Vec::new());
    }

    /// Clique no botao do Pomodoro (barra ou Home): parado inicia, a correr
    /// pausa, pausado retoma.
    fn pomodoro_click(&mut self) {
        self.pomodoro_command(PomodoroCommand::Click);
    }

    /// Botao direito no Pomodoro: o menu nativo do estado de agora (so a
    /// accao que se aplica, Pular fase, Parar e os presets com a marca no
    /// que esta em uso).
    fn pomodoro_menu(&mut self) {
        let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        let items = self.pomodoro.menu_items();
        let picked = pick_pomodoro_from_menu(owner, &items);
        if let Some(command) = pomodoro_menu_command(&items, picked) {
            self.pomodoro_command(command);
        }
    }

    /// Clique, menu e `pomodoro:` passam todos por aqui. A decisao e do
    /// `PomodoroController`; aqui so se faz o que ele devolve.
    fn pomodoro_command(&mut self, command: PomodoroCommand) {
        // `run_command` agenda a cadeia nova em `self.timers` (gate:
        // `only_one_tick_chain_is_ever_alive`).
        let outcome = self
            .pomodoro
            .run_command(command, Instant::now(), &self.timers);
        let saved = outcome.save.map(|settings| {
            crate::pomodoro_ui::save_settings(&self.config.data_dir.join("pomodoro"), settings)
        });
        let notice = match saved {
            Some(Err(error)) => Some(format!(
                "Não foi possível gravar as opções do Pomodoro: {error}"
            )),
            _ => outcome.notice,
        };
        if let Some(notice) = notice {
            self.show_splash(notice, POMODORO_NOTICE_SECONDS);
        }
        self.pomodoro_changed();
    }

    /// Um tique: o caminho inteiro e `pomodoro_ui::pomodoro_tick` (gate
    /// `the_app_tick_announces_a_phase_end_as_the_window_can_see_it`): so o
    /// da cadeia viva mexe no motor, o seguinte fica agendado e o fim de uma
    /// fase toca o som, pisca a barra de tarefas e mostra o aviso conforme a
    /// janela. O `App` so da as pecas (`impl PomodoroHost for App`).
    fn pomodoro_tick(&mut self, token: u64) {
        crate::pomodoro_ui::pomodoro_tick(self, token, Instant::now());
    }

    /// Minimizada e a da frente, para o fim de uma fase do Pomodoro.
    fn window_attention(&self) -> WindowAttention {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let Some(window) = &self.window else {
            return WindowAttention {
                minimized: true,
                foreground: false,
            };
        };
        let foreground =
            window_hwnd(window).is_some_and(|owner| unsafe { GetForegroundWindow() } == owner);
        WindowAttention {
            minimized: window.is_minimized().unwrap_or(false),
            foreground,
        }
    }

    /// O tempo mudou: repinta o botao (a barra, ou a Home) e, se a dica do
    /// Pomodoro estiver a vista, poe-lhe o tempo novo sem a esconder.
    fn pomodoro_changed(&mut self) {
        let hovered = match self.surface {
            Surface::Comparator => self.bar_hover == Some(BarHit::Tool(Tool::Pomodoro)),
            Surface::Home => self.home_tool_hover == Some(Tool::Pomodoro),
            _ => false,
        };
        if hovered {
            refresh_hint_text(&tool_hint_at(
                Tool::Pomodoro,
                &self.pomodoro,
                Instant::now(),
            ));
        }
        if matches!(self.surface, Surface::Comparator | Surface::Home) {
            self.request_redraw();
        }
    }

    /// O tempo que falta no Pomodoro ("mm:ss", "⏸ mm:ss" pausado) para ir ao
    /// lado do icone, ou `None` parado (o botao so com o icone).
    fn pomodoro_label(&self) -> Option<String> {
        self.pomodoro.label(Instant::now())
    }

    /// A etiqueta ja no formato que a barra e a Home desenham e medem, com a
    /// fase para a cor.
    fn pomodoro_bar_label(&self) -> Option<BarLabel> {
        self.pomodoro_label()
            .as_deref()
            .and_then(BarLabel::new)
            .map(|label| label.with_phase(self.pomodoro.active_phase()))
    }

    fn run_tool_action(&mut self, action: ToolAction) {
        // O clique pode abrir um painel por cima do botao: a dica nao fica.
        hover_tooltip(std::ptr::null_mut(), "");
        match action {
            ToolAction::PomodoroClick => self.pomodoro_click(),
            ToolAction::PomodoroMenu => self.pomodoro_menu(),
            ToolAction::ToggleNotes => self.open_notes(),
            ToolAction::ToggleBreath => self.open_service_panel(Service::Breath),
        }
    }

    /// Botao direito no comparador: nas ferramentas vai para elas (so o
    /// Pomodoro tem menu); no resto, o menu das abas de sempre.
    fn right_click_comparator(&mut self) {
        let hit = self.comparator_bar_hit();
        if matches!(hit, Some(BarHit::Tool(_))) {
            // O menu do Pomodoro tem o seu proprio ciclo de mensagens, como o
            // das abas (`context_menu_comparator`): um arrasto a meio acaba
            // aqui, ou o largar do botao esquerdo ja nao chegaria a barra.
            self.forget_tab_gesture();
            if let Some(action) = bar_tool_action(hit, ToolClick::Right) {
                self.run_tool_action(action);
            }
            return;
        }
        self.context_menu_comparator();
    }

    fn right_click_home(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        if let HomeClick::Tool(tool) = home_click_target(
            (size.width as f64, size.height as f64),
            window.scale_factor(),
            self.pomodoro_bar_label(),
            self.cursor.0,
            self.cursor.1,
        ) && let Some(action) = tool_action(tool, ToolClick::Right)
        {
            self.run_tool_action(action);
        }
    }

    fn handle_panel_message(&mut self, post: side_panel::PanelPost) {
        // `receive` segue a copia do editor; um pedido de uma pagina que ja
        // saiu nao chega aqui (o texto que trazia ja foi gravado).
        let message = match self.side_panel.receive(post) {
            side_panel::Received::Current(message) => message,
            side_panel::Received::Late(saved) => {
                if let Some(Err(error)) = saved {
                    self.show_splash(error, 6);
                }
                return;
            }
        };
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
                // Aberto pelas Notas (botao, Ctrl+Shift+Z): agora a pagina
                // ja existe e o que esperava corre, pela ordem.
                for script in self.side_panel.mark_ready() {
                    self.panel_eval(&script);
                }
            }
            PanelMessage::Search(query) => self.memory.query(query),
            PanelMessage::Open(input) => {
                self.close_side_panel(PanelExit::OpenItem);
                self.handle_input(input);
            }
            PanelMessage::Close => self.close_side_panel(PanelExit::CloseButton),
            // Ja seguido por `SidePanel::receive`.
            PanelMessage::NoteDraft(_) => {}
            PanelMessage::NoteSaveRefused => self.panel_run(notes_reply_script(
                &NotesReply::Failed(NOTE_SAVE_REFUSED.to_string()),
            )),
            notes @ (PanelMessage::NotesList
            | PanelMessage::NotesSearch(_)
            | PanelMessage::NoteOpen(_)
            | PanelMessage::NoteSave(_)
            | PanelMessage::NoteDelete(_)) => {
                if let Some(command) = notes_command_for(notes) {
                    self.submit_notes(command, NotesOrigin::Panel);
                }
            }
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
        self.live_eval(&live_theme_script(&panel_theme_vars(&Theme::system())));
        let theme = ThemeChoice::current().webview_theme();
        // O tema "Sistema" do leitor de livros acompanha o tema da app.
        if self.surface == Surface::Epub
            && let Some(webview) = &self.webview
        {
            let _ = webview.set_theme(theme);
        }
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
        // A faixa e o botao do servico aberto falam do servico e do modo dele.
        if let Some(panel) = &self.service_panel
            && let Some(hint) =
                service_panel_hint(hit, panel.service, panel.state.badge(panel.audio))
        {
            return Some(hint);
        }
        let comp = self.comparator.as_ref();
        let column = match hit {
            BarHit::Column(index)
            | BarHit::AddTab(index)
            | BarHit::ColumnBack(index)
            | BarHit::ColumnForward(index)
            | BarHit::TabOverflow(index) => Some(index),
            BarHit::ContextTab { source_index, .. }
            | BarHit::CloseTab { source_index, .. }
            | BarHit::ContextGroup { source_index, .. } => Some(source_index),
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
        // O que o clique na pilula vai fazer (o mesmo `chip_click` do
        // clique): uma pilula sozinha, cujas abas ficaram fora do corte,
        // mostra-as -- nao diz "recolher".
        let group = match hit {
            BarHit::ContextGroup {
                source_index,
                group_index,
            } => {
                let drawn = self
                    .bar_layout()
                    .is_some_and(|layout| layout.group_members_drawn(source_index, group_index));
                comp.and_then(|comp| comp.groups.get(source_index))
                    .and_then(|groups| groups.get(group_index))
                    .map(|group| {
                        (
                            group.name.as_str(),
                            chip_click(group.collapsed, drawn) != ChipClick::Collapse,
                        )
                    })
            }
            _ => None,
        };
        let maximized = unsafe { IsZoomed(owner) != 0 };
        bar_hint(
            hit,
            &self.pomodoro,
            Instant::now(),
            provider,
            maximized,
            tab_url,
            group,
        )
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
        if let Some(comp) = &mut self.comparator {
            let _ = remove_context_tab(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
        }
        self.request_redraw();
    }

    /// Um clique na pilula: recolhe o grupo cujas abas estao a vista; abre o
    /// recolhido; e o grupo cujas abas ficaram fora do corte (a pilula esta
    /// sozinha, com cara de recolhida) mostra-as -- em vez de o recolher sem
    /// se ver mudanca nenhuma. Fechado, as abas continuam abertas.
    fn toggle_context_group(&mut self, source_index: usize, group_index: usize) {
        let members_drawn = self
            .bar_layout()
            .is_some_and(|layout| layout.group_members_drawn(source_index, group_index));
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
        {
            let _ = apply_chip_click(
                &comp.contexts[source_index],
                &mut comp.groups[source_index],
                &mut comp.bar_focus[source_index],
                group_index,
                members_drawn,
            );
        }
        self.request_redraw();
    }

    /// "Adicionar a um novo grupo". Como no Chrome, o grupo novo abre logo o
    /// seu editor -- o menu do grupo, com as cores, junto da pilula --: a cor
    /// escolhe-se ja, nao fica uma que ninguem escolheu.
    fn group_context_tab(&mut self, source_index: usize, context_index: usize) {
        let mut created = None;
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
        {
            let ComparatorState {
                contexts,
                groups,
                next_group_id,
                bar_focus,
                ..
            } = comp;
            let id = contexts[source_index].get(context_index).map(|tab| tab.id);
            created = regroup_context_tab(
                &mut contexts[source_index],
                &mut groups[source_index],
                next_group_id,
                context_index,
            );
            if created.is_some() {
                bar_focus[source_index] = id;
            }
        }
        self.request_redraw();
        if let Some(group_index) = created {
            self.show_group_menu(source_index, group_index, true);
        }
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
            // A aba vai para junto dos outros membros, que podem estar longe
            // das abas recentes: fica a ancora da coluna, para nao sumir.
            let moved = comp.contexts[source_index]
                .get(context_index)
                .map(|tab| tab.id);
            join_context_group(&mut comp.contexts[source_index], id, context_index);
            prune_empty_groups(&comp.contexts[source_index], &mut comp.groups[source_index]);
            if moved.is_some() {
                comp.bar_focus[source_index] = moved;
            }
        }
        self.request_redraw();
    }

    fn ungroup_context_tab(&mut self, source_index: usize, context_index: usize) {
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
        {
            let moved = comp.contexts[source_index]
                .get(context_index)
                .map(|tab| tab.id);
            leave_context_group(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
            if moved.is_some() {
                comp.bar_focus[source_index] = moved;
            }
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

    /// Botao direito na pilula de uma IA: o mesmo item de rolagem que o menu
    /// dentro da coluna oferece, para se descobrir tambem pela barra.
    fn column_pill_menu(&mut self, col_index: usize) {
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        // A dica da pilula nao fica a flutuar por cima do menu.
        hover_tooltip(std::ptr::null_mut(), "");
        // O mesmo item que o botao direito dentro da coluna, decidido pelo
        // mesmo `column_menu_responder` (menu proprio: nenhum item nativo).
        let request = column_menu_responder(col_index, self.auto_scroll.clone())(0);
        // Vive ate ao fim da funcao: o Win32 le o texto enquanto desenha.
        let label = wide_null(request.label);
        let command = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            AppendMenuW(menu, MF_STRING, request.command, label.as_ptr());
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
        if command == request.command
            && let Some(event) = request.selected()
        {
            let _ = self.proxy.send_event(event);
        }
    }

    /// O "‹N" de uma coluna: a lista de todas as abas dela, cada grupo num
    /// submenu (com a amostra da cor) com as suas. Escolher uma abre-a ao lado
    /// e fa-la ancora da coluna -- a barra passa a mostra-la, pilula incluida.
    /// Sem isto, uma aba guardada que ficasse fora do corte das mais recentes
    /// continuava no `tabs.json` sem se poder abrir, recolorir nem fechar.
    fn show_tab_list_menu(&mut self, source_index: usize) {
        use windows_sys::Win32::UI::WindowsAndMessaging::MF_CHECKED;
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        hover_tooltip(std::ptr::null_mut(), "");
        let Some((entries, count, open, colors)) = self.comparator.as_ref().and_then(|comp| {
            let tabs = comp.contexts.get(source_index)?;
            let groups = &comp.groups[source_index];
            let open = comp
                .split
                .as_ref()
                .filter(|split| split.source_index == source_index)
                .and_then(|split| split.context_id)
                .and_then(|id| tabs.iter().position(|tab| tab.id == id));
            let colors: Vec<Rgb> = groups.iter().map(|group| group.color.rgb()).collect();
            Some((tab_list_entries(tabs, groups), tabs.len(), open, colors))
        }) else {
            return;
        };
        if count == 0 {
            return;
        }
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor().max(1.0));
        let point = self
            .bar_layout()
            .map(|layout| layout.tab_overflow[source_index.min(COMPARATOR_COLUMNS - 1)])
            .filter(|button| button.width > 0.0)
            .map(|button| {
                let mut point = POINT {
                    x: button.x.round() as i32,
                    y: (button.y + button.height).round() as i32,
                };
                unsafe {
                    ClientToScreen(hwnd, &mut point);
                }
                point
            })
            .unwrap_or_else(|| self.bar_menu_point(hwnd));

        let selected = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            // Os textos e as amostras vivem ate ao fim do bloco.
            let mut texts: Vec<Vec<u16>> = Vec::new();
            let mut swatches: Vec<*mut core::ffi::c_void> = Vec::new();
            let size = (16.0 * scale).round() as i32;
            let flags = |index: usize| {
                if Some(index) == open {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                }
            };
            for entry in &entries {
                match entry {
                    TabListEntry::Tab { index, label } => {
                        texts.push(wide_null(label));
                        let text = texts.last().map_or(std::ptr::null(), |text| text.as_ptr());
                        AppendMenuW(menu, flags(*index), TAB_LIST_BASE + index, text);
                    }
                    TabListEntry::Group { group, name, tabs } => {
                        let submenu = CreatePopupMenu();
                        if submenu.is_null() {
                            continue;
                        }
                        for (index, label) in tabs {
                            texts.push(wide_null(label));
                            let text = texts.last().map_or(std::ptr::null(), |text| text.as_ptr());
                            AppendMenuW(submenu, flags(*index), TAB_LIST_BASE + index, text);
                        }
                        texts.push(wide_null(&format!("{name} ({})", tabs.len())));
                        let swatch = color_swatch_bitmap(
                            colors.get(*group).copied().unwrap_or((0, 0, 0)),
                            size,
                        );
                        if !swatch.is_null() {
                            swatches.push(swatch);
                        }
                        // O submenu passa a ser do menu e morre com ele.
                        let text = texts.last().map_or(&[0u16][..], |text| text.as_slice());
                        append_swatch_submenu(menu, submenu, text, swatch);
                    }
                }
            }
            let selected = TrackPopupMenu(
                menu,
                // Aberto pelo botao esquerdo: sem TPM_RIGHTBUTTON.
                TPM_RETURNCMD,
                point.x,
                point.y,
                0,
                hwnd,
                std::ptr::null(),
            ) as usize;
            DestroyMenu(menu);
            for swatch in swatches {
                DeleteObject(swatch as _);
            }
            drop(texts);
            selected
        };
        if let Some(index) = tab_list_command(selected, count) {
            self.open_listed_tab(source_index, index);
        }
    }

    /// Uma aba escolhida na lista "‹N": passa a ancora da coluna e abre ao
    /// lado (se ja estava aberta, fica so a ancora).
    fn open_listed_tab(&mut self, source_index: usize, context_index: usize) {
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
            && let Some(id) = comp.contexts[source_index]
                .get(context_index)
                .map(|tab| tab.id)
        {
            comp.bar_focus[source_index] = Some(id);
        }
        self.open_context_tab(source_index, context_index);
        self.request_redraw();
    }

    fn context_menu_comparator(&mut self) {
        // Um arrasto a meio acaba aqui: o menu tem o seu proprio ciclo de
        // mensagens e o largar do botao esquerdo ja nao chegaria a barra.
        self.forget_tab_gesture();
        hover_tooltip(std::ptr::null_mut(), "");
        match bar_menu_for(self.comparator_bar_hit()) {
            Some(BarMenu::Tab {
                source_index,
                context_index,
            }) => self.show_tab_menu(source_index, context_index),
            Some(BarMenu::Group {
                source_index,
                group_index,
            }) => self.show_group_menu(source_index, group_index, false),
            Some(BarMenu::Column(col_index)) => self.column_pill_menu(col_index),
            None => {}
        }
    }

    /// Onde o menu do botao direito abre: no cursor, em coordenadas de ecra.
    fn bar_menu_point(&self, hwnd: HWND) -> POINT {
        let mut point = POINT {
            x: self.cursor.0.round() as i32,
            y: self.cursor.1.round() as i32,
        };
        unsafe {
            ClientToScreen(hwnd, &mut point);
        }
        point
    }

    fn show_tab_menu(&mut self, source_index: usize, context_index: usize) {
        use windows_sys::Win32::UI::WindowsAndMessaging::MF_POPUP;
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor().max(1.0));

        // Lidos antes de abrir o menu: dentro do bloco `unsafe` ja nao ha
        // emprestimo do estado que sobreviva ao `TrackPopupMenu`.
        let Some((joinable, colors, own_group)) = self.comparator.as_ref().and_then(|comp| {
            let tab = comp.contexts.get(source_index)?.get(context_index)?;
            let groups = &comp.groups[source_index];
            let joinable = Self::joinable_context_groups(groups, tab.group);
            let colors: Vec<Rgb> = joinable
                .iter()
                .map(|(index, _)| groups.get(*index).map_or((0, 0, 0), |g| g.color.rgb()))
                .collect();
            // O grupo da aba (id e cor atual), para o submenu "Cor do grupo".
            let own_group = tab
                .group
                .and_then(|id| groups.iter().find(|group| group.id == id))
                .map(|group| (group.id, group.color));
            Some((joinable, colors, own_group))
        }) else {
            return;
        };
        let in_group = own_group.is_some();
        let point = self.bar_menu_point(hwnd);

        let selected = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            // Os textos vivem ate ao fim do bloco: o menu so os le enquanto
            // esta aberto.
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
            let new_group = wide_null("Adicionar a um novo grupo");
            let move_to = wide_null("Mover para o grupo");
            let ungroup = wide_null("Remover do grupo");
            let group_color = wide_null("Cor do grupo");
            let color_labels: Vec<Vec<u16>> = GroupColor::ALL
                .iter()
                .map(|color| wide_null(group_color_label(*color)))
                .collect();
            let join_labels: Vec<Vec<u16>> =
                joinable.iter().map(|(_, name)| wide_null(name)).collect();

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
            AppendMenuW(menu, MF_STRING, TAB_MENU_NEW_GROUP, new_group.as_ptr());
            // "Mover para o grupo ▸": um submenu com os outros grupos da
            // coluna, cada um com a amostra da sua cor, como no Chrome.
            let mut swatches: Vec<*mut core::ffi::c_void> = Vec::new();
            if !joinable.is_empty() {
                let submenu = CreatePopupMenu();
                if !submenu.is_null() {
                    let size = (16.0 * scale).round() as i32;
                    for (offset, label) in join_labels.iter().enumerate() {
                        let swatch = color_swatch_bitmap(colors[offset], size);
                        if !swatch.is_null() {
                            swatches.push(swatch);
                        }
                        append_swatch_item(
                            submenu,
                            TAB_MENU_GROUP_BASE + offset,
                            label,
                            swatch,
                            false,
                        );
                    }
                    // MF_POPUP: o submenu passa a ser do menu e morre com ele.
                    AppendMenuW(menu, MF_POPUP, submenu as usize, move_to.as_ptr());
                }
            }
            if in_group {
                AppendMenuW(menu, MF_STRING, TAB_MENU_UNGROUP, ungroup.as_ptr());
            }
            // "Cor do grupo ▸" numa aba agrupada: a cor muda-se tambem onde
            // estao as abas, nao so na pilula -- as mesmas amostras e ids do
            // menu do grupo, com a atual marcada.
            if let Some((_, current)) = own_group {
                let submenu = CreatePopupMenu();
                if !submenu.is_null() {
                    let size = (16.0 * scale).round() as i32;
                    for (index, color) in GroupColor::ALL.iter().enumerate() {
                        let swatch = color_swatch_bitmap(color.rgb(), size);
                        if !swatch.is_null() {
                            swatches.push(swatch);
                        }
                        append_swatch_item(
                            submenu,
                            GROUP_MENU_COLOR_BASE + index,
                            &color_labels[index],
                            swatch,
                            *color == current,
                        );
                    }
                    AppendMenuW(menu, MF_POPUP, submenu as usize, group_color.as_ptr());
                }
            }

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
            // Os bitmaps dos itens nao sao do menu: apagam-se depois dele.
            for swatch in swatches {
                DeleteObject(swatch as _);
            }
            selected
        };

        match tab_menu_command(selected, &joinable) {
            Some(TabMenuCommand::Open) => {
                self.open_context_tab(source_index, context_index);
            }
            Some(TabMenuCommand::Fullscreen) => {
                self.open_context_tab_fullscreen(source_index, context_index)
            }
            Some(TabMenuCommand::Close) => self.close_context_tab(source_index, context_index),
            Some(TabMenuCommand::CloseOthers) => {
                self.close_other_context_tabs(source_index, context_index)
            }
            Some(TabMenuCommand::CloseAll) => {
                self.close_all_context_tabs(source_index, context_index)
            }
            Some(TabMenuCommand::NewGroup) => self.group_context_tab(source_index, context_index),
            Some(TabMenuCommand::Ungroup) => self.ungroup_context_tab(source_index, context_index),
            Some(TabMenuCommand::MoveToGroup(group_index)) => {
                self.join_context_tab_group(source_index, context_index, group_index)
            }
            Some(TabMenuCommand::GroupColor(color)) => {
                if let Some((group_id, _)) = own_group {
                    self.apply_group_menu(source_index, group_id, GroupMenuCommand::Color(color));
                }
            }
            None => {}
        }
    }

    /// Por baixo da pilula `group_index` da coluna, em coordenadas de ecra:
    /// onde o Chrome abre o editor de um grupo acabado de criar.
    fn group_chip_point(
        &self,
        hwnd: HWND,
        source_index: usize,
        group_index: usize,
    ) -> Option<POINT> {
        let layout = self.bar_layout()?;
        let visual = (0..layout.group_pill_counts.get(source_index).copied()?)
            .find(|visual| layout.group_pill_indices[source_index][*visual] == group_index)?;
        let chip = layout.group_pills[source_index][visual];
        let mut point = POINT {
            x: chip.x.round() as i32,
            y: (chip.y + chip.height).round() as i32,
        };
        unsafe {
            ClientToScreen(hwnd, &mut point);
        }
        Some(point)
    }

    /// Botao direito na pilula de um grupo: a cor (com a atual marcada),
    /// recolher/expandir, desagrupar e fechar -- o menu do grupo do Chrome.
    /// `at_chip`: abre por baixo da pilula (o grupo acabou de nascer), nao no
    /// rato.
    fn show_group_menu(&mut self, source_index: usize, group_index: usize, at_chip: bool) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MF_DISABLED, MF_GRAYED};
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor().max(1.0));
        let Some((group_id, name, current, collapsed)) = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.groups.get(source_index))
            .and_then(|groups| groups.get(group_index))
            .map(|group| (group.id, group.name.clone(), group.color, group.collapsed))
        else {
            return;
        };
        let point = at_chip
            .then(|| self.group_chip_point(hwnd, source_index, group_index))
            .flatten()
            .unwrap_or_else(|| self.bar_menu_point(hwnd));

        let selected = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            let title = wide_null(&format!("Grupo \u{201C}{name}\u{201D}"));
            let toggle = wide_null(if collapsed {
                "Expandir grupo"
            } else {
                "Recolher grupo"
            });
            let ungroup = wide_null("Desagrupar");
            let close = wide_null("Fechar grupo");
            let color_labels: Vec<Vec<u16>> = GroupColor::ALL
                .iter()
                .map(|color| wide_null(group_color_label(*color)))
                .collect();

            AppendMenuW(menu, MF_STRING | MF_DISABLED | MF_GRAYED, 0, title.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            let size = (16.0 * scale).round() as i32;
            let mut swatches: Vec<*mut core::ffi::c_void> = Vec::new();
            for (index, color) in GroupColor::ALL.iter().enumerate() {
                let swatch = color_swatch_bitmap(color.rgb(), size);
                if !swatch.is_null() {
                    swatches.push(swatch);
                }
                append_swatch_item(
                    menu,
                    GROUP_MENU_COLOR_BASE + index,
                    &color_labels[index],
                    swatch,
                    *color == current,
                );
            }
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, GROUP_MENU_TOGGLE, toggle.as_ptr());
            AppendMenuW(menu, MF_STRING, GROUP_MENU_UNGROUP, ungroup.as_ptr());
            AppendMenuW(menu, MF_STRING, GROUP_MENU_CLOSE, close.as_ptr());

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
            for swatch in swatches {
                DeleteObject(swatch as _);
            }
            selected
        };

        if let Some(command) = group_menu_command(selected) {
            self.apply_group_menu(source_index, group_id, command);
        }
    }

    /// Executa um comando do menu do grupo. Se fechar a aba que esta aberta
    /// ao lado, a gaveta fecha com ela.
    fn apply_group_menu(&mut self, source_index: usize, group_id: u64, command: GroupMenuCommand) {
        let Some(comp) = &mut self.comparator else {
            return;
        };
        if source_index >= COMPARATOR_COLUMNS {
            return;
        }
        let closed = apply_group_command(
            &mut comp.contexts[source_index],
            &mut comp.groups[source_index],
            group_id,
            command,
        );
        let closes_active = comp.split.as_ref().is_some_and(|split| {
            split.source_index == source_index
                && split.context_id.is_some_and(|id| closed.contains(&id))
        });
        if closes_active {
            self.close_split();
        }
        self.request_redraw();
    }

    /// Botao esquerdo em baixo na barra. Abas, o x delas e as pilulas dos
    /// grupos so decidem ao largar (podem virar arrasto); o resto responde ja.
    fn press_comparator(&mut self) {
        let hit = self.comparator_bar_hit();
        self.tab_gesture_count = self.tab_gesture_count.wrapping_add(1).max(1);
        let gesture = self.tab_gesture_count;
        self.tab_press = match (hit, self.bar_layout(), &self.comparator) {
            (Some(hit), Some(layout), Some(comp)) => tab_press(
                &layout,
                &comp.contexts,
                &comp.groups,
                hit,
                self.cursor,
                gesture,
            ),
            _ => None,
        };
        self.publish_tab_gesture();
        if self.tab_press.is_some() {
            hover_tooltip(std::ptr::null_mut(), "");
            return;
        }
        self.click_comparator(hit);
    }

    /// Diz ao subclass da janela que gesto tem o rato (0: nenhum).
    fn publish_tab_gesture(&self) {
        TAB_GESTURE_LIVE.store(
            self.tab_press.map_or(0, |press| press.gesture),
            Ordering::Release,
        );
    }

    /// Esquece o gesto da fila sem fazer nada com ele (a superficie mudou por
    /// baixo dele). O botao que ainda estiver em baixo solta o rato sozinho.
    fn forget_tab_gesture(&mut self) {
        self.tab_press = None;
        self.publish_tab_gesture();
    }

    /// Um passo do gesto sobre a fila de abas, contra a barra e o modelo de
    /// agora (`tab_gesture_step`). O arrasto so mexe na ordem e nos grupos
    /// das abas: nenhuma pagina navega nem recarrega, e a aba aberta ao lado
    /// continua aberta, porque e reconhecida pela identidade e nao pelo sitio.
    fn tab_gesture(&mut self, input: TabGestureInput) -> TabGestureEffect {
        if self.tab_press.is_none() {
            return TabGestureEffect::Ignored;
        }
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor().max(1.0));
        let layout = self.bar_layout();
        let effect = match (layout, &self.comparator) {
            (Some(layout), Some(comp)) => tab_gesture_step(
                &mut self.tab_press,
                input,
                &TabRowView {
                    layout: &layout,
                    contexts: &comp.contexts,
                    groups: &comp.groups,
                    scale,
                },
            ),
            // Sem comparador nao ha fila: o gesto que ficou para tras acaba.
            _ => TabGestureEffect::Cancelled {
                was_dragging: self.tab_press.take().is_some_and(|press| press.dragging),
            },
        };
        // O gesto sai de publicacao ANTES de o rato ser solto: o
        // WM_CAPTURECHANGED que o ReleaseCapture manda ja nao encontra nada.
        self.publish_tab_gesture();
        if matches!(input, TabGestureInput::Release { .. }) {
            self.release_tab_capture();
        }
        match effect {
            TabGestureEffect::Started => {
                self.hold_tab_capture();
                self.bar_hover = None;
                hover_tooltip(std::ptr::null_mut(), "");
                self.request_redraw();
            }
            TabGestureEffect::Moved | TabGestureEffect::Cancelled { was_dragging: true } => {
                self.request_redraw();
            }
            TabGestureEffect::Click(hit) => self.click_comparator(Some(hit)),
            TabGestureEffect::Drop { .. } => {
                if let Some(comp) = &mut self.comparator {
                    let _ = apply_tab_gesture(
                        &mut comp.contexts,
                        &mut comp.groups,
                        &mut comp.bar_focus,
                        effect,
                    );
                }
                self.request_redraw();
            }
            TabGestureEffect::Cancelled { .. }
            | TabGestureEffect::Pending
            | TabGestureEffect::Ignored => {}
        }
        effect
    }

    /// Enquanto se arrasta, o rato e desta janela mesmo fora dela: o largar
    /// chega sempre aqui, e largar fora da fila cancela. O winit ja prende o
    /// rato ao premir; isto garante-o se alguem o tiver soltado entretanto.
    fn hold_tab_capture(&self) {
        if let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) {
            unsafe {
                if GetCapture() != hwnd {
                    SetCapture(hwnd);
                }
            }
        }
    }

    /// Solta o rato no fim do gesto -- sempre DEPOIS de o gesto ter sido
    /// tirado, que o ReleaseCapture manda o WM_CAPTURECHANGED na hora.
    fn release_tab_capture(&self) {
        if let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) {
            unsafe {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
            }
        }
    }

    /// Botao esquerdo largado: completa o clique, larga o arrasto ou nada.
    fn release_comparator(&mut self) {
        if self.tab_press.is_none() {
            return;
        }
        if self.surface != Surface::Comparator {
            self.forget_tab_gesture();
            return;
        }
        let hit = self.comparator_bar_hit();
        let _ = self.tab_gesture(TabGestureInput::Release {
            cursor: self.cursor,
            hit,
        });
        self.update_bar_hover();
        self.request_redraw();
    }

    /// Esc -- da janela, ou o "voltar" que a pagina manda quando o teclado
    /// esta nela: a meio de um arrasto cancela-o; fora dele e o "voltar".
    fn escape_or_back(&mut self) {
        // Com o painel de servicos em tela cheia, o Esc so sai da tela cheia
        // -- venha de onde vier (o teclado pode ter ficado numa coluna, por
        // baixo do painel).
        if self.service_covers_window() {
            self.service_input(ServiceInput::Escape);
            return;
        }
        if self.tab_gesture(TabGestureInput::Escape) == TabGestureEffect::Ignored {
            self.go_back();
        }
    }

    /// O que a barra desenha do arrasto em curso, se houver um.
    fn drag_paint(&self) -> Option<DragPaint> {
        let press = self.tab_press.filter(|press| press.dragging)?;
        let layout = self.bar_layout()?;
        let comp = self.comparator.as_ref()?;
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor().max(1.0));
        tab_drag_paint(
            press,
            &TabRowView {
                layout: &layout,
                contexts: &comp.contexts,
                groups: &comp.groups,
                scale,
            },
            self.cursor,
        )
    }

    /// O que um clique no alvo `hit` da barra faz. Os botoes respondem ao
    /// premir; abas, o x e as pilulas chegam aqui ao largar, pelo
    /// `release_comparator`.
    fn click_comparator(&mut self, hit: Option<BarHit>) {
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
            Some(BarHit::ServiceStrip(button)) => self.service_input(button.input()),
            Some(BarHit::GmailToggle) => self.toggle_gmail_notifications(),
            Some(BarHit::Tool(_)) => {
                if let Some(action) = bar_tool_action(hit, ToolClick::Left) {
                    self.run_tool_action(action);
                }
            }
            Some(BarHit::GeminiLive) => self.toggle_live_panel(),
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
            Some(BarHit::CloseTab {
                source_index,
                context_index,
            }) => self.close_context_tab(source_index, context_index),
            Some(BarHit::ContextGroup {
                source_index,
                group_index,
            }) => self.toggle_context_group(source_index, group_index),
            Some(BarHit::TabOverflow(index)) => self.show_tab_list_menu(index),
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
        let (x, y) = self.cursor;
        match home_click_target(
            (size.width as f64, size.height as f64),
            window.scale_factor(),
            self.pomodoro_bar_label(),
            x,
            y,
        ) {
            HomeClick::Tool(tool) => {
                if let Some(action) = tool_action(tool, ToolClick::Left) {
                    self.run_tool_action(action);
                }
            }
            // Sem a barra do Windows, a faixa de cima arrasta a janela -- como
            // a barra do comparador.
            HomeClick::Drag => {
                let _ = window.drag_window();
            }
            HomeClick::Go => {
                debug_log(format_args!("click_home: botao Ir"));
                self.submit_current();
            }
            HomeClick::Nothing => {}
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
    /// Ctrl+Shift+Z sem pagina com selecao: nota nova no painel.
    NewNote,
    /// Ctrl+O: o dialogo de livros EPUB. So nativo: o mapa de teclas das
    /// paginas (`NEURALIA_KEYMAP_SCRIPT`) nao o conhece, para nao nascer uma
    /// accao IPC nova no canal das paginas remotas.
    OpenEpub,
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
        ("o", false) => Some(MainShortcut::OpenEpub),
        ("z", true) => Some(MainShortcut::NewNote),
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

fn comparator_split_key(comp: &ComparatorState) -> Option<(usize, Option<u64>, bool)> {
    comp.split
        .as_ref()
        .map(|split| (split.source_index, split.context_id, split.private))
}

/// Que aba de contexto uma fonte aberta ao lado passa a ser. Uma fonte
/// privada nao entra na lista -- e por isso nunca chega ao `tabs.json`, nem
/// ao ecra da proxima sessao; reabrir uma aba ja gravada reutiliza a sua
/// identidade.
fn record_split_context(
    contexts: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    next_context_id: &mut u64,
    url: String,
    private: bool,
    existing_context_id: Option<u64>,
    opener: Option<u64>,
) -> Option<u64> {
    if private {
        None
    } else if let Some(id) = existing_context_id {
        Some(id)
    } else if !tab_session::storable_url(&url) {
        // Um endereco que o `tabs.json` nao guardaria (grande demais) nao vira
        // aba: a fonte abre ao lado na mesma, mas a barra e o ficheiro nunca
        // discordam -- antes a aba aparecia e sumia sem aviso no reinicio,
        // levando o grupo que so ela tinha.
        None
    } else {
        Some(remember_context_tab(
            contexts,
            groups,
            next_context_id,
            url,
            opener,
        ))
    }
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
    /// `pomodoro:`, `pomodoro:pausar`, `pomodoro:50`... (None: palavra
    /// desconhecida -> a ajuda).
    Pomodoro(Option<PomodoroCommand>),
    ResearchCompare,
    ResearchSynthesize,
    ResearchExport,
    /// `traduzir:<texto>`: o texto nas tres IAs com o pedido fixo de
    /// traducao (`CompareRequest::translate`) -- e o que o Historico guarda de
    /// um Traduzir da barra. None: sem texto -> a ajuda.
    Translate(Option<String>),
    /// `livros:` (e `biblioteca:`, `books:`, `library:`): a biblioteca.
    Library,
    /// `epub:` sozinho abre o diálogo de arquivos; `epub:<caminho>` abre esse
    /// arquivo.
    OpenEpub(Option<PathBuf>),
    /// Sem comando próprio: segue para o `parse_intent`.
    Intent,
}

/// A ajuda de um `traduzir:` sem texto.
const TRANSLATE_COMMAND_HELP: &str = "Escreva o texto depois de traduzir:";

/// `text` sem `prefix` a frente, se comecar por ele (maiusculas ou nao).
fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
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
        ("livros:", InputRoute::Library),
        ("biblioteca:", InputRoute::Library),
        ("books:", InputRoute::Library),
        ("library:", InputRoute::Library),
        ("!livros", InputRoute::Library),
        ("!books", InputRoute::Library),
    ] {
        if trimmed.eq_ignore_ascii_case(command) {
            return route;
        }
    }

    // `epub:` (ou `!epub`) sozinho abre o diálogo; com um caminho à frente
    // (aspas do "Copiar como caminho" do Explorer aceites) abre esse arquivo.
    for prefix in ["epub:", "!epub"] {
        let Some(rest) = trimmed
            .get(..prefix.len())
            .filter(|head| head.eq_ignore_ascii_case(prefix))
            .map(|_| &trimmed[prefix.len()..])
        else {
            continue;
        };
        if prefix.starts_with('!') && !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let path = rest.trim().trim_matches('"').trim();
        return InputRoute::OpenEpub((!path.is_empty()).then(|| PathBuf::from(path)));
    }

    if let Some(word) = trimmed
        .strip_prefix("tema:")
        .or_else(|| trimmed.strip_prefix("theme:"))
    {
        return InputRoute::Theme(ThemeChoice::parse(word));
    }
    // Sem distinguir maiusculas, como os `research:`. Os dois pontos sao
    // obrigatorios, como no `tema:`: "pomodoro tecnica" continua a ser uma
    // pesquisa sobre o metodo.
    if let Some(word) = strip_prefix_ignore_ascii_case(trimmed, "pomodoro:") {
        return InputRoute::Pomodoro(parse_pomodoro_command(word));
    }
    if let Some(text) = strip_prefix_ignore_ascii_case(trimmed, TRANSLATE_COMMAND) {
        let text = text.trim();
        return InputRoute::Translate((!text.is_empty()).then(|| text.to_string()));
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

/// As pecas do `App` que o tique do Pomodoro usa (`pomodoro_ui::pomodoro_tick`).
impl PomodoroHost for App {
    type Timers = Timers;

    fn pomodoro_parts(&mut self) -> (&mut PomodoroController, &Timers) {
        (&mut self.pomodoro, &self.timers)
    }

    fn attention(&self) -> WindowAttention {
        self.window_attention()
    }

    fn chime(&mut self) {
        pomodoro_sound();
    }

    fn flash_taskbar(&mut self) {
        if let Some(owner) = self.window.as_ref().and_then(window_hwnd) {
            flash_taskbar(owner);
        }
    }

    fn notice(&mut self, message: String) {
        self.show_background_splash(message, POMODORO_PHASE_END_SECONDS);
    }

    fn repaint(&mut self) {
        self.pomodoro_changed();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        match event_loop.create_window(main_window_attributes()) {
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

        // Os `DroppedFile` de um mesmo gesto chegam no mesmo lote: seguem
        // juntos, e so o ultimo livro adicionado abre.
        if !self.pending_drops.is_empty() {
            let dropped = std::mem::take(&mut self.pending_drops);
            self.route_dropped_files(dropped);
        }

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

        self.observe_tab_session();
    }

    /// Fechar a janela com o comparador aberto: as abas ficam gravadas.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        let _ = self.save_tab_session();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ExitRequested => {
                self.save_notes_draft_before_exit();
                event_loop.exit();
            }
            UserEvent::SaveTabSession(token) => self.save_due_tab_session(token),
            UserEvent::HomeRequested => self.show_home(),
            UserEvent::BackRequested => self.escape_or_back(),
            UserEvent::TabCaptureLost(gesture) => {
                let _ = self.tab_gesture(TabGestureInput::CaptureLost { gesture });
            }
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
            UserEvent::PomodoroTick(token) => self.pomodoro_tick(token),
            UserEvent::HideSplash(token) => self.hide_splash(token),
            UserEvent::CaptionReveal => self.refresh_caption_reveal(),
            UserEvent::ResizePanel => self.resize_panel(),
            UserEvent::PanelResizeDone => self.save_panel_widths(),
            UserEvent::ServiceFullscreen { generation, on } => {
                if self.service_event_is_current(generation) {
                    self.service_input(ServiceInput::PageFullscreen(on));
                }
            }
            UserEvent::ServiceAudio {
                generation,
                playing,
            } => {
                if let Some(panel) = self
                    .service_panel
                    .as_mut()
                    .filter(|panel| panel.generation == generation)
                {
                    panel.audio = playing;
                    self.request_redraw();
                }
            }
            UserEvent::ServiceEscape(generation) => {
                if self.service_event_is_current(generation) {
                    self.service_input(ServiceInput::Escape);
                }
            }
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
            UserEvent::Panel(post) => self.handle_panel_message(post),
            UserEvent::NotesReady { origin, reply } => self.notes_ready(origin, reply),
            UserEvent::NoteRequested { target, via } => self.request_note_from_page(target, via),
            UserEvent::NoteRefusedPrivate => {
                self.show_splash(NOTE_PRIVATE_REFUSAL.to_string(), 3);
            }
            UserEvent::NoteCaptured { raw, source } => {
                self.note_captured(&raw, source.as_deref());
            }
            UserEvent::NewNote => self.new_note_in_panel(),
            UserEvent::Live(message) => self.handle_live_message(message),
            UserEvent::GmailAnswer(open) => self.answer_gmail(open),
            UserEvent::ClearHistory => {
                if !self.confirm_clear_history() {
                    return;
                }
                self.forget_tab_session();
                self.memory.clear(&mut self.current_research);
                // Na biblioteca de livros, some quando cada livro foi aberto
                // ("Continuar lendo", recentes); posições e marcadores ficam.
                if self.epub.is_some()
                    || self
                        .config
                        .data_dir
                        .join("library")
                        .join(neural_core::library::INDEX_FILE)
                        .exists()
                {
                    self.submit_epub_job(EpubJob::ClearReadingHistory);
                }
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
                if self.side_panel.is_open() {
                    self.panel_show_history(result);
                } else {
                    self.show_history_entries(result);
                }
            }
            UserEvent::HistoryWriteFailed(error) => {
                self.show_splash(format!("Histórico não foi gravado: {error}"), 4);
            }
            UserEvent::MemoryQueryReady { query, result } => {
                if self.side_panel.is_open() {
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
            UserEvent::SearchSelection { text, intent } => {
                self.search_card_event(SearchCardInput::Request { text, intent })
            }
            UserEvent::SearchCardAnswer {
                token,
                button,
                shown,
            } => self.search_card_event(SearchCardInput::Answer {
                token,
                button,
                shown,
            }),
            UserEvent::SearchCardExpired(token) => {
                self.search_card_event(SearchCardInput::Expire(token))
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
            UserEvent::OpenSplitFromSplit { source_index, url } => {
                self.open_split_from_split(source_index, url);
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
            UserEvent::ColumnHint { col, hint } => self.show_column_hint(col, hint),
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
                    self.refresh_caption_reveal();
                    self.needs_clear = true;
                    self.position_omnibox();
                    self.request_redraw();
                    if lifecycle_probe_enabled() {
                        LIFECYCLE_HOME_READY.store(true, Ordering::Release);
                    }
                }
            }
            UserEvent::RestoreComparator => {
                if self.service_frame().is_some_and(|frame| frame.exit_button) {
                    self.service_input(ServiceInput::ToggleFullscreen);
                } else if self.surface == Surface::Comparator {
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
            UserEvent::EpubNotice(notice) => self.handle_epub_notice(notice),
            UserEvent::EpubUi(request) => self.handle_epub_ui(request),
            UserEvent::EpubDropped(paths) => {
                if self.surface == Surface::Epub {
                    self.route_dropped_files(paths);
                }
            }
            UserEvent::OpenEpubDialog => self.open_epub_dialog(true),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.save_notes_draft_before_exit();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if self.needs_clear {
                    self.clear_client();
                    self.needs_clear = false;
                }
                match self.surface {
                    Surface::Home => {
                        if let Some(window) = &self.window {
                            draw_home(
                                window,
                                self.status.as_deref(),
                                self.home_go_hover,
                                self.home_tool_hover,
                                self.pomodoro_bar_label(),
                            );
                        }
                    }
                    Surface::Comparator => {
                        let drag = self.drag_paint();
                        if let Some(window) = &self.window
                            && let Some(comp) = &self.comparator
                        {
                            let service = self.service_panel.as_ref().map(|panel| {
                                (
                                    panel.service,
                                    panel.state.badge(panel.audio),
                                    self.service_strip_physical(),
                                )
                            });
                            draw_comparator_bar(
                                window,
                                comp,
                                self.bar_hover,
                                self.bar_visible(),
                                self.auto_scroll.get(),
                                drag,
                                self.pomodoro_bar_label(),
                                &self.live_panel,
                            );
                            if let Some((service, badge, strip)) = service {
                                draw_service_chrome(
                                    window,
                                    comp.split.is_some(),
                                    service,
                                    badge,
                                    strip,
                                    self.bar_hover,
                                );
                            }
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
                self.position_live_panel();
                self.after_panel_change();
                self.position_search_card();
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
                self.position_search_card();
                self.position_exit_button();
                self.position_palette();
                self.sync_comparator_splitters();
                self.after_panel_change();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                // A arrastar uma aba ou um grupo, a barra so redesenha a fila
                // reordenada; o realce e as dicas esperam pelo largar.
                let dragging = self.surface == Surface::Comparator
                    && matches!(
                        self.tab_gesture(TabGestureInput::Move {
                            cursor: self.cursor,
                            button_down: left_button_down(),
                        }),
                        TabGestureEffect::Started | TabGestureEffect::Moved
                    );
                if !dragging && self.surface == Surface::Comparator && self.bar_visible() {
                    self.update_bar_hover();
                }
                self.update_home_go_hover();
                self.refresh_caption_reveal();
                self.update_home_tool_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = (-1.0, -1.0);
                if self.surface == Surface::Comparator {
                    self.update_bar_hover();
                }
                self.update_home_go_hover();
                self.refresh_caption_reveal();
                self.update_home_tool_hover();
            }
            // Um evento por arquivo; o lote inteiro segue no `about_to_wait`.
            WindowEvent::DroppedFile(path) => self.pending_drops.push(path),
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
                Surface::Comparator => self.press_comparator(),
                _ => {}
            },
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => self.release_comparator(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => match self.surface {
                Surface::Comparator => self.right_click_comparator(),
                Surface::Home => self.right_click_home(),
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                if let Some(shortcut) = main_window_shortcut(&event.logical_key, self.modifiers) {
                    match shortcut {
                        MainShortcut::AutoScroll => self.toggle_auto_scroll(),
                        MainShortcut::Reload => self.reload_page(),
                        MainShortcut::History => self.toggle_side_panel(),
                        MainShortcut::NewTab => self.new_tab(0),
                        MainShortcut::OpenEpub => self.open_epub_dialog(true),
                        MainShortcut::NewNote => self.new_note_in_panel(),
                    }
                    return;
                }
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => self.escape_or_back(),
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

/// O que a interface faz com um aviso do worker de livros, conforme a
/// superfície em que a pessoa está quando ele chega.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EpubNoticePlan {
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

fn plan_epub_notice(notice: &EpubNotice, surface: Surface) -> EpubNoticePlan {
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
fn epub_serve_job(
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
fn epub_http_response(response: EpubResponse) -> HttpResponse<Cow<'static, [u8]>> {
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
const EPUB_DIALOG_BUFFER: usize = 64 * 1024;

/// O dialogo "Abrir" do Windows, so com `*.epub` e selecao multipla. Modal
/// sobre a janela principal; devolve os caminhos escolhidos (nenhum se a
/// pessoa cancelou).
fn pick_epub_files(owner: HWND) -> Vec<PathBuf> {
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
        // A WebView unica (web externa, Leitor, PDF). As colunas e o Split
        // tratam o `note` antes de chegar aqui.
        IpcAction::Note { via } => UserEvent::NoteRequested { target: None, via },
        // Colunas, Split normal, Web externa, Reader e PDF. O Split privado
        // recusa antes de chegar aqui (`split_ipc_event_impl`).
        IpcAction::Search { text, intent } => UserEvent::SearchSelection { text, intent },
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

struct PendingSearch {
    token: u64,
    intent: SearchIntent,
    question: String,
    shown_at: Instant,
}

/// Um cartao de cada vez: pendente -> confirmado, cancelado, expirado ou
/// trocado por um pedido novo.
#[derive(Default)]
struct SearchCard {
    pending: Option<PendingSearch>,
    last_token: u64,
}

impl SearchCard {
    fn step(&mut self, input: SearchCardInput, now: Instant) -> SearchCardOutcome {
        match input {
            SearchCardInput::Request { text, intent } => {
                let Some(SelectionSearch::Compare(question)) = selection_search(&text) else {
                    return SearchCardOutcome::Ignored;
                };
                self.last_token = self.last_token.wrapping_add(1).max(1);
                let token = self.last_token;
                let text = question.clone();
                let replaced = self
                    .pending
                    .replace(PendingSearch {
                        token,
                        intent,
                        question,
                        shown_at: now,
                    })
                    .is_some();
                SearchCardOutcome::Show {
                    token,
                    intent,
                    text,
                    replaced,
                }
            }
            SearchCardInput::Answer {
                token,
                button,
                shown,
            } => {
                let Some(pending) = self.pending.take_if(|pending| pending.token == token) else {
                    return SearchCardOutcome::Ignored;
                };
                // O que o cartao pintou deste texto: o resto nao se viu e nao
                // vai, por mais que a pagina o tenha posto la.
                let seen = search_card_shown(&pending.question, shown);
                match button {
                    SearchCardButton::Cancel => SearchCardOutcome::Cancelled,
                    SearchCardButton::Confirm
                        if !seen.is_empty()
                            && now.saturating_duration_since(pending.shown_at)
                                >= SEARCH_CARD_ARM =>
                    {
                        SearchCardOutcome::Confirmed(CompareRequest::selection(
                            pending.intent,
                            seen,
                        ))
                    }
                    SearchCardButton::Confirm => {
                        // Cedo demais, ou nada a vista: o cartao fica, a
                        // espera de um clique a serio.
                        self.pending = Some(pending);
                        SearchCardOutcome::Ignored
                    }
                }
            }
            SearchCardInput::Expire(token) => {
                if self
                    .pending
                    .take_if(|pending| pending.token == token)
                    .is_some()
                {
                    SearchCardOutcome::Expired
                } else {
                    SearchCardOutcome::Ignored
                }
            }
        }
    }
}

/// Quem executa o cartao: o App no produto, um registo nos gates.
trait SearchCardHost {
    fn show_search_card(&mut self, token: u64, intent: SearchIntent, text: &str);
    fn hide_search_card(&mut self);
    fn expire_search_card_after(&mut self, token: u64, delay: Duration);
    fn compare_selection(&mut self, request: CompareRequest);
}

fn apply_search_card(host: &mut impl SearchCardHost, outcome: SearchCardOutcome) {
    match outcome {
        SearchCardOutcome::Show {
            token,
            intent,
            text,
            ..
        } => {
            host.show_search_card(token, intent, &text);
            host.expire_search_card_after(token, Duration::from_secs(SEARCH_CARD_SECONDS));
        }
        SearchCardOutcome::Confirmed(request) => {
            host.hide_search_card();
            host.compare_selection(request);
        }
        SearchCardOutcome::Cancelled | SearchCardOutcome::Expired => host.hide_search_card(),
        SearchCardOutcome::Ignored => {}
    }
}

impl SearchCardHost for App {
    fn show_search_card(&mut self, token: u64, intent: SearchIntent, text: &str) {
        if let Ok(mut view) = SEARCH_CARD_VIEW.lock() {
            *view = Some((token, intent, text.to_string()));
        }
        // Um clique a meio no cartao anterior nao passa para o novo.
        SEARCH_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
        if self.search_card_popup.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let Some(owner) = window_hwnd(window) else {
                return;
            };
            let scale = window.scale_factor().max(1.0);
            let width = (SEARCH_CARD_WIDTH * scale).round() as i32;
            let height = (SEARCH_CARD_HEIGHT * scale).round() as i32;
            unsafe {
                // Owned pela janela principal, como o splash e o aviso do
                // Gmail: acima do WebView2 e da pagina, nao acima das outras
                // aplicacoes; nasce invisivel e sem ativacao.
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
                    Some(search_card_subclass),
                    SEARCH_CARD_SUBCLASS_ID,
                    (&*self.search_card_sink as *const SearchCardSink) as usize,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let corner = (18.0 * scale).round() as i32;
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, corner, corner);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.search_card_popup = Some(created);
            }
        }
        self.position_search_card();
    }

    fn hide_search_card(&mut self) {
        if let Some(card) = self.search_card_popup.take() {
            unsafe {
                DestroyWindow(card);
            }
        }
        if let Ok(mut view) = SEARCH_CARD_VIEW.lock() {
            *view = None;
        }
        if let Ok(mut painted) = SEARCH_CARD_PAINTED.lock() {
            *painted = (0, 0);
        }
        SEARCH_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
    }

    fn expire_search_card_after(&mut self, token: u64, delay: Duration) {
        self.timers
            .after(delay, UserEvent::SearchCardExpired(token));
    }

    fn compare_selection(&mut self, request: CompareRequest) {
        self.compare(request);
    }
}

/// O mapa de teclas (e a barra de selecao que vive nele) com a capability e
/// o sinal de superficie privada postos. Um painel privado nao mostra o
/// "Mandar para IA" nem o "Traduzir": o texto dele nao pode ir parar ao
/// historico nem a memoria.
fn bind_page_script(script: &str, capability: &str, private: bool) -> String {
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

/// O script da Web externa, tal como `external_webview_builder` o injeta.
fn external_init_script(capability: &str, agent_enabled: bool) -> String {
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

/// O painel Split como o builder o monta. O script injetado, o mapa IPC e o
/// perfil anonimo saem TODOS do mesmo `private`: o builder recebe-o uma vez
/// e nao o volta a decidir em cada sitio.
struct SplitPage {
    init_script: String,
    ipc: SplitIpc,
}

#[derive(Clone, Copy)]
struct SplitIpc {
    source_index: usize,
    private: bool,
}

impl SplitIpc {
    fn event(self, action: IpcAction) -> Option<UserEvent> {
        App::split_ipc_event_impl(self.source_index, self.private, action)
    }
}

fn split_page(
    source_index: usize,
    source_name: &str,
    capability: &str,
    private: bool,
) -> SplitPage {
    SplitPage {
        init_script: bind_page_script(
            &format!(
                "window.__neuralia_col_index = {source_index}; window.__neuralia_col_name = '{source_name}';\n{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}"
            ),
            capability,
            private,
        ),
        ipc: SplitIpc {
            source_index,
            private,
        },
    }
}

/// O que `open_split_mode` faz com um pedido, decidido sem janela.
enum SplitOpenPlan {
    /// O comparador ja nao esta la: abre como Web normal (nunca um privado).
    Web(String),
    Ignore,
    /// Recusado; a mensagem vai para o aviso do meio da janela.
    Refuse {
        message: &'static str,
        seconds: u64,
    },
    Build(SplitBuild),
}

/// Tudo o que o builder do Split recebe. O `private` do pedido entra UMA vez
/// (em `split_open_plan`) e daqui saem o script injetado (sem o Mandar para
/// IA nem o Traduzir), o
/// mapa IPC (que recusa `search`) e o perfil anonimo do WebView2; o builder
/// e o resto de `open_split_mode` so leem isto.
struct SplitBuild {
    url: Url,
    source_name: &'static str,
    /// A origem local que a URL digitada autorizou (ver `local_origin_of`).
    local_origin: Option<String>,
    capability: String,
    page: SplitPage,
    incognito: bool,
}

/// A decisao de `open_split_mode`, com os mesmos argumentos que ele recebe
/// (o nome da coluna de origem, se o comparador ainda a tem, e a fonte da
/// capability). E aqui que o `private` chega ao builder.
fn split_open_plan(
    surface: Surface,
    source_name: Option<&'static str>,
    source_index: usize,
    url: String,
    allow_local: bool,
    private: bool,
    capability: impl FnOnce() -> String,
) -> SplitOpenPlan {
    match App::split_request_fallback(surface, private) {
        SplitFallback::OpenSplit => {}
        SplitFallback::OpenWeb => return SplitOpenPlan::Web(url),
        SplitFallback::Ignore => return SplitOpenPlan::Ignore,
    }
    let Ok(valid) = neural_core::validate_web_url(&url) else {
        return SplitOpenPlan::Refuse {
            message: "URL da fonte inválida.",
            seconds: 3,
        };
    };
    if !allow_local && neural_core::is_local_network_target(&valid) {
        return SplitOpenPlan::Refuse {
            message: "A página não pode redirecionar a fonte para a rede local.",
            seconds: 4,
        };
    }
    let Some(source_name) = source_name else {
        return SplitOpenPlan::Ignore;
    };
    let capability = capability();
    let page = split_page(source_index, source_name, &capability, private);
    SplitOpenPlan::Build(SplitBuild {
        local_origin: allow_local.then(|| valid.origin().ascii_serialization()),
        url: valid,
        source_name,
        capability,
        incognito: page.ipc.private,
        page,
    })
}

/// O handler IPC do Split, tal como o wry o recebe: envelope autenticado
/// pela capability e depois o mapa do painel (o privado recusa `search`).
/// `send` e o proxy do event loop no app e um registo nos gates.
fn split_ipc_handler<S>(
    capability: String,
    ipc: SplitIpc,
    send: S,
) -> impl Fn(wry::http::Request<String>) + 'static
where
    S: Fn(UserEvent) + 'static,
{
    move |request| {
        let Some(action) = parse_ipc_message(request.body(), &capability, COMPARATOR_COLUMNS)
        else {
            return;
        };
        if let Some(event) = ipc.event(action) {
            send(event);
        }
    }
}

/// O que `configure_split_webview` chama no builder, com os nomes do wry. O
/// produto passa o `WebViewBuilder`; o gate passa um registo e ve o perfil,
/// o script e os handlers que o WebView2 receberia -- e chama-os.
trait SplitWebViewTarget: Sized {
    fn with_incognito(self, incognito: bool) -> Self;
    fn with_initialization_script(self, script: String) -> Self;
    fn with_ipc_handler(self, handler: impl Fn(Request<String>) + 'static) -> Self;
    fn with_navigation_handler(self, handler: impl Fn(String) -> bool + 'static) -> Self;
    /// So o endereco do popup: as `NewWindowFeatures` do wry trazem o
    /// ICoreWebView2 de quem abriu, e nao se usam.
    fn with_new_window_req_handler(
        self,
        handler: impl Fn(String) -> NewWindowResponse + 'static,
    ) -> Self;
    fn with_permission_handler(
        self,
        handler: impl Fn(PermissionKind) -> PermissionResponse + Send + Sync + 'static,
    ) -> Self;
    fn with_focused(self, focused: bool) -> Self;
}

impl SplitWebViewTarget for WebViewBuilder<'static> {
    fn with_incognito(self, incognito: bool) -> Self {
        WebViewBuilder::with_incognito(self, incognito)
    }
    fn with_initialization_script(self, script: String) -> Self {
        WebViewBuilder::with_initialization_script(self, script)
    }
    fn with_ipc_handler(self, handler: impl Fn(Request<String>) + 'static) -> Self {
        WebViewBuilder::with_ipc_handler(self, handler)
    }
    fn with_navigation_handler(self, handler: impl Fn(String) -> bool + 'static) -> Self {
        WebViewBuilder::with_navigation_handler(self, handler)
    }
    fn with_new_window_req_handler(
        self,
        handler: impl Fn(String) -> NewWindowResponse + 'static,
    ) -> Self {
        WebViewBuilder::with_new_window_req_handler(self, move |target, _features| handler(target))
    }
    fn with_permission_handler(
        self,
        handler: impl Fn(PermissionKind) -> PermissionResponse + Send + Sync + 'static,
    ) -> Self {
        WebViewBuilder::with_permission_handler(self, handler)
    }
    fn with_focused(self, focused: bool) -> Self {
        WebViewBuilder::with_focused(self, focused)
    }
}

/// Monta o WebView do Split a partir do `SplitBuild` que `split_open_plan`
/// decidiu: nao volta a decidir script, IPC nem perfil. `send` e o proxy do
/// event loop no produto e um registo no gate.
fn configure_split_webview<B, S>(builder: B, build: &SplitBuild, send: S) -> B
where
    B: SplitWebViewTarget,
    S: Fn(UserEvent) + Clone + 'static,
{
    let ipc = build.page.ipc;
    let source_index = ipc.source_index;
    let local_origin = build.local_origin.clone();
    let new_window_send = send.clone();

    builder
        .with_incognito(build.incognito)
        .with_initialization_script(build.page.init_script.clone())
        .with_ipc_handler(split_ipc_handler(build.capability.clone(), ipc, send))
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
        .with_new_window_req_handler(move |target| {
            if remote_web_target(&target, None) {
                // Um link que a fonte manda abrir noutra aba: o do Split
                // normal nasce no grupo da aba de onde saiu (2.1.7).
                let event = if ipc.private {
                    UserEvent::OpenPrivateSplit {
                        source_index,
                        url: target,
                    }
                } else {
                    UserEvent::OpenSplitFromSplit {
                        source_index,
                        url: target,
                    }
                };
                new_window_send(event);
            }
            NewWindowResponse::Deny
        })
        .with_permission_handler(|kind| web_media_permission(kind, true))
        .with_focused(true)
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
    /// `pomodoro:` -- o botao do Pomodoro vive na barra do comparador, e e
    /// aqui (a palette) que o teclado escreve comandos: sem esta rota,
    /// "pomodoro:pausar" dava "esquema nao permitido" e "pomodoro: 50" ia
    /// perguntar a IA e ficava no historico. `None`: palavra desconhecida
    /// (a ajuda).
    Pomodoro(Option<PomodoroCommand>),
    /// `tema:` -- o mesmo comando da omnibox da Home.
    Theme(Option<ThemeChoice>),
}

fn route_palette(input: &str, source_index: usize, private: bool) -> PaletteRoute {
    let input = input.trim();
    if input.is_empty() || source_index >= COMPARATOR_COLUMNS {
        return PaletteRoute::Invalid(None);
    }
    // Os comandos locais da omnibox que fazem sentido sem sair do
    // comparador, pela MESMA `route_input` da Home. Nada disto sai do
    // computador, nem num painel privado.
    match route_input(input) {
        InputRoute::Pomodoro(command) => return PaletteRoute::Pomodoro(command),
        InputRoute::Theme(choice) => return PaletteRoute::Theme(choice),
        _ => {}
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

/// Os pedidos de permissao do painel do Gemini Live. Camera, microfone e
/// captura de ecra so pelo aviso do proprio WebView2 (`Default`); o resto e
/// recusado. Nunca um `Allow`: e o utilizador quem decide, no aviso. O painel
/// passa ESTA funcao ao `with_permission_handler`, e e ela que o gate chama.
fn live_panel_permission(kind: PermissionKind) -> PermissionResponse {
    web_media_permission(kind, true)
}

/// O handler do canal do painel do Gemini Live, tal como o wry o recebe. So
/// mensagens publicadas pela pagina do painel e da lista fechada chegam a
/// `send` (no app, o proxy do event loop; nos gates, um registo).
fn live_panel_ipc_handler<S>(send: S) -> impl Fn(wry::http::Request<String>) + 'static
where
    S: Fn(UserEvent) + 'static,
{
    move |request| {
        if let Some(message) = live_ipc_message(&request.uri().to_string(), request.body()) {
            send(UserEvent::Live(message));
        }
    }
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

// O arrasto das abas (2.1.7) e a etiqueta do Pomodoro chegam ambos da `App`.
#[allow(clippy::too_many_arguments)]
fn draw_comparator_bar<W>(
    window: &Window,
    comp: &ComparatorState,
    hover: Option<BarHit>,
    visible: bool,
    auto_scroll: bool,
    drag: Option<DragPaint>,
    pomodoro_label: Option<BarLabel>,
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
            bar_columns(comp, pomodoro_label),
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
            visible,
            hover,
            auto_scroll,
            drag,
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
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
unsafe fn paint_comparator_bar<W>(
    target: *mut core::ffi::c_void,
    width: i32,
    scale: f64,
    names: &[&str],
    visible: bool,
    hover: Option<BarHit>,
    auto_scroll: bool,
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
        visible,
        hover,
        auto_scroll,
        None,
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
    visible: bool,
    hover: Option<BarHit>,
    auto_scroll: bool,
    drag: Option<DragPaint>,
    live: &LivePanel<W>,
    theme: &Theme,
) {
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
    draw_live_button(
        target,
        controls.live,
        live.indicator(),
        hover == Some(BarHit::GeminiLive),
        scale,
        theme,
    );
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

#[cfg(test)]
pub(super) const ALL_MODULES: &[(&str, &str)] = &[
    ("tests.rs", include_str!("windows_app/tests.rs")),
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
    ("services.rs", include_str!("windows_app/services.rs")),
];

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
mod tests;

mod theme;
use theme::*;

mod icons;
use icons::*;

mod bar_layout;
use bar_layout::*;

mod tab_row;
use tab_row::*;

mod native;
use native::*;

mod splash;
use splash::*;

mod page_scripts;
use page_scripts::*;

mod notes;
use notes::*;

mod side_panel;
use side_panel::*;
mod services;
use services::*;

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

/// Id do item de rolagem nos menus de uma coluna: o que o `TrackPopupMenu` da
/// pilula devolve e o que o item acrescentado ao menu do WebView2 entrega.
/// Zero e o "fechou sem escolher" do Win32, por isso nunca e um comando.
const COLUMN_MENU_AUTO_SCROLL: usize = 1;

/// O item escolhido num menu de coluna vira o evento que o Ctrl+R premido
/// DENTRO dessa coluna produz -- o mesmo despacho, `column_ipc_event_impl`,
/// para o atalho e o menu nunca divergirem.
fn column_menu_event(col_index: usize, command: usize) -> Option<UserEvent> {
    if col_index >= COMPARATOR_COLUMNS {
        return None;
    }
    match command {
        COLUMN_MENU_AUTO_SCROLL => App::column_ipc_event_impl(col_index, IpcAction::AutoScroll),
        _ => None,
    }
}

/// Que WebView e esta, para quem decide o que o botao direito lhe acrescenta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebViewHost {
    /// Uma das colunas das IAs no comparador.
    Column(usize),
    /// A fonte aberta ao lado da coluna indicada -- tambem a resposta de uma
    /// IA pedida no painel privado, e a unica pagina a vista em tela cheia.
    Split(usize),
    /// Historico e memoria, a direita.
    SidePanel,
    /// Meet, WhatsApp, YouTube e Gmail no painel.
    Service,
}

/// Recebem o item de rolagem as paginas que rolam sozinhas: as colunas das
/// IAs e a fonte aberta ao lado (o `auto_scroll_tick` rola-a e o Ctrl+R
/// funciona nela -- sem o item, a resposta de uma IA aberta no painel privado
/// rolava sem nenhum botao direito para a parar). O painel lateral e os
/// servicos nao rolam: ficam com o menu nativo do WebView2 tal como vem.
fn context_menu_column(host: WebViewHost) -> Option<usize> {
    match host {
        WebViewHost::Column(index) | WebViewHost::Split(index) if index < COMPARATOR_COLUMNS => {
            Some(index)
        }
        WebViewHost::Column(_)
        | WebViewHost::Split(_)
        | WebViewHost::SidePanel
        | WebViewHost::Service => None,
    }
}

/// O que `install_context_menu` faz com uma WebView acabada de construir:
/// chama `register` so para uma coluna, com o indice dela, e devolve a linha
/// de log quando o registo falha -- um runtime WebView2 sem o
/// ContextMenuRequested. Essa falha nao sobe: a coluna abre, com o menu
/// nativo inteiro, e so o item de rolagem fica de fora.
fn install_column_menu(
    host: WebViewHost,
    register: impl FnOnce(usize) -> Result<(), String>,
) -> Option<String> {
    let col_index = context_menu_column(host)?;
    register(col_index)
        .err()
        .map(|error| format!("context menu: coluna {col_index} sem o item de rolagem ({error})"))
}

/// Onde o item de rolagem entra num menu nativo com `native` itens: DEPOIS de
/// todos eles, separado por uma linha quando ha algo acima. Copiar, colar,
/// inspecionar e o resto ficam nos lugares em que o WebView2 os pos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColumnMenuPlacement {
    separator_at: Option<u32>,
    item_at: u32,
}

fn column_menu_placement(native: u32) -> ColumnMenuPlacement {
    if native == 0 {
        ColumnMenuPlacement {
            separator_at: None,
            item_at: 0,
        }
    } else {
        ColumnMenuPlacement {
            separator_at: Some(native),
            item_at: native + 1,
        }
    }
}

/// O item de rolagem de UM botao direito: o rotulo lido no instante do
/// pedido, onde entra entre os `native` itens do menu, e o id que o
/// representa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColumnMenuRequest {
    col_index: usize,
    label: &'static str,
    placement: ColumnMenuPlacement,
    command: usize,
}

impl ColumnMenuRequest {
    /// O evento de escolher o item: o do Ctrl+R premido na coluna.
    fn selected(&self) -> Option<UserEvent> {
        column_menu_event(self.col_index, self.command)
    }
}

/// O que responde a cada botao direito de uma coluna. Criado UMA vez, quando
/// a WebView e registada (ou quando a pilula abre o menu), e chamado a cada
/// pedido: por isso o rotulo le o `SharedFlag` dentro da resposta, nunca na
/// criacao -- um rotulo lido no registo ficava preso ao estado do arranque.
/// `register_column_context_menu` e `column_pill_menu` so copiam para o
/// Win32/COM o que isto decide.
fn column_menu_responder(
    col_index: usize,
    auto_scroll: SharedFlag,
) -> impl Fn(u32) -> ColumnMenuRequest {
    move |native| ColumnMenuRequest {
        col_index,
        label: auto_scroll_menu_label(auto_scroll.get()),
        placement: column_menu_placement(native),
        command: COLUMN_MENU_AUTO_SCROLL,
    }
}

/// Acrescenta ao menu nativo do botao direito de uma coluna o item de
/// rolagem, com o rotulo do estado no instante do clique, no lugar que
/// `column_menu_placement` decide. Precisa do ContextMenuRequested
/// (ICoreWebView2_11 e ICoreWebView2Environment9); num runtime sem ele devolve
/// o erro e a coluna fica so com o menu nativo. Uma falha a montar um menu
/// concreto fica no log e esse menu abre como o WebView2 o trouxe.
fn register_column_context_menu(
    webview: &WebView,
    col_index: usize,
    auto_scroll: SharedFlag,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    use webview2_com::{
        ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR, ICoreWebView2_11,
            ICoreWebView2ContextMenuRequestedEventArgs, ICoreWebView2Environment9,
        },
    };
    use windows_core::{HSTRING, Interface};
    use wry::WebViewExtWindows;

    let core = webview
        .webview()
        .cast::<ICoreWebView2_11>()
        .map_err(|error| format!("ICoreWebView2_11 indisponível: {error}"))?;
    let environment = webview
        .environment()
        .cast::<ICoreWebView2Environment9>()
        .map_err(|error| format!("ICoreWebView2Environment9 indisponível: {error}"))?;

    let respond = column_menu_responder(col_index, auto_scroll);
    let add_item =
        move |args: &ICoreWebView2ContextMenuRequestedEventArgs| -> windows_core::Result<()> {
            unsafe {
                let items = args.MenuItems()?;
                let mut native = 0u32;
                items.Count(&mut native)?;
                let request = respond(native);
                let label = HSTRING::from(request.label);
                let placement = request.placement;
                let proxy = proxy.clone();
                let selected = CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                    if let Some(event) = request.selected() {
                        let _ = proxy.send_event(event);
                    }
                    Ok(())
                }));
                // Tudo criado antes de mexer no menu: uma falha a meio nao deixa
                // um separador solto no fim do menu nativo.
                let item = environment.CreateContextMenuItem(
                    &label,
                    None,
                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
                )?;
                let mut selected_token = 0i64;
                item.add_CustomItemSelected(&selected, &mut selected_token)?;
                let separator = match placement.separator_at {
                    Some(index) => Some((
                        index,
                        environment.CreateContextMenuItem(
                            &HSTRING::new(),
                            None,
                            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR,
                        )?,
                    )),
                    None => None,
                };
                if let Some((index, separator)) = separator {
                    items.InsertValueAtIndex(index, &separator)?;
                }
                items.InsertValueAtIndex(placement.item_at, &item)?;
            }
            Ok(())
        };
    let handler = ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
        if let Some(args) = args
            && let Err(error) = add_item(&args)
        {
            debug_log(format_args!(
                "context menu: coluna {col_index} abriu sem o item de rolagem ({error})"
            ));
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core.add_ContextMenuRequested(&handler, &mut token) }
        .map_err(|error| format!("add_ContextMenuRequested falhou: {error}"))
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
