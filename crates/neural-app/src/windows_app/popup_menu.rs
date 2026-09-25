//! Os menus nativos (infra-notify-popups, critica C14): UM caminho para
//! todos os `TrackPopupMenu` da app -- o do Pomodoro, o do tema, os das
//! abas, dos grupos, da lista "‹N" e da pilula de uma coluna.
//!
//! `PopupMenu` descreve o menu (itens com icone, marca, cinzentos com a
//! razao a direita, separadores, submenus) e `track_popup_menu` abre-o. O
//! que isto acrescenta a um `TrackPopupMenu` solto e o teclado: antes de
//! abrir, regista-se a ORIGEM (o `GetFocus()` e, se ele estiver dentro de
//! uma WebView, qual); depois de o menu fechar -- com escolha ou sem ela --
//! o teclado volta la, pela `MoveFocus(PROGRAMMATIC)` do WebView2
//! (`WebView::focus`) ou por `SetFocus` num EDIT nosso. Sem isto, fechar um
//! menu deixava a pagina ou a omnibox sem teclado ate um clique. A unica
//! excepcao e um comando que declara que muda o teclado de proposito
//! (`MenuCommand::moves_focus`: abrir uma aba). Gate real Win32
//! `popup_menu_restores_origin_focus` (so CI).

use super::*;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, IsChild, IsWindow, MF_CHECKED, MF_DISABLED, MF_GRAYED, MF_POPUP,
    PostMessageW, SetForegroundWindow, WM_NULL,
};

/// O icone a esquerda do texto de um item: a amostra redonda de uma cor
/// (os grupos das abas).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum MenuIcon {
    Swatch(Rgb),
}

/// Um item que se escolhe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct MenuCommand {
    /// O que o `TrackPopupMenu` devolve (0 fica para "fechado sem escolha").
    pub(in crate::windows_app) id: usize,
    pub(in crate::windows_app) label: String,
    pub(in crate::windows_app) icon: Option<MenuIcon>,
    pub(in crate::windows_app) checked: bool,
    /// Cinzento e sem clique. `Some("")`: so cinzento (um titulo);
    /// `Some(razao)`: a razao aparece a direita do texto.
    pub(in crate::windows_app) disabled: Option<String>,
    /// Escolher este comando poe o teclado noutro sitio de proposito: o
    /// menu nao o devolve a origem.
    pub(in crate::windows_app) moves_focus: bool,
}

impl MenuCommand {
    pub(in crate::windows_app) fn new(id: usize, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            icon: None,
            checked: false,
            disabled: None,
            moves_focus: false,
        }
    }

    pub(in crate::windows_app) fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub(in crate::windows_app) fn icon(mut self, icon: MenuIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Cinzento; `reason` vazia para um titulo sem razao.
    pub(in crate::windows_app) fn disabled(mut self, reason: impl Into<String>) -> Self {
        self.disabled = Some(reason.into());
        self
    }

    pub(in crate::windows_app) fn moves_focus(mut self) -> Self {
        self.moves_focus = true;
        self
    }

    /// O texto do item: o rotulo e, num cinzento com razao, a razao depois
    /// de um tab (a coluna dos atalhos, alinhada a direita).
    pub(in crate::windows_app) fn text(&self) -> String {
        match self.disabled.as_deref() {
            Some(reason) if !reason.is_empty() => format!("{}\t{reason}", self.label),
            _ => self.label.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum MenuEntry {
    Command(MenuCommand),
    Separator,
    Submenu {
        label: String,
        icon: Option<MenuIcon>,
        entries: Vec<MenuEntry>,
    },
}

impl From<MenuCommand> for MenuEntry {
    fn from(command: MenuCommand) -> Self {
        Self::Command(command)
    }
}

/// Um menu nativo, descrito antes de abrir.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::windows_app) struct PopupMenu {
    pub(in crate::windows_app) entries: Vec<MenuEntry>,
}

impl PopupMenu {
    pub(in crate::windows_app) fn push(&mut self, entry: impl Into<MenuEntry>) {
        self.entries.push(entry.into());
    }

    pub(in crate::windows_app) fn separator(&mut self) {
        self.entries.push(MenuEntry::Separator);
    }

    /// O comando `id`, onde quer que esteja (submenus incluidos).
    pub(in crate::windows_app) fn command(&self, id: usize) -> Option<&MenuCommand> {
        fn find(entries: &[MenuEntry], id: usize) -> Option<&MenuCommand> {
            entries.iter().find_map(|entry| match entry {
                MenuEntry::Command(command) if command.id == id => Some(command),
                MenuEntry::Submenu { entries, .. } => find(entries, id),
                _ => None,
            })
        }
        (id != 0).then(|| find(&self.entries, id)).flatten()
    }
}

/// Quem abriu o menu: o botao (sem `TPM_RIGHTBUTTON`, o direito nao escolhe
/// itens num menu aberto pelo esquerdo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum MenuButton {
    Left,
    Right,
}

/// Uma vista que pode receber o teclado de volta pelo WebView2: a janela
/// que a hospeda (o foco esta nela se o `GetFocus()` for ela ou uma
/// descendente) e a `MoveFocus(PROGRAMMATIC)`.
pub(in crate::windows_app) trait FocusHost {
    fn host_window(&self) -> HWND;
    fn take_focus(&self);
}

impl FocusHost for WebView {
    fn host_window(&self) -> HWND {
        use wry::WebViewExtWindows;
        self.hwnd().0 as HWND
    }

    fn take_focus(&self) {
        // wry: ICoreWebView2Controller::MoveFocus(PROGRAMMATIC).
        let _ = self.focus();
    }
}

/// Onde estava o teclado quando o menu abriu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum FocusOrigin {
    /// Em lado nenhum desta thread.
    Nowhere,
    /// Dentro da vista `hosts[index]`.
    Host(usize),
    /// Numa janela nossa fora das vistas (o EDIT da omnibox, a janela).
    Window(HWND),
}

