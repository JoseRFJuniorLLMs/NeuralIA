use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::agent_security::{
    AgentPermissionPolicy, AgentSecurityAction, FieldKind, PolicyDecision,
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
    struct StoredOutcome<'a> {
        goal: &'a str,
        outcome: &'a AgentOutcome,
    }

    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes =
        serde_json::to_vec_pretty(&StoredOutcome { goal, outcome }).map_err(io::Error::other)?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(temp, path)
        }
        Err(error) => Err(error),
    }
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

            let action = match self.planner.plan(goal, &page, &self.trace) {
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

            if let Err(error) = validate_action_generation(&action, page.generation) {
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

fn validate_action_generation(action: &AgentAction, current: u64) -> Result<(), String> {
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

    if target.is_some_and(|target| target.generation != current) {
        Err("stale element reference after navigation".into())
    } else {
        Ok(())
    }
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
    use std::collections::VecDeque;

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
            elements: Vec::new(),
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
