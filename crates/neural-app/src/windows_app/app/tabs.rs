//! Abas: sincronizacao da sessao de abas, abas de contexto e grupos, menus de
//! aba e de grupo, gestos, pressionar/largar/clicar na barra
//! (split-windows-app-c).
use crate::windows_app::*;

impl App {
    /// Corre depois de cada lote de eventos: se as abas ou os grupos mudaram,
    /// agenda a gravacao. Olhar aqui, e nao em cada sitio que mexe nas abas,
    /// apanha tambem os gestos da barra que ainda vao nascer.
    pub(in crate::windows_app) fn observe_tab_session(&mut self) {
        let Some(comp) = &self.comparator else {
            return;
        };
        if let Some(token) =
            self.tab_session
                .observe(&comp.contexts, &comp.groups, comparator_split_key(comp))
        {
            self.timers
                .after(TAB_SESSION_DEBOUNCE, UserEvent::SaveTabSession(token));
        }
    }

    /// Grava ja, se o disco estiver atrasado em relacao a barra: antes de o
    /// comparador ser destruido e ao sair (`TabPersistence::save_now`).
    pub(in crate::windows_app) fn save_tab_session(&mut self) -> std::io::Result<TabSave> {
        let Some(comp) = &mut self.comparator else {
            return Ok(TabSave::Unchanged);
        };
        let split = comparator_split_key(comp);
        self.tab_session
            .save_now(&mut comp.contexts, &mut comp.groups, split)
    }

    /// O `SaveTabSession(token)` do fim do atraso: grava se ainda for o
    /// ultimo agendado (`TabPersistence::save_due`) e diz ao dono o que correu
    /// mal ou o que mudou.
    pub(in crate::windows_app) fn save_due_tab_session(&mut self, token: u64) {
        let result = self.comparator.as_mut().and_then(|comp| {
            let split = comparator_split_key(comp);
            self.tab_session
                .save_due(token, &mut comp.contexts, &mut comp.groups, split)
        });
        if let Some(notice) = result.as_ref().and_then(tab_save_notice) {
            self.request_redraw();
            self.show_splash(notice, 4);
        }
    }

    /// Parte de "Apagar historico": o modelo vivo, o ficheiro e as copias,
    /// tudo de uma vez (`TabPersistence::forget`).
    pub(in crate::windows_app) fn forget_tab_session(&mut self) {
        let result = match &mut self.comparator {
            Some(comp) => {
                let split = comparator_split_key(comp);
                self.tab_session
                    .forget(&mut comp.contexts, &mut comp.groups, split)
            }
            None => self.tab_session.forget(
                &mut std::array::from_fn(|_| Vec::new()),
                &mut std::array::from_fn(|_| Vec::new()),
                None,
            ),
        };
        self.request_redraw();
        if let Err(error) = result {
            self.show_splash(
                format!("Não foi possível apagar as abas salvas: {error}"),
                4,
            );
        }
    }

    fn context_tab_identity(
        &self,
        source_index: usize,
        context_index: usize,
    ) -> Option<(u64, String)> {
        self.comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .and_then(|tabs| tabs.get(context_index))
            .map(|tab| (tab.id, tab.url.clone()))
    }

    fn open_context_tab(&mut self, source_index: usize, context_index: usize) -> bool {
        let active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .map(|split| (split.source_index, split.context_id));
        self.context_tab_identity(source_index, context_index)
            .is_some_and(|(context_id, url)| {
                context_tab_click_is_noop(active, source_index, context_id)
                    || self.open_split_mode(source_index, url, false, false, Some(context_id))
            })
    }

    fn open_context_tab_fullscreen(&mut self, source_index: usize, context_index: usize) {
        if self.open_context_tab(source_index, context_index)
            && !self
                .comparator
                .as_ref()
                .and_then(|comp| comp.split.as_ref())
                .is_some_and(|split| split.fullscreen)
        {
            self.toggle_split_fullscreen();
        }
    }

    fn close_context_tab(&mut self, source_index: usize, context_index: usize) {
        let Some((context_id, _url)) = self.context_tab_identity(source_index, context_index)
        else {
            return;
        };
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref())
            .is_some_and(|split| {
                split.source_index == source_index && split.context_id == Some(context_id)
            });
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator {
            let _ = remove_context_tab(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
        }
        self.request_redraw();
    }

