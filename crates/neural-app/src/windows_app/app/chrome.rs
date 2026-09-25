//! Janela, omnibox, botoes de caption e saida, hover da barra, splash e cartao
//! de pesquisa, tema e dicas: os metodos do App que pintam e posicionam o
//! chrome nativo (split-windows-app-c).
use crate::windows_app::*;

impl App {
    /// Toda a navegacao passa por aqui: a thread do Reader observa este contador
    /// para saber que o resultado que esta a buscar ja nao interessa a ninguem.
    pub(in crate::windows_app) fn next_generation(&mut self) -> u64 {
        self.navigation_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(in crate::windows_app) fn current_generation(&self) -> u64 {
        self.navigation_generation.load(Ordering::SeqCst)
    }

    /// Pinta a area de cliente inteira com o fundo do tema. Corre quando a
    /// superficie muda ou a janela muda de tamanho, nunca a cada realce do rato.
    /// Marca o ecra como sujo e limpa-o JA. Nao chega agendar para o proximo
    /// `RedrawRequested`: a seguir a isto vem quase sempre a construcao de um
    /// WebView, e durante essa construcao o winit deixa de entregar redraws.
    fn mark_dirty(&mut self) {
        self.needs_clear = true;
        ERASE_PENDING.store(true, Ordering::SeqCst);
        self.clear_client();
    }

    pub(in crate::windows_app) fn clear_client(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };
        unsafe {
            let hdc = GetDC(hwnd);
            if hdc.is_null() {
                return;
            }
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let brush = CreateSolidBrush(rgb3(Theme::system().page_bg));
                FillRect(hdc, &client, brush);
                DeleteObject(brush as _);
            }
            let _ = ReleaseDC(hwnd, hdc);
        }
    }

    pub(in crate::windows_app) fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// A janela ganhou ou perdeu o foco do teclado: muda o ritmo da animacao
    /// da Home (66 ms com foco, 250 ms sem) e arruma as janelas auxiliares,
    /// que so fazem sentido enquanto a app esta a frente.
    pub(in crate::windows_app) fn on_focus_changed(&mut self, focused: bool) {
        debug_log(format_args!(
            "focus={focused} surface={:?} home_focused={}",
            self.surface, self.home_focused
        ));
        if self.home_focused == focused {
            return;
        }
        self.home_focused = focused;
        if focused {
            self.show_unseen_phase_end();
            self.resume_home_animation();
            // Os popups owned reaparecem com o dono, mas a geometria pode ter
            // mudado enquanto estivemos fora (outro ecra, outro DPI, outra
            // maximizacao), por isso recalcula-se em vez de se confiar nela.
            self.sync_comparator_splitters();
            self.sync_exit_button();
            self.sync_caption_buttons();
            self.sync_panel_handle();
            return;
        }
        // Sem foco nao ha o que arrastar nem de onde sair: as auxiliares que
        // so servem o rato saem da frente ate a janela voltar.
        self.hide_comparator_splitters();
        self.hide_exit_button();
        self.hide_panel_handle();
    }

    /// A janela ficou inteiramente tapada (ou deixou de estar). Enquanto esta
    /// tapada nao se pinta nada.
    pub(in crate::windows_app) fn on_occluded_changed(&mut self, occluded: bool) {
        if self.home_occluded == occluded {
            return;
        }
        self.home_occluded = occluded;
        if !occluded {
            // Restaurada da barra de tarefas: o foco pode ir direto para a
            // WebView e o `Focused` da janela nunca chegar.
            self.show_unseen_phase_end();
            self.resume_home_animation();
        }
    }

    /// Ao voltar a ser vista, a Home repinta ja: o prazo do frame seguinte
    /// pode ter ficado a 250 ms de distancia, e esperar por ele daria a
    /// sensacao de uma janela congelada.
    fn resume_home_animation(&mut self) {
        if self.surface != Surface::Home {
            return;
        }
        self.next_home_frame = Instant::now();
        self.request_redraw();
    }

    /// Reinstala a subclasse da janela principal depois de transições de
    /// decoração. No Windows, alternar a moldura pode substituir o HWND nativo;
    /// SetWindowSubclass é idempotente para o mesmo callback/id e atualiza o
    /// reference_data quando a janela continua a mesma.
    pub(in crate::windows_app) fn ensure_window_subclass(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(parent) = window_hwnd(window) else {
            return;
        };
        let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
        unsafe {
            if SetWindowSubclass(parent, Some(window_subclass), WINDOW_SUBCLASS_ID, proxy_ptr) == 0
            {
                eprintln!("failed to subclass effective NeuralIA HWND");
            }

            // A troca de decorations pode substituir/reparentar o HWND nativo.
            // A omnibox e o Home sao filhos Win32 reais: se continuarem ligados
            // ao HWND antigo, ficam invisiveis ou deixam de receber teclado/rato.
            for child in [self.omnibox, self.home_button].into_iter().flatten() {
                if GetParent(child) != parent {
                    SetParent(child, parent);
                    if GetParent(child) != parent {
                        eprintln!("failed to reparent native NeuralIA control to effective HWND");
                    }
                }
            }
        }
    }

    pub(in crate::windows_app) fn create_omnibox(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(parent) = window_hwnd(window) else {
            return;
        };

        unsafe {
            // Sem WS_EX_CLIENTEDGE: a moldura afundada e quadrada e nao ha
            // forma de a arredondar. A borda visivel passa a ser a pilula.
            let edit = CreateWindowExW(
                0,
                windows_sys::w!("EDIT"),
                windows_sys::w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                0,
                0,
                100,
                32,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if edit.is_null() {
                return;
            }

            let cue: Vec<u16> = "Pergunte algo ou cole uma URL"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            SendMessageW(edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
            SendMessageW(edit, EM_SETLIMITTEXT, 2048, 0);

            let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
            if SetWindowSubclass(edit, Some(omnibox_subclass), OMNIBOX_SUBCLASS_ID, proxy_ptr) == 0
            {
                DestroyWindow(edit);
                return;
            }

            if SetWindowSubclass(parent, Some(window_subclass), WINDOW_SUBCLASS_ID, proxy_ptr) == 0
            {
                eprintln!("failed to subclass NeuralIA parent HWND while creating omnibox");
            }

            self.omnibox = Some(edit);
            self.position_omnibox();
            SetFocus(edit);
        }
    }

    pub(in crate::windows_app) fn position_omnibox(&mut self) {
        let (Some(window), Some(edit)) = (&self.window, self.omnibox) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);

        // O EDIT precisa manter a mesma identidade Win32 durante as trocas de
        // decorations/HWND. Fora da Home ele continua WS_VISIBLE, mas fica
        // estacionado muito fora do cliente e com 1x1 px: não aparece na
        // titlebar nem disputa espaço com as abas.
        let inner = if self.surface == Surface::Home {
            let layout =
                HomeLayout::new(size.width as f64, size.height as f64, window.scale_factor());
            let pad_x = 22.0 * scale;
            let pad_y = 5.0 * scale;
            UiRect {
                x: layout.input.x + pad_x,
                y: layout.input.y + pad_y,
                width: (layout.input.width - pad_x * 2.0).max(1.0),
                height: (layout.input.height - pad_y * 2.0).max(1.0),
            }
        } else {
            UiRect {
                x: -4096.0 * scale,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }
        };

        unsafe {
            SetWindowPos(
                edit,
                std::ptr::null_mut(),
                inner.x.round() as i32,
                inner.y.round() as i32,
                inner.width.round() as i32,
                inner.height.round() as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            apply_omnibox_interactivity(edit, self.surface);
            ShowWindow(edit, SW_SHOW);
        }
        if self.surface == Surface::Home {
            self.apply_omnibox_font(inner.height);
        }
        self.needs_clear = true;
        self.request_redraw();
    }

    /// Fonte proporcional a altura da barra: acompanha o tamanho da caixa e o DPI.
    fn apply_omnibox_font(&mut self, box_height: f64) {
        let Some(edit) = self.omnibox else {
            return;
        };
        let height = -((box_height * 0.58).round() as i32).clamp(18, 80);
        if self.omnibox_font_height == height && self.omnibox_font.is_some() {
            return;
        }

        unsafe {
            let font = create_font(height, FW_NORMAL as i32);
            if font.is_null() {
                return;
            }
            SendMessageW(edit, WM_SETFONT, font as usize, 1);
            if let Some(previous) = self.omnibox_font.replace(font) {
                DeleteObject(previous as _);
            }
            self.omnibox_font_height = height;

            // O espacamento ja vem do encaixe dentro da pilula.
            let margin = 0usize;
            SendMessageW(
                edit,
                EM_SETMARGINS,
                EC_LEFTMARGIN | EC_RIGHTMARGIN,
                ((margin << 16) | margin) as isize,
            );
        }
    }

    /// Unica saida de qualquer superficie web. Leva o comparador junto: era
    /// aqui que os tres WebViews sobreviviam ao regresso a Home e o contador
    /// podia chegar a quatro somando o Full Web.
    /// Sai do ecra completo. Faltava em quase todas as saidas: bastava um login
    /// ou um Esc para a janela ficar sem barra de titulo e sem forma de voltar.
    pub(in crate::windows_app) fn leave_fullscreen(&mut self) {
        if let Some(window) = &self.window {
            window.set_fullscreen(None);
        }
    }

    pub(in crate::windows_app) fn destroy_web_surfaces(&mut self) {
        if lifecycle_probe_enabled() {
            LIFECYCLE_COMPARATOR_READY.store(false, Ordering::Release);
            LIFECYCLE_HOME_READY.store(false, Ordering::Release);
        }
        self.mark_dirty();
        self.close_palette();
        self.finish_agent(AgentTermination::UserStopped);

        // Derruba as superfícies WebView ANTES de alterar fullscreen/decoração.
        // No Windows, essas transições podem substituir ou reparentear o HWND
        // principal. Fazer a troca de chrome com controllers ainda vivos deixa
        // hosts WRY_WEBVIEW da segunda abertura presos ao HWND anterior e eles
        // reaparecem sobre a Home mesmo depois do drop.
        //
        // As abas e os grupos morrem com o comparador: gravam-se antes, para
        // a proxima pesquisa (ou o proximo arranque) os trazer de volta.
        let _ = self.save_tab_session();
        if let Some(comparator) = self.comparator.take() {
            for view in &comparator.views {
                let _ = view.webview.set_visible(false);
                let _ = view.webview.focus_parent();
            }
            if let Some(split) = &comparator.split {
                let _ = split.webview.set_visible(false);
                let _ = split.webview.focus_parent();
            }
            drop(comparator);
        }
        if let Some(webview) = self.webview.take() {
            let _ = webview.set_visible(false);
            let _ = webview.focus_parent();
            drop(webview);
        }
        // Os paineis da direita tambem sao superficies web e nao sobrevivem a
        // esta saida. O do Gemini Live em especial: so escondido pelo
        // `hide_orphaned_wry_hosts` la em baixo, continuava vivo a mandar a
        // tela, a camera e o microfone ao Google, sem o olho vermelho (a barra
        // so se pinta no comparador) e sem o botao Desligar -- bastava um erro
        // nativo (`show_native_error`) ou um link para a Web completa.
        self.close_live_panel();
        self.close_service_panel();
        // O do Ctrl+H pela saida unica: o texto de uma nota a meio vai para
        // o disco antes de a pagina sair (gate
        // `every_way_out_of_the_side_panel_saves_the_note_being_typed_once`).
        // `SurfaceChange` nao mexe no teclado: esta troca trata dele.
        self.close_side_panel(PanelExit::SurfaceChange);
        // Sem painel: a pega some e o gancho da roda sai.
        self.after_panel_change();

        if let Some(button) = self.exit_button.take() {
            unsafe {
                DestroyWindow(button);
            }
        }
        if let Some(button) = self.home_button.take() {
            unsafe {
                DestroyWindow(button);
            }
        }
        if let Some(buttons) = self.caption_buttons.take() {
            unsafe {
                DestroyWindow(buttons);
            }
        }
        for splitter in &mut self.splitters {
            if let Some(hwnd) = splitter.take() {
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }

        // O drop dos controllers pode concluir a destruição dos HWNDs WRY no
        // pump de mensagens seguinte. Restaurar a decoração aqui, no mesmo
        // stack, pode reparentear esses hosts para o novo HWND da Home e deixá-los
        // visíveis a partir da segunda abertura. Deixe o event loop respirar
        // antes de trocar o chrome nativo.
        self.leave_fullscreen();

        // Teardown e transicao para Home sao coisas diferentes. Antes esta
        // funcao sempre agendava RestoreHomeDecorations; quando um novo
        // comparador era aberto a partir da propria Home, esse timer podia
        // disparar dentro do pump aninhado de build_as_child e recolocar a
        // moldura da Home no meio da criacao dos tres WRY_WEBVIEW. O resultado
        // era exatamente a regressao dos ciclos 2+: hosts visiveis presos ao
        // HWND errado. Aqui so destruimos. Quem realmente entra na Home agenda
        // a restauracao depois.
        if let Some(window) = &self.window {
            hide_orphaned_wry_hosts(window);
        }

        if let Ok(mut bytes) = self.pdf_bytes.lock() {
            *bytes = Vec::new();
        }
        self.reading_pdf = false;
        self.page_source = None;
    }

    pub(in crate::windows_app) fn schedule_home_restoration(&self) {
        if lifecycle_probe_enabled() {
            LIFECYCLE_HOME_READY.store(false, Ordering::Release);
        }
        self.timers
            .after(Duration::from_millis(40), UserEvent::RestoreHomeDecorations);
    }

    pub(in crate::windows_app) fn show_home(&mut self) {
        debug_log(format_args!("show_home (surface era {:?})", self.surface));
        self.caption_reveal.reset();
        self.close_side_panel(PanelExit::Home);
        self.close_service_panel();
        self.close_live_panel();
        self.next_generation();
        self.surface = Surface::Home;

        // Home é uma fronteira de ciclo de vida real. Destruir os controllers
        // aqui garante que nenhum host WRY_WEBVIEW sobreviva oculto/reparentado
        // entre pesquisas. Reuso dentro do próprio comparador continua possível,
        // mas sair para Home sempre encerra as superfícies web.
        self.destroy_web_surfaces();
        self.schedule_home_restoration();

        self.bar_hover = None;
        self.forget_tab_gesture();
        self.status = None;
        self.next_home_frame = Instant::now();
        self.show_omnibox(true);
        self.position_omnibox();
        self.request_redraw();
    }

    pub(in crate::windows_app) fn show_native_error(&mut self, message: impl Into<String>) {
        self.next_generation();
        self.destroy_web_surfaces();
        self.surface = Surface::Home;
        self.schedule_home_restoration();
        self.status = Some(message.into());
        self.show_omnibox(true);
        self.position_omnibox();
        self.request_redraw();
    }

    /// `show_home` reescreve o estado, por isso a mensagem tem de vir depois
    /// dele — antes desta correcao o aviso de apagado nunca chegava a aparecer.
    pub(in crate::windows_app) fn report_history_cleared(&mut self, result: Result<(), String>) {
        self.show_home();
        self.status = Some(match result {
            Ok(()) => "Histórico local apagado.".to_string(),
            Err(error) => format!("Não foi possível apagar o histórico: {error}"),
        });
        self.request_redraw();
    }

    pub(in crate::windows_app) fn show_native_text(&self, title: &str, text: &str) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };
        let body = wide_null(text);
        let title = wide_null(title);
        unsafe {
            MessageBoxW(
                hwnd,
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    pub(in crate::windows_app) fn show_memory_results(
        &self,
        query: &str,
        result: Result<Vec<MemoryHit>, String>,
    ) {
        let text = match result {
            Err(error) => format!("Não foi possível consultar a memória: {error}"),
            Ok(hits) if hits.is_empty() => {
                if query.trim().is_empty() {
                    "Memória semântica vazia.".to_string()
                } else {
                    format!("Nenhum resultado para \"{query}\".")
                }
            }
            Ok(hits) => hits
                .into_iter()
                .enumerate()
                .map(|(index, hit)| {
                    let source = hit
                        .provider
                        .as_deref()
                        .or(hit.url.as_deref())
                        .unwrap_or("local");
                    let via = hit.matched_by.join("+");
                    format!(
                        "{}. {}\r\n   {} · {}\r\n   {}{}",
                        index + 1,
                        hit.title,
                        source,
                        via,
                        hit.excerpt,
                        hit.url
                            .as_deref()
                            .map(|url| format!("\r\n   {url}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\r\n\r\n"),
        };
        self.show_native_text("NeuralIA — Memória semântica", &text);
    }

    /// Aviso flutuante, centrado na janela, que se apaga sozinho: a resposta
    /// a um gesto do utilizador (`SplashKind::Notice`).
    pub(in crate::windows_app) fn show_splash(&mut self, text: String, seconds: u64) {
        if let Some(frame) = self.splash_board.show(text, seconds, SplashKind::Notice) {
            self.present_splash(frame);
        }
    }

    /// Aviso que chega sozinho, sem gesto nenhum (o fim de uma fase do
    /// Pomodoro): com a pergunta da rolagem a vista, espera por ela.
    pub(in crate::windows_app) fn show_background_splash(&mut self, text: String, seconds: u64) {
        if let Some(frame) = self
            .splash_board
            .show(text, seconds, SplashKind::Background)
        {
            self.present_splash(frame);
        }
    }

    /// Poe `frame` no popup (criando-o se preciso) e agenda o fim dele.
    pub(in crate::windows_app) fn present_splash(&mut self, frame: SplashFrame) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SPLASH_WIDTH * scale).round() as i32;
        let height = (SPLASH_HEIGHT * scale).round() as i32;

        // A dica do rato e o aviso nascem os dois no centro da janela: com a
        // dica viva do Pomodoro por baixo (refrescada a cada segundo), um
        // fim de fase empilhava duas mensagens no mesmo sitio. A dica sai;
        // escondida, `refresh_hint_text` ja nao a traz de volta.
        hover_tooltip(std::ptr::null_mut(), "");
        let SplashFrame {
            text,
            asks,
            seconds,
            token,
        } = frame;
        SPLASH_ASKS.store(asks, Ordering::SeqCst);
        if let Ok(mut slot) = SPLASH_TEXT.lock() {
            *slot = text;
        }

        if self.splash.is_none() {
            unsafe {
                // Popup OWNED pela janela principal (`owner` em hWndParent), e
                // nao filha nem TOPMOST. Uma janela owned fica sempre acima do
                // dono e das filhas dele -- o WebView2 incluido -- e some com
                // ele quando a app vai para tras; o TOPMOST que aqui estava
                // punha este aviso por cima de TODAS as aplicacoes depois de
                // um Alt+Tab, que nunca foi o que se queria.
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
                let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                if SetWindowSubclass(
                    created,
                    Some(splash_subclass),
                    SPLASH_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, height, height);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.splash = Some(created);
            }
        }

        self.position_splash();

        self.timers
            .after(Duration::from_secs(seconds), UserEvent::HideSplash(token));
    }

    /// Centra o aviso no fundo da janela. Vive em coordenadas de ECRA: se
    /// so se calculasse ao nascer, arrastar a janela deixava-o para tras.
    pub(in crate::windows_app) fn position_splash(&self) {
        let (Some(window), Some(splash)) = (&self.window, self.splash) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SPLASH_WIDTH * scale).round() as i32;
        let height = (SPLASH_HEIGHT * scale).round() as i32;

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let (x, y) = splash_origin(client.right, client.bottom, width, height);
            SetWindowPos(
                splash,
                std::ptr::null_mut(),
                origin.x + x,
                origin.y + y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(splash);
            InvalidateRect(splash, std::ptr::null(), 1);
        }
    }

    pub(in crate::windows_app) fn hide_splash(&mut self, token: u64) {
        let SplashHide::Hide {
            question_expired,
            next,
        } = self.splash_board.hide(token)
        else {
            return;
        };
        SPLASH_ASKS.store(false, Ordering::SeqCst);
        // So o fim do quadro da PROPRIA pergunta e um "nao" (ver
        // `SplashBoard`); um aviso que a substituiu nao responde nada.
        if question_expired {
            self.auto_scroll_answered = true;
            self.auto_scroll.set(false);
        }
        if let Some(splash) = self.splash.take() {
            unsafe {
                DestroyWindow(splash);
            }
        }
        if let Some(frame) = next {
            self.present_splash(frame);
        }
    }

    /// Centra o cartao de pesquisa na janela. Em coordenadas de ECRA, como o
    /// splash: refaz-se quando a janela se mexe.
    pub(in crate::windows_app) fn position_search_card(&self) {
        let (Some(window), Some(card)) = (&self.window, self.search_card_popup) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (SEARCH_CARD_WIDTH * scale).round() as i32;
        let height = (SEARCH_CARD_HEIGHT * scale).round() as i32;
        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let (x, y) = splash_origin(client.right, client.bottom, width, height);
            SetWindowPos(
                card,
                std::ptr::null_mut(),
                origin.x + x,
                origin.y + y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(card);
            InvalidateRect(card, std::ptr::null(), 1);
        }
    }

    fn is_fullscreen_column(&self) -> bool {
        false
    }

    /// O chrome permanece visível também quando uma IA ocupa toda a área de
    /// conteúdo. "Expandir" não significa tomar o monitor inteiro.
    pub(in crate::windows_app) fn bar_visible(&self) -> bool {
        self.comparator.is_some()
    }

    pub(in crate::windows_app) fn bar_layout(&self) -> Option<BarLayout> {
        let (Some(window), Some(comp)) = (&self.window, &self.comparator) else {
            return None;
        };
        // O mesmo plano que o desenho usa. Se aqui se contassem so as abas, o
        // rato acertaria noutro sitio que nao o que esta no ecra.
        Some(BarLayout::with_rows(
            window.inner_size().width as f64,
            window.scale_factor(),
            self.bar_visible(),
            bar_columns(comp, self.pomodoro_bar_label()),
            tab_rows_focused(
                &comp.contexts,
                &comp.groups,
                active_context(comp),
                comp.bar_focus,
            ),
        ))
    }

    /// Home nativo da barra. O desenho da barra continua existindo por baixo,
    /// mas o clique pertence a uma janela Win32 real, acima de qualquer filho
    /// WebView2. Assim o controlo nao depende do foco nem da entrega de eventos
    /// do winit para voltar à Home.
    pub(in crate::windows_app) fn sync_home_button(&mut self) {
        let wanted = self.surface == Surface::Comparator && !self.is_fullscreen_column();
        if !wanted {
            if let Some(button) = self.home_button.take() {
                unsafe {
                    DestroyWindow(button);
                }
            }
            return;
        }

        let (Some(window), Some(layout)) = (&self.window, self.bar_layout()) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let rect = layout.home;
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        if self.home_button.is_none() {
            unsafe {
                let created = CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("NeuralIA.Home"),
                    WS_CHILD | WS_VISIBLE,
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    rect.width.round() as i32,
                    rect.height.round() as i32,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if created.is_null() {
                    return;
                }
                let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                if SetWindowSubclass(
                    created,
                    Some(home_button_subclass),
                    HOME_BUTTON_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let width = rect.width.round() as i32;
                let height = rect.height.round() as i32;
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, height, height);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.home_button = Some(created);
            }
        }

        if let Some(button) = self.home_button {
            unsafe {
                SetWindowPos(
                    button,
                    std::ptr::null_mut(),
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    rect.width.round() as i32,
                    rect.height.round() as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                ShowWindow(button, SW_SHOW);
                InvalidateRect(button, std::ptr::null(), 1);
            }
        }
    }

    /// Controles de janela reais acima dos filhos WebView2. Se a troca de
    /// decoracao produzir outro HWND, o controlo e destruido e recriado no
    /// novo pai; nao se usa SetParent neste overlay.
    pub(in crate::windows_app) fn sync_caption_buttons(&mut self) {
        let wanted = caption_buttons_wanted(self.surface, self.bar_visible());
        if !wanted {
            if let Some(buttons) = self.caption_buttons.take() {
                unsafe {
                    DestroyWindow(buttons);
                }
            }
            return;
        }

        let Some(window) = &self.window else {
            return;
        };
        // Na Home nao ha barra do comparador, mas os botoes da janela ficam no
        // mesmo sitio: a geometria deles so depende da largura.
        let area = match self.bar_layout() {
            Some(layout) if self.surface == Surface::Comparator => caption_area(&layout),
            _ => home_caption_rect(window.inner_size().width as f64, window.scale_factor()),
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let left = area.x;
        let width = area.width;
        let height = area.height;

        if let Some(buttons) = self.caption_buttons
            && unsafe { GetParent(buttons) } != owner
        {
            unsafe {
                DestroyWindow(buttons);
            }
            self.caption_buttons = None;
        }

        let visible = caption_buttons_visible(
            self.surface,
            self.caption_reveal.shown(),
            self.service_covers_window(),
        );
        if self.caption_buttons.is_none() {
            unsafe {
                // Nasce escondida na Home: aparecer e desaparecer logo a
                // seguir era um piscar no canto a cada regresso a Home.
                let created = CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("NeuralIA.CaptionControls"),
                    if visible {
                        WS_CHILD | WS_VISIBLE
                    } else {
                        WS_CHILD
                    },
                    left.round() as i32,
                    0,
                    width.round() as i32,
                    height.round() as i32,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if created.is_null() {
                    return;
                }
                let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                if SetWindowSubclass(
                    created,
                    Some(caption_buttons_subclass),
                    CAPTION_BUTTONS_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                self.caption_buttons = Some(created);
            }
        }

        if let Some(buttons) = self.caption_buttons {
            place_caption_buttons(
                buttons,
                left.round() as i32,
                width.round() as i32,
                height.round() as i32,
                visible,
            );
        }
    }

    /// Onde estao os tres botoes da janela na Home, em pixels do cliente.
    fn home_caption_area(&self) -> Option<Area> {
        let window = self.window.as_ref()?;
        Some(home_caption_rect(
            window.inner_size().width as f64,
            window.scale_factor(),
        ))
    }

    /// Na Home: os botoes aparecem quando o rato entra na zona deles e
    /// somem 300 ms depois de ele sair (CaptionReveal). A posicao do rato e
    /// lida ao Windows, nao ao ultimo CursorMoved: por cima dos proprios
    /// botoes (outra janela) ou fora da janela a janela principal nao ve
    /// movimento nenhum.
    pub(in crate::windows_app) fn refresh_caption_reveal(&mut self) {
        if self.surface != Surface::Home {
            return;
        }
        let (Some(window), Some(area)) = (&self.window, self.home_caption_area()) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let mut point = POINT { x: 0, y: 0 };
        let inside =
            unsafe { GetCursorPos(&mut point) != 0 && ScreenToClient(owner, &mut point) != 0 }
                && caption_hot_zone(area, CAPTION_HOT_MARGIN * scale)
                    .contains(point.x as f64, point.y as f64);
        match self.caption_reveal.observe(inside, now_ms()) {
            RevealStep::Show | RevealStep::Hide => self.sync_caption_buttons(),
            RevealStep::ScheduleHide(delay) => self
                .timers
                .after(Duration::from_millis(delay), UserEvent::CaptionReveal),
            RevealStep::Nothing => {}
        }
    }

    /// Cria/mostra/esconde o botao flutuante de saida. Existe apenas enquanto
    /// houver uma coluna em ecra completo -- e a unica saida sempre visivel,
    /// porque a barra de titulo desapareceu e a barra da app auto-esconde-se.
    pub(in crate::windows_app) fn sync_exit_button(&mut self) {
        // Em fullscreen e o controlo nativo permanente de saida. Nao depende
        // de hover nem de redimensionar o WebView.
        let wanted = (self.surface == Surface::Comparator && self.is_fullscreen_column())
            || self.service_frame().is_some_and(|frame| frame.exit_button);

        if !wanted {
            if let Some(button) = self.exit_button.take() {
                unsafe {
                    DestroyWindow(button);
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
        let scale = window.scale_factor().max(1.0);
        let width = (EXIT_BUTTON_WIDTH * scale).round() as i32;
        let height = (EXIT_BUTTON_HEIGHT * scale).round() as i32;

        if self.exit_button.is_none() {
            unsafe {
                // Popup owned pela janela principal, e nao filha: uma filha
                // ficaria por baixo do WebView2 na ordem Z e nunca se veria.
                // Ser owned ja garante o lugar acima do dono e das filhas
                // dele; o TOPMOST so acrescentava ficar por cima das outras
                // aplicacoes depois de um Alt+Tab.
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
                let proxy_ptr = (&*self.omnibox_proxy as *const EventLoopProxy<UserEvent>) as usize;
                if SetWindowSubclass(
                    created,
                    Some(exit_button_subclass),
                    EXIT_BUTTON_SUBCLASS_ID,
                    proxy_ptr,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, height, height);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.exit_button = Some(created);
            }
        }

        self.position_exit_button();
    }

    /// Centrado no topo, logo abaixo da barra revelada. Em coordenadas de
    /// ecra, como as outras auxiliares: tem de seguir a janela que se arrasta.
    pub(in crate::windows_app) fn position_exit_button(&self) {
        let (Some(window), Some(button)) = (&self.window, self.exit_button) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (EXIT_BUTTON_WIDTH * scale).round() as i32;
        let height = (EXIT_BUTTON_HEIGHT * scale).round() as i32;

        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let top = ((COMPARATOR_CHROME_HEIGHT + 10.0) * scale).round() as i32;
            SetWindowPos(
                button,
                std::ptr::null_mut(),
                origin.x + (client.right - width) / 2,
                origin.y + top,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(button);
        }
    }

    /// Esconde o botao sem o destruir. Serve a perda de foco: a janela volta
    /// e o `sync_exit_button` decide de novo, sem recriar nada entretanto.
    fn hide_exit_button(&self) {
        if let Some(button) = self.exit_button {
            unsafe {
                ShowWindow(button, SW_HIDE);
            }
        }
    }

    /// Controlos da direita tal como estao desenhados AGORA, ou `None` se a
    /// barra nao estiver a ser mostrada. A geometria vem toda de
    /// `right_controls`: nao ha uma segunda copia da conta por aqui.
    fn right_controls(&self) -> Option<RightControls> {
        let window = self.window.as_ref()?;
        if self.surface != Surface::Comparator || !self.bar_visible() {
            return None;
        }
        let split_active = self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some());
        Some(right_controls(
            window.inner_size().width as f64,
            window.scale_factor().max(1.0),
            split_active,
            self.pomodoro_bar_label(),
        ))
    }

    /// A faixa do painel de servicos, em pixels do cliente (os do rato).
    pub(in crate::windows_app) fn service_strip_physical(&self) -> Option<Area> {
        let strip = self.service_frame()?.strip?;
        let scale = self.window.as_ref()?.scale_factor().max(1.0);
        Some(Area {
            x: strip.x * scale,
            y: strip.y * scale,
            width: strip.width * scale,
            height: strip.height * scale,
        })
    }

    pub(in crate::windows_app) fn comparator_bar_hit(&self) -> Option<BarHit> {
        if let Some(strip) = self.service_strip_physical() {
            let scale = self
                .window
                .as_ref()
                .map_or(1.0, |window| window.scale_factor().max(1.0));
            if let Some(button) = strip_hit(strip, scale, self.cursor.0, self.cursor.1) {
                return Some(BarHit::ServiceStrip(button));
            }
        }
        bar_hit_at(
            self.right_controls(),
            self.bar_layout(),
            self.cursor.0,
            self.cursor.1,
        )
    }

    /// Ferramentas da Home sob o rato: realce e dica, como na barra.
    pub(in crate::windows_app) fn update_home_tool_hover(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        if self.surface != Surface::Home {
            // Fora da Home a dica e da barra (`update_bar_hover` correu antes
            // neste mesmo movimento): so se esquece o realce, sem a apagar.
            self.home_tool_hover = None;
            return;
        }
        let next = home_tool_hit(
            window.inner_size().width as f64,
            window.scale_factor(),
            self.pomodoro_bar_label(),
            self.cursor.0,
            self.cursor.1,
        );
        if next == self.home_tool_hover {
            return;
        }
        self.home_tool_hover = next;
        if let Some(owner) = window_hwnd(window) {
            let text = next.map(|tool| tool_hint_at(tool, &self.pomodoro, Instant::now()));
            hover_tooltip(owner, text.as_deref().unwrap_or(""));
        }
        self.request_redraw();
    }

    /// "Ir" da Home sob o rato: degradê e mao, para se ver que esta vivo e
    /// responde ao clique. Fora da Home volta tudo ao normal.
    pub(in crate::windows_app) fn update_home_go_hover(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let hovered = home_go_hovered(
            self.surface,
            (size.width as f64, size.height as f64),
            window.scale_factor(),
            self.cursor,
        );
        if hovered == self.home_go_hover {
            return;
        }
        self.home_go_hover = hovered;
        window.set_cursor(if hovered {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        });
        self.request_redraw();
    }

    /// A dica centrada de um controlo injetado numa coluna. O texto e o nome
    /// da IA saem daqui; a pagina so disse qual dos controlos tem o rato.
    pub(in crate::windows_app) fn show_column_hint(&mut self, col: usize, hint: ColumnHint) {
        let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        match column_hint_step(self.column_hint, col, hint, latest_tooltip_request()) {
            ColumnHintStep::Show => {
                let Some(provider) = self
                    .comparator
                    .as_ref()
                    .filter(|_| self.surface == Surface::Comparator)
                    .and_then(|comp| comp.views.get(col))
                    .map(|view| view.name)
                else {
                    return;
                };
                let request = hover_tooltip(owner, &column_hint_text(hint, provider));
                self.column_hint = Some(ColumnHintOwner { col, request });
            }
            ColumnHintStep::Clear => {
                hover_tooltip(owner, "");
                self.column_hint = None;
            }
            ColumnHintStep::Keep => {}
        }
    }

    pub(in crate::windows_app) fn update_bar_hover(&mut self) {
        let next = self.comparator_bar_hit();
        if next != self.bar_hover {
            self.bar_hover = next;
            self.request_redraw();
            if let Some(owner) = self.window.as_ref().and_then(window_hwnd) {
                let text = next.and_then(|hit| self.bar_tooltip_text(hit, owner));
                hover_tooltip(owner, text.as_deref().unwrap_or(""));
            }
        }
    }

    /// Tema novo (mudou no Windows ou foi escolhido): barra, botoes nativos,
    /// popups auxiliares e paginas. Antes, na mudanca do Windows, so a barra
    /// se redesenhava e os botoes nativos ficavam com as cores velhas.
    pub(in crate::windows_app) fn refresh_theme(&mut self) {
        use windows_sys::Win32::Graphics::Gdi::{
            RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow,
        };
        use wry::WebViewExtWindows;
        Theme::invalidate();
        self.needs_clear = true;
        self.request_redraw();
        if let Some(owner) = self.window.as_ref().and_then(window_hwnd) {
            unsafe {
                RedrawWindow(
                    owner,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                );
            }
        }
        // Os popups owned nao sao filhos: o RDW_ALLCHILDREN nao os alcanca.
        for hwnd in self.splitters.iter().flatten() {
            unsafe {
                InvalidateRect(*hwnd, std::ptr::null(), 1);
            }
        }
        self.panel_eval(&format!(
            "window.__neuraliaPanel && window.__neuraliaPanel.theme({});",
            panel_theme_vars(&Theme::system())
        ));
        self.live_eval(&live_theme_script(&panel_theme_vars(&Theme::system())));
        let theme = ThemeChoice::current().webview_theme();
        // O tema "Sistema" do leitor de livros acompanha o tema da app.
        if self.surface == Surface::Epub
            && let Some(webview) = &self.webview
        {
            let _ = webview.set_theme(theme);
        }
        if let Some(comp) = &self.comparator {
            for view in &comp.views {
                let _ = view.webview.set_theme(theme);
            }
            if let Some(split) = &comp.split {
                let _ = split.webview.set_theme(theme);
            }
        }
    }

    /// A dica do alvo `hit`, com o nome da IA, o endereco da aba ou o estado
    /// do grupo que o clique vai usar.
    fn bar_tooltip_text(&self, hit: BarHit, owner: HWND) -> Option<String> {
        // A faixa e o botao do servico aberto falam do servico e do modo dele.
        if let Some(panel) = &self.service_panel
            && let Some(hint) =
                service_panel_hint(hit, panel.service, panel.state.badge(panel.audio))
        {
            return Some(hint);
        }
        let comp = self.comparator.as_ref();
        let column = match hit {
            BarHit::Column(index)
            | BarHit::AddTab(index)
            | BarHit::ColumnBack(index)
            | BarHit::ColumnForward(index)
            | BarHit::TabOverflow(index) => Some(index),
            BarHit::ContextTab { source_index, .. }
            | BarHit::CloseTab { source_index, .. }
            | BarHit::ContextGroup { source_index, .. } => Some(source_index),
            _ => None,
        };
        let provider = column
            .and_then(|index| comp.and_then(|comp| comp.views.get(index)))
            .map_or("IA", |view| view.name);
        let tab_url = match hit {
            BarHit::ContextTab {
                source_index,
                context_index,
            } => comp
                .and_then(|comp| comp.contexts.get(source_index))
                .and_then(|tabs| tabs.get(context_index))
                .map(|tab| tab.url.as_str()),
            _ => None,
        };
        // O que o clique na pilula vai fazer (o mesmo `chip_click` do
        // clique): uma pilula sozinha, cujas abas ficaram fora do corte,
        // mostra-as -- nao diz "recolher".
        let group = match hit {
            BarHit::ContextGroup {
                source_index,
                group_index,
            } => {
                let drawn = self
                    .bar_layout()
                    .is_some_and(|layout| layout.group_members_drawn(source_index, group_index));
                comp.and_then(|comp| comp.groups.get(source_index))
                    .and_then(|groups| groups.get(group_index))
                    .map(|group| {
                        (
                            group.name.as_str(),
                            chip_click(group.collapsed, drawn) != ChipClick::Collapse,
                        )
                    })
            }
            _ => None,
        };
        let maximized = unsafe { IsZoomed(owner) != 0 };
        bar_hint(
            hit,
            &self.pomodoro,
            Instant::now(),
            provider,
            maximized,
            tab_url,
            group,
        )
    }
}
