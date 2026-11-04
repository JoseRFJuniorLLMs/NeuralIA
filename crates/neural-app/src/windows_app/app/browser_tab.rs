//! Abas Web independentes: cada clique no + superior cria um WebView2 próprio.
//! A barra é nativa e não envia pesquisas às IAs do comparador.
use crate::windows_app::*;
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

pub(in crate::windows_app) fn left_browser_content_spans(
    width: f64,
    minimized: &[bool],
    browser_minimized: bool,
    browser_expanded: bool,
) -> (f64, Vec<(usize, f64, f64)>) {
    let width = width.max(1.0);
    if browser_expanded && !browser_minimized {
        return (width, Vec::new());
    }
    let visible: Vec<usize> = minimized
        .iter()
        .enumerate()
        // A Web ocupa o espaço da primeira IA, sem destruir a sessão
        // dela. Ao minimizar a Web, a primeira IA volta a aparecer.
        .filter_map(|(index, &min)| (!min && (browser_minimized || index != 0)).then_some(index))
        .collect();
    let slots = visible.len() + usize::from(!browser_minimized);
    if slots == 0 {
        return (0.0, Vec::new());
    }
    let browser_width = if browser_minimized {
        0.0
    } else {
        width / slots as f64
    };
    let ai_width = (width - browser_width) / visible.len().max(1) as f64;
    let mut spans = Vec::with_capacity(visible.len());
    for (position, index) in visible.into_iter().enumerate() {
        let x = browser_width + position as f64 * ai_width;
        let w = if position + 1 == slots - usize::from(!browser_minimized) {
            (width - x).max(1.0)
        } else {
            ai_width.max(1.0)
        };
        spans.push((index, x, w));
    }
    (browser_width, spans)
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
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Toolbar nativa, visualmente igual às pílulas do NeuralIA. Hitboxes
/// independentes evitam que a WebView intercepte os botões.
pub(in crate::windows_app) unsafe extern "system" fn left_browser_toolbar_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    reference: usize,
) -> LRESULT {
    match message {
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_ERASEBKGND => 1,
        WM_CTLCOLOREDIT => {
            let theme = Theme::system();
            let hdc = wparam as *mut core::ffi::c_void;
            SetTextColor(hdc, rgb3(theme.fg));
            SetBkColor(hdc, rgb3(theme.surface));
            omnibox_brush(theme.surface) as LRESULT
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let theme = Theme::system();
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let width = (client.right - client.left).max(1) as f64;
                    let height = (client.bottom - client.top).max(1) as f64;
                    let scale = (height / LEFT_BROWSER_BAR_HEIGHT).max(1.0);
                    let background = CreateSolidBrush(rgb3(theme.bar_bg));
                    FillRect(hdc, &client, background);
                    DeleteObject(background as _);
                    fill_pill(
                        hdc,
                        UiRect {
                            x: 8.0 * scale,
                            y: 6.0 * scale,
                            width: (width - 100.0 * scale).max(1.0),
                            height: 36.0 * scale,
                        },
                        18.0 * scale,
                        theme.surface,
                        Some((theme.surface_line, scale)),
                        theme.bar_bg,
                    );
                    // Lupa desenhada vetorialmente (não depende de emoji/fonte).
                    let pen =
                        CreatePen(PS_SOLID, (2.0 * scale).round() as i32, rgb3(theme.fg_muted));
                    let old_pen = SelectObject(hdc, pen as _);
                    let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
                    let cx = (23.0 * scale).round() as i32;
                    let cy = (22.0 * scale).round() as i32;
                    let radius = (6.0 * scale).round() as i32;
                    Ellipse(hdc, cx - radius, cy - radius, cx + radius, cy + radius);
                    MoveToEx(hdc, cx + radius - 1, cy + radius - 1, std::ptr::null_mut());
                    LineTo(
                        hdc,
                        cx + radius + (4.0 * scale).round() as i32,
                        cy + radius + (4.0 * scale).round() as i32,
                    );
                    SelectObject(hdc, old_brush);
                    SelectObject(hdc, old_pen);
                    DeleteObject(pen as _);
                    let font = create_font((-16.0 * scale).round() as i32, FW_NORMAL as i32);
                    for (index, label) in ["−", "□"].iter().enumerate() {
                        draw_pill(
                            hdc,
                            UiRect {
                                x: width - (84.0 - index as f64 * 40.0) * scale,
                                y: 6.0 * scale,
                                width: 36.0 * scale,
                                height: 36.0 * scale,
                            },
                            label,
                            PillStyle::new(theme.surface, theme.surface_line, theme.fg),
                            scale,
                            font,
                            theme.bar_bg,
                        );
                    }
                    DeleteObject(font as _);
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        WM_LBUTTONUP => {
            if reference != 0 {
                let mut area = RECT::default();
                if GetClientRect(hwnd, &mut area) != 0 {
                    let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                    let width = area.right - area.left;
                    let scale =
                        ((area.bottom - area.top) as f64 / LEFT_BROWSER_BAR_HEIGHT).max(1.0);
                    let event = if x >= width - (44.0 * scale).round() as i32 {
                        Some(UserEvent::LeftBrowserExpand)
                    } else if x >= width - (84.0 * scale).round() as i32 {
                        Some(UserEvent::LeftBrowserMinimize)
                    } else {
                        None
                    };
                    if let Some(event) = event {
                        let proxy = &*(reference as *const EventLoopProxy<UserEvent>);
                        let _ = proxy.send_event(event);
                    }
                }
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
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
            .with_html(browser_new_tab_html())
            .with_new_window_req_handler(move |target, _features| {
                if let Some(event) = left_browser_new_window_event(target) {
                    let _ = new_window_proxy.send_event(event);
                }
                wry::NewWindowResponse::Deny
            });
        let hooked = self.hooked_builder(builder, WebViewHost::External, None);
        let view = match hooked.build_hooked_as_child(window) {
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
        self.left_browser_tabs.push(LeftBrowserTab {
            view,
            title: format!("Nova aba {}", index + 1),
        });
        self.left_browser_active = Some(index);
        self.left_browser_minimized = false;
        self.left_browser_expanded = false;
        if let Some(address) = self.left_browser_address {
            unsafe { SetWindowTextW(address, wide_null("").as_ptr()) };
        }
        self.hide_comparator_splitters();
        self.update_comparator_layout();
        if let Some(address) = self.left_browser_address {
            unsafe { SetFocus(address) };
        }
        self.request_redraw();
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
        self.update_comparator_layout();
        self.request_redraw();
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
        } else {
            let next = match self.left_browser_active {
                Some(active) if active > index => active - 1,
                Some(active) if active == index => index.min(self.left_browser_tabs.len() - 1),
                Some(active) => active,
                None => 0,
            };
            self.left_browser_active = Some(next);
            self.sync_left_browser_address();
            self.update_comparator_layout();
            self.request_redraw();
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
        self.update_comparator_layout();
        self.request_redraw();
    }

    pub(in crate::windows_app) fn toggle_left_browser_expanded(&mut self) {
        if self.left_browser_tabs.is_empty() {
            return;
        }
        self.left_browser_minimized = false;
        self.left_browser_expanded = !self.left_browser_expanded;
        self.update_comparator_layout();
        self.request_redraw();
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
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }
        const FIRST_BROWSER_MENU_ID: usize = 1000;
        for (index, tab) in self.left_browser_tabs.iter().enumerate() {
            let label = wide_null(&format!("{} — {}", index + 1, tab.title));
            unsafe {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    FIRST_BROWSER_MENU_ID + index,
                    label.as_ptr(),
                );
            }
        }
        let chosen = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                position.x,
                position.y,
                0,
                owner,
                std::ptr::null(),
            )
        } as usize;
        unsafe { DestroyMenu(menu) };
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
        let edit_width = (w - (144.0 * scale).round() as i32).max(1);
        unsafe {
            SetWindowPos(
                toolbar,
                std::ptr::null_mut(),
                0,
                (content_y * scale).round() as i32,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            SetWindowPos(
                address,
                std::ptr::null_mut(),
                (38.0 * scale).round() as i32,
                (10.0 * scale).round() as i32,
                edit_width,
                (28.0 * scale).round() as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let region = CreateRoundRectRgn(
                0,
                0,
                edit_width + 1,
                (28.0 * scale).round() as i32 + 1,
                (9.0 * scale).round() as i32,
                (9.0 * scale).round() as i32,
            );
            if !region.is_null() {
                SetWindowRgn(address, region, 1);
            }
            ShowWindow(toolbar, SW_SHOW);
            ShowWindow(address, SW_SHOW);
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
            for index in count..3 {
                minimized[index] = true;
            }
            let (browser_width, spans) =
                left_browser_content_spans(1200.0, &minimized, false, false);
            assert_eq!(spans.len(), count.saturating_sub(1));
            let sum: f64 = spans.iter().map(|(_, _, w)| *w).sum();
            assert!((browser_width + sum - 1200.0).abs() < 0.01);
            assert!(spans.iter().all(|(_, _, w)| *w > 0.0));
        }
        let (full, spans) = left_browser_content_spans(1200.0, &[false; 3], false, true);
        assert_eq!(full, 1200.0);
        assert!(spans.is_empty());
        let (hidden, spans) =
            left_browser_content_spans(1200.0, &[false, true, false], true, false);
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
}