    /// Um clique na pilula: recolhe o grupo cujas abas estao a vista; abre o
    /// recolhido; e o grupo cujas abas ficaram fora do corte (a pilula esta
    /// sozinha, com cara de recolhida) mostra-as -- em vez de o recolher sem
    /// se ver mudanca nenhuma. Fechado, as abas continuam abertas.
    fn toggle_context_group(&mut self, source_index: usize, group_index: usize) {
        let members_drawn = self
            .bar_layout()
            .is_some_and(|layout| layout.group_members_drawn(source_index, group_index));
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
        {
            let _ = apply_chip_click(
                &comp.contexts[source_index],
                &mut comp.groups[source_index],
                &mut comp.bar_focus[source_index],
                group_index,
                members_drawn,
            );
        }
        self.request_redraw();
    }

    /// "Adicionar a um novo grupo". Como no Chrome, o grupo novo abre logo o
    /// seu editor -- o menu do grupo, com as cores, junto da pilula --: a cor
    /// escolhe-se ja, nao fica uma que ninguem escolheu.
    fn group_context_tab(&mut self, source_index: usize, context_index: usize) {
        let mut created = None;
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
        {
            let ComparatorState {
                contexts,
                groups,
                next_group_id,
                bar_focus,
                ..
            } = comp;
            let id = contexts[source_index].get(context_index).map(|tab| tab.id);
            created = regroup_context_tab(
                &mut contexts[source_index],
                &mut groups[source_index],
                next_group_id,
                context_index,
            );
            if created.is_some() {
                bar_focus[source_index] = id;
            }
        }
        self.request_redraw();
        if let Some(group_index) = created {
            self.show_group_menu(source_index, group_index, true);
        }
    }

    fn join_context_tab_group(
        &mut self,
        source_index: usize,
        context_index: usize,
        group_index: usize,
    ) {
        if let Some(comp) = &mut self.comparator
            && let Some(id) = comp.groups[source_index]
                .get(group_index)
                .map(|group| group.id)
        {
            // A aba vai para junto dos outros membros, que podem estar longe
            // das abas recentes: fica a ancora da coluna, para nao sumir.
            let moved = comp.contexts[source_index]
                .get(context_index)
                .map(|tab| tab.id);
            join_context_group(&mut comp.contexts[source_index], id, context_index);
            prune_empty_groups(&comp.contexts[source_index], &mut comp.groups[source_index]);
            if moved.is_some() {
                comp.bar_focus[source_index] = moved;
            }
        }
        self.request_redraw();
    }

