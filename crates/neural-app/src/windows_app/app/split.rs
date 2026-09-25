use std::sync::atomic::Ordering;
use url::Url;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    PAINTSTRUCT, ScreenToClient,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, GetCursorPos, HTCLIENT, SW_HIDE, SWP_NOACTIVATE,
    SetWindowPos, ShowWindow, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT,
};
use winit::event_loop::EventLoopProxy;
use winit::window::Fullscreen;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::Request;
use wry::{NewWindowResponse, PermissionKind, PermissionResponse, WebView, WebViewBuilder};

use neural_core::{MemoryDocument, MemoryKind, MemorySourceKind};

use crate::windows_app::*;

/// O que fazer com um pedido de split conforme a superficie atual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum SplitFallback {
    OpenSplit,
    OpenWeb,
    Ignore,
}

/// A fonte aberta ao lado (Split). Generica na vista como a
/// `ComparatorView`: quem decide se um Split privado pode ser lido recebe o
/// Split inteiro, com o `private` dele, e nao um booleano copiado a parte.
pub(in crate::windows_app) struct SplitView<V = WebView> {
    pub(in crate::windows_app) webview: V,
    pub(in crate::windows_app) source_index: usize,
    /// Identidade da aba que originou este Split. URL nao e identidade:
    /// a mesma fonte pode existir em dois grupos diferentes.
    pub(in crate::windows_app) context_id: Option<u64>,
    pub(in crate::windows_app) fullscreen: bool,
    pub(in crate::windows_app) private: bool,
}

pub(in crate::windows_app) fn split_build_is_current(
    start_generation: u64,
    current_generation: u64,
    surface: Surface,
) -> bool {
    start_generation == current_generation && surface == Surface::Comparator
}

pub(in crate::windows_app) fn commit_split_build<T, E, P>(
    result: Result<T, E>,
    current_split: &mut Option<P>,
    expanded: &mut Option<usize>,
) -> Result<(T, Option<P>), E> {
    match result {
        Ok(value) => {
            *expanded = None;
            Ok((value, current_split.take()))
        }
        Err(error) => Err(error),
    }
}

pub(in crate::windows_app) unsafe extern "system" fn comparator_splitter_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    match message {
        // A classe STATIC responde HTTRANSPARENT quando nao tem SS_NOTIFY: o
        // sistema entrega entao o rato a janela de baixo -- aqui, o WebView2.
        // Sem esta linha nenhum WM_LBUTTONDOWN chega, o SetCapture nunca corre
        // e o arrasto do divisor e codigo morto. O botao de saida (1299) e a
        // palette (1410) ja carregavam este mesmo override.
        WM_NCHITTEST => return HTCLIENT as LRESULT,
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            return 0;
        }
        WM_MOUSEMOVE => {
            if GetCapture() == hwnd {
                let mut point = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut point) != 0 {
                    let divider = subclass_id.saturating_sub(SPLITTER_SUBCLASS_BASE);
                    // Publicar antes de marcar o pedido: quem for atende-lo ja
                    // encontra a posicao nova.
                    RESIZE_DIVIDER.store(divider, Ordering::Release);
                    RESIZE_X.store(point.x, Ordering::Release);
                    if !RESIZE_PENDING.swap(true, Ordering::AcqRel) {
                        let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                        if proxy.send_event(UserEvent::ResizeComparator).is_err() {
                            // Ninguem vai limpar a marca; sem isto o arrasto
                            // ficava mudo para sempre depois de um erro.
                            RESIZE_PENDING.store(false, Ordering::Release);
                        }
                    }
                }
            }
            return 0;
        }
        WM_LBUTTONUP => {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            return 0;
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let theme = Theme::system();
                    let bg = CreateSolidBrush(rgb3(theme.bar_bg));
                    FillRect(hdc, &client, bg);
                    DeleteObject(bg as _);
                    let center = (client.right - client.left) / 2;
                    let line = RECT {
                        left: center,
                        top: 0,
                        right: center + 1,
                        bottom: client.bottom,
                    };
                    let brush = CreateSolidBrush(rgb3(theme.surface_line));
                    FillRect(hdc, &line, brush);
                    DeleteObject(brush as _);
                }
                EndPaint(hwnd, &paint);
            }
            return 0;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

