use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, POINT};

use crate::pomodoro_ui::{
    PomodoroCommand, PomodoroController, PomodoroHost, PomodoroMenuItem, WindowAttention,
    pomodoro_menu_command,
};
use crate::windows_app::*;
use crate::windows_app::{
    App, BarHit, Surface, Timers,
    bar_layout::{BarLabel, ToolAction},
    bar_tooltip_label, hover_tooltip,
    icons::{ICON_SLOT_BREATH, ICON_SLOT_NOTES, ICON_SLOT_POMODORO},
    native::{flash_taskbar, window_hwnd},
    page_scripts::PANEL_NOTES_BUTTON_SCRIPT,
    refresh_hint_text,
    services::Service,
    theme::{Rgb, Theme},
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

    /// Botao direito no comparador: nas ferramentas vai para elas (so o
    /// Pomodoro tem menu); no resto, o menu das abas de sempre.
    pub(in crate::windows_app) fn right_click_comparator(&mut self) {
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

    pub(in crate::windows_app) fn right_click_home(&mut self) {
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

    /// Abre a palette nativa sobre a coluna `source_index`. E um popup Win32,
    /// nao um <input> no DOM: a pagina remota nem ve o que se escreve nem
    /// consegue submeter nada por ela.
    pub(in crate::windows_app) fn open_ai_palette(&mut self, source_index: usize) {
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
    pub(in crate::windows_app) fn palette_source(&self) -> Option<(usize, bool)> {
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
    pub(in crate::windows_app) fn position_palette(&self) {
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
    pub(in crate::windows_app) fn close_palette(&mut self) {
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
