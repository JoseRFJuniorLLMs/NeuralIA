# NeuralIA — Future Architecture Specifications

This directory contains **proposed** specifications for the next architectural
steps of NeuralIA. They extend the current Rust + WRY + system-WebView design;
they do not authorize a rewrite.

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
- Home remains native, fast and network-idle.
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

## Adoption rule

These files are design proposals until an implementation milestone explicitly
promotes them to normative status. A future feature MUST NOT silently violate
the existing product charter, performance budget or threat model.

The boring rule is intentional: architecture should be allowed to evolve;
scope creep should not be allowed to wear an architecture badge.
