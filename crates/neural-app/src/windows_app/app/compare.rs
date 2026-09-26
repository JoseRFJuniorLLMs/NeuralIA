//! Comparador: perguntar, comparar e exportar a sessao de pesquisa, abrir e
//! expandir as colunas, o builder e o IPC de cada coluna, abrir em todas
//! (split-windows-app-c).
use crate::windows_app::*;

impl App {
    fn current_research_item_ids(&self) -> Vec<String> {
        self.current_research
            .as_ref()
            .map(|session| {
                session
                    .items
                    .iter()
                    .filter(|item| {
                        matches!(
                            item.kind,
                            ResearchItemKind::Source | ResearchItemKind::ProviderAnswer
                        )
                    })
                    .map(|item| item.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(in crate::windows_app) fn compare_current_research(&self) {
        let Some(session) = &self.current_research else {
            self.show_native_text(
                "NeuralIA — Research Session",
                "Nenhuma sessão de pesquisa está ativa.",
            );
            return;
        };
        let ids = self.current_research_item_ids();
        let facts = session.comparison(&ids);
        let text = if facts.is_empty() {
            "Ainda não há fontes/respostas suficientes para comparar.".to_string()
        } else {
            facts
                .into_iter()
                .map(|fact| {
                    format!(
                        "{}\nEntidades: {}\nNúmeros: {}\nDatas: {}",
                        fact.source,
                        fact.entities.join(", "),
                        fact.numbers.join(", "),
                        fact.dates.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\r\n\r\n")
        };
        self.show_native_text("NeuralIA — Comparação da pesquisa", &text);
    }

    pub(in crate::windows_app) fn synthesize_current_research(&mut self) {
        let ids = self.current_research_item_ids();
        let Some(session) = &mut self.current_research else {
            self.show_native_text(
                "NeuralIA — Research Session",
                "Nenhuma sessão de pesquisa está ativa.",
            );
            return;
        };
        if ids.is_empty() {
            self.show_native_text(
                "NeuralIA — Síntese",
                "Ainda não há fontes/respostas para sintetizar.",
            );
            return;
        }
        let snapshot = session.synthesize(&ids).clone();
        self.memory.save_session(session.clone());
        self.show_native_text("NeuralIA — Síntese com proveniência", &snapshot.output);
    }

    pub(in crate::windows_app) fn export_current_research(&mut self) {
        let Some(session) = &self.current_research else {
            self.show_native_text(
                "NeuralIA — Research Session",
                "Nenhuma sessão de pesquisa está ativa.",
            );
            return;
        };
        let dir = self.config.data_dir.join("research-exports");
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.show_splash(format!("Export: {error}"), 4);
            return;
        }
        let path = dir.join(format!("{}.md", session.id));
        match std::fs::write(&path, session.export_markdown()) {
            Ok(()) => self.show_native_text(
                "NeuralIA — Pesquisa exportada",
                &format!("Markdown salvo em:\r\n{}", path.display()),
            ),
            Err(error) => self.show_splash(format!("Export: {error}"), 4),
        }
    }

    /// Um unico fornecedor: o Google AI Mode, num so WebView. E a saida para
    /// quem nao quer a pergunta em tres sitios ao mesmo tempo (`ask:` ou `?`).
    pub(in crate::windows_app) fn ask(&mut self, query: String) {
        let url = match google_ai_url(&query, &self.config.language) {
            Ok(url) => url,
            Err(error) => {
                self.show_native_error(error.to_string());
                return;
            }
        };
        self.next_generation();
        self.record(HistoryKind::Ask, query, url.to_string());
        self.open_external(url.as_str());
    }

    /// O cartao "Mandar para as 3 IAs?" / "Traduzir nas 3 IAs?": a decisao e
    /// a de `SearchCard::step`
    /// (pura, testada) e o efeito o de `apply_search_card`; aqui so se junta
    /// o relogio. O `compare` so corre para um `Confirmed`.
    pub(in crate::windows_app) fn search_card_event(&mut self, input: SearchCardInput) {
        let outcome = self.search_card.step(input, Instant::now());
        apply_search_card(self, outcome);
    }

    /// Destino normal de uma pergunta: a mesma consulta segue em simultaneo
    /// para o Google AI Mode, o ChatGPT e o Claude, lado a lado. O nome e a
    /// entrada do Historico sao os de `compare_records`.
    pub(in crate::windows_app) fn compare(&mut self, request: CompareRequest) {
        self.next_generation();

        let (session, question_memory, reopen) = compare_records(&request);
        self.memory.capture(question_memory);
        self.memory.save_session(session.clone());
        self.current_research = Some(session);

        self.record(HistoryKind::Ask, reopen, "comparator-3col".to_string());
        self.open_comparator(&request.prompt);
    }

    fn open_comparator(&mut self, query: &str) {
        self.close_side_panel(PanelExit::NewSearch);
        self.close_service_panel();
        self.close_live_panel();
        let reuse_comparator = self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.views.len() == COMPARATOR_COLUMNS);
        if !reuse_comparator {
            self.destroy_web_surfaces();
        }
        self.show_omnibox_passive(true);
        self.position_omnibox();

        let google_url = match google_ai_url(query, &self.config.language) {
            Ok(u) => u,
            Err(e) => {
                self.show_native_error(e.to_string());
                return;
            }
        };
        let chatgpt_url = match chatgpt_search_url(query) {
            Ok(u) => u,
            Err(e) => {
                self.show_native_error(e.to_string());
                return;
            }
        };
        let claude_url = match claude_search_url(query) {
            Ok(u) => u,
            Err(e) => {
                self.show_native_error(e.to_string());
                return;
            }
        };

        let Some(window) = &self.window else {
            return;
        };
        // No comparador o chrome e nosso: a primeira linha recebe as abas e os
        // controles de janela; a segunda fica reservada aos provedores.
        debug_log(format_args!("open_comparator: set_decorations(false)"));
        window.set_decorations(false);
        self.ensure_window_subclass();

        if reuse_comparator {
            let urls = [
                google_url.as_str(),
                chatgpt_url.as_str(),
                claude_url.as_str(),
            ];
            let mut reload_error = None;
            if let Some(comparator) = &mut self.comparator {
                comparator.expanded = None;
                comparator.minimized = [false; COMPARATOR_COLUMNS];
                comparator.weights = [1.0; COMPARATOR_COLUMNS];
                comparator.bar_focus = [None; COMPARATOR_COLUMNS];
                // A pesquisa nova nao apaga abas nem grupos: passa a ser o
                // contexto ativo de cada coluna, e a fonte que estava aberta
                // ao lado fica na barra, por carregar. Quem decide o que
                // continua aberto e `start_new_search`; aqui so se obedece.
                let mut active = [None; COMPARATOR_COLUMNS];
                if let Some((column, id)) = persisted_active(comparator_split_key(comparator)) {
                    active[column] = Some(id);
                }
                let ComparatorState {
                    contexts,
                    groups,
                    next_context_id,
                    next_group_id,
                    split,
                    ..
                } = comparator;
                start_new_search(
                    contexts,
                    groups,
                    next_context_id,
                    next_group_id,
                    &mut active,
                );
                let split_stays = split.as_ref().is_some_and(|split| {
                    persisted_active(Some((split.source_index, split.context_id, split.private)))
                        .is_some_and(|(column, id)| active[column] == Some(id))
                });
                if !split_stays && let Some(split) = split.take() {
                    let _ = split.webview.set_visible(false);
                    let _ = split.webview.focus_parent();
                    drop(split);
                }
                for (view, url) in comparator.views.iter().zip(urls) {
                    let encoded = match serde_json::to_string(url) {
                        Ok(encoded) => encoded,
                        Err(error) => {
                            reload_error =
                                Some(format!("URL inválida ao reutilizar {}: {error}", view.name));
                            break;
                        }
                    };
                    let script = format!("window.location.replace({encoded});");
                    if let Err(error) = view.webview.evaluate_script(&script) {
                        reload_error = Some(format!(
                            "WebView2 não pôde reutilizar {}: {error}",
                            view.name
                        ));
                        break;
                    }
                }
            }
            if let Some(error) = reload_error {
                self.show_native_error(error);
                return;
            }
            self.activate_comparator(false);
            return;
        }

        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;

        let content_h = (logical_h - COMPARATOR_CHROME_HEIGHT).max(100.0);
        let content_y = COMPARATOR_CHROME_HEIGHT;
        let n = 3.0;
        let col_w = logical_w / n;

        // As abas e os grupos da sessao anterior voltam a barra. Nenhuma
        // carrega agora: a pergunta e o contexto ativo de cada coluna, e cada
        // aba restaurada so abre quando for escolhida. O que esta no disco e
        // o que acabou de ser lido: nada a regravar ate alguma aba mudar.
        let (restored, tabs_notice) = self.tab_session.restore();

        let targets = [
            ("Google Gemini", google_url),
            ("ChatGPT", chatgpt_url),
            ("Claude", claude_url),
        ];

        let mut views = Vec::new();
        for (i, (name, url)) in targets.into_iter().enumerate() {
            let col_x = i as f64 * col_w;
            let actual_w = if i == 2 { logical_w - col_x } else { col_w };

            let bounds = wry::Rect {
                position: LogicalPosition::new(col_x, content_y).into(),
                size: LogicalSize::new(actual_w, content_h).into(),
            };

            let builder = self
                .comparator_webview_builder(i, name)
                .with_bounds(bounds)
                .with_url(url.as_str());
            let hooked = self.hooked_builder(builder, WebViewHost::Column(i), None);

            match hooked.build_hooked_as_child(window) {
                Ok(wv) => {
                    let _ = wv.zoom(self.zoom);
                    #[cfg(feature = "accel-spike")]
                    self.accel_spike_hook(&wv, crate::accel_spike::SpikeHost::Column);
                    views.push(ComparatorView { webview: wv, name });
                }
                Err(error) => {
                    self.show_native_error(format!("WebView2 não pôde abrir {name}: {error}"));
                    return;
                }
            }
        }

        self.comparator = Some(ComparatorState {
            views,
            expanded: None,
            minimized: [false; COMPARATOR_COLUMNS],
            weights: [1.0; COMPARATOR_COLUMNS],
            split: None,
            contexts: restored.contexts,
            groups: restored.groups,
            next_group_id: restored.next_group_id,
            next_context_id: restored.next_context_id,
            bar_focus: [None; COMPARATOR_COLUMNS],
            panel_width: 0.0,
        });
        self.activate_comparator(true);
        if let Some(notice) = tabs_notice {
            self.show_splash(notice, 5);
        }
    }

    fn activate_comparator(&mut self, sync_remote_buttons: bool) {
        self.bar_hover = None;
        self.forget_tab_gesture();
        self.surface = Surface::Comparator;

        // build_as_child nasce antes de self.comparator existir, portanto o
        // primeiro layout feito durante a construcao nao pode passar pela
        // rotina que tambem torna cada controller visivel. Reaplicar aqui e
        // essencial também ao reutilizar controllers estacionados na Home.
        self.needs_clear = true;
        self.update_comparator_layout();
        self.sync_comparator_splitters();
        // Na reutilização acabámos de iniciar três navegações. Executar outro
        // script remoto aqui pode manter WebView2 dentro do pump aninhado e
        // impedir o callback de devolver o controlo ao winit. O relayout de
        // 40 ms sincroniza os botões depois que o event loop já respirou.
        if sync_remote_buttons {
            self.sync_comparator_buttons();
        }
        self.sync_exit_button();
        self.sync_home_button();
        self.sync_caption_buttons();
        self.show_omnibox_passive(true);
        self.position_omnibox();

        for delay_ms in COMPARATOR_INITIAL_RELAYOUT_DELAYS_MS {
            self.timers.after(
                Duration::from_millis(delay_ms),
                UserEvent::RelayoutComparator,
            );
        }

        // Chegar aqui significa que os tres build_as_child ja retornaram,
        // self.comparator ja existe e o layout inicial foi aplicado. Nesta
        // altura set_decorations(false) pode ja ter trocado/reparentado o HWND
        // nativo. Reinstale a subclass NO HWND efetivo antes de publicar Ready:
        // o gate pode enviar Home imediatamente depois de observar o flag.
        self.ensure_window_subclass();

        // Ready só é publicado em about_to_wait(), depois de devolver o
        // controlo ao event loop fora do pump aninhado do WebView2.
        self.schedule_gmail_probe(4);
        self.begin_reading_session(false);
        self.request_redraw();
    }

    /// Alterna: o botao injetado na pagina pede sempre "expandir", e e aqui que
    /// isso vira "sair da tela cheia" quando a coluna ja esta expandida. Sem a
    /// barra nativa em tela cheia, esse botao e o Esc sao o caminho de volta.
    pub(in crate::windows_app) fn expand_comparator(&mut self, idx: usize) {
        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some())
        {
            self.close_split();
        }

        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
            && comp.minimized[idx]
        {
            comp.minimized[idx] = false;
            comp.expanded = None;
            self.bar_hover = None;
            self.forget_tab_gesture();
            self.needs_clear = true;
            self.update_comparator_layout();
            self.sync_comparator_splitters();
            self.sync_comparator_buttons();
            self.request_redraw();
            return;
        }

        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
        {
            comp.expanded = if comp.expanded == Some(idx) {
                None
            } else {
                Some(idx)
            };
        }
        self.bar_hover = None;
        self.forget_tab_gesture();
        self.needs_clear = true;

        // Expandir ocupa apenas a área de conteúdo. A titlebar do NeuralIA e
        // minimizar/maximizar/fechar permanecem acessíveis.
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

    pub(in crate::windows_app) fn minimize_comparator(&mut self, idx: usize) {
        if self.surface != Surface::Comparator {
            return;
        }

        let can_minimize = self
            .comparator
            .as_ref()
            .is_some_and(|comp| can_minimize_column(&comp.minimized, comp.views.len(), idx));
        if !can_minimize {
            self.show_splash(
                "Pelo menos um painel precisa continuar visível.".to_string(),
                2,
            );
            return;
        }

        // So agora a operacao foi validada. Antes, um pedido impossivel para a
        // ultima coluna visivel fechava a fonte lateral e depois dizia que nao
        // podia minimizar: o clique rejeitado destruia estado.
        if self
            .comparator
            .as_ref()
            .is_some_and(|comp| comp.split.is_some())
        {
            self.close_split();
        }

        let mut was_expanded = false;
        if let Some(comp) = &mut self.comparator
            && idx < comp.views.len()
        {
            was_expanded = comp.expanded == Some(idx);
            comp.expanded = None;
            comp.minimized[idx] = true;
        }

        if was_expanded && let Some(window) = &self.window {
            window.set_fullscreen(None);
        }

        self.bar_hover = None;
        self.forget_tab_gesture();
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

    pub(in crate::windows_app) fn restore_comparator(&mut self) {
        if let Some(comp) = &mut self.comparator {
            comp.expanded = None;
        }
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

    /// O botao vive dentro da pagina e nao sabe o estado; o app diz-lho.
    pub(in crate::windows_app) fn sync_comparator_buttons(&self) {
        let Some(comp) = &self.comparator else {
            return;
        };
        for (index, view) in comp.views.iter().enumerate() {
            let script = if comp.expanded == Some(index) {
                COMPARATOR_BUTTON_EXPANDED
            } else {
                COMPARATOR_BUTTON_COLLAPSED
            };
            let _ = view.webview.evaluate_script(script);
        }
    }

    pub(in crate::windows_app) fn update_comparator_layout(&self) {
        let (Some(window), Some(comp)) = (&self.window, &self.comparator) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor().max(1.0);
        let logical_w = comparator_logical_width(size.width as f64 / scale, comp.panel_width);
        let logical_h = size.height as f64 / scale;

        let content_h = (logical_h - COMPARATOR_CHROME_HEIGHT).max(100.0);
        let content_y = COMPARATOR_CHROME_HEIGHT;

        // Fonte lateral: a IA que originou o link continua visível, as demais
        // ficam vivas e preservam estado para reaparecer ao fechar a gaveta.
        if let Some(split) = &comp.split {
            if split.fullscreen {
                for view in &comp.views {
                    let _ = view.webview.set_visible(false);
                }
                let _ = split.webview.set_bounds(wry::Rect {
                    position: LogicalPosition::new(0.0, content_y).into(),
                    size: LogicalSize::new(logical_w, content_h).into(),
                });
                let _ = split.webview.set_visible(true);
                return;
            }

            let ai_width = (logical_w * 0.54).clamp(logical_w * 0.38, logical_w * 0.68);
            for (index, view) in comp.views.iter().enumerate() {
                if index == split.source_index {
                    let _ = view.webview.set_bounds(wry::Rect {
                        position: LogicalPosition::new(0.0, content_y).into(),
                        size: LogicalSize::new(ai_width, content_h).into(),
                    });
                    let _ = view.webview.set_visible(true);
                } else {
                    let _ = view.webview.set_visible(false);
                }
            }
            let _ = split.webview.set_bounds(wry::Rect {
                position: LogicalPosition::new(ai_width, content_y).into(),
                size: LogicalSize::new((logical_w - ai_width).max(1.0), content_h).into(),
            });
            let _ = split.webview.set_visible(true);
            return;
        }

        match comp.expanded {
            Some(idx) => {
                // Fullscreen fica geometricamente estavel. A versao anterior
                // mudava o bounds do WebView toda vez que o cursor tocava o
                // topo para mostrar/esconder chrome, causando flicker e pump
                // de layout em cascata no WebView2.
                for (i, v) in comp.views.iter().enumerate() {
                    if i == idx {
                        let _ = v.webview.set_bounds(wry::Rect {
                            position: LogicalPosition::new(0.0, content_y).into(),
                            size: LogicalSize::new(logical_w, content_h).into(),
                        });
                        let _ = v.webview.set_visible(true);
                    } else {
                        let _ = v.webview.set_visible(false);
                    }
                }
            }
            None => {
                let spans = visible_column_spans(
                    logical_w,
                    comp.views.len(),
                    &comp.weights,
                    &comp.minimized,
                );
                for span in &spans {
                    let v = &comp.views[span.index];
                    let _ = v.webview.set_bounds(wry::Rect {
                        position: LogicalPosition::new(span.x, content_y).into(),
                        size: LogicalSize::new(span.width.max(1.0), content_h).into(),
                    });
                    let _ = v.webview.set_visible(true);
                }

                for (index, v) in comp.views.iter().enumerate() {
                    if comp.minimized[index] {
                        let _ = v.webview.set_visible(false);
                    }
                }
            }
        }
    }

    /// Que evento nasce de uma mensagem vinda da coluna `col_index`.
    ///
    /// Vive fora do closure do IPC de proposito. A decisao que aqui se toma --
    /// em especial a de um clique num link ir para a propria coluna ou para o
    /// painel lateral -- e a que o utilizador ve, e dentro de um closure de
    /// `WebViewBuilder` nao havia forma de a exercitar sem abrir uma janela.
    #[allow(clippy::needless_pass_by_value)]
    pub(in crate::windows_app) fn column_ipc_event_impl(
        col_index: usize,
        action: IpcAction,
    ) -> Option<UserEvent> {
        match action {
            IpcAction::ResearchAnswer { col, text } if col == col_index => {
                Some(UserEvent::ResearchAnswer {
                    source_index: col_index,
                    text,
                })
            }
            // Uma coluna so fala por si: o `col` tem de ser o dela.
            IpcAction::Ask { col, text } if col == col_index => Some(UserEvent::AskEverywhere {
                source_index: col_index,
                text,
            }),
            // Clique simples: a pagina abre nas TRES colunas, para se ver o
            // que cada IA diz dela. Ctrl+clique: abre no painel lateral e a
            // barra de titulo guarda a aba -- o "novo separador" do Chrome.
            IpcAction::Link { col, url, aside } if col == col_index => Some(if aside {
                UserEvent::OpenSplit {
                    source_index: col_index,
                    url,
                }
            } else {
                UserEvent::OpenEverywhere(url)
            }),
            IpcAction::Split { col, url } if col == col_index => Some(UserEvent::OpenSplit {
                source_index: col_index,
                url,
            }),
            IpcAction::Palette { col } if col == col_index => {
                Some(UserEvent::OpenPalette(col_index))
            }
            IpcAction::Omnibox => Some(UserEvent::OpenPalette(col_index)),
            IpcAction::Reload => Some(UserEvent::ReloadTarget(PageTarget::Column(col_index))),
            IpcAction::Print => Some(UserEvent::PrintTarget(PageTarget::Column(col_index))),
            IpcAction::DevTools => {
                Some(UserEvent::OpenDevToolsTarget(PageTarget::Column(col_index)))
            }
            IpcAction::ViewSource => {
                Some(UserEvent::ViewSourceTarget(PageTarget::Column(col_index)))
            }
            IpcAction::Fullscreen => Some(UserEvent::ExpandComparator(col_index)),
            IpcAction::ShortcutExpand { col } => Some(UserEvent::ExpandComparator(col)),
            IpcAction::Minimize { col } if col == col_index => {
                Some(UserEvent::MinimizeComparator(col_index))
            }
            IpcAction::NewTab { col: Some(col) } if col == col_index => {
                Some(UserEvent::NewTab(col_index))
            }
            IpcAction::Expand { col } if col == col_index => {
                Some(UserEvent::ExpandComparator(col_index))
            }
            IpcAction::Hint { col, hint } if col == col_index => {
                Some(UserEvent::ColumnHint { col, hint })
            }
            // Ctrl+Shift+Z ou Salvar nota: a selecao e lida DESTA coluna,
            // pelo lado nativo.
            IpcAction::Note { via } => Some(UserEvent::NoteRequested {
                target: Some(PageTarget::Column(col_index)),
                via,
            }),
            other => common_ipc_event(other),
        }
    }

    fn comparator_webview_builder(
        &self,
        col_index: usize,
        col_name: &'static str,
    ) -> WebViewBuilder<'static> {
        let ipc_proxy = self.proxy.clone();
        let new_window_proxy = self.proxy.clone();
        let capability = remote_capability();
        let ipc_capability = capability.clone();

        // Um script por chamada, e nao os tres concatenados num so. O
        // WebView2 executa cada script de inicializacao isoladamente: assim
        // uma excecao ao nivel de topo de um deles -- o `sessionStorage` do
        // auto-submit, por exemplo, que lanca com armazenamento particionado --
        // deixa de levar atras o COMPARATOR_INJECT_SCRIPT, e com ele os
        // cliques nos links e os controlos da coluna.
        let [prelude, keymap, auto_submit, inject] =
            comparator_init_scripts(col_index, col_name, &capability);

        themed_webview_builder()
            .with_initialization_script(prelude)
            .with_initialization_script(keymap)
            .with_initialization_script(auto_submit)
            .with_initialization_script(inject)
            .with_ipc_handler(move |request| {
                let Some(action) =
                    parse_ipc_message(request.body(), &ipc_capability, COMPARATOR_COLUMNS)
                else {
                    return;
                };
                let event = Self::column_ipc_event_impl(col_index, action);
                if let Some(event) = event {
                    let _ = ipc_proxy.send_event(event);
                }
            })
            // A trava de navegacao (NavGate::Web, sem origem local) vem de
            // `hooked_builder`, como em todas as WebViews.
            .with_new_window_req_handler(move |target, _features| {
                // `about:blank` NAO. Muitos sites abrem uma ligacao com
                // `window.open('', '_blank')` e so depois atribuem o endereco
                // ao popup: o WebView2 levanta o pedido com `about:blank`, e
                // carregar isso na coluna apagava a conversa da IA e nao
                // abria link nenhum. Ficar quieto deixa a pagina como estava.
                if !target.eq_ignore_ascii_case("about:blank") && remote_web_target(&target, None) {
                    let _ = new_window_proxy.send_event(UserEvent::OpenInColumn(col_index, target));
                }
                NewWindowResponse::Deny
            })
            .with_permission_handler(|kind| web_media_permission(kind, true))
            .with_focused(true)
    }

    /// O login abre-se com `window.open`, e ate aqui isso destruia as tres
    /// colunas para pôr um WebView unico no lugar delas -- perdia-se a
    /// comparacao e o ecra ficava com os pixeis das janelas mortas. O popup
    /// pertence a coluna que o pediu e e nela que carrega.
    pub(in crate::windows_app) fn open_in_column(&mut self, index: usize, url: String) {
        if self.surface != Surface::Comparator {
            self.web(url);
            return;
        }

        let Ok(valid) = neural_core::validate_web_url(&url) else {
            self.show_splash("URL do link inválida.".to_string(), 3);
            return;
        };
        if neural_core::is_local_network_target(&valid) {
            self.show_splash(
                "O link não pode redirecionar a coluna para a rede local.".to_string(),
                4,
            );
            return;
        }

        let loaded = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.views.get(index))
            .is_some_and(|view| view.webview.load_url(valid.as_str()).is_ok());

        // Um popup/link que falha pertence à coluna que o originou. Nunca
        // derrube as três colunas para abrir um WebView solitário como fallback:
        // além de destruir a comparação, isso reabria a corrida de lifecycle.
        if !loaded {
            self.show_splash(
                "Não consegui abrir esse link na coluna sem perder a comparação.".to_string(),
                4,
            );
        }
    }

    /// A mesma pagina nas tres colunas.
    ///
    /// Nao mexe no comparador nem no painel lateral: so troca o endereco de
    /// cada coluna. Se alguma recusar -- uma coluna minimizada nao tem
    /// WebView --, as outras seguem na mesma; um clique num link nao pode
    /// desmontar a comparacao por causa de uma delas.
    pub(in crate::windows_app) fn open_everywhere(&mut self, url: String) {
        if self.surface != Surface::Comparator {
            self.web(url);
            return;
        }
        let Ok(valid) = neural_core::validate_web_url(&url) else {
            self.show_splash("URL da fonte inválida.".to_string(), 3);
            return;
        };
        if neural_core::is_local_network_target(&valid) {
            self.show_splash(
                "A página não pode redirecionar a fonte para a rede local.".to_string(),
                4,
            );
            return;
        }

        let target = valid.to_string();
        let Some(comp) = &self.comparator else {
            return;
        };
        for view in &comp.views {
            let _ = view.webview.load_url(&target);
        }
        self.request_redraw();
    }

    pub(in crate::windows_app) fn new_tab(&mut self, source_index: usize) {
        if self.surface == Surface::Comparator {
            self.open_ai_palette(source_index.min(COMPARATOR_COLUMNS - 1));
        } else {
            self.focus_omnibox();
        }
    }

    /// URL de pergunta do fornecedor da coluna.
    /// Pergunta escrita e enviada numa coluna: segue tambem para as outras,
    /// cada uma no seu fornecedor -- como o clique num link, que abre em
    /// todas. A coluna de origem nao e tocada: ja esta a enviar e mantem o
    /// contexto da conversa dela.
    pub(in crate::windows_app) fn ask_other_columns(&mut self, source_index: usize, text: String) {
        if self.surface != Surface::Comparator {
            return;
        }
        let Some(count) = self.comparator.as_ref().map(|comp| comp.views.len()) else {
            return;
        };
        let mut urls = Vec::new();
        for index in ask_targets(source_index, count) {
            match self.provider_query_url(index, &text) {
                Ok(url) => urls.push((index, url)),
                Err(error) => {
                    self.show_splash(error.to_string(), 3);
                    return;
                }
            }
        }
        debug_log(format_args!(
            "ask: coluna {source_index} -> {} coluna(s), {} chars",
            urls.len(),
            text.chars().count()
        ));
        if let Some(comp) = &self.comparator {
            for (index, url) in &urls {
                if let Some(view) = comp.views.get(*index) {
                    let _ = view.webview.load_url(url.as_str());
                }
            }
        }
        if let Some((_, url)) = urls.first() {
            self.record(HistoryKind::Ask, text, url.to_string());
        }
    }

    pub(in crate::windows_app) fn provider_query_url(
        &self,
        source_index: usize,
        query: &str,
    ) -> neural_core::Result<Url> {
        match source_index {
            0 => google_ai_url(query, &self.config.language),
            1 => chatgpt_search_url(query),
            _ => claude_search_url(query),
        }
    }
}
