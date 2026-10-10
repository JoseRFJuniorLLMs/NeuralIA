use std::sync::atomic::Ordering;

use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{ClientToScreen, InvalidateRect, ScreenToClient},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetParent, HWND_TOP, SW_HIDE, SWP_NOACTIVATE,
        SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WS_CHILD,
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
    LIVE_PROTOCOL, LiveAction, LiveKeyStore, LiveMessage, LiveToolCall, live_ipc_message,
    live_page_url, live_step, live_tool_response_script, serve_live_asset,
};
use crate::panel_chrome::{
    Area, EXIT_PAGE_FULLSCREEN_SCRIPT, PANEL_HANDLE_WIDTH, PANEL_WIDTHS_FILE, PanelKind,
    SERVICE_STRIP_HEIGHT, ScreenRect, ServiceBadge, ServiceEffect, ServiceFrame, ServiceInput,
    ServicePanelState, panel_area, panel_handle_area, panel_width, panel_width_from_drag,
};
use crate::windows_app::*;
use crate::windows_app::{
    App, BarHit, COMPARATOR_CHROME_HEIGHT, Surface, TITLE_TAB_HEIGHT, Tool, WebViewHost, debug_log,
    install_wheel_hook,
    native::{
        PANEL_HANDLE_SUBCLASS_ID, PANEL_RESIZE_PENDING, PANEL_RESIZE_X, SetWindowSubclass,
        WHEEL_APP_HWND, WHEEL_PANEL_ACTIVE, WHEEL_PANEL_BOTTOM, WHEEL_PANEL_HOST, WHEEL_PANEL_LEFT,
        WHEEL_PANEL_RIGHT, WHEEL_PANEL_TOP, panel_handle_subclass, uninstall_wheel_hook,
    },
    notes::{
        NOTE_SAVE_REFUSED, NoteDraft, NotesCommand, NotesOrigin, NotesReply, notes_command_for,
        notes_reply_script,
    },
    page_scripts::{
        PANEL_NEW_NOTE_SCRIPT, PANEL_SHOW_ABOUT_SCRIPT, PANEL_SHOW_NOTES_SCRIPT,
        PANEL_SHOW_OBSIDIAN_SCRIPT,
    },
    services::{
        Service, ServicePanel, close_service_panel_in, logical_rect, open_panel_width_for,
        raise_webview_host, register_service_panel_events, service_event_is_current,
        service_panel_permission, start_whatsapp_with_notifications,
    },
    side_panel::{
        self, PANEL_RECENT_LIMIT, PANEL_SUGGESTION_LIMIT, PanelExit, PanelMessage,
        history_panel_items, memory_panel_items, panel_bounds, panel_html, panel_render_script,
        panel_theme_vars, suggestion_panel_items,
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

fn service_button_hint(service: Service, badge: Option<ServiceBadge>) -> Option<String> {
    if badge.is_none() && service.keeps_running_in_background() {
        Some(format!(
            "{} aberto ao lado · clique para ocultar, continua em segundo plano",
            service.label()
        ))
    } else {
        Some(service_icon_hint(service.label(), badge))
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
        BarHit::ServiceStrip(StripButton::Close) if service.keeps_running_in_background() => Some(
            format!("Ocultar {}: continua em segundo plano", service.label()),
        ),
        BarHit::ServiceStrip(button) => Some(button.hint(service.label())),
        BarHit::GmailToggle if service == Service::Gmail => service_button_hint(service, badge),
        BarHit::Service(hit_service) if hit_service == service => {
            service_button_hint(service, badge)
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

pub(in crate::windows_app) fn service_transition_input(
    service: Service,
    state: ServicePanelState,
) -> Option<ServiceInput> {
    if service.keeps_running_in_background() {
        (!state.minimized()).then_some(ServiceInput::Minimize)
    } else {
        Some(ServiceInput::Close)
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
                self.minimize_live_panel();
            }
            self.service_input(ServiceInput::IconClick);
            return;
        }
        self.close_service_panel();
        // Um painel de cada vez.
        self.close_side_panel(PanelExit::OtherPanel);
        self.minimize_live_panel();
        if let Some(index) = self
            .background_services
            .iter()
            .position(|panel| panel.service == service)
        {
            let mut panel = self.background_services.remove(index);
            panel.state = ServicePanelState::default();
            let _ = panel.webview.focus();
            self.service_panel = Some(panel);
            self.apply_service_frame();
            return;
        }
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
        // A trava de navegacao (NavGate::Service, a politica do servico) vem
        // de `hooked_builder`, como em todas as WebViews.
        let builder = themed_webview_builder()
            .with_incognito(service.private())
            .with_url(if service == Service::WhatsApp {
                "about:blank"
            } else {
                service.url()
            })
            .with_bounds(logical_rect(area))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            // Caminho A do WebRTC: camera e microfone pelo aviso do
            // WebView2 -- salvo no painel privado, onde sao recusados.
            .with_permission_handler(move |kind| service_panel_permission(service, kind));
        let hooked = self.hooked_builder(builder, WebViewHost::Service(service), None);
        let built = hooked.build_hooked_as_child(window);
        match built {
            Ok(panel) => {
                let _ = panel.focus();
                #[cfg(feature = "accel-spike")]
                self.accel_spike_hook(&panel, crate::accel_spike::SpikeHost::Service);
                self.service_generation = self.service_generation.wrapping_add(1);
                let generation = self.service_generation;
                // Sem estes avisos o painel abre na mesma; so a tela cheia da
                // pagina, o ponto "a tocar" e o Esc ficam de fora.
                if let Err(error) =
                    register_service_panel_events(&panel, generation, self.proxy.clone())
                {
                    debug_log(format_args!("service panel: sem avisos ({error})"));
                }
                if service == Service::WhatsApp
                    && let Err(error) = start_whatsapp_with_notifications(&panel)
                {
                    debug_log(format_args!("service panel: {error}"));
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
                if service == Service::Gmail {
                    self.schedule_gmail_probe(4);
                }
            }
            Err(error) => {
                self.show_splash(
                    format!("Não foi possível abrir {}: {error}", service.label()),
                    3,
                );
            }
        }
    }

    /// Uma transicao interna (Ctrl+H, Home, nova pesquisa, EPUB) preserva o
    /// WhatsApp e YouTube continuam vivos como abas de fundo minimizadas.
    pub(in crate::windows_app) fn service_panel_for_transition(&mut self) {
        let Some(panel) = self.service_panel.as_ref() else {
            return;
        };
        if let Some(input) = service_transition_input(panel.service, panel.state) {
            self.service_input(input);
        }
    }

    /// Outro painel da direita precisa do espaco. Os servicos persistentes minimizam;
    /// os demais servicos fecham como antes.
    pub(in crate::windows_app) fn close_docked_service_panel(&mut self) {
        if self
            .service_panel
            .as_ref()
            .is_some_and(|panel| !panel.state.minimized())
        {
            self.service_panel_for_transition();
        }
    }

    pub(in crate::windows_app) fn close_service_panel(&mut self) {
        let Some(service) = self.service_panel.as_ref().map(|panel| panel.service) else {
            return;
        };
        // O Gmail usa outro WebView para avisos. Antes de descartar o painel,
        // aproveita a sessao que a pessoa acabou de abrir nele.
        if service == Service::Gmail {
            self.maybe_start_gmail_monitor();
        }
        if service.keeps_running_in_background() {
            let mut panel = self.service_panel.take().expect("painel conferido acima");
            let _ = panel.webview.evaluate_script(EXIT_PAGE_FULLSCREEN_SCRIPT);
            let _ = panel.webview.set_visible(false);
            let _ = panel.webview.focus_parent();
            if self.surface == Surface::Home
                && let Some(edit) = self.omnibox
            {
                unsafe { SetFocus(edit) };
            }
            panel.state = ServicePanelState::default();
            let _ = panel.state.step(ServiceInput::Minimize);
            self.background_services.push(panel);
            debug_log(format_args!("service panel: {service:?} em segundo plano"));
        } else {
            // O teclado volta a omnibox (Home) ou a janela: `release_panel`
            // (gate `closing_a_panel_gives_the_keyboard_back`).
            close_service_panel_in(&mut self.service_panel, self.surface, self.omnibox);
            debug_log(format_args!("service panel: fechado"));
        }
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
        let kind = if self.live_panel.is_docked() {
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
        let host = if self.live_panel.is_docked() {
            self.live_panel.view()?.hwnd().0 as HWND
        } else {
            self.side_panel.view()?.hwnd().0 as HWND
        };
        Some((area, host))
    }

    /// Depois de qualquer mudanca nos paineis da direita: a pega e a roda.
    pub(in crate::windows_app) fn after_panel_change(&mut self) {
        self.sync_document_chrome();
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

    /// A pega e filha da janela: segue visibilidade/posicao do dono e nao
    /// desaparece quando o teclado passa para uma WebView filha.
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
            && unsafe { GetParent(handle) } != owner
        {
            unsafe {
                DestroyWindow(handle);
            }
            self.panel_handle = None;
        }
        let handle = match self.panel_handle {
            Some(handle) => handle,
            None => {
                let proxy_ptr = (&*self.omnibox_proxy
                    as *const EventLoopProxy<crate::windows_app::UserEvent>)
                    as usize;
                let created = create_panel_handle(owner, proxy_ptr);
                if created.is_null() {
                    return;
                }
                self.panel_handle = Some(created);
                created
            }
        };
        position_panel_handle(handle, area, scale);
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
    /// minimiza para segundo plano continuando a ver a tela e a falar; de novo,
    /// restaura para o lado. O botao Desligar dentro do painel desliga de vez.
    pub(in crate::windows_app) fn toggle_live_panel(&mut self) {
        if !self.live_panel.is_open() {
            self.open_live_panel();
        } else if self.live_panel.is_minimized() {
            self.restore_live_panel();
        } else {
            self.minimize_live_panel();
        }
    }

    pub(in crate::windows_app) fn minimize_live_panel(&mut self) {
        if !self.live_panel.is_open() || self.live_panel.is_minimized() {
            return;
        }
        self.live_panel.minimize();
        if let Some(panel) = self.live_panel.view() {
            let _ = panel.set_visible(false);
            let _ = panel.focus_parent();
        }
        debug_log(format_args!("live panel: minimizado para segundo plano"));
        self.fit_comparator_to_panel();
        self.after_panel_change();
        self.request_redraw();
    }

    pub(in crate::windows_app) fn restore_live_panel(&mut self) {
        if !self.live_panel.is_open() || !self.live_panel.is_minimized() {
            return;
        }
        self.close_docked_service_panel();
        self.close_side_panel(PanelExit::OtherPanel);
        self.live_panel.restore();
        self.position_live_panel();
        if let Some(panel) = self.live_panel.view() {
            let _ = panel.set_visible(true);
            let _ = panel.focus();
        }
        debug_log(format_args!("live panel: restaurado"));
        self.fit_comparator_to_panel();
        self.after_panel_change();
        self.request_redraw();
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
        // A trava de navegacao (NavGate::Live: so a pagina do painel) vem de
        // `hooked_builder`, como em todas as WebViews.
        let builder = themed_webview_builder()
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
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_permission_handler(live_panel_permission);
        let hooked = self.hooked_builder(builder, WebViewHost::Live, None);
        let built = hooked.build_hooked_as_child(window);
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
        if self.live_panel.is_minimized() {
            return;
        }
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
            LiveAction::Minimize => self.minimize_live_panel(),
            LiveAction::Tool(call) => self.execute_live_tool_call(call),
            LiveAction::Nothing => {}
        }
        self.request_redraw();
    }

    /// SPEC-0117: Dispatcher das 40 ferramentas do Gemini Live para os 12
    /// subsistemas nativos do NeuralIA. Devolve o resultado estruturado ao
    /// WebSocket do Gemini Live via `live_tool_response_script`.
    pub(in crate::windows_app) fn execute_live_tool_call(&mut self, call: LiveToolCall) {
        let str_arg = |key: &str| -> String {
            call.params
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let bool_arg = |key: &str, default: bool| -> bool {
            call.params
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        let usize_arg = |key: &str, default: usize| -> usize {
            call.params
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .map_or(default, |v| v as usize)
        };

        let result: serde_json::Value = match call.name.as_str() {
            // 1. Historico Inteligente & Memoria Semantica
            "history_list_recent" => {
                let limit = usize_arg("limit", 20).clamp(1, 100);
                if let Some(res) = self.privacy.recent_history(limit) {
                    self.show_history_entries(res);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "history_list_recent",
                    "limit": limit,
                    "summary": format!("Listando até {limit} itens recentes do histórico"),
                })
            }
            "history_search" => {
                let query = str_arg("query");
                let limit = usize_arg("limit", 20).clamp(1, 100);
                if !self.side_panel.is_open() {
                    self.open_side_panel();
                }
                if let Some(res) = self.privacy.recent_history(limit) {
                    self.panel_show_history(res);
                }
                if !query.is_empty() {
                    self.privacy.query_memory(query.clone());
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "history_search",
                    "query": query,
                    "limit": limit,
                })
            }
            "history_reopen" => {
                let target = str_arg("target");
                let reopened = !target.is_empty();
                if reopened {
                    self.handle_input(target.clone());
                }
                serde_json::json!({
                    "ok": reopened,
                    "tool": "history_reopen",
                    "target": target,
                })
            }
            "history_clear" => {
                self.clear_history();
                serde_json::json!({
                    "ok": true,
                    "tool": "history_clear",
                    "status": "confirmation_dialog_shown",
                })
            }
            "memory_semantic_query" => {
                let query = str_arg("query");
                let current_question = self
                    .current_research
                    .as_ref()
                    .map(|s| s.question.clone())
                    .unwrap_or_default();
                let current_items = self.current_research.as_ref().map_or(0, |s| s.items.len());
                if !query.is_empty() {
                    self.privacy.query_memory(query.clone());
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "memory_semantic_query",
                    "query": query,
                    "current_research_question": current_question,
                    "current_research_items": current_items,
                })
            }
            "memory_timeline_browse" => {
                let action = str_arg("action");
                if action == "rebuild" {
                    let scheduled = self.privacy.rebuild_memory();
                    serde_json::json!({
                        "ok": true,
                        "tool": "memory_timeline_browse",
                        "action": "rebuild",
                        "scheduled": scheduled,
                    })
                } else {
                    if !self.side_panel.is_open() {
                        self.open_side_panel();
                    }
                    let summary = match &self.current_research {
                        Some(session) => serde_json::json!({
                            "active": true,
                            "id": session.id,
                            "question": session.question,
                            "item_count": session.items.len(),
                        }),
                        None => serde_json::json!({ "active": false }),
                    };
                    serde_json::json!({
                        "ok": true,
                        "tool": "memory_timeline_browse",
                        "session": summary,
                    })
                }
            }

            // 2. Navegacao, Pesquisa IA, Layout & Automacao DOM
            "browser_search_ai" => {
                let query = str_arg("query");
                let started = !query.is_empty();
                if started {
                    self.compare(crate::windows_app::search_card::CompareRequest::ask(
                        query.clone(),
                    ));
                }
                serde_json::json!({
                    "ok": started,
                    "tool": "browser_search_ai",
                    "query": query,
                })
            }
            "browser_navigate" => {
                let url = str_arg("url");
                let mode = str_arg("mode");
                let col = usize_arg("column", 0).min(2);
                if mode == "home" || url == "home" {
                    let went_home = self.request_home();
                    serde_json::json!({
                        "ok": true,
                        "tool": "browser_navigate",
                        "mode": "home",
                        "went_home": went_home,
                    })
                } else if mode == "reader" && !url.is_empty() {
                    self.read(url.clone());
                    serde_json::json!({
                        "ok": true,
                        "tool": "browser_navigate",
                        "url": url,
                        "mode": "reader",
                    })
                } else {
                    let opened = !url.is_empty();
                    if opened {
                        match mode.as_str() {
                            "split" => {
                                let _ = self.open_split(col, url.clone(), false);
                            }
                            "private_split" => {
                                let _ = self.open_split_mode(col, url.clone(), false, true, None);
                            }
                            "column" => self.open_in_column(col, url.clone()),
                            "everywhere" => self.open_everywhere(url.clone()),
                            _ => self.web(url.clone()),
                        }
                    }
                    serde_json::json!({
                        "ok": opened,
                        "tool": "browser_navigate",
                        "url": url,
                        "mode": if mode.is_empty() { "web".to_string() } else { mode },
                    })
                }
            }
            "browser_layout" => {
                let action = str_arg("action");
                let col = usize_arg("column", 0).min(2);
                match action.as_str() {
                    "expand" => self.expand_comparator(col),
                    "minimize" => self.minimize_comparator(col),
                    "restore" => self.restore_comparator(),
                    "close_split" => self.close_split(),
                    "toggle_split_fullscreen" => self.toggle_split_fullscreen(),
                    "home" => {
                        let _ = self.request_home();
                    }
                    _ => self.restore_comparator(),
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "browser_layout",
                    "action": action,
                    "column": col,
                })
            }
            "browser_tabs" => {
                let action = str_arg("action");
                let col = usize_arg("column", 0).min(2);
                match action.as_str() {
                    "new" => self.new_tab(col),
                    "back" => self.navigate_column(
                        col,
                        crate::windows_app::app::navigation::HistoryStep::Back,
                    ),
                    "forward" => self.navigate_column(
                        col,
                        crate::windows_app::app::navigation::HistoryStep::Forward,
                    ),
                    "close_split" => self.close_split(),
                    _ => self.new_tab(col),
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "browser_tabs",
                    "action": action,
                    "column": col,
                })
            }
            "browser_dom_action" => {
                let spec = str_arg("spec");
                let goal = str_arg("goal");
                let action = str_arg("action");
                let target = str_arg("target");
                let cmd = if !spec.is_empty() {
                    spec
                } else if !goal.is_empty() {
                    goal
                } else if !action.is_empty() && !target.is_empty() {
                    format!("{action}:{target}")
                } else {
                    action
                };
                if !cmd.is_empty() && cmd != "inspect" {
                    self.start_browser_agent(&cmd);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "browser_dom_action",
                    "surface": format!("{:?}", self.surface),
                    "spec": cmd,
                })
            }

            // 3. Consenso & Sintese de Pesquisa
            "consensus_compare_columns" => {
                self.compare_current_research();
                serde_json::json!({ "ok": true, "tool": "consensus_compare_columns" })
            }
            "research_session_manage" => {
                let action = str_arg("action");
                match action.as_str() {
                    "export" => self.export_current_research(),
                    "compare" => self.compare_current_research(),
                    _ => self.synthesize_current_research(),
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "research_session_manage",
                    "action": if action.is_empty() { "synthesize".to_string() } else { action },
                })
            }

            // 4. Gravador de Tela, Clipes & Snapshot OCR
            "recorder_screen" => {
                let action = str_arg("action");
                let summary = str_arg("summary");
                if action == "stop" && !summary.is_empty() {
                    let draft = NoteDraft {
                        title: "Gravação de Tela — Gemini Live".to_string(),
                        body: summary,
                        tags: vec!["recording".to_string(), "live".to_string()],
                        source: None,
                    };
                    self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "recorder_screen",
                    "action": if action.is_empty() { "start".to_string() } else { action },
                })
            }
            "recorder_clip_last" => {
                let seconds = usize_arg("seconds", 30).clamp(5, 300);
                let title = str_arg("title");
                let clip_title = if title.is_empty() {
                    format!("Clipe dos últimos {seconds}s — Gemini Live")
                } else {
                    title
                };
                let draft = NoteDraft {
                    title: clip_title.clone(),
                    body: format!(
                        "Clipe retroativo dos últimos {seconds} segundos registado pelo Gemini Live."
                    ),
                    tags: vec!["clip".to_string(), "live".to_string()],
                    source: None,
                };
                self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                serde_json::json!({
                    "ok": true,
                    "tool": "recorder_clip_last",
                    "seconds": seconds,
                    "title": clip_title,
                })
            }
            "recorder_snapshot_ocr" => {
                let prompt = str_arg("prompt");
                let extracted = str_arg("extracted_text");
                let save_to_note = bool_arg("save_to_note", true);
                if save_to_note && (!extracted.is_empty() || !prompt.is_empty()) {
                    let draft = NoteDraft {
                        title: if prompt.is_empty() {
                            "Snapshot OCR — Gemini Live".to_string()
                        } else {
                            format!("OCR: {}", prompt.chars().take(60).collect::<String>())
                        },
                        body: if extracted.is_empty() {
                            prompt.clone()
                        } else {
                            extracted
                        },
                        tags: vec!["ocr".to_string(), "snapshot".to_string()],
                        source: None,
                    };
                    self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "recorder_snapshot_ocr",
                    "prompt": prompt,
                    "saved_to_note": save_to_note,
                })
            }

            // 5. Traducao em Tempo Real & Legendagem ao Vivo
            "translate_surface" => {
                let target = str_arg("target");
                let host = match target.as_str() {
                    "col0" | "google" => WebViewHost::Column(0),
                    "col1" | "chatgpt" => WebViewHost::Column(1),
                    "col2" | "claude" => WebViewHost::Column(2),
                    "split" => WebViewHost::Split(0),
                    _ => WebViewHost::External,
                };
                self.translation_event(crate::windows_app::translation::TranslateEvent::Requested(
                    host,
                ));
                serde_json::json!({
                    "ok": true,
                    "tool": "translate_surface",
                    "target": target,
                })
            }
            "translate_live_audio" => {
                let active = bool_arg("enabled", bool_arg("active", true));
                let source_lang = str_arg("source_lang");
                let target_lang = str_arg("target_lang");
                let label = if active {
                    format!(
                        "Intérprete de voz ao vivo ativo ({} → {})",
                        if source_lang.is_empty() {
                            "auto"
                        } else {
                            &source_lang
                        },
                        if target_lang.is_empty() {
                            "pt-BR"
                        } else {
                            &target_lang
                        }
                    )
                } else {
                    "Intérprete de voz ao vivo desativado".to_string()
                };
                self.show_splash(label, 3);
                serde_json::json!({
                    "ok": true,
                    "tool": "translate_live_audio",
                    "active": active,
                    "source_lang": source_lang,
                    "target_lang": target_lang,
                })
            }
            "translate_selection" => {
                let text = str_arg("text");
                let target_lang = str_arg("target_lang");
                let started = !text.is_empty();
                if started {
                    self.compare(crate::windows_app::search_card::CompareRequest::translate(
                        &text,
                    ));
                }
                serde_json::json!({
                    "ok": started,
                    "tool": "translate_selection",
                    "target_lang": target_lang,
                })
            }
            "captions_live_overlay" => {
                let active = bool_arg("enabled", bool_arg("active", true));
                let lang = str_arg("lang");
                let translate_to = str_arg("translate_to");
                serde_json::json!({
                    "ok": true,
                    "tool": "captions_live_overlay",
                    "active": active,
                    "lang": lang,
                    "translate_to": translate_to,
                })
            }
            "captions_export" => {
                let format = str_arg("format");
                let content = str_arg("content");
                let save_to_zettel = bool_arg("save_to_zettel", true);
                if save_to_zettel {
                    let draft = NoteDraft {
                        title: format!(
                            "Legendas ao Vivo ({})",
                            if format.is_empty() { "md" } else { &format }
                        ),
                        body: if content.is_empty() {
                            "Transcrição/legendas exportadas pelo Gemini Live.".to_string()
                        } else {
                            content
                        },
                        tags: vec!["captions".to_string(), "live".to_string()],
                        source: None,
                    };
                    self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "captions_export",
                    "format": if format.is_empty() { "md".to_string() } else { format },
                    "saved_to_zettel": save_to_zettel,
                })
            }

            // 6. Copiloto de Reunioes (Google Meet & Microsoft Teams)
            "meeting_open_or_join" => {
                let service = str_arg("service");
                let target = if service.eq_ignore_ascii_case("teams") {
                    Service::Teams
                } else {
                    Service::Meet
                };
                self.open_service_panel(target);
                serde_json::json!({
                    "ok": true,
                    "tool": "meeting_open_or_join",
                    "service": target.label(),
                })
            }
            "meeting_copilot_mode" => {
                let active = bool_arg("enabled", bool_arg("active", true));
                let goal = str_arg("goal");
                self.show_splash(
                    if active {
                        "Copiloto de Reunião ativado no Gemini Live".to_string()
                    } else {
                        "Copiloto de Reunião desativado".to_string()
                    },
                    3,
                );
                serde_json::json!({
                    "ok": true,
                    "tool": "meeting_copilot_mode",
                    "active": active,
                    "goal": goal,
                })
            }
            "meeting_summarize_so_far" => {
                let title = str_arg("title");
                let summary = str_arg("summary");
                let note_title = if title.is_empty() {
                    "Ata de Reunião — Gemini Live".to_string()
                } else {
                    title
                };
                let draft = NoteDraft {
                    title: note_title.clone(),
                    body: if summary.is_empty() {
                        "Resumo executivo da reunião gerado pelo Copiloto Gemini Live.".to_string()
                    } else {
                        summary
                    },
                    tags: vec!["meeting".to_string(), "ata".to_string()],
                    source: None,
                };
                self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                serde_json::json!({
                    "ok": true,
                    "tool": "meeting_summarize_so_far",
                    "title": note_title,
                    "saved_to_zettel": true,
                })
            }
            "meeting_send_chat" => {
                let message = str_arg("message");
                let items = str_arg("items");
                let body = if !message.is_empty() { message } else { items };
                if !body.is_empty() {
                    let draft = NoteDraft {
                        title: "Notas / Chat de Reunião — Gemini Live".to_string(),
                        body,
                        tags: vec!["meeting".to_string(), "chat".to_string()],
                        source: None,
                    };
                    self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "meeting_send_chat",
                })
            }

            // 7. Leitor Imersivo, PDF & Biblioteca EPUB
            "reader_read_aloud" => {
                let action = str_arg("action");
                match action.as_str() {
                    "toggle_autoscroll" | "autoscroll" => self.toggle_auto_scroll(),
                    "stop" | "pause" => {
                        self.for_each_visible_webview(|webview| {
                            let _ = webview.evaluate_script(
                                "if (window.speechSynthesis) window.speechSynthesis.cancel();",
                            );
                        });
                    }
                    _ => {
                        self.toggle_auto_scroll();
                    }
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "reader_read_aloud",
                    "action": if action.is_empty() { "start".to_string() } else { action },
                })
            }
            "reader_navigate_doc" => {
                let url = str_arg("url");
                let action = str_arg("action");
                if !url.is_empty() {
                    self.read(url.clone());
                } else if action == "autoscroll" {
                    self.toggle_auto_scroll();
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "reader_navigate_doc",
                    "url": url,
                    "action": action,
                })
            }
            "reader_highlight_and_note" => {
                let quote = str_arg("quote");
                let comment = str_arg("comment");
                let url = str_arg("url");
                let body = if comment.is_empty() {
                    format!("> {quote}\n")
                } else {
                    format!("> {quote}\n\n{comment}\n")
                };
                let draft = NoteDraft {
                    title: if quote.is_empty() {
                        "Destaque de Leitura — Gemini Live".to_string()
                    } else {
                        quote.chars().take(60).collect()
                    },
                    body,
                    tags: vec!["leitura".to_string(), "destaque".to_string()],
                    source: (!url.is_empty()).then_some(url),
                };
                self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                serde_json::json!({
                    "ok": true,
                    "tool": "reader_highlight_and_note",
                })
            }

            // 8. Zettelkasten & Grafo Obsidian
            "notes_manage" => {
                let action = str_arg("action");
                let id = str_arg("id");
                let title = str_arg("title");
                let body = str_arg("body");
                let query = str_arg("query");
                match action.as_str() {
                    "create" => {
                        let draft = NoteDraft {
                            title: if title.is_empty() {
                                "Nota do Gemini Live".to_string()
                            } else {
                                title
                            },
                            body,
                            tags: vec!["live".to_string()],
                            source: None,
                        };
                        self.submit_notes(NotesCommand::Create(draft), NotesOrigin::Closed);
                    }
                    "search" => {
                        self.show_notes_panel(Vec::new());
                        if !query.is_empty() {
                            self.submit_notes(NotesCommand::Search(query), NotesOrigin::Panel);
                        }
                    }
                    "open" if !id.is_empty() => {
                        self.show_notes_panel(Vec::new());
                        self.submit_notes(NotesCommand::Open(id), NotesOrigin::Panel);
                    }
                    "delete" if !id.is_empty() => {
                        self.submit_notes(NotesCommand::Delete(id), NotesOrigin::Panel);
                    }
                    _ => {
                        self.show_notes_panel(Vec::new());
                    }
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "notes_manage",
                    "action": if action.is_empty() { "panel".to_string() } else { action },
                })
            }
            "notes_obsidian_graph" => {
                self.show_obsidian_panel();
                serde_json::json!({
                    "ok": true,
                    "tool": "notes_obsidian_graph",
                })
            }

            // 9. Downloads, Biblioteca EPUB & Favoritos
            "downloads_manage" => {
                self.show_downloads_panel();
                serde_json::json!({
                    "ok": true,
                    "tool": "downloads_manage",
                })
            }
            "library_documents" => {
                let path = str_arg("path");
                if path.is_empty() {
                    self.open_library();
                } else {
                    self.open_epub(std::path::PathBuf::from(&path));
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "library_documents",
                    "path": path,
                })
            }
            "bookmarks_manage" => {
                self.show_bookmarks_panel();
                serde_json::json!({
                    "ok": true,
                    "tool": "bookmarks_manage",
                })
            }

            // 10. Painel de Servicos & Hub de Agentes Externos (MCP)
            "services_panel" => {
                let service_name = str_arg("service");
                let action = str_arg("action");
                let svc = match service_name.to_ascii_lowercase().as_str() {
                    "whatsapp" => Some(Service::WhatsApp),
                    "youtube" => Some(Service::YouTube),
                    "gmail" => Some(Service::Gmail),
                    "outlook" => Some(Service::Outlook),
                    "teams" => Some(Service::Teams),
                    "meet" => Some(Service::Meet),
                    "breath" | "respiracao" => Some(Service::Breath),
                    _ => None,
                };
                match (svc, action.as_str()) {
                    (_, "close") => self.close_service_panel(),
                    (_, "minimize") => self.service_input(ServiceInput::Minimize),
                    (_, "fullscreen") => self.service_input(ServiceInput::ToggleFullscreen),
                    (Some(target), _) => self.open_service_panel(target),
                    (None, _) => {}
                }
                serde_json::json!({
                    "ok": svc.is_some() || matches!(action.as_str(), "close" | "minimize" | "fullscreen"),
                    "tool": "services_panel",
                    "service": service_name,
                    "action": if action.is_empty() { "open".to_string() } else { action },
                })
            }
            "services_gmail_status" => {
                let action = str_arg("action");
                if action == "open" {
                    self.open_service_panel(Service::Gmail);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "services_gmail_status",
                    "unread": self.gmail_last_unread.unwrap_or(0),
                })
            }
            "services_media_control" => {
                let service_name = str_arg("service");
                if service_name.eq_ignore_ascii_case("breath")
                    || service_name.eq_ignore_ascii_case("respiracao")
                {
                    self.open_service_panel(Service::Breath);
                } else {
                    self.open_service_panel(Service::YouTube);
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "services_media_control",
                    "service": service_name,
                })
            }
            "agents_hub_manage" => {
                let agent = str_arg("agent");
                let message = str_arg("message");
                let sent = if let Some(hub) = self.agents.hub()
                    && !agent.is_empty()
                    && !message.is_empty()
                {
                    hub.user_message(&agent, &message).is_ok()
                } else {
                    false
                };
                serde_json::json!({
                    "ok": true,
                    "tool": "agents_hub_manage",
                    "agent": agent,
                    "sent": sent,
                })
            }

            // 11. Foco, Produtividade & Anti-Distracao
            "focus_pomodoro" => {
                let action = str_arg("action");
                let cmd = crate::pomodoro_ui::parse_pomodoro_command(&action)
                    .unwrap_or(crate::pomodoro_ui::PomodoroCommand::Click);
                self.pomodoro_command(cmd);
                serde_json::json!({
                    "ok": true,
                    "tool": "focus_pomodoro",
                    "action": if action.is_empty() { "click".to_string() } else { action },
                })
            }
            "focus_anti_distraction" => {
                let mode = str_arg("mode");
                let cmd = match mode.as_str() {
                    "on" => crate::windows_app::DistractionCommand::On,
                    "off" => crate::windows_app::DistractionCommand::Off,
                    _ => crate::windows_app::DistractionCommand::Status,
                };
                self.distraction_command(cmd);
                serde_json::json!({
                    "ok": true,
                    "tool": "focus_anti_distraction",
                    "mode": if mode.is_empty() { "status".to_string() } else { mode },
                })
            }

            // 12. Sistema, Tema, Zoom & Controle da Sessao Live
            "system_control" => {
                let action = str_arg("action");
                match action.as_str() {
                    "theme_dark" => self.choose_theme(crate::windows_app::theme::ThemeChoice::Dark),
                    "theme_light" => {
                        self.choose_theme(crate::windows_app::theme::ThemeChoice::Light)
                    }
                    "theme_system" => {
                        self.choose_theme(crate::windows_app::theme::ThemeChoice::System)
                    }
                    "zoom_in" => self.step_zoom(1),
                    "zoom_out" => self.step_zoom(-1),
                    "zoom_reset" => self.set_zoom(1.0),
                    "downloads" => self.show_downloads_panel(),
                    "bookmarks" => self.show_bookmarks_panel(),
                    "minimize_live" => self.minimize_live_panel(),
                    "restore_live" => self.restore_live_panel(),
                    "close_live" => {
                        self.close_live_panel();
                        return;
                    }
                    "about" => self.show_about(),
                    "check_update" => self.check_and_apply_update(true),
                    _ => {}
                }
                serde_json::json!({
                    "ok": true,
                    "tool": "system_control",
                    "action": action,
                })
            }

            other => serde_json::json!({
                "ok": false,
                "error": format!("unknown tool: {other}"),
            }),
        };

        let script = live_tool_response_script(&call.id, &call.name, &result);
        self.live_eval(&script);
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
            self.live_panel.is_docked(),
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
        self.minimize_live_panel();
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
        // Criado por ultimo, fica por cima das outras WebViews. A trava de
        // navegacao (NavGate::SidePanel: so o proprio HTML local) vem de
        // `hooked_builder`, como em todas as WebViews.
        let builder = themed_webview_builder()
            .with_html(panel_html(&Theme::system()))
            .with_bounds(bounds)
            .with_ipc_handler(move |request| {
                if let Some(post) = side_panel::PanelPost::parse(ticket, request.body()) {
                    let _ = proxy.send_event(crate::windows_app::UserEvent::Panel(post));
                }
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny);
        let hooked = self.hooked_builder(builder, WebViewHost::SidePanel, None);
        let built = hooked.build_hooked_as_child(window);
        match built {
            Ok(panel) => {
                let _ = panel.focus();
                #[cfg(feature = "accel-spike")]
                self.accel_spike_hook(&panel, crate::accel_spike::SpikeHost::SidePanel);
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
            let bounds = if self.downloads_ui.document_ticket.is_some()
                && self.side_panel.active_ticket() == self.downloads_ui.document_ticket
            {
                downloads_ui::documents::document_content_bounds(
                    bounds,
                    self.window
                        .as_ref()
                        .map_or(1.0, |window| window.scale_factor()),
                )
            } else {
                bounds
            };
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

    /// Abre o painel lateral na aba Obsidian e carrega o grafo.
    #[allow(dead_code)]
    pub(in crate::windows_app) fn show_obsidian_panel(&mut self) {
        if !self.side_panel.is_open() {
            self.open_side_panel();
        }
        if !self.side_panel.is_open() {
            return;
        }
        self.panel_run(PANEL_SHOW_OBSIDIAN_SCRIPT.to_string());
    }

    /// Abre o painel lateral na aba Sobre.
    pub(in crate::windows_app) fn show_about_panel(&mut self) {
        if !self.side_panel.is_open() {
            self.open_side_panel();
        }
        if !self.side_panel.is_open() {
            return;
        }
        self.panel_run(PANEL_SHOW_ABOUT_SCRIPT.to_string());
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
                if let Some(result) = self.privacy.recent_history(PANEL_RECENT_LIMIT) {
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
                    self.privacy.query_memory(question);
                }
                // Aberto pelas Notas (botao, Ctrl+Shift+Z): agora a pagina
                // ja existe e o que esperava corre, pela ordem.
                for script in self.side_panel.mark_ready() {
                    self.panel_eval(&script);
                }
            }
            PanelMessage::Search(query) => self.privacy.query_memory(query),
            PanelMessage::Open(input) => {
                self.close_side_panel(PanelExit::OpenItem);
                self.handle_input(input);
            }
            PanelMessage::Close => self.close_side_panel(PanelExit::CloseButton),
            PanelMessage::Downloads(request) => self.downloads_panel_request(request),
            // Ja seguido por `SidePanel::receive`.
            PanelMessage::NoteDraft(_) => {}
            PanelMessage::NoteSaveRefused => self.panel_run(notes_reply_script(
                &NotesReply::Failed(NOTE_SAVE_REFUSED.to_string()),
            )),
            PanelMessage::Bookmarks(request) => self.bookmark_panel_request(request),
            PanelMessage::ObsidianGraph => {
                // A leitura do histórico fica no worker das notas. Aqui só
                // se entrega a loja: o event loop não espera o trinco do ficheiro.
                let history = self.privacy.history_store();
                if let Err(error) = self.notes.submit_graph(history, NotesOrigin::Panel) {
                    self.panel_run(notes_reply_script(&NotesReply::Failed(error)));
                }
            }
            PanelMessage::CheckUpdate => {
                self.check_and_apply_update(true);
            }
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
    pub(in crate::windows_app) fn save_notes_draft_before_exit(&mut self) {
        let _ = self.side_panel.exit(NOTES_EXIT_WAIT);
    }

    /// Resposta do worker das notas. A de uma selecao abre o painel na nota
    /// criada; a do painel so vai para o painel que ainda estiver aberto.
    pub(in crate::windows_app) fn notes_ready(&mut self, origin: NotesOrigin, reply: NotesReply) {
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
                | NotesReply::Conflict { .. }
                | NotesReply::Graph(_) => {}
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
                | NotesReply::Conflict { .. }
                | NotesReply::Graph(_) => {}
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
                | NotesReply::Missing { .. }
                | NotesReply::Graph(_) => {}
            },
        }
    }

    /// Ctrl+Shift+Z ou "Salvar nota" numa pagina: a nota da WebView
    /// `target`. O Ctrl+Shift+Z le a selecao dela e nunca le o Split privado;
    /// o Salvar nota traz o texto no pedido e so tira dela a fonte.
    pub(in crate::windows_app) fn request_note_from_page(
        &mut self,
        target: Option<PageTarget>,
        via: NoteVia,
    ) {
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
    pub(in crate::windows_app) fn note_captured(&mut self, raw: &str, source: Option<&str>) {
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
}

#[cfg(test)]
mod youtube_transition_tests {
    use super::*;

    #[test]
    fn account_services_keep_their_webviews_when_hidden() {
        for service in [
            Service::Teams,
            Service::Outlook,
            Service::WhatsApp,
            Service::YouTube,
            Service::Gmail,
        ] {
            assert!(service.keeps_running_in_background());
            assert_eq!(
                service_panel_hint(BarHit::ServiceStrip(StripButton::Close), service, None),
                Some(format!(
                    "Ocultar {}: continua em segundo plano",
                    service.label()
                ))
            );
        }
        for service in [Service::Meet, Service::Breath] {
            assert!(!service.keeps_running_in_background());
        }
    }

    #[test]
    fn internal_transitions_keep_account_services_alive() {
        for service in [Service::YouTube, Service::WhatsApp] {
            assert_eq!(
                service_transition_input(service, ServicePanelState::default()),
                Some(ServiceInput::Minimize)
            );
        }
    }

    #[test]
    fn an_already_minimized_background_service_needs_no_second_transition() {
        let mut state = ServicePanelState::default();
        assert_eq!(state.step(ServiceInput::Minimize), ServiceEffect::Relayout);
        for service in [
            Service::YouTube,
            Service::WhatsApp,
            Service::Teams,
            Service::Outlook,
            Service::Gmail,
        ] {
            assert_eq!(service_transition_input(service, state), None);
        }
    }

    #[test]
    fn youtube_surface_transitions_use_the_preserving_path_not_direct_close() {
        let chrome = include_str!("chrome.rs");
        let compare = include_str!("compare.rs");
        let pages = include_str!("pages.rs");

        let destroy = chrome
            .split("pub(in crate::windows_app) fn destroy_web_surfaces")
            .nth(1)
            .and_then(|tail| tail.split("pub(in crate::windows_app) fn").next())
            .expect("destroy_web_surfaces body");
        assert!(destroy.contains("self.service_panel_for_transition();"));
        assert!(!destroy.contains("self.close_service_panel();"));

        assert!(chrome.contains("self.service_panel_for_transition();"));
        assert!(compare.contains("self.service_panel_for_transition();"));
        assert!(pages.contains("self.service_panel_for_transition();"));
    }

    #[test]
    fn other_services_keep_the_old_close_policy() {
        for service in [Service::Meet, Service::Breath] {
            assert_eq!(
                service_transition_input(service, ServicePanelState::default()),
                Some(ServiceInput::Close)
            );
        }
    }
}

/// Same creation and placement used by the UI and the native regression gate.
fn create_panel_handle(owner: HWND, proxy_ptr: usize) -> HWND {
    unsafe {
        let created = CreateWindowExW(
            0,
            windows_sys::w!("STATIC"),
            windows_sys::w!("NeuralIA.PanelResize"),
            WS_CHILD,
            0,
            0,
            1,
            1,
            owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        if !created.is_null()
            && SetWindowSubclass(
                created,
                Some(panel_handle_subclass),
                PANEL_HANDLE_SUBCLASS_ID,
                proxy_ptr,
            ) == 0
        {
            DestroyWindow(created);
            return std::ptr::null_mut();
        }
        created
    }
}

fn position_panel_handle(handle: HWND, area: Area, scale: f64) {
    unsafe {
        SetWindowPos(
            handle,
            HWND_TOP,
            (area.x * scale).round() as i32,
            (area.y * scale).round() as i32,
            (area.width * scale).round().max(3.0) as i32,
            (area.height * scale).round().max(1.0) as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        InvalidateRect(handle, std::ptr::null(), 0);
    }
}

#[cfg(test)]
mod panel_handle_gates;
