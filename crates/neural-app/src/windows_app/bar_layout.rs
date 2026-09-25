use super::*;

/// Cor do tempo e do contorno do botao do Pomodoro: a da fase
/// (`phase_color`: tomate no foco, verde nas pausas) acertada ao fundo do
/// botao para ler bem nos dois temas; sem sessao, a letra de sempre.
pub(in crate::windows_app) fn tool_label_color(
    phase: Option<Phase>,
    fill: Rgb,
    theme: &Theme,
) -> Rgb {
    match phase {
        Some(phase) => readable(phase_color(phase), fill, 4.5),
        None => theme.fg,
    }
}

/// Botao do rato que carregou numa ferramenta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum ToolClick {
    Left,
    Right,
}

/// O que um clique numa ferramenta faz -- na barra ou na Home, e o mesmo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum ToolAction {
    /// Inicia, pausa ou retoma o Pomodoro (`App::pomodoro_click`).
    PomodoroClick,
    /// Menu de opcoes do Pomodoro (`App::pomodoro_menu`).
    PomodoroMenu,
    /// Abre o painel do Ctrl+H nas notas; aberto, fecha-o.
    ToggleNotes,
    /// Abre o video da respiracao no painel anonimo; aberto, fecha-o.
    ToggleBreath,
}

/// A unica tabela clique -> accao das ferramentas. O botao direito so faz
/// alguma coisa no Pomodoro: nas outras duas nao ha menu, e um clique direito
/// perdido nao pode abrir nem fechar paineis.
pub(in crate::windows_app) fn tool_action(tool: Tool, click: ToolClick) -> Option<ToolAction> {
    match (tool, click) {
        (Tool::Pomodoro, ToolClick::Left) => Some(ToolAction::PomodoroClick),
        (Tool::Pomodoro, ToolClick::Right) => Some(ToolAction::PomodoroMenu),
        (Tool::Notes, ToolClick::Left) => Some(ToolAction::ToggleNotes),
        (Tool::Breath, ToolClick::Left) => Some(ToolAction::ToggleBreath),
        (Tool::Notes | Tool::Breath, ToolClick::Right) => None,
    }
}

/// Clique na barra do comparador: so os botoes das ferramentas dao uma
/// `ToolAction`; o resto da barra segue o caminho que ja tinha.
pub(in crate::windows_app) fn bar_tool_action(
    hit: Option<BarHit>,
    click: ToolClick,
) -> Option<ToolAction> {
    match hit {
        Some(BarHit::Tool(tool)) => tool_action(tool, click),
        _ => None,
    }
}

/// Etiqueta curta ao lado do icone do Pomodoro ("mm:ss"). Tamanho fixo e
/// `Copy` para poder andar dentro de `BarColumns`: desenho e hit-testing leem
/// a MESMA etiqueta, e por isso a mesma largura. Leva tambem a fase da
/// sessao, que so o desenho usa (a cor do tempo); a largura nao depende dela.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct BarLabel {
    pub(in crate::windows_app) bytes: [u8; BAR_LABEL_MAX_BYTES],
    pub(in crate::windows_app) len: u8,
    pub(in crate::windows_app) phase: Option<Phase>,
}

pub(in crate::windows_app) const BAR_LABEL_MAX_BYTES: usize = 16;
/// Largura reservada por caractere e margem da etiqueta, em pixeis logicos a
/// letra de 13 px da barra. Reserva-se por caractere e nao pelo texto medido:
/// "11:11" e "00:00" ocupam o mesmo, e a barra nao treme a cada segundo.
pub(in crate::windows_app) const BAR_LABEL_CHAR_WIDTH: f64 = 7.5;
pub(in crate::windows_app) const BAR_LABEL_PADDING: f64 = 8.0;

impl BarLabel {
    /// `None` para texto vazio. Texto comprido e cortado numa fronteira de
    /// caractere, nunca a meio de um.
    pub(in crate::windows_app) fn new(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut end = text.len().min(BAR_LABEL_MAX_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0u8; BAR_LABEL_MAX_BYTES];
        bytes[..end].copy_from_slice(&text.as_bytes()[..end]);
        Some(Self {
            bytes,
            len: end as u8,
            phase: None,
        })
    }

    /// A mesma etiqueta, pintada na cor de `phase` (`tool_label_color`).
    pub(in crate::windows_app) fn with_phase(self, phase: Option<Phase>) -> Self {
        Self { phase, ..self }
    }

    pub(in crate::windows_app) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    /// Quanto o botao alarga para a etiqueta, em pixeis logicos.
    pub(in crate::windows_app) fn width(&self) -> f64 {
        self.as_str().chars().count() as f64 * BAR_LABEL_CHAR_WIDTH + BAR_LABEL_PADDING
    }
}

impl std::fmt::Debug for BarLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BarLabel({:?})", self.as_str())
    }
}

/// Os tres botoes das ferramentas, encostados a `right`: Respiracao na ponta,
/// Notas antes e o Pomodoro por ultimo -- e so ele alarga para a esquerda
/// com a etiqueta, para os outros dois nao saltarem quando ela aparece.
pub(in crate::windows_app) fn tool_button_row(
    right: f64,
    y: f64,
    size: f64,
    gap: f64,
    label_width: f64,
) -> [UiRect; 3] {
    let breath = UiRect {
        x: right - size,
        y,
        width: size,
        height: size,
    };
    let notes = UiRect {
        x: breath.x - gap - size,
        ..breath
    };
    let pomodoro = UiRect {
        x: notes.x - gap - size - label_width,
        width: size + label_width,
        ..breath
    };
    [pomodoro, notes, breath]
}

