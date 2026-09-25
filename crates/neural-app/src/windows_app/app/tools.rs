use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, POINT};

use crate::pomodoro_ui::{
    pomodoro_menu_command, PomodoroCommand, PomodoroController, PomodoroHost, PomodoroMenuItem,
    WindowAttention,
};
use crate::windows_app::{
    bar_layout::{BarLabel, ToolAction},
    bar_tooltip_label, hover_tooltip,
    icons::{ICON_SLOT_BREATH, ICON_SLOT_NOTES, ICON_SLOT_POMODORO},
    native::{flash_taskbar, window_hwnd},
    page_scripts::PANEL_NOTES_BUTTON_SCRIPT,
    refresh_hint_text,
    services::Service,
    theme::{Rgb, Theme},
    App, BarHit, Surface, Timers,
};

/// Quanto ficam no ecra os avisos do Pomodoro: os dos comandos, e os do fim
/// de uma fase (mais tempo: quem estava concentrado pode nao estar a olhar).
pub(in crate::windows_app) const POMODORO_NOTICE_SECONDS: u64 = 3;
pub(in crate::windows_app) const POMODORO_PHASE_END_SECONDS: u64 = 8;

/// As ferramentas da barra e da Home, na ordem em que aparecem (da esquerda
/// para a direita): pedidas pelo dono como botoes, ao lado dos servicos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum Tool {
    Pomodoro,
    /// Zettelkasten: as notas vivem no painel do Ctrl+H.
    Notes,
    /// Respiracao guiada (metodo Wim Hof): o video no painel anonimo.
    Breath,
}

impl Tool {
    pub(in crate::windows_app) const ALL: [Tool; 3] = [Tool::Pomodoro, Tool::Notes, Tool::Breath];

    pub(in crate::windows_app) fn icon_slot(self) -> usize {
        match self {
            Self::Pomodoro => ICON_SLOT_POMODORO,
            Self::Notes => ICON_SLOT_NOTES,
            Self::Breath => ICON_SLOT_BREATH,
        }
    }

    /// O tomate tem cor propria; as outras duas marcas sao brancas e seguem
    /// o tema, como a videochamada e o envelope.
    pub(in crate::windows_app) fn icon_tint(self, theme: &Theme) -> Option<Rgb> {
        match self {
            Self::Pomodoro => None,
            Self::Notes | Self::Breath => Some(theme.fg),
        }
    }

    /// A dica: o que o clique FAZ, como as outras dicas da barra.
    pub(in crate::windows_app) fn tooltip(self) -> &'static str {
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
pub(in crate::windows_app) fn tool_hint(tool: Tool, pomodoro: Option<&str>) -> String {
    match (tool, pomodoro) {
        (Tool::Pomodoro, Some(session)) => session.to_string(),
        _ => tool.tooltip().to_string(),
    }
}

/// A dica de uma ferramenta em `now`, com o Pomodoro da app: e o que a Home
/// (`update_home_tool_hover`), a barra (`bar_hint`) e o refresco de cada
/// segundo (`pomodoro_changed`) mostram.
pub(in crate::windows_app) fn tool_hint_at(
    tool: Tool,
    pomodoro: &PomodoroController,
    now: Instant,
) -> String {
    let session = match tool {
        Tool::Pomodoro => pomodoro.hint(now),
        Tool::Notes | Tool::Breath => None,
    };
    tool_hint(tool, session.as_deref())
}

/// A dica de um alvo da barra, como `App::bar_tooltip_text` a mostra: as
/// ferramentas pela `tool_hint_at` (a do Pomodoro diz a sessao), o resto
/// pela `bar_tooltip_label`.
pub(in crate::windows_app) fn bar_hint(
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

/// Som curto do sistema no fim de uma fase do Pomodoro (o "Asterisco" do
/// esquema de sons do Windows; sem som configurado, nada).
pub(in crate::windows_app) fn pomodoro_sound() {
    use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;
    unsafe {
        MessageBeep(MB_ICONASTERISK);
    }
}

/// Menu do botao direito do Pomodoro no cursor, feito das linhas de
/// `PomodoroController::menu_items` (o modelo e o `pick_theme_from_menu`).
/// Devolve o id escolhido; 0 se o menu fechou sem escolha.
pub(in crate::windows_app) fn pick_pomodoro_from_menu(
    hwnd: HWND,
    items: &[PomodoroMenuItem],
) -> usize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GA_ROOT, GetAncestor, GetCursorPos, MF_CHECKED,
        MF_GRAYED, MF_SEPARATOR, MF_STRING, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
        TrackPopupMenu,
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

impl App {
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
    pub(in crate::windows_app) fn open_notes(&mut self) {
        if self.side_panel.is_open() {
            self.panel_run(PANEL_NOTES_BUTTON_SCRIPT.to_string());
            return;
        }
        self.show_notes_panel(Vec::new());
    }

    /// Clique no botao do Pomodoro (barra ou Home): parado inicia, a correr
    /// pausa, pausado retoma.
    pub(in crate::windows_app) fn pomodoro_click(&mut self) {
        self.pomodoro_command(PomodoroCommand::Click);
    }

    /// Botao direito no Pomodoro: o menu nativo do estado de agora (so a
    /// accao que se aplica, Pular fase, Parar e os presets com a marca no
    /// que esta em uso).
    pub(in crate::windows_app) fn pomodoro_menu(&mut self) {
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
    pub(in crate::windows_app) fn pomodoro_command(&mut self, command: PomodoroCommand) {
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
    pub(in crate::windows_app) fn pomodoro_tick(&mut self, token: u64) {
        crate::pomodoro_ui::pomodoro_tick(self, token, Instant::now());
    }

    /// Um fim de fase do Pomodoro que a janela nao viu (estava minimizada ou
    /// atras de outra) aparece quando ela volta -- uma vez.
    pub(in crate::windows_app) fn show_unseen_phase_end(&mut self) {
        if let Some(message) = self.pomodoro.window_back() {
            self.show_background_splash(message, POMODORO_PHASE_END_SECONDS);
        }
    }

    /// Minimizada e a da frente, para o fim de uma fase do Pomodoro.
    pub(in crate::windows_app) fn window_attention(&self) -> WindowAttention {
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
    pub(in crate::windows_app) fn pomodoro_changed(&mut self) {
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
    pub(in crate::windows_app) fn pomodoro_label(&self) -> Option<String> {
        self.pomodoro.label(Instant::now())
    }

    /// A etiqueta ja no formato que a barra e a Home desenham e medem, com a
    /// fase para a cor.
    pub(in crate::windows_app) fn pomodoro_bar_label(&self) -> Option<BarLabel> {
        self.pomodoro_label()
            .as_deref()
            .and_then(BarLabel::new)
            .map(|label| label.with_phase(self.pomodoro.active_phase()))
    }

    pub(in crate::windows_app) fn run_tool_action(&mut self, action: ToolAction) {
        // O clique pode abrir um painel por cima do botao: a dica nao fica.
        hover_tooltip(std::ptr::null_mut(), "");
        match action {
            ToolAction::PomodoroClick => self.pomodoro_click(),
            ToolAction::PomodoroMenu => self.pomodoro_menu(),
            ToolAction::ToggleNotes => self.open_notes(),
            ToolAction::ToggleBreath => self.open_service_panel(Service::Breath),
        }
    }
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
