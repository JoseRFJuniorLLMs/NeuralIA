# SPEC-0105 — Web Agent Runtime

**Status:** Implementada no caminho embarcado — gate em `decide_agent_step`; auditoria adversarial independente pendente  
**Target:** NeuralIA 2.0  
**Dependency:** SPEC-0104

## 1. Purpose

NeuralIA provides bounded assisted Web navigation for deliberately narrow
workflows: search, select/filter, click, extract and stop for the user when risk
increases. It is not an unrestricted desktop robot and it does not expose a
generic model-provided JavaScript tool.

## 2. The runtime that actually ships

As of 2026-09-19 the application runtime is the path below:

```text
AGENT_OBSERVER_SCRIPT
        │  bounded WebView2 IPC observation
        ▼
handle_agent_observation
        │
        ▼
decide_agent_step
  ├─ step/wall-clock budget
  ├─ semantic element selection
  └─ AgentPermissionPolicy
        │
        ├─ Stop
        ├─ Extract
        └─ Act ──► native confirmation when required
                     │
                     ▼
               execute_agent_action
                     │
                     ▼
                next observation
```

`decide_agent_step` is intentionally free of UI, WebView and memory side
effects so the security/limit decision can be tested on the same code that the
product calls.

`neural_core::AgentRuntime`, `AgentPlanner` and `AgentToolExecutor` remain
a reference/library harness. They are **not** the execution loop used by
`neural-app`. Tests of that harness are useful unit coverage but are not, by
themselves, acceptance evidence for this specification.

## 3. Observation and action vocabulary

The injected observer emits a bounded structured page observation. The
application resolves explicit commands into the existing `AgentAction` types,
currently including:

- search/type text;
- click;
- select/filter;
- extraction;
- initial navigation to a validated HTTP(S) target.

The page observation is data, not authority. The model/user plan never supplies
raw JavaScript for execution. `agent_action_script` is application-owned code
generated from a validated structured action.

## 4. Decision gate

Every actionable observation passes through `decide_agent_step`.

The function:

1. reads limits from `AgentRuntimeConfig::default()`, avoiding a second set of
   hard-coded budgets in the application;
2. resolves the next structured action against the fresh observation;
3. maps it to `AgentSecurityAction`;
4. calls the same `AgentPermissionPolicy` used by the product;
5. stops restricted actions;
6. returns a native-confirmation reason for confirmable sensitive actions;
7. returns an executable action only after the policy gate allows it.

The tests introduced with PR #43 deliberately fail when
`policy.evaluate(...)` is removed from this shipped decision function.

## 5. Element references and DOM movement

Each observation rebuilds short-lived element records with generation, role,
accessible name/text, origin/frame and interactability information. Commands
are resolved again from the latest observation before execution. The
application-owned execution script also guards the expected element properties
before acting.

References are therefore ephemeral. A stale page must produce a new
observation rather than granting authority to an old target.

## 6. Runtime limits

The shipped decision path uses the same default budget source as the reference
runtime:

- maximum steps;
- maximum wall time;
- bounded observation payload;
- origin policy;
- policy-controlled sensitive actions.

The product stops before consulting/executing the next action when the step or
wall-clock budget is exhausted.

## 7. Human in the loop

Sensitive confirmable actions are returned as `AgentStepDecision::Act` with a
confirmation reason. `handle_agent_observation` displays the native approval
dialog and records the answer in `AgentPermissionPolicy` before execution.

Restricted actions, including password/payment-class authority, do not become
allowed merely because a user confirmation was requested.

CAPTCHA, 2FA and payment completion remain manual user work.

## 8. Trace and interruption

Executed actions append application-owned summaries to the local agent trace.
Extraction is redacted before being shown or captured into semantic memory.

The UI advertises Esc/Home as immediate manual interruption. A stopped run does
not continue acting on later observations.

## 9. Planner independence

The current shipped planner is intentionally boring: the `agent:` command is
parsed into a bounded queue of structured browser commands. A future local or
cloud planner may replace that command source, but it must feed the same
structured decision/policy boundary.

Changing planner technology must not create a second execution path around
`decide_agent_step`.

## 10. Browser integration

The implementation reuses the existing system WebView2 architecture. Remote
page-to-native traffic uses the bounded IPC channel from SPEC-0108. The agent
does not require CEF, Electron or Playwright as the primary browser engine.

## 11. Failure handling

On budget exhaustion, missing element, restricted authority, user rejection or
execution error, the application terminates the run with an explicit
`AgentTermination` reason. Unexpected page changes are handled by observing
again; the agent does not bypass authentication challenges.

## 12. Acceptance criteria

The shipped SPEC-0105 gate is green only when:

1. the product path calls `decide_agent_step` before execution;
2. search/click/select actions are resolved from a fresh structured observation;
3. restricted actions stop before reaching the page;
4. confirmable sensitive actions require the native human gate;
5. step and wall-clock budgets terminate the product path;
6. no arbitrary model-provided JavaScript execution channel exists;
7. action/extraction results produce local trace material;
8. the product-wiring acceptance test cites SPEC-0105 and fails if the shipped
   decision path is bypassed.

The independent adversarial review required by `AGENTS.md` §4/§7 remains a
separate release gate.
