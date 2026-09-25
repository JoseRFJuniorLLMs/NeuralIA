use std::{
    ffi::OsString,
    sync::{Mutex, atomic::Ordering},
    time::Duration,
};

use url::Url;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, ClientToScreen, CreateRoundRectRgn, CreateSolidBrush, DT_END_ELLIPSIS,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, EndPaint, FW_BOLD, FW_NORMAL,
        FillRect, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
        TRANSPARENT,
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetClientRect, HTCLIENT, SW_HIDE, SWP_NOACTIVATE,
        SetWindowPos, ShowWindow, WM_LBUTTONUP, WM_NCHITTEST, WM_PAINT,
    },
};
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event_loop::EventLoopProxy,
};
use wry::{NewWindowResponse, PermissionResponse};

use crate::ipc::{IpcAction, parse_ipc_message};
use crate::windows_app::{
    AUX_POPUP_EX_STYLE, AUX_POPUP_STYLE, App, COMPARATOR_COLUMNS, UiRect, UserEvent, create_font,
    icons::{PillStyle, draw_pill, draw_text, rgb3},
    native::{DefSubclassProc, SetWindowSubclass, window_hwnd},
    page_scripts::GMAIL_MONITOR_SCRIPT,
    remote_capability,
    search_card::{gmail_toast_buttons, show_popup_without_activation},
    services::{GMAIL_NOTIFICATIONS, Service, gmail_field, gmail_is_new_mail, save_gmail_setting},
    theme::{Theme, on_color, themed_webview_builder},
};

/// Quanto tempo o aviso de correio novo fica no canto.
/// Com a pergunta "Abrir?" o aviso fica mais tempo a vista.
pub(in crate::windows_app) const GMAIL_TOAST_SECONDS: u64 = 12;

pub(in crate::windows_app) const GMAIL_TOAST_SUBCLASS_ID: usize = 0x4E4D;

pub(in crate::windows_app) const GMAIL_TOAST_WIDTH: f64 = 390.0;
pub(in crate::windows_app) const GMAIL_TOAST_HEIGHT: f64 = 68.0;

pub(in crate::windows_app) static GMAIL_TOAST_TEXT: Mutex<String> = Mutex::new(String::new());

pub(in crate::windows_app) unsafe extern "system" fn gmail_toast_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    // A pergunta recebe cliques: um STATIC devolve HTTRANSPARENT.
    if message == WM_NCHITTEST {
        return HTCLIENT as LRESULT;
    }
    if message == WM_LBUTTONUP && reference_data != 0 {
        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client) != 0 {
            let scale = ((client.bottom - client.top) as f64 / GMAIL_TOAST_HEIGHT).max(1.0);
            let (open, no) = gmail_toast_buttons(&client, scale);
            let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
            let inside =
                |rect: &RECT| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
            let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
            if inside(&open) {
                let _ = proxy.send_event(UserEvent::GmailAnswer(true));
            } else if inside(&no) {
                let _ = proxy.send_event(UserEvent::GmailAnswer(false));
            }
        }
        return 0;
    }
    if message == WM_PAINT {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        if !hdc.is_null() {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let theme = Theme::system();
                let background = CreateSolidBrush(rgb3(theme.surface));
                FillRect(hdc, &client, background);
                DeleteObject(background as _);

                let scale = ((client.bottom - client.top) as f64 / GMAIL_TOAST_HEIGHT).max(1.0);
                let title_font = create_font((-13.0 * scale) as i32, FW_BOLD as i32);
                let body_font = create_font((-12.0 * scale) as i32, FW_NORMAL as i32);
                let (open_button, no_button) = gmail_toast_buttons(&client, scale);
                let buttons_left = open_button.left - (8.0 * scale) as i32;
                let old_font = SelectObject(hdc, title_font as _);
                SetBkMode(hdc, TRANSPARENT as i32);

                SetTextColor(hdc, rgb3(theme.accent));
                let mut title = RECT {
                    left: (16.0 * scale) as i32,
                    top: (7.0 * scale) as i32,
                    right: buttons_left,
                    bottom: (28.0 * scale) as i32,
                };
                draw_text(
                    hdc,
                    "Gmail · novo e-mail — abrir?",
                    &mut title,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );

                SelectObject(hdc, body_font as _);
                SetTextColor(hdc, rgb3(theme.fg));
                let text = GMAIL_TOAST_TEXT
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let mut body = RECT {
                    left: (16.0 * scale) as i32,
                    top: (28.0 * scale) as i32,
                    right: buttons_left,
                    bottom: client.bottom - (7.0 * scale) as i32,
                };
                draw_text(
                    hdc,
                    &text,
                    &mut body,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );

                for (rect, label, primary) in
                    [(open_button, "Abrir", true), (no_button, "Não", false)]
                {
                    let pill = UiRect {
                        x: rect.left as f64,
                        y: rect.top as f64,
                        width: (rect.right - rect.left) as f64,
                        height: (rect.bottom - rect.top) as f64,
                    };
                    let style = if primary {
                        PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
                    } else {
                        PillStyle::new(theme.surface_line, theme.surface_line, theme.fg)
                    };
                    draw_pill(hdc, pill, label, style, scale, body_font, theme.surface);
                }

                SelectObject(hdc, old_font);
                DeleteObject(title_font as _);
                DeleteObject(body_font as _);
            }
            EndPaint(hwnd, &paint);
        }
        return 0;
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

