//! Shared structured vocabulary used by the shipped NeuralIA agent path.
//!
//! This module is deliberately *not* an agent runtime. The execution loop that
//! ships lives in `neural-app` (`decide_agent_step` -> native confirmation ->
//! `execute_agent_action`). These types are only the data/config contract that
//! both sides use.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::agent_security::FieldKind;

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
