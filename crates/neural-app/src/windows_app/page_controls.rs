use super::*;

const PAGE_CONTROLS_SUBCLASS: usize = 0x50414745;
static PAGE_PRESSED: AtomicUsize = AtomicUsize::new(NATIVE_BUTTON_NONE);
static PAGE_HOVER: AtomicUsize = AtomicUsize::new(NATIVE_BUTTON_NONE);

pub(in crate::windows_app) fn page_controls_wanted(surface: Surface) -> bool {
    matches!(
        surface,
        Surface::External | Surface::Reader | Surface::Pdf | Surface::Epub
    )
}

fn page_control_release(
    pressed: Option<usize>,
    captured: bool,
    width: i32,
    height: i32,
    x: i32,
    y: i32,
) -> Option<UserEvent> {
    match native_caption_release(pressed, captured, width, height, x, y)? {
        0 => Some(UserEvent::MinimizeActivePage(CommandOrigin::Window)),
        1 => Some(UserEvent::ToggleColumnFullscreen),
        2 => Some(UserEvent::HomeRequested),
        _ => None,
    }
}

fn page_control_hint(index: usize) -> &'static str {
    match index {
        0 => "Minimizar a página · restaurar pela barra de tarefas (Ctrl+Shift+M)",
        1 => "Entrar ou sair da tela cheia (F11)",
        2 => "Fechar a página e voltar à Home (Alt+Home)",
        _ => "",
    }
}

