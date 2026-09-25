use std::sync::atomic::Ordering;

use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{ClientToScreen, InvalidateRect, ScreenToClient},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SW_HIDE, SWP_NOACTIVATE, SetWindowPos, ShowWindow,
    },
};
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event_loop::EventLoopProxy,
    window::Fullscreen,
};
use wry::{NewWindowResponse, PermissionKind, PermissionResponse, WebViewExtWindows};

use neural_core::{HistoryEntry, MemoryHit};

use crate::gemini_live::{
    LIVE_PROTOCOL, LiveAction, LiveKeyStore, LiveMessage, live_ipc_message, live_page_url,
    live_panel_navigation, live_step, serve_live_asset,
};
use crate::panel_chrome::{
    Area, EXIT_PAGE_FULLSCREEN_SCRIPT, PANEL_HANDLE_WIDTH, PANEL_WIDTHS_FILE, PanelKind,
    SERVICE_STRIP_HEIGHT, ScreenRect, ServiceBadge, ServiceEffect, ServiceFrame, ServiceInput,
    ServicePanelState, panel_area, panel_handle_area, panel_width, panel_width_from_drag,
};
use crate::windows_app::{
    AUX_POPUP_EX_STYLE, AUX_POPUP_STYLE, App, BarHit, COMPARATOR_CHROME_HEIGHT, Surface,
    TITLE_TAB_HEIGHT, Tool, WebViewHost, debug_log, install_wheel_hook,
    native::{
        PANEL_HANDLE_SUBCLASS_ID, PANEL_RESIZE_PENDING, PANEL_RESIZE_X, SetWindowSubclass,
        WHEEL_APP_HWND, WHEEL_PANEL_ACTIVE, WHEEL_PANEL_BOTTOM, WHEEL_PANEL_HOST, WHEEL_PANEL_LEFT,
        WHEEL_PANEL_RIGHT, WHEEL_PANEL_TOP, panel_handle_subclass, uninstall_wheel_hook,
    },
    notes::{NOTE_SAVE_REFUSED, NotesOrigin, NotesReply, notes_command_for, notes_reply_script},
    page_scripts::{PANEL_NEW_NOTE_SCRIPT, PANEL_SHOW_NOTES_SCRIPT},
    services::{
        Service, ServicePanel, close_service_panel_in, logical_rect, open_panel_width_for,
        raise_webview_host, register_service_panel_events, service_event_is_current,
        service_panel_navigation, service_panel_permission,
    },
    show_popup_without_activation,
    side_panel::{
        self, PANEL_RECENT_LIMIT, PANEL_SUGGESTION_LIMIT, PanelExit, PanelMessage,
        history_panel_items, memory_panel_items, panel_allows_navigation, panel_bounds, panel_html,
        panel_render_script, panel_theme_vars, suggestion_panel_items,
    },
    theme::{Theme, themed_webview_builder},
    web_media_permission, window_hwnd,
};