impl App {
    pub(in crate::windows_app) fn split_ipc_event_impl(
        source_index: usize,
        private: bool,
        action: IpcAction,
    ) -> Option<UserEvent> {
        match action {
            // O Ctrl+Shift+Z no Split privado nao faz notas: a pagina nem
            // chega a ser lida. O "Salvar nota" da barra e um pedido
            // explicito de quem le, e grava (so nas notas; o aviso diz que
            // foi no modo privado).
            IpcAction::Note {
                via: NoteVia::Shortcut,
            } if private => Some(UserEvent::NoteRefusedPrivate),
            IpcAction::Note { via } => Some(UserEvent::NoteRequested {
                target: Some(PageTarget::Split),
                via,
            }),
            // Defesa em profundidade: a barra de um painel privado nem mostra
            // o "Mandar para IA" nem o "Traduzir", e mesmo que uma mensagem
            // chegasse o texto nao pode sair para o comparador (historico e
            // memoria).
            IpcAction::Search { .. } if private => None,
            IpcAction::SplitClose => Some(UserEvent::CloseSplit),
            IpcAction::SplitExpand | IpcAction::Fullscreen => {
                Some(UserEvent::ToggleSplitFullscreen)
            }
            IpcAction::Palette { col } if col == source_index => {
                Some(UserEvent::OpenPalette(source_index))
            }
            IpcAction::Omnibox => Some(UserEvent::OpenPalette(source_index)),
            IpcAction::NewTab { col: Some(col) } if col == source_index => {
                Some(UserEvent::NewTab(source_index))
            }
            IpcAction::ShortcutExpand { col } => Some(UserEvent::ExpandComparator(col)),
            IpcAction::Reload => Some(UserEvent::ReloadTarget(PageTarget::Split)),
            IpcAction::Print => Some(UserEvent::PrintTarget(PageTarget::Split)),
            IpcAction::DevTools => Some(UserEvent::OpenDevToolsTarget(PageTarget::Split)),
            IpcAction::ViewSource => Some(UserEvent::ViewSourceTarget(PageTarget::Split)),
            other => common_ipc_event(other),
        }
    }

