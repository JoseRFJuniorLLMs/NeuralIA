use std::{path::PathBuf, sync::atomic::Ordering};

use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            EnableWindow, GetAsyncKeyState, SetFocus, VK_CONTROL, VK_MENU, VK_SHIFT,
        },
        WindowsAndMessaging::{
            IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO, MessageBoxW, SW_HIDE, SW_SHOW,
            SendMessageW, SetWindowTextW, ShowWindow, WM_CHAR, WM_KEYDOWN,
        },
    },
};
use winit::event_loop::EventLoopProxy;

use crate::pomodoro_ui::{POMODORO_COMMAND_HELP, PomodoroCommand, parse_pomodoro_command};
use crate::windows_app::*;
use crate::windows_app::{
    App, COMPARATOR_COLUMNS, HistoryEntry, HistoryKind, Intent, LIFECYCLE_LAST_HOME_NONCE, Surface,
    THEME_COMMAND_HELP, UserEvent, debug_log,
    native::{DefSubclassProc, EM_SETSEL, lifecycle_probe_enabled, lifecycle_probe_home_message},
    search_card::{CompareRequest, TRANSLATE_COMMAND},
    theme::ThemeChoice,
    wide_null, window_hwnd, window_text,
};
use neural_core::parse_intent;

/// Quantas entradas do historico a caixa "history:" mostra.
pub(in crate::windows_app) const HISTORY_RECENT_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum HistoryStep {
    Back,
    Forward,
}

/// Qual pagina o ‹ › move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum HistoryNav {
    /// A fonte aberta ao lado de uma coluna (onde se seguem links).
    Split,
    /// A coluna expandida.
    Column(usize),
    /// A pagina cheia (Web ou Leitor).
    Page,
    /// Sem uma pagina so: voltar e o do app.
    App,
}

pub(in crate::windows_app) fn history_nav_target(
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

/// Apagar TUDO so com um "Sim" explicito. Fechar a caixa, "Nao" ou uma caixa
/// que nem abriu (0) deixam o historico como estava.
pub(in crate::windows_app) fn clear_history_confirmed(answer: i32) -> bool {
    answer == IDYES
}

/// Para onde vai o texto que o utilizador submeteu, decidido sem tocar na
/// janela, na memória, na rede nem no agente. É a SPEC-0106 -- a composição do
/// produto -- num sítio onde um teste lhe consegue chegar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum InputRoute {
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
pub(in crate::windows_app) const TRANSLATE_COMMAND_HELP: &str =
    "Escreva o texto depois de traduzir:";

/// `text` sem `prefix` a frente, se comecar por ele (maiusculas ou nao).
pub(in crate::windows_app) fn strip_prefix_ignore_ascii_case<'a>(
    text: &'a str,
    prefix: &str,
) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

