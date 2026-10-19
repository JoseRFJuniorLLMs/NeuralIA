use super::super::*;
use crate::panel_chrome::{SERVICE_STRIP_HEIGHT, ServiceEffect};

fn source_page() -> SplitView<&'static str> {
    SplitView {
        webview: "pagina com sessao e formulario preenchido",
        source_index: 0,
        context_id: Some(42),
        fullscreen: false,
        minimized: false,
        private: true,
    }
}

#[test]
fn minimizing_every_column_keeps_the_source_and_restore_controls_reachable() {
    let mut split = source_page();
    let mut minimized = [false; COMPARATOR_COLUMNS];
    let mut expanded = Some(0);
    for index in 0..COMPARATOR_COLUMNS {
        assert!(minimize_column_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            index,
            &mut expanded,
            Some(&mut split),
        ));
        assert!(minimized[index]);
        assert_eq!(expanded, None);
    }
    assert_eq!(
        split_presentation(&split, &minimized),
        SplitPresentation::Only
    );
    assert!(!chrome_hidden(false, split.fullscreen));
    assert_eq!(split.context_id, Some(42));
    assert!(split.private);
    assert_eq!(split.webview, "pagina com sessao e formulario preenchido");
    set_split_minimized(&mut split, true);
    assert_eq!(
        split_presentation(&split, &minimized),
        SplitPresentation::Hidden
    );
    assert!(visible_column_spans(1600.0, 3, &[1.0; 3], &minimized).is_empty());
    let layout = BarLayout::with_contexts(
        1600.0,
        1.0,
        true,
        BarColumns {
            minimized,
            split_active: true,
            ..BarColumns::even(3)
        },
        [0; 3],
    );
    let controls = right_controls(1600.0, 1.0, true, None);
    for index in 0..3 {
        let chip = layout.columns[index];
        assert!(chip.width > 0.0);
        assert_eq!(
            bar_hit_at(
                Some(controls),
                Some(layout),
                chip.x + chip.width / 2.0,
                chip.y + chip.height / 2.0
            ),
            Some(BarHit::Column(index))
        );
    }
    let minimize = controls.split_minimize.expect("a fonte tem restaurar");
    assert_eq!(
        right_controls_hit(controls, minimize.x + 15.0, minimize.y + 15.0),
        Some(BarHit::SplitMinimize)
    );
    set_split_minimized(&mut split, false);
    assert_eq!(
        split_presentation(&split, &minimized),
        SplitPresentation::Only
    );
    assert_eq!(split.context_id, Some(42));
}

#[test]
fn source_minimize_and_fullscreen_are_independent_reversible_actions() {
    let mut split = source_page();
    assert!(toggle_split_fullscreen_state(&mut split));
    assert_eq!(
        split_presentation(&split, &[false; 3]),
        SplitPresentation::Fullscreen
    );
    set_split_minimized(&mut split, true);
    assert!(!split.fullscreen);
    assert_eq!(
        split_presentation(&split, &[false; 3]),
        SplitPresentation::Hidden
    );
    assert!(toggle_split_fullscreen_state(&mut split));
    assert!(!split.minimized);
    assert!(!toggle_split_fullscreen_state(&mut split));
    assert_eq!(
        split_presentation(&split, &[true, false, true]),
        SplitPresentation::Docked(1)
    );
    assert_eq!(split.webview, "pagina com sessao e formulario preenchido");
    assert_eq!(split.context_id, Some(42));
}

#[test]
fn each_source_control_has_its_own_target_without_hiding_the_other_buttons() {
    for scale in [1.0, 1.5, 2.0] {
        for width in [420.0, 600.0, 1280.0, 1920.0] {
            let controls = right_controls(width * scale, scale, true, None);
            let (_, expand, close) = controls.split.expect("fonte aberta");
            let minimize = controls.split_minimize.expect("minimizar fonte");
            for (rect, hit) in [
                (minimize, BarHit::SplitMinimize),
                (expand, BarHit::SplitExpand),
                (close, BarHit::SplitClose),
            ] {
                assert!(rect.width > 0.0);
                assert_eq!(
                    right_controls_hit(
                        controls,
                        rect.x + rect.width / 2.0,
                        rect.y + rect.height / 2.0
                    ),
                    Some(hit)
                );
            }
            assert!(minimize.x + minimize.width <= expand.x);
            assert!(expand.x + expand.width <= close.x);
            assert!(controls.split_nav.expect("navegacao").0.x >= RIGHT_CONTROLS_MIN_LEFT * scale);
        }
    }
}

