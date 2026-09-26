use std::{ffi::OsString, sync::atomic::Ordering, time::Duration};

use winit::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, PermissionResponse};

use crate::ipc::{IpcAction, parse_ipc_message};
use crate::notify::{Notice, NoticeAction, NoticeKind, NoticeReply};
use crate::windows_app::{
    App, COMPARATOR_COLUMNS, UserEvent, WebViewHost,
    page_scripts::GMAIL_MONITOR_SCRIPT,
    remote_capability,
    services::{GMAIL_NOTIFICATIONS, gmail_field, gmail_is_new_mail, save_gmail_setting},
    theme::themed_webview_builder,
    toast::ToastHost,
};

/// Quanto tempo o aviso de correio novo fica no canto.
/// Com a pergunta "Abrir?" o aviso fica mais tempo a vista.
pub(in crate::windows_app) const GMAIL_TOAST_SECONDS: u64 = 12;

/// O titulo do aviso de correio novo, escrito pelo nativo.
pub(in crate::windows_app) const GMAIL_TOAST_TITLE: &str = "Gmail · novo e-mail — abrir?";

/// O aviso de correio novo como `Notice` do centro de avisos: o titulo, o
/// corpo (remetente e assunto, ou o que houver), "Abrir" e "Nao" com as
/// larguras de sempre e 12 s no canto. O corpo traz conteudo do e-mail:
/// no modo privado o centro troca-o pela linha neutra.
pub(in crate::windows_app) fn gmail_notice(sender: &str, subject: &str) -> Notice {
    let body = match (sender.trim(), subject.trim()) {
        ("", "") => "Nova mensagem na sua caixa de entrada".to_string(),
        ("", subject) => subject.to_string(),
        (sender, "") => sender.to_string(),
        (sender, subject) => format!("{sender} · {subject}"),
    };
    Notice {
        kind: NoticeKind::Gmail,
        title: GMAIL_TOAST_TITLE.to_string(),
        body,
        actions: vec![
            NoticeAction {
                label: "Abrir",
                width: 70.0,
                primary: true,
                reply: NoticeReply::Open,
            },
            NoticeAction {
                label: "Não",
                width: 54.0,
                primary: false,
                reply: NoticeReply::Dismiss,
            },
        ],
        ttl: Duration::from_secs(GMAIL_TOAST_SECONDS),
        content_bearing: true,
    }
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
    /// Correio novo: um `Notice` do Gmail para o canto (`toast.rs`).
    pub(in crate::windows_app) fn show_gmail_toast(&mut self, sender: &str, subject: &str) {
        self.notify(gmail_notice(sender, subject));
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

        // A trava de navegacao (NavGate::Gmail: so o Gmail e o login da
        // Google, em https) vem de `hooked_builder`, como em todas as
        // WebViews; a tabela tambem recusa downloads deste monitor, que
        // ninguem ve.
        let result = self
            .hooked_builder(
                themed_webview_builder()
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
                    .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                    .with_permission_handler(|_| PermissionResponse::Deny)
                    .with_focused(false)
                    .with_bounds(bounds)
                    .with_url("https://mail.google.com/mail/u/0/#inbox"),
                WebViewHost::GmailMonitor,
                None,
            )
            .build_as_child(window);

        if let Ok(webview) = result {
            self.install_webview_hooks(&webview, WebViewHost::GmailMonitor);
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
            // O aviso do Gmail a vista sai com eles.
            if self.notify.current_kind() == Some(NoticeKind::Gmail) {
                self.hide_toast_window();
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
}
