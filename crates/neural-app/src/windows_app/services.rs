use super::*;

// Servicos no painel lateral (caminho A do WebRTC, aprovado pelo dono): o
// servico corre como uma pagina da internet comum -- contatos e chamadas sao
// os dele; o NeuralIA so libera camera e microfone pelo aviso do WebView2.
// Sem scripts injetados e sem o canal IPC do painel do historico.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum Service {
    /// Videochamada: o Google Meet.
    Meet,
    WhatsApp,
    YouTube,
    Gmail,
    /// Respiracao guiada (metodo Wim Hof), pedida pelo dono "em modo
    /// anonimo": o video no painel InPrivate, sem camera nem microfone.
    Breath,
}

/// O video de respiracao que o dono escolheu.
pub(in crate::windows_app) const BREATH_VIDEO_URL: &str = "https://www.youtube.com/watch?v=UJBknAsxfrA";

impl Service {
    pub(in crate::windows_app) fn url(self) -> &'static str {
        match self {
            Self::Meet => "https://meet.google.com/",
            Self::WhatsApp => "https://web.whatsapp.com/",
            Self::YouTube => "https://www.youtube.com/",
            Self::Gmail => "https://mail.google.com/mail/u/0/#inbox",
            Self::Breath => BREATH_VIDEO_URL,
        }
    }

    pub(in crate::windows_app) fn label(self) -> &'static str {
        match self {
            Self::Meet => "Videochamada (Google Meet)",
            Self::WhatsApp => "WhatsApp",
            Self::YouTube => "YouTube",
            Self::Gmail => "Gmail",
            Self::Breath => "Respiração guiada (método Wim Hof)",
        }
    }

    /// Painel anonimo: WebView2 InPrivate (nada fica no perfil -- cookies,
    /// cache, historico do WebView), camera e microfone recusados, e a pagina
    /// presa ao que a abriu. Os outros servicos precisam da conta do
    /// utilizador e por isso nao podem ser privados.
    pub(in crate::windows_app) fn private(self) -> bool {
        match self {
            Self::Breath => true,
            Self::Meet | Self::WhatsApp | Self::YouTube | Self::Gmail => false,
        }
    }
}

/// Para onde vai o teclado quando um painel ao lado fecha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum PanelCloseFocus {
    /// Na Home: a omnibox, para continuar a escrever.
    Omnibox,
    /// No resto: a janela (os atalhos da barra continuam a funcionar).
    Window,
}

pub(in crate::windows_app) fn focus_after_panel_close(surface: Surface) -> PanelCloseFocus {
    match surface {
        Surface::Home => PanelCloseFocus::Omnibox,
        Surface::Reader
        | Surface::External
        | Surface::Comparator
        | Surface::Pdf
        | Surface::Epub => PanelCloseFocus::Window,
    }
}

/// O que fechar um painel da direita faz a vista dele antes de a largar.
/// Generico para os gates correrem sem WebView; no app e a `WebView`.
pub(in crate::windows_app) trait PanelView {
    /// A janela principal fica com o teclado (`WebView::focus_parent`).
    fn give_keyboard_to_window(&self);
}

impl PanelView for WebView {
    fn give_keyboard_to_window(&self) {
        let _ = self.focus_parent();
    }
}

/// A saida de um painel da direita: a de um servico
/// (`close_service_panel_in`) e a do Ctrl+H (`side_panel::SidePanel::dismiss`).
/// O painel tinha o teclado (`panel.focus()` ao abrir) e largar a WebView nao
/// o devolve a ninguem: fechar o video da respiracao na Home deixava a omnibox
/// sem teclado ate um clique. Fora da Home a janela fica com ele ANTES de a
/// vista sair; na Home o EDIT da omnibox (`omnibox`), DEPOIS -- direto, sem
/// `focus_omnibox`, que passa pelo `show_home`. `focus: None` (uma troca de
/// superficie, que trata do teclado ela propria, ou a saida da app): so larga.
pub(in crate::windows_app) fn release_panel<V: PanelView>(
    view: V,
    focus: Option<PanelCloseFocus>,
    omnibox: Option<HWND>,
) {
    if focus == Some(PanelCloseFocus::Window) {
        view.give_keyboard_to_window();
    }
    drop(view);
    if focus == Some(PanelCloseFocus::Omnibox)
        && let Some(edit) = omnibox
    {
        unsafe {
            SetFocus(edit);
        }
    }
}