    /// O WebView do Split: o `WebViewBuilder` do tema, configurado por
    /// `configure_split_webview` com o `SplitBuild` que `split_open_plan`
    /// decidiu. Nada mais se lhe acrescenta aqui.
    pub(in crate::windows_app) fn split_webview_builder(
        &self,
        build: &SplitBuild,
    ) -> WebViewBuilder<'static> {
        let proxy = self.proxy.clone();
        configure_split_webview(themed_webview_builder(), build, move |event| {
            let _ = proxy.send_event(event);
        })
    }

    pub(in crate::windows_app) fn open_split(
        &mut self,
        source_index: usize,
        url: String,
        allow_local: bool,
    ) -> bool {
        self.open_split_mode(source_index, url, allow_local, false, None)
    }

    /// Um pedido de split que chega depois de o comparador desaparecer (o
    /// popup da pagina ficou na fila atras do Home) ainda pode abrir como Web
    /// normal. Um pedido PRIVADO nao: web() grava historico, captura memoria
    /// e usa o perfil com cookies normais.
    pub(in crate::windows_app) fn split_request_fallback(
        surface: Surface,
        private: bool,
    ) -> SplitFallback {
        if surface == Surface::Comparator {
            SplitFallback::OpenSplit
        } else if private {
            SplitFallback::Ignore
        } else {
            SplitFallback::OpenWeb
        }
    }

    pub(in crate::windows_app) fn open_split_mode(
        &mut self,
        source_index: usize,
        url: String,
        allow_local: bool,
        private: bool,
        existing_context_id: Option<u64>,
    ) -> bool {
        self.open_split_opened_by(
            source_index,
            url,
            allow_local,
            private,
            existing_context_id,
            None,
        )
    }

    /// Um link que a fonte aberta ao lado mandou abrir noutra aba: a aba nova
    /// nasce no grupo da aba de onde saiu, como no Chrome.
    pub(in crate::windows_app) fn open_split_from_split(
        &mut self,
        source_index: usize,
        url: String,
    ) {
        let opener = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .filter(|split| split.source_index == source_index && !split.private)
            .and_then(|split| split.context_id);
        let _ = self.open_split_opened_by(source_index, url, false, false, None, opener);
    }

    /// `opener`: a aba de onde o link saiu, quando saiu de uma aba. Se ela
    /// estiver num grupo, a aba nova entra no fim do troco desse grupo.
    pub(in crate::windows_app) fn open_split_opened_by(
        &mut self,
        source_index: usize,
        url: String,
        allow_local: bool,
        private: bool,
        existing_context_id: Option<u64>,
        opener: Option<u64>,
    ) -> bool {
        let column_name = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(source_index))
            .map(|view| view.name);
        // Tudo o que depende do `private` sai daqui (ver `split_open_plan`).
        let build = match split_open_plan(
            self.surface,
            column_name,
            source_index,
            url,
            allow_local,
            private,
            remote_capability,
        ) {
            SplitOpenPlan::Web(url) => {
                self.web(url);
                return true;
            }
            SplitOpenPlan::Ignore => return false,
            SplitOpenPlan::Refuse { message, seconds } => {
                self.show_splash(message.to_string(), seconds);
                return false;
            }
            SplitOpenPlan::Build(build) => build,
        };
        // Memoria, sessao, aba de contexto e o SplitView leem o mesmo valor
        // que o builder recebeu.
        let private = build.incognito;
        let valid = build.url.clone();
        let source_name = build.source_name;

        let generation = self.current_generation();
        let Some(window) = &self.window else {
            return false;
        };
        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;
        let ai_width = logical_w * 0.54;
        let bounds = wry::Rect {
            position: LogicalPosition::new(ai_width, COMPARATOR_CHROME_HEIGHT).into(),
            size: LogicalSize::new(
                (logical_w - ai_width).max(1.0),
                (logical_h - COMPARATOR_CHROME_HEIGHT).max(100.0),
            )
            .into(),
        };

        // Construir primeiro, com o estado antigo intacto. WebView2 pode falhar
        // ou entrar num pump aninhado; uma tentativa falhada nao pode destruir
        // o Split que o utilizador ainda esta a ver nem sair da expansao atual.
        let built = self
            .split_webview_builder(&build)
            .with_bounds(bounds)
            .with_url(valid.as_str())
            .build_as_child(window);

        if !split_build_is_current(generation, self.current_generation(), self.surface) {
            if let Ok(webview) = built {
                drop(webview);
            }
            return false;
        }

        let committed = match &mut self.comparator {
            Some(comp) => commit_split_build(built, &mut comp.split, &mut comp.expanded),
            None => {
                if let Ok(webview) = built {
                    drop(webview);
                }
                return false;
            }
        };

        match committed {
            Ok((webview, previous)) => {
                if let Some(previous) = previous {
                    let _ = previous.webview.set_visible(false);
                    let _ = previous.webview.focus_parent();
                    drop(previous);
                }

                self.leave_fullscreen();
                self.hide_comparator_splitters();

                // So uma fonte que abriu de verdade entra na memoria/sessao.
                // Antes, uma falha de build deixava uma fonte fantasma gravada.
                if split_open_records_source(existing_context_id, private)
                    && let Some((title, mut document)) =
                        split_source_memory(&valid, source_name, private)
                {
                    let value = valid.to_string();
                    if let Some(session) = &mut self.current_research {
                        document = document.session(session.id.clone());
                        let memory_id = document.id.clone();
                        session.add_source(
                            Some(source_name.to_string()),
                            title,
                            value.clone(),
                            Some(memory_id),
                            value,
                        );
                        self.memory.save_session(session.clone());
                    }
                    self.memory.capture(document);
                }

                let _ = webview.zoom(self.zoom);
                self.install_context_menu(&webview, WebViewHost::Split(source_index));
                let mut lost_grouped = 0usize;
                if let Some(comp) = &mut self.comparator {
                    let ComparatorState {
                        contexts,
                        groups,
                        next_context_id,
                        ..
                    } = comp;
                    let grouped_before = grouped_tab_ids(&contexts[source_index]);
                    let context_id = record_split_context(
                        &mut contexts[source_index],
                        &mut groups[source_index],
                        next_context_id,
                        valid.to_string(),
                        private,
                        existing_context_id,
                        opener,
                    );
                    lost_grouped = lost_grouped_tabs(&grouped_before, &contexts[source_index]);
                    // A aba que se abriu e aquela para onde se olha: nasce a
                    // meio da fila (no fim do grupo de quem a abriu) e fica
                    // a vista.
                    if context_id.is_some() {
                        comp.bar_focus[source_index] = context_id;
                    }
                    comp.split = Some(SplitView {
                        webview,
                        source_index,
                        context_id,
                        fullscreen: false,
                        private,
                    });
                }
                if let Some(notice) = lost_grouped_notice(lost_grouped) {
                    self.show_splash(notice, 5);
                }
                self.update_comparator_layout();
                self.sync_comparator_splitters();
                self.request_redraw();
                true
            }
            Err(error) => {
                // O estado anterior continua vivo. Reaplica a geometria para
                // garantir que nem um resize ocorrido durante o pump do build
                // deixe uma metade vazia.
                self.update_comparator_layout();
                self.sync_comparator_splitters();
                self.request_redraw();
                self.show_splash(format!("Não consegui abrir a fonte ao lado: {error}"), 4);
                false
            }
        }
    }

    pub(in crate::windows_app) fn open_private_panel(&mut self) {
        if self.surface != Surface::Comparator {
            return;
        }
        let source_index = self
            .comparator
            .as_ref()
            .and_then(|comp| {
                comp.expanded.or_else(|| {
                    comp.views
                        .iter()
                        .enumerate()
                        .find_map(|(index, _)| (!comp.minimized[index]).then_some(index))
                })
            })
            .unwrap_or(0);
        let _ = self.open_split_mode(
            source_index,
            "https://www.google.com/".to_string(),
            false,
            true,
            None,
        );
    }

    pub(in crate::windows_app) fn close_split(&mut self) {
        let was_fullscreen = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| split.fullscreen);
        if let Some(comp) = &mut self.comparator
            && let Some(split) = comp.split.take()
        {
            drop(split);
        }
        if was_fullscreen && let Some(window) = &self.window {
            window.set_fullscreen(None);
        }
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    pub(in crate::windows_app) fn toggle_split_fullscreen(&mut self) {
        let Some(fullscreen) = self
            .comparator
            .as_mut()
            .and_then(|comp| comp.split.as_mut())
            .map(|split| {
                split.fullscreen = !split.fullscreen;
                split.fullscreen
            })
        else {
            return;
        };

        if let Some(window) = &self.window {
            window.set_fullscreen(if fullscreen {
                Some(Fullscreen::Borderless(None))
            } else {
                None
            });
        }
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }

    pub(in crate::windows_app) fn hide_comparator_splitters(&self) {
        for hwnd in self.splitters.iter().flatten() {
            unsafe {
                ShowWindow(*hwnd, SW_HIDE);
            }
        }
    }

    pub(in crate::windows_app) fn sync_comparator_splitters(&mut self) {
        // Todo o divisor que nao couber na geometria de agora e escondido
        // abaixo, um a um: uma transicao 3 -> 2 -> 1 colunas, ou Comparator
        // -> Split View, nunca deixa um splitter da geometria anterior a
        // vista. Antes escondiam-se todos aqui em cima e mostravam-se logo a
        // seguir -- o que piscava a cada WM_MOVE agora que esta funcao
        // tambem corre quando a janela e arrastada.
        let Some(window) = &self.window else {
            self.hide_comparator_splitters();
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            self.hide_comparator_splitters();
            return;
        };

        let (show, boundaries, content_height, scale) = if let Some(comp) = &self.comparator {
            let scale = window.scale_factor().max(1.0);
            let size = window.inner_size();
            let logical_w = comparator_logical_width(size.width as f64 / scale, comp.panel_width);
            let logical_h = size.height as f64 / scale;
            // Com o painel de servicos em tela cheia por cima de tudo, os
            // divisores (popups, acima das WebViews) ficavam a flutuar sobre
            // o video.
            let show = self.surface == Surface::Comparator
                && comp.split.is_none()
                && comp.expanded.is_none()
                && !self.service_covers_window();
            // Um divisor por fronteira entre colunas visiveis: o fim de cada
            // faixa menos a ultima, na mesma geometria que as WebViews usam.
            let spans =
                visible_column_spans(logical_w, comp.views.len(), &comp.weights, &comp.minimized);
            let boundaries: Vec<f64> = spans
                .iter()
                .take(spans.len().saturating_sub(1))
                .map(|span| span.x + span.width)
                .collect();
            (
                show,
                boundaries,
                (logical_h - COMPARATOR_CHROME_HEIGHT).max(1.0),
                scale,
            )
        } else {
            (false, Vec::new(), 1.0, window.scale_factor().max(1.0))
        };

        let mut origin = POINT { x: 0, y: 0 };
        unsafe {
            ClientToScreen(owner, &mut origin);
        }

        for slot in 0..self.splitters.len() {
            if !show || slot >= boundaries.len() {
                if let Some(hwnd) = self.splitters[slot] {
                    unsafe {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                }
                continue;
            }

            let hwnd = match self.splitters[slot] {
                Some(hwnd) => hwnd,
                None => unsafe {
                    let width = (SPLITTER_WIDTH * scale).round().max(3.0) as i32;
                    let height = (content_height * scale).round().max(1.0) as i32;
                    // Owned pela janela principal: e o que o poe acima dos
                    // WebView2 (filhas do dono) sem o pousar sobre o ambiente
                    // de trabalho inteiro. Um divisor a flutuar por cima de
                    // outra aplicacao era o que o TOPMOST daqui fazia.
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
                        continue;
                    }
                    let proxy_ptr =
                        (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                    if SetWindowSubclass(
                        created,
                        Some(comparator_splitter_subclass),
                        SPLITTER_SUBCLASS_BASE + slot,
                        proxy_ptr,
                    ) == 0
                    {
                        DestroyWindow(created);
                        continue;
                    }
                    self.splitters[slot] = Some(created);
                    created
                },
            };

            let width = (SPLITTER_WIDTH * scale).round().max(3.0) as i32;
            let x =
                origin.x + (boundaries[slot] * scale - SPLITTER_WIDTH * scale / 2.0).round() as i32;
            let y = origin.y + (COMPARATOR_CHROME_HEIGHT * scale).round() as i32;
            let height = (content_height * scale).round().max(1.0) as i32;
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE,
                );
                show_popup_without_activation(hwnd);
                InvalidateRect(hwnd, std::ptr::null(), 1);
            }
        }
    }

    pub(in crate::windows_app) fn resize_comparator(&mut self, divider: usize, screen_x: i32) {
        let (Some(window), Some(comp)) = (&self.window, &mut self.comparator) else {
            return;
        };
        if comp.split.is_some() || comp.expanded.is_some() {
            return;
        }

        let visible: Vec<usize> = comp
            .views
            .iter()
            .enumerate()
            .filter_map(|(index, _)| (!comp.minimized[index]).then_some(index))
            .collect();
        if divider + 1 >= visible.len() {
            return;
        }

        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let mut point = POINT { x: screen_x, y: 0 };
        unsafe {
            ScreenToClient(owner, &mut point);
        }
        let scale = window.scale_factor().max(1.0);
        let logical_w =
            comparator_logical_width(window.inner_size().width as f64 / scale, comp.panel_width);
        let mouse_x = (point.x as f64 / scale).clamp(0.0, logical_w);

        comp.weights = resized_weights(&comp.weights, &visible, divider, mouse_x, logical_w);

        self.update_comparator_layout();
        self.sync_comparator_splitters();
        self.request_redraw();
    }
}

