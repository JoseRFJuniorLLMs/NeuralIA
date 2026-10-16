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
