//! Abas Web independentes: cada clique no + superior cria um WebView2 próprio.
//! A barra é nativa e não envia pesquisas às IAs do comparador.
use crate::windows_app::*;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW;

pub(in crate::windows_app) const LEFT_BROWSER_BAR_HEIGHT: f64 = 48.0;
const LEFT_BROWSER_SUBCLASS_ID: usize = 0x4E91;
const LEFT_BROWSER_TOOLBAR_ID: usize = 0x4E92;

pub(in crate::windows_app) struct LeftBrowserTab {
    pub(in crate::windows_app) view: WebView,
    pub(in crate::windows_app) title: String,
}

pub(in crate::windows_app) fn left_browser_new_window_event(target: String) -> Option<UserEvent> {
    (!target.eq_ignore_ascii_case("about:blank") && remote_web_target(&target, None))
        .then_some(UserEvent::LeftBrowserSubmit(target))
}

pub(in crate::windows_app) fn left_browser_visible(
    surface: Surface,
    expanded: Option<usize>,
    split_fullscreen: bool,
) -> bool {
    surface == Surface::Comparator && expanded.is_none() && !split_fullscreen
}

/// Peso inicial da aba Web: o mesmo de uma coluna de IA, para ela nascer com
/// a largura de uma coluna e não com um terço fixo.
pub(in crate::windows_app) const LEFT_BROWSER_DEFAULT_WEIGHT: f64 = 1.0;

/// A geometria das colunas com a aba Web à vista. A Web ocupa a faixa da
/// primeira IA (a sessão dela continua viva, só escondida), com o seu próprio
/// peso; as outras IAs mantêm os delas. Assim a Web reparte a largura pela
/// mesma conta que as colunas (`visible_column_spans`) e o divisor entre a
/// Web e a IA ao lado arrasta-se como qualquer outro (`resized_weights`).
/// Com a Web minimizada a geometria é a das IAs, sem mudança nenhuma.
pub(in crate::windows_app) fn left_browser_column_model(
    weights: &[f64; COMPARATOR_COLUMNS],
    minimized: &[bool; COMPARATOR_COLUMNS],
    browser_weight: f64,
    browser_minimized: bool,
) -> ([f64; COMPARATOR_COLUMNS], [bool; COMPARATOR_COLUMNS]) {
    let mut weights = *weights;
    let mut minimized = *minimized;
    if !browser_minimized {
        weights[0] = browser_weight.max(0.05);
        minimized[0] = false;
    }
    (weights, minimized)
}

/// Largura da Web e faixas `(coluna, x, largura)` das IAs que ficam à vista.
pub(in crate::windows_app) fn left_browser_content_spans(
    width: f64,
    columns: usize,
    weights: &[f64; COMPARATOR_COLUMNS],
    minimized: &[bool; COMPARATOR_COLUMNS],
    browser: (f64, bool, bool),
) -> (f64, Vec<(usize, f64, f64)>) {
    let (browser_weight, browser_minimized, browser_expanded) = browser;
    let width = width.max(1.0);
    if browser_expanded && !browser_minimized {
        return (width, Vec::new());
    }
    let (weights, minimized) =
        left_browser_column_model(weights, minimized, browser_weight, browser_minimized);
    // A Web precisa da faixa 0 mesmo que o comparador ainda não tenha vistas.
    let columns = if browser_minimized {
        columns
    } else {
        columns.max(1)
    };
    let mut browser_width = 0.0;
    let mut spans = Vec::new();
    for span in visible_column_spans(width, columns, &weights, &minimized) {
        if span.index == 0 && !browser_minimized {
            browser_width = span.width.max(1.0);
        } else {
            spans.push((span.index, span.x, span.width.max(1.0)));
        }
    }
    (browser_width, spans)
}

/// Pesos depois de arrastar o divisor `divider` com a aba Web à vista:
/// devolve o peso novo da Web e os das IAs. Só o par vizinho do divisor muda,
/// como entre duas IAs; a primeira IA, escondida atrás da Web, guarda o seu.
pub(in crate::windows_app) fn left_browser_resized_weights(
    columns: usize,
    weights: &[f64; COMPARATOR_COLUMNS],
    minimized: &[bool; COMPARATOR_COLUMNS],
    browser_weight: f64,
    browser_minimized: bool,
    drag: (usize, f64, f64),
) -> (f64, [f64; COMPARATOR_COLUMNS]) {
    let (divider, mouse_x, logical_w) = drag;
    let (model_weights, model_minimized) =
        left_browser_column_model(weights, minimized, browser_weight, browser_minimized);
    let columns = if browser_minimized {
        columns
    } else {
        columns.max(1)
    };
    let visible: Vec<usize> = (0..columns.min(COMPARATOR_COLUMNS))
        .filter(|index| !model_minimized[*index])
        .collect();
    let next = resized_weights(&model_weights, &visible, divider, mouse_x, logical_w);
    if browser_minimized {
        return (browser_weight, next);
    }
    let mut ai = next;
    ai[0] = weights[0];
    (next[0], ai)
}

/// Os três botões da barra Web, pela ordem em que aparecem: os mesmos da
/// janela (`caption_buttons_subclass`), com o mesmo desenho e o mesmo tamanho.
pub(in crate::windows_app) const LEFT_BROWSER_CAPTION_WIDTH: f64 = 46.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum LeftBrowserButton {
    Minimize,
    Maximize,
    Close,
}