/// O que uma fonte aberta em Split View deixa na memória semântica: o título e
/// o documento, ou **nada** quando o painel é privado.
///
/// "Private/incognito navigation never enters semantic memory" está na lista de
/// restrições inegociáveis do `md/README.md`. É aqui que isso se decide para
/// este caminho, fora de qualquer janela, para um teste poder ficar vermelho se
/// alguém inverter a condição.
/// Reabrir uma aba de contexto ja gravada nao e uma fonte nova: a fonte
/// entrou na sessao e na memoria quando a aba nasceu. Sem isto, cada clique
/// A, B, A, B acrescentava outra copia a sessao persistida.
pub(in crate::windows_app) fn split_open_records_source(
    existing_context_id: Option<u64>,
    private: bool,
) -> bool {
    existing_context_id.is_none() && !private
}

/// Clicar na aba de contexto que o split ja mostra nao reconstroi o WebView:
/// reconstruir voltava a URL original da aba e perdia o que o utilizador
/// escreveu ou navegou.
pub(in crate::windows_app) fn context_tab_click_is_noop(
    active: Option<(usize, Option<u64>)>,
    source_index: usize,
    context_id: u64,
) -> bool {
    active == Some((source_index, Some(context_id)))
}

pub(in crate::windows_app) fn comparator_split_key(
    comp: &ComparatorState,
) -> Option<(usize, Option<u64>, bool)> {
    comp.split
        .as_ref()
        .map(|split| (split.source_index, split.context_id, split.private))
}