/// A dica do icone do servico aberto: minimizado, diz que volta ao clique (e
/// se continua a tocar); aberto, que fecha.
pub(in crate::windows_app) fn service_icon_hint(
    label: &str,
    badge: Option<ServiceBadge>,
) -> String {
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
pub(in crate::windows_app) fn service_panel_hint(
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

/// Onde comecam os paineis da direita (Ctrl+H, servicos, Gemini Live), em
/// pixels logicos. No comparador, abaixo da barra. Na Home, abaixo da fila dos
/// botoes da janela: com o painel a comecar no topo, a WebView dele tapava a
/// zona que os acorda, a janela principal nunca via o rato la e minimizar,
/// maximizar e fechar ficavam impossiveis de encontrar com o painel aberto.
pub(in crate::windows_app) fn right_panel_top(surface: Surface) -> f64 {
    match surface {
        Surface::Comparator => COMPARATOR_CHROME_HEIGHT,
        Surface::Home => TITLE_TAB_HEIGHT,
        _ => 0.0,
    }
}

/// Os pedidos de permissao do painel do Gemini Live. Camera, microfone e
/// captura de ecra so pelo aviso do proprio WebView2 (`Default`); o resto e
/// recusado. Nunca um `Allow`: e o utilizador quem decide, no aviso. O painel
/// passa ESTA funcao ao `with_permission_handler`, e e ela que o gate chama.
pub(in crate::windows_app) fn live_panel_permission(kind: PermissionKind) -> PermissionResponse {
    web_media_permission(kind, true)
}

/// O handler do canal do painel do Gemini Live, tal como o wry o recebe. So
/// mensagens publicadas pela pagina do painel e da lista fechada chegam a
/// `send` (no app, o proxy do event loop; nos gates, um registo).
pub(in crate::windows_app) fn live_panel_ipc_handler<S>(
    send: S,
) -> impl Fn(wry::http::Request<String>) + 'static
where
    S: Fn(crate::windows_app::UserEvent) + 'static,
{
    move |request| {
        if let Some(message) = live_ipc_message(&request.uri().to_string(), request.body()) {
            send(crate::windows_app::UserEvent::Live(message));
        }
    }
}

impl App {
    /// Os icones da barra: o servico abre no painel ao lado; de novo, fecha
    /// -- ou, minimizado, volta (`ServicePanelState`).
    pub(in crate::windows_app) fn open_service_panel(&mut self, service: Service) {
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
    pub(in crate::windows_app) fn close_docked_service_panel(&mut self) {
        if self
            .service_panel
            .as_ref()
            .is_some_and(|panel| !panel.state.minimized())
        {
            self.close_service_panel();
        }
    }

    pub(in crate::windows_app) fn close_service_panel(&mut self) {
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
    pub(in crate::windows_app) fn panel_space(&self) -> Option<(f64, f64, f64, f64)> {
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

    pub(in crate::windows_app) fn chosen_panel_width(
        &self,
        kind: PanelKind,
        logical_w: f64,
    ) -> f64 {
        panel_width(kind, self.panel_widths.get(kind), logical_w)
    }

    /// O que o painel de servicos, no modo `state`, pede a janela agora.
    pub(in crate::windows_app) fn service_frame_for(
        &self,
        state: ServicePanelState,
    ) -> Option<ServiceFrame> {
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

    pub(in crate::windows_app) fn service_frame(&self) -> Option<ServiceFrame> {
        self.service_frame_for(self.service_panel.as_ref()?.state)
    }

    pub(in crate::windows_app) fn service_covers_window(&self) -> bool {
        self.service_frame()
            .is_some_and(|frame| frame.window_fullscreen)
    }

    pub(in crate::windows_app) fn service_event_is_current(&self, generation: u64) -> bool {
        service_event_is_current(
            self.service_panel.as_ref().map(|panel| panel.generation),
            generation,
        )
    }

    pub(in crate::windows_app) fn position_service_panel(&self) {
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
    pub(in crate::windows_app) fn service_input(&mut self, input: ServiceInput) {
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
    pub(in crate::windows_app) fn apply_service_frame(&mut self) {
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
    pub(in crate::windows_app) fn docked_right_panel(&self) -> Option<(PanelKind, Area)> {
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
    pub(in crate::windows_app) fn visible_right_panel(&self) -> Option<(Area, HWND)> {
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
    pub(in crate::windows_app) fn after_panel_change(&mut self) {
        self.sync_panel_handle();
        self.sync_wheel_route();
    }

    /// A roda sobre o painel da direita vai para o painel (`wheel_hook`).
    pub(in crate::windows_app) fn sync_wheel_route(&self) {
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
    pub(in crate::windows_app) fn sync_panel_handle(&mut self) {
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
                let proxy_ptr = (&*self.omnibox_proxy
                    as *const EventLoopProxy<crate::windows_app::UserEvent>)
                    as usize;
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

    pub(in crate::windows_app) fn hide_panel_handle(&self) {
        if let Some(handle) = self.panel_handle {
            unsafe {
                ShowWindow(handle, SW_HIDE);
            }
        }
    }

    /// A pega foi arrastada: a borda do painel segue o rato, presa a
    /// [300 px, 60% da janela], e as colunas refluem para o lado.
    pub(in crate::windows_app) fn resize_panel(&mut self) {
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
    pub(in crate::windows_app) fn relayout_right_panels(&mut self) {
        self.position_side_panel();
        self.position_service_panel();
        self.position_live_panel();
        self.fit_comparator_to_panel();
        self.after_panel_change();
        self.needs_clear = true;
        self.request_redraw();
    }

    /// Largou a pega: a largura fica gravada (escrita atomica).
    pub(in crate::windows_app) fn save_panel_widths(&mut self) {
        let path = self.config.data_dir.join(PANEL_WIDTHS_FILE);
        if let Err(error) = self.panel_widths.save(&path) {
            self.show_splash(format!("A largura do painel não foi gravada: {error}"), 3);
        }
    }

    /// O olho da barra: liga o Gemini Live (abre o painel, que pede a chave
    /// na primeira vez e depois liga tela, camera e microfone); de novo,
    /// desliga -- fechar o painel destroi a pagina e com ela tudo o que
    /// estava a ser capturado.
    pub(in crate::windows_app) fn toggle_live_panel(&mut self) {
        if self.live_panel.is_open() {
            self.close_live_panel();
        } else {
            self.open_live_panel();
        }
    }

    pub(in crate::windows_app) fn live_panel_rect(&self) -> Option<wry::Rect> {
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

    pub(in crate::windows_app) fn open_live_panel(&mut self) {
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

    pub(in crate::windows_app) fn close_live_panel(&mut self) {
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

    pub(in crate::windows_app) fn position_live_panel(&self) {
        if let (Some(panel), Some(bounds)) = (self.live_panel.view(), self.live_panel_rect()) {
            let _ = panel.set_bounds(bounds);
        }
    }

    pub(in crate::windows_app) fn live_eval(&self, script: &str) {
        if let Some(panel) = self.live_panel.view() {
            let _ = panel.evaluate_script(script);
        }
    }

    pub(in crate::windows_app) fn handle_live_message(&mut self, message: LiveMessage) {
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

    /// Largura que o painel aberto ocupa a direita do comparador (0 sem painel).
    pub(in crate::windows_app) fn open_panel_width(&self) -> f64 {
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
    pub(in crate::windows_app) fn fit_comparator_to_panel(&mut self) {
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
    pub(in crate::windows_app) fn toggle_side_panel(&mut self) {
        if self.side_panel.is_open() {
            self.close_side_panel(PanelExit::CtrlH);
        } else {
            self.open_side_panel();
        }
    }

    pub(in crate::windows_app) fn side_panel_rect(&self) -> Option<wry::Rect> {
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

    pub(in crate::windows_app) fn open_side_panel(&mut self) {
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
                    let _ = proxy.send_event(crate::windows_app::UserEvent::Panel(post));
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
    pub(in crate::windows_app) fn close_side_panel(&mut self, exit: PanelExit) {
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

    pub(in crate::windows_app) fn position_side_panel(&self) {
        if let (Some(panel), Some(bounds)) = (self.side_panel.view(), self.side_panel_rect()) {
            let _ = panel.set_bounds(bounds);
        }
    }

    pub(in crate::windows_app) fn panel_eval(&self, script: &str) {
        if let Some(panel) = self.side_panel.view() {
            let _ = panel.evaluate_script(script);
        }
    }

    /// Corre `script` no painel, ou guarda-o para o "ready" se a pagina
    /// ainda nao correu o script dela. Sem painel, nao faz nada.
    pub(in crate::windows_app) fn panel_run(&mut self, script: String) {
        if let Some(script) = self.side_panel.run(script) {
            self.panel_eval(&script);
        }
    }

    /// Abre (se preciso -- nunca fecha) o painel ja nas Notas e corre la os
    /// `scripts`, pela ordem.
    pub(in crate::windows_app) fn show_notes_panel(&mut self, scripts: Vec<String>) {
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

    /// Ctrl+Shift+Z na Home ou na barra: o painel nas Notas, com uma nota
    /// nova em branco no editor.
    pub(in crate::windows_app) fn new_note_in_panel(&mut self) {
        self.show_notes_panel(vec![PANEL_NEW_NOTE_SCRIPT.to_string()]);
    }

    pub(in crate::windows_app) fn handle_panel_message(&mut self, post: side_panel::PanelPost) {
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

    pub(in crate::windows_app) fn panel_show_history(
        &self,
        result: Result<Vec<HistoryEntry>, String>,
    ) {
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

    pub(in crate::windows_app) fn panel_show_memory(
        &mut self,
        query: &str,
        result: Result<Vec<MemoryHit>, String>,
    ) {
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
}
