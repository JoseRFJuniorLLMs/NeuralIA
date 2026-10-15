use super::*;

// ===================== o registo dos comandos (infra-commands-keymap) =====================
//
// Cada comando que a NeuralIA sabe correr por nome e uma linha de
// `COMMANDS`: a chave estavel (a que os atalhos personalizados e os
// recentes da paleta vao guardar), o rotulo em pt-BR, a categoria da
// paleta, as palavras que o procuram, os atalhos -- cada um com o seu
// ambito (`KeyScope`, em `keymap.rs`) -- e o alias da omnibox, quando ha
// um. `resolve_command(id, origem)` diz o que o comando faz a partir de
// onde foi pedido, e o event loop corre so o evento que ela devolve para
// um `UserEvent::RunCommandKey` -- por `command_key_event`, que le o
// comando e a origem do proprio evento (o event loop entrega-lhe o evento
// inteiro e nao escolhe origem nenhuma).
//
// Regra (critica C13 do plano 2.3): um atalho entra no MESMO PR que o seu
// comando. Este registo nasceu com o que a 2.2.0 ja tinha -- os atalhos da
// janela e da omnibox (antes `main_window_shortcut` e o subclass do EDIT,
// cada um com a sua lista) e os botoes da barra que ja eram um evento. Cada
// feature da 2.3 acrescenta aqui a sua linha, com o seu atalho, no PR dela:
// o Ctrl+J dos Downloads (downloads-ui, `Global`) e o Ctrl+D dos favoritos
// (`CommandId::Bookmark`, `Global`), que corre contra o hospedeiro de onde
// veio a tecla (`bookmark_target`) e nunca passa pelo IPC.

/// Um comando do registo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::windows_app) enum CommandId {
    /// Liga ou desliga a rolagem automatica.
    AutoScroll,
    FocusAddress,
    NavBack,
    NavForward,
    LiveToggle,
    LiveStop,
    PrivatePanel,
    AboutPanel,
    ShortcutMap,
    PomodoroToggle,
    BreathPanel,
    MeetPanel,
    TeamsPanel,
    OutlookPanel,
    WhatsAppPanel,
    YoutubePanel,
    GmailPanel,
    PanelMinimize,
    PanelFullscreen,
    PanelClose,
    TranslatePage,
    ColumnOne,
    ColumnTwo,
    ColumnThree,
    TabList,
    WindowMin,
    WindowMax,
    PrintPage,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    NextTab,
    PrevTab,
    ToggleGroup,

    /// Recarrega a pagina de onde veio (ou as visiveis, da janela).
    Reload,
    /// O painel do Ctrl+H (historico, memoria e notas).
    History,
    /// Pergunta nova: a paleta da coluna no comparador, a omnibox na Home.
    NewTab,
    /// O dialogo "Adicionar livros EPUB".
    OpenEpub,
    /// Nota da selecao da pagina de onde veio; sem pagina, nota em branco.
    NewNote,
    /// Apagar historico (pergunta antes).
    ClearHistory,
    /// Voltar a Home.
    Home,
    /// Fechar a fonte aberta ao lado.
    CloseSplit,
    /// Expandir ou reduzir a fonte aberta ao lado.
    SplitFullscreen,
    /// Minimizar ou restaurar a página que recebeu o atalho.
    MinimizePage,
    /// Minimizar todas as colunas e a fonte sem perder a sessão.
    MinimizeAll,
    /// Fechar a NeuralIA (as abas ficam gravadas).
    Exit,
    /// Ctrl+D: a pagina de onde veio para os favoritos; na Home (ou num
    /// painel), abre os Favoritos.
    Bookmark,
    /// A seccao Downloads do painel (downloads-ui): Ctrl+J e a seta da barra.
    Downloads,
}

impl CommandId {
    /// Cada comando, uma vez: o que os gates percorrem.
    #[cfg(test)]
    pub(in crate::windows_app) const ALL: [CommandId; 48] = [
        CommandId::AutoScroll,
        CommandId::FocusAddress,
        CommandId::NavBack,
        CommandId::NavForward,
        CommandId::LiveToggle,
        CommandId::LiveStop,
        CommandId::PrivatePanel,
        CommandId::AboutPanel,
        CommandId::ShortcutMap,
        CommandId::PomodoroToggle,
        CommandId::BreathPanel,
        CommandId::MeetPanel,
        CommandId::TeamsPanel,
        CommandId::OutlookPanel,
        CommandId::WhatsAppPanel,
        CommandId::YoutubePanel,
        CommandId::GmailPanel,
        CommandId::PanelMinimize,
        CommandId::PanelFullscreen,
        CommandId::PanelClose,
        CommandId::TranslatePage,
        CommandId::ColumnOne,
        CommandId::ColumnTwo,
        CommandId::ColumnThree,
        CommandId::TabList,
        CommandId::WindowMin,
        CommandId::WindowMax,
        CommandId::PrintPage,
        CommandId::ZoomIn,
        CommandId::ZoomOut,
        CommandId::ZoomReset,
        CommandId::NextTab,
        CommandId::PrevTab,
        CommandId::ToggleGroup,
        CommandId::Reload,
        CommandId::History,
        CommandId::NewTab,
        CommandId::OpenEpub,
        CommandId::NewNote,
        CommandId::ClearHistory,
        CommandId::Home,
        CommandId::CloseSplit,
        CommandId::SplitFullscreen,
        CommandId::MinimizePage,
        CommandId::MinimizeAll,
        CommandId::Exit,
        CommandId::Bookmark,
        CommandId::Downloads,
    ];

    /// A chave estavel do comando (a da sua linha em `COMMANDS`).
    pub(in crate::windows_app) fn key(self) -> &'static str {
        command_row(self).map_or("?", |row| row.key)
    }
}