impl LeftBrowserButton {
    pub(in crate::windows_app) const ALL: [Self; 3] = [Self::Minimize, Self::Maximize, Self::Close];

    fn glyph(self, expanded: bool) -> &'static str {
        match self {
            Self::Minimize => "—",
            Self::Maximize if expanded => "❐",
            Self::Maximize => "□",
            Self::Close => "×",
        }
    }

    pub(in crate::windows_app) fn tooltip(self, expanded: bool) -> &'static str {
        match self {
            Self::Minimize => "Minimizar aba Web",
            Self::Maximize if expanded => "Restaurar aba Web",
            Self::Maximize => "Maximizar aba Web",
            Self::Close => "Fechar aba Web",
        }
    }

    pub(in crate::windows_app) fn event(self) -> UserEvent {
        match self {
            Self::Minimize => UserEvent::LeftBrowserMinimize,
            Self::Maximize => UserEvent::LeftBrowserExpand,
            Self::Close => UserEvent::LeftBrowserClose,
        }
    }

    /// O índice em `caption_button_style`: só o fechar fica vermelho.
    fn caption_index(self) -> usize {
        match self {
            Self::Minimize => 0,
            Self::Maximize => 1,
            Self::Close => 2,
        }
    }
}

/// Geometria da barra Web em pixels do cliente da barra: o campo de endereço
/// (pílula com lupa, como a omnibox) e, encostados à direita, os três botões
/// de janela com 46 × 32 px lógicos, a medida dos da barra de título.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::windows_app) struct LeftBrowserToolbarLayout {
    pub(in crate::windows_app) address: UiRect,
    pub(in crate::windows_app) edit: UiRect,
    pub(in crate::windows_app) buttons: [UiRect; 3],
}

impl LeftBrowserToolbarLayout {
    pub(in crate::windows_app) fn new(width: f64, scale: f64) -> Self {
        let scale = scale.max(1.0);
        let width = width.max(1.0);
        let bar_h = LEFT_BROWSER_BAR_HEIGHT * scale;
        let button_w = LEFT_BROWSER_CAPTION_WIDTH * scale;
        let button_h = TITLE_TAB_HEIGHT * scale;
        let button_y = (bar_h - button_h) / 2.0;
        let margin = 8.0 * scale;
        // Folga à direita: o divisor arrastável da fronteira (7 px centrados
        // nela) nunca tapa o fechar.
        let right = 4.0 * scale;
        // Numa faixa estreita os botões encolhem por igual, mas nunca saem.
        let group_w = (button_w * 3.0).min((width - margin - right).max(3.0));
        let each = group_w / 3.0;
        let group_x = (width - right - group_w).max(0.0);
        let buttons = std::array::from_fn(|index| UiRect {
            x: group_x + index as f64 * each,
            y: button_y,
            width: each,
            height: button_h,
        });
        let pill_h = 36.0 * scale;
        let address = UiRect {
            x: margin,
            y: (bar_h - pill_h) / 2.0,
            width: (group_x - margin * 2.0).max(0.0),
            height: pill_h,
        };
        // O EDIT vive dentro da pílula, depois da lupa.
        let lens = 32.0 * scale;
        let edit_h = 24.0 * scale;
        let edit = UiRect {
            x: address.x + lens,
            y: address.y + (pill_h - edit_h) / 2.0,
            width: (address.width - lens - 12.0 * scale).max(1.0),
            height: edit_h,
        };
        Self {
            address,
            edit,
            buttons,
        }
    }

    pub(in crate::windows_app) fn hit(&self, x: f64, y: f64) -> Option<LeftBrowserButton> {
        LeftBrowserButton::ALL
            .into_iter()
            .zip(self.buttons)
            .find(|(_, rect)| rect.width > 0.0 && rect.contains(x, y))
            .map(|(button, _)| button)
    }
}

/// Estado que a barra (sem acesso ao `App`) precisa para se pintar.
static LEFT_BROWSER_HOVER: AtomicUsize = AtomicUsize::new(usize::MAX);
static LEFT_BROWSER_PRESSED: AtomicUsize = AtomicUsize::new(usize::MAX);
static LEFT_BROWSER_EXPANDED: AtomicBool = AtomicBool::new(false);
/// Instante (`GetMessageTime`) do último clique na faixa vazia da barra:
/// dois cliques seguidos maximizam/restauram, como numa barra de título.
static LEFT_BROWSER_LAST_CLICK: AtomicI32 = AtomicI32::new(0);

fn left_browser_button_at(index: usize) -> Option<LeftBrowserButton> {
    LeftBrowserButton::ALL.get(index).copied()
}

fn left_browser_button_index(button: Option<LeftBrowserButton>) -> usize {
    button
        .and_then(|button| LeftBrowserButton::ALL.iter().position(|b| *b == button))
        .unwrap_or(usize::MAX)
}

fn lparam_point(lparam: LPARAM) -> (f64, f64) {
    let x = (lparam as u32 & 0xffff) as u16 as i16 as f64;
    let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as f64;
    (x, y)
}