/// Que aba de contexto uma fonte aberta ao lado passa a ser. Uma fonte
/// privada nao entra na lista -- e por isso nunca chega ao `tabs.json`, nem
/// ao ecra da proxima sessao; reabrir uma aba ja gravada reutiliza a sua
/// identidade.
pub(in crate::windows_app) fn record_split_context(
    contexts: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    next_context_id: &mut u64,
    url: String,
    private: bool,
    existing_context_id: Option<u64>,
    opener: Option<u64>,
) -> Option<u64> {
    if private {
        None
    } else if let Some(id) = existing_context_id {
        Some(id)
    } else if !tab_session::storable_url(&url) {
        // Um endereco que o `tabs.json` nao guardaria (grande demais) nao vira
        // aba: a fonte abre ao lado na mesma, mas a barra e o ficheiro nunca
        // discordam -- antes a aba aparecia e sumia sem aviso no reinicio,
        // levando o grupo que so ela tinha.
        None
    } else {
        Some(remember_context_tab(
            contexts,
            groups,
            next_context_id,
            url,
            opener,
        ))
    }
}

pub(in crate::windows_app) fn split_source_memory(
    url: &Url,
    source_name: &str,
    private: bool,
) -> Option<(String, MemoryDocument)> {
    if private {
        return None;
    }
    let value = url.to_string();
    let title = url
        .host_str()
        .map(|host| format!("Fonte · {host}"))
        .unwrap_or_else(|| "Fonte Web".to_string());
    let document = MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Web,
        title.clone(),
        Some(value.clone()),
        value,
    )
    .provider(source_name);
    Some((title, document))
}