/// O estado do comparador de que a barra precisa. Anda sempre junto -- quem
/// arrasta um divisor muda os pesos, quem minimiza muda as duas coisas -- e
/// agrupa-lo evita que a barra receba uma parte e esqueca a outra, que era
/// exactamente como os rotulos deixavam de estar sobre as colunas.
#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct BarColumns {
    pub(in crate::windows_app) count: usize,
    pub(in crate::windows_app) weights: [f64; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) minimized: [bool; COMPARATOR_COLUMNS],
    /// Ha uma gaveta aberta: os botoes do Split ocupam o canto direito e os
    /// chips tem de parar antes deles.
    pub(in crate::windows_app) split_active: bool,
    /// Largura logica do painel lateral a direita; as colunas ficam antes dele.
    pub(in crate::windows_app) panel_width: f64,
    /// Tempo que falta no Pomodoro, ao lado do icone dele; alarga o botao.
    pub(in crate::windows_app) pomodoro_label: Option<BarLabel>,
}

impl BarColumns {
    /// Colunas iguais, nenhuma minimizada -- o estado de partida, e o que os
    /// testes de geometria usam quando os pesos nao sao o assunto.
    pub(in crate::windows_app) fn even(count: usize) -> Self {
        Self {
            count,
            weights: [1.0; COMPARATOR_COLUMNS],
            minimized: [false; COMPARATOR_COLUMNS],
            split_active: false,
            panel_width: 0.0,
            pomodoro_label: None,
        }
    }
}