/// De onde um comando parte -- e contra ela que corre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum CommandOrigin {
    /// A propria janela com o teclado (depois de um clique na barra).
    Window,
    /// A omnibox da Home (o EDIT nativo).
    Omnibox,
    /// Uma WebView: o hospedeiro fixado quando o `AcceleratorKeyPressed`
    /// dela foi registado. Nunca vem da pagina.
    Host(WebViewHost),
}

impl CommandOrigin {
    /// A origem para uma linha de log.
    pub(in crate::windows_app) fn describe(self) -> String {
        match self {
            CommandOrigin::Window => "janela".to_string(),
            CommandOrigin::Omnibox => "omnibox".to_string(),
            CommandOrigin::Host(host) => host.describe(),
        }
    }
}

/// A categoria de um comando na paleta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum CommandCategory {
    Navegacao,
    Pagina,
    FonteAoLado,
    PesquisaIa,
    Ferramentas,
    HistoricoMemoria,
    Janela,
}

impl CommandCategory {
    /// O nome que a paleta mostra.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) fn label(self) -> &'static str {
        match self {
            CommandCategory::Navegacao => "Navegação",
            CommandCategory::Pagina => "Página",
            CommandCategory::FonteAoLado => "Fonte ao lado",
            CommandCategory::PesquisaIa => "Pesquisa e IA",
            CommandCategory::Ferramentas => "Ferramentas",
            CommandCategory::HistoricoMemoria => "Histórico e memória",
            CommandCategory::Janela => "Janela",
        }
    }
}

/// Um atalho de um comando e o ambito onde vale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct ChordSpec {
    pub(in crate::windows_app) scope: KeyScope,
    pub(in crate::windows_app) chord: Chord,
}

impl ChordSpec {
    /// Um atalho da janela e da omnibox.
    const fn window(chord: Chord) -> ChordSpec {
        ChordSpec {
            scope: KeyScope::Window,
            chord,
        }
    }

    /// Um atalho de toda a parte com teclado: a janela, a omnibox e cada
    /// WebView (menos o monitor do Gmail), preso pelo `AcceleratorKeyPressed`
    /// -- a pagina nao ve o keydown dele.
    const fn global(chord: Chord) -> ChordSpec {
        ChordSpec {
            scope: KeyScope::Global,
            chord,
        }
    }
}

/// Uma linha do registo. O rotulo, a categoria, as palavras e o alias sao
/// o que a paleta (onda 5) le; o mapa de teclas le o id e os atalhos.
#[derive(Debug)]
pub(in crate::windows_app) struct CommandRow {
    pub(in crate::windows_app) id: CommandId,
    /// Estavel, ASCII, minusculas e hifens: nunca muda depois de publicada.
    pub(in crate::windows_app) key: &'static str,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) label: &'static str,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) category: CommandCategory,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) keywords: &'static [&'static str],
    pub(in crate::windows_app) chords: &'static [ChordSpec],
    /// O que escrito na omnibox faz o mesmo (gate: `route_input` leva-o ao
    /// mesmo sitio).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) alias: Option<&'static str>,
}

