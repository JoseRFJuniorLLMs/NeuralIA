//! O aviso do canto (infra-notify-popups): UMA janela para todos os tipos
//! de `Notice`, generalizada do aviso do Gmail -- que passou a ser so um
//! `Notice` do tipo Gmail (`gmail_notice`), com o mesmo texto, tamanho,
//! botoes, prazo e respostas de antes.
//!
//! A janela e um popup OWNED pela janela principal (acima do WebView2 por
//! ser owned, nunca TOPMOST), `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`, nasce
//! invisivel e so aparece com `SW_SHOWNOACTIVATE`; o clique nao a ativa
//! (`MA_NOACTIVATE`). Nunca tira o foco a quem o tinha: gate real Win32
//! `toast_never_activates` (so CI).
//!
//! O que decide (fila, Foco, privacidade, token) e `crate::notify`; aqui so
//! se pinta, se posiciona e se faz o que ele devolve (`apply_notify`, com o
//! `App` como `ToastHost`).

use super::*;

use crate::notify::{Notice, NoticeKind, NoticeReply, NotifyCentre, ToastFrame, ToastHide};

pub(in crate::windows_app) const TOAST_SUBCLASS_ID: usize = 0x4E4D;

/// O aviso, em pixeis logicos: o do Gmail de sempre.
pub(in crate::windows_app) const TOAST_WIDTH: f64 = 390.0;
pub(in crate::windows_app) const TOAST_HEIGHT: f64 = 68.0;
/// Distancia ao canto inferior direito da janela.
pub(in crate::windows_app) const TOAST_MARGIN: f64 = 18.0;

/// O que o feature do aviso recebe pelo event loop (o padrao do `theme.rs`:
/// uma variante `UserEvent::Notify`, o resto aqui).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum NotifyEvent {
    /// Clique no botao `index` do aviso `token` (o que estava pintado).
    Answer { token: u64, index: usize },
    /// O prazo do aviso `token` passou.
    Hide(u64),
}

/// Um botao pintado: rotulo, largura logica, principal.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::windows_app) struct ToastButtonView {
    pub(in crate::windows_app) label: &'static str,
    pub(in crate::windows_app) width: f64,
    pub(in crate::windows_app) primary: bool,
}

/// O que a janela pinta. Fora do `App` porque quem pinta e o procedimento
/// de janela (como `SPLASH_TEXT`).
#[derive(Debug, Clone, PartialEq)]
pub(in crate::windows_app) struct ToastView {
    pub(in crate::windows_app) token: u64,
    pub(in crate::windows_app) title: String,
    pub(in crate::windows_app) body: String,
    pub(in crate::windows_app) buttons: Vec<ToastButtonView>,
    /// O corpo em ate duas linhas (os avisos dos downloads dizem porque);
    /// o do Gmail fica na linha de sempre, com reticencias.
    pub(in crate::windows_app) wrap: bool,
}

impl ToastView {
    pub(in crate::windows_app) fn of(frame: &ToastFrame) -> Self {
        Self {
            token: frame.token,
            title: frame.notice.title.clone(),
            body: frame.notice.body.clone(),
            buttons: frame
                .notice
                .actions
                .iter()
                .map(|action| ToastButtonView {
                    label: action.label,
                    width: action.width,
                    primary: action.primary,
                })
                .collect(),
            wrap: frame.notice.kind != NoticeKind::Gmail,
        }
    }
}

pub(in crate::windows_app) static TOAST_VIEW: Mutex<Option<ToastView>> = Mutex::new(None);

pub(in crate::windows_app) fn toast_scale(client: &RECT) -> f64 {
    ((client.bottom - client.top) as f64 / TOAST_HEIGHT).max(1.0)
}

/// Os botoes no canto direito do aviso, em pixeis do cliente, pela ordem das
/// accoes: o ultimo encostado a direita, os outros a esquerda dele. Com as
/// larguras do Gmail (70, 54) sao exatamente o "Abrir" e o "Nao" de sempre.
pub(in crate::windows_app) fn toast_buttons(
    client: &RECT,
    scale: f64,
    widths: &[f64],
) -> Vec<RECT> {
    let height = (26.0 * scale).round() as i32;
    let top = (client.bottom - height) / 2;
    let gap = (6.0 * scale).round() as i32;
    let mut right = client.right - (12.0 * scale).round() as i32;
    let mut rects: Vec<RECT> = widths
        .iter()
        .rev()
        .map(|width| {
            let rect = RECT {
                left: right - (width * scale).round() as i32,
                top,
                right,
                bottom: top + height,
            };
            right = rect.left - gap;
            rect
        })
        .collect();
    rects.reverse();
    rects
}