pub(in crate::windows_app) fn route_input(input: &str) -> InputRoute {
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

/// Para onde vai o que o utilizador escreveu na palette. Puro, para se poder
/// testar sem janela: e aqui que se decide que um painel privado nunca
/// carrega nada na coluna normal nem passa pelo historico.
#[derive(Debug, PartialEq)]
pub(in crate::windows_app) enum PaletteRoute {
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

pub(in crate::windows_app) fn route_palette(
    input: &str,
    source_index: usize,
    private: bool,
) -> PaletteRoute {
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

/// Um `WM_KEYDOWN` da omnibox como a tecla do mapa de teclas: a tecla
/// virtual, os modificadores lidos agora e a repeticao (o bit 30 do
/// `lParam`: a tecla ja estava em baixo).
pub(in crate::windows_app) fn omnibox_accelerator_input(
    virtual_key: usize,
    lparam: LPARAM,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> AcceleratorInput {
    AcceleratorInput {
        vk: virtual_key as u32,
        down: true,
        ctrl,
        shift,
        alt,
        repeat: (lparam >> 30) & 1 == 1,
    }
}

pub(in crate::windows_app) unsafe extern "system" fn omnibox_subclass(
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
        let alt = (GetAsyncKeyState(VK_MENU as i32) as u16 & 0x8000) != 0;

        // Os atalhos: o mesmo mapa de teclas da janela e das WebViews, com a
        // omnibox como origem (Ctrl+H, Ctrl+N, Ctrl+O, Ctrl+R, Ctrl+Shift+R,
        // Ctrl+Shift+Z, Ctrl+Shift+Delete). Um atalho preso nao chega ao EDIT.
        let decision = keymap_decision(
            omnibox_accelerator_input(wparam, lparam, ctrl, shift, alt),
            CommandOrigin::Omnibox,
        );
        if decision.handled {
            if let Some(event) = decision.event {
                let _ = proxy.send_event(event);
            }
            return 0;
        }

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
            _ => {}
        }
    }
    // O Ctrl+Shift+Z (nota nova, no mapa de teclas) ja foi tratado: o
    // carater 0x1A que o TranslateMessage gera a seguir seria o "desfazer"
    // do EDIT. O Ctrl+Z sozinho continua a desfazer.
    if message == WM_CHAR
        && wparam == 0x1A
        && (GetAsyncKeyState(VK_SHIFT as i32) as u16 & 0x8000) != 0
    {
        return 0;
    }

    DefSubclassProc(hwnd, message, wparam, lparam)
}

impl App {
    pub(in crate::windows_app) fn set_omnibox_text(&self, text: &str) {
        let Some(edit) = self.omnibox else {
            return;
        };
        let value = wide_null(text);
        unsafe {
            SetWindowTextW(edit, value.as_ptr());
            SendMessageW(edit, EM_SETSEL, 0, -1);
        }
    }

    pub(in crate::windows_app) fn set_omnibox_visibility(&self, visible: bool, focus: bool) {
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

    pub(in crate::windows_app) fn show_omnibox(&self, visible: bool) {
        self.set_omnibox_visibility(visible, visible);
    }

    pub(in crate::windows_app) fn show_omnibox_passive(&self, visible: bool) {
        self.set_omnibox_visibility(visible, false);
    }

    pub(in crate::windows_app) fn omnibox_text(&self) -> String {
        self.omnibox
            .map(|edit| unsafe { window_text(edit) })
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// O equivalente ao Ctrl+L do Chrome. Na Home foca a caixa principal;
    /// no comparador abre a palette flutuante da coluna ativa.
    pub(in crate::windows_app) fn focus_omnibox(&mut self) {
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

    pub(in crate::windows_app) fn handle_input(&mut self, input: String) {
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

    pub(in crate::windows_app) fn submit_current(&mut self) {
        let input = self.omnibox_text();
        if !input.is_empty() {
            self.handle_input(input);
        }
    }

    /// Entrada da palette nativa. `source_index` e `private` vem do estado
    /// nativo escrito ao abrir a palette, nunca da pagina; a decisao de rota
    /// e pura (`route_palette`) e testada sem janela.
    pub(in crate::windows_app) fn submit_palette(
        &mut self,
        source_index: usize,
        input: String,
        private: bool,
    ) {
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
    pub(in crate::windows_app) fn go_back(&mut self) {
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
    pub(in crate::windows_app) fn navigate_history(&mut self, step: HistoryStep) {
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
    pub(in crate::windows_app) fn navigate_column(&mut self, index: usize, step: HistoryStep) {
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

    /// Pede a lista ao worker; a caixa aparece quando `HistoryLoaded` voltar.
    /// A leitura (lock + ficheiro inteiro) nunca corre no event loop.
    pub(in crate::windows_app) fn show_recent_history(&self) {
        if let Some(result) = self.history.recent(HISTORY_RECENT_LIMIT) {
            self.show_history_entries(result);
        }
    }

    pub(in crate::windows_app) fn show_history_entries(
        &self,
        result: Result<Vec<HistoryEntry>, String>,
    ) {
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
    pub(in crate::windows_app) fn confirm_clear_history(&self) -> bool {
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return false;
        };
        let body = wide_null(
            "Apagar TODO o histórico e a memória local da NeuralIA?\n\nNos livros, some o registro de quando cada um foi aberto; a posição de leitura e os marcadores ficam.\n\nA lista de downloads também se apaga; os arquivos baixados ficam.\n\nIsto não pode ser desfeito.",
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

    /// Liga/desliga a rolagem de leitura. O temporizador e nativo e nao vive na
    /// pagina: assim sobrevive a navegacao dentro do site.
    pub(in crate::windows_app) fn toggle_auto_scroll(&mut self) {
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

    /// Abrir um documento: anuncia e da tempo de se comecar a ler em paz antes
    /// do primeiro avanco.
    pub(in crate::windows_app) fn begin_reading_session(&mut self, is_pdf: bool) {
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
            SplashKind::Question(AUTO_SCROLL_QUESTION),
        ) {
            self.present_splash(frame);
        }
    }

    /// A resposta a uma pergunta do meio da janela: o botao `index` da
    /// pergunta de `asker`.
    pub(in crate::windows_app) fn answer_splash(&mut self, asker: SplashAsker, index: usize) {
        match asker {
            // Sim e o primeiro botao de `AUTO_SCROLL_QUESTION`.
            SplashAsker::AutoScroll => self.answer_auto_scroll(index == 0),
        }
    }

    /// Sem resposta nao se mexe: se a pergunta desaparecer sozinha, fica "nao"
    /// ate a pessoa carregar em F8.
    pub(in crate::windows_app) fn answer_auto_scroll(&mut self, yes: bool) {
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
    pub(in crate::windows_app) fn step_zoom(&mut self, direction: i32) {
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

    pub(in crate::windows_app) fn set_zoom(&mut self, zoom: f64) {
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

    pub(in crate::windows_app) fn reload_target(&mut self, target: PageTarget) {
        let reloaded = self
            .page_target_webview(target)
            .is_some_and(|webview| webview.reload().is_ok());
        if !reloaded {
            self.show_splash("Não há página ativa para recarregar.".to_string(), 3);
        }
    }

    pub(in crate::windows_app) fn print_target(&mut self, target: PageTarget) {
        let printed = self
            .page_target_webview(target)
            .is_some_and(|webview| webview.print().is_ok());
        if !printed {
            self.show_splash("Não há página ativa para imprimir.".to_string(), 3);
        }
    }

    pub(in crate::windows_app) fn open_devtools_target(&mut self, target: PageTarget) {
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

    pub(in crate::windows_app) fn view_source_target(&mut self, target: PageTarget) {
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

    pub(in crate::windows_app) fn reload_page(&mut self) {
        self.for_each_visible_webview(|webview| {
            let _ = webview.reload();
        });
    }

    pub(in crate::windows_app) fn print_page(&mut self) {
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

    /// Inspetor do Chromium, o mesmo que o F12 abre num navegador.
    pub(in crate::windows_app) fn open_devtools(&mut self) {
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
    pub(in crate::windows_app) fn view_source(&mut self) {
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

    pub(in crate::windows_app) fn toggle_column_fullscreen(&mut self) {
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

    pub(in crate::windows_app) fn auto_scroll_tick(&mut self, token: u64) {
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
}
