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
// o primeiro e o Ctrl+D dos favoritos (`CommandId::Bookmark`, ambito
// `Global`), que corre contra o hospedeiro de onde veio a tecla
// (`bookmark_target`) e nunca passa pelo IPC.

/// Um comando do registo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::windows_app) enum CommandId {
    /// Liga ou desliga a rolagem automatica.
    AutoScroll,
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
    /// Fechar a NeuralIA (as abas ficam gravadas).
    Exit,
    /// Ctrl+D: a pagina de onde veio para os favoritos; na Home (ou num
    /// painel), abre os Favoritos.
    Bookmark,
}

impl CommandId {
    /// Cada comando, uma vez: o que os gates percorrem.
    #[cfg(test)]
    pub(in crate::windows_app) const ALL: [CommandId; 12] = [
        CommandId::AutoScroll,
        CommandId::Reload,
        CommandId::History,
        CommandId::NewTab,
        CommandId::OpenEpub,
        CommandId::NewNote,
        CommandId::ClearHistory,
        CommandId::Home,
        CommandId::CloseSplit,
        CommandId::SplitFullscreen,
        CommandId::Exit,
        CommandId::Bookmark,
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
    /// WebView (menos o monitor do Gmail), preso no `AcceleratorKeyPressed`.
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
        id: CommandId::AutoScroll,
        key: "rolagem-automatica",
        label: "Rolagem automática",
        category: CommandCategory::Pagina,
        keywords: &["rolar", "rolagem", "scroll", "ler", "leitura"],
        chords: &[ChordSpec::window(Chord::ctrl(b'R'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::Reload,
        key: "recarregar",
        label: "Recarregar a página",
        category: CommandCategory::Pagina,
        keywords: &["recarregar", "atualizar", "reload"],
        chords: &[ChordSpec::window(Chord::ctrl_shift(b'R'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::History,
        key: "historico",
        label: "Histórico, memória e notas",
        category: CommandCategory::HistoricoMemoria,
        keywords: &["histórico", "historico", "memória", "notas", "painel"],
        chords: &[ChordSpec::window(Chord::ctrl(b'H'))],
        alias: None,
    },
    CommandRow {
        id: CommandId::NewTab,
        key: "nova-pergunta",
        label: "Nova pergunta",
        category: CommandCategory::PesquisaIa,
        keywords: &["nova", "aba", "pergunta", "perguntar"],
        chords: &[ChordSpec::window(Chord::ctrl(b'N'))],
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
        chords: &[],
        alias: None,
    },
    CommandRow {
        id: CommandId::CloseSplit,
        key: "fechar-fonte",
        label: "Fechar a fonte aberta ao lado",
        category: CommandCategory::FonteAoLado,
        keywords: &["fechar", "fonte", "lado", "split"],
        chords: &[],
        alias: None,
    },
    CommandRow {
        id: CommandId::SplitFullscreen,
        key: "expandir-fonte",
        label: "Expandir ou reduzir a fonte aberta ao lado",
        category: CommandCategory::FonteAoLado,
        keywords: &["expandir", "reduzir", "fonte", "lado", "tela cheia"],
        chords: &[],
        alias: None,
    },
    CommandRow {
        id: CommandId::Exit,
        key: "sair",
        label: "Fechar a NeuralIA",
        category: CommandCategory::Janela,
        keywords: &["sair", "fechar", "janela"],
        chords: &[],
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
        CommandId::AutoScroll => UserEvent::ToggleAutoScroll,
        CommandId::Reload => UserEvent::ReloadPage,
        CommandId::History => UserEvent::ShowHistory,
        CommandId::NewTab => UserEvent::NewTab(0),
        CommandId::OpenEpub => UserEvent::OpenEpubDialog,
        CommandId::NewNote => UserEvent::NewNote,
        CommandId::ClearHistory => UserEvent::ClearHistory,
        CommandId::Home => UserEvent::HomeRequested,
        CommandId::CloseSplit => UserEvent::CloseSplit,
        CommandId::SplitFullscreen => UserEvent::ToggleSplitFullscreen,
        CommandId::Exit => UserEvent::ExitRequested,
        CommandId::Bookmark => UserEvent::Bookmarks(BookmarksEvent::Request {
            target: BookmarkTarget::Window,
            via: BookmarkVia::Shortcut,
        }),
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
        CommandId::Reload => Some(IpcAction::Reload),
        CommandId::NewNote => Some(IpcAction::Note {
            via: NoteVia::Shortcut,
        }),
        CommandId::NewTab => Some(IpcAction::NewTab { col: column }),
        CommandId::AutoScroll
        | CommandId::History
        | CommandId::OpenEpub
        | CommandId::ClearHistory
        | CommandId::Home
        | CommandId::CloseSplit
        | CommandId::SplitFullscreen
        | CommandId::Exit
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
        // Gestos sobre um item concreto (a aba, o grupo, a coluna, a faixa
        // do servico, a pagina ao lado): o alvo debaixo do rato e parte do
        // pedido, nao um comando.
        BarHit::Back
        | BarHit::Forward
        | BarHit::ColumnBack(_)
        | BarHit::ColumnForward(_)
        // A estrela e da pagina debaixo dela (o Ctrl+D e o mesmo pedido
        // com a origem do teclado).
        | BarHit::ColumnBookmark(_)
        | BarHit::SplitBookmark
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
        | BarHit::WindowMaximize => None,
    }
}
