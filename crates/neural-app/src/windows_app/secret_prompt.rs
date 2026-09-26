use super::*;

use std::cell::RefCell;

use crate::secrets::{API_KEY_MAX_CHARS, ApiKey, KeySlot, KeyVault, validate_api_key, wipe};
use crate::stores::{KEYS_STORE, LIVE_KEY_STORE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_PUSHBUTTON, ES_PASSWORD, FindWindowExW, GetWindowRect, SWP_NOSIZE,
    SetWindowTextW, WM_COMMAND, WM_DESTROY, WS_BORDER, WS_CLIPCHILDREN,
};

// ===================== pedido de chave (SecretPrompt) =====================
//
// Modulo de feature (o padrao de `theme.rs`): `UserEvent::Keys(KeyEvent)`, o
// campo `App::keys` e o braco `keys_event` no event loop. O pedido de chave
// e um popup NATIVO com um EDIT `ES_PASSWORD`: a chave nunca entra numa
// WebView, num URL ou num log. E o unico popup ativavel alem do EDIT da
// palette -- recebe teclado de proposito -- por isso e owned pela janela
// principal, sem `WS_EX_NOACTIVATE` e sem TOPMOST (fica acima do NeuralIA, nao
// das outras aplicacoes). Enter (ou "Salvar e verificar") com uma chave com a
// forma do slot manda `KeyEvent::Entered`; Esc ou "Cancelar", `Cancelled`;
// "Esquecer chave", `Forget`. O EDIT e limpo antes de o popup ser destruido,
// e logo que a chave sai dele.
//
// Quem abre o pedido sao os consumidores das chaves: a Traducao (o
// «Guardar chave» do cartao sem chave, `translation.rs`) e a primeira; o
// juiz do Consenso, o BYOM e os conectores chegam nas ondas seguintes.

/// O que o pedido de chave manda ao event loop.
#[derive(Debug)]
pub(in crate::windows_app) enum KeyEvent {
    /// Enter ou "Salvar e verificar" com uma chave com a forma do slot. O
    /// EDIT ja foi limpo quando isto sai.
    Entered { slot: KeySlot, key: ApiKey },
    /// "Esquecer chave".
    Forget(KeySlot),
    /// Esc ou "Cancelar".
    Cancelled,
}

pub(in crate::windows_app) const SECRET_PROMPT_NOTE: &str =
    "Fica cifrada neste Windows e só serve para o NeuralIA.";
pub(in crate::windows_app) const SECRET_PROMPT_SAVE: &str = "Salvar e verificar";
pub(in crate::windows_app) const SECRET_PROMPT_CANCEL: &str = "Cancelar";
pub(in crate::windows_app) const SECRET_PROMPT_FORGET: &str = "Esquecer chave";
pub(in crate::windows_app) const SECRET_PROMPT_INVALID: &str =
    "Essa chave não tem a forma certa. Confira e cole de novo.";

/// O titulo do pedido, pelo slot.
pub(in crate::windows_app) fn secret_prompt_title(slot: &KeySlot) -> String {
    match slot {
        KeySlot::OpenAi => "Cole a chave da OpenAI (começa por sk-)".to_string(),
        KeySlot::Anthropic => "Cole a chave da Anthropic (começa por sk-ant-)".to_string(),
        KeySlot::Gemini => "Cole a chave do Gemini (da AI Studio)".to_string(),
        other => format!("Cole a chave do {}", other.label()),
    }
}

/// A decisao do Enter, sem janela: com a forma do slot, o evento; sem ela,
/// nada (o pedido fica aberto e mostra `SECRET_PROMPT_INVALID`).
pub(in crate::windows_app) fn secret_prompt_submit(
    slot: &KeySlot,
    typed: &str,
) -> Option<KeyEvent> {
    validate_api_key(slot, typed).map(|key| KeyEvent::Entered {
        slot: slot.clone(),
        key,
    })
}

/// Ativavel (sem `WS_EX_NOACTIVATE`: recebe teclado) e nunca TOPMOST.
pub(in crate::windows_app) const SECRET_PROMPT_EX_STYLE: u32 = WS_EX_TOOLWINDOW;
/// `WS_CLIPCHILDREN`: repintar o aviso nao pinta por cima do EDIT e dos botoes.
pub(in crate::windows_app) const SECRET_PROMPT_STYLE: u32 = WS_POPUP | WS_BORDER | WS_CLIPCHILDREN;
pub(in crate::windows_app) const SECRET_PROMPT_EDIT_STYLE: u32 =
    WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32;