/// O registo. Um atalho por ambito, uma linha por comando (gate
/// `keymap_chords_are_unique_per_scope`).
pub(in crate::windows_app) const COMMANDS: &[CommandRow] = &[
    CommandRow {
        id: CommandId::PrintPage,
        key: "imprimir",
        label: "Imprimir página",
        category: CommandCategory::Navegacao,
        keywords: &["imprimir"],
        chords: &[ChordSpec::global(Chord::ctrl(b'P'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::ZoomIn,
        key: "zoom-mais",
        label: "Aumentar zoom",
        category: CommandCategory::Navegacao,
        keywords: &["zoom-mais"],
        chords: &[
            ChordSpec::global(Chord::ctrl(0xBB)),
            ChordSpec::global(Chord::ctrl_shift(0xBB)),
            ChordSpec::global(Chord::ctrl(0x6B)),
        ],
        alias: None,
    },
    CommandRow {
        id: CommandId::ZoomOut,
        key: "zoom-menos",
        label: "Diminuir zoom",
        category: CommandCategory::Navegacao,
        keywords: &["zoom-menos"],
        chords: &[
            ChordSpec::global(Chord::ctrl(0xBD)),
            ChordSpec::global(Chord::ctrl(0x6D)),
        ],
        alias: None,
    },
    CommandRow {
        id: CommandId::ZoomReset,
        key: "zoom-reset",
        label: "Restaurar zoom",
        category: CommandCategory::Navegacao,
        keywords: &["zoom-reset"],
        chords: &[ChordSpec::global(Chord::ctrl(b'0'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::NextTab,
        key: "proxima-aba",
        label: "Próxima aba",
        category: CommandCategory::Navegacao,
        keywords: &["proxima-aba"],
        chords: &[ChordSpec::global(Chord::ctrl(0x09))],
        alias: None,
    },
    CommandRow {
        id: CommandId::PrevTab,
        key: "aba-anterior",
        label: "Aba anterior",
        category: CommandCategory::Navegacao,
        keywords: &["aba-anterior"],
        chords: &[ChordSpec::global(Chord::ctrl_shift(0x09))],
        alias: None,
    },
    CommandRow {
        id: CommandId::ToggleGroup,
        key: "alternar-grupo",
        label: "Expandir ou recolher grupo da aba ativa",
        category: CommandCategory::Navegacao,
        keywords: &["alternar-grupo"],
        chords: &[ChordSpec::global(Chord::alt(b'C'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::LiveStop,
        key: "desligar-live",
        label: "Desligar Gemini Live",
        category: CommandCategory::Ferramentas,
        keywords: &["desligar-live"],
        chords: &[ChordSpec::global(Chord {
            vk: b'G' as u32,
            ctrl: false,
            shift: true,
            alt: true,
        })],
        alias: None,
    },
    CommandRow {
        id: CommandId::FocusAddress,
        key: "endereco",
        label: "Barra de endereço",
        category: CommandCategory::Ferramentas,
        keywords: &["endereco"],
        chords: &[
            ChordSpec::global(Chord::ctrl(b'L')),
            ChordSpec::global(Chord::alt(b'D')),
            ChordSpec::global(Chord::key(0x75)),
        ],
        alias: None,
    },
    CommandRow {
        id: CommandId::NavBack,
        key: "voltar",
        label: "Voltar",
        category: CommandCategory::Ferramentas,
        keywords: &["voltar"],
        chords: &[ChordSpec::global(Chord::alt(0x25))],
        alias: None,
    },
    CommandRow {
        id: CommandId::NavForward,
        key: "avancar",
        label: "Avançar",
        category: CommandCategory::Ferramentas,
        keywords: &["avancar"],
        chords: &[ChordSpec::global(Chord::alt(0x27))],
        alias: None,
    },
    CommandRow {
        id: CommandId::LiveToggle,
        key: "gemini-live",
        label: "Abrir, minimizar ou restaurar Gemini Live",
        category: CommandCategory::Ferramentas,
        keywords: &["gemini-live"],
        chords: &[ChordSpec::global(Chord::alt(b'G'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::PrivatePanel,
        key: "privado",
        label: "Navegação privada",
        category: CommandCategory::Ferramentas,
        keywords: &["privado"],
        chords: &[ChordSpec::global(Chord::ctrl_shift(b'N'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::AboutPanel,
        key: "sobre",
        label: "Sobre o NeuralIA",
        category: CommandCategory::Ferramentas,
        keywords: &["sobre"],
        chords: &[ChordSpec::global(Chord::key(0x70))],
        alias: None,
    },
    CommandRow {
        id: CommandId::ShortcutMap,
        key: "atalhos",
        label: "Mapa de teclas de atalho",
        category: CommandCategory::Ferramentas,
        keywords: &["atalhos"],
        chords: &[ChordSpec::global(Chord::ctrl_shift(0x70))],
        alias: None,
    },
    CommandRow {
        id: CommandId::PomodoroToggle,
        key: "pomodoro",
        label: "Iniciar ou pausar Pomodoro",
        category: CommandCategory::Ferramentas,
        keywords: &["pomodoro"],
        chords: &[ChordSpec::global(Chord::alt(b'P'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::BreathPanel,
        key: "respiracao",
        label: "Respiração guiada",
        category: CommandCategory::Ferramentas,
        keywords: &["respiracao"],
        chords: &[ChordSpec::global(Chord::alt(b'B'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::MeetPanel,
        key: "meet",
        label: "Google Meet",
        category: CommandCategory::Ferramentas,
        keywords: &["meet"],
        chords: &[ChordSpec::global(Chord::alt(b'1'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::TeamsPanel,
        key: "teams",
        label: "Microsoft Teams",
        category: CommandCategory::Ferramentas,
        keywords: &["teams"],
        chords: &[ChordSpec::global(Chord::alt(b'2'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::OutlookPanel,
        key: "outlook",
        label: "Outlook",
        category: CommandCategory::Ferramentas,
        keywords: &["outlook"],
        chords: &[ChordSpec::global(Chord::alt(b'3'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::WhatsAppPanel,
        key: "whatsapp",
        label: "WhatsApp",
        category: CommandCategory::Ferramentas,
        keywords: &["whatsapp"],
        chords: &[ChordSpec::global(Chord::alt(b'4'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::YoutubePanel,
        key: "youtube",
        label: "YouTube",
        category: CommandCategory::Ferramentas,
        keywords: &["youtube"],
        chords: &[ChordSpec::global(Chord::alt(b'5'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::GmailPanel,
        key: "gmail",
        label: "Gmail",
        category: CommandCategory::Ferramentas,
        keywords: &["gmail"],
        chords: &[ChordSpec::global(Chord::alt(b'6'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::PanelMinimize,
        key: "minimizar-painel",
        label: "Minimizar ou restaurar painel",
        category: CommandCategory::Ferramentas,
        keywords: &["minimizar-painel"],
        chords: &[ChordSpec::global(Chord::alt(b'M'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::PanelFullscreen,
        key: "tela-cheia-painel",
        label: "Tela cheia do painel de serviços",
        category: CommandCategory::Ferramentas,
        keywords: &["tela-cheia-painel"],
        chords: &[ChordSpec::global(Chord::alt(b'F'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::PanelClose,
        key: "fechar-painel",
        label: "Fechar painel",
        category: CommandCategory::Ferramentas,
        keywords: &["fechar-painel"],
        chords: &[ChordSpec::global(Chord::alt(b'W'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::TranslatePage,
        key: "traduzir-pagina",
        label: "Traduzir página",
        category: CommandCategory::Ferramentas,
        keywords: &["traduzir-pagina"],
        chords: &[ChordSpec::global(Chord::alt(b'T'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::ColumnOne,
        key: "coluna-1",
        label: "Expandir ou restaurar primeira IA",
        category: CommandCategory::Ferramentas,
        keywords: &["coluna-1"],
        chords: &[ChordSpec::global(Chord::ctrl(b'1'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::ColumnTwo,
        key: "coluna-2",
        label: "Expandir ou restaurar segunda IA",
        category: CommandCategory::Ferramentas,
        keywords: &["coluna-2"],
        chords: &[ChordSpec::global(Chord::ctrl(b'2'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::ColumnThree,
        key: "coluna-3",
        label: "Expandir ou restaurar terceira IA",
        category: CommandCategory::Ferramentas,
        keywords: &["coluna-3"],
        chords: &[ChordSpec::global(Chord::ctrl(b'3'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::TabList,
        key: "lista-abas",
        label: "Lista das abas da coluna",
        category: CommandCategory::Ferramentas,
        keywords: &["lista-abas"],
        chords: &[ChordSpec::global(Chord::alt(0x28))],
        alias: None,
    },
    CommandRow {
        id: CommandId::WindowMin,
        key: "minimizar-janela",
        label: "Minimizar janela",
        category: CommandCategory::Ferramentas,
        keywords: &["minimizar-janela"],
        chords: &[ChordSpec::global(Chord::key(0x78))],
        alias: None,
    },
    CommandRow {
        id: CommandId::WindowMax,
        key: "maximizar-janela",
        label: "Maximizar ou restaurar janela",
        category: CommandCategory::Ferramentas,
        keywords: &["maximizar-janela"],
        chords: &[ChordSpec::global(Chord::key(0x79))],
        alias: None,
    },
    CommandRow {
        id: CommandId::AutoScroll,
        key: "rolagem-automatica",
        label: "Rolagem automática",
        category: CommandCategory::Pagina,
        keywords: &["rolar", "rolagem", "scroll", "ler", "leitura"],
        chords: &[ChordSpec::global(Chord::key(0x77))],
        alias: None,
    },
    CommandRow {
        id: CommandId::Reload,
        key: "recarregar",
        label: "Recarregar a página",
        category: CommandCategory::Pagina,
        keywords: &["recarregar", "atualizar", "reload"],
        chords: &[
            ChordSpec::global(Chord::ctrl(b'R')),
            ChordSpec::global(Chord::ctrl_shift(b'R')),
            ChordSpec::global(Chord::key(0x74)),
        ],
        alias: None,
    },
    CommandRow {
        id: CommandId::History,
        key: "historico",
        label: "Histórico, memória e notas",
        category: CommandCategory::HistoricoMemoria,
        keywords: &["histórico", "historico", "memória", "notas", "painel"],
        chords: &[ChordSpec::global(Chord::ctrl(b'H'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::NewTab,
        key: "nova-pergunta",
        label: "Nova pergunta",
        category: CommandCategory::PesquisaIa,
        keywords: &["nova", "aba", "pergunta", "perguntar"],
        chords: &[
            ChordSpec::global(Chord::ctrl(b'T')),
            ChordSpec::global(Chord::ctrl(b'N')),
        ],
        alias: None,
    },
    CommandRow {
        id: CommandId::OpenEpub,
        key: "adicionar-livros",
        label: "Adicionar livros EPUB",
        category: CommandCategory::Ferramentas,
        keywords: &["livros", "epub", "biblioteca", "abrir"],
        chords: &[ChordSpec::window(Chord::ctrl(b'O'))],
        alias: Some("epub:"),
    },
    CommandRow {
        id: CommandId::NewNote,
        key: "nota-nova",
        label: "Nota nova",
        category: CommandCategory::HistoricoMemoria,
        keywords: &["nota", "notas", "anotar", "zettelkasten"],
        chords: &[ChordSpec::window(Chord::ctrl_shift(b'Z'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::ClearHistory,
        key: "apagar-historico",
        label: "Apagar histórico",
        category: CommandCategory::HistoricoMemoria,
        keywords: &["apagar", "limpar", "histórico", "historico", "memória"],
        chords: &[ChordSpec::window(Chord::ctrl_shift(
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_DELETE as u8,
        ))],
        alias: None,
    },
    CommandRow {
        id: CommandId::Home,
        key: "home",
        label: "Voltar à Home",
        category: CommandCategory::Navegacao,
        keywords: &["home", "início", "inicio", "voltar"],
        chords: &[ChordSpec::global(Chord::alt(
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_HOME as u8,
        ))],
        alias: None,
    },
    CommandRow {
        id: CommandId::CloseSplit,
        key: "fechar-fonte",
        label: "Fechar a aba ativa",
        category: CommandCategory::FonteAoLado,
        keywords: &["fechar", "fonte", "lado", "split"],
        chords: &[ChordSpec::global(Chord::ctrl(b'W'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::SplitFullscreen,
        key: "expandir-fonte",
        label: "Expandir ou reduzir a fonte aberta ao lado",
        category: CommandCategory::FonteAoLado,
        keywords: &["expandir", "reduzir", "fonte", "lado", "tela cheia"],
        chords: &[ChordSpec::global(Chord::key(
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F11 as u8,
        ))],
        alias: None,
    },
    CommandRow {
        id: CommandId::Exit,
        key: "sair",
        label: "Fechar a NeuralIA",
        category: CommandCategory::Janela,
        keywords: &["sair", "fechar", "janela"],
        chords: &[ChordSpec::window(Chord::ctrl_shift(b'W'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::MinimizePage,
        key: "minimizar-pagina",
        label: "Minimizar ou restaurar a página",
        category: CommandCategory::Pagina,
        keywords: &["minimizar", "restaurar", "página", "coluna", "fonte"],
        chords: &[ChordSpec::global(Chord::ctrl_shift(b'M'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::MinimizeAll,
        key: "minimizar-tudo",
        label: "Minimizar todas as páginas",
        category: CommandCategory::Pagina,
        keywords: &["minimizar", "tudo", "colunas", "fontes"],
        chords: &[ChordSpec::global(Chord::ctrl_shift(
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F9 as u8,
        ))],
        alias: None,
    },
    CommandRow {
        id: CommandId::Bookmark,
        key: "adicionar-favorito",
        label: "Adicionar aos favoritos",
        category: CommandCategory::Pagina,
        keywords: &["favorito", "favoritos", "marcador", "bookmark", "estrela"],
        chords: &[ChordSpec::global(Chord::ctrl(b'D'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::Downloads,
        key: "downloads",
        label: "Downloads",
        category: CommandCategory::Ferramentas,
        keywords: &["downloads", "baixados", "transferências", "arquivos"],
        chords: &[ChordSpec::global(Chord::ctrl(b'J'))],
        alias: None,
    },
];

/// A linha de um comando.
pub(in crate::windows_app) fn command_row(id: CommandId) -> Option<&'static CommandRow> {
    COMMANDS.iter().find(|row| row.id == id)
}

/// O evento de um comando pedido da janela ou da omnibox: o que o
/// `main_window_shortcut` e o subclass da omnibox faziam na 2.2.0, e o que
/// um comando que nao depende da pagina faz de qualquer origem.
fn chrome_event(id: CommandId) -> UserEvent {
    match id {
        CommandId::PrintPage => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ZoomIn => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ZoomOut => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ZoomReset => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::NextTab => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::PrevTab => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ToggleGroup => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::FocusAddress => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::NavBack => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::NavForward => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::LiveStop => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::LiveToggle => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::PrivatePanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::AboutPanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ShortcutMap => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::PomodoroToggle => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::BreathPanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::MeetPanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::TeamsPanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::OutlookPanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::WhatsAppPanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::YoutubePanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::GmailPanel => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::PanelMinimize => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::PanelFullscreen => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::PanelClose => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::TranslatePage => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ColumnOne => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ColumnTwo => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::ColumnThree => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::TabList => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::WindowMin => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::WindowMax => UserEvent::NativeShortcut(id, CommandOrigin::Window),
        CommandId::AutoScroll => UserEvent::ToggleAutoScroll,
        CommandId::Reload => UserEvent::ReloadPage,
        CommandId::History => UserEvent::ShowHistory,
        CommandId::NewTab => UserEvent::NewTab(0),
        CommandId::OpenEpub => UserEvent::OpenEpubDialog,
        CommandId::NewNote => UserEvent::NewNote,
        CommandId::ClearHistory => UserEvent::ClearHistory,
        CommandId::Home => UserEvent::HomeRequested,
        CommandId::CloseSplit => UserEvent::CloseActiveTab(CommandOrigin::Window),
        CommandId::SplitFullscreen => UserEvent::ToggleColumnFullscreen,
        CommandId::MinimizePage => UserEvent::MinimizeActivePage(CommandOrigin::Window),
        CommandId::MinimizeAll => UserEvent::MinimizeAllColumns,
        CommandId::Exit => UserEvent::ExitRequested,
        CommandId::Bookmark => UserEvent::Bookmarks(BookmarksEvent::Request {
            target: BookmarkTarget::Window,
            via: BookmarkVia::Shortcut,
        }),
        CommandId::Downloads => UserEvent::DownloadsUi(DownloadsUiEvent::Show),
    }
}

/// Os comandos que agem sobre a pagina de onde vieram, e o pedido que o
/// mapa de teclas dessa pagina manda pelo IPC para o mesmo gesto. A partir
/// de uma pagina, o comando da o evento que esse pedido da -- pelo mesmo
/// despacho (`column_ipc_event_impl`, `split_ipc_event_impl`,
/// `common_ipc_event`), para o atalho nativo e o do script nunca
/// divergirem: a coluna recarrega-se a si, o Split privado recusa a nota.
fn page_action(id: CommandId, column: Option<usize>) -> Option<IpcAction> {
    match id {
        CommandId::PrintPage => None,
        CommandId::ZoomIn => None,
        CommandId::ZoomOut => None,
        CommandId::ZoomReset => None,
        CommandId::NextTab => None,
        CommandId::PrevTab => None,
        CommandId::ToggleGroup => None,

        CommandId::LiveStop => None,
        CommandId::Reload => Some(IpcAction::Reload),
        CommandId::NewNote => Some(IpcAction::Note {
            via: NoteVia::Shortcut,
        }),
        CommandId::NewTab => Some(IpcAction::NewTab { col: column }),
        CommandId::FocusAddress |
        CommandId::NavBack |
        CommandId::NavForward |
        CommandId::LiveToggle |
        CommandId::PrivatePanel |
        CommandId::AboutPanel |
        CommandId::ShortcutMap |
        CommandId::PomodoroToggle |
        CommandId::BreathPanel |
        CommandId::MeetPanel |
        CommandId::TeamsPanel |
        CommandId::OutlookPanel |
        CommandId::WhatsAppPanel |
        CommandId::YoutubePanel |
        CommandId::GmailPanel |
        CommandId::PanelMinimize |
        CommandId::PanelFullscreen |
        CommandId::PanelClose |
        CommandId::TranslatePage |
        CommandId::ColumnOne |
        CommandId::ColumnTwo |
        CommandId::ColumnThree |
        CommandId::TabList |
        CommandId::WindowMin |
        CommandId::WindowMax |
        CommandId::AutoScroll
        | CommandId::History
        | CommandId::OpenEpub
        | CommandId::ClearHistory
        | CommandId::Home
        | CommandId::CloseSplit
        | CommandId::SplitFullscreen
        | CommandId::MinimizePage
        | CommandId::MinimizeAll
        | CommandId::Exit
        | CommandId::Downloads
        // Nativo: a origem e o hospedeiro (`bookmark_target`), nunca um
        // pedido do mapa de teclas da pagina.
        | CommandId::Bookmark => None,
    }
}

/// O que o comando `id` faz, pedido a partir de `origin`. `None`: nada, ali
/// (recarregar um painel, qualquer coisa a partir do monitor escondido do
/// Gmail, uma coluna que nao existe). Pura: o event loop corre o evento.
pub(in crate::windows_app) fn resolve_command(
    id: CommandId,
    origin: CommandOrigin,
) -> Option<UserEvent> {
    if is_native_shortcut(id) {
        if matches!(origin, CommandOrigin::Host(WebViewHost::Column(index) | WebViewHost::Split(index) | WebViewHost::PrivateSplit(index)) if index >= COMPARATOR_COLUMNS)
        {
            return None;
        }
        return (origin != CommandOrigin::Host(WebViewHost::GmailMonitor))
            .then_some(UserEvent::NativeShortcut(id, origin));
    }
    if id == CommandId::CloseSplit {
        return match origin {
            CommandOrigin::Host(WebViewHost::GmailMonitor) => None,
            CommandOrigin::Host(
                WebViewHost::Column(index)
                | WebViewHost::Split(index)
                | WebViewHost::PrivateSplit(index),
            ) if index >= COMPARATOR_COLUMNS => None,
            _ => Some(UserEvent::CloseActiveTab(origin)),
        };
    }
    if id == CommandId::MinimizePage {
        return match origin {
            CommandOrigin::Host(WebViewHost::GmailMonitor) => None,
            CommandOrigin::Host(
                WebViewHost::Column(index)
                | WebViewHost::Split(index)
                | WebViewHost::PrivateSplit(index),
            ) if index >= COMPARATOR_COLUMNS => None,
            _ => Some(UserEvent::MinimizeActivePage(origin)),
        };
    }
    // O Ctrl+D leva a origem inteira: a pagina e a do hospedeiro que
    // recebeu a tecla (a coluna 2 e a coluna 2), a janela na Home abre os
    // Favoritos.
    if id == CommandId::Bookmark {
        return bookmark_target(origin).map(|target| {
            UserEvent::Bookmarks(BookmarksEvent::Request {
                target,
                via: BookmarkVia::Shortcut,
            })
        });
    }
    let host = match origin {
        CommandOrigin::Window | CommandOrigin::Omnibox => return Some(chrome_event(id)),
        CommandOrigin::Host(host) => host,
    };
    match host {
        WebViewHost::Column(index) => {
            if index >= COMPARATOR_COLUMNS {
                return None;
            }
            match page_action(id, Some(index)) {
                Some(action) => App::column_ipc_event_impl(index, action),
                None => Some(chrome_event(id)),
            }
        }
        WebViewHost::Split(index) | WebViewHost::PrivateSplit(index) => {
            if index >= COMPARATOR_COLUMNS {
                return None;
            }
            let private = matches!(host, WebViewHost::PrivateSplit(_));
            match page_action(id, Some(index)) {
                Some(action) => App::split_ipc_event_impl(index, private, action),
                None => Some(chrome_event(id)),
            }
        }
        WebViewHost::External | WebViewHost::Reader | WebViewHost::Pdf => {
            match page_action(id, None) {
                Some(action) => common_ipc_event(action),
                None => Some(chrome_event(id)),
            }
        }
        // As paginas locais sem mapa de teclas e os paineis: sem pagina que
        // o recarregar alcance; a nota e em branco, como na janela.
        WebViewHost::Epub
        | WebViewHost::Live
        | WebViewHost::SidePanel
        | WebViewHost::Service(_) => match id {
            CommandId::Reload => None,
            other => Some(chrome_event(other)),
        },
        WebViewHost::GmailMonitor => None,
    }
}

/// O que um `UserEvent::RunCommandKey` corre no event loop: o evento que
/// `resolve_command` da para o comando contra a origem que veio COM a
/// tecla, lida do proprio evento -- o braco do event loop entrega-o inteiro
/// e nao tem origem nenhuma para passar. `None` para qualquer outro evento
/// e para um comando que nada faz dali. Nunca devolve outro
/// `RunCommandKey` (um salto so).
pub(in crate::windows_app) fn command_key_event(press: &UserEvent) -> Option<UserEvent> {
    let &UserEvent::RunCommandKey { key, origin } = press else {
        return None;
    };
    debug_log(format_args!(
        "atalho: {} ({})",
        key.key(),
        origin.describe()
    ));
    resolve_command(key, origin)
}

/// O comando que um clique num alvo da barra corre (da janela), para a
/// paleta passar os cliques por `resolve_command`. Exaustivo, sem `_`: um
/// alvo novo na barra tem de dizer aqui se e um comando.
#[cfg_attr(not(test), allow(dead_code))]
pub(in crate::windows_app) fn bar_hit_command(hit: BarHit) -> Option<CommandId> {
    match hit {
        BarHit::Home => Some(CommandId::Home),
        BarHit::SplitClose => Some(CommandId::CloseSplit),
        BarHit::SplitExpand => Some(CommandId::SplitFullscreen),
        BarHit::WindowClose => Some(CommandId::Exit),
        BarHit::Downloads => Some(CommandId::Downloads),
        // Gestos sobre um item concreto (a aba, o grupo, a coluna, a faixa
        // do servico, a pagina ao lado): o alvo debaixo do rato e parte do
        // pedido, nao um comando.
        BarHit::Back
        | BarHit::Forward
        | BarHit::ColumnBack(_)
        | BarHit::ColumnForward(_)
        | BarHit::ColumnReload(_)
        | BarHit::ColumnTranslate(_)
        // A estrela e da pagina debaixo dela (o Ctrl+D e o mesmo pedido
        // com a origem do teclado).
        | BarHit::ColumnMinimize(_)
        | BarHit::ColumnBookmark(_)
        | BarHit::SplitBookmark
        | BarHit::SplitMinimize
        | BarHit::Column(_)
        | BarHit::AddTab(_)
        | BarHit::ContextTab { .. }
        | BarHit::CloseTab { .. }
        | BarHit::ContextGroup { .. }
        | BarHit::TabOverflow(_)
        | BarHit::ServiceStrip(_) => None,
        // Botoes cujo comando ainda nao esta no registo: a paleta e as
        // features que os donos trazem registam-nos.
        BarHit::Private
        | BarHit::Service(_)
        | BarHit::GmailToggle
        | BarHit::Tool(_)
        | BarHit::GeminiLive
        | BarHit::WindowMinimize
        | BarHit::WindowMaximize
        | BarHit::AddressBar => None,
    }
}

pub(in crate::windows_app) fn is_native_shortcut(id: CommandId) -> bool {
    if matches!(
        id,
        CommandId::PrintPage
            | CommandId::ZoomIn
            | CommandId::ZoomOut
            | CommandId::ZoomReset
            | CommandId::NextTab
            | CommandId::PrevTab
            | CommandId::ToggleGroup
    ) {
        return true;
    }
    if id == CommandId::LiveStop {
        return true;
    }
    matches!(
        id,
        CommandId::FocusAddress
            | CommandId::NavBack
            | CommandId::NavForward
            | CommandId::LiveToggle
            | CommandId::PrivatePanel
            | CommandId::AboutPanel
            | CommandId::ShortcutMap
            | CommandId::PomodoroToggle
            | CommandId::BreathPanel
            | CommandId::MeetPanel
            | CommandId::TeamsPanel
            | CommandId::OutlookPanel
            | CommandId::WhatsAppPanel
            | CommandId::YoutubePanel
            | CommandId::GmailPanel
            | CommandId::PanelMinimize
            | CommandId::PanelFullscreen
            | CommandId::PanelClose
            | CommandId::TranslatePage
            | CommandId::ColumnOne
            | CommandId::ColumnTwo
            | CommandId::ColumnThree
            | CommandId::TabList
            | CommandId::WindowMin
            | CommandId::WindowMax
    )
}

impl App {
    pub(in crate::windows_app) fn run_native_shortcut(
        &mut self,
        id: CommandId,
        origin: CommandOrigin,
    ) {
        use crate::panel_chrome::ServiceInput;
        use crate::windows_app::app::navigation::HistoryStep;
        use crate::windows_app::bar_layout::ToolAction;
        let column = match origin {
            CommandOrigin::Host(
                WebViewHost::Column(index)
                | WebViewHost::Split(index)
                | WebViewHost::PrivateSplit(index),
            ) => index,
            _ => self
                .comparator
                .as_ref()
                .and_then(|comp| comp.expanded)
                .unwrap_or(0),
        };
        match id {
            CommandId::PrintPage => match origin {
                CommandOrigin::Host(WebViewHost::Column(index)) => {
                    self.print_target(PageTarget::Column(index))
                }
                CommandOrigin::Host(WebViewHost::Split(_) | WebViewHost::PrivateSplit(_)) => {
                    self.print_target(PageTarget::Split)
                }
                _ => self.print_page(),
            },
            CommandId::ZoomIn => {
                let event: Option<UserEvent> = Some(UserEvent::ZoomIn);
                if let Some(event) = event {
                    let _ = self.proxy.send_event(event);
                }
            }
            CommandId::ZoomOut => {
                let event: Option<UserEvent> = Some(UserEvent::ZoomOut);
                if let Some(event) = event {
                    let _ = self.proxy.send_event(event);
                }
            }
            CommandId::ZoomReset => {
                let event: Option<UserEvent> = Some(UserEvent::ZoomReset);
                if let Some(event) = event {
                    let _ = self.proxy.send_event(event);
                }
            }
            CommandId::NextTab => self.cycle_context_shortcut(column, false),
            CommandId::PrevTab => self.cycle_context_shortcut(column, true),
            CommandId::ToggleGroup => self.toggle_active_group_shortcut(column),
            CommandId::FocusAddress => {
                self.focus_omnibox();
            }
            CommandId::NavBack => {
                if let CommandOrigin::Host(WebViewHost::Column(index)) = origin {
                    self.navigate_column(index, HistoryStep::Back);
                } else {
                    self.navigate_history(HistoryStep::Back);
                }
            }
            CommandId::NavForward => {
                if let CommandOrigin::Host(WebViewHost::Column(index)) = origin {
                    self.navigate_column(index, HistoryStep::Forward);
                } else {
                    self.navigate_history(HistoryStep::Forward);
                }
            }
            CommandId::LiveStop => {
                self.close_live_panel();
            }
            CommandId::LiveToggle => {
                self.toggle_live_panel();
            }
            CommandId::PrivatePanel => {
                self.open_private_panel();
            }
            CommandId::AboutPanel => {
                self.show_about();
            }
            CommandId::ShortcutMap => {
                {
                    self.show_about();
                    self.panel_run("window.neuraliaShowSection('shortcuts');".to_string());
                };
            }
            CommandId::PomodoroToggle => {
                self.run_tool_action(ToolAction::PomodoroClick);
            }
            CommandId::BreathPanel => {
                self.open_service_panel(Service::Breath);
            }
            CommandId::MeetPanel => {
                self.open_service_panel(Service::Meet);
            }
            CommandId::TeamsPanel => {
                self.open_service_panel(Service::Teams);
            }
            CommandId::OutlookPanel => {
                self.open_service_panel(Service::Outlook);
            }
            CommandId::WhatsAppPanel => {
                self.open_service_panel(Service::WhatsApp);
            }
            CommandId::YoutubePanel => {
                self.open_service_panel(Service::YouTube);
            }
            CommandId::GmailPanel => {
                self.open_service_panel(Service::Gmail);
            }
            CommandId::PanelMinimize | CommandId::PanelFullscreen | CommandId::PanelClose => {
                let target = panel_shortcut_target(
                    origin,
                    self.live_panel.is_open(),
                    self.live_panel.is_docked(),
                    self.side_panel.is_open(),
                    self.service_panel.is_some(),
                );
                match panel_shortcut_action(id, target, self.live_panel.is_minimized()) {
                    PanelShortcutAction::MinimizeLive => self.minimize_live_panel(),
                    PanelShortcutAction::RestoreLive => self.restore_live_panel(),
                    PanelShortcutAction::CloseSidePanel => {
                        self.close_side_panel(PanelExit::CloseButton)
                    }
                    PanelShortcutAction::MinimizeService => {
                        self.service_input(ServiceInput::Minimize)
                    }
                    PanelShortcutAction::FullscreenService => {
                        self.service_input(ServiceInput::ToggleFullscreen)
                    }
                    PanelShortcutAction::CloseService => self.conceal_service_panel(),
                    PanelShortcutAction::Nothing => {}
                }
            }
            CommandId::TranslatePage => {
                self.translation_event(TranslateEvent::Requested(WebViewHost::Column(column)));
            }
            CommandId::ColumnOne => {
                self.expand_comparator(0);
            }
            CommandId::ColumnTwo => {
                self.expand_comparator(1);
            }
            CommandId::ColumnThree => {
                self.expand_comparator(2);
            }
            CommandId::TabList => {
                self.show_tab_list_menu(column);
            }
            CommandId::WindowMin => {
                if let Some(window) = &self.window {
                    window.set_minimized(true);
                }
            }
            CommandId::WindowMax => {
                if let Some(window) = &self.window {
                    window.set_maximized(!window.is_maximized());
                }
            }
            _ => {}
        }
    }
}

pub(in crate::windows_app) fn shortcut_text(chord: Chord) -> String {
    let key = match chord.vk {
        0x70..=0x87 => format!("F{}", chord.vk - 0x70 + 1),
        0x25 => "←".into(),
        0x27 => "→".into(),
        0x28 => "↓".into(),
        0x09 => "Tab".into(),
        0xBB => "+".into(),
        0x6B => "Num+".into(),
        0xBD => "-".into(),
        0x6D => "Num-".into(),
        0x24 => "Home".into(),
        0x2e => "Delete".into(),
        value => char::from_u32(value).unwrap_or('?').to_string(),
    };
    format!(
        "{}{}{}{}",
        if chord.ctrl { "Ctrl+" } else { "" },
        if chord.shift { "Shift+" } else { "" },
        if chord.alt { "Alt+" } else { "" },
        key
    )
}

pub(in crate::windows_app) fn shortcut_map_script() -> String {
    let mut rows: Vec<_> = COMMANDS.iter().map(|row| serde_json::json!({
        "label": row.label, "keys": row.chords.iter().map(|spec| shortcut_text(spec.chord)).collect::<Vec<_>>().join(" / ")
    })).collect();
    if let Some(live_rows) = crate::gemini_live::live_panel_shortcuts().as_array() {
        rows.extend(live_rows.iter().filter(|row| row["native"] != true).map(|row|
            serde_json::json!({"label":format!("Gemini Live: {}",row["label"].as_str().unwrap_or("")),"keys":row["keys"]})));
    }
    format!(
        "window.neuraliaSetShortcuts && window.neuraliaSetShortcuts({});",
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum PanelShortcutTarget {
    Live,
    SidePanel,
    Service,
    Nothing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum PanelShortcutAction {
    MinimizeLive,
    RestoreLive,
    CloseSidePanel,
    MinimizeService,
    FullscreenService,
    CloseService,
    Nothing,
}
pub(in crate::windows_app) fn panel_shortcut_target(
    origin: CommandOrigin,
    live_open: bool,
    live_docked: bool,
    side_open: bool,
    service_open: bool,
) -> PanelShortcutTarget {
    match origin {
        CommandOrigin::Host(WebViewHost::Live) => PanelShortcutTarget::Live,
        CommandOrigin::Host(WebViewHost::SidePanel) => PanelShortcutTarget::SidePanel,
        CommandOrigin::Host(WebViewHost::Service(_)) => PanelShortcutTarget::Service,
        _ if live_docked => PanelShortcutTarget::Live,
        _ if side_open => PanelShortcutTarget::SidePanel,
        _ if service_open => PanelShortcutTarget::Service,
        _ if live_open => PanelShortcutTarget::Live,
        _ => PanelShortcutTarget::Nothing,
    }
}
pub(in crate::windows_app) fn panel_shortcut_action(
    command: CommandId,
    target: PanelShortcutTarget,
    live_minimized: bool,
) -> PanelShortcutAction {
    match (command, target) {
        (CommandId::PanelMinimize, PanelShortcutTarget::Live) if live_minimized => {
            PanelShortcutAction::RestoreLive
        }
        (CommandId::PanelMinimize | CommandId::PanelClose, PanelShortcutTarget::Live) => {
            PanelShortcutAction::MinimizeLive
        }
        (CommandId::PanelMinimize | CommandId::PanelClose, PanelShortcutTarget::SidePanel) => {
            PanelShortcutAction::CloseSidePanel
        }
        (CommandId::PanelMinimize, PanelShortcutTarget::Service) => {
            PanelShortcutAction::MinimizeService
        }
        (CommandId::PanelFullscreen, PanelShortcutTarget::Service) => {
            PanelShortcutAction::FullscreenService
        }
        (CommandId::PanelClose, PanelShortcutTarget::Service) => PanelShortcutAction::CloseService,
        _ => PanelShortcutAction::Nothing,
    }
}