/// O botao debaixo de (x, y); bordas semiabertas, como o resto da UI.
pub(in crate::windows_app) fn toast_hit(buttons: &[RECT], x: i32, y: i32) -> Option<usize> {
    buttons
        .iter()
        .position(|rect| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
}

/// Canto superior esquerdo do aviso, em coordenadas de ecra: encostado ao
/// canto inferior direito do cliente da janela (`origin` e o canto do
/// cliente no ecra), a `TOAST_MARGIN` dele.
pub(in crate::windows_app) fn toast_origin(
    origin: POINT,
    client: &RECT,
    width: i32,
    height: i32,
    scale: f64,
) -> (i32, i32) {
    let margin = (TOAST_MARGIN * scale) as i32;
    (
        origin.x + client.right - width - margin,
        origin.y + client.bottom - height - margin,
    )
}

pub(in crate::windows_app) unsafe extern "system" fn toast_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    // Os botoes recebem cliques (um STATIC devolve HTTRANSPARENT) e clicar
    // no aviso nao o ativa: o foco fica onde estava.
    if let Some(result) = popup_no_activate_message(message) {
        return result;
    }
    match message {
        WM_LBUTTONUP => {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 && reference_data != 0 {
                let view = TOAST_VIEW.lock().ok().and_then(|view| view.clone());
                if let Some(view) = view {
                    let widths: Vec<f64> = view.buttons.iter().map(|b| b.width).collect();
                    let buttons = toast_buttons(&client, toast_scale(&client), &widths);
                    let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                    let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
                    if let Some(index) = toast_hit(&buttons, x, y) {
                        let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                        let _ = proxy.send_event(UserEvent::Notify(NotifyEvent::Answer {
                            token: view.token,
                            index,
                        }));
                    }
                }
            }
            0
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let view = TOAST_VIEW.lock().ok().and_then(|view| view.clone());
                    paint_toast(hdc, &client, view.as_ref());
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

/// Titulo na cor de destaque, corpo numa linha com reticencias, os botoes
/// como pilulas a direita (o principal cheio).
unsafe fn paint_toast(hdc: *mut core::ffi::c_void, client: &RECT, view: Option<&ToastView>) {
    let theme = Theme::system();
    let background = CreateSolidBrush(rgb3(theme.surface));
    FillRect(hdc, client, background);
    DeleteObject(background as _);
    let Some(view) = view else {
        return;
    };

    let scale = toast_scale(client);
    let title_font = create_font((-13.0 * scale) as i32, FW_BOLD as i32);
    let body_font = create_font((-12.0 * scale) as i32, FW_NORMAL as i32);
    let widths: Vec<f64> = view.buttons.iter().map(|button| button.width).collect();
    let buttons = toast_buttons(client, scale, &widths);
    let buttons_left = buttons
        .first()
        .map_or(client.right - (16.0 * scale) as i32, |first| {
            first.left - (8.0 * scale) as i32
        });
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
        &view.title,
        &mut title,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
    );

    SelectObject(hdc, body_font as _);
    SetTextColor(hdc, rgb3(theme.fg));
    let mut body = RECT {
        left: (16.0 * scale) as i32,
        top: (28.0 * scale) as i32,
        right: buttons_left,
        bottom: client.bottom - (7.0 * scale) as i32,
    };
    let format = if view.wrap {
        DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS | DT_NOPREFIX
    } else {
        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX
    };
    draw_text(hdc, &view.body, &mut body, format);

    for (rect, button) in buttons.iter().zip(&view.buttons) {
        let pill = UiRect {
            x: rect.left as f64,
            y: rect.top as f64,
            width: (rect.right - rect.left) as f64,
            height: (rect.bottom - rect.top) as f64,
        };
        let style = if button.primary {
            PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
        } else {
            PillStyle::new(theme.surface_line, theme.surface_line, theme.fg)
        };
        draw_pill(
            hdc,
            pill,
            button.label,
            style,
            scale,
            body_font,
            theme.surface,
        );
    }

    SelectObject(hdc, old_font);
    DeleteObject(title_font as _);
    DeleteObject(body_font as _);
}

/// A receita de criacao do aviso: invisivel, sem ativacao, owned por
/// `owner` e nunca TOPMOST -- um aviso nosso nao tem nada que tapar a
/// aplicacao de outra pessoa. `proxy` e o `EventLoopProxy<UserEvent>` que
/// recebe os cliques (0: nenhum).
pub(in crate::windows_app) unsafe fn create_toast_window(
    owner: HWND,
    width: i32,
    height: i32,
    proxy: usize,
) -> Option<HWND> {
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
        return None;
    }
    if SetWindowSubclass(created, Some(toast_subclass), TOAST_SUBCLASS_ID, proxy) == 0 {
        DestroyWindow(created);
        return None;
    }
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 18, 18);
    if !region.is_null() {
        SetWindowRgn(created, region, 1);
    }
    Some(created)
}

/// Poe o aviso no sitio e mostra-o sem o ativar.
pub(in crate::windows_app) unsafe fn place_toast(
    toast: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    SetWindowPos(
        toast,
        std::ptr::null_mut(),
        x,
        y,
        width,
        height,
        SWP_NOACTIVATE,
    );
    show_popup_without_activation(toast);
    InvalidateRect(toast, std::ptr::null(), 1);
}

/// Para onde um botao do aviso leva, fora dele.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum NoticeTarget {
    Service(Service),
    /// A seccao Downloads do painel (downloads-ui).
    Downloads,
}

