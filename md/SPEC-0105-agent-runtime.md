# SPEC-0105 — Web Agent Runtime

**Status:** Proposed  
**Target:** NeuralIA 2.0  
**Dependency:** SPEC-0104 must be implemented first

## 1. Purpose

NeuralIA should eventually be able to carry out bounded web tasks such as:

- research several pages;
- apply filters;
- fill non-sensitive search forms;
- collect structured results;
- compare options;
- prepare a workflow for user confirmation.

The goal is **assisted autonomous navigation**, not an unrestricted desktop
robot.

## 2. Observation hierarchy

The runtime uses the cheapest structured signal first:

1. DOM;
2. accessibility tree;
3. browser/DevTools metadata;
4. rendered screenshot/vision fallback.

Computer vision is a fallback, not the default click engine.

If the DOM says a visible element is a button with a stable role and label,
guessing pixel coordinates with a multimodal model is unnecessary.

## 3. Core loop

```text
Observe
  │
  ▼
Plan
  │
  ▼
Validate against policy
  │
  ▼
Execute one bounded action
  │
  ▼
Observe result
  └─────────────► repeat
```

Every iteration is finite and auditable.

## 4. Action vocabulary

The first runtime SHOULD use a small explicit action enum.

```rust
enum AgentAction {
    Navigate { url: String },
    Click { target: ElementRef },
    TypeText { target: ElementRef, text: String },
    Select { target: ElementRef, value: String },
    Scroll { target: ScrollTarget, amount: ScrollAmount },
    Extract { target: ElementRef, schema: ExtractSchema },
    Wait { condition: WaitCondition },
    AskUser { reason: String },
    Finish { summary: String },
}
```

There is no generic “execute arbitrary JavaScript from the model” action.

## 5. Element references

The observer produces stable, short-lived element references with:

- role;
- accessible name;
- relevant text;
- bounding rectangle;
- DOM selector/path metadata;
- origin/frame identity;
- visibility/interactability state.

The planner refers to IDs, not raw injected JavaScript.

References expire after meaningful navigation/DOM replacement.

## 6. Planner interface

The planner receives:

- user goal;
- sanitized current observation;
- prior action/result summaries;
- remaining step/time budget;
- allowed capabilities.

The planner returns structured candidate actions.

Malformed output is rejected.

## 7. Runtime limits

Every run has:

- maximum steps;
- maximum wall time;
- per-navigation timeout;
- maximum open temporary pages;
- origin policy;
- action retry limit;
- extraction size budget.

Loops terminate rather than “thinking harder forever”.

## 8. Human-in-the-loop

When SPEC-0104 classifies an action as sensitive, the runtime emits a native
approval request that explains:

- what will happen;
- which site/origin;
- what data will be sent;
- whether the action is reversible.

After the user completes CAPTCHA, 2FA or payment manually, the agent MAY resume
from a fresh observation.

## 9. Browser integration strategy

Primary implementation should reuse the existing system WebView architecture.

Preferred exploration order:

1. WebView2 DOM/script bridge with narrow app-owned adapters;
2. DevTools protocol for observation/debug metadata;
3. accessibility tree;
4. screenshot capture for vision fallback.

CEF/Electron are not justified merely to implement the agent.

Playwright MAY exist later as an optional external worker for specialized
automation, but it is not the primary browser engine.

## 10. Model independence

The runtime is not coupled to LangGraph, AutoGen or a particular model vendor.

A simple native state machine is preferred first.

Cloud or local planners can implement the same structured interface.

## 11. Failure handling

On unexpected page change, stale element, challenge page or policy failure:

- stop the current action;
- re-observe;
- retry only within budget;
- ask the user when uncertainty affects a sensitive operation.

The agent never bypasses CAPTCHA or authentication challenges.

## 12. Acceptance criteria

1. a test workflow can navigate, search, filter and extract structured results;
2. the same workflow survives ordinary DOM movement through semantic element
   references;
3. no arbitrary model-provided JavaScript is executed;
4. Class C/D actions stop for approval;
5. step/time budgets terminate loops;
6. interruption returns manual control immediately;
7. full action/result traces are available locally.
