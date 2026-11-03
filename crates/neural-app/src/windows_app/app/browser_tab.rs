//! Aba Web independente no lado esquerdo do comparador.
//! Usa um EDIT Win32 persistente: navegar nunca entrega texto às IAs.
use crate::windows_app::*;
use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW;

pub(in crate::windows_app) const LEFT_BROWSER_BAR_HEIGHT: f64 = 48.0;
const LEFT_BROWSER_SUBCLASS_ID: usize = 0x4E91;

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

impl App {
    /// Fecha só a aba Web; não mata o comparador nem executa qualquer busca.
    pub(in crate::windows_app) fn close_left_browser_tab(&mut self) {
        if let Some(address) = self.left_browser_address.take() {
            unsafe { DestroyWindow(address) };
        }
        if self.surface == Surface::Comparator
            && let Some(view) = self.webview.take()
        {
            let _ = view.set_visible(false);
            let _ = view.focus_parent();
            drop(view);
        }
    }

    /// Abre a aba branca dentro da coluna esquerda (WebView2 filha do HWND).
    /// Um erro de inicialização deixa a UI anterior intacta.
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
        // O nascimento deve manter o gancho de segurança External do produto.
        // Criamos como child e aplicamos o retângulo ANTES de publicar o view.
        let builder = self
            .external_webview_builder(None, false)
            .with_html(browser_new_tab_html());
        let hooked = self.hooked_builder(builder, WebViewHost::External, None);
        let result = hooked.build_hooked_as_child(window);
        let view = match result {
            Ok(view) => view,
            Err(error) => {
                self.show_native_error(format!("Não foi possível criar a aba Web: {error}"));
                return;
            }
        };
        // Ao contrario do build top-level anterior, a WebView vive somente
        // dentro da area esquerda, preservando as superficies das IAs.
        if let Err(error) = view.set_bounds(bounds) {
            drop(view);
            self.show_native_error(format!("Não foi possível posicionar a aba Web: {error}"));
            return;
        }
        let address = unsafe {
            CreateWindowExW(
                0,
                windows_sys::w!("EDIT"),
                windows_sys::w!(""),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | ES_AUTOHSCROLL as u32
                    | windows_sys::Win32::UI::WindowsAndMessaging::WS_BORDER,
                0,
                0,
                100,
                34,
                owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if address.is_null() {
            drop(view);
            self.show_native_error("Não foi possível criar a barra de endereço Web.".to_string());
            return;
        }
        let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
        if unsafe {
            SetWindowSubclass(
                address,
                Some(left_browser_edit_subclass),
                LEFT_BROWSER_SUBCLASS_ID,
                proxy_ptr,
            )
        } == 0
        {
            unsafe { DestroyWindow(address) };
            drop(view);
            self.show_native_error("Não foi possível inicializar o endereço Web.".to_string());
            return;
        }
        let cue = wide_null("Digite um endereço ou pesquise na Web (sem IA)");
        unsafe {
            SendMessageW(address, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
            SendMessageW(address, EM_SETLIMITTEXT, 2048, 0);
        }

        self.close_left_browser_tab();
        self.left_browser_address = Some(address);
        self.webview = Some(view);
        self.hide_comparator_splitters();
        self.update_comparator_layout();
        unsafe { SetFocus(address) };
        self.request_redraw();
    }

    /// A aba Web aceita URLs normais; texto livre pesquisa no Google,
    /// nunca na omnibox inteligente e nunca nas três IAs.
    pub(in crate::windows_app) fn submit_left_browser_address(&mut self, input: &str) {
        if self.surface != Surface::Comparator || self.left_browser_address.is_none() {
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
        if let Some(browser) = &self.webview
            && let Err(error) = browser.load_url(url.as_str())
        {
            self.show_native_error(format!("Não foi possível navegar: {error}"));
        }
    }

    /// Ao voltar a focar a barra, mostra o URL atual, sem ler dados da IA.
    pub(in crate::windows_app) fn sync_left_browser_address(&self) {
        if self.surface != Surface::Comparator {
            return;
        }
        let (Some(address), Some(browser)) = (self.left_browser_address, &self.webview) else {
            return;
        };
        if let Ok(url) = browser.url()
            && (url.starts_with("http://") || url.starts_with("https://"))
        {
            unsafe {
                SetWindowTextW(address, wide_null(&url).as_ptr());
                SendMessageW(address, EM_SETSEL, 0, -1);
            }
        }
    }
}
