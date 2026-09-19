use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::agent_security::{
    AgentPermissionPolicy, AgentSecurityAction, FieldKind, PolicyDecision, redact_sensitive_text,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentElement {
    pub id: String,
    pub generation: u64,
    pub role: String,
    pub name: String,
    pub text: String,
    pub origin: String,
    pub frame: String,
    pub visible: bool,
    pub interactable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedPage {
    pub generation: u64,
    pub url: String,
    pub title: String,
    pub text_excerpt: String,
    #[serde(default)]
    pub elements: Vec<AgentElement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentAction {
    Navigate {
        url: String,
    },
    Click {
        target: AgentElement,
    },
    TypeText {
        target: AgentElement,
        text: String,
        field: FieldKind,
    },
    Select {
        target: AgentElement,
        value: String,
    },
    Submit {
        target: AgentElement,
        description: String,
    },
    Scroll {
        amount: i32,
    },
    Extract {
        target: Option<AgentElement>,
        schema: String,
    },
    Wait {
        millis: u64,
    },
    AskUser {
        reason: String,
    },
    Finish {
        summary: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub page: ObservedPage,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStep {
    pub number: usize,
    pub action: AgentAction,
    pub result_summary: String,
}

pub trait AgentPlanner {
    fn plan(
        &mut self,
        goal: &str,
        page: &ObservedPage,
        trace: &[AgentStep],
    ) -> Result<AgentAction, String>;
}

pub trait AgentToolExecutor {
    fn execute(&mut self, action: &AgentAction) -> Result<ToolResult, String>;
}

#[derive(Debug, Clone)]
pub struct AgentRuntimeConfig {
    pub max_steps: usize,
    pub max_wall_time: Duration,
    pub max_wait: Duration,
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            max_steps: 24,
            max_wall_time: Duration::from_secs(120),
            max_wait: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentOutcome {
    Completed {
        summary: String,
        trace: Vec<AgentStep>,
    },
    NeedsApproval {
        action: AgentAction,
        decision: PolicyDecision,
        trace: Vec<AgentStep>,
    },
    NeedsUser {
        reason: String,
        trace: Vec<AgentStep>,
    },
    Stopped {
        trace: Vec<AgentStep>,
    },
    StepLimit {
        trace: Vec<AgentStep>,
    },
    Failed {
        error: String,
        trace: Vec<AgentStep>,
    },
}

pub fn save_agent_outcome(
    path: impl AsRef<Path>,
    goal: &str,
    outcome: &AgentOutcome,
) -> io::Result<()> {
    #[derive(Serialize)]
    struct StoredOutcome {
        goal: String,
        outcome: AgentOutcome,
    }

    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stored = StoredOutcome {
        goal: redact_sensitive_text(goal),
        outcome: redacted_outcome(outcome),
    };
    let bytes = serde_json::to_vec_pretty(&stored).map_err(io::Error::other)?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(temp, path)
        }
        Err(error) => Err(error),
    }
}

fn redacted_outcome(outcome: &AgentOutcome) -> AgentOutcome {
    match outcome {
        AgentOutcome::Completed { summary, trace } => AgentOutcome::Completed {
            summary: redact_sensitive_text(summary),
            trace: redacted_trace(trace),
        },
        AgentOutcome::NeedsApproval {
            action,
            decision,
            trace,
        } => AgentOutcome::NeedsApproval {
            action: redacted_action(action),
            decision: decision.clone(),
            trace: redacted_trace(trace),
        },
        AgentOutcome::NeedsUser { reason, trace } => AgentOutcome::NeedsUser {
            reason: redact_sensitive_text(reason),
            trace: redacted_trace(trace),
        },
        AgentOutcome::Stopped { trace } => AgentOutcome::Stopped {
            trace: redacted_trace(trace),
        },
        AgentOutcome::StepLimit { trace } => AgentOutcome::StepLimit {
            trace: redacted_trace(trace),
        },
        AgentOutcome::Failed { error, trace } => AgentOutcome::Failed {
            error: redact_sensitive_text(error),
            trace: redacted_trace(trace),
        },
    }
}

fn redacted_trace(trace: &[AgentStep]) -> Vec<AgentStep> {
    trace
        .iter()
        .map(|step| AgentStep {
            number: step.number,
            action: redacted_action(&step.action),
            result_summary: redact_sensitive_text(&step.result_summary),
        })
        .collect()
}

fn redacted_action(action: &AgentAction) -> AgentAction {
    let mut redacted = action.clone();
    match &mut redacted {
        AgentAction::TypeText { text, field, .. }
            if matches!(
                *field,
                FieldKind::Email
                    | FieldKind::Password
                    | FieldKind::PaymentCard
                    | FieldKind::Otp
                    | FieldKind::Unknown
            ) =>
        {
            let chars = text.chars().count();
            *text = format!("[REDACTED {chars} chars]");
        }
        AgentAction::Select { value, .. } => {
            *value = redact_sensitive_text(value);
        }
        AgentAction::Submit { description, .. } => {
            *description = redact_sensitive_text(description);
        }
        AgentAction::AskUser { reason } => {
            *reason = redact_sensitive_text(reason);
        }
        AgentAction::Finish { summary } => {
            *summary = redact_sensitive_text(summary);
        }
        _ => {}
    }
    redacted
}

fn sanitize_observed_page(page: &ObservedPage) -> ObservedPage {
    let mut sanitized = page.clone();
    sanitized.title = redact_sensitive_text(&sanitized.title);
    sanitized.text_excerpt = redact_sensitive_text(&sanitized.text_excerpt);
    for element in &mut sanitized.elements {
        element.text = if element_looks_sensitive(element) {
            "[REDACTED]".into()
        } else {
            redact_sensitive_text(&element.text)
        };
    }
    sanitized
}

fn element_looks_sensitive(element: &AgentElement) -> bool {
    let material = format!("{} {}", element.role, element.name).to_ascii_lowercase();
    [
        "password", "senha", "otp", "one-time", "card", "cartão", "cartao", "cvv", "cvc",
    ]
    .iter()
    .any(|needle| material.contains(needle))
}

pub struct AgentRuntime<P, E> {
    planner: P,
    executor: E,
    policy: AgentPermissionPolicy,
    config: AgentRuntimeConfig,
    trace: Vec<AgentStep>,
}

impl<P, E> AgentRuntime<P, E>
where
    P: AgentPlanner,
    E: AgentToolExecutor,
{
    pub fn new(
        planner: P,
        executor: E,
        policy: AgentPermissionPolicy,
        config: AgentRuntimeConfig,
    ) -> Self {
        Self {
            planner,
            executor,
            policy,
            config,
            trace: Vec::new(),
        }
    }

    pub fn policy(&self) -> &AgentPermissionPolicy {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut AgentPermissionPolicy {
        &mut self.policy
    }

    pub fn run(&mut self, goal: &str, mut page: ObservedPage) -> AgentOutcome {
        let started = Instant::now();

        for number in 1..=self.config.max_steps {
            if self.policy.stopped() {
                return AgentOutcome::Stopped {
                    trace: self.trace.clone(),
                };
            }
            if started.elapsed() >= self.config.max_wall_time {
                return AgentOutcome::Failed {
                    error: "agent wall-time budget exhausted".into(),
                    trace: self.trace.clone(),
                };
            }

            let planner_page = sanitize_observed_page(&page);
            let action = match self.planner.plan(goal, &planner_page, &self.trace) {
                Ok(action) => action,
                Err(error) => {
                    return AgentOutcome::Failed {
                        error,
                        trace: self.trace.clone(),
                    };
                }
            };

            if let AgentAction::Finish { summary } = action {
                return AgentOutcome::Completed {
                    summary,
                    trace: self.trace.clone(),
                };
            }
            if let AgentAction::AskUser { reason } = action {
                return AgentOutcome::NeedsUser {
                    reason,
                    trace: self.trace.clone(),
                };
            }

            if let Err(error) = validate_action_reference(&action, &page) {
                return AgentOutcome::Failed {
                    error,
                    trace: self.trace.clone(),
                };
            }

            if let Some(security_action) = security_action(&action, &page) {
                let decision = self.policy.evaluate(&security_action);
                if !decision.allowed {
                    if decision.requires_confirmation {
                        return AgentOutcome::NeedsApproval {
                            action,
                            decision,
                            trace: self.trace.clone(),
                        };
                    }
                    return AgentOutcome::Failed {
                        error: decision.reason,
                        trace: self.trace.clone(),
                    };
                }
            }

            if let AgentAction::Wait { millis } = &action
                && Duration::from_millis(*millis) > self.config.max_wait
            {
                return AgentOutcome::Failed {
                    error: "requested wait exceeds runtime budget".into(),
                    trace: self.trace.clone(),
                };
            }

            let result = match self.executor.execute(&action) {
                Ok(result) => result,
                Err(error) => {
                    return AgentOutcome::Failed {
                        error,
                        trace: self.trace.clone(),
                    };
                }
            };

            let result_summary = result
                .output
                .clone()
                .unwrap_or_else(|| format!("observed {}", result.page.url));
            self.trace.push(AgentStep {
                number,
                action,
                result_summary,
            });
            page = result.page;
        }

        AgentOutcome::StepLimit {
            trace: self.trace.clone(),
        }
    }
}

fn validate_action_reference(action: &AgentAction, page: &ObservedPage) -> Result<(), String> {
    let target = match action {
        AgentAction::Click { target }
        | AgentAction::TypeText { target, .. }
        | AgentAction::Select { target, .. }
        | AgentAction::Submit { target, .. } => Some(target),
        AgentAction::Extract {
            target: Some(target),
            ..
        } => Some(target),
        _ => None,
    };

    let Some(target) = target else {
        return Ok(());
    };
    if target.generation != page.generation {
        return Err("stale element reference after navigation".into());
    }

    let Some(observed) = page
        .elements
        .iter()
        .find(|element| element.id == target.id && element.generation == target.generation)
    else {
        return Err("element reference was not issued by current observation".into());
    };

    if observed.origin != target.origin
        || observed.frame != target.frame
        || observed.role != target.role
        || observed.name != target.name
        || observed.visible != target.visible
        || observed.interactable != target.interactable
    {
        return Err("element reference metadata does not match current observation".into());
    }

    if matches!(
        action,
        AgentAction::Click { .. }
            | AgentAction::TypeText { .. }
            | AgentAction::Select { .. }
            | AgentAction::Submit { .. }
    ) && (!observed.visible || !observed.interactable)
    {
        return Err("element is not currently visible and interactable".into());
    }

    Ok(())
}

fn security_action(action: &AgentAction, page: &ObservedPage) -> Option<AgentSecurityAction> {
    Some(match action {
        AgentAction::Navigate { url } => AgentSecurityAction::Navigate { url: url.clone() },
        AgentAction::Click { target } => AgentSecurityAction::Click {
            origin: target.origin.clone(),
            label: target.name.clone(),
        },
        AgentAction::TypeText {
            target,
            field,
            text,
        } => AgentSecurityAction::TypeText {
            origin: target.origin.clone(),
            field: *field,
            value_summary: format!("{} chars", text.chars().count()),
        },
        AgentAction::Select { target, value } => AgentSecurityAction::Click {
            origin: target.origin.clone(),
            label: format!("select {} = {}", target.name, value),
        },
        AgentAction::Submit {
            target,
            description,
        } => AgentSecurityAction::Submit {
            origin: target.origin.clone(),
            description: description.clone(),
        },
        AgentAction::Scroll { .. } | AgentAction::Wait { .. } => AgentSecurityAction::Read {
            origin: page_origin(page),
        },
        AgentAction::Extract { .. } => AgentSecurityAction::Extract {
            origin: page_origin(page),
        },
        AgentAction::AskUser { .. } | AgentAction::Finish { .. } => return None,
    })
}

fn page_origin(page: &ObservedPage) -> String {
    url::Url::parse(&page.url)
        .ok()
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| page.url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

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

    struct MockExecutor {
        page: ObservedPage,
    }

    impl AgentToolExecutor for MockExecutor {
        fn execute(&mut self, action: &AgentAction) -> Result<ToolResult, String> {
            if matches!(action, AgentAction::Navigate { .. }) {
                self.page.generation += 1;
            }
            Ok(ToolResult {
                page: self.page.clone(),
                output: Some("ok".into()),
            })
        }
    }

    fn page() -> ObservedPage {
        ObservedPage {
            generation: 1,
            url: "https://example.com/search".into(),
            title: "Search".into(),
            text_excerpt: "results".into(),
            elements: vec![
                element("textbox", "query"),
                element("article", "result"),
                element("button", "Send"),
                element("button", "Old"),
            ],
        }
    }

    fn element(role: &str, name: &str) -> AgentElement {
        AgentElement {
            id: format!("{role}-{name}"),
            generation: 1,
            role: role.into(),
            name: name.into(),
            text: String::new(),
            origin: "https://example.com".into(),
            frame: "top".into(),
            visible: true,
            interactable: true,
        }
    }

    #[test]
    fn bounded_read_search_extract_workflow_completes() {
        let search = element("textbox", "query");
        let result = element("article", "result");
        let planner = QueuePlanner {
            actions: VecDeque::from([
                AgentAction::TypeText {
                    target: search,
                    text: "rust webview".into(),
                    field: FieldKind::Search,
                },
                AgentAction::Extract {
                    target: Some(result),
                    schema: "title,url".into(),
                },
                AgentAction::Finish {
                    summary: "done".into(),
                },
            ]),
        };
        let executor = MockExecutor { page: page() };
        let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        policy.grant_reversible_session_actions(true);
        let mut runtime =
            AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());

        match runtime.run("research", page()) {
            AgentOutcome::Completed { summary, trace } => {
                assert_eq!(summary, "done");
                assert_eq!(trace.len(), 2);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn sensitive_submit_stops_for_human_approval() {
        let submit = element("button", "Send");
        let planner = QueuePlanner {
            actions: VecDeque::from([AgentAction::Submit {
                target: submit,
                description: "send message".into(),
            }]),
        };
        let executor = MockExecutor { page: page() };
        let policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        let mut runtime =
            AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());

        assert!(matches!(
            runtime.run("send", page()),
            AgentOutcome::NeedsApproval { .. }
        ));
    }

    #[test]
    fn stale_elements_are_rejected_after_navigation() {
        let mut stale = element("button", "Old");
        stale.generation = 0;
        let planner = QueuePlanner {
            actions: VecDeque::from([AgentAction::Click { target: stale }]),
        };
        let executor = MockExecutor { page: page() };
        let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        policy.grant_reversible_session_actions(true);
        let mut runtime =
            AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());

        match runtime.run("click", page()) {
            AgentOutcome::Failed { error, .. } => assert!(error.contains("stale")),
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn completed_trace_can_be_persisted_locally() {
        let planner = QueuePlanner {
            actions: VecDeque::from([AgentAction::Finish {
                summary: "done".into(),
            }]),
        };
        let executor = MockExecutor { page: page() };
        let policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        let mut runtime =
            AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());
        let outcome = runtime.run("research", page());

        let root =
            std::env::temp_dir().join(format!("neuralia-agent-trace-{}", std::process::id()));
        let path = root.join("trace.json");
        save_agent_outcome(&path, "research", &outcome).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("research"));
        assert!(text.contains("Completed"));
        let _ = fs::remove_dir_all(root);
    }

    struct CapturePlanner {
        seen: Arc<Mutex<ObservedPage>>,
    }

    impl AgentPlanner for CapturePlanner {
        fn plan(
            &mut self,
            _goal: &str,
            page: &ObservedPage,
            _trace: &[AgentStep],
        ) -> Result<AgentAction, String> {
            *self.seen.lock().unwrap() = page.clone();
            Ok(AgentAction::Finish {
                summary: "done".into(),
            })
        }
    }

    #[test]
    fn persisted_outcome_redacts_restricted_text_values() {
        let target = element("password", "Password");
        let planner = QueuePlanner {
            actions: VecDeque::from([AgentAction::TypeText {
                target: target.clone(),
                text: "synthetic-sensitive-value".into(),
                field: FieldKind::Password,
            }]),
        };
        let executor = MockExecutor { page: page() };
        let policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        let mut runtime =
            AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());

        let mut observed = page();
        observed.elements.push(target);
        let outcome = runtime.run("credential field", observed);
        assert!(matches!(outcome, AgentOutcome::NeedsApproval { .. }));

        let root =
            std::env::temp_dir().join(format!("neuralia-agent-redaction-{}", std::process::id()));
        let path = root.join("trace.json");
        save_agent_outcome(&path, "token=synthetic-sensitive-value", &outcome).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("synthetic-sensitive-value"));
        assert!(text.contains("REDACTED"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn planner_receives_sanitized_observation() {
        let seen = Arc::new(Mutex::new(page()));
        let planner = CapturePlanner {
            seen: Arc::clone(&seen),
        };
        let executor = MockExecutor { page: page() };
        let policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        let mut runtime =
            AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());

        let mut observed = page();
        observed.text_excerpt = "token=synthetic-sensitive-value\nbody: visible".into();
        let _ = runtime.run("inspect", observed);

        let seen = seen.lock().unwrap();
        assert!(!seen.text_excerpt.contains("synthetic-sensitive-value"));
        assert!(seen.text_excerpt.contains("body: visible"));
    }

    #[test]
    fn fabricated_element_reference_is_rejected() {
        let forged = AgentElement {
            id: "not-issued".into(),
            ..element("button", "Send")
        };
        let planner = QueuePlanner {
            actions: VecDeque::from([AgentAction::Click { target: forged }]),
        };
        let executor = MockExecutor { page: page() };
        let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        policy.grant_reversible_session_actions(true);
        let mut runtime =
            AgentRuntime::new(planner, executor, policy, AgentRuntimeConfig::default());

        match runtime.run("click", page()) {
            AgentOutcome::Failed { error, .. } => assert!(error.contains("not issued")),
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn runtime_has_no_arbitrary_javascript_action() {
        let variants = format!(
            "{:?}",
            AgentAction::Finish {
                summary: String::new()
            }
        );
        assert!(!variants.contains("JavaScript"));
    }
}
