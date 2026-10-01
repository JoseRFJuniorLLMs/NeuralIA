use super::*;

fn risk_test_page(target: AgentElement) -> ObservedPage {
    ObservedPage {
        generation: 1,
        url: "https://hostile.example/form".into(),
        title: "fixture".into(),
        text_excerpt: "fixture".into(),
        elements: vec![target],
    }
}

fn risk_test_element(role: &str, name: &str) -> AgentElement {
    AgentElement {
        id: "target".into(),
        generation: 1,
        role: role.into(),
        name: name.into(),
        text: name.into(),
        origin: "https://hostile.example".into(),
        frame: "top".into(),
        visible: true,
        interactable: true,
    }
}

#[test]
fn generic_click_never_inherits_a_reversible_grant_from_page_metadata() {
    let target = risk_test_element("button", "Next");
    let page = risk_test_page(target.clone());
    let action = AgentAction::Click { target };
    let security = app_agent_security_action(&action, &page);

    assert_eq!(security.risk(), ActionRisk::Sensitive);

    let mut policy = AgentPermissionPolicy::new(Some("https://hostile.example".into()));
    policy.grant_reversible_session_actions(true);
    let decision = policy.evaluate(&security);
    assert_eq!(decision.risk, ActionRisk::Sensitive);
    assert!(!decision.allowed);
    assert!(decision.requires_confirmation);
}

#[test]
fn structured_select_keeps_the_native_reversible_class() {
    let target = risk_test_element("combobox", "Sort");
    let page = risk_test_page(target.clone());
    let action = AgentAction::Select {
        target,
        value: "recent".into(),
    };
    let security = app_agent_security_action(&action, &page);

    assert_eq!(security.risk(), ActionRisk::Reversible);

    let mut policy = AgentPermissionPolicy::new(Some("https://hostile.example".into()));
    policy.grant_reversible_session_actions(true);
    let decision = policy.evaluate(&security);
    assert!(decision.allowed);
    assert!(!decision.requires_confirmation);
}
