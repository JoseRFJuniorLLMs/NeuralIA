use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::security::is_local_network_target;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub enum ActionRisk {
    ReadOnly,
    Reversible,
    Sensitive,
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    Search,
    Text,
    Email,
    Password,
    PaymentCard,
    Otp,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSecurityAction {
    Read {
        origin: String,
    },
    Extract {
        origin: String,
    },
    Navigate {
        url: String,
    },
    Click {
        origin: String,
        label: String,
    },
    TypeText {
        origin: String,
        field: FieldKind,
        value_summary: String,
    },
    Submit {
        origin: String,
        description: String,
    },
    Upload {
        origin: String,
        file_name: String,
    },
    DeleteRemote {
        origin: String,
        description: String,
    },
    Payment {
        origin: String,
        description: String,
    },
    Password {
        origin: String,
    },
    Otp {
        origin: String,
    },
    Captcha {
        origin: String,
    },
}

impl AgentSecurityAction {
    pub fn risk(&self) -> ActionRisk {
        match self {
            Self::Read { .. } | Self::Extract { .. } | Self::Navigate { .. } => {
                ActionRisk::ReadOnly
            }
            Self::Click { .. }
            | Self::TypeText {
                field: FieldKind::Search | FieldKind::Text,
                ..
            } => ActionRisk::Reversible,
            Self::TypeText {
                field: FieldKind::Email | FieldKind::Unknown,
                ..
            }
            | Self::Submit { .. }
            | Self::Upload { .. }
            | Self::DeleteRemote { .. } => ActionRisk::Sensitive,
            Self::TypeText {
                field: FieldKind::Password | FieldKind::PaymentCard | FieldKind::Otp,
                ..
            }
            | Self::Payment { .. }
            | Self::Password { .. }
            | Self::Otp { .. }
            | Self::Captcha { .. } => ActionRisk::Restricted,
        }
    }

    pub fn origin(&self) -> Option<String> {
        match self {
            Self::Navigate { url } => Url::parse(url)
                .ok()
                .map(|url| url.origin().ascii_serialization()),
            Self::Read { origin }
            | Self::Extract { origin }
            | Self::Click { origin, .. }
            | Self::TypeText { origin, .. }
            | Self::Submit { origin, .. }
            | Self::Upload { origin, .. }
            | Self::DeleteRemote { origin, .. }
            | Self::Payment { origin, .. }
            | Self::Password { origin }
            | Self::Otp { origin }
            | Self::Captcha { origin } => Some(origin.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub requires_confirmation: bool,
    pub risk: ActionRisk,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub sequence: u64,
    pub risk: ActionRisk,
    pub action: String,
    pub origin: Option<String>,
    pub allowed: bool,
    pub confirmation_required: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPermissionPolicy {
    initial_origin: Option<String>,
    approved_origins: BTreeSet<String>,
    session_reversible_grant: bool,
    stopped: bool,
    sequence: u64,
    audit: Vec<AuditEntry>,
}

impl AgentPermissionPolicy {
    pub fn new(initial_origin: Option<String>) -> Self {
        let mut approved_origins = BTreeSet::new();
        if let Some(origin) = initial_origin.as_ref() {
            approved_origins.insert(origin.clone());
        }
        Self {
            initial_origin,
            approved_origins,
            session_reversible_grant: false,
            stopped: false,
            sequence: 0,
            audit: Vec::new(),
        }
    }

    pub fn grant_reversible_session_actions(&mut self, granted: bool) {
        self.session_reversible_grant = granted;
    }

    pub fn approve_origin(&mut self, origin: impl Into<String>) {
        self.approved_origins.insert(origin.into());
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn resume(&mut self) {
        self.stopped = false;
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn audit(&self) -> &[AuditEntry] {
        &self.audit
    }

    pub fn evaluate(&mut self, action: &AgentSecurityAction) -> PolicyDecision {
        let risk = action.risk();
        let mut decision = if self.stopped {
            PolicyDecision {
                allowed: false,
                requires_confirmation: false,
                risk,
                reason: "agent stopped by user".into(),
            }
        } else {
            self.evaluate_live(action, risk)
        };

        if risk == ActionRisk::Restricted {
            decision.allowed = false;
            decision.requires_confirmation = true;
        }

        self.sequence = self.sequence.saturating_add(1);
        self.audit.push(AuditEntry {
            sequence: self.sequence,
            risk,
            action: audit_action_name(action).to_string(),
            origin: action.origin(),
            allowed: decision.allowed,
            confirmation_required: decision.requires_confirmation,
            reason: decision.reason.clone(),
        });
        decision
    }

    fn evaluate_live(&self, action: &AgentSecurityAction, risk: ActionRisk) -> PolicyDecision {
        if let AgentSecurityAction::Navigate { url } = action {
            let Ok(parsed) = Url::parse(url) else {
                return deny(risk, "invalid navigation URL");
            };

            if is_local_network_target(&parsed) {
                return confirm(risk, "navigation from web agent to local/private network");
            }

            let origin = parsed.origin().ascii_serialization();
            if self.initial_origin.is_some() && !self.approved_origins.contains(&origin) {
                return confirm(risk, "cross-origin navigation needs approval");
            }
        }

        match risk {
            ActionRisk::ReadOnly => allow(risk, "read-only action"),
            ActionRisk::Reversible if self.session_reversible_grant => {
                allow(risk, "reversible session grant")
            }
            ActionRisk::Reversible => confirm(risk, "reversible interaction needs session grant"),
            ActionRisk::Sensitive => confirm(risk, "sensitive remote-state change"),
            ActionRisk::Restricted => confirm(risk, "human-only action"),
        }
    }

    pub fn approval_fingerprint(&self, action: &AgentSecurityAction) -> String {
        let mut hash = Sha256::new();
        hash.update(format!("{action:?}"));
        hash.update(self.sequence.to_le_bytes());
        if let Some(initial) = &self.initial_origin {
            hash.update(initial.as_bytes());
        }
        format!("{:x}", hash.finalize())
    }
}

fn allow(risk: ActionRisk, reason: &str) -> PolicyDecision {
    PolicyDecision {
        allowed: true,
        requires_confirmation: false,
        risk,
        reason: reason.into(),
    }
}

fn confirm(risk: ActionRisk, reason: &str) -> PolicyDecision {
    PolicyDecision {
        allowed: false,
        requires_confirmation: true,
        risk,
        reason: reason.into(),
    }
}

fn deny(risk: ActionRisk, reason: &str) -> PolicyDecision {
    PolicyDecision {
        allowed: false,
        requires_confirmation: false,
        risk,
        reason: reason.into(),
    }
}

fn audit_action_name(action: &AgentSecurityAction) -> &'static str {
    match action {
        AgentSecurityAction::Read { .. } => "read",
        AgentSecurityAction::Extract { .. } => "extract",
        AgentSecurityAction::Navigate { .. } => "navigate",
        AgentSecurityAction::Click { .. } => "click",
        AgentSecurityAction::TypeText { .. } => "type",
        AgentSecurityAction::Submit { .. } => "submit",
        AgentSecurityAction::Upload { .. } => "upload",
        AgentSecurityAction::DeleteRemote { .. } => "delete",
        AgentSecurityAction::Payment { .. } => "payment",
        AgentSecurityAction::Password { .. } => "password",
        AgentSecurityAction::Otp { .. } => "otp",
        AgentSecurityAction::Captcha { .. } => "captcha",
    }
}

pub fn redact_sensitive_text(input: &str) -> String {
    let mut output = Vec::new();
    for raw in input.lines() {
        let lower = raw.to_ascii_lowercase();
        let sensitive = [
            "authorization:",
            "cookie:",
            "set-cookie:",
            "password=",
            "password:",
            "passwd=",
            "otp=",
            "token=",
            "access_token",
            "refresh_token",
            "card_number",
            "cvv=",
        ]
        .iter()
        .any(|needle| lower.contains(needle));

        if sensitive {
            let key = raw.split([':', '=']).next().unwrap_or("sensitive").trim();
            output.push(format!("{key}: [REDACTED]"));
        } else {
            output.push(raw.to_string());
        }
    }
    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_values_are_redacted_before_storage_or_model_context() {
        let input = "title: ok\nAuthorization: Bearer abc\npassword=hunter2\nbody: visible";
        let clean = redact_sensitive_text(input);
        assert!(clean.contains("body: visible"));
        assert!(!clean.contains("Bearer abc"));
        assert!(!clean.contains("hunter2"));
    }

    #[test]
    fn public_page_cannot_pivot_into_private_network_silently() {
        let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
        let decision = policy.evaluate(&AgentSecurityAction::Navigate {
            url: "http://127.0.0.1:9000/admin".into(),
        });
        assert!(!decision.allowed);
        assert!(decision.requires_confirmation);
    }

    #[test]
    fn password_payment_otp_and_captcha_are_human_only() {
        let mut policy = AgentPermissionPolicy::new(Some("https://shop.example".into()));
        for action in [
            AgentSecurityAction::Password {
                origin: "https://shop.example".into(),
            },
            AgentSecurityAction::Otp {
                origin: "https://shop.example".into(),
            },
            AgentSecurityAction::Captcha {
                origin: "https://shop.example".into(),
            },
            AgentSecurityAction::Payment {
                origin: "https://shop.example".into(),
                description: "confirm purchase".into(),
            },
        ] {
            let decision = policy.evaluate(&action);
            assert!(!decision.allowed);
            assert!(decision.requires_confirmation);
            assert_eq!(decision.risk, ActionRisk::Restricted);
        }
    }

    #[test]
    fn kill_switch_blocks_even_read_only_actions() {
        let mut policy = AgentPermissionPolicy::new(None);
        policy.stop();
        let decision = policy.evaluate(&AgentSecurityAction::Extract {
            origin: "https://example.com".into(),
        });
        assert!(!decision.allowed);
        assert!(!decision.requires_confirmation);
    }
}