/// A origem, pura: `focus` e o `GetFocus()`; `inside(host, focus)` diz se
/// `focus` e descendente de `host` (`IsChild` no produto).
pub(in crate::windows_app) fn focus_origin(
    focus: HWND,
    hosts: &[HWND],
    inside: impl Fn(HWND, HWND) -> bool,
) -> FocusOrigin {
    if focus.is_null() {
        return FocusOrigin::Nowhere;
    }
    hosts
        .iter()
        .position(|host| !host.is_null() && (*host == focus || inside(*host, focus)))
        .map_or(FocusOrigin::Window(focus), FocusOrigin::Host)
}

/// O que fazer ao teclado depois do menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum FocusRestore {
    Leave,
    /// `MoveFocus(PROGRAMMATIC)` na vista `hosts[index]`.
    Host(usize),
    /// `SetFocus` na janela.
    Window(HWND),
}

/// Pura: o teclado volta sempre a origem, salvo se o comando escolhido o
/// muda de proposito.
pub(in crate::windows_app) fn decide_focus_restore(
    origin: FocusOrigin,
    chosen_moves_focus: bool,
) -> FocusRestore {
    if chosen_moves_focus {
        return FocusRestore::Leave;
    }
    match origin {
        FocusOrigin::Nowhere => FocusRestore::Leave,
        FocusOrigin::Host(index) => FocusRestore::Host(index),
        FocusOrigin::Window(hwnd) => FocusRestore::Window(hwnd),
    }
}

fn restore_focus(restore: FocusRestore, hosts: &[&dyn FocusHost]) {
    match restore {
        FocusRestore::Leave => {}
        FocusRestore::Host(index) => {
            if let Some(host) = hosts.get(index) {
                host.take_focus();
            }
        }
        FocusRestore::Window(hwnd) => unsafe {
            if IsWindow(hwnd) != 0 {
                SetFocus(hwnd);
            }
        },
    }
}

/// Acrescenta os itens a `menu`. Os textos e as amostras ficam vivos ate o
/// menu fechar: o menu nao e dono dos bitmaps dos seus itens.
unsafe fn append_entries(
    menu: *mut core::ffi::c_void,
    entries: &[MenuEntry],
    icon_size: i32,
    texts: &mut Vec<Vec<u16>>,
    bitmaps: &mut Vec<*mut core::ffi::c_void>,
) {
    let icon_bitmap = |icon: Option<MenuIcon>, bitmaps: &mut Vec<*mut core::ffi::c_void>| {
        let Some(MenuIcon::Swatch(color)) = icon else {
            return std::ptr::null_mut();
        };
        let swatch = color_swatch_bitmap(color, icon_size);
        if !swatch.is_null() {
            bitmaps.push(swatch);
        }
        swatch
    };
    for entry in entries {
        match entry {
            MenuEntry::Separator => {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            }
            MenuEntry::Command(command) => {
                texts.push(wide_null(&command.text()));
                let text = texts.last().map_or(&[0u16][..], |text| text.as_slice());
                if command.icon.is_some() {
                    let swatch = icon_bitmap(command.icon, bitmaps);
                    append_swatch_item(menu, command.id, text, swatch, command.checked);
                } else {
                    let mut flags = MF_STRING;
                    if command.checked {
                        flags |= MF_CHECKED;
                    }
                    if command.disabled.is_some() {
                        flags |= MF_DISABLED | MF_GRAYED;
                    }
                    AppendMenuW(menu, flags, command.id, text.as_ptr());
                }
            }
            MenuEntry::Submenu {
                label,
                icon,
                entries,
            } => {
                let submenu = CreatePopupMenu();
                if submenu.is_null() {
                    continue;
                }
                append_entries(submenu, entries, icon_size, texts, bitmaps);
                texts.push(wide_null(label));
                let text = texts.last().map_or(&[0u16][..], |text| text.as_slice());
                // O submenu passa a ser do menu e morre com ele.
                if icon.is_some() {
                    let swatch = icon_bitmap(*icon, bitmaps);
                    append_swatch_submenu(menu, submenu, text, swatch);
                } else {
                    AppendMenuW(menu, MF_POPUP, submenu as usize, text.as_ptr());
                }
            }
        }
    }
}