/// Fecha o painel de servico que houver, com o teclado devolvido por
/// `release_panel`. `false`: nao havia nenhum. No app a vista e o
/// `ServicePanel` inteiro (a WebView dele); nos gates, uma de mentira.
pub(in crate::windows_app) fn close_service_panel_in<V: PanelView>(
    slot: &mut Option<V>,
    surface: Surface,
    omnibox: Option<HWND>,
) -> bool {
    let Some(view) = slot.take() else {
        return false;
    };
    release_panel(view, Some(focus_after_panel_close(surface)), omnibox);
    true
}

/// O servico aberto no painel da direita e o modo em que esta.
pub(in crate::windows_app) struct ServicePanel {
    pub(in crate::windows_app) service: Service,
    pub(in crate::windows_app) webview: WebView,
    /// Encostado, minimizado (a tocar, escondido) ou em tela cheia.
    pub(in crate::windows_app) state: ServicePanelState,
    /// A pagina esta a tocar som (IsDocumentPlayingAudio do WebView2).
    pub(in crate::windows_app) audio: bool,
    /// Numero deste painel; os avisos do WebView2 trazem-no.
    pub(in crate::windows_app) generation: u64,
}

/// Fechar o painel de servicos devolve o teclado pela WebView dele
/// (`close_service_panel_in`).
impl PanelView for ServicePanel {
    fn give_keyboard_to_window(&self) {
        self.webview.give_keyboard_to_window();
    }
}

/// Um aviso do WebView2 do painel `event` so vale se esse painel ainda for
/// o aberto: fechar e abrir outro deixa avisos atrasados do anterior na fila.
pub(in crate::windows_app) fn service_event_is_current(open: Option<u64>, event: u64) -> bool {
    open == Some(event)
}

/// A tecla que o WebView2 do painel de servicos viu: so o Esc em baixo vira
/// evento; se e dele ou da pagina decide o modo do painel.
pub(in crate::windows_app) fn service_key_event(
    generation: u64,
    virtual_key: u32,
    key_down: bool,
) -> Option<UserEvent> {
    is_escape_down(virtual_key, key_down).then_some(UserEvent::ServiceEscape(generation))
}

/// A largura que as colunas cedem ao painel da direita aberto. O de servicos
/// minimizado nao cede nada: as colunas voltam a ocupar a janela toda.
pub(in crate::windows_app) fn reserved_panel_width(
    service: Option<ServicePanelState>,
    live_open: bool,
    side_open: bool,
    service_width: f64,
    history_width: f64,
) -> f64 {
    // O de servicos minimizado nao cede nada; se ao lado houver outro painel
    // aberto, e esse que conta.
    match service.map(|state| state.reserved_width(service_width)) {
        Some(width) if width > 0.0 => width,
        _ if live_open => service_width,
        _ if side_open => history_width,
        _ => 0.0,
    }
}

pub(in crate::windows_app) fn logical_rect(area: Area) -> wry::Rect {
    wry::Rect {
        position: LogicalPosition::new(area.x, area.y).into(),
        size: LogicalSize::new(area.width.max(1.0), area.height.max(1.0)).into(),
    }
}

