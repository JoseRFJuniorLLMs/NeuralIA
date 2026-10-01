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
| [SPEC-0108](SPEC-0108-secure-webview-ipc-channel.md) | Bounded authenticated page→native WebView2 IPC | 2.1 |
| [SPEC-0109](SPEC-0109-webrtc-media-permissions.md) | WebRTC media permissions with native consent | 2.1 |
| [SPEC-0110](SPEC-0110-pdf-text-reader.md) | Bounded PDF text extraction into semantic memory | 2.5 |
| [SPEC-0114](SPEC-0114-anti-distracao.md) | Anti-distraction: cookie banners, newsletter modals, large fixed bars (reject-only on known CMPs) | 2.4 |
| [SPEC-0115](SPEC-0115-chat-surface.md) | NeuralIA Chat Surface (Multi-Provider Conversation Layer) | 2.7 |
| [SPEC-0116](SPEC-0116-neural-read-aloud.md) | Neural Read Aloud: Edge TTS WebSocket protocol and audio pipeline | 2.7 |

A matriz que responde explicitamente “este teste exercita o caminho que
embarca?” está em
[AUDIT-SPEC-0100-0108](AUDIT-SPEC-0100-0108.md), hoje ampliada até as SPECs existentes de 0116 (0111–0113 não existem no repositório).

## Implementation status

- **SPEC-0100–0101:** implemented baselines; core behavior and the shipped
  `neural-app` wiring are both gated.
- **SPEC-0102:** partial. Deterministic local semantics/embeddings are integrated;
  model-pack install/verify/benchmark/explicit-activation/fallback lifecycle is
  covered in `neural-core`, but no model backend or product UI/startup wiring ships;
  the context budget (`neural_core::context_budget`) is library-only until the
  2.5 consensus wires it (§8.2).
- **SPEC-0103:** implemented. The shipped JavaScript rails have provider fixtures,
  paired-ratio performance gates and deliberate sabotage proofs in CI; the Rust
  parser remains a reference path, not a substitute for the shipped JS gate.
- **SPEC-0104:** security policy is used by the shipped agent path; independent
  adversarial review remains a release gate.
- **SPEC-0105:** the shipped runtime is `handle_agent_observation → decide_agent_step`;
  its gate is tested on the product path. The unused parallel
  `neural-core::AgentRuntime` loop was removed by PR #80; only the shared
  protocol/configuration types remain in `agent_protocol`.
- **SPEC-0106:** execution roadmap, not a runtime feature or a standalone "green"
  object-construction test.
- **SPEC-0107:** partial advanced integration. SQLite/FTS5, RRF
  lexical/entity/graph/semantic retrieval, granular forget/tombstones, Doctor/rebuild,
  async capture, scale gates and Ctrl+H semantic recall ship. Optional local
  embedding/model backend, model compatibility in Doctor, real-backend startup-idle
  gates and some granular maintenance UX remain open.
- **SPEC-0108:** bounded WebView2 IPC is implemented and product-tested; the
  independent adversarial/release gate remains pending.
- **SPEC-0109:** WebRTC media capture uses the native WebView2 consent flow on
  visible web surfaces; unrelated/sensitive permissions remain fail-closed.
- **SPEC-0110:** bounded PDF TextLayer extraction is integrated into semantic
  memory through the authenticated PDF-only IPC path; image-only PDFs remain
  outside scope because there is no OCR.
- **SPEC-0114:** integrated into `main` through release/2.4.0 (PR #169). The
  node:vm/script policy gates ship; a real-WebView2 COM E2E and the global
  settings/`/distracoes` controls remain open.
- **SPEC-0111–0113:** no specification files exist under these numbers; this is
  a numbering gap, not an implementation state.
- **SPEC-0115:** NeuralIA Chat Surface (Multi-Provider Conversation Layer). Core domain model and structured turns ship in `neural_core::chat`.
- **SPEC-0116:** Leitura em Voz Alta Neural (Edge TTS Protocol & SSML Builder). Core protocol and synthesis builder ship in `neural_core::speech`.

`crates/neural-core/tests/spec_010x_acceptance.rs` now keeps SPEC numbers only
where the tested core is the same core used by the product. Reference-only checks for the Rust semantic-timeline parser and roadmap
object coexistence no longer present themselves as product acceptance gates;
the unused `AgentRuntime` harness itself was removed by PR #80.
Shipped wiring is pinned in `crates/neural-app/tests/spec_product_wiring.rs`,
and SPEC-0107 Phase 1 has its own operational acceptance test.

Future changes MUST NOT silently violate the existing product charter,
performance budget, privacy guarantees or threat model.

The boring rule is intentional: architecture should be allowed to evolve;
scope creep should not be allowed to wear an architecture badge.