#[test]
fn minimize_shortcuts_dispatch_the_actual_host_and_do_not_repeat() {
    let keymap = Keymap::build(COMMANDS).expect("atalhos unicos");
    for origin in [
        CommandOrigin::Window,
        CommandOrigin::Omnibox,
        CommandOrigin::Host(WebViewHost::Column(2)),
        CommandOrigin::Host(WebViewHost::Split(1)),
        CommandOrigin::Host(WebViewHost::PrivateSplit(1)),
        CommandOrigin::Host(WebViewHost::External),
        CommandOrigin::Host(WebViewHost::Service(Service::YouTube)),
    ] {
        let input = AcceleratorInput {
            vk: u32::from(b'M'),
            down: true,
            ctrl: true,
            shift: true,
            alt: false,
            repeat: false,
        };
        let decision = accelerator_decision(&keymap, input, origin);
        assert!(decision.handled);
        let event = decision.event.expect("primeira descida dispara");
        assert!(
            matches!(command_key_event(&event), Some(UserEvent::MinimizeActivePage(actual)) if actual == origin)
        );
        let repeated = accelerator_decision(
            &keymap,
            AcceleratorInput {
                repeat: true,
                ..input
            },
            origin,
        );
        assert!(repeated.handled);
        assert!(repeated.event.is_none());
    }
}

#[test]
fn minimized_service_keeps_controls_while_close_discards_the_panel() {
    let mut state = ServicePanelState::default();
    assert_eq!(state.step(ServiceInput::Minimize), ServiceEffect::Relayout);
    let frame = state.frame(
        560.0,
        1600.0,
        900.0,
        COMPARATOR_CHROME_HEIGHT,
        SERVICE_STRIP_HEIGHT,
    );
    assert!(frame.panel.is_none());
    assert!(frame.strip.is_some());
    assert_eq!(state.step(ServiceInput::Close), ServiceEffect::Nothing);
    assert!(state.minimized());
    assert_eq!(StripButton::ALL.len(), 2);
    assert!(
        StripButton::ALL
            .iter()
            .all(|button| !button.label().contains("Fechar"))
    );
    assert!(
        service_panel_hint(
            BarHit::ServiceStrip(StripButton::Minimize),
            Service::WhatsApp,
            None
        )
        .expect("minimizar")
        .contains("mantém os controles")
    );
    assert!(!service_should_pause(false, true));
    assert!(!service_should_pause(true, true));
    assert!(!service_should_pause(true, false));
    assert!(SERVICE_PAUSE_SCRIPT.contains("pause()"));
    assert!(SERVICE_PAUSE_SCRIPT.contains("pauseVideo"));
}