/// Poe o contentor da WebView por cima de todos os irmaos (as colunas, os
/// botoes nativos), sem o ativar.
pub(in crate::windows_app) fn raise_webview_host(webview: &WebView) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_TOP, SWP_NOMOVE, SWP_NOSIZE};
    use wry::WebViewExtWindows;
    let host = webview.hwnd().0 as HWND;
    if host.is_null() {
        return;
    }
    unsafe {
        SetWindowPos(
            host,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Os avisos do WebView2 do painel de servicos: a pagina entrou/saiu de tela
/// cheia (ContainsFullScreenElementChanged), comecou/parou de tocar som
/// (IsDocumentPlayingAudioChanged) e o Esc (AcceleratorKeyPressed, que o
/// WebView2 levanta para o Esc mesmo com o foco na pagina). Cada closure so
/// le o que o WebView2 diz e manda um evento com o numero do painel.
pub(in crate::windows_app) fn register_service_panel_events(
    webview: &WebView,
    generation: u64,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    use webview2_com::{
        AcceleratorKeyPressedEventHandler, ContainsFullScreenElementChangedEventHandler,
        IsDocumentPlayingAudioChangedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
            COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN, ICoreWebView2_8,
        },
    };
    use windows_core::{BOOL, Interface};
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let fullscreen_proxy = proxy.clone();
    let fullscreen =
        ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
            if let Some(sender) = sender {
                let mut contains = BOOL::default();
                unsafe { sender.ContainsFullScreenElement(&mut contains)? };
                let _ = fullscreen_proxy.send_event(UserEvent::ServiceFullscreen {
                    generation,
                    on: contains.as_bool(),
                });
            }
            Ok(())
        }));
    let mut token = 0i64;
    unsafe { core.add_ContainsFullScreenElementChanged(&fullscreen, &mut token) }
        .map_err(|error| format!("ContainsFullScreenElementChanged: {error}"))?;

    // O som e so para o ponto no icone: um runtime sem ICoreWebView2_8 fica
    // sem ele e o resto funciona.
    if let Ok(core8) = core.cast::<ICoreWebView2_8>() {
        let audio_proxy = proxy.clone();
        let audio =
            IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(sender) =
                    sender.and_then(|sender| sender.cast::<ICoreWebView2_8>().ok())
                {
                    let mut playing = BOOL::default();
                    unsafe { sender.IsDocumentPlayingAudio(&mut playing)? };
                    let _ = audio_proxy.send_event(UserEvent::ServiceAudio {
                        generation,
                        playing: playing.as_bool(),
                    });
                }
                Ok(())
            }));
        let mut token = 0i64;
        let _ = unsafe { core8.add_IsDocumentPlayingAudioChanged(&audio, &mut token) };
    }

    let controller = webview.controller();
    let keys = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        if let Some(args) = args {
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
            let mut key = 0u32;
            unsafe {
                args.KeyEventKind(&mut kind)?;
                args.VirtualKey(&mut key)?;
            }
            let down = kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                || kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN;
            // Sem marcar "tratado": a pagina recebe o Esc como sempre.
            if let Some(event) = service_key_event(generation, key, down) {
                let _ = proxy.send_event(event);
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { controller.add_AcceleratorKeyPressed(&keys, &mut token) }
        .map_err(|error| format!("AcceleratorKeyPressed: {error}"))
}

/// So paginas da internet: um servico nunca abre file:, javascript: nem os
/// esquemas internos do NeuralIA.
pub(in crate::windows_app) fn service_panel_allows_navigation(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    lower == "about:blank" || lower.starts_with("https://") || lower.starts_with("http://")
}

/// O painel da respiracao existe para um video. Fica preso ao YouTube (e a
/// pagina de consentimento de cookies que ele mostra a uma sessao sem
/// cookies, como e sempre a InPrivate), so em https -- para nao virar um
/// navegador anonimo sem as protecoes do painel Privado. Nem o login da
/// Google: entrar numa conta no painel "anonimo" contradiz o pedido.
pub(in crate::windows_app) fn breath_panel_allows_navigation(target: &str) -> bool {
    let target = target.trim();
    if target.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    let Ok(url) = Url::parse(target) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
        || host == "consent.google.com"
}

/// A politica de navegacao de cada servico, num so sitio.
pub(in crate::windows_app) fn service_panel_navigation(service: Service, target: &str) -> bool {
    match service {
        Service::Breath => breath_panel_allows_navigation(target),
        Service::Meet | Service::WhatsApp | Service::YouTube | Service::Gmail => {
            service_panel_allows_navigation(target)
        }
    }
}

/// Camera e microfone: pelo aviso do WebView2 nos servicos da conta do
/// utilizador; recusados, sem perguntar, no painel privado.
pub(in crate::windows_app) fn service_panel_permission(
    service: Service,
    kind: PermissionKind,
) -> PermissionResponse {
    if service.private() {
        return PermissionResponse::Deny;
    }
    web_media_permission(kind, true)
}

/// Largura logica que o painel aberto tira ao comparador (0 fora dele ou
/// sem painel). Qualquer servico conta -- a Respiracao incluida --, porque
/// o painel dele fica ao lado das colunas como os outros (o estado vem do
/// `ServicePanel`, igual para todos); o do Gemini Live (`live_panel`) tem a
/// largura dos servicos. As larguras sao as escolhidas pela borda
/// (`panel-width.json`), e o de servicos minimizado nao cede nada
/// (`reserved_panel_width`).
pub(in crate::windows_app) fn open_panel_width_for(
    surface: Surface,
    service: Option<ServicePanelState>,
    live_panel: bool,
    side_panel: bool,
    logical_w: f64,
    widths: PanelWidths,
) -> f64 {
    if surface != Surface::Comparator {
        return 0.0;
    }
    reserved_panel_width(
        service,
        live_panel,
        side_panel,
        panel_width(
            PanelKind::Service,
            widths.get(PanelKind::Service),
            logical_w,
        ),
        panel_width(
            PanelKind::History,
            widths.get(PanelKind::History),
            logical_w,
        ),
    )
}

/// Mais largo do que o do historico: o WhatsApp e o Meet precisam de espaco.
#[cfg(test)]
pub(in crate::windows_app) fn service_panel_bounds(
    logical_w: f64,
    logical_h: f64,
    top: f64,
) -> (f64, f64, f64, f64) {
    panel_bounds(PanelKind::Service, None, logical_w, logical_h, top)
}

/// Botao redondo so com icone (servicos, Gmail, Privado). O icone branco e
/// pintado com `tint`; os coloridos (WhatsApp, YouTube) vao com `None`.
pub(in crate::windows_app) unsafe fn draw_icon_button(
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

/// Botao de uma ferramenta: o mesmo circulo dos servicos, com o icone no
/// quadrado da esquerda e, se houver, a etiqueta (o tempo do Pomodoro) no
/// resto -- `right_controls` e `home_tool_buttons` ja alargaram o botao.
/// Com `phase` (uma sessao do Pomodoro em curso) o tempo e o contorno vao na
/// cor da fase.
#[allow(clippy::too_many_arguments)]
pub(in crate::windows_app) unsafe fn draw_tool_button(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    tool: Tool,
    label: Option<&str>,
    phase: Option<Phase>,
    hovered: bool,
    scale: f64,
    font: *mut core::ffi::c_void,
    theme: &Theme,
    background: Rgb,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let fill = if hovered {
        theme.surface_line
    } else {
        theme.surface
    };
    let label_color = tool_label_color(phase, fill, theme);
    let border = if phase.is_some() {
        label_color
    } else {
        theme.surface_line
    };
    fill_pill(
        hdc,
        rect,
        rect.height / 2.0,
        fill,
        Some((border, scale)),
        background,
    );
    let size = (rect.height * 0.6).round() as i32;
    let x = (rect.x + (rect.height - size as f64) / 2.0).round() as i32;
    let y = (rect.y + (rect.height - size as f64) / 2.0).round() as i32;
    draw_icon(
        hdc,
        tool.icon_slot(),
        x,
        y,
        size,
        fill,
        tool.icon_tint(theme),
    );

    // So ha etiqueta se o botao alargou para ela; senao seria escrita por
    // cima do icone.
    if let Some(text) = label
        && rect.width > rect.height + 1.0
    {
        SelectObject(hdc, font as _);
        SetTextColor(hdc, rgb3(label_color));
        SetBkMode(hdc, TRANSPARENT as i32);
        let mut text_rect = RECT {
            left: (rect.x + rect.height * 0.85).round() as i32,
            top: rect.y.round() as i32,
            right: (rect.x + rect.width - rect.height * 0.25).round() as i32,
            bottom: (rect.y + rect.height).round() as i32,
        };
        draw_text(
            hdc,
            text,
            &mut text_rect,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
        );
    }
}

/// A dica do olho. O estado ve-se na cor do botao.
pub(in crate::windows_app) const LIVE_TOOLTIP: &str = "Gemini Live: ver a tela, câmera e microfone (liga/desliga)";

/// Vermelho de "a gravar": com o Gemini Live ligado o botao fica cheio desta
/// cor, para ninguem esquecer que a tela, a camera e o microfone estao a sair.
pub(in crate::windows_app) const LIVE_ON_RED: Rgb = (217, 48, 37);

/// Fundo, borda e cor do olho no botao do Gemini Live. Cheio de vermelho so
/// quando algo pode estar a sair; com o painel aberto e nada a sair (a pedir
/// a chave, ou a sessao caiu) o olho e a borda ficam vermelhos, o fundo nao.
pub(in crate::windows_app) fn live_button_colors(
    indicator: LiveIndicator,
    hovered: bool,
    theme: &Theme,
) -> (Rgb, Rgb, Rgb) {
    match (indicator, hovered) {
        (LiveIndicator::Live, false) => (LIVE_ON_RED, LIVE_ON_RED, (255, 255, 255)),
        (LiveIndicator::Live, true) => {
            let deep = mix(LIVE_ON_RED, (0, 0, 0), 0.18);
            (deep, deep, (255, 255, 255))
        }
        (LiveIndicator::Standby, true) => (theme.surface_line, LIVE_ON_RED, LIVE_ON_RED),
        (LiveIndicator::Standby, false) => (theme.surface, LIVE_ON_RED, LIVE_ON_RED),
        (LiveIndicator::Off, true) => (theme.surface_line, theme.surface_line, theme.fg),
        (LiveIndicator::Off, false) => (theme.surface, theme.surface_line, theme.fg),
    }
}

pub(in crate::windows_app) unsafe fn draw_live_button(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    indicator: LiveIndicator,
    hovered: bool,
    scale: f64,
    theme: &Theme,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let (fill, border, tint) = live_button_colors(indicator, hovered, theme);
    fill_pill(
        hdc,
        rect,
        rect.height / 2.0,
        fill,
        Some((border, scale)),
        theme.bar_bg,
    );
    let size = (rect.height * 0.6).round() as i32;
    let x = (rect.x + (rect.width - size as f64) / 2.0).round() as i32;
    let y = (rect.y + (rect.height - size as f64) / 2.0).round() as i32;
    draw_icon(hdc, ICON_SLOT_LIVE, x, y, size, fill, Some(tint));
}

/// Avisos do Gmail ligados (o botao do envelope). Guardado em
/// `<data_dir>/gmail` como "ligado"/"desligado"; sem ficheiro, ligado.
pub(in crate::windows_app) static GMAIL_NOTIFICATIONS: AtomicBool = AtomicBool::new(true);

pub(in crate::windows_app) fn load_gmail_setting(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|text| !text.trim().eq_ignore_ascii_case("desligado"))
        .unwrap_or(true)
}

pub(in crate::windows_app) fn save_gmail_setting(path: &std::path::Path, on: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, if on { "ligado" } else { "desligado" })?;
    std::fs::rename(&temp, path)
}

