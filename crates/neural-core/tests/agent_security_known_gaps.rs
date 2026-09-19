//! Executable reproductions for known SPEC-0104/0105 gaps.
//!
//! These tests are deliberately ignored while the audited runtime files are
//! frozen. They MUST NOT be counted as acceptance coverage. When each finding
//! is fixed, remove the corresponding `#[ignore]` and make it pass in CI.

use std::{
    collections::VecDeque,
    fs,
    sync::{Arc, Mutex},
};

use neural_core::{
    ActionRisk, AgentAction, AgentElement, AgentOutcome, AgentPermissionPolicy, AgentPlanner,
    AgentRuntime, AgentRuntimeConfig, AgentSecurityAction, AgentToolExecutor, FieldKind,
    ObservedPage, ToolResult, save_agent_outcome,
};
use neural_core::agent_runtime::AgentStep;

fn page(text: &str) -> ObservedPage {
    ObservedPage {
        generation: 1,
        url: "https://example.com/form".into(),
        title: "Form".into(),
        text_excerpt: text.into(),
        elements: vec![element("real", "textbox", "query", "https://example.com")],
    }
}

fn element(id: &str, role: &str, name: &str, origin: &str) -> AgentElement {
    AgentElement {
        id: id.into(),
        generation: 1,
        role: role.into(),
        name: name.into(),
        text: String::new(),
        origin: origin.into(),
        frame: "top".into(),
        visible: true,
        interactable: true,
    }
}

struct QueuePlanner {
    actions: VecDeque<AgentAction>,
}

impl AgentPlanner for QueuePlanner {
    fn plan(
        &mut self,
        _goal: &str,
        _page: &ObservedPage,
        _trace: &[AgentStep],
    ) -> Result<AgentAction, String> {
        self.actions
            .pop_front()
            .ok_or_else(|| "planner exhausted".to_string())
    }
}

struct NoopExecutor;

impl AgentToolExecutor for NoopExecutor {
    fn execute(&mut self, _action: &AgentAction) -> Result<ToolResult, String> {
        Ok(ToolResult {
            page: page("ok"),
            output: Some("ok".into()),
        })
    }
}

#[test]
#[ignore = "known gap CRITICAL-01: save_agent_outcome serializes sensitive action text"]
fn restricted_type_text_never_persists_plaintext_secret() {
    let password_target = element("password", "password", "Password", "https://example.com");
    let planner = QueuePlanner {
        actions: VecDeque::from([AgentAction::TypeText {
            target: password_target,
            text: "hunter2".into(),
            field: FieldKind::Password,
        }]),
    };
    let policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
    let mut runtime =
        AgentRuntime::new(planner, NoopExecutor, policy, AgentRuntimeConfig::default());

    let outcome = runtime.run("login", page("Login form"));
    assert!(matches!(outcome, AgentOutcome::NeedsApproval { .. }));

    let root = std::env::temp_dir().join(format!(
        "neuralia-known-gap-secret-{}",
        std::process::id()
    ));
    let path = root.join("outcome.json");
    save_agent_outcome(&path, "login", &outcome).unwrap();
    let stored = fs::read_to_string(&path).unwrap();

    assert!(
        !stored.contains("hunter2"),
        "restricted field plaintext must never enter persisted agent outcome"
    );

    let _ = fs::remove_dir_all(root);
}

struct CapturePlanner {
    seen: Arc<Mutex<String>>,
}

impl AgentPlanner for CapturePlanner {
    fn plan(
        &mut self,
        _goal: &str,
        page: &ObservedPage,
        _trace: &[AgentStep],
    ) -> Result<AgentAction, String> {
        *self.seen.lock().unwrap() = page.text_excerpt.clone();
        Ok(AgentAction::Finish {
            summary: "done".into(),
        })
    }
}

#[test]
#[ignore = "known gap HIGH-01: planner receives ObservedPage without mandatory sanitization"]
fn planner_boundary_receives_sanitized_observation() {
    let seen = Arc::new(Mutex::new(String::new()));
    let planner = CapturePlanner {
        seen: Arc::clone(&seen),
    };
    let policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
    let mut runtime =
        AgentRuntime::new(planner, NoopExecutor, policy, AgentRuntimeConfig::default());

    let _ = runtime.run(
        "inspect",
        page("Authorization: Bearer abc\npassword=hunter2\nbody: visible"),
    );

    let observed = seen.lock().unwrap().clone();
    assert!(!observed.contains("Bearer abc"));
    assert!(!observed.contains("hunter2"));
    assert!(observed.contains("body: visible"));
}

struct FlagExecutor {
    executed: Arc<Mutex<bool>>,
}

impl AgentToolExecutor for FlagExecutor {
    fn execute(&mut self, _action: &AgentAction) -> Result<ToolResult, String> {
        *self.executed.lock().unwrap() = true;
        Ok(ToolResult {
            page: page("ok"),
            output: Some("executed".into()),
        })
    }
}

#[test]
#[ignore = "known gap HIGH-02: element ref authenticity checks generation only"]
fn fabricated_element_reference_is_rejected_before_executor() {
    let forged = element("not-issued-by-observer", "button", "Continue", "https://example.com");
    let planner = QueuePlanner {
        actions: VecDeque::from([
            AgentAction::Click { target: forged },
            AgentAction::Finish {
                summary: "done".into(),
            },
        ]),
    };
    let executed = Arc::new(Mutex::new(false));
    let executor = FlagExecutor {
        executed: Arc::clone(&executed),
    };
    let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
    policy.grant_reversible_session_actions(true);
    let mut runtime =
        AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());

    let _ = runtime.run("click", page("benign"));

    assert!(
        !*executed.lock().unwrap(),
        "runtime must resolve planner IDs against the current observation"
    );
}

#[test]
#[ignore = "known gap HIGH-03: reversible grant is not scoped to origin"]
fn reversible_session_grant_does_not_cross_origin_boundary() {
    let mut policy = AgentPermissionPolicy::new(Some("https://a.example".into()));
    policy.grant_reversible_session_actions(true);

    let decision = policy.evaluate(&AgentSecurityAction::Click {
        origin: "https://b.example".into(),
        label: "Continue".into(),
    });

    assert_eq!(decision.risk, ActionRisk::Reversible);
    assert!(!decision.allowed);
    assert!(decision.requires_confirmation);
}