/// Abre `menu` em `at` (coordenadas de ecra), dono `owner`, e devolve o id
/// escolhido (0: fechado sem escolha). Antes regista a origem do teclado
/// entre `hosts` (as WebViews que o podiam ter); depois devolve-o la, salvo
/// se o comando escolhido o muda de proposito.
pub(in crate::windows_app) fn track_popup_menu(
    menu: &PopupMenu,
    owner: HWND,
    at: POINT,
    button: MenuButton,
    scale: f64,
    hosts: &[&dyn FocusHost],
) -> usize {
    let host_windows: Vec<HWND> = hosts.iter().map(|host| host.host_window()).collect();
    // A origem, ANTES de o menu abrir: depois dele o teclado ja pode estar
    // na janela dona.
    let origin = focus_origin(unsafe { GetFocus() }, &host_windows, |host, hwnd| unsafe {
        IsChild(host, hwnd) != 0
    });
    let picked = unsafe { run_menu(menu, owner, at, button, scale) };
    let moves_focus = menu
        .command(picked)
        .is_some_and(|command| command.moves_focus);
    restore_focus(decide_focus_restore(origin, moves_focus), hosts);
    picked
}

unsafe fn run_menu(
    menu: &PopupMenu,
    owner: HWND,
    at: POINT,
    button: MenuButton,
    scale: f64,
) -> usize {
    let handle = CreatePopupMenu();
    if handle.is_null() {
        return 0;
    }
    let mut texts: Vec<Vec<u16>> = Vec::new();
    let mut bitmaps: Vec<*mut core::ffi::c_void> = Vec::new();
    let icon_size = (16.0 * scale).round() as i32;
    append_entries(handle, &menu.entries, icon_size, &mut texts, &mut bitmaps);
    let root = GetAncestor(owner, GA_ROOT);
    let root = if root.is_null() { owner } else { root };
    // Sem o dono em primeiro plano, o menu nao fecha ao clicar fora.
    SetForegroundWindow(root);
    let flags = match button {
        MenuButton::Left => TPM_RETURNCMD,
        MenuButton::Right => TPM_RETURNCMD | TPM_RIGHTBUTTON,
    };
    let picked = TrackPopupMenu(handle, flags, at.x, at.y, 0, root, std::ptr::null());
    // Documentado no TrackPopupMenu (e no KB135788): com o dono posto em
    // primeiro plano so para o menu, a mensagem seguinte ao fecho tem de
    // chegar a ele, senao o PROXIMO menu abre e fecha sozinho.
    PostMessageW(root, WM_NULL, 0, 0);
    DestroyMenu(handle);
    for bitmap in bitmaps {
        DeleteObject(bitmap as _);
    }
    drop(texts);
    usize::try_from(picked).unwrap_or(0)
}

/// O cursor, em coordenadas de ecra: onde abrem os menus do botao direito
/// que nascem num procedimento de janela.
pub(in crate::windows_app) fn cursor_point() -> POINT {
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut cursor);
    }
    cursor
}

impl App {
    /// As WebViews que podiam ter o teclado quando um menu abre: as colunas,
    /// a fonte aberta ao lado, a web unica (Leitor, PDF, web externa), o
    /// painel do Ctrl+H e o de servicos.
    pub(in crate::windows_app) fn focus_hosts(&self) -> Vec<&dyn FocusHost> {
        let mut hosts: Vec<&dyn FocusHost> = Vec::new();
        if let Some(comp) = &self.comparator {
            for view in &comp.views {
                hosts.push(&view.webview);
            }
            if let Some(split) = &comp.split {
                hosts.push(&split.webview);
            }
        }
        if let Some(webview) = &self.webview {
            hosts.push(webview);
        }
        if let Some(panel) = self.side_panel.view() {
            hosts.push(panel);
        }
        if let Some(panel) = &self.service_panel {
            hosts.push(&panel.webview);
        }
        hosts
    }

    /// Abre `menu` pela janela principal, com o teclado devolvido a origem.
    pub(in crate::windows_app) fn track_menu(
        &self,
        menu: &PopupMenu,
        at: POINT,
        button: MenuButton,
    ) -> usize {
        let Some(window) = &self.window else {
            return 0;
        };
        let Some(owner) = window_hwnd(window) else {
            return 0;
        };
        let scale = window.scale_factor().max(1.0);
        track_popup_menu(menu, owner, at, button, scale, &self.focus_hosts())
    }
}