/// O painel Split como o builder o monta. O script injetado, o mapa IPC e o
/// perfil anonimo saem TODOS do mesmo `private`: o builder recebe-o uma vez
/// e nao o volta a decidir em cada sitio.
pub(in crate::windows_app) struct SplitPage {
    pub(in crate::windows_app) init_script: String,
    pub(in crate::windows_app) ipc: SplitIpc,
}

#[derive(Clone, Copy)]
pub(in crate::windows_app) struct SplitIpc {
    pub(in crate::windows_app) source_index: usize,
    pub(in crate::windows_app) private: bool,
}

impl SplitIpc {
    pub(in crate::windows_app) fn event(self, action: IpcAction) -> Option<UserEvent> {
        App::split_ipc_event_impl(self.source_index, self.private, action)
    }
}

pub(in crate::windows_app) fn split_page(
    source_index: usize,
    source_name: &str,
    capability: &str,
    private: bool,
) -> SplitPage {
    SplitPage {
        init_script: bind_page_script(
            &format!(
                "window.__neuralia_col_index = {source_index}; window.__neuralia_col_name = '{source_name}';\n{NEURALIA_KEYMAP_SCRIPT}\n{SPLIT_SCROLL_RAIL_SCRIPT}"
            ),
            capability,
            private,
        ),
        ipc: SplitIpc {
            source_index,
            private,
        },
    }
}