pub(in crate::windows_app) unsafe extern "system" fn left_browser_edit_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    reference: usize,
) -> LRESULT {
    if reference != 0 {
        let proxy = unsafe { &*(reference as *const EventLoopProxy<UserEvent>) };
        if message == WM_SETFOCUS {
            let _ = proxy.send_event(UserEvent::LeftBrowserFocus);
        }
        if message == WM_KEYDOWN {
            if wparam == 13 {
                let input = unsafe { window_text(hwnd) };
                let _ = proxy.send_event(UserEvent::LeftBrowserSubmit(input));
                return 0;
            }
            let ctrl = (unsafe { GetAsyncKeyState(VK_CONTROL as i32) } as u16 & 0x8000) != 0;
            if ctrl && (wparam == 0x41 || wparam == 0x4C) {
                unsafe { SendMessageW(hwnd, EM_SETSEL, 0, -1) };
                return 0;
            }
        }
    }
    if message == WM_SETFOCUS || message == WM_KILLFOCUS {
        // A borda da pílula muda com o foco, como a da omnibox.
        unsafe { InvalidateRect(GetParent(hwnd), std::ptr::null(), 0) };
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Barra da aba Web. Segue o padrão das barras da NeuralIA: o campo de
/// endereço é a pílula com lupa da omnibox (borda de foco incluída) e, à
/// direita, os três botões da janela (`caption_buttons_subclass`) com o
/// mesmo desenho, o mesmo tamanho, a mesma dica e o mesmo clique confirmado
/// ao soltar: minimizar, maximizar/restaurar (❐ quando maximizada) e fechar,
/// vermelho debaixo do rato. Dois cliques na faixa vazia maximizam ou
/// restauram, como numa barra de título.
pub(in crate::windows_app) unsafe extern "system" fn left_browser_toolbar_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    reference: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetMessageTime;

    let layout = || {
        let mut client = RECT::default();
        if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
            return None;
        }
        let width = (client.right - client.left).max(1) as f64;
        let height = (client.bottom - client.top).max(1) as f64;
        let scale = (height / LEFT_BROWSER_BAR_HEIGHT).max(1.0);
        Some((client, scale, LeftBrowserToolbarLayout::new(width, scale)))
    };
    let send = |event: UserEvent| {
        if reference != 0 {
            let proxy = unsafe { &*(reference as *const EventLoopProxy<UserEvent>) };
            let _ = proxy.send_event(event);
        }
    };

    match message {
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_ERASEBKGND => 1,
        WM_CTLCOLOREDIT => {
            let theme = Theme::system();
            let hdc = wparam as *mut core::ffi::c_void;
            unsafe {
                SetTextColor(hdc, rgb3(theme.fg));
                SetBkColor(hdc, rgb3(theme.surface));
            }
            omnibox_brush(theme.surface) as LRESULT
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
            if !hdc.is_null() {
                if let Some((client, scale, layout)) = layout() {
                    unsafe { paint_left_browser_toolbar(hwnd, hdc, client, scale, layout) };
                }
                unsafe { EndPaint(hwnd, &paint) };
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some((_, _, layout)) = layout() {
                let (x, y) = lparam_point(lparam);
                let index = left_browser_button_index(layout.hit(x, y));
                let last = LEFT_BROWSER_HOVER.swap(index, Ordering::AcqRel);
                if index != last {
                    if last == usize::MAX {
                        track_mouse_leave(hwnd);
                    }
                    let expanded = LEFT_BROWSER_EXPANDED.load(Ordering::Acquire);
                    let text =
                        left_browser_button_at(index).map_or("", |button| button.tooltip(expanded));
                    hover_tooltip(hwnd, text);
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                }
            }
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
        WM_MOUSELEAVE => {
            LEFT_BROWSER_HOVER.store(usize::MAX, Ordering::Release);
            hover_tooltip(hwnd, "");
            unsafe {
                InvalidateRect(hwnd, std::ptr::null(), 0);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        WM_LBUTTONDOWN => {
            if let Some((_, _, layout)) = layout() {
                let (x, y) = lparam_point(lparam);
                match layout.hit(x, y) {
                    Some(button) => {
                        LEFT_BROWSER_PRESSED
                            .store(left_browser_button_index(Some(button)), Ordering::Release);
                        hover_tooltip(hwnd, "");
                        unsafe { SetCapture(hwnd) };
                    }
                    None if !layout.address.contains(x, y) => {
                        let now = unsafe { GetMessageTime() };
                        let last = LEFT_BROWSER_LAST_CLICK.swap(now, Ordering::AcqRel);
                        let limit = unsafe { GetDoubleClickTime() } as i32;
                        if left_browser_double_click(last, now, limit) {
                            LEFT_BROWSER_LAST_CLICK.store(0, Ordering::Release);
                            send(UserEvent::LeftBrowserExpand);
                        }
                    }
                    None => {}
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let captured = unsafe { GetCapture() } == hwnd;
            let pressed = LEFT_BROWSER_PRESSED.swap(usize::MAX, Ordering::AcqRel);
            if captured {
                unsafe { ReleaseCapture() };
            }
            if captured
                && let Some((_, _, layout)) = layout()
                && let Some(button) = left_browser_button_at(pressed)
            {
                let (x, y) = lparam_point(lparam);
                // Só conta se o botão for largado onde foi carregado, como
                // os botões da janela: arrastar para fora cancela.
                if layout.hit(x, y) == Some(button) {
                    send(button.event());
                }
            }
            0
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            LEFT_BROWSER_PRESSED.store(usize::MAX, Ordering::Release);
            0
        }
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

/// Dois cliques na faixa vazia da barra dentro do tempo de duplo clique do
/// Windows. `last == 0` é "não houve clique antes".
pub(in crate::windows_app) fn left_browser_double_click(last: i32, now: i32, limit: i32) -> bool {
    let elapsed = now.wrapping_sub(last);
    last != 0 && (0..=limit).contains(&elapsed)
}

unsafe fn paint_left_browser_toolbar(
    hwnd: HWND,
    hdc: *mut core::ffi::c_void,
    client: RECT,
    scale: f64,
    layout: LeftBrowserToolbarLayout,
) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;

    let theme = Theme::system();
    unsafe {
        let background = CreateSolidBrush(rgb3(theme.bar_bg));
        FillRect(hdc, &client, background);
        DeleteObject(background as _);
        // Separador fino por baixo, como a linha da barra principal.
        let line = RECT {
            left: client.left,
            top: client.bottom - (scale.round() as i32).max(1),
            right: client.right,
            bottom: client.bottom,
        };
        let line_brush = CreateSolidBrush(rgb3(theme.surface_line));
        FillRect(hdc, &line, line_brush);
        DeleteObject(line_brush as _);

        let address = layout.address;
        if address.width > 0.0 {
            let focus = GetFocus();
            let focused = !focus.is_null() && GetParent(focus) == hwnd;
            let (border, border_width) = theme.omnibox_border(focused);
            fill_pill(
                hdc,
                address,
                address.height / 2.0,
                theme.surface,
                Some((border, border_width * scale)),
                theme.bar_bg,
            );
            let lens = RECT {
                left: (address.x + 8.0 * scale) as i32,
                top: (address.y + 4.0 * scale) as i32,
                right: (address.x + 32.0 * scale) as i32,
                bottom: (address.y + address.height - 4.0 * scale) as i32,
            };
            draw_search_lens(hdc, lens, theme.fg_muted, scale);
        }

        let expanded = LEFT_BROWSER_EXPANDED.load(Ordering::Acquire);
        let hovered = left_browser_button_at(LEFT_BROWSER_HOVER.load(Ordering::Acquire));
        let font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);
        for (button, rect) in LeftBrowserButton::ALL.into_iter().zip(layout.buttons) {
            draw_pill(
                hdc,
                rect,
                button.glyph(expanded),
                caption_button_style(button.caption_index(), hovered == Some(button), &theme),
                scale,
                font,
                theme.bar_bg,
            );
        }
        DeleteObject(font as _);
    }
}

impl App {
    /// Fecha todas as abas Web, sem alterar as sessões das três IAs.
    pub(in crate::windows_app) fn close_left_browser_tab(&mut self) {
        for tab in self.left_browser_tabs.drain(..) {
            let _ = tab.view.set_visible(false);
            let _ = tab.view.focus_parent();
        }
        self.left_browser_active = None;
        self.left_browser_minimized = false;
        self.left_browser_expanded = false;
        LEFT_BROWSER_EXPANDED.store(false, Ordering::Release);
        LEFT_BROWSER_HOVER.store(usize::MAX, Ordering::Release);
        LEFT_BROWSER_PRESSED.store(usize::MAX, Ordering::Release);
        if let Some(address) = self.left_browser_address.take() {
            unsafe { DestroyWindow(address) };
        }
        if let Some(toolbar) = self.left_browser_toolbar.take() {
            unsafe { DestroyWindow(toolbar) };
        }
        if let Some(font) = self.left_browser_font.take() {
            unsafe { DeleteObject(font as _) };
        }
        self.request_redraw();
    }

    pub(in crate::windows_app) fn open_left_browser_tab(&mut self) {
        self.create_left_browser_tab(None);
    }

    pub(in crate::windows_app) fn open_left_browser_link(&mut self, target: Url) {
        self.prepare_browser_link_workspace();
        self.create_left_browser_tab(Some(target));
    }

    fn create_left_browser_tab(&mut self, target: Option<Url>) {
        if self.surface != Surface::Comparator {
            return;
        }
        let Some(window) = &self.window else { return };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let size = window.inner_size();
        let logical_width = size.width as f64 / scale;
        let logical_height = size.height as f64 / scale;
        let left_width = (comparator_logical_width(
            logical_width,
            self.comparator.as_ref().map_or(0.0, |c| c.panel_width),
        ) / 3.0)
            .max(1.0);
        let bounds = wry::Rect {
            position: LogicalPosition::new(0.0, COMPARATOR_CHROME_HEIGHT + LEFT_BROWSER_BAR_HEIGHT)
                .into(),
            size: LogicalSize::new(
                left_width,
                (logical_height - COMPARATOR_CHROME_HEIGHT - LEFT_BROWSER_BAR_HEIGHT).max(1.0),
            )
            .into(),
        };
        let new_window_proxy = self.proxy.clone();
        let builder = self
            .external_webview_builder(None, false)
            .with_bounds(bounds)
            .with_new_window_req_handler(move |target, _features| {
                if let Some(event) = left_browser_new_window_event(target) {
                    let _ = new_window_proxy.send_event(event);
                }
                wry::NewWindowResponse::Deny
            });
        let builder = match &target {
            Some(url) => builder.with_url(url.as_str()),
            None => builder.with_html(browser_new_tab_html()),
        };
        let hooked = self.hooked_builder(builder, WebViewHost::External, None);
        let result = hooked.build_hooked_as_child(window);
        let view = match result {
            Ok(view) => view,
            Err(error) => {
                self.show_splash(format!("Não foi possível criar a aba Web: {error}"), 4);
                return;
            }
        };

        if self.left_browser_toolbar.is_none() && !self.create_left_browser_toolbar(owner, scale) {
            drop(view);
            return;
        }

        if let Some(old) = self
            .left_browser_active
            .and_then(|index| self.left_browser_tabs.get(index))
        {
            let _ = old.view.set_visible(false);
        }
        let index = self.left_browser_tabs.len();
        let title = target.as_ref().map_or_else(
            || format!("Nova aba {}", index + 1),
            |url| url.host_str().unwrap_or("Web").to_owned(),
        );
        self.left_browser_tabs.push(LeftBrowserTab { view, title });
        self.left_browser_active = Some(index);
        self.left_browser_minimized = false;
        self.left_browser_expanded = false;
        if let Some(address) = self.left_browser_address {
            let text = target.as_ref().map_or("", Url::as_str);
            unsafe { SetWindowTextW(address, wide_null(text).as_ptr()) };
        }
        self.refresh_left_browser_layout();
        if target.is_some() {
            let _ = self.left_browser_tabs[index].view.focus();
        } else if let Some(address) = self.left_browser_address {
            unsafe { SetFocus(address) };
        }
    }

    /// Reaplica a geometria depois de a aba Web mudar de estado: as faixas, os
    /// divisores arrastáveis (a Web redimensiona-se como uma coluna) e o
    /// glifo ❐/□ do botão de maximizar.
    pub(in crate::windows_app) fn refresh_left_browser_layout(&mut self) {
        LEFT_BROWSER_EXPANDED.store(
            self.left_browser_expanded && !self.left_browser_minimized,
            Ordering::Release,
        );
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        if let Some(toolbar) = self.left_browser_toolbar {
            unsafe { InvalidateRect(toolbar, std::ptr::null(), 0) };
        }
        self.request_redraw();
    }

    /// Com abas Web abertas e à vista do comparador, a geometria das colunas
    /// é a da Web (`left_browser_column_model`), e não a das IAs sozinhas.
    pub(in crate::windows_app) fn left_browser_owns_layout(&self) -> bool {
        !self.left_browser_tabs.is_empty()
            && self.comparator.as_ref().is_some_and(|comp| {
                left_browser_visible(
                    self.surface,
                    comp.expanded,
                    comp.split.as_ref().is_some_and(|split| split.fullscreen),
                )
            })
    }

    /// O arrasto de um divisor com a aba Web na geometria: a fronteira Web|IA
    /// muda o peso da Web, as outras mudam o das IAs, como sem a Web.
    pub(in crate::windows_app) fn resize_left_browser_columns(
        &mut self,
        divider: usize,
        screen_x: i32,
    ) {
        if self.left_browser_expanded && !self.left_browser_minimized {
            return;
        }
        let content_x = self.left_panel_width();
        let (Some(window), Some(comp)) = (&self.window, &mut self.comparator) else {
            return;
        };
        if comp.expanded.is_some() {
            return;
        }
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let mut point = POINT { x: screen_x, y: 0 };
        unsafe { ScreenToClient(owner, &mut point) };
        let scale = window.scale_factor().max(1.0);
        let logical_w = comparator_logical_width(
            window.inner_size().width as f64 / scale,
            comp.panel_width + content_x,
        );
        let mouse_x = (point.x as f64 / scale - content_x).clamp(0.0, logical_w);
        let (browser_weight, weights) = left_browser_resized_weights(
            comp.views.len(),
            &comp.weights,
            &comp.minimized,
            self.left_browser_weight,
            self.left_browser_minimized,
            (divider, mouse_x, logical_w),
        );
        comp.weights = weights;
        self.left_browser_weight = browser_weight;
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    /// O x da barra Web fecha a aba Web ativa; as IAs ficam como estavam.
    pub(in crate::windows_app) fn close_active_left_browser_tab(&mut self) {
        if let Some(index) = self.left_browser_active {
            self.close_one_left_browser_tab(index);
        }
    }

    fn create_left_browser_toolbar(&mut self, owner: HWND, scale: f64) -> bool {
        let toolbar = unsafe {
            CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                windows_sys::w!("NeuralIA.WebToolbar"),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                300,
                (LEFT_BROWSER_BAR_HEIGHT * scale).round() as i32,
                owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if toolbar.is_null() {
            self.show_splash("Não foi possível criar a barra Web.".to_owned(), 4);
            return false;
        }
        let reference = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
        if unsafe {
            SetWindowSubclass(
                toolbar,
                Some(left_browser_toolbar_subclass),
                LEFT_BROWSER_TOOLBAR_ID,
                reference,
            )
        } == 0
        {
            unsafe { DestroyWindow(toolbar) };
            self.show_splash("Não foi possível inicializar a barra Web.".to_owned(), 4);
            return false;
        }
        let address = unsafe {
            CreateWindowExW(
                0,
                windows_sys::w!("EDIT"),
                windows_sys::w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                38,
                10,
                160,
                (28.0 * scale).round() as i32,
                toolbar,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if address.is_null()
            || unsafe {
                SetWindowSubclass(
                    address,
                    Some(left_browser_edit_subclass),
                    LEFT_BROWSER_SUBCLASS_ID,
                    reference,
                )
            } == 0
        {
            unsafe { DestroyWindow(toolbar) };
            self.show_splash("Não foi possível inicializar o campo Web.".to_owned(), 4);
            return false;
        }
        let cue = wide_null("Endereço ou pesquisa Web");
        let font = unsafe { create_font((-17.0 * scale).round() as i32, FW_NORMAL as i32) };
        unsafe {
            SendMessageW(address, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
            SendMessageW(address, EM_SETLIMITTEXT, 2048, 0);
            if !font.is_null() {
                SendMessageW(address, WM_SETFONT, font as usize, 1);
            }
        }
        self.left_browser_toolbar = Some(toolbar);
        self.left_browser_address = Some(address);
        self.left_browser_font = (!font.is_null()).then_some(font);
        true
    }

    pub(in crate::windows_app) fn switch_left_browser_tab(&mut self, index: usize) {
        if index >= self.left_browser_tabs.len() {
            return;
        }
        if let Some(old) = self
            .left_browser_active
            .and_then(|i| self.left_browser_tabs.get(i))
        {
            let _ = old.view.set_visible(false);
        }
        self.left_browser_active = Some(index);
        self.left_browser_minimized = false;
        self.sync_left_browser_address();
        self.refresh_left_browser_layout();
    }

    pub(in crate::windows_app) fn close_one_left_browser_tab(&mut self, index: usize) {
        if index >= self.left_browser_tabs.len() {
            return;
        }
        let tab = self.left_browser_tabs.remove(index);
        let _ = tab.view.set_visible(false);
        let _ = tab.view.focus_parent();
        drop(tab);
        if self.left_browser_tabs.is_empty() {
            self.close_left_browser_tab();
            self.update_comparator_layout();
            self.sync_comparator_splitters();
        } else {
            let next = match self.left_browser_active {
                Some(active) if active > index => active - 1,
                Some(active) if active == index => index.min(self.left_browser_tabs.len() - 1),
                Some(active) => active,
                None => 0,
            };
            self.left_browser_active = Some(next);
            self.sync_left_browser_address();
            self.refresh_left_browser_layout();
        }
    }

    pub(in crate::windows_app) fn toggle_left_browser_minimized(&mut self) {
        if self.left_browser_tabs.is_empty() {
            return;
        }
        self.left_browser_minimized = !self.left_browser_minimized;
        if self.left_browser_minimized {
            self.left_browser_expanded = false;
        }
        self.refresh_left_browser_layout();
    }

    pub(in crate::windows_app) fn toggle_left_browser_expanded(&mut self) {
        if self.left_browser_tabs.is_empty() {
            return;
        }
        self.left_browser_minimized = false;
        self.left_browser_expanded = !self.left_browser_expanded;
        self.refresh_left_browser_layout();
    }

    /// Menu nativo de abas excedentes: seleção explícita, não um ciclo
    /// que muda de aba sem dizer qual será aberta.
    pub(in crate::windows_app) fn show_left_browser_tab_list(&mut self) {
        let (Some(window), Some(layout)) = (&self.window, self.bar_layout()) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let anchor = layout.browser_tab_overflow;
        let mut position = POINT {
            x: (anchor.x * scale).round() as i32,
            y: ((anchor.y + anchor.height) * scale).round() as i32,
        };
        unsafe { ClientToScreen(owner, &mut position) };
        let mut menu = PopupMenu::default();
        const FIRST_BROWSER_MENU_ID: usize = 1000;
        for (index, tab) in self.left_browser_tabs.iter().enumerate() {
            menu.push(
                MenuCommand::new(
                    FIRST_BROWSER_MENU_ID + index,
                    format!("{} — {}", index + 1, tab.title),
                )
                .checked(self.left_browser_active == Some(index))
                .moves_focus(),
            );
        }
        let chosen = self.track_menu(&menu, position, MenuButton::Left);
        if chosen >= FIRST_BROWSER_MENU_ID {
            self.switch_left_browser_tab(chosen - FIRST_BROWSER_MENU_ID);
        }
    }

    pub(in crate::windows_app) fn position_left_browser_toolbar(
        &self,
        width: f64,
        content_y: f64,
        scale: f64,
    ) {
        let (Some(toolbar), Some(address)) = (self.left_browser_toolbar, self.left_browser_address)
        else {
            return;
        };
        let w = (width.max(1.0) * scale).round() as i32;
        let h = (LEFT_BROWSER_BAR_HEIGHT * scale).round() as i32;
        // O campo segue a mesma geometria que a pintura usa para a pílula e
        // para os botões: nunca fica por baixo do minimizar/maximizar/fechar.
        let layout = LeftBrowserToolbarLayout::new(w as f64, scale);
        let edit = layout.edit;
        let edit_visible = layout.address.width > edit.x - layout.address.x;
        unsafe {
            SetWindowPos(
                toolbar,
                std::ptr::null_mut(),
                (self.left_panel_width() * scale).round() as i32,
                (content_y * scale).round() as i32,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            SetWindowPos(
                address,
                std::ptr::null_mut(),
                edit.x.round() as i32,
                edit.y.round() as i32,
                edit.width.round().max(1.0) as i32,
                edit.height.round().max(1.0) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            ShowWindow(toolbar, SW_SHOW);
            ShowWindow(address, if edit_visible { SW_SHOW } else { SW_HIDE });
            InvalidateRect(toolbar, std::ptr::null(), 0);
        }
    }

    pub(in crate::windows_app) fn submit_left_browser_address(&mut self, input: &str) {
        if self.surface != Surface::Comparator {
            return;
        }
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        let Some(target) = detect_web_target(input).or_else(|| google_search_url(input)) else {
            return;
        };
        let Ok(url) = neural_core::validate_web_url(&target) else {
            self.show_splash("Endereço Web inválido.".to_string(), 3);
            return;
        };
        if is_local_network_target(&url) {
            self.show_splash(
                "Endereço de rede local não permitido nesta aba.".to_string(),
                4,
            );
            return;
        }
        if let Some(tab) = self
            .left_browser_active
            .and_then(|index| self.left_browser_tabs.get_mut(index))
        {
            if let Err(error) = tab.view.load_url(url.as_str()) {
                self.show_splash(format!("Não foi possível navegar: {error}"), 4);
            } else {
                tab.title = url.host_str().unwrap_or("Web").to_string();
                self.request_redraw();
            }
        }
    }

    pub(in crate::windows_app) fn sync_left_browser_address(&self) {
        let (Some(address), Some(tab)) = (
            self.left_browser_address,
            self.left_browser_active
                .and_then(|index| self.left_browser_tabs.get(index)),
        ) else {
            return;
        };
        if let Ok(url) = tab.view.url() {
            unsafe {
                let value = if url.starts_with("http://") || url.starts_with("https://") {
                    url.as_str()
                } else {
                    ""
                };
                SetWindowTextW(address, wide_null(value).as_ptr());
                SendMessageW(address, EM_SETSEL, 0, -1);
            }
        }
    }
}

#[cfg(test)]
mod left_browser_tests {
    use super::*;

    #[test]
    fn layout_never_reserves_an_invisible_middle_column() {
        for count in 0..=3 {
            let mut minimized = [false; 3];
            for item in minimized.iter_mut().skip(count) {
                *item = true;
            }
            let (browser_width, spans) = left_browser_content_spans(
                1200.0,
                3,
                &[1.0; 3],
                &minimized,
                (LEFT_BROWSER_DEFAULT_WEIGHT, false, false),
            );
            assert_eq!(spans.len(), count.saturating_sub(1));
            let sum: f64 = spans.iter().map(|(_, _, w)| *w).sum();
            assert!((browser_width + sum - 1200.0).abs() < 0.01);
            assert!(spans.iter().all(|(_, _, w)| *w > 0.0));
        }
        let (full, spans) = left_browser_content_spans(
            1200.0,
            3,
            &[1.0; 3],
            &[false; 3],
            (LEFT_BROWSER_DEFAULT_WEIGHT, false, true),
        );
        assert_eq!(full, 1200.0);
        assert!(spans.is_empty());
        let (hidden, spans) = left_browser_content_spans(
            1200.0,
            3,
            &[1.0; 3],
            &[false, true, false],
            (LEFT_BROWSER_DEFAULT_WEIGHT, true, false),
        );
        assert_eq!(hidden, 0.0);
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn empty_new_window_never_dispatches_an_ai_search() {
        assert!(left_browser_new_window_event("about:blank".to_string()).is_none());
        assert!(matches!(
            left_browser_new_window_event("https://example.org".to_string()),
            Some(UserEvent::LeftBrowserSubmit(_))
        ));
    }
    /// A barra Web tem os mesmos três botões da janela, com a mesma medida
    /// (46 × 32 lógicos), encostados à direita, e o campo nunca lhes passa
    /// por baixo -- em qualquer escala e largura de faixa.
    #[test]
    fn web_bar_has_standard_window_buttons_that_never_overlap_the_address() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for logical in [420.0, 640.0, 960.0, 1600.0] {
                let width = logical * scale;
                let layout = LeftBrowserToolbarLayout::new(width, scale);
                let [minimize, maximize, close] = layout.buttons;
                for button in layout.buttons {
                    assert!(
                        (button.width - 46.0 * scale).abs() < 0.01,
                        "{logical}@{scale}"
                    );
                    assert!(
                        (button.height - 32.0 * scale).abs() < 0.01,
                        "{logical}@{scale}"
                    );
                    assert!(button.y >= 0.0);
                    assert!(button.y + button.height <= LEFT_BROWSER_BAR_HEIGHT * scale);
                }
                assert!((maximize.x - (minimize.x + minimize.width)).abs() < 0.01);
                assert!((close.x - (maximize.x + maximize.width)).abs() < 0.01);
                assert!(close.x + close.width <= width - 3.0 * scale);
                assert!(close.x + close.width >= width - 5.0 * scale);
                assert!(layout.address.x + layout.address.width <= minimize.x);
                assert!(layout.edit.x >= layout.address.x);
                assert!(
                    layout.edit.x + layout.edit.width <= layout.address.x + layout.address.width
                );
                for (button, rect) in LeftBrowserButton::ALL.into_iter().zip(layout.buttons) {
                    let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
                    assert_eq!(layout.hit(center.0, center.1), Some(button));
                }
                let inside_address = (
                    layout.edit.x + layout.edit.width / 2.0,
                    layout.edit.y + layout.edit.height / 2.0,
                );
                assert_eq!(layout.hit(inside_address.0, inside_address.1), None);
            }
        }
    }

    #[test]
    fn narrow_web_bar_keeps_every_button_inside_the_strip() {
        for width in [1.0, 20.0, 90.0, 150.0] {
            let layout = LeftBrowserToolbarLayout::new(width, 1.0);
            for button in layout.buttons {
                assert!(button.x >= 0.0, "{width}");
                assert!(button.x + button.width <= width.max(3.0) + 0.01, "{width}");
            }
            assert!(layout.address.width >= 0.0);
        }
    }

    #[test]
    fn web_bar_buttons_minimize_maximize_and_close_like_the_window() {
        let events: Vec<_> = LeftBrowserButton::ALL
            .into_iter()
            .map(LeftBrowserButton::event)
            .collect();
        assert!(matches!(events[0], UserEvent::LeftBrowserMinimize));
        assert!(matches!(events[1], UserEvent::LeftBrowserExpand));
        assert!(matches!(events[2], UserEvent::LeftBrowserClose));
        assert_eq!(LeftBrowserButton::Maximize.glyph(false), "□");
        assert_eq!(LeftBrowserButton::Maximize.glyph(true), "❐");
        assert_eq!(
            LeftBrowserButton::Maximize.tooltip(true),
            "Restaurar aba Web"
        );
        assert_eq!(
            LeftBrowserButton::Maximize.tooltip(false),
            "Maximizar aba Web"
        );
        // Só o fechar usa o estilo vermelho dos botões da janela.
        assert_eq!(LeftBrowserButton::Close.caption_index(), 2);
        assert_ne!(LeftBrowserButton::Minimize.caption_index(), 2);
        assert_ne!(LeftBrowserButton::Maximize.caption_index(), 2);
    }

    #[test]
    fn web_bar_paints_with_the_window_button_style() {
        let source = include_str!("browser_tab.rs");
        let paint = source
            .split("unsafe fn paint_left_browser_toolbar")
            .nth(1)
            .expect("paint function");
        assert!(paint.contains("caption_button_style("));
        assert!(paint.contains("omnibox_border("));
    }

    #[test]
    fn web_width_follows_its_weight_like_a_column() {
        let (browser, spans) =
            left_browser_content_spans(1200.0, 3, &[1.0; 3], &[false; 3], (2.0, false, false));
        assert!((browser - 600.0).abs() < 0.01);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].0, 1);
        assert!((spans[0].1 - 600.0).abs() < 0.01);
        assert!((spans[1].1 + spans[1].2 - 1200.0).abs() < 0.01);
        // A primeira IA fica atrás da Web e o peso dela não conta.
        let (same, _) = left_browser_content_spans(
            1200.0,
            3,
            &[9.0, 1.0, 1.0],
            &[true, false, false],
            (2.0, false, false),
        );
        assert!((same - 600.0).abs() < 0.01);
    }

    #[test]
    fn dragging_the_web_divider_resizes_only_the_web_and_its_neighbour() {
        let weights = [0.7, 1.0, 1.0];
        let minimized = [false; 3];
        // Fronteira Web|IA levada de 400 para 600 numa faixa de 1200.
        let (browser, ai) =
            left_browser_resized_weights(3, &weights, &minimized, 1.0, false, (0, 600.0, 1200.0));
        assert!(browser > 1.0, "the web must grow: {browser}");
        assert_eq!(ai[0], 0.7, "the hidden first AI keeps its weight");
        assert_eq!(ai[2], 1.0, "the far column does not move");
        let (width, spans) =
            left_browser_content_spans(1200.0, 3, &ai, &minimized, (browser, false, false));
        assert!((width - 600.0).abs() < 1.0, "web is {width}");
        assert!((spans[0].1 - width).abs() < 0.01);
        assert!((spans[1].1 - 800.0).abs() < 1.0, "the far column stays put");
        // Para além do mínimo da IA vizinha o divisor pára: ela fica com
        // MIN_PANEL_WIDTH, como entre duas IAs.
        let (wide, wide_ai) =
            left_browser_resized_weights(3, &weights, &minimized, 1.0, false, (0, 790.0, 1200.0));
        let (wide_width, wide_spans) =
            left_browser_content_spans(1200.0, 3, &wide_ai, &minimized, (wide, false, false));
        assert!(
            (wide_width - (800.0 - MIN_PANEL_WIDTH)).abs() < 1.0,
            "web is {wide_width}"
        );
        assert!((wide_spans[0].2 - MIN_PANEL_WIDTH).abs() < 1.0);
        // Encolher abaixo do mínimo das colunas não é possível.
        let (tiny, tiny_ai) =
            left_browser_resized_weights(3, &weights, &minimized, 1.0, false, (0, 10.0, 1200.0));
        let (tiny_width, _) =
            left_browser_content_spans(1200.0, 3, &tiny_ai, &minimized, (tiny, false, false));
        assert!(tiny_width >= MIN_PANEL_WIDTH - 0.01, "web is {tiny_width}");
        // Com a Web minimizada, os divisores voltam a ser só das IAs.
        let (kept, next) =
            left_browser_resized_weights(3, &weights, &minimized, 1.3, true, (0, 300.0, 1200.0));
        assert_eq!(kept, 1.3);
        assert_eq!(
            next,
            resized_weights(&weights, &[0, 1, 2], 0, 300.0, 1200.0)
        );
    }

    #[test]
    fn two_clicks_on_the_empty_bar_toggle_maximize() {
        assert!(left_browser_double_click(1000, 1200, 500));
        assert!(!left_browser_double_click(1000, 1600, 500));
        assert!(!left_browser_double_click(0, 100, 500));
        assert!(!left_browser_double_click(1000, 900, 500));
    }
}
