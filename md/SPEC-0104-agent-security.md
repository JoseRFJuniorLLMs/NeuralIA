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

May execute under a session grant only when the reversible class comes from a
**native structured action**, not from page-provided role/name/text:

- select filters through the native `Select` action;
- fill non-sensitive search fields through the native `Search` action;
- navigate pagination when represented by a bounded native navigation action;
- open/close temporary tabs.

A generic DOM click is ambiguous remote authority and is therefore **not**
Class B merely because the page labels it “Next”, “Continue” or similar.
Remote metadata may cause a stricter classification, but it cannot lower the
native risk floor.

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

## 4.1. Untrusted-metadata risk floor

The shipped policy distinguishes **intent class** from **page description**.

- generic `AgentSecurityAction::Click` is Class C by default;
- `AgentSecurityAction::Select` is Class B because the application created a
  structured select action before consulting the page policy metadata;
- `TypeText(Search)` is Class B;
- arbitrary `TypeText(Text|Email|Unknown)` is Class C;
- password/payment-card/OTP typing remains Class D.

The page's role, accessible name and text may be used to find an element or to
raise risk (for example a payment-looking click), but they are never sufficient
evidence to downgrade a generic action into the reversible session grant.

The product gate `generic_click_never_inherits_a_reversible_grant_from_page_metadata`
exercises the shipped mapping; CI sabotage deliberately restores the unsafe
Class-B click and requires that gate to turn red.

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
returns manual control.

## 10.1. Native policy boundary implemented

The native policy exposes the four permission classes explicitly as
`CapabilityClass::{AReadOnly,BReversible,CSensitive,DRestricted}`. Every
`AgentSecurityAction` maps to one class before a policy decision is recorded.

Class B authority is a session grant and is revoked by the kill switch. Class C
may proceed only after a positive one-action user confirmation. Class D remains
human-only even after a positive confirmation record; the confirmation is
audited but never becomes autonomous authority.

`redact_sensitive_text` is the shared model/storage firewall for textual
context. It removes authorization/cookie material, password/OTP/token forms,
API/client secrets and payment-card/CVV/CVC forms before those strings are
persisted or handed to reference planner context. The browser still must avoid
creating general profile/credential tools; redaction is defense in depth, not
permission to expose those sources.

The kill switch is fail-closed: `stop()` blocks all subsequent policy
decisions and revokes reversible-session and extra-origin grants. A later
`resume()` does not silently restore those grants.

## 11. Acceptance criteria

Agent execution is forbidden from release until:

1. capability classes exist in native code;
2. sensitive actions require the right approval level, and page metadata cannot
   downgrade a generic click/text action into the reversible session grant;
3. cookies/passwords are never exposed as model context;
4. public-to-private network pivot tests pass;
5. adversarial prompt-injection fixtures cannot grant extra tools;
6. every executed action is auditable;
7. kill-switch tests pass.
