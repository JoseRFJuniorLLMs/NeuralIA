# SPEC-0104 — Agent Security and Permission Architecture

**Status:** Implementada — auditoria independente pendente (NeuralIA 2.0)  
**Target:** NeuralIA 2.0 security gate

## 1. Principle

A web page is **untrusted data**.

It never becomes an authority merely because an AI model can read it.

This specification is a prerequisite for any NeuralIA feature that performs
autonomous web actions.

## 2. Trust domains

```text
USER INTENT            trusted instruction
     │
     ▼
PLANNER                untrusted model output
     │
     ▼
POLICY ENGINE          trusted native code
     │
     ▼
TOOL EXECUTOR          narrowly scoped capability
     │
     ▼
WEB PAGE               untrusted environment
```

Remote page text, DOM attributes, accessibility labels, screenshots and model
outputs are all treated as potentially hostile.

## 3. No direct tool authority from page content

The following design is forbidden:

```text
web page → prompt → LLM → unrestricted tool call
```

Instead:

```text
page observation
      │
      ▼
structured facts
      │
      ▼
planner proposal
      │
      ▼
native policy validation
      │
      ▼
bounded action
```

## 4. Permission classes

### Class A — read-only

May execute without confirmation when user initiated:

- read DOM/accessibility text;
- scroll;
- inspect links;
- extract table data;
- open same-purpose source pages.

### Class B — reversible interaction

May execute under a session grant:

- select filters;
- fill non-sensitive search fields;
- navigate pagination;
- open/close temporary tabs.

### Class C — sensitive communication/state

Requires explicit confirmation immediately before execution:

- submit a form;
- send email/message;
- post content;
- upload a file;
- create/delete remote records;
- modify account settings.

### Class D — financial/authentication/irreversible

Must always stop for human control:

- payment or purchase confirmation;
- password entry/change;
- 2FA/OTP;
- CAPTCHA;
- signing a legal agreement;
- destructive account operations.

The agent may prepare a Class D action but MUST NOT complete it autonomously.

The native core represents these levels explicitly as `PermissionClass`.
`AgentSecurityAction::permission_class()` is the single mapping from actions
to A/B/C/D. The session grant applies only to Class B; Class C remains an
immediate per-action confirmation and Class D cannot be unlocked by a session
grant or by recording an approval.

## 5. Sensitive data firewall

The observer supplied to models MUST exclude or redact where possible:

- cookies;
- authorization headers;
- password inputs;
- payment-card inputs;
- browser profile secrets;
- OS credential stores;
- unrelated local files.

An agent never gets a general “read browser profile” tool.

The core text firewall normalizes whitespace/quoting before matching common
credential keys, so JSON-like fields and spaced assignments do not bypass the
redactor. This is defense in depth: structured observers must still avoid
collecting secrets in the first place.

## 6. Origin policy

Each agent run maintains:

- initial origin;
- currently approved origins;
- navigation chain;
- local/private-network policy.

Public pages MUST NOT pivot into loopback/private-network targets without a
fresh explicit user decision.

Cross-origin navigation may reduce granted capabilities.

## 7. Prompt-injection defenses

The runtime MUST test against:

- visible hostile instructions;
- CSS-hidden text;
- white-on-white content;
- aria-label injections;
- HTML comments/data attributes;
- nested iframes;
- encoded/Unicode-obfuscated instructions;
- fake system-message UI;
- instructions embedded in PDFs/documents.

The correct response is not “detect all prompt injection”. That is impossible.

The security boundary is that injected text **cannot grant capabilities**.

## 8. Dual-model architectures

A separate extractor/planner model MAY reduce risk, but it is not a security
boundary.

The native policy engine remains authoritative.

## 9. Audit log

Every agent run records locally:

- user goal;
- observation summaries;
- proposed actions;
- policy decisions;
- user confirmations;
- executed actions;
- origin changes;
- termination reason.

Secrets and full sensitive field contents MUST NOT be logged.

## 10. Kill switch

The user can stop an agent immediately.

Stopping prevents new actions, cancels queued actions where possible and
returns manual control. It also revokes the Class B session grant and any
cross-origin approvals accumulated during the run; resuming starts from the
initial origin without restoring those grants.

## 11. Acceptance criteria

Agent execution is forbidden from release until:

1. capability classes exist in native code;
2. sensitive actions require the right approval level;
3. cookies/passwords are never exposed as model context;
4. public-to-private network pivot tests pass;
5. adversarial prompt-injection fixtures cannot grant extra tools;
6. every executed action is auditable;
7. kill-switch tests pass.