/// O que `open_split_mode` faz com um pedido, decidido sem janela.
pub(in crate::windows_app) enum SplitOpenPlan {
    /// O comparador ja nao esta la: abre como Web normal (nunca um privado).
    Web(String),
    Ignore,
    /// Recusado; a mensagem vai para o aviso do meio da janela.
    Refuse {
        message: &'static str,
        seconds: u64,
    },
    Build(SplitBuild),
}

/// Tudo o que o builder do Split recebe. O `private` do pedido entra UMA vez
/// (em `split_open_plan`) e daqui saem o script injetado (sem o Mandar para
/// IA nem o Traduzir), o
/// mapa IPC (que recusa `search`) e o perfil anonimo do WebView2; o builder
/// e o resto de `open_split_mode` so leem isto.
pub(in crate::windows_app) struct SplitBuild {
    pub(in crate::windows_app) url: Url,
    pub(in crate::windows_app) source_name: &'static str,
    /// A origem local que a URL digitada autorizou (ver `local_origin_of`).
    pub(in crate::windows_app) local_origin: Option<String>,
    pub(in crate::windows_app) capability: String,
    pub(in crate::windows_app) page: SplitPage,
    pub(in crate::windows_app) incognito: bool,
}

/// A decisao de `open_split_mode`, com os mesmos argumentos que ele recebe
/// (o nome da coluna de origem, se o comparador ainda a tem, e a fonte da
/// capability). E aqui que o `private` chega ao builder.
pub(in crate::windows_app) fn split_open_plan(
    surface: Surface,
    source_name: Option<&'static str>,
    source_index: usize,
    url: String,
    allow_local: bool,
    private: bool,
    capability: impl FnOnce() -> String,
) -> SplitOpenPlan {
    match App::split_request_fallback(surface, private) {
        SplitFallback::OpenSplit => {}
        SplitFallback::OpenWeb => return SplitOpenPlan::Web(url),
        SplitFallback::Ignore => return SplitOpenPlan::Ignore,
    }
    let Ok(valid) = neural_core::validate_web_url(&url) else {
        return SplitOpenPlan::Refuse {
            message: "URL da fonte inválida.",
            seconds: 3,
        };
    };
    if !allow_local && neural_core::is_local_network_target(&valid) {
        return SplitOpenPlan::Refuse {
            message: "A página não pode redirecionar a fonte para a rede local.",
            seconds: 4,
        };
    }
    let Some(source_name) = source_name else {
        return SplitOpenPlan::Ignore;
    };
    let capability = capability();
    let page = split_page(source_index, source_name, &capability, private);
    SplitOpenPlan::Build(SplitBuild {
        local_origin: allow_local.then(|| valid.origin().ascii_serialization()),
        url: valid,
        source_name,
        capability,
        incognito: page.ipc.private,
        page,
    })
}

/// O handler IPC do Split, tal como o wry o recebe: envelope autenticado
/// pela capability e depois o mapa do painel (o privado recusa `search`).
/// `send` e o proxy do event loop no app e um registo nos gates.
pub(in crate::windows_app) fn split_ipc_handler<S>(
    capability: String,
    ipc: SplitIpc,
    send: S,
) -> impl Fn(wry::http::Request<String>) + 'static
where
    S: Fn(UserEvent) + 'static,
{
    move |request| {
        let Some(action) = parse_ipc_message(request.body(), &capability, COMPARATOR_COLUMNS)
        else {
            return;
        };
        if let Some(event) = ipc.event(action) {
            send(event);
        }
    }
}