    fn ungroup_context_tab(&mut self, source_index: usize, context_index: usize) {
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
        {
            let moved = comp.contexts[source_index]
                .get(context_index)
                .map(|tab| tab.id);
            leave_context_group(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
            if moved.is_some() {
                comp.bar_focus[source_index] = moved;
            }
        }
        self.request_redraw();
    }

    fn close_other_context_tabs(&mut self, source_index: usize, context_index: usize) {
        let valid_context = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .is_some_and(|tabs| context_index < tabs.len());
        if !valid_context {
            return;
        }
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref().map(|split| (comp, split)))
            .is_some_and(|(comp, split)| {
                split.source_index == source_index
                    && active_context_removed_by_scope(
                        &comp.contexts[source_index],
                        context_index,
                        split.context_id,
                        true,
                    )
            });
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator {
            let _ = close_other_context_tabs_in_scope(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
        }
        self.request_redraw();
    }

    fn close_all_context_tabs(&mut self, source_index: usize, context_index: usize) {
        let valid_context = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.contexts.get(source_index))
            .is_some_and(|tabs| context_index < tabs.len());
        if !valid_context {
            return;
        }
        let closes_active = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.split.as_ref().map(|split| (comp, split)))
            .is_some_and(|(comp, split)| {
                split.source_index == source_index
                    && active_context_removed_by_scope(
                        &comp.contexts[source_index],
                        context_index,
                        split.context_id,
                        false,
                    )
            });
        if closes_active {
            self.close_split();
        }
        if let Some(comp) = &mut self.comparator {
            let _ = close_context_tab_scope(
                &mut comp.contexts[source_index],
                &mut comp.groups[source_index],
                context_index,
            );
        }
        self.request_redraw();
    }

    pub(in crate::windows_app) fn joinable_context_groups(
        groups: &[ContextGroup],
        current_group: Option<u64>,
    ) -> Vec<(usize, String)> {
        groups
            .iter()
            .enumerate()
            .filter(|(_, group)| Some(group.id) != current_group)
            .map(|(index, group)| (index, group.name.clone()))
            .collect()
    }

    /// Botao direito na pilula de uma IA: o mesmo item de rolagem que o menu
    /// dentro da coluna oferece, para se descobrir tambem pela barra.
    fn column_pill_menu(&mut self, col_index: usize) {
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        // A dica da pilula nao fica a flutuar por cima do menu.
        hover_tooltip(std::ptr::null_mut(), "");
        // Os mesmos itens que o botao direito dentro da coluna, decididos
        // pelo mesmo `webview_menu_responder` (menu proprio: nenhum item
        // nativo).
        let request =
            webview_menu_responder(WebViewHost::Column(col_index), self.auto_scroll.clone())(0);
        let mut menu = PopupMenu::default();
        for item in &request.items {
            menu.push(MenuCommand::new(item.id, item.label));
        }
        let point = self.bar_menu_point(hwnd);
        let command = self.track_menu(&menu, point, MenuButton::Right);
        if let Some(event) = request.item(command).and_then(|item| item.selected()) {
            let _ = self.proxy.send_event(event);
        }
    }

    /// O "‹N" de uma coluna: a lista de todas as abas dela, cada grupo num
    /// submenu (com a amostra da cor) com as suas. Escolher uma abre-a ao lado
    /// e fa-la ancora da coluna -- a barra passa a mostra-la, pilula incluida.
    /// Sem isto, uma aba guardada que ficasse fora do corte das mais recentes
    /// continuava no `tabs.json` sem se poder abrir, recolorir nem fechar.
    fn show_tab_list_menu(&mut self, source_index: usize) {
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        hover_tooltip(std::ptr::null_mut(), "");
        let Some((entries, count, open, colors)) = self.comparator.as_ref().and_then(|comp| {
            let tabs = comp.contexts.get(source_index)?;
            let groups = &comp.groups[source_index];
            let open = comp
                .split
                .as_ref()
                .filter(|split| split.source_index == source_index)
                .and_then(|split| split.context_id)
                .and_then(|id| tabs.iter().position(|tab| tab.id == id));
            let colors: Vec<Rgb> = groups.iter().map(|group| group.color.rgb()).collect();
            Some((tab_list_entries(tabs, groups), tabs.len(), open, colors))
        }) else {
            return;
        };
        if count == 0 {
            return;
        }
        let point = self
            .bar_layout()
            .map(|layout| layout.tab_overflow[source_index.min(COMPARATOR_COLUMNS - 1)])
            .filter(|button| button.width > 0.0)
            .map(|button| {
                let mut point = POINT {
                    x: button.x.round() as i32,
                    y: (button.y + button.height).round() as i32,
                };
                unsafe {
                    ClientToScreen(hwnd, &mut point);
                }
                point
            })
            .unwrap_or_else(|| self.bar_menu_point(hwnd));

        // Escolher uma aba abre-a ao lado, com o teclado nela: o menu nao o
        // devolve a origem.
        let tab_item = |index: usize, label: &str| {
            MenuCommand::new(TAB_LIST_BASE + index, label)
                .checked(Some(index) == open)
                .moves_focus()
        };
        let mut menu = PopupMenu::default();
        for entry in &entries {
            match entry {
                TabListEntry::Tab { index, label } => menu.push(tab_item(*index, label)),
                TabListEntry::Group { group, name, tabs } => menu.push(MenuEntry::Submenu {
                    label: format!("{name} ({})", tabs.len()),
                    icon: Some(MenuIcon::Swatch(
                        colors.get(*group).copied().unwrap_or((0, 0, 0)),
                    )),
                    entries: tabs
                        .iter()
                        .map(|(index, label)| tab_item(*index, label).into())
                        .collect(),
                }),
            }
        }
        // Aberto pelo botao esquerdo: sem TPM_RIGHTBUTTON.
        let selected = self.track_menu(&menu, point, MenuButton::Left);
        if let Some(index) = tab_list_command(selected, count) {
            self.open_listed_tab(source_index, index);
        }
    }

    /// Uma aba escolhida na lista "‹N": passa a ancora da coluna e abre ao
    /// lado (se ja estava aberta, fica so a ancora).
    fn open_listed_tab(&mut self, source_index: usize, context_index: usize) {
        if let Some(comp) = &mut self.comparator
            && source_index < COMPARATOR_COLUMNS
            && let Some(id) = comp.contexts[source_index]
                .get(context_index)
                .map(|tab| tab.id)
        {
            comp.bar_focus[source_index] = Some(id);
        }
        self.open_context_tab(source_index, context_index);
        self.request_redraw();
    }

    pub(in crate::windows_app) fn context_menu_comparator(&mut self) {
        // Um arrasto a meio acaba aqui: o menu tem o seu proprio ciclo de
        // mensagens e o largar do botao esquerdo ja nao chegaria a barra.
        self.forget_tab_gesture();
        hover_tooltip(std::ptr::null_mut(), "");
        match bar_menu_for(self.comparator_bar_hit()) {
            Some(BarMenu::Tab {
                source_index,
                context_index,
            }) => self.show_tab_menu(source_index, context_index),
            Some(BarMenu::Group {
                source_index,
                group_index,
            }) => self.show_group_menu(source_index, group_index, false),
            Some(BarMenu::Column(col_index)) => self.column_pill_menu(col_index),
            None => {}
        }
    }

    /// Onde o menu do botao direito abre: no cursor, em coordenadas de ecra.
    fn bar_menu_point(&self, hwnd: HWND) -> POINT {
        let mut point = POINT {
            x: self.cursor.0.round() as i32,
            y: self.cursor.1.round() as i32,
        };
        unsafe {
            ClientToScreen(hwnd, &mut point);
        }
        point
    }

    fn show_tab_menu(&mut self, source_index: usize, context_index: usize) {
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };

        // Lidos antes de abrir o menu: durante o `TrackPopupMenu` ja nao ha
        // emprestimo do estado que sobreviva.
        let Some((joinable, colors, own_group)) = self.comparator.as_ref().and_then(|comp| {
            let tab = comp.contexts.get(source_index)?.get(context_index)?;
            let groups = &comp.groups[source_index];
            let joinable = Self::joinable_context_groups(groups, tab.group);
            let colors: Vec<Rgb> = joinable
                .iter()
                .map(|(index, _)| groups.get(*index).map_or((0, 0, 0), |g| g.color.rgb()))
                .collect();
            // O grupo da aba (id e cor atual), para o submenu "Cor do grupo".
            let own_group = tab
                .group
                .and_then(|id| groups.iter().find(|group| group.id == id))
                .map(|group| (group.id, group.color));
            Some((joinable, colors, own_group))
        }) else {
            return;
        };
        let in_group = own_group.is_some();
        let point = self.bar_menu_point(hwnd);

        let mut menu = PopupMenu::default();
        // Abrir poe o teclado na aba aberta: o menu nao o devolve a origem.
        menu.push(MenuCommand::new(TAB_MENU_OPEN, "Abrir").moves_focus());
        menu.push(MenuCommand::new(TAB_MENU_FULLSCREEN, "Abrir em tela cheia").moves_focus());
        menu.separator();
        menu.push(MenuCommand::new(TAB_MENU_CLOSE, "Fechar aba"));
        menu.push(MenuCommand::new(
            TAB_MENU_CLOSE_OTHERS,
            if in_group {
                "Fechar outras abas deste grupo"
            } else {
                "Fechar outras abas sem grupo"
            },
        ));
        menu.push(MenuCommand::new(
            TAB_MENU_CLOSE_ALL,
            if in_group {
                "Fechar todas deste grupo"
            } else {
                "Fechar todas as abas sem grupo"
            },
        ));
        menu.separator();
        menu.push(MenuCommand::new(
            TAB_MENU_NEW_GROUP,
            "Adicionar a um novo grupo",
        ));
        // "Mover para o grupo ▸": um submenu com os outros grupos da coluna,
        // cada um com a amostra da sua cor, como no Chrome.
        if !joinable.is_empty() {
            menu.push(MenuEntry::Submenu {
                label: "Mover para o grupo".to_string(),
                icon: None,
                entries: joinable
                    .iter()
                    .enumerate()
                    .map(|(offset, (_, name))| {
                        MenuCommand::new(TAB_MENU_GROUP_BASE + offset, name.as_str())
                            .icon(MenuIcon::Swatch(colors[offset]))
                            .into()
                    })
                    .collect(),
            });
        }
        if in_group {
            menu.push(MenuCommand::new(TAB_MENU_UNGROUP, "Remover do grupo"));
        }
        // "Cor do grupo ▸" numa aba agrupada: a cor muda-se tambem onde
        // estao as abas, nao so na pilula -- as mesmas amostras e ids do
        // menu do grupo, com a atual marcada.
        if let Some((_, current)) = own_group {
            menu.push(MenuEntry::Submenu {
                label: "Cor do grupo".to_string(),
                icon: None,
                entries: group_color_items(current),
            });
        }

        let selected = self.track_menu(&menu, point, MenuButton::Right);

        match tab_menu_command(selected, &joinable) {
            Some(TabMenuCommand::Open) => {
                self.open_context_tab(source_index, context_index);
            }
            Some(TabMenuCommand::Fullscreen) => {
                self.open_context_tab_fullscreen(source_index, context_index)
            }
            Some(TabMenuCommand::Close) => self.close_context_tab(source_index, context_index),
            Some(TabMenuCommand::CloseOthers) => {
                self.close_other_context_tabs(source_index, context_index)
            }
            Some(TabMenuCommand::CloseAll) => {
                self.close_all_context_tabs(source_index, context_index)
            }
            Some(TabMenuCommand::NewGroup) => self.group_context_tab(source_index, context_index),
            Some(TabMenuCommand::Ungroup) => self.ungroup_context_tab(source_index, context_index),
            Some(TabMenuCommand::MoveToGroup(group_index)) => {
                self.join_context_tab_group(source_index, context_index, group_index)
            }
            Some(TabMenuCommand::GroupColor(color)) => {
                if let Some((group_id, _)) = own_group {
                    self.apply_group_menu(source_index, group_id, GroupMenuCommand::Color(color));
                }
            }
            None => {}
        }
    }

    /// Por baixo da pilula `group_index` da coluna, em coordenadas de ecra:
    /// onde o Chrome abre o editor de um grupo acabado de criar.
    fn group_chip_point(
        &self,
        hwnd: HWND,
        source_index: usize,
        group_index: usize,
    ) -> Option<POINT> {
        let layout = self.bar_layout()?;
        let visual = (0..layout.group_pill_counts.get(source_index).copied()?)
            .find(|visual| layout.group_pill_indices[source_index][*visual] == group_index)?;
        let chip = layout.group_pills[source_index][visual];
        let mut point = POINT {
            x: chip.x.round() as i32,
            y: (chip.y + chip.height).round() as i32,
        };
        unsafe {
            ClientToScreen(hwnd, &mut point);
        }
        Some(point)
    }

    /// Botao direito na pilula de um grupo: a cor (com a atual marcada),
    /// recolher/expandir, desagrupar e fechar -- o menu do grupo do Chrome.
    /// `at_chip`: abre por baixo da pilula (o grupo acabou de nascer), nao no
    /// rato.
    fn show_group_menu(&mut self, source_index: usize, group_index: usize, at_chip: bool) {
        let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) else {
            return;
        };
        let Some((group_id, name, current, collapsed)) = self
            .comparator
            .as_ref()
            .and_then(|comp| comp.groups.get(source_index))
            .and_then(|groups| groups.get(group_index))
            .map(|group| (group.id, group.name.clone(), group.color, group.collapsed))
        else {
            return;
        };
        let point = at_chip
            .then(|| self.group_chip_point(hwnd, source_index, group_index))
            .flatten()
            .unwrap_or_else(|| self.bar_menu_point(hwnd));

        let mut menu = PopupMenu::default();
        // O nome do grupo por cima, cinzento: um titulo, nao um comando.
        menu.push(MenuCommand::new(0, format!("Grupo \u{201C}{name}\u{201D}")).disabled(""));
        menu.separator();
        menu.entries.extend(group_color_items(current));
        menu.separator();
        menu.push(MenuCommand::new(
            GROUP_MENU_TOGGLE,
            if collapsed {
                "Expandir grupo"
            } else {
                "Recolher grupo"
            },
        ));
        menu.push(MenuCommand::new(GROUP_MENU_UNGROUP, "Desagrupar"));
        menu.push(MenuCommand::new(GROUP_MENU_CLOSE, "Fechar grupo"));

        let selected = self.track_menu(&menu, point, MenuButton::Right);

        if let Some(command) = group_menu_command(selected) {
            self.apply_group_menu(source_index, group_id, command);
        }
    }

    /// Executa um comando do menu do grupo. Se fechar a aba que esta aberta
    /// ao lado, a gaveta fecha com ela.
    fn apply_group_menu(&mut self, source_index: usize, group_id: u64, command: GroupMenuCommand) {
        let Some(comp) = &mut self.comparator else {
            return;
        };
        if source_index >= COMPARATOR_COLUMNS {
            return;
        }
        let closed = apply_group_command(
            &mut comp.contexts[source_index],
            &mut comp.groups[source_index],
            group_id,
            command,
        );
        let closes_active = comp.split.as_ref().is_some_and(|split| {
            split.source_index == source_index
                && split.context_id.is_some_and(|id| closed.contains(&id))
        });
        if closes_active {
            self.close_split();
        }
        self.request_redraw();
    }

    /// Botao esquerdo em baixo na barra. Abas, o x delas e as pilulas dos
    /// grupos so decidem ao largar (podem virar arrasto); o resto responde ja.
    pub(in crate::windows_app) fn press_comparator(&mut self) {
        let hit = self.comparator_bar_hit();
        self.tab_gesture_count = self.tab_gesture_count.wrapping_add(1).max(1);
        let gesture = self.tab_gesture_count;
        self.tab_press = match (hit, self.bar_layout(), &self.comparator) {
            (Some(hit), Some(layout), Some(comp)) => tab_press(
                &layout,
                &comp.contexts,
                &comp.groups,
                hit,
                self.cursor,
                gesture,
            ),
            _ => None,
        };
        self.publish_tab_gesture();
        if self.tab_press.is_some() {
            hover_tooltip(std::ptr::null_mut(), "");
            return;
        }
        self.click_comparator(hit);
    }

    /// Diz ao subclass da janela que gesto tem o rato (0: nenhum).
    fn publish_tab_gesture(&self) {
        TAB_GESTURE_LIVE.store(
            self.tab_press.map_or(0, |press| press.gesture),
            Ordering::Release,
        );
    }

    /// Esquece o gesto da fila sem fazer nada com ele (a superficie mudou por
    /// baixo dele). O botao que ainda estiver em baixo solta o rato sozinho.
    pub(in crate::windows_app) fn forget_tab_gesture(&mut self) {
        self.tab_press = None;
        self.publish_tab_gesture();
    }

    /// Um passo do gesto sobre a fila de abas, contra a barra e o modelo de
    /// agora (`tab_gesture_step`). O arrasto so mexe na ordem e nos grupos
    /// das abas: nenhuma pagina navega nem recarrega, e a aba aberta ao lado
    /// continua aberta, porque e reconhecida pela identidade e nao pelo sitio.
    pub(in crate::windows_app) fn tab_gesture(
        &mut self,
        input: TabGestureInput,
    ) -> TabGestureEffect {
        if self.tab_press.is_none() {
            return TabGestureEffect::Ignored;
        }
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor().max(1.0));
        let layout = self.bar_layout();
        let effect = match (layout, &self.comparator) {
            (Some(layout), Some(comp)) => tab_gesture_step(
                &mut self.tab_press,
                input,
                &TabRowView {
                    layout: &layout,
                    contexts: &comp.contexts,
                    groups: &comp.groups,
                    scale,
                },
            ),
            // Sem comparador nao ha fila: o gesto que ficou para tras acaba.
            _ => TabGestureEffect::Cancelled {
                was_dragging: self.tab_press.take().is_some_and(|press| press.dragging),
            },
        };
        // O gesto sai de publicacao ANTES de o rato ser solto: o
        // WM_CAPTURECHANGED que o ReleaseCapture manda ja nao encontra nada.
        self.publish_tab_gesture();
        if matches!(input, TabGestureInput::Release { .. }) {
            self.release_tab_capture();
        }
        match effect {
            TabGestureEffect::Started => {
                self.hold_tab_capture();
                self.bar_hover = None;
                hover_tooltip(std::ptr::null_mut(), "");
                self.request_redraw();
            }
            TabGestureEffect::Moved | TabGestureEffect::Cancelled { was_dragging: true } => {
                self.request_redraw();
            }
            TabGestureEffect::Click(hit) => self.click_comparator(Some(hit)),
            TabGestureEffect::Drop { .. } => {
                if let Some(comp) = &mut self.comparator {
                    let _ = apply_tab_gesture(
                        &mut comp.contexts,
                        &mut comp.groups,
                        &mut comp.bar_focus,
                        effect,
                    );
                }
                self.request_redraw();
            }
            TabGestureEffect::Cancelled { .. }
            | TabGestureEffect::Pending
            | TabGestureEffect::Ignored => {}
        }
        effect
    }

    /// Enquanto se arrasta, o rato e desta janela mesmo fora dela: o largar
    /// chega sempre aqui, e largar fora da fila cancela. O winit ja prende o
    /// rato ao premir; isto garante-o se alguem o tiver soltado entretanto.
    fn hold_tab_capture(&self) {
        if let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) {
            unsafe {
                if GetCapture() != hwnd {
                    SetCapture(hwnd);
                }
            }
        }
    }

    /// Solta o rato no fim do gesto -- sempre DEPOIS de o gesto ter sido
    /// tirado, que o ReleaseCapture manda o WM_CAPTURECHANGED na hora.
    fn release_tab_capture(&self) {
        if let Some(hwnd) = self.window.as_ref().and_then(window_hwnd) {
            unsafe {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
            }
        }
    }

    /// Botao esquerdo largado: completa o clique, larga o arrasto ou nada.
    pub(in crate::windows_app) fn release_comparator(&mut self) {
        if self.tab_press.is_none() {
            return;
        }
        if self.surface != Surface::Comparator {
            self.forget_tab_gesture();
            return;
        }
        let hit = self.comparator_bar_hit();
        let _ = self.tab_gesture(TabGestureInput::Release {
            cursor: self.cursor,
            hit,
        });
        self.update_bar_hover();
        self.request_redraw();
    }

    /// Esc -- da janela, ou o "voltar" que a pagina manda quando o teclado
    /// esta nela: a meio de um arrasto cancela-o; fora dele e o "voltar".
    pub(in crate::windows_app) fn escape_or_back(&mut self) {
        // Com o painel de servicos em tela cheia, o Esc so sai da tela cheia
        // -- venha de onde vier (o teclado pode ter ficado numa coluna, por
        // baixo do painel).
        if self.service_covers_window() {
            self.service_input(ServiceInput::Escape);
            return;
        }
        if self.tab_gesture(TabGestureInput::Escape) == TabGestureEffect::Ignored {
            self.go_back();
        }
    }

    /// O que a barra desenha do arrasto em curso, se houver um.
    pub(in crate::windows_app) fn drag_paint(&self) -> Option<DragPaint> {
        let press = self.tab_press.filter(|press| press.dragging)?;
        let layout = self.bar_layout()?;
        let comp = self.comparator.as_ref()?;
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor().max(1.0));
        tab_drag_paint(
            press,
            &TabRowView {
                layout: &layout,
                contexts: &comp.contexts,
                groups: &comp.groups,
                scale,
            },
            self.cursor,
        )
    }

    /// O que um clique no alvo `hit` da barra faz. Os botoes respondem ao
    /// premir; abas, o x e as pilulas chegam aqui ao largar, pelo
    /// `release_comparator`.
    fn click_comparator(&mut self, hit: Option<BarHit>) {
        // O clique pode trocar a superficie; a dica nao fica a flutuar sobre
        // a tela nova.
        hover_tooltip(std::ptr::null_mut(), "");
        match hit {
            Some(BarHit::WindowMinimize) => {
                if let Some(window) = &self.window {
                    window.set_minimized(true);
                }
            }
            Some(BarHit::WindowMaximize) => {
                if let Some(window) = &self.window {
                    window.set_maximized(!window.is_maximized());
                }
            }
            Some(BarHit::WindowClose) => {
                let _ = self.proxy.send_event(UserEvent::ExitRequested);
            }
            Some(BarHit::Private) => self.open_private_panel(),
            Some(BarHit::Service(service)) => self.open_service_panel(service),
            Some(BarHit::ServiceStrip(button)) => self.service_input(button.input()),
            Some(BarHit::GmailToggle) => self.toggle_gmail_notifications(),
            Some(BarHit::Tool(_)) => {
                if let Some(action) = bar_tool_action(hit, ToolClick::Left) {
                    self.run_tool_action(action);
                }
            }
            Some(BarHit::GeminiLive) => self.toggle_live_panel(),
            Some(BarHit::Downloads) => self.toggle_downloads_panel(),
            Some(BarHit::SplitClose) => self.close_split(),
            Some(BarHit::SplitExpand) => self.toggle_split_fullscreen(),
            Some(BarHit::Home) => {
                self.request_home();
            }
            Some(BarHit::Back) => self.navigate_history(HistoryStep::Back),
            Some(BarHit::Forward) => self.navigate_history(HistoryStep::Forward),
            Some(BarHit::ColumnBack(index)) => self.navigate_column(index, HistoryStep::Back),
            Some(BarHit::ColumnForward(index)) => self.navigate_column(index, HistoryStep::Forward),
            Some(BarHit::Column(index)) => self.expand_comparator(index),
            Some(BarHit::AddTab(index)) => self.open_ai_palette(index),
            Some(BarHit::ContextTab {
                source_index,
                context_index,
            }) => {
                self.open_context_tab(source_index, context_index);
            }
            Some(BarHit::CloseTab {
                source_index,
                context_index,
            }) => self.close_context_tab(source_index, context_index),
            Some(BarHit::ContextGroup {
                source_index,
                group_index,
            }) => self.toggle_context_group(source_index, group_index),
            Some(BarHit::TabOverflow(index)) => self.show_tab_list_menu(index),
            None => {
                let scale = self
                    .window
                    .as_ref()
                    .map(|window| window.scale_factor().max(1.0))
                    .unwrap_or(1.0);
                if self.cursor.1 >= 0.0
                    && self.cursor.1 <= TITLE_TAB_HEIGHT * scale
                    && let Some(window) = &self.window
                {
                    let _ = window.drag_window();
                }
            }
        }
    }

    pub(in crate::windows_app) fn click_home(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let (x, y) = self.cursor;
        match home_click_target(
            (size.width as f64, size.height as f64),
            window.scale_factor(),
            self.pomodoro_bar_label(),
            x,
            y,
        ) {
            HomeClick::Tool(tool) => {
                if let Some(action) = tool_action(tool, ToolClick::Left) {
                    self.run_tool_action(action);
                }
            }
            // Sem a barra do Windows, a faixa de cima arrasta a janela -- como
            // a barra do comparador.
            HomeClick::Drag => {
                let _ = window.drag_window();
            }
            HomeClick::Go => {
                debug_log(format_args!("click_home: botao Ir"));
                self.submit_current();
            }
            HomeClick::Nothing => {}
        }
    }
}

/// As cores de um grupo como itens de menu, com a amostra de cada uma e a
/// `current` marcada: o menu do grupo e o submenu "Cor do grupo" de uma aba
/// agrupada usam os mesmos ids (`GROUP_MENU_COLOR_BASE`).
pub(in crate::windows_app) fn group_color_items(current: GroupColor) -> Vec<MenuEntry> {
    GroupColor::ALL
        .iter()
        .enumerate()
        .map(|(index, color)| {
            MenuCommand::new(GROUP_MENU_COLOR_BASE + index, group_color_label(*color))
                .icon(MenuIcon::Swatch(color.rgb()))
                .checked(*color == current)
                .into()
        })
        .collect()
}