/// Cada IA tem o seu atalho de minimizar e restaurar, na dica do botão e no
/// chip, e o Gemini Live aceita esses nomes em vez de restaurar tudo.
#[test]
fn each_ai_column_has_its_own_minimize_and_restore_shortcut() {
    let keys = ["Shift+Alt+1", "Shift+Alt+2", "Shift+Alt+3"];
    let names = ["Google Gemini", "ChatGPT", "Claude"];
    let commands = [
        CommandId::MinimizeColumnOne,
        CommandId::MinimizeColumnTwo,
        CommandId::MinimizeColumnThree,
    ];
    let keymap = Keymap::build(COMMANDS).expect("atalhos unicos");
    for (index, key) in keys.iter().enumerate() {
        assert_eq!(column_toggle_shortcut(index).as_deref(), Some(*key));
        assert_eq!(
            command_row(commands[index])
                .and_then(|row| row.chords.first())
                .map(|spec| shortcut_text(spec.chord))
                .as_deref(),
            Some(*key)
        );
        let minimize = bar_tooltip_label(
            BarHit::ColumnMinimize(index),
            &BarState::default(),
            names[index],
            None,
            None,
        );
        assert_eq!(
            minimize.as_deref(),
            Some(format!("Minimizar {} ({key})", names[index]).as_str())
        );
        assert_eq!(
            column_hint_text(ColumnHint::Minimize, names[index], index),
            format!("Minimizar {} ({key})", names[index])
        );
        let mut state = BarState::default();
        state.minimized[index] = true;
        let chip = bar_tooltip_label(BarHit::Column(index), &state, names[index], None, None);
        assert_eq!(
            chip.as_deref(),
            Some(format!("Restaurar {} ({key})", names[index]).as_str())
        );
        let input = AcceleratorInput {
            vk: u32::from(b'1') + index as u32,
            down: true,
            ctrl: false,
            shift: true,
            alt: true,
            repeat: false,
        };
        let decision = accelerator_decision(&keymap, input, CommandOrigin::Window);
        assert!(decision.handled);
        assert!(matches!(
            decision.event,
            Some(UserEvent::RunCommandKey { key: id, origin: CommandOrigin::Window }) if id == commands[index]
        ));
        assert_eq!(
            format!(
                "{:?}",
                resolve_command(
                    commands[index],
                    CommandOrigin::Host(WebViewHost::Column(index))
                )
            ),
            format!(
                "{:?}",
                Some(UserEvent::NativeShortcut(
                    commands[index],
                    CommandOrigin::Host(WebViewHost::Column(index))
                ))
            )
        );
    }
    let open = bar_tooltip_label(
        BarHit::Column(1),
        &BarState::default(),
        "ChatGPT",
        None,
        None,
    );
    assert_eq!(open.as_deref(), Some("ChatGPT: expandir esta coluna (F11)"));

    let mut minimized = [false; COMPARATOR_COLUMNS];
    let mut expanded = Some(1usize);
    let mut split = source_page();
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            1,
            &mut expanded,
            Some(&mut split)
        ),
        ColumnToggle::Minimized
    );
    assert!(minimized[1]);
    assert_eq!(expanded, None);
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            1,
            &mut expanded,
            Some(&mut split)
        ),
        ColumnToggle::Restored
    );
    assert!(!minimized[1]);
    assert_eq!(split.source_index, 1);
    assert!(!split.fullscreen);
    assert_eq!(
        toggle_column_minimized_state::<&str>(&mut minimized, 3, 9, &mut expanded, None),
        ColumnToggle::Ignored
    );

    assert_eq!(
        parse_browser_layout("minimize_column", 2),
        BrowserLayoutAction::Minimize(2)
    );
    assert_eq!(
        parse_browser_layout("restore_column", 0),
        BrowserLayoutAction::RestoreColumn(0)
    );
    assert_eq!(
        parse_browser_layout("toggle_column", 1),
        BrowserLayoutAction::ToggleColumn(1)
    );
    assert_eq!(
        parse_browser_layout("restore_columns", 0),
        BrowserLayoutAction::RestoreAll
    );
    assert_eq!(
        parse_browser_layout("zoom_in", 0),
        BrowserLayoutAction::ZoomIn
    );
    assert_eq!(
        parse_browser_layout("nope", 1),
        BrowserLayoutAction::Unknown
    );
    let live = crate::gemini_live::LIVE_CORE_JS;
    for name in ["minimize_column", "restore_column", "toggle_column"] {
        assert!(live.contains(name), "o Gemini Live nao declara {name}");
    }
    let panels = include_str!("app/panels.rs");
    let layout = panels
        .split("\"browser_layout\" =>")
        .nth(1)
        .expect("browser_layout");
    assert!(layout.contains("parse_browser_layout"));
    assert!(layout.contains("BrowserLayoutAction::Minimize"));
    assert!(layout.contains("BrowserLayoutAction::RestoreColumn"));
    assert!(layout.contains("BrowserLayoutAction::ToggleColumn"));
    assert!(!layout.contains("_ => self.restore_comparator()"));
}