/// `NEURALIA_NO_GMAIL` desliga o monitor do Gmail por completo (SPEC-0005,
/// SECURITY.md): sem sonda, sem leitura de cookies, sem WebView escondido. O
/// gate de ciclo de vida define-a porque conta processos e nao distingue a
/// excepcao intencional de um vazamento.
pub(in crate::windows_app) fn gmail_monitor_enabled() -> bool {
    gmail_monitor_enabled_for(std::env::var_os("NEURALIA_NO_GMAIL"))
        && GMAIL_NOTIFICATIONS.load(Ordering::Acquire)
}

/// Basta a variavel EXISTIR, como em NEURALIA_REDUCE_MOTION: `=0` ou vazia
/// tambem desligam. Um interruptor de privacidade que dependesse do valor
/// deixava passar quem o definiu mal.
pub(in crate::windows_app) fn gmail_monitor_enabled_for(no_gmail: Option<OsString>) -> bool {
    no_gmail.is_none()
}

impl App {
    pub(in crate::windows_app) fn show_gmail_toast(&mut self, sender: &str, subject: &str) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (GMAIL_TOAST_WIDTH * scale).round() as i32;
        let height = (GMAIL_TOAST_HEIGHT * scale).round() as i32;

        let body = match (sender.trim(), subject.trim()) {
            ("", "") => "Nova mensagem na sua caixa de entrada".to_string(),
            ("", subject) => subject.to_string(),
            (sender, "") => sender.to_string(),
            (sender, subject) => format!("{sender} · {subject}"),
        };
        if let Ok(mut slot) = GMAIL_TOAST_TEXT.lock() {
            *slot = body;
        }

        if self.gmail_toast.is_none() {
            unsafe {
                // Owned pela janela principal, como o splash: sobe acima do
                // WebView2 por ser owned, e nao acima do resto do ambiente de
                // trabalho -- um aviso de email nosso nao tem nada que tapar a
                // aplicacao de outra pessoa.
                let created = CreateWindowExW(
                    AUX_POPUP_EX_STYLE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    AUX_POPUP_STYLE,
                    0,
                    0,
                    width,
                    height,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if created.is_null() {
                    return;
                }
                if SetWindowSubclass(
                    created,
                    Some(gmail_toast_subclass),
                    GMAIL_TOAST_SUBCLASS_ID,
                    (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 18, 18);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.gmail_toast = Some(created);
            }
        }

        self.position_gmail_toast();

        self.gmail_toast_token = self.gmail_toast_token.wrapping_add(1);
        self.timers.after(
            Duration::from_secs(GMAIL_TOAST_SECONDS),
            UserEvent::HideGmailToast(self.gmail_toast_token),
        );
    }

    /// Encosta o aviso ao canto inferior direito da janela. Como o splash,
    /// tem de ser refeito sempre que a janela se mexe.
    pub(in crate::windows_app) fn position_gmail_toast(&self) {
        let (Some(window), Some(toast)) = (&self.window, self.gmail_toast) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (GMAIL_TOAST_WIDTH * scale).round() as i32;
        let height = (GMAIL_TOAST_HEIGHT * scale).round() as i32;

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let margin = (18.0 * scale) as i32;
            SetWindowPos(
                toast,
                std::ptr::null_mut(),
                origin.x + client.right - width - margin,
                origin.y + client.bottom - height - margin,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(toast);
            InvalidateRect(toast, std::ptr::null(), 1);
        }
    }

    pub(in crate::windows_app) fn hide_gmail_toast(&mut self, token: u64) {
        if token != self.gmail_toast_token {
            return;
        }
        if let Some(toast) = self.gmail_toast.take() {
            unsafe {
                DestroyWindow(toast);
            }
        }
    }

    pub(in crate::windows_app) fn google_session_available(&self) -> bool {
        let source = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.first().map(|view| &view.webview))
            .or(self.webview.as_ref());
        let Some(source) = source else {
            return false;
        };

        source
            .cookies_for_url("https://mail.google.com/")
            .ok()
            .is_some_and(|cookies| {
                cookies.iter().any(|cookie| {
                    matches!(
                        cookie.name(),
                        "SID" | "HSID" | "SSID" | "SAPISID" | "__Secure-1PSID" | "__Secure-3PSID"
                    )
                })
            })
    }

