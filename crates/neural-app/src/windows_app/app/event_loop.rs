//! O event loop do winit: resumed, about_to_wait, exiting, user_event e
//! window_event, cada braco como estava na raiz (split-windows-app-c).
use crate::windows_app::*;

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        match event_loop.create_window(main_window_attributes()) {
            Ok(window) => {
                window.set_ime_allowed(true);
                self.window = Some(window);
                self.create_omnibox();
                self.sync_caption_buttons();
                self.request_redraw();
                // Spike de aceleradores (so no build de CI com a feature):
                // antes do SubmitText, para a pergunta da rolagem ja estar
                // respondida quando o comparador abrir.
                #[cfg(feature = "accel-spike")]
                self.accel_spike_start();

                // Abertura: a consulta padrao ja entra na omnibox e vai direto
                // para a tela de resultados, sem esperar Enter do utilizador.
                let startup = startup_input();
                if !startup.is_empty() {
                    self.set_omnibox_text(&startup);
                    debug_log(format_args!(
                        "startup: SubmitText ({} chars)",
                        startup.chars().count()
                    ));
                    let _ = self.proxy.send_event(UserEvent::SubmitText(startup));
                }
            }
            Err(error) => {
                eprintln!("window creation failed: {error}");
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // set_decorations pode trocar/reparentear o HWND depois do callback que
        // pediu a mudança. Este é o primeiro ponto garantido depois de cada lote
        // de eventos, já fora do pump aninhado do WebView2. Reinstalar a
        // subclass aqui é idempotente e garante que Home, atalhos e o probe
        // continuem chegando à janela REAL também na segunda abertura.
        self.ensure_window_subclass();

        // Os `DroppedFile` de um mesmo gesto chegam no mesmo lote: seguem
        // juntos, e so o ultimo livro adicionado abre.
        if !self.pending_drops.is_empty() {
            let dropped = std::mem::take(&mut self.pending_drops);
            self.route_dropped_files(dropped);
        }

        if lifecycle_probe_enabled()
            && self.surface == Surface::Comparator
            && self.comparator.is_some()
            && !LIFECYCLE_COMPARATOR_READY.load(Ordering::Acquire)
        {
            self.needs_clear = true;
            self.update_comparator_layout();
            self.sync_comparator_splitters();
            self.sync_exit_button();
            self.sync_home_button();
            self.sync_caption_buttons();
            self.show_omnibox_passive(true);
            self.position_omnibox();
            LIFECYCLE_COMPARATOR_READY.store(true, Ordering::Release);
            self.request_redraw();
        }

        let interval = if self.surface == Surface::Home && home_animation_enabled() {
            // O `Occluded` do Windows nao cobre a minimizacao em todos os
            // casos, por isso pergunta-se tambem a janela.
            let minimized = self
                .window
                .as_ref()
                .and_then(|window| window.is_minimized())
                .unwrap_or(false);
            home_frame_interval(minimized, self.home_occluded, self.home_focused)
        } else {
            None
        };

        match interval {
            Some(interval) => {
                let now = Instant::now();
                if now >= self.next_home_frame {
                    self.next_home_frame = now + interval;
                    self.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_home_frame));
            }
            // Sem prazo nenhum: o laco dorme ate chegar um evento de verdade.
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }

        self.observe_tab_session();
    }

    /// Fechar a janela com o comparador aberto: as abas ficam gravadas.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        let _ = self.save_tab_session();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        #[cfg(feature = "accel-spike")]
        let Some(event) = self.accel_spike_filter(event) else {
            return;
        };
        match event {
            UserEvent::ExitRequested => {
                self.save_notes_draft_before_exit();
                event_loop.exit();
            }
            UserEvent::SaveTabSession(token) => self.save_due_tab_session(token),
            UserEvent::HomeRequested => self.show_home(),
            UserEvent::BackRequested => self.escape_or_back(),
            UserEvent::TabCaptureLost(gesture) => {
                let _ = self.tab_gesture(TabGestureInput::CaptureLost { gesture });
            }
            UserEvent::ToggleAutoScroll => self.toggle_auto_scroll(),
            UserEvent::SplashAnswer { asker, index } => self.answer_splash(asker, index),
            UserEvent::ZoomIn => self.step_zoom(1),
            UserEvent::ZoomOut => self.step_zoom(-1),
            UserEvent::ZoomReset => self.set_zoom(1.0),
            UserEvent::ReloadPage => self.reload_page(),
            UserEvent::ReloadTarget(target) => self.reload_target(target),
            UserEvent::PrintPage => self.print_page(),
            UserEvent::PrintTarget(target) => self.print_target(target),
            UserEvent::FocusOmnibox => self.focus_omnibox(),
            UserEvent::ToggleColumnFullscreen => self.toggle_column_fullscreen(),
            UserEvent::OpenDevTools => self.open_devtools(),
            UserEvent::OpenDevToolsTarget(target) => self.open_devtools_target(target),
            UserEvent::ViewSource => self.view_source(),
            UserEvent::ViewSourceTarget(target) => self.view_source_target(target),
            UserEvent::AutoScrollTick(token) => self.auto_scroll_tick(token),
            UserEvent::PomodoroTick(token) => self.pomodoro_tick(token),
            UserEvent::HideSplash(token) => self.hide_splash(token),
            UserEvent::CaptionReveal => self.refresh_caption_reveal(),
            UserEvent::ResizePanel => self.resize_panel(),
            UserEvent::PanelResizeDone => self.save_panel_widths(),
            UserEvent::ServiceFullscreen { generation, on } => {
                if self.service_event_is_current(generation) {
                    self.service_input(ServiceInput::PageFullscreen(on));
                }
            }
            UserEvent::ServiceAudio {
                generation,
                playing,
            } => {
                if let Some(panel) = self
                    .service_panel
                    .as_mut()
                    .filter(|panel| panel.generation == generation)
                {
                    panel.audio = playing;
                    self.request_redraw();
                }
            }
            UserEvent::ServiceEscape(generation) => {
                if self.service_event_is_current(generation) {
                    self.service_input(ServiceInput::Escape);
                }
            }
            UserEvent::GmailProbe(token) => {
                if token == self.gmail_probe_token && self.gmail_monitor.is_none() {
                    self.maybe_start_gmail_monitor();
                    if self.gmail_monitor.is_none()
                        && (self.webview.is_some() || self.comparator.is_some())
                    {
                        self.schedule_gmail_probe(60);
                    }
                }
            }
            UserEvent::GmailInboxState {
                unread,
                sender,
                subject,
                key,
            } => self.handle_gmail_state(unread, sender, subject, key),
            UserEvent::ShowHistory => self.toggle_side_panel(),
            UserEvent::Theme(event) => self.theme_event(event),
            UserEvent::Keys(event) => self.keys_event(event),
            UserEvent::Panel(post) => self.handle_panel_message(post),
            UserEvent::NotesReady { origin, reply } => self.notes_ready(origin, reply),
            UserEvent::NoteRequested { target, via } => self.request_note_from_page(target, via),
            UserEvent::NoteRefusedPrivate => {
                self.show_splash(NOTE_PRIVATE_REFUSAL.to_string(), 3);
            }
            UserEvent::NoteCaptured { raw, source } => {
                self.note_captured(&raw, source.as_deref());
            }
            UserEvent::NewNote => self.new_note_in_panel(),
            UserEvent::Live(message) => self.handle_live_message(message),
            UserEvent::Notify(event) => self.notify_event(event),
            // Pergunta e depois percorre a tabela dos alvos
            // (`clear_history::CLEAR_HISTORY_TARGETS`), um braco so.
            UserEvent::ClearHistory => self.clear_history(),
            UserEvent::HistoryCleared(result) => self.report_history_cleared(result),
            UserEvent::HistoryLoaded(result) => {
                if self.side_panel.is_open() {
                    self.panel_show_history(result);
                } else {
                    self.show_history_entries(result);
                }
            }
            UserEvent::HistoryWriteFailed(error) => {
                self.show_splash(format!("Histórico não foi gravado: {error}"), 4);
            }
            UserEvent::MemoryQueryReady { query, result } => {
                if self.side_panel.is_open() {
                    self.panel_show_memory(&query, result);
                } else {
                    self.show_memory_results(&query, result);
                }
            }
            UserEvent::MemoryCleared(result) => {
                if let Err(error) = result {
                    self.show_splash(format!("Memória: {error}"), 4);
                } else {
                    self.status = Some("Histórico e memória semântica apagados.".to_string());
                    self.request_redraw();
                }
            }
            UserEvent::AskEverywhere { source_index, text } => {
                self.ask_other_columns(source_index, text)
            }
            UserEvent::SearchSelection { text, intent } => {
                self.search_card_event(SearchCardInput::Request { text, intent })
            }
            UserEvent::SearchCardAnswer {
                token,
                button,
                shown,
            } => self.search_card_event(SearchCardInput::Answer {
                token,
                button,
                shown,
            }),
            UserEvent::SearchCardExpired(token) => {
                self.search_card_event(SearchCardInput::Expire(token))
            }
            UserEvent::ResearchAnswer { source_index, text } => {
                let provider = self
                    .comparator
                    .as_ref()
                    .and_then(|comp| comp.views.get(source_index))
                    .map(|view| view.name.to_string());
                if let (Some(provider), Some(session)) = (provider, &mut self.current_research) {
                    session.upsert_provider_answer(provider, text, None);
                    self.memory.save_session(session.clone());
                }
            }
            UserEvent::AgentObservation(page) => {
                self.handle_agent_observation(page);
            }
            UserEvent::SubmitText(input) => {
                if surface_accepts_omnibox_submit(self.surface) {
                    let input = input.trim().to_string();
                    if !input.is_empty() {
                        self.handle_input(input);
                    }
                }
            }
            UserEvent::OpenExternal(url) => self.web(url),
            UserEvent::OpenInColumn(index, url) => self.open_in_column(index, url),
            UserEvent::OpenEverywhere(url) => self.open_everywhere(url),
            UserEvent::OpenSplit { source_index, url } => {
                let _ = self.open_split(source_index, url, false);
            }
            UserEvent::OpenSplitFromSplit { source_index, url } => {
                self.open_split_from_split(source_index, url);
            }
            UserEvent::OpenPrivateSplit { source_index, url } => {
                let _ = self.open_split_mode(source_index, url, false, true, None);
            }
            UserEvent::NewTab(index) => self.new_tab(index),
            UserEvent::CloseSplit => self.close_split(),
            UserEvent::ToggleSplitFullscreen => self.toggle_split_fullscreen(),
            UserEvent::OpenPalette(index) => {
                if self.surface == Surface::Comparator {
                    self.open_ai_palette(index);
                }
            }
            UserEvent::PaletteSubmit {
                source_index,
                input,
                private,
            } => {
                // So vale o que a palette aberta prometeu: se entretanto foi
                // fechada (a coluna pode ja nem existir), a entrada cai.
                if self.palette_source() == Some((source_index, private)) {
                    self.close_palette();
                    self.submit_palette(source_index, input, private);
                }
            }
            UserEvent::ClosePalette(generation) => {
                if generation == self.palette_host.generation.get() {
                    self.close_palette();
                }
            }
            UserEvent::ExpandComparator(idx) => {
                if self.surface == Surface::Comparator {
                    self.expand_comparator(idx);
                }
            }
            UserEvent::MinimizeComparator(idx) => {
                if self.surface == Surface::Comparator {
                    self.minimize_comparator(idx);
                }
            }
            UserEvent::ColumnHint { col, hint } => self.show_column_hint(col, hint),
            UserEvent::ResizeComparator => {
                // Limpar a marca ANTES de ler: um movimento que chegue durante
                // o reposicionamento volta a enfileirar e nao se perde. Ao
                // contrario, um movimento entre a leitura e a limpeza seria
                // engolido e o divisor parava onde nao devia.
                RESIZE_PENDING.store(false, Ordering::Release);
                if self.surface == Surface::Comparator {
                    self.resize_comparator(
                        RESIZE_DIVIDER.load(Ordering::Acquire),
                        RESIZE_X.load(Ordering::Acquire),
                    );
                }
            }
            UserEvent::RelayoutComparator => {
                if self.surface == Surface::Comparator {
                    // set_decorations(false) pode substituir/reconfigurar o HWND
                    // depois de open_comparator() regressar. Reinstalar a subclass
                    // aqui prende os comandos nativos ao HWND que ficou realmente
                    // ativo, em vez de ao handle anterior da Home.
                    self.ensure_window_subclass();
                    self.needs_clear = true;
                    self.update_comparator_layout();
                    self.sync_comparator_splitters();
                    self.sync_comparator_buttons();
                    self.sync_exit_button();
                    self.sync_home_button();
                    self.sync_caption_buttons();
                    self.show_omnibox_passive(true);
                    self.position_omnibox();
                    self.request_redraw();
                }
            }
            UserEvent::RestoreHomeDecorations => {
                debug_log(format_args!(
                    "RestoreHomeDecorations: surface={:?}",
                    self.surface
                ));
                if self.surface == Surface::Home {
                    // O controller já foi descartado: esconde-se qualquer host
                    // WRY ainda preso ao HWND (a regressão em que, do segundo
                    // ciclo em diante, três WRY_WEBVIEW ficavam visíveis sobre a
                    // Home apesar de os WebView Rust já terem sido dropados).
                    // A Home fica sem a moldura do Windows, como o comparador;
                    // aqui so se limpam os hosts WRY orfaos e se mostram os
                    // botoes da janela do proprio app.
                    if let Some(window) = &self.window {
                        hide_orphaned_wry_hosts(window);
                    }
                    self.ensure_window_subclass();
                    self.sync_caption_buttons();
                    self.refresh_caption_reveal();
                    self.needs_clear = true;
                    self.position_omnibox();
                    self.request_redraw();
                    if lifecycle_probe_enabled() {
                        LIFECYCLE_HOME_READY.store(true, Ordering::Release);
                    }
                }
            }
            UserEvent::RestoreComparator => {
                if self.service_frame().is_some_and(|frame| frame.exit_button) {
                    self.service_input(ServiceInput::ToggleFullscreen);
                } else if self.surface == Surface::Comparator {
                    self.restore_comparator();
                }
            }
            UserEvent::PdfReady {
                generation,
                url,
                result,
            } => {
                if generation != self.current_generation() {
                    return;
                }
                match result {
                    Ok(bytes) => self.open_pdf(&url, bytes),
                    Err(error) => self.show_native_error(format!("PDF: {error}")),
                }
            }
            UserEvent::ReaderReady {
                generation,
                input,
                result,
            } => {
                if generation != self.current_generation() {
                    return;
                }
                match result {
                    Ok(article) => {
                        self.record(HistoryKind::Read, input, article.source_url.clone());
                        self.capture_reader_memory(&article);
                        self.open_reader(&article);
                    }
                    Err(error) => self.show_native_error(format!("Reader: {error}")),
                }
            }
            UserEvent::EpubNotice(notice) => self.handle_epub_notice(notice),
            UserEvent::EpubUi(request) => self.handle_epub_ui(request),
            UserEvent::EpubDropped(paths) => {
                if self.surface == Surface::Epub {
                    self.route_dropped_files(paths);
                }
            }
            UserEvent::OpenEpubDialog => self.open_epub_dialog(true),
            // `accel_spike_filter` ja a consumiu.
            #[cfg(feature = "accel-spike")]
            UserEvent::AccelSpike(_) => {}
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.save_notes_draft_before_exit();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if self.needs_clear {
                    self.clear_client();
                    self.needs_clear = false;
                }
                match self.surface {
                    Surface::Home => {
                        if let Some(window) = &self.window {
                            draw_home(
                                window,
                                self.status.as_deref(),
                                self.home_go_hover,
                                self.home_tool_hover,
                                self.pomodoro_bar_label(),
                            );
                        }
                    }
                    Surface::Comparator => {
                        let state = self.bar_state();
                        if let Some(window) = &self.window
                            && let Some(comp) = &self.comparator
                        {
                            let service = self.service_panel.as_ref().map(|panel| {
                                (
                                    panel.service,
                                    panel.state.badge(panel.audio),
                                    self.service_strip_physical(),
                                )
                            });
                            draw_comparator_bar(window, comp, state, &self.live_panel);
                            if let Some((service, badge, strip)) = service {
                                draw_service_chrome(
                                    window,
                                    comp.split.is_some(),
                                    service,
                                    badge,
                                    strip,
                                    self.bar_hover,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                debug_log(format_args!(
                    "resized {}x{} surface={:?}",
                    size.width, size.height, self.surface
                ));
                self.fit_comparator_to_panel();
                self.position_side_panel();
                self.position_service_panel();
                self.position_live_panel();
                self.after_panel_change();
                self.position_search_card();
                if self.surface == Surface::Home {
                    self.sync_caption_buttons();
                }
                match self.surface {
                    Surface::Home => {
                        self.needs_clear = true;
                        self.position_omnibox();
                        self.request_redraw();
                    }
                    Surface::Comparator => {
                        self.needs_clear = true;
                        self.update_comparator_layout();
                        self.sync_comparator_splitters();
                        self.sync_exit_button();
                        self.sync_home_button();
                        self.sync_caption_buttons();
                        self.position_omnibox();
                        self.position_palette();
                        self.request_redraw();
                    }
                    _ => {}
                }
            }
            // Splash, toast, botao de saida, divisores e palette sao popups em
            // coordenadas de ECRA: mover a janela nao lhes toca. Ate aqui so o
            // Resized os sincronizava, por isso arrastar a janela deixava-os
            // para tras, no sitio onde ela estava antes.
            WindowEvent::Moved(_) => {
                self.position_splash();
                self.position_toast();
                self.position_search_card();
                self.position_exit_button();
                self.position_palette();
                self.sync_comparator_splitters();
                self.after_panel_change();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                // A arrastar uma aba ou um grupo, a barra so redesenha a fila
                // reordenada; o realce e as dicas esperam pelo largar.
                let dragging = self.surface == Surface::Comparator
                    && matches!(
                        self.tab_gesture(TabGestureInput::Move {
                            cursor: self.cursor,
                            button_down: left_button_down(),
                        }),
                        TabGestureEffect::Started | TabGestureEffect::Moved
                    );
                if !dragging && self.surface == Surface::Comparator && self.bar_visible() {
                    self.update_bar_hover();
                }
                self.update_home_go_hover();
                self.refresh_caption_reveal();
                self.update_home_tool_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = (-1.0, -1.0);
                if self.surface == Surface::Comparator {
                    self.update_bar_hover();
                }
                self.update_home_go_hover();
                self.refresh_caption_reveal();
                self.update_home_tool_hover();
            }
            // Um evento por arquivo; o lote inteiro segue no `about_to_wait`.
            WindowEvent::DroppedFile(path) => self.pending_drops.push(path),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Focused(focused) => self.on_focus_changed(focused),
            WindowEvent::Occluded(occluded) => self.on_occluded_changed(occluded),
            // O tema do sistema mudou: o cache de 1 s tem de cair agora, e o
            // fundo inteiro e repintado porque ate a cor da pagina mudou.
            WindowEvent::ThemeChanged(_) => self.refresh_theme(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => match self.surface {
                Surface::Home => self.click_home(),
                Surface::Comparator => self.press_comparator(),
                _ => {}
            },
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => self.release_comparator(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => match self.surface {
                Surface::Comparator => self.right_click_comparator(),
                Surface::Home => self.right_click_home(),
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                if let Some(shortcut) = main_window_shortcut(&event.logical_key, self.modifiers) {
                    match shortcut {
                        MainShortcut::AutoScroll => self.toggle_auto_scroll(),
                        MainShortcut::Reload => self.reload_page(),
                        MainShortcut::History => self.toggle_side_panel(),
                        MainShortcut::NewTab => self.new_tab(0),
                        MainShortcut::OpenEpub => self.open_epub_dialog(true),
                        MainShortcut::NewNote => self.new_note_in_panel(),
                    }
                    return;
                }
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => self.escape_or_back(),
                    Key::Named(NamedKey::F8) => self.toggle_auto_scroll(),
                    Key::Character(ref c) if self.surface == Surface::Comparator => {
                        match c.as_str() {
                            "1" => self.expand_comparator(0),
                            "2" => self.expand_comparator(1),
                            "3" => self.expand_comparator(2),
                            "0" => self.restore_comparator(),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