/// O que o painel de servicos acrescenta ao chrome nativo: o ponto no icone
/// do servico minimizado (vermelho a tocar, na cor de destaque em silencio) e
/// a faixa [— Minimizar] [⛶ Tela cheia] [× Fechar] por cima do painel.
pub(in crate::windows_app) fn draw_service_chrome(
    window: &Window,
    split_active: bool,
    service: Service,
    badge: Option<ServiceBadge>,
    strip: Option<Area>,
    hover: Option<BarHit>,
) {
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let scale = window.scale_factor().max(1.0);
    let theme = Theme::system();
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return;
        }
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);

        if let Some(badge) = badge {
            // Os icones dos servicos e o botao da Respiracao nao dependem da
            // etiqueta do Pomodoro (so o proprio Pomodoro alarga).
            let controls = right_controls(client.right.max(1) as f64, scale, split_active, None);
            if let Some(icon) = service_icon_rect(controls, service) {
                let color = match badge {
                    ServiceBadge::Playing => LIVE_ON_RED,
                    ServiceBadge::Minimized => theme.accent,
                };
                let radius = (icon.height * 0.17).max(3.0);
                let cx = icon.x + icon.width - radius * 0.9;
                let cy = icon.y + radius * 0.9;
                let brush = CreateSolidBrush(rgb3(color));
                let pen = CreatePen(
                    PS_SOLID as _,
                    (1.5 * scale).round() as i32,
                    rgb3(theme.bar_bg),
                );
                let old_brush = SelectObject(hdc, brush as _);
                let old_pen = SelectObject(hdc, pen as _);
                Ellipse(
                    hdc,
                    (cx - radius).round() as i32,
                    (cy - radius).round() as i32,
                    (cx + radius).round() as i32,
                    (cy + radius).round() as i32,
                );
                SelectObject(hdc, old_pen);
                SelectObject(hdc, old_brush);
                DeleteObject(pen as _);
                DeleteObject(brush as _);
            }
        }

        if let Some(strip) = strip {
            let area = RECT {
                left: strip.x.round() as i32,
                top: strip.y.round() as i32,
                right: (strip.x + strip.width).round() as i32,
                bottom: (strip.y + strip.height).round() as i32,
            };
            let bg = CreateSolidBrush(rgb3(theme.bar_bg));
            FillRect(hdc, &area, bg);
            DeleteObject(bg as _);
            let line = RECT {
                top: area.bottom - 1,
                ..area
            };
            let line_brush = CreateSolidBrush(rgb3(theme.bar_line));
            FillRect(hdc, &line, line_brush);
            DeleteObject(line_brush as _);

            let font = create_font((-12.5 * scale).round() as i32, FW_NORMAL as i32);
            let old_font = SelectObject(hdc, font as _);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, rgb3(theme.fg_muted));
            let buttons = strip_buttons(strip, scale);
            let mut label = RECT {
                left: area.left + (12.0 * scale).round() as i32,
                right: (buttons[0].x - 6.0 * scale).round() as i32,
                ..area
            };
            draw_text(
                hdc,
                service.label(),
                &mut label,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            for (rect, button) in buttons.iter().zip(StripButton::ALL) {
                let hovered = hover == Some(BarHit::ServiceStrip(button));
                let style = match (button, hovered) {
                    (StripButton::Close, _) => caption_button_style(2, hovered, &theme),
                    (_, true) => PillStyle::new(theme.surface_line, theme.surface_line, theme.fg),
                    (_, false) => PillStyle::new(theme.surface, theme.surface_line, theme.fg),
                };
                draw_pill(
                    hdc,
                    UiRect {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: rect.height,
                    },
                    button.label(),
                    style,
                    scale,
                    font,
                    theme.bar_bg,
                );
            }
            SelectObject(hdc, old_font);
            DeleteObject(font as _);
        }
        ReleaseDC(hwnd, hdc);
    }
}

/// Corta um campo vindo do monitor a `GMAIL_FIELD_MAX_CHARS` chars, na
/// fronteira de char e nao de byte: `String::truncate` a meio de um UTF-8
/// entra em panico, e um remetente com acentos e o caso normal. O que ja
/// cabe volta intacto, sem alocar.
pub(in crate::windows_app) fn gmail_field(mut value: String) -> String {
    if value.len() <= GMAIL_FIELD_MAX_CHARS {
        return value;
    }
    if let Some((offset, _)) = value.char_indices().nth(GMAIL_FIELD_MAX_CHARS) {
        value.truncate(offset);
    }
    value
}

pub(in crate::windows_app) fn gmail_is_new_mail(
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
