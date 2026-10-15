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
fn minimized_service_keeps_controls_while_hide_retires_the_strip() {
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
    assert_eq!(state.step(ServiceInput::Close), ServiceEffect::Close);
    assert!(
        service_panel_hint(
            BarHit::ServiceStrip(StripButton::Close),
            Service::WhatsApp,
            None
        )
        .expect("ocultar")
        .contains("recolhe também os controles")
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
}