    pub(in crate::windows_app) fn schedule_gmail_probe(&mut self, seconds: u64) {
        // Desligado por NEURALIA_NO_GMAIL nem se sonda: a sonda le cookies e
        // acabaria por criar o WebView que a variavel promete nao existir.
        if self.gmail_monitor.is_some() || !gmail_monitor_enabled() {
            return;
        }
        self.gmail_probe_token = self.gmail_probe_token.wrapping_add(1);
        self.timers.after(
            Duration::from_secs(seconds),
            UserEvent::GmailProbe(self.gmail_probe_token),
        );
    }

    pub(in crate::windows_app) fn maybe_start_gmail_monitor(&mut self) {
        if self.gmail_monitor.is_some()
            || !gmail_monitor_enabled()
            || !self.google_session_available()
        {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };

        let capability = remote_capability();
        let ipc_capability = capability.clone();
        let proxy = self.proxy.clone();
        let init_script = GMAIL_MONITOR_SCRIPT.replace("__NEURALIA_CAP__", &capability);
        let bounds = wry::Rect {
            position: LogicalPosition::new(-10_000.0, -10_000.0).into(),
            size: LogicalSize::new(1.0, 1.0).into(),
        };

        let result = themed_webview_builder()
            .with_initialization_script(init_script)
            .with_ipc_handler(move |request| {
                let Some(IpcAction::GmailState {
                    unread,
                    sender,
                    subject,
                    key,
                }) = parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                let _ = proxy.send_event(UserEvent::GmailInboxState {
                    unread,
                    sender,
                    subject,
                    key,
                });
            })
            .with_navigation_handler(move |target| {
                if target
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("neuralia:"))
                {
                    return false;
                }
                Url::parse(&target).ok().is_some_and(|url| {
                    url.scheme() == "https"
                        && matches!(
                            url.host_str(),
                            Some("mail.google.com") | Some("accounts.google.com")
                        )
                })
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_focused(false)
            .with_bounds(bounds)
            .with_url("https://mail.google.com/mail/u/0/#inbox")
            .build_as_child(window);

        if let Ok(webview) = result {
            self.gmail_monitor = Some(webview);
        }
    }

    pub(in crate::windows_app) fn handle_gmail_state(
        &mut self,
        unread: u32,
        sender: String,
        subject: String,
        key: String,
    ) {
        // O corte do script nao conta: ele corre em mail.google.com.
        let sender = gmail_field(sender);
        let subject = gmail_field(subject);
        let key = gmail_field(key);
        let notify = gmail_is_new_mail(
            self.gmail_last_unread,
            self.gmail_last_key.as_deref(),
            unread,
            &key,
        );
        self.gmail_last_unread = Some(unread);
        self.gmail_last_key = Some(key);
        if notify {
            self.show_gmail_toast(&sender, &subject);
        }
    }

    /// O envelope da barra: liga e desliga os avisos do Gmail, e guarda.
    pub(in crate::windows_app) fn toggle_gmail_notifications(&mut self) {
        let on = !GMAIL_NOTIFICATIONS.load(Ordering::Acquire);
        GMAIL_NOTIFICATIONS.store(on, Ordering::Release);
        if let Err(error) = save_gmail_setting(&self.config.data_dir.join("gmail"), on) {
            self.show_native_error(format!("Não foi possível guardar a escolha: {error}"));
        }
        if on {
            self.schedule_gmail_probe(1);
        } else {
            // Desligar e mesmo desligar: sem WebView escondida a ler o Gmail.
            self.gmail_monitor = None;
            if let Some(toast) = self.gmail_toast {
                unsafe {
                    ShowWindow(toast, SW_HIDE);
                }
            }
        }
        self.show_splash(
            if on {
                "Avisos do Gmail ligados.".to_string()
            } else {
                "Avisos do Gmail desligados.".to_string()
            },
            2,
        );
        self.request_redraw();
    }

    /// Resposta ao "Abrir?" do aviso do Gmail.
    pub(in crate::windows_app) fn answer_gmail(&mut self, open: bool) {
        if let Some(toast) = self.gmail_toast {
            unsafe {
                ShowWindow(toast, SW_HIDE);
            }
        }
        if open {
            self.open_service_panel(Service::Gmail);
        }
    }
}