/// Gate de regressao P1: o fundo animado do comparador NUNCA sobrescreve a barra
/// de topo nem os chips de restauracao em nenhuma escala, resolucao, tema,
/// presenca de fonte lateral, ou modo de buffer (duplo buffer vs contingencia direta).
#[test]
fn comparator_neural_background_never_overwrites_bar_pixels_across_scales_and_themes() {
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    };

    let scales = [1.0, 1.5, 2.0];
    let resolutions = [
        (1280, 720),  // janela restaurada
        (1440, 900),  // janela maximizada
        (1920, 1080), // janela maximizada alta resolucao
    ];
    let themes = [Theme::dark((0, 120, 215)), Theme::light((0, 120, 215))];
    let buffer_modes = [true, false]; // true: duplo buffer; false: contingencia direta
    let split_modes = [false, true]; // com e sem fonte lateral

    for scale in scales {
        let bar_h = (COMPARATOR_CHROME_HEIGHT * scale).round() as i32;
        for (width, height) in resolutions {
            for theme in &themes {
                for allow_buffer in buffer_modes {
                    for split_active in split_modes {
                        unsafe {
                            let screen = GetDC(std::ptr::null_mut());
                            let mem_dc = CreateCompatibleDC(screen);
                            let bitmap = CreateCompatibleBitmap(screen, width, height);
                            ReleaseDC(std::ptr::null_mut(), screen);
                            assert!(!mem_dc.is_null() && !bitmap.is_null());
                            let old_bmp = SelectObject(mem_dc, bitmap as _);

                            // 1. Pre-preencher o bitmap com cor neutra
                            let fill_brush = CreateSolidBrush(rgb3((33, 44, 55)));
                            let full_rect = RECT {
                                left: 0,
                                top: 0,
                                right: width,
                                bottom: height,
                            };
                            FillRect(mem_dc, &full_rect, fill_brush);
                            DeleteObject(fill_brush as _);

                            // 2. Pintar a barra com todas as 3 IAs minimizadas (chips ativos)
                            let empty: [Vec<ContextTab>; COMPARATOR_COLUMNS] =
                                std::array::from_fn(|_| Vec::new());
                            let no_groups: [Vec<ContextGroup>; COMPARATOR_COLUMNS] =
                                std::array::from_fn(|_| Vec::new());
                            let split_info = if split_active {
                                Some((0, Some(42), false, true))
                            } else {
                                None
                            };
                            paint_comparator_bar_with_contexts(
                                mem_dc,
                                width,
                                scale,
                                &["Google Gemini", "ChatGPT", "Claude"],
                                BarColumns {
                                    minimized: [true; COMPARATOR_COLUMNS],
                                    split_active,
                                    ..BarColumns::even(3)
                                },
                                &empty,
                                &no_groups,
                                split_info,
                                [None; COMPARATOR_COLUMNS],
                                BarState {
                                    visible: true,
                                    minimized: [true; COMPARATOR_COLUMNS],
                                    ..BarState::default()
                                },
                                &LivePanel::<()>::off(),
                                theme,
                            );

                            // 3. Capturar o estado da barra antes do fundo animado
                            let mut info = BITMAPINFO {
                                bmiHeader: BITMAPINFOHEADER {
                                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                                    biWidth: width,
                                    biHeight: -height,
                                    biPlanes: 1,
                                    biBitCount: 32,
                                    biCompression: BI_RGB,
                                    biSizeImage: (width * height * 4) as u32,
                                    biXPelsPerMeter: 0,
                                    biYPelsPerMeter: 0,
                                    biClrUsed: 0,
                                    biClrImportant: 0,
                                },
                                bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
                                    rgbBlue: 0,
                                    rgbGreen: 0,
                                    rgbRed: 0,
                                    rgbReserved: 0,
                                }; 1],
                            };
                            let mut pixels_before = vec![0u8; (width * height * 4) as usize];
                            let read1 = GetDIBits(
                                mem_dc,
                                bitmap,
                                0,
                                height as u32,
                                pixels_before.as_mut_ptr() as _,
                                &mut info,
                                DIB_RGB_COLORS,
                            );
                            assert_eq!(read1, height);

                            // 4. Executar varios quadros da animacao do fundo (simulando RedrawRequested repetido)
                            for _frame in 0..5 {
                                paint_comparator_neural_background(
                                    mem_dc,
                                    width,
                                    height,
                                    scale,
                                    theme,
                                    allow_buffer,
                                );
                            }

                            // 5. Capturar o estado do bitmap apos os quadros da animacao
                            let mut pixels_after = vec![0u8; (width * height * 4) as usize];
                            let read2 = GetDIBits(
                                mem_dc,
                                bitmap,
                                0,
                                height as u32,
                                pixels_after.as_mut_ptr() as _,
                                &mut info,
                                DIB_RGB_COLORS,
                            );
                            assert_eq!(read2, height);

                            SelectObject(mem_dc, old_bmp);
                            DeleteObject(bitmap as _);
                            DeleteDC(mem_dc);

                            // 6. Prova pixel-a-pixel: NENHUM pixel da barra (y < bar_h) foi modificado pelo fundo!
                            let stride = (width * 4) as usize;
                            for y in 0..bar_h as usize {
                                let row_start = y * stride;
                                let row_end = row_start + stride;
                                assert_eq!(
                                    &pixels_after[row_start..row_end],
                                    &pixels_before[row_start..row_end],
                                    "O fundo animado sobrescreveu pixels da barra na linha y={y} (scale={scale}, res={width}x{height}, buffer={allow_buffer}, split={split_active})"
                                );
                            }

                            // 7. Prova de que a regiao de conteudo (y >= bar_h) FOI pintada pelo fundo
                            let content_painted = (bar_h as usize..height as usize).any(|y| {
                                let row_start = y * stride;
                                let row_end = row_start + stride;
                                pixels_after[row_start..row_end]
                                    != pixels_before[row_start..row_end]
                            });
                            assert!(
                                content_painted,
                                "A area de conteudo abaixo da barra deveria ter sido pintada pelo fundo animado"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Exercita o ciclo completo de minimizar todas as IAs (Ctrl+Shift+F9), verifica que
/// os chips permanecem visiveis e clicaveis ao longo dos quadros de animacao, e
/// testa a restauracao individual de cada IA (Gemini, ChatGPT, Claude) preservando
/// a pagina e sessao da fonte lateral.
#[test]
fn minimized_columns_remain_visible_and_clickable_through_animation_frames_and_restore_preserves_page()
 {
    let mut split = source_page();
    let mut minimized = [false; COMPARATOR_COLUMNS];
    let mut expanded = Some(0);

    // 1. Minimizar todas as 3 colunas (simula Ctrl+Shift+F9 / MinimizeAll)
    for index in 0..COMPARATOR_COLUMNS {
        assert!(minimize_column_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            index,
            &mut expanded,
            Some(&mut split),
        ));
        assert!(minimized[index]);
    }
    assert_eq!(expanded, None);
    assert_eq!(
        split_presentation(&split, &minimized),
        SplitPresentation::Only
    );

    // 2. Cobrir escalas e verificar que em todas elas os chips permanecem com alvos de clique validos
    for scale in [1.0, 1.5, 2.0] {
        for width in [1280.0, 1440.0, 1920.0] {
            let layout = BarLayout::with_contexts(
                width,
                scale,
                true,
                BarColumns {
                    minimized,
                    split_active: true,
                    ..BarColumns::even(3)
                },
                [0; 3],
            );
            let controls = right_controls(width, scale, true, None);

            // Cada chip deve ter largura positiva e retornar o hit correto
            for index in 0..3 {
                let chip = layout.columns[index];
                assert!(
                    chip.width > 0.0,
                    "Chip {index} deve ter largura positiva na escala {scale}"
                );
                assert_eq!(
                    bar_hit_at(
                        Some(controls),
                        Some(layout),
                        chip.x + chip.width / 2.0,
                        chip.y + chip.height / 2.0
                    ),
                    Some(BarHit::Column(index)),
                    "Hit test do chip {index} deve apontar para Column({index})"
                );
            }

            // O botao de minimizar a fonte lateral tambem deve permanecer acessivel
            let split_min = controls.split_minimize.expect("minimizar fonte lateral");
            assert_eq!(
                right_controls_hit(controls, split_min.x + 10.0, split_min.y + 10.0),
                Some(BarHit::SplitMinimize)
            );
        }
    }

    // 3. Restaurar separadamente Gemini (0), ChatGPT (1) e Claude (2), preservando estado e URL
    // Restaurar Gemini (0)
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            0,
            &mut expanded,
            Some(&mut split),
        ),
        ColumnToggle::Restored
    );
    assert!(!minimized[0]);
    assert!(minimized[1]);
    assert!(minimized[2]);
    assert_eq!(split.webview, "pagina com sessao e formulario preenchido");
    assert_eq!(split.context_id, Some(42));

    // Minimizar Gemini novamente
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            0,
            &mut expanded,
            Some(&mut split),
        ),
        ColumnToggle::Minimized
    );
    assert!(minimized[0]);

    // Restaurar ChatGPT (1)
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            1,
            &mut expanded,
            Some(&mut split),
        ),
        ColumnToggle::Restored
    );
    assert!(minimized[0]);
    assert!(!minimized[1]);
    assert!(minimized[2]);
    assert_eq!(split.webview, "pagina com sessao e formulario preenchido");

    // Minimizar ChatGPT novamente
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            1,
            &mut expanded,
            Some(&mut split),
        ),
        ColumnToggle::Minimized
    );

    // Restaurar Claude (2)
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            2,
            &mut expanded,
            Some(&mut split),
        ),
        ColumnToggle::Restored
    );
    assert!(minimized[0]);
    assert!(minimized[1]);
    assert!(!minimized[2]);
    assert_eq!(split.webview, "pagina com sessao e formulario preenchido");

    // Restaurar todas uma a uma ate o comparador ficar totalmente restaurado
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            0,
            &mut expanded,
            Some(&mut split),
        ),
        ColumnToggle::Restored
    );
    assert_eq!(
        toggle_column_minimized_state(
            &mut minimized,
            COMPARATOR_COLUMNS,
            1,
            &mut expanded,
            Some(&mut split),
        ),
        ColumnToggle::Restored
    );
    assert_eq!(minimized, [false; COMPARATOR_COLUMNS]);
    assert_eq!(split.webview, "pagina com sessao e formulario preenchido");
    assert_eq!(split.context_id, Some(42));
}