/// Geometria em duas linhas. As fontes ficam na title bar; os provedores ficam
/// numa segunda linha, sem disputar espaco com as abas.
#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct BarLayout {
    pub(in crate::windows_app) visible: bool,
    pub(in crate::windows_app) height: f64,
    pub(in crate::windows_app) home: UiRect,
    pub(in crate::windows_app) back: UiRect,
    pub(in crate::windows_app) forward: UiRect,
    pub(in crate::windows_app) column_back: [UiRect; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) column_forward: [UiRect; COMPARATOR_COLUMNS],
    /// Por coluna: a pilula do provedor sobre a sua faixa, ou -- se estiver
    /// minimizada -- o chip compacto encostado aos controlos da direita.
    pub(in crate::windows_app) columns: [UiRect; COMPARATOR_COLUMNS],
    /// Quais das `columns` sao chips. O desenho precisa de saber porque o
    /// chip e apagado e nao leva o botao "+".
    pub(in crate::windows_app) minimized: [bool; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) add_tabs: [UiRect; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) context_tabs:
        [[UiRect; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) context_indices:
        [[usize; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) context_tab_counts: [usize; COMPARATOR_COLUMNS],
    /// O x de fechar de cada aba visivel; sem largura quando a aba e estreita
    /// de mais para o ter sem esconder o titulo.
    pub(in crate::windows_app) tab_closes: [[UiRect; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    /// O grupo (indice na coluna) de cada aba visivel.
    pub(in crate::windows_app) tab_owners:
        [[Option<usize>; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
    /// Pilulas dos grupos, intercaladas com as abas na mesma fila.
    pub(in crate::windows_app) group_pills: [[UiRect; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) group_pill_indices:
        [[usize; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) group_pill_counts: [usize; COMPARATOR_COLUMNS],
    /// O sublinhado de cada pilula, na cor do grupo: vai da pilula ao fim da
    /// ultima aba do grupo que esta a vista. Sem largura quando o grupo esta
    /// recolhido e nenhuma aba dele aparece.
    pub(in crate::windows_app) group_lines: [[UiRect; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
    /// O "‹N" de cada coluna: abre a lista de todas as abas dela. So existe
    /// quando ha abas que a barra nao mostra (nem guarda atras de uma pilula
    /// recolhida) -- as antigas que o corte deixou de fora, ou as que a
    /// largura da janela nao deixou desenhar.
    pub(in crate::windows_app) tab_overflow: [UiRect; COMPARATOR_COLUMNS],
    /// Quantas abas o "‹N" de cada coluna esconde (0: nao ha botao).
    pub(in crate::windows_app) tab_overflow_counts: [usize; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) columns_len: usize,
    pub(in crate::windows_app) window_minimize: UiRect,
    pub(in crate::windows_app) window_maximize: UiRect,
    pub(in crate::windows_app) window_close: UiRect,
}

impl BarLayout {
    #[cfg(test)]
    pub(in crate::windows_app) fn new(
        client_width: f64,
        scale: f64,
        visible: bool,
        columns: usize,
    ) -> Self {
        Self::with_contexts(
            client_width,
            scale,
            visible,
            BarColumns::even(columns),
            [0; COMPARATOR_COLUMNS],
        )
    }

    /// Atalho para quem so sabe quantas abas tem cada coluna: nenhuma delas
    /// esta agrupada.
    #[cfg(test)]
    pub(in crate::windows_app) fn with_contexts(
        client_width: f64,
        scale: f64,
        visible: bool,
        columns: BarColumns,
        context_counts: [usize; COMPARATOR_COLUMNS],
    ) -> Self {
        Self::with_rows(
            client_width,
            scale,
            visible,
            columns,
            std::array::from_fn(|index| TabRow::plain(context_counts[index])),
        )
    }

    pub(in crate::windows_app) fn with_rows(
        client_width: f64,
        scale: f64,
        visible: bool,
        columns: BarColumns,
        rows: [TabRow; COMPARATOR_COLUMNS],
    ) -> Self {
        let scale = scale.max(1.0);
        let empty = UiRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        if !visible {
            return Self {
                visible: false,
                height: 0.0,
                home: empty,
                back: empty,
                forward: empty,
                column_back: [empty; COMPARATOR_COLUMNS],
                column_forward: [empty; COMPARATOR_COLUMNS],
                columns: [empty; COMPARATOR_COLUMNS],
                minimized: [false; COMPARATOR_COLUMNS],
                add_tabs: [empty; COMPARATOR_COLUMNS],
                context_tabs: [[empty; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                context_indices: [[0; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                context_tab_counts: [0; COMPARATOR_COLUMNS],
                tab_closes: [[empty; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                tab_owners: [[None; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS],
                group_pills: [[empty; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
                group_pill_indices: [[0; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
                group_pill_counts: [0; COMPARATOR_COLUMNS],
                group_lines: [[empty; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS],
                tab_overflow: [empty; COMPARATOR_COLUMNS],
                tab_overflow_counts: [0; COMPARATOR_COLUMNS],
                columns_len: 0,
                window_minimize: empty,
                window_maximize: empty,
                window_close: empty,
            };
        }

        let height = COMPARATOR_CHROME_HEIGHT * scale;
        let title_h = TITLE_TAB_HEIGHT * scale;
        let caption_w = 46.0 * scale;
        let window_close = UiRect {
            x: (client_width - caption_w).max(0.0),
            y: 0.0,
            width: caption_w,
            height: title_h,
        };
        let window_maximize = UiRect {
            x: (window_close.x - caption_w).max(0.0),
            y: 0.0,
            width: caption_w,
            height: title_h,
        };
        let window_minimize = UiRect {
            x: (window_maximize.x - caption_w).max(0.0),
            y: 0.0,
            width: caption_w,
            height: title_h,
        };

        let pad = 7.0 * scale;
        let row_y = title_h + 7.0 * scale;
        let row_h = 30.0 * scale;
        let home = UiRect {
            x: pad,
            y: row_y,
            width: 72.0 * scale,
            height: row_h,
        };

        let mut columns_rect = [empty; COMPARATOR_COLUMNS];
        let mut plus_rect = [empty; COMPARATOR_COLUMNS];
        let mut column_back = [empty; COMPARATOR_COLUMNS];
        let mut column_forward = [empty; COMPARATOR_COLUMNS];
        let mut tabs = [[empty; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS];
        let mut tab_indices = [[0usize; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS];
        let mut tab_counts = [0usize; COMPARATOR_COLUMNS];
        let mut pills = [[empty; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS];
        let mut pill_indices = [[0usize; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS];
        let mut pill_counts = [0usize; COMPARATOR_COLUMNS];
        let mut closes = [[empty; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS];
        let mut owners = [[None; MAX_VISIBLE_CONTEXT_TABS]; COMPARATOR_COLUMNS];
        let mut lines = [[empty; MAX_VISIBLE_TAB_SLOTS]; COMPARATOR_COLUMNS];
        let columns_len = columns.count.min(COMPARATOR_COLUMNS);

        // Linha dos provedores, agora livre das abas. As faixas vem da MESMA
        // funcao que posiciona os WebViews: depois de arrastar um divisor o
        // rotulo continua sobre a sua coluna, e o hit-testing com ele.
        let spans = visible_column_spans(
            (client_width / scale - columns.panel_width).max(1.0),
            columns.count,
            &columns.weights,
            &columns.minimized,
        );
        let group_pad = 6.0 * scale;
        let gap = 4.0 * scale;
        let provider_width = 116.0 * scale;
        let plus_width = 26.0 * scale;
        let chip_w = 62.0 * scale;
        let chip_gap = 5.0 * scale;

        // Os chips das colunas minimizadas e os controlos da direita sao
        // reservados ANTES de distribuir as pilulas. Ao contrario, a pilula
        // estendia-se ate a borda da janela e aterrava por cima do botao
        // "Privado" ou de um chip -- e como o hit-testing resolve por ordem de
        // indice, o clique ia parar a coluna errada.
        let hidden: Vec<usize> = (0..columns_len)
            .filter(|index| columns.minimized[*index])
            .collect();
        let chips_w = if hidden.is_empty() {
            0.0
        } else {
            hidden.len() as f64 * chip_w + chip_gap * hidden.len().saturating_sub(1) as f64
        };
        let controls = right_controls(
            client_width,
            scale,
            columns.split_active,
            columns.pomodoro_label,
        );
        let controls_left = controls.leftmost();
        let (back, forward) = controls.split_nav.unwrap_or((empty, empty));
        let reserved = if hidden.is_empty() {
            0.0
        } else {
            chips_w + 8.0 * scale
        };
        let bar_right = (controls_left - 8.0 * scale - reserved).max(pad);

        for (slot, span) in spans.iter().enumerate() {
            let mut left = span.x * scale + group_pad;
            if slot == 0 {
                left = left.max(home.x + home.width + 8.0 * scale);
            }
            let right = ((span.x + span.width) * scale - group_pad).min(bar_right);
            // O `.max()` que aqui estava punha o chao ACIMA do tecto: garantia
            // `available >= provider_width + plus_width + gap`, o que tornava o
            // `.min()` de baixo matematicamente morto e a pilula nunca encolhia.
            let available = (right - left).max(0.0);
            // "+", ‹ e › depois da pilula: ela encolhe primeiro.
            let nav_width = plus_width;
            let nav_gap = 4.0 * scale;
            let reserved_after = plus_width + gap + 2.0 * (nav_width + nav_gap);
            let pill = provider_width.min((available - reserved_after).max(0.0));
            columns_rect[span.index] = UiRect {
                x: left,
                y: row_y,
                width: pill,
                height: row_h,
            };
            // O "+" fica sempre dentro da faixa da sua coluna. Se nao couber,
            // desaparece -- em vez de ficar invisivel mas clicavel por cima do
            // vizinho, que e o pior dos dois mundos.
            let plus_x = left + pill + gap;
            plus_rect[span.index] = UiRect {
                x: plus_x,
                y: row_y + 2.0 * scale,
                width: if plus_x + plus_width <= right {
                    plus_width
                } else {
                    0.0
                },
                height: row_h - 4.0 * scale,
            };
            // ‹ e › desta IA. Tal como o "+", ou cabem na faixa ou nao existem.
            let back_x = plus_x + plus_width + nav_gap;
            let forward_x = back_x + nav_width + nav_gap;
            let fits = forward_x + nav_width <= right;
            column_back[span.index] = UiRect {
                x: back_x,
                y: row_y + 2.0 * scale,
                width: if fits { nav_width } else { 0.0 },
                height: row_h - 4.0 * scale,
            };
            column_forward[span.index] = UiRect {
                x: forward_x,
                width: if fits { nav_width } else { 0.0 },
                ..column_back[span.index]
            };
        }

        // Colunas minimizadas: nao tem faixa, mas nao podem desaparecer da
        // barra -- e o chip que as traz de volta com um clique. Encostam-se a
        // direita, logo antes de Privado/Split, para nao roubarem espaco as
        // colunas que estao mesmo a ser vistas.
        if !hidden.is_empty() {
            // O espaco ja foi reservado acima; o `.max(bar_right)` garante que
            // os chips nunca recuam para dentro da faixa das pilulas, mesmo com
            // a janela absurdamente estreita.
            let mut x = (controls_left - 8.0 * scale - chips_w).max(bar_right);
            for index in hidden {
                columns_rect[index] = UiRect {
                    x,
                    y: row_y,
                    width: chip_w,
                    height: row_h,
                };
                x += chip_w + chip_gap;
            }
        }

        // Linha superior: todas as fontes/abas, antes das ferramentas e dos
        // controles da janela. A etiqueta do Pomodoro ja vai reservada: as
        // abas nao mexem quando ele arranca.
        let tabs_left = 90.0 * scale;
        let tabs_right = (title_tools_left(client_width, scale, columns.pomodoro_label)
            - 8.0 * scale)
            .min(window_minimize.x - 8.0 * scale)
            .max(tabs_left);
        let visible_rows = &rows[..columns_len];
        let total_slots: usize = visible_rows.iter().map(|row| row.len).sum();
        let mut overflow = [empty; COMPARATOR_COLUMNS];
        let mut overflow_counts = [0usize; COMPARATOR_COLUMNS];
        if (total_slots > 0 || visible_rows.iter().any(|row| row.total_tabs > 0))
            && tabs_right > tabs_left
        {
            let metrics = TitleRowMetrics::fit(visible_rows, tabs_right - tabs_left, scale);
            let gap = metrics.gap;
            let budgets = metrics.budgets(visible_rows, tabs_right - tabs_left);
            let mut x = tabs_left;
            let tab_y = 3.0 * scale;
            let tab_h = (title_h - 6.0 * scale).max(20.0 * scale);

            let close_size = TAB_CLOSE_SIZE * scale;
            let line_h = GROUP_LINE_HEIGHT * scale;
            for index in 0..columns_len {
                // O que cabe na parte da faixa que e desta coluna: saem
                // primeiro as abas antigas, nunca a aberta ao lado nem a que
                // acabou de mexer, nunca uma pilula antes das suas abas. O
                // que sai conta no "‹N" -- nada desaparece sem aviso.
                let (kept, hidden) = metrics.fit_row(&rows[index], budgets[index]);
                if hidden > 0 && x + metrics.overflow <= tabs_right {
                    overflow[index] = UiRect {
                        x,
                        y: tab_y,
                        width: metrics.overflow,
                        height: tab_h,
                    };
                    overflow_counts[index] = hidden;
                    x += metrics.overflow + gap;
                }
                // A pilula cujo sublinhado ainda pode crescer: (lugar visual
                // da pilula, indice do grupo).
                let mut open_line: Option<(usize, usize)> = None;
                for (position, slot) in rows[index].visible().iter().copied().enumerate() {
                    if !kept[position] {
                        continue;
                    }
                    let width = metrics.width(slot);
                    if x + width > tabs_right + 0.5 {
                        break;
                    }
                    let width = width.min(tabs_right - x);
                    let rect = UiRect {
                        x,
                        y: tab_y,
                        width,
                        height: tab_h,
                    };
                    match slot {
                        TabSlot::Group(group) => {
                            let visual = pill_counts[index];
                            pills[index][visual] = rect;
                            pill_indices[index][visual] = group;
                            pill_counts[index] += 1;
                            // Nasce sem largura: so as abas do grupo que
                            // aparecem a estendem.
                            lines[index][visual] = UiRect {
                                x: rect.x,
                                y: tab_y + tab_h,
                                width: 0.0,
                                height: line_h,
                            };
                            open_line = Some((visual, group));
                        }
                        TabSlot::Tab(context) => {
                            let visual = tab_counts[index];
                            if visual >= MAX_VISIBLE_CONTEXT_TABS {
                                continue;
                            }
                            let owner = rows[index].owner(position);
                            tabs[index][visual] = rect;
                            tab_indices[index][visual] = context;
                            owners[index][visual] = owner;
                            closes[index][visual] = tab_close_rect(rect, scale, close_size);
                            tab_counts[index] += 1;
                            match open_line {
                                Some((pill, group)) if owner == Some(group) => {
                                    let line = &mut lines[index][pill];
                                    line.width = rect.x + rect.width - line.x;
                                }
                                _ => open_line = None,
                            }
                        }
                    }
                    x += width + gap;
                }
            }
        }

        Self {
            visible: true,
            height,
            home,
            back,
            forward,
            column_back,
            column_forward,
            columns: columns_rect,
            minimized: columns.minimized,
            add_tabs: plus_rect,
            context_tabs: tabs,
            context_indices: tab_indices,
            context_tab_counts: tab_counts,
            tab_closes: closes,
            tab_owners: owners,
            group_pills: pills,
            group_pill_indices: pill_indices,
            group_pill_counts: pill_counts,
            group_lines: lines,
            tab_overflow: overflow,
            tab_overflow_counts: overflow_counts,
            columns_len,
            window_minimize,
            window_maximize,
            window_close,
        }
    }

    pub(in crate::windows_app) fn hit(&self, x: f64, y: f64) -> Option<BarHit> {
        if !self.visible || y > self.height {
            return None;
        }
        if self.window_close.contains(x, y) {
            return Some(BarHit::WindowClose);
        }
        if self.window_maximize.contains(x, y) {
            return Some(BarHit::WindowMaximize);
        }
        if self.window_minimize.contains(x, y) {
            return Some(BarHit::WindowMinimize);
        }
        for index in 0..self.columns_len {
            if self.tab_overflow[index].contains(x, y) {
                return Some(BarHit::TabOverflow(index));
            }
            for visual in 0..self.group_pill_counts[index] {
                if self.group_pills[index][visual].contains(x, y) {
                    return Some(BarHit::ContextGroup {
                        source_index: index,
                        group_index: self.group_pill_indices[index][visual],
                    });
                }
            }
            for visual in 0..self.context_tab_counts[index] {
                // O x esta dentro da aba: ganha-lhe o clique.
                if self.tab_closes[index][visual].contains(x, y) {
                    return Some(BarHit::CloseTab {
                        source_index: index,
                        context_index: self.context_indices[index][visual],
                    });
                }
                if self.context_tabs[index][visual].contains(x, y) {
                    return Some(BarHit::ContextTab {
                        source_index: index,
                        context_index: self.context_indices[index][visual],
                    });
                }
            }
        }
        if self.home.contains(x, y) {
            return Some(BarHit::Home);
        }
        if self.back.contains(x, y) {
            return Some(BarHit::Back);
        }
        if self.forward.contains(x, y) {
            return Some(BarHit::Forward);
        }
        for index in 0..self.columns_len {
            if self.column_back[index].contains(x, y) {
                return Some(BarHit::ColumnBack(index));
            }
            if self.column_forward[index].contains(x, y) {
                return Some(BarHit::ColumnForward(index));
            }
        }
        for index in 0..self.columns_len {
            if self.add_tabs[index].contains(x, y) {
                return Some(BarHit::AddTab(index));
            }
            if self.columns[index].contains(x, y) {
                return Some(BarHit::Column(index));
            }
        }
        None
    }

    /// Alguma aba do grupo `group_index` da coluna esta desenhada?
    pub(in crate::windows_app) fn group_members_drawn(
        &self,
        column: usize,
        group_index: usize,
    ) -> bool {
        column < self.columns_len
            && (0..self.context_tab_counts[column])
                .any(|visual| self.tab_owners[column][visual] == Some(group_index))
    }

    /// A fila de uma coluna -- pilulas e abas -- da esquerda para a direita,
    /// tal como esta desenhada. E o que o arrasto usa para saber onde larga.
    pub(in crate::windows_app) fn row_items(&self, column: usize) -> Vec<RowItem> {
        if column >= self.columns_len {
            return Vec::new();
        }
        let mut items: Vec<RowItem> = (0..self.group_pill_counts[column])
            .map(|visual| RowItem {
                kind: RowKind::Chip(self.group_pill_indices[column][visual]),
                rect: self.group_pills[column][visual],
            })
            .chain((0..self.context_tab_counts[column]).map(|visual| RowItem {
                kind: RowKind::Tab {
                    context: self.context_indices[column][visual],
                    owner: self.tab_owners[column][visual],
                },
                rect: self.context_tabs[column][visual],
            }))
            .collect();
        items.sort_by(|a, b| a.rect.x.total_cmp(&b.rect.x));
        items
    }
}

/// Lado do x de fechar de cada aba e do seu alvo, em pixeis logicos.
pub(in crate::windows_app) const TAB_CLOSE_SIZE: f64 = 16.0;
/// Espessura do sublinhado de um grupo.
pub(in crate::windows_app) const GROUP_LINE_HEIGHT: f64 = 2.0;

/// O x de fechar, encostado a direita da aba e centrado na altura. Uma aba
/// estreita de mais nao o leva (como no Chrome): so sobraria o x.
pub(in crate::windows_app) fn tab_close_rect(tab: UiRect, scale: f64, size: f64) -> UiRect {
    if tab.width < 48.0 * scale {
        return UiRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }
    UiRect {
        x: tab.x + tab.width - size - 6.0 * scale,
        y: tab.y + (tab.height - size) / 2.0,
        width: size,
        height: size,
    }
}

/// Como se pinta o x de uma aba. Como no Chrome, esta em TODAS as abas com
/// largura para ele (`tab_close_rect` tira-o as estreitas), discreto, na cor
/// do texto apagado; sob o proprio rato fica vermelho com a cruz branca -- o
/// mesmo vermelho do fechar da janela. Antes so aparecia com o rato na aba:
/// parada, a barra nao mostrava x nenhum.
pub(in crate::windows_app) fn tab_close_style(
    close_hovered: bool,
    tab_fill: Rgb,
    theme: &Theme,
) -> PillStyle {
    if close_hovered {
        return caption_button_style(2, true, theme);
    }
    PillStyle::new(tab_fill, tab_fill, theme.fg_muted)
}

/// A largura mais estreita a que uma aba desce antes de sair da barra: fica
/// so o comeco do titulo (o x de fechar some abaixo de 48 px, como no Chrome).
pub(in crate::windows_app) const TAB_MIN_WIDTH: f64 = 36.0;
pub(in crate::windows_app) const TAB_MAX_WIDTH: f64 = 156.0;
/// A pilula de um grupo e um rotulo, nao um titulo de pagina: largura fixa,
/// que so encolhe quando as abas ja nao podem.
pub(in crate::windows_app) const GROUP_CHIP_WIDTH: f64 = 74.0;
pub(in crate::windows_app) const GROUP_CHIP_MIN_WIDTH: f64 = 44.0;
/// O "‹N" de uma coluna com abas fora da vista.
pub(in crate::windows_app) const TAB_OVERFLOW_WIDTH: f64 = 30.0;

/// Larguras da fila de abas da barra de titulo, e o corte de cada coluna
/// quando nem assim cabe tudo.
#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct TitleRowMetrics {
    pub(in crate::windows_app) gap: f64,
    pub(in crate::windows_app) chip: f64,
    pub(in crate::windows_app) tab: f64,
    pub(in crate::windows_app) overflow: f64,
}

impl TitleRowMetrics {
    /// Primeiro encolhem as abas (ate `TAB_MIN_WIDTH`), depois as pilulas (ate
    /// `GROUP_CHIP_MIN_WIDTH`); o que mesmo assim nao couber sai por
    /// `fit_row`. Antes, abaixo dos 56 px, os lugares do fim ficavam de fora
    /// -- a terceira coluna inteira a 800 px -- e uma pilula entrava espremida
    /// sem nenhuma das suas abas, com cara de recolhida.
    pub(in crate::windows_app) fn fit(rows: &[TabRow], area: f64, scale: f64) -> Self {
        let gap = 3.0 * scale;
        let overflow = TAB_OVERFLOW_WIDTH * scale;
        let (mut tabs, mut chips, mut buttons) = (0usize, 0usize, 0usize);
        for row in rows {
            for slot in row.visible() {
                match slot {
                    TabSlot::Tab(_) => tabs += 1,
                    TabSlot::Group(_) => chips += 1,
                }
            }
            if row.hidden_tabs(&[true; MAX_VISIBLE_TAB_SLOTS]) > 0 {
                buttons += 1;
            }
        }
        // Cada coisa leva o seu espaco a direita; a faixa ganha um no fim.
        let room = area + gap - gap * (tabs + chips + buttons) as f64 - overflow * buttons as f64;
        let mut chip = GROUP_CHIP_WIDTH * scale;
        let tab = if tabs == 0 {
            0.0
        } else {
            ((room - chip * chips as f64) / tabs as f64)
                .clamp(TAB_MIN_WIDTH * scale, TAB_MAX_WIDTH * scale)
        };
        if chips > 0 && room - chip * chips as f64 - tab * (tabs as f64) < 0.0 {
            chip = ((room - tab * tabs as f64) / chips as f64)
                .clamp(GROUP_CHIP_MIN_WIDTH * scale, chip);
        }
        Self {
            gap,
            chip,
            tab,
            overflow,
        }
    }

    pub(in crate::windows_app) fn width(&self, slot: TabSlot) -> f64 {
        match slot {
            TabSlot::Group(_) => self.chip,
            TabSlot::Tab(_) => self.tab,
        }
    }

    /// Quanto ocupa a fila com os lugares `kept` (e o "‹N", se `button`),
    /// cada coisa com o seu espaco a direita.
    pub(in crate::windows_app) fn need(&self, row: &TabRow, kept: &[bool], button: bool) -> f64 {
        let slots: f64 = row
            .visible()
            .iter()
            .zip(kept)
            .filter(|(_, kept)| **kept)
            .map(|(slot, _)| self.width(*slot) + self.gap)
            .sum();
        slots
            + if button {
                self.overflow + self.gap
            } else {
                0.0
            }
    }

    /// A parte da faixa de cada coluna. Se tudo cabe, cada uma leva o que
    /// pede; senao reparte-se por igual, e o que uma coluna curta nao usa
    /// passa as outras -- a ultima IA nunca fica sem abas por a primeira ter
    /// muitas.
    pub(in crate::windows_app) fn budgets(
        &self,
        rows: &[TabRow],
        area: f64,
    ) -> [f64; COMPARATOR_COLUMNS] {
        let count = rows.len().min(COMPARATOR_COLUMNS);
        let all = [true; MAX_VISIBLE_TAB_SLOTS];
        let mut demand = [0.0; COMPARATOR_COLUMNS];
        for (index, row) in rows.iter().enumerate().take(count) {
            demand[index] = self.need(row, &all, row.hidden_tabs(&all) > 0);
        }
        let area = area + self.gap;
        if demand.iter().sum::<f64>() <= area {
            return demand;
        }
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by(|a, b| demand[*a].total_cmp(&demand[*b]));
        let mut budgets = [0.0; COMPARATOR_COLUMNS];
        let mut left = area;
        for (done, index) in order.iter().enumerate() {
            budgets[*index] = demand[*index].min(left / (count - done) as f64);
            left -= budgets[*index];
        }
        budgets
    }

    /// Que lugares da fila cabem em `budget`, e quantas abas ficam fora da
    /// vista -- o numero do "‹N", cujo botao tambem conta na largura.
    pub(in crate::windows_app) fn fit_row(
        &self,
        row: &TabRow,
        budget: f64,
    ) -> ([bool; MAX_VISIBLE_TAB_SLOTS], usize) {
        let mut kept = [false; MAX_VISIBLE_TAB_SLOTS];
        for slot in kept.iter_mut().take(row.len) {
            *slot = true;
        }
        loop {
            let hidden = row.hidden_tabs(&kept);
            if self.need(row, &kept, hidden > 0) <= budget + 1e-6 {
                return (kept, hidden);
            }
            let victims = cut_victims(row, &kept);
            if victims.is_empty() {
                return (kept, hidden);
            }
            for victim in victims {
                kept[victim] = false;
            }
        }
    }
}

/// O que sai a seguir de uma fila que nao cabe: o lugar mais a esquerda (o
/// mais antigo), menos as ancoras -- a aba aberta ao lado e a acabada de
/// mexer, que so saem quando nada mais pode. Uma pilula nunca sai antes das
/// suas abas (nunca fica uma aba agrupada sem pilula), e sai JUNTO com a
/// ultima delas: uma pilula aberta sozinha parecia recolhida e o clique
/// nela recolhia sem se ver nada (so a pilula de um grupo que ja estava
/// inteiro fora do corte fica sozinha -- e o clique nela mostra-o).
pub(in crate::windows_app) fn cut_victims(row: &TabRow, kept: &[bool]) -> Vec<usize> {
    let alive = |position: usize| kept.get(position).copied().unwrap_or(false);
    let is_tab = |position: usize| matches!(row.slots[position], TabSlot::Tab(_));
    let chip_of =
        |group: usize| (0..row.len).find(|position| row.slots[*position] == TabSlot::Group(group));
    let members = |group: usize, only_kept: bool| {
        (0..row.len)
            .filter(|position| {
                (!only_kept || alive(*position))
                    && is_tab(*position)
                    && row.owners[*position] == Some(group)
            })
            .count()
    };
    let droppable = |position: usize, anchors_too: bool| {
        (anchors_too || !row.pinned[position])
            && match row.slots[position] {
                TabSlot::Tab(_) => true,
                TabSlot::Group(group) => members(group, true) == 0,
            }
    };
    let Some(victim) = (0..row.len)
        .find(|position| alive(*position) && droppable(*position, false))
        .or_else(|| (0..row.len).find(|position| alive(*position) && droppable(*position, true)))
    else {
        return Vec::new();
    };
    let mut victims = vec![victim];
    if let Some(group) = row.owners[victim]
        && members(group, true) == 1
        && let Some(chip) = chip_of(group)
        && alive(chip)
        && !row.collapsed[chip]
    {
        victims.push(chip);
    }
    victims
}

pub(in crate::windows_app) fn surface_accepts_omnibox_submit(surface: Surface) -> bool {
    matches!(surface, Surface::Home)
}

/// Mantem o HWND da omnibox vivo entre trocas de decoracao, mas remove a sua
/// autoridade de teclado fora da Home. Esta e a unica funcao que decide a
/// interatividade do EDIT nativo; producao e gate exercitam o mesmo caminho.
pub(in crate::windows_app) unsafe fn apply_omnibox_interactivity(edit: HWND, surface: Surface) {
    let interactive = surface_accepts_omnibox_submit(surface);
    EnableWindow(edit, if interactive { 1 } else { 0 });
    if !interactive && GetFocus() == edit {
        let parent = GetParent(edit);
        if !parent.is_null() {
            SetFocus(parent);
        }
    }
}

/// Os controlos do canto direito da segunda linha.
#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct RightControls {
    pub(in crate::windows_app) private: UiRect,
    /// Videochamada, WhatsApp, YouTube e Gmail, a esquerda do Privado.
    pub(in crate::windows_app) services: [UiRect; 4],
    /// Gemini Live, logo a esquerda dos servicos: o inicio do canto.
    pub(in crate::windows_app) live: UiRect,
    /// Pomodoro, Notas e Respiracao (ordem de `Tool::ALL`) na linha de CIMA,
    /// antes dos botoes da janela -- o mesmo sitio da Home
    /// (`home_tool_buttons`). Na segunda linha tiravam a largura toda a
    /// ultima coluna: a 1280 px a terceira IA ficava sem pilula, sem ‹ › e,
    /// com o Pomodoro a correr, sem "+". O do Pomodoro alarga com o tempo.
    pub(in crate::windows_app) tools: [UiRect; 3],
    /// Rotulo, expandir e fechar da gaveta; `None` quando nao ha gaveta.
    pub(in crate::windows_app) split: Option<(UiRect, UiRect, UiRect)>,
    /// ‹ e › da fonte da gaveta, a esquerda do rotulo.
    pub(in crate::windows_app) split_nav: Option<(UiRect, UiRect)>,
}

/// Onde acaba o botao Home da segunda linha (7 + 72 px) mais a folga de 8:
/// os controlos da direita nunca descem daqui.
pub(in crate::windows_app) const RIGHT_CONTROLS_MIN_LEFT: f64 = 87.0;
/// Rotulo da gaveta ("Fonte · ChatGPT") inteiro, e o minimo que ainda se le.
pub(in crate::windows_app) const SPLIT_LABEL_WIDTH: f64 = 150.0;
pub(in crate::windows_app) const SPLIT_LABEL_MIN_WIDTH: f64 = 60.0;

/// Quanto cede, numa janela estreita, o rotulo da gaveta (so informa):
/// encolhe ate desaparecer abaixo do minimo. Em pixeis logicos; `room` e o
/// que sobra depois dos botoes.
pub(in crate::windows_app) fn split_label_width(room: f64, split_active: bool) -> f64 {
    if !split_active {
        return 0.0;
    }
    let fits = room.min(SPLIT_LABEL_WIDTH);
    if fits >= SPLIT_LABEL_MIN_WIDTH {
        fits
    } else {
        0.0
    }
}

/// A etiqueta mais larga que o Pomodoro mostra ("⏸ mm:ss"). As abas da
/// linha de cima param antes dela sempre, com ou sem sessao: arrancar ou
/// pausar um Pomodoro nao mexe em nenhuma aba.
pub(in crate::windows_app) const POMODORO_LABEL_RESERVE: &str = "⏸ 00:00";

/// Onde comecam as ferramentas na linha de cima, com a etiqueta do Pomodoro
/// ja reservada: as abas acabam antes disto.
pub(in crate::windows_app) fn title_tools_left(
    client_width: f64,
    scale: f64,
    pomodoro_label: Option<BarLabel>,
) -> f64 {
    let reserved = home_tool_buttons(client_width, scale, BarLabel::new(POMODORO_LABEL_RESERVE));
    let actual = home_tool_buttons(client_width, scale, pomodoro_label);
    reserved[0].x.min(actual[0].x)
}

/// Geometria dos controlos encostados a direita. A mesma conta estava escrita
/// tres vezes -- no desenho, no hit-testing e agora nos chips -- e as copias
/// ja tinham comecado a divergir; aqui ela e uma so.
pub(in crate::windows_app) fn right_controls(
    client_width: f64,
    scale: f64,
    split_active: bool,
    pomodoro_label: Option<BarLabel>,
) -> RightControls {
    let margin = 8.0 * scale;
    let row_y = (TITLE_TAB_HEIGHT + 7.0) * scale;
    let row_h = 30.0 * scale;
    let gap = 5.0 * scale;
    // Botoes redondos so com icone, como no Chrome.
    let icon = row_h;
    let icon_gap = 4.0 * scale;

    // Tudo o que tem largura fixa, em pixeis logicos: a gaveta sem o rotulo
    // (fechar, expandir, ‹ e › e as folgas), o Privado, os quatro servicos e
    // o Gemini Live. O resto e do rotulo da gaveta.
    let logical = |value: f64| value / scale;
    let split_fixed = if split_active {
        30.0 + 5.0 + 30.0 + 5.0 + 6.0 + 26.0 + 4.0 + 26.0 + 6.0
    } else {
        0.0
    };
    let icons = logical(icon) * 6.0 + logical(icon_gap) * 5.0;
    let room = logical(client_width) - 8.0 - split_fixed - icons - RIGHT_CONTROLS_MIN_LEFT;
    let split_label_w = split_label_width(room, split_active);

    let split = split_active.then(|| {
        let close = UiRect {
            x: client_width - margin - 30.0 * scale,
            y: row_y,
            width: 30.0 * scale,
            height: row_h,
        };
        let expand = UiRect {
            x: close.x - gap - 30.0 * scale,
            y: row_y,
            width: 30.0 * scale,
            height: row_h,
        };
        let label = UiRect {
            x: expand.x - gap - split_label_w * scale,
            y: row_y,
            width: split_label_w * scale,
            height: row_h,
        };
        (label, expand, close)
    });

    let split_nav = split.map(|(label, _, _)| {
        let size = row_h - 4.0 * scale;
        let forward = UiRect {
            x: label.x - 6.0 * scale - size,
            y: row_y + 2.0 * scale,
            width: size,
            height: size,
        };
        let back = UiRect {
            x: forward.x - 4.0 * scale - size,
            ..forward
        };
        (back, forward)
    });
    let right = match split_nav {
        Some((back, _)) => back.x - 6.0 * scale,
        None => client_width - margin,
    };
    // Privado a direita e, a esquerda dele, videochamada, WhatsApp, YouTube,
    // Gmail e o Gemini Live. As ferramentas ficam na linha de cima.
    let private = UiRect {
        x: right - icon,
        y: row_y,
        width: icon,
        height: icon,
    };
    let services: [UiRect; 4] = std::array::from_fn(|index| UiRect {
        x: private.x - (4 - index) as f64 * (icon + icon_gap),
        y: row_y,
        width: icon,
        height: icon,
    });
    let live = UiRect {
        x: services[0].x - (icon + icon_gap),
        y: row_y,
        width: icon,
        height: icon,
    };
    let tools = home_tool_buttons(client_width, scale, pomodoro_label);

    RightControls {
        private,
        services,
        live,
        tools,
        split,
        split_nav,
    }
}

impl RightControls {
    /// Onde comecam os controlos da direita na segunda linha: as colunas
    /// acabam aqui. As ferramentas, na linha de cima, nao contam.
    pub(in crate::windows_app) fn leftmost(&self) -> f64 {
        self.live.x
    }
}

/// O que cada icone do canto direito faz, na ordem de `RightControls::services`.
pub(in crate::windows_app) const SERVICE_BUTTON_HITS: [BarHit; 4] = [
    BarHit::Service(Service::Meet),
    BarHit::Service(Service::WhatsApp),
    BarHit::Service(Service::YouTube),
    BarHit::GmailToggle,
];

/// O botao da barra que abre `service`: um dos icones dos servicos ou, para
/// a Respiracao (uma ferramenta), o botao dela na linha do titulo. E nele que
/// o painel minimizado poe o ponto (`draw_service_chrome`).
pub(in crate::windows_app) fn service_icon_rect(
    controls: RightControls,
    service: Service,
) -> Option<UiRect> {
    if service == Service::Breath {
        return Tool::ALL
            .iter()
            .zip(controls.tools)
            .find(|(tool, _)| **tool == Tool::Breath)
            .map(|(_, rect)| rect);
    }
    controls
        .services
        .iter()
        .zip(SERVICE_BUTTON_HITS)
        .find(|(_, hit)| *hit == BarHit::Service(service))
        .map(|(rect, _)| *rect)
}

pub(in crate::windows_app) fn right_controls_hit(
    controls: RightControls,
    x: f64,
    y: f64,
) -> Option<BarHit> {
    for (rect, tool) in controls.tools.iter().zip(Tool::ALL) {
        if rect.contains(x, y) {
            return Some(BarHit::Tool(tool));
        }
    }
    if controls.live.contains(x, y) {
        return Some(BarHit::GeminiLive);
    }
    for (rect, hit) in controls.services.iter().zip(SERVICE_BUTTON_HITS) {
        if rect.contains(x, y) {
            return Some(hit);
        }
    }
    if controls.private.contains(x, y) {
        return Some(BarHit::Private);
    }
    if let Some((_label, expand, close)) = controls.split {
        if close.contains(x, y) {
            return Some(BarHit::SplitClose);
        }
        if expand.contains(x, y) {
            return Some(BarHit::SplitExpand);
        }
    }
    None
}

/// O alvo da barra num ponto: os controlos da direita primeiro, depois o
/// resto. E o que `App::comparator_bar_hit` usa para o clique esquerdo, o
/// direito e a dica -- os tres veem o mesmo botao.
pub(in crate::windows_app) fn bar_hit_at(
    controls: Option<RightControls>,
    layout: Option<BarLayout>,
    x: f64,
    y: f64,
) -> Option<BarHit> {
    if let Some(controls) = controls
        && let Some(hit) = right_controls_hit(controls, x, y)
    {
        return Some(hit);
    }
    layout.and_then(|layout| layout.hit(x, y))
}