/// O que um botao do aviso faz fora dele: abrir um servico, a seccao
/// Downloads, ou nada.
pub(in crate::windows_app) fn notice_target(
    kind: NoticeKind,
    reply: NoticeReply,
) -> Option<NoticeTarget> {
    match (kind, reply) {
        (NoticeKind::Gmail, NoticeReply::Open) => Some(NoticeTarget::Service(Service::Gmail)),
        (NoticeKind::Download, NoticeReply::Open) => Some(NoticeTarget::Downloads),
        _ => None,
    }
}

/// Quem executa o aviso: o `App` no produto, um registo nos gates.
pub(in crate::windows_app) trait ToastHost {
    fn notify_centre(&mut self) -> &mut NotifyCentre;
    /// Pinta `frame` (criando a janela, se preciso) e poe-no a vista.
    fn show_toast(&mut self, frame: &ToastFrame);
    /// Esconde a janela sem a destruir (a resposta a um botao).
    fn hide_toast_window(&mut self);
    /// Destroi a janela (o prazo passou).
    fn destroy_toast(&mut self);
    fn hide_toast_after(&mut self, token: u64, delay: Duration);
    fn open_service(&mut self, service: Service);
    /// O «Ver» de um download: a seccao Downloads do painel.
    fn open_downloads(&mut self);
}

/// O que entra no aviso.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::windows_app) enum NotifyInput {
    Post(Notice),
    Event(NotifyEvent),
}

fn present(host: &mut impl ToastHost, frame: ToastFrame) {
    host.show_toast(&frame);
    host.hide_toast_after(frame.token, frame.notice.ttl);
}

/// O caminho inteiro do aviso: o centro decide, o host faz.
pub(in crate::windows_app) fn apply_notify(host: &mut impl ToastHost, input: NotifyInput) {
    match input {
        NotifyInput::Post(notice) => {
            if let Some(frame) = host.notify_centre().post(notice) {
                present(host, frame);
            }
        }
        NotifyInput::Event(NotifyEvent::Hide(token)) => {
            if let ToastHide::Hide { next } = host.notify_centre().hidden(token) {
                host.destroy_toast();
                if let Some(frame) = next {
                    present(host, frame);
                }
            }
        }
        NotifyInput::Event(NotifyEvent::Answer { token, index }) => {
            if let Some((kind, reply)) = host.notify_centre().answer(token, index) {
                host.hide_toast_window();
                match notice_target(kind, reply) {
                    Some(NoticeTarget::Service(service)) => host.open_service(service),
                    Some(NoticeTarget::Downloads) => host.open_downloads(),
                    None => {}
                }
            }
        }
    }
}

impl ToastHost for App {
    fn notify_centre(&mut self) -> &mut NotifyCentre {
        &mut self.notify
    }

    fn show_toast(&mut self, frame: &ToastFrame) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (TOAST_WIDTH * scale).round() as i32;
        let height = (TOAST_HEIGHT * scale).round() as i32;
        if let Ok(mut view) = TOAST_VIEW.lock() {
            *view = Some(ToastView::of(frame));
        }
        if self.toast.is_none() {
            let proxy = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
            let Some(created) = (unsafe { create_toast_window(owner, width, height, proxy) })
            else {
                return;
            };
            self.toast = Some(created);
        }
        self.position_toast();
    }

    fn hide_toast_window(&mut self) {
        if let Some(toast) = self.toast {
            unsafe {
                ShowWindow(toast, SW_HIDE);
            }
        }
    }

    fn destroy_toast(&mut self) {
        if let Some(toast) = self.toast.take() {
            unsafe {
                DestroyWindow(toast);
            }
        }
    }

    fn hide_toast_after(&mut self, token: u64, delay: Duration) {
        self.timers
            .after(delay, UserEvent::Notify(NotifyEvent::Hide(token)));
    }

    fn open_service(&mut self, service: Service) {
        self.open_service_panel(service);
    }

    fn open_downloads(&mut self) {
        self.show_downloads_panel();
    }
}

impl App {
    /// O unico braco do aviso no `user_event`.
    pub(in crate::windows_app) fn notify_event(&mut self, event: NotifyEvent) {
        apply_notify(self, NotifyInput::Event(event));
    }

    /// Um aviso novo para o canto (ou para a fila).
    pub(in crate::windows_app) fn notify(&mut self, notice: Notice) {
        apply_notify(self, NotifyInput::Post(notice));
    }

    /// Encosta o aviso ao canto inferior direito da janela. Vive em
    /// coordenadas de ECRA, como o splash: refaz-se quando a janela se mexe.
    pub(in crate::windows_app) fn position_toast(&self) {
        let (Some(window), Some(toast)) = (&self.window, self.toast) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (TOAST_WIDTH * scale).round() as i32;
        let height = (TOAST_HEIGHT * scale).round() as i32;

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let (x, y) = toast_origin(origin, &client, width, height, scale);
            place_toast(toast, x, y, width, height);
        }
    }
}