/// O que `configure_split_webview` chama no builder, com os nomes do wry. O
/// produto passa o `WebViewBuilder`; o gate passa um registo e ve o perfil,
/// o script e os handlers que o WebView2 receberia -- e chama-os.
pub(in crate::windows_app) trait SplitWebViewTarget: Sized {
    fn with_incognito(self, incognito: bool) -> Self;
    fn with_initialization_script(self, script: String) -> Self;
    fn with_ipc_handler(self, handler: impl Fn(Request<String>) + 'static) -> Self;
    fn with_navigation_handler(self, handler: impl Fn(String) -> bool + 'static) -> Self;
    /// So o endereco do popup: as `NewWindowFeatures` do wry trazem o
    /// ICoreWebView2 de quem abriu, e nao se usam.
    fn with_new_window_req_handler(
        self,
        handler: impl Fn(String) -> NewWindowResponse + 'static,
    ) -> Self;
    fn with_permission_handler(
        self,
        handler: impl Fn(PermissionKind) -> PermissionResponse + Send + Sync + 'static,
    ) -> Self;
    fn with_focused(self, focused: bool) -> Self;
}

impl SplitWebViewTarget for WebViewBuilder<'static> {
    fn with_incognito(self, incognito: bool) -> Self {
        WebViewBuilder::with_incognito(self, incognito)
    }
    fn with_initialization_script(self, script: String) -> Self {
        WebViewBuilder::with_initialization_script(self, script)
    }
    fn with_ipc_handler(self, handler: impl Fn(Request<String>) + 'static) -> Self {
        WebViewBuilder::with_ipc_handler(self, handler)
    }
    fn with_navigation_handler(self, handler: impl Fn(String) -> bool + 'static) -> Self {
        WebViewBuilder::with_navigation_handler(self, handler)
    }
    fn with_new_window_req_handler(
        self,
        handler: impl Fn(String) -> NewWindowResponse + 'static,
    ) -> Self {
        WebViewBuilder::with_new_window_req_handler(self, move |target, _features| handler(target))
    }
    fn with_permission_handler(
        self,
        handler: impl Fn(PermissionKind) -> PermissionResponse + Send + Sync + 'static,
    ) -> Self {
        WebViewBuilder::with_permission_handler(self, handler)
    }
    fn with_focused(self, focused: bool) -> Self {
        WebViewBuilder::with_focused(self, focused)
    }
}

/// Monta o WebView do Split a partir do `SplitBuild` que `split_open_plan`
/// decidiu: nao volta a decidir script, IPC nem perfil. `send` e o proxy do
/// event loop no produto e um registo no gate.
pub(in crate::windows_app) fn configure_split_webview<B, S>(
    builder: B,
    build: &SplitBuild,
    send: S,
) -> B
where
    B: SplitWebViewTarget,
    S: Fn(UserEvent) + Clone + 'static,
{
    let ipc = build.page.ipc;
    let source_index = ipc.source_index;
    let local_origin = build.local_origin.clone();
    let new_window_send = send.clone();

    builder
        .with_incognito(build.incognito)
        .with_initialization_script(build.page.init_script.clone())
        .with_ipc_handler(split_ipc_handler(build.capability.clone(), ipc, send))
        .with_navigation_handler(move |target| {
            if target
                .get(..9)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("neuralia:"))
            {
                return false;
            }
            remote_web_target(&target, local_origin.as_deref())
                || is_view_source_target(&target, local_origin.as_deref())
        })
        .with_new_window_req_handler(move |target| {
            if remote_web_target(&target, None) {
                // Um link que a fonte manda abrir noutra aba: o do Split
                // normal nasce no grupo da aba de onde saiu (2.1.7).
                let event = if ipc.private {
                    UserEvent::OpenPrivateSplit {
                        source_index,
                        url: target,
                    }
                } else {
                    UserEvent::OpenSplitFromSplit {
                        source_index,
                        url: target,
                    }
                };
                new_window_send(event);
            }
            NewWindowResponse::Deny
        })
        .with_permission_handler(|kind| web_media_permission(kind, true))
        .with_focused(true)
}