unsafe extern "system" fn page_controls_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    reference: usize,
) -> LRESULT {
    // Native controls remain reachable even when remote CSS fills the page.
    unsafe {
        match message {
            WM_NCHITTEST => HTCLIENT as LRESULT,
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut paint);
                if !hdc.is_null() {
                    let mut area = RECT::default();
                    if GetClientRect(hwnd, &mut area) != 0 {
                        let width = area.right.max(1) as f64;
                        let height = area.bottom.max(1) as f64;
                        let scale = (height / TITLE_TAB_HEIGHT).max(1.0);
                        let font = create_font((-13.0 * scale) as i32, FW_NORMAL as i32);
                        let theme = Theme::system();
                        for (index, label) in ["−", "⛶", "×"].into_iter().enumerate() {
                            draw_pill(
                                hdc,
                                UiRect {
                                    x: index as f64 * width / 3.0,
                                    y: 0.0,
                                    width: width / 3.0,
                                    height,
                                },
                                label,
                                caption_button_style(
                                    index,
                                    PAGE_HOVER.load(Ordering::Acquire) == index,
                                    &theme,
                                ),
                                scale,
                                font,
                                theme.page_bg,
                            );
                        }
                        DeleteObject(font as _);
                    }
                    EndPaint(hwnd, &paint);
                }
                0
            }
            WM_LBUTTONDOWN => {
                let mut area = RECT::default();
                if GetClientRect(hwnd, &mut area) != 0 {
                    let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                    if let Some(index) = native_button_index(area.right, x) {
                        PAGE_PRESSED.store(index, Ordering::Release);
                        hover_tooltip(hwnd, "");
                        SetCapture(hwnd);
                    }
                }
                0
            }
            WM_LBUTTONUP => {
                let captured = GetCapture() == hwnd;
                let pressed = take_native_pressed_button(&PAGE_PRESSED, || {
                    if captured {
                        ReleaseCapture();
                    }
                });
                let mut area = RECT::default();
                if GetClientRect(hwnd, &mut area) != 0 && reference != 0 {
                    let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                    let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
                    if let Some(event) =
                        page_control_release(pressed, captured, area.right, area.bottom, x, y)
                    {
                        let proxy = &*(reference as *const EventLoopProxy<UserEvent>);
                        let _ = proxy.send_event(event);
                    }
                }
                0
            }
            WM_MOUSEMOVE => {
                let mut area = RECT::default();
                if GetClientRect(hwnd, &mut area) != 0 {
                    let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                    let index = native_button_index(area.right, x);
                    let last =
                        PAGE_HOVER.swap(index.unwrap_or(NATIVE_BUTTON_NONE), Ordering::AcqRel);
                    if index != Some(last) {
                        track_mouse_leave(hwnd);
                        hover_tooltip(hwnd, index.map_or("", page_control_hint));
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            WM_MOUSELEAVE => {
                PAGE_HOVER.store(NATIVE_BUTTON_NONE, Ordering::Release);
                hover_tooltip(hwnd, "");
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            WM_CAPTURECHANGED | WM_CANCELMODE => {
                PAGE_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
                0
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

impl App {
    pub(in crate::windows_app) fn sync_page_controls(&mut self) {
        if !page_controls_wanted(self.surface) {
            if let Some(controls) = self.page_controls.take() {
                unsafe {
                    DestroyWindow(controls);
                }
            }
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let area = home_caption_rect(window.inner_size().width as f64, window.scale_factor());
        if self
            .page_controls
            .is_some_and(|controls| unsafe { GetParent(controls) != owner })
            && let Some(controls) = self.page_controls.take()
        {
            unsafe {
                DestroyWindow(controls);
            }
        }
        if self.page_controls.is_none() {
            let controls = unsafe {
                CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("NeuralIA.PageControls"),
                    WS_CHILD,
                    0,
                    0,
                    area.width.round() as i32,
                    area.height.round() as i32,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            if controls.is_null() {
                return;
            }
            let proxy = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
            if unsafe {
                SetWindowSubclass(
                    controls,
                    Some(page_controls_subclass),
                    PAGE_CONTROLS_SUBCLASS,
                    proxy,
                )
            } == 0
            {
                unsafe {
                    DestroyWindow(controls);
                }
                return;
            }
            self.page_controls = Some(controls);
        }
        if let Some(controls) = self.page_controls {
            place_caption_buttons(
                controls,
                area.x.round() as i32,
                area.width.round() as i32,
                area.height.round() as i32,
                true,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_buttons_route_only_complete_clicks_to_native_actions() {
        for (index, x) in [23, 69, 115].into_iter().enumerate() {
            let event = page_control_release(Some(index), true, 138, 36, x, 18).unwrap();
            match index {
                0 => assert!(matches!(
                    event,
                    UserEvent::MinimizeActivePage(CommandOrigin::Window)
                )),
                1 => assert!(matches!(event, UserEvent::ToggleColumnFullscreen)),
                _ => assert!(matches!(event, UserEvent::HomeRequested)),
            }
            for (captured, release_x, release_y) in [
                (false, x, 18),
                (true, -1, 18),
                (true, 138, 18),
                (true, x, -1),
                (true, x, 36),
            ] {
                assert!(
                    page_control_release(Some(index), captured, 138, 36, release_x, release_y)
                        .is_none()
                );
            }
            assert!(page_control_release(Some((index + 1) % 3), true, 138, 36, x, 18).is_none());
            assert!(page_control_release(None, true, 138, 36, x, 18).is_none());
        }
    }

    #[test]
    fn standalone_pages_have_controls_and_matching_shortcuts() {
        for surface in [
            Surface::External,
            Surface::Reader,
            Surface::Pdf,
            Surface::Epub,
        ] {
            assert!(page_controls_wanted(surface));
        }
        for surface in [Surface::Home, Surface::Comparator] {
            assert!(!page_controls_wanted(surface));
        }
        for (index, chord, key) in [
            (0, Chord::ctrl_shift(b'M'), CommandId::MinimizePage),
            (1, Chord::key(0x7a), CommandId::SplitFullscreen),
            (2, Chord::alt(0x24), CommandId::Home),
        ] {
            assert!(
                page_control_hint(index).contains('(') || page_control_hint(index).contains('·')
            );
            let input = AcceleratorInput {
                vk: chord.vk,
                ctrl: chord.ctrl,
                shift: chord.shift,
                alt: chord.alt,
                down: true,
                repeat: false,
            };
            for origin in [
                CommandOrigin::Window,
                CommandOrigin::Host(WebViewHost::External),
            ] {
                let event = keymap_decision(input, origin).event;
                assert!(
                    matches!(event, Some(UserEvent::RunCommandKey { key: actual, .. }) if actual == key),
                    "{key:?} em {origin:?}"
                );
            }
        }
    }
}