/// Bem acima da chave mais longa aceite (`API_KEY_MAX_CHARS`): uma colagem
/// longa demais chega inteira a validacao e e recusada, em vez de o EDIT a
/// cortar em silencio e guardar meia chave.
pub(in crate::windows_app) const SECRET_PROMPT_EDIT_LIMIT: usize = 4 * API_KEY_MAX_CHARS;
const _: () = assert!(SECRET_PROMPT_EDIT_LIMIT > API_KEY_MAX_CHARS);

const SECRET_PROMPT_SUBCLASS_ID: usize = 0x4E60;
const SECRET_PROMPT_EDIT_SUBCLASS_ID: usize = 0x4E61;
const EM_EMPTYUNDOBUFFER: u32 = 0x00CD;
/// Os ids dos tres botoes (o `hMenu` de um filho e o id dele).
pub(in crate::windows_app) const SECRET_PROMPT_SAVE_ID: u16 = 1;
pub(in crate::windows_app) const SECRET_PROMPT_CANCEL_ID: u16 = 2;
pub(in crate::windows_app) const SECRET_PROMPT_FORGET_ID: u16 = 3;

const SECRET_PROMPT_WIDTH: f64 = 460.0;
const SECRET_PROMPT_HEIGHT: f64 = 188.0;
const SECRET_PROMPT_PAD: f64 = 20.0;
const SECRET_PROMPT_EDIT_TOP: f64 = 70.0;
const SECRET_PROMPT_EDIT_HEIGHT: f64 = 30.0;
const SECRET_PROMPT_ERROR_TOP: f64 = 106.0;
const SECRET_PROMPT_BUTTON_TOP: f64 = 138.0;
const SECRET_PROMPT_BUTTON_HEIGHT: f64 = 32.0;

/// Para onde o pedido manda os eventos: o proxy do event loop no app, um
/// registo nos gates.
pub(in crate::windows_app) type KeySink = Box<dyn Fn(UserEvent)>;

/// O que as subclasses do popup e do EDIT leem. Numa Box: o endereco fica
/// estavel enquanto as subclasses o guardarem.
pub(in crate::windows_app) struct SecretPromptHost {
    sink: KeySink,
    /// O slot do pedido aberto; `None` fechado (um Enter atrasado cai).
    slot: RefCell<Option<KeySlot>>,
    /// O ultimo Enter trouxe algo sem a forma do slot.
    invalid: Cell<bool>,
}

impl SecretPromptHost {
    pub(in crate::windows_app) fn new(sink: KeySink) -> Self {
        Self {
            sink,
            slot: RefCell::new(None),
            invalid: Cell::new(false),
        }
    }

    pub(in crate::windows_app) fn open_for(&self, slot: KeySlot) {
        *self.slot.borrow_mut() = Some(slot);
        self.invalid.set(false);
    }

    pub(in crate::windows_app) fn close(&self) {
        *self.slot.borrow_mut() = None;
        self.invalid.set(false);
    }

    pub(in crate::windows_app) fn slot(&self) -> Option<KeySlot> {
        self.slot.borrow().clone()
    }

    pub(in crate::windows_app) fn is_invalid(&self) -> bool {
        self.invalid.get()
    }

    fn send(&self, event: KeyEvent) {
        (self.sink)(UserEvent::Keys(event));
    }
}

/// As janelas de um pedido aberto.
pub(in crate::windows_app) struct SecretPromptWindow {
    pub(in crate::windows_app) popup: HWND,
    pub(in crate::windows_app) edit: HWND,
    font: *mut core::ffi::c_void,
}

/// Le o EDIT e apaga o buffer UTF-16 da copia.
unsafe fn read_secret_text(edit: HWND) -> String {
    let length = GetWindowTextLengthW(edit);
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32);
    let text = if copied <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..copied as usize])
    };
    wipe(std::slice::from_raw_parts_mut(
        buffer.as_mut_ptr() as *mut u8,
        buffer.len() * 2,
    ));
    text
}

/// Tira a chave do EDIT (texto e desfazer).
pub(in crate::windows_app) unsafe fn clear_secret_edit(edit: HWND) {
    SetWindowTextW(edit, windows_sys::w!(""));
    SendMessageW(edit, EM_EMPTYUNDOBUFFER, 0, 0);
}

