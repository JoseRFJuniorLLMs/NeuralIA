# NeuralIA — Future Architecture Specifications

This directory contains the NeuralIA future-architecture specifications and
their current implementation state. Some are implemented baselines; others are
roadmaps or phased work. They extend the Rust + WRY + system-WebView design
without turning the project into a Chromium/Electron rewrite.

## Product thesis

NeuralIA should evolve from a multi-AI browser into a **local-first research and
navigation environment that can understand, organize and, later, safely act on
the Web**.

The browser remains infrastructure. The product is the research workflow.

## Non-negotiable constraints

- Do not embed Chromium, CEF or Electron in the default distribution.
- Do not move the application shell to JavaScript.
- Do not make Playwright the primary navigation engine.
- Do not ship a multi-gigabyte local chat model in the base installer.
- Home remains native and fast, and network-idle until the user has browsed
  with a Google session. From that point the Gmail monitor keeps one hidden
  WebView alive by design — including while Home is showing — and only
  `NEURALIA_NO_GMAIL` removes it. The Home performance gate
  (`scripts/measure-home.ps1`) measures the network-idle state, before any
  session exists; it does not cover the monitor.
- Private/incognito navigation never enters semantic memory.
- Remote page content is untrusted data, never authority.
- Agent actions are policy-gated and auditable.
- Sensitive or irreversible actions require explicit human approval.
- New capabilities must preserve the performance and threat-model budgets of
  the existing normative specs.

## Specifications

| Spec | Subject | Intended milestone |
|---|---|---|
| [SPEC-0100](SPEC-0100-semantic-memory.md) | Local semantic memory and conceptual history search | 1.7 |
| [SPEC-0101](SPEC-0101-research-sessions.md) | Multi-source research sessions and cross-source synthesis | 1.8 |
| [SPEC-0102](SPEC-0102-local-intelligence.md) | Optional on-device intelligence/model packs | 1.9 |
| [SPEC-0103](SPEC-0103-semantic-timeline.md) | Semantic response/navigation timeline | 1.9 |
| [SPEC-0104](SPEC-0104-agent-security.md) | Agent security, permissions and prompt-injection defense | 2.0 gate |
| [SPEC-0105](SPEC-0105-agent-runtime.md) | DOM/accessibility-first web agent runtime | 2.0 |
| [SPEC-0106](SPEC-0106-execution-roadmap.md) | Implementation order and release gates | 1.7 → 2.0 |
| [SPEC-0107](SPEC-0107-ai-memory-native-integration.md) | Native import/adaptation of ai-memory into NeuralIA memory crates | 1.7–1.9 |

A matriz que responde explicitamente “este teste exercita o caminho que
embarca?” está em
[AUDIT-SPEC-0100-0108](AUDIT-SPEC-0100-0108.md).

## Implementation status

- **SPEC-0100–0101:** implemented baselines; core behavior and the shipped
  `neural-app` wiring are both gated.
- **SPEC-0102:** partial. Deterministic local semantics/embeddings are integrated;
  model-pack install/verify/benchmark/explicit-activation/fallback lifecycle is
  covered in `neural-core`, but no model backend or product UI/startup wiring ships.
- **SPEC-0103:** implemented. The shipped JavaScript rails have provider fixtures,
  paired-ratio performance gates and deliberate sabotage proofs in CI; the Rust
  parser remains a reference path, not a substitute for the shipped JS gate.
- **SPEC-0104:** security policy is used by the shipped agent path; independent
  adversarial review remains a release gate.
- **SPEC-0105:** the shipped runtime is `handle_agent_observation → decide_agent_step`;
  its gate is now tested on the product path. `neural-core::AgentRuntime` is a
  reference harness, not the browser execution loop.
- **SPEC-0106:** execution roadmap, not a runtime feature or a standalone "green"
  object-construction test.
- **SPEC-0107:** Phase 0 is complete and Phase 1 is partial: SQLite/FTS5,
  retrieval/rerank, tombstones and rebuild are operational and acceptance-tested;
  the full integration/UX/performance criteria remain open.
- **SPEC-0108:** bounded WebView2 IPC is implemented and product-tested; the
  independent adversarial/release gate remains pending.

`crates/neural-core/tests/spec_010x_acceptance.rs` now keeps SPEC numbers only
where the tested core is the same core used by the product. Reference-only
checks for the Rust semantic-timeline parser, `AgentRuntime` harness and roadmap
object coexistence no longer present themselves as product acceptance gates.
Shipped wiring is pinned in `crates/neural-app/tests/spec_product_wiring.rs`,
and SPEC-0107 Phase 1 has its own operational acceptance test.

Future changes MUST NOT silently violate the existing product charter,
performance budget, privacy guarantees or threat model.

The boring rule is intentional: architecture should be allowed to evolve;
scope creep should not be allowed to wear an architecture badge.