/// Enter ou "Salvar e verificar": valida o que esta no EDIT para o slot do
/// pedido. Com a forma certa, limpa o EDIT e so depois manda a chave; sem
/// ela, fica aberto com o aviso.
unsafe fn submit_secret_prompt(edit: HWND, host: &SecretPromptHost) {
    let Some(slot) = host.slot() else {
        return;
    };
    let mut typed = read_secret_text(edit);
    let decided = secret_prompt_submit(&slot, &typed);
    wipe(typed.as_mut_vec());
    match decided {
        Some(event) => {
            clear_secret_edit(edit);
            host.invalid.set(false);
            host.send(event);
        }
        None => {
            host.invalid.set(true);
            InvalidateRect(GetParent(edit), std::ptr::null(), 1);
        }
    }
}

/// Cria o pedido (invisivel) owned por `owner`, com o EDIT e os tres botoes.
/// O mesmo caminho serve o produto (`App::open_secret_prompt`) e o gate de
/// janela.
pub(in crate::windows_app) unsafe fn create_secret_prompt(
    owner: HWND,
    host: &SecretPromptHost,
    scale: f64,
) -> Option<SecretPromptWindow> {
    let px = |logical: f64| (logical * scale).round() as i32;
    let width = px(SECRET_PROMPT_WIDTH);
    let popup = CreateWindowExW(
        SECRET_PROMPT_EX_STYLE,
        windows_sys::w!("STATIC"),
        windows_sys::w!("NeuralIA — chave"),
        SECRET_PROMPT_STYLE,
        0,
        0,
        width,
        px(SECRET_PROMPT_HEIGHT),
        owner,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null(),
    );
    if popup.is_null() {
        return None;
    }
    let host_ptr = host as *const SecretPromptHost as usize;
    if SetWindowSubclass(
        popup,
        Some(secret_prompt_subclass),
        SECRET_PROMPT_SUBCLASS_ID,
        host_ptr,
    ) == 0
    {
        DestroyWindow(popup);
        return None;
    }
    let pad = px(SECRET_PROMPT_PAD);
    let edit = CreateWindowExW(
        0,
        windows_sys::w!("EDIT"),
        windows_sys::w!(""),
        SECRET_PROMPT_EDIT_STYLE,
        pad,
        px(SECRET_PROMPT_EDIT_TOP),
        width - 2 * pad,
        px(SECRET_PROMPT_EDIT_HEIGHT),
        popup,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null(),
    );
    if edit.is_null()
        || SetWindowSubclass(
            edit,
            Some(secret_prompt_edit_subclass),
            SECRET_PROMPT_EDIT_SUBCLASS_ID,
            host_ptr,
        ) == 0
    {
        DestroyWindow(popup);
        return None;
    }
    SendMessageW(edit, EM_SETLIMITTEXT, SECRET_PROMPT_EDIT_LIMIT, 0);
    let margin = (6.0 * scale) as usize;
    SendMessageW(
        edit,
        EM_SETMARGINS,
        EC_LEFTMARGIN | EC_RIGHTMARGIN,
        ((margin << 16) | margin) as isize,
    );
    let font = create_font(-px(15.0), FW_NORMAL as i32);
    if !font.is_null() {
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
    }

    // [Esquecer chave]          [Cancelar] [Salvar e verificar]
    let top = px(SECRET_PROMPT_BUTTON_TOP);
    let height = px(SECRET_PROMPT_BUTTON_HEIGHT);
    let gap = px(8.0);
    let save_w = px(150.0);
    let cancel_w = px(96.0);
    let forget_w = px(126.0);
    let layout = [
        (
            SECRET_PROMPT_SAVE_ID,
            SECRET_PROMPT_SAVE,
            width - pad - save_w,
            save_w,
        ),
        (
            SECRET_PROMPT_CANCEL_ID,
            SECRET_PROMPT_CANCEL,
            width - pad - save_w - gap - cancel_w,
            cancel_w,
        ),
        (SECRET_PROMPT_FORGET_ID, SECRET_PROMPT_FORGET, pad, forget_w),
    ];
    for (id, label, x, w) in layout {
        let text = wide_null(label);
        let button = CreateWindowExW(
            0,
            windows_sys::w!("BUTTON"),
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            x,
            top,
            w,
            height,
            popup,
            id as usize as _,
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        if button.is_null() {
            DestroyWindow(popup);
            if !font.is_null() {
                DeleteObject(font as _);
            }
            return None;
        }
        if !font.is_null() {
            SendMessageW(button, WM_SETFONT, font as usize, 1);
        }
    }
    Some(SecretPromptWindow { popup, edit, font })
}

/// Mostra o pedido ao centro do dono e poe o teclado no EDIT: e o unico
/// popup (alem da palette) que fica com o foco, porque e para escrever.
pub(in crate::windows_app) unsafe fn show_secret_prompt(owner: HWND, prompt: &SecretPromptWindow) {
    let mut area = RECT::default();
    let mut own = RECT::default();
    if GetWindowRect(owner, &mut area) != 0 && GetWindowRect(prompt.popup, &mut own) != 0 {
        let (width, height) = (own.right - own.left, own.bottom - own.top);
        SetWindowPos(
            prompt.popup,
            std::ptr::null_mut(),
            area.left + ((area.right - area.left) - width) / 2,
            area.top + ((area.bottom - area.top) - height) / 3,
            0,
            0,
            SWP_NOZORDER | SWP_NOSIZE,
        );
    }
    ShowWindow(prompt.popup, SW_SHOW);
    SetFocus(prompt.edit);
}

/// Fecha o pedido: o EDIT fica vazio ANTES de o popup ser destruido.
pub(in crate::windows_app) unsafe fn destroy_secret_prompt(prompt: SecretPromptWindow) {
    clear_secret_edit(prompt.edit);
    DestroyWindow(prompt.popup);
    if !prompt.font.is_null() {
        DeleteObject(prompt.font as _);
    }
}

/// O popup: pinta o titulo, a nota e o aviso, da ao EDIT as cores do tema e
/// recebe os cliques dos tres botoes.
unsafe extern "system" fn secret_prompt_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    let host = &*(reference_data as *const SecretPromptHost);
    match message {
        // STATIC devolve HTTRANSPARENT: o clique atravessava o popup.
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_COMMAND if ((wparam >> 16) & 0xFFFF) as u32 == BN_CLICKED => {
            match (wparam & 0xFFFF) as u16 {
                SECRET_PROMPT_SAVE_ID => {
                    if let Some(edit) = find_edit(hwnd) {
                        submit_secret_prompt(edit, host);
                    }
                }
                SECRET_PROMPT_CANCEL_ID => host.send(KeyEvent::Cancelled),
                SECRET_PROMPT_FORGET_ID => {
                    if let Some(slot) = host.slot() {
                        host.send(KeyEvent::Forget(slot));
                    }
                }
                _ => {}
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let theme = Theme::system();
            let hdc = wparam as *mut core::ffi::c_void;
            SetTextColor(hdc, rgb3(theme.fg));
            SetBkColor(hdc, rgb3(theme.page_bg));
            omnibox_brush(theme.page_bg) as LRESULT
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    paint_secret_prompt(hdc, &client, host);
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

unsafe fn find_edit(popup: HWND) -> Option<HWND> {
    let edit = FindWindowExW(
        popup,
        std::ptr::null_mut(),
        windows_sys::w!("EDIT"),
        std::ptr::null(),
    );
    (!edit.is_null()).then_some(edit)
}

unsafe fn paint_secret_prompt(hdc: *mut core::ffi::c_void, client: &RECT, host: &SecretPromptHost) {
    let theme = Theme::system();
    let scale = ((client.bottom - client.top) as f64 / SECRET_PROMPT_HEIGHT).max(1.0);
    let px = |logical: f64| (logical * scale).round() as i32;
    let background = CreateSolidBrush(rgb3(theme.surface));
    FillRect(hdc, client, background);
    DeleteObject(background as _);
    SetBkMode(hdc, TRANSPARENT as i32);
    let title = host
        .slot()
        .map(|slot| secret_prompt_title(&slot))
        .unwrap_or_default();
    let lines = [
        (
            title.as_str(),
            px(16.0),
            px(26.0),
            -px(16.0),
            FW_BOLD as i32,
            theme.fg,
        ),
        (
            SECRET_PROMPT_NOTE,
            px(44.0),
            px(20.0),
            -px(12.5),
            FW_NORMAL as i32,
            theme.fg_muted,
        ),
    ];
    for (text, top, height, size, weight, color) in lines {
        let font = create_font(size, weight);
        let old = SelectObject(hdc, font as _);
        SetTextColor(hdc, rgb3(color));
        let mut rect = RECT {
            left: px(SECRET_PROMPT_PAD),
            top,
            right: client.right - px(SECRET_PROMPT_PAD),
            bottom: top + height,
        };
        draw_text(
            hdc,
            text,
            &mut rect,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
        DeleteObject(font as _);
    }
    if host.is_invalid() {
        let font = create_font(-px(12.5), FW_NORMAL as i32);
        let old = SelectObject(hdc, font as _);
        SetTextColor(hdc, rgb3(theme.accent));
        let mut rect = RECT {
            left: px(SECRET_PROMPT_PAD),
            top: px(SECRET_PROMPT_ERROR_TOP),
            right: client.right - px(SECRET_PROMPT_PAD),
            bottom: px(SECRET_PROMPT_ERROR_TOP) + px(24.0),
        };
        draw_text(
            hdc,
            SECRET_PROMPT_INVALID,
            &mut rect,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
        DeleteObject(font as _);
    }
}

/// O EDIT: Enter manda, Esc cancela; e a destruicao nunca o deixa com texto.
unsafe extern "system" fn secret_prompt_edit_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    let host = &*(reference_data as *const SecretPromptHost);
    match message {
        WM_KEYDOWN => match wparam as u16 {
            VK_RETURN => {
                submit_secret_prompt(hwnd, host);
                return 0;
            }
            VK_ESCAPE => {
                host.send(KeyEvent::Cancelled);
                return 0;
            }
            // Um EDIT de uma linha nao trata Ctrl+A sozinho (ver omnibox).
            0x41 if (GetAsyncKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0 => {
                SendMessageW(hwnd, EM_SETSEL, 0, -1);
                return 0;
            }
            _ => {}
        },
        // O caracter de Enter/Escape ja foi tratado; deixa-lo chegar ao EDIT
        // fazia o sistema apitar.
        WM_CHAR if wparam == 13 || wparam == 27 => return 0,
        // Rede de seguranca: o dono destruido leva o pedido consigo sem
        // passar por `destroy_secret_prompt`.
        WM_DESTROY => clear_secret_edit(hwnd),
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

/// O estado do pedido de chave no `App`: o cofre (aberto na primeira vez que
/// e preciso, com os grants do registo), o popup aberto e o host.
pub(in crate::windows_app) struct KeysState {
    vault: Option<KeyVault>,
    prompt: Option<SecretPromptWindow>,
    host: Box<SecretPromptHost>,
}

impl KeysState {
    pub(in crate::windows_app) fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            vault: None,
            prompt: None,
            host: Box::new(SecretPromptHost::new(Box::new(move |event| {
                let _ = proxy.send_event(event);
            }))),
        }
    }
}

impl App {
    /// O cofre, aberto com os grants `keys/` e `gemini-live.key` do registo
    /// das lojas na primeira vez que e preciso.
    fn key_vault(&mut self) -> Option<&KeyVault> {
        if self.keys.vault.is_none() {
            let stores = self.stores.as_ref()?;
            let keys = stores.grant(KEYS_STORE).ok()?;
            let live = stores.grant(LIVE_KEY_STORE).ok()?;
            self.keys.vault = KeyVault::open(keys, live).ok();
        }
        self.keys.vault.as_ref()
    }

    /// Abre o pedido de chave do `slot` (um de cada vez). E a porta dos
    /// consumidores das chaves (a Traducao e a primeira).
    pub(in crate::windows_app) fn open_secret_prompt(&mut self, slot: KeySlot) {
        self.close_secret_prompt();
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        self.keys.host.open_for(slot);
        match unsafe { create_secret_prompt(owner, &self.keys.host, scale) } {
            Some(prompt) => {
                unsafe { show_secret_prompt(owner, &prompt) };
                self.keys.prompt = Some(prompt);
            }
            None => self.keys.host.close(),
        }
    }

    /// Fecha o pedido aberto. O host esquece o slot primeiro: um Enter que
    /// chegue atrasado ja nao encontra pedido.
    pub(in crate::windows_app) fn close_secret_prompt(&mut self) {
        self.keys.host.close();
        if let Some(prompt) = self.keys.prompt.take() {
            unsafe { destroy_secret_prompt(prompt) };
        }
    }

    /// O unico braco do pedido de chave no `user_event`.
    pub(in crate::windows_app) fn keys_event(&mut self, event: KeyEvent) {
        match event {
            KeyEvent::Entered { slot, key } => {
                self.close_secret_prompt();
                let text = match self.key_vault().map(|vault| vault.save(&slot, &key)) {
                    Some(Ok(())) => format!("Chave de {} guardada.", slot.label()),
                    Some(Err(error)) => format!("Não foi possível guardar a chave: {error}"),
                    None => "Não foi possível abrir o cofre das chaves.".to_string(),
                };
                self.show_splash(text, 3);
            }
            KeyEvent::Forget(slot) => {
                self.close_secret_prompt();
                let text = match self.key_vault().map(|vault| vault.forget(&slot)) {
                    Some(Ok(())) => format!("Chave de {} esquecida.", slot.label()),
                    Some(Err(error)) => format!("Não foi possível esquecer a chave: {error}"),
                    None => "Não foi possível abrir o cofre das chaves.".to_string(),
                };
                self.show_splash(text, 3);
            }
            KeyEvent::Cancelled => self.close_secret_prompt(),
        }
    }
}
