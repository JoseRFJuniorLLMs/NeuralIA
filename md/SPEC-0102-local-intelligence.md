# SPEC-0102 — Optional Local Intelligence

**Status:** Parcial — inteligência local determinística integrada; `ModelPackManager` é biblioteca, não feature de produto  
**Target:** NeuralIA 1.9

## 1. Purpose

NeuralIA benefits from on-device intelligence, but the default browser must not
turn into a multi-gigabyte chatbot runtime.

Local intelligence exists to make the browser **smarter in the background**.

## 2. Initial tasks

The first local model pack SHOULD focus on bounded tasks:

- text embeddings;
- intent classification;
- semantic grouping;
- entity extraction;
- relevance scoring;
- short-page summarization;
- query rewriting;
- local routing between Reader, provider query, memory search and agent tools.

A general-purpose local chat assistant is explicitly not required for the
first release.

## 3. Packaging

The base NeuralIA installer MUST remain usable without a model pack.

```text
NeuralIA base
├── Rust core
├── native UI
├── WRY / system WebView
└── optional model interface

Optional packs
├── semantic-small
└── assistant-local (future)
```

Model packs:

- are downloaded only after explicit user action;
- include a manifest, version, hash and license metadata;
- can be removed independently;
- are never required to launch Home.

## 4. Runtime abstraction

The core SHOULD depend on capabilities, not a specific inference engine.

```rust
trait LocalIntelligence {
    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>, LocalAiError>;
    fn classify(&self, input: &str) -> Result<IntentClass, LocalAiError>;
    fn summarize(&self, input: &str, budget: usize) -> Result<String, LocalAiError>;
}
```

Possible backends include ONNX Runtime or llama.cpp-compatible runtimes, but
neither becomes a product-level dependency until measured.

WebGPU MAY be used where it is genuinely advantageous. It is not a requirement.

## 5. Resource budgets

The local model is lazy-loaded.

- Native Home resident model memory: **0 bytes**.
- Model unload/idle policy MUST exist.
- The first semantic pack SHOULD target a modest memory footprint.
- Large 1B–3B models are optional future packs, not the base experience.
- Background inference yields to interactive navigation.

A model that makes a lightweight browser permanently consume gigabytes has
failed this specification regardless of benchmark quality.

## 6. Offline behavior

Once installed, a local model pack MUST perform its supported tasks without
network access.

No prompt, page content or embedding is uploaded merely because local
intelligence is enabled.

## 7. Safety

Local models do not gain browser authority.

They produce:

- labels;
- vectors;
- structured suggestions;
- candidate plans.

Execution still goes through explicit application policy.

## 8. Deterministic fallbacks

Every feature that depends on a model MUST define a non-model fallback when
practical:

- lexical history search;
- deterministic intent routing;
- DOM/Reader extraction;
- proportional timeline ticks.

The UI must not become unusable because a model pack is absent or failed to
load.

## 8.1. Current implementation boundary

The shipped product already uses deterministic local semantics: memory documents
receive local hashed embeddings and research comparison uses the same
model-independent local intelligence primitives. This path is exercised by the
core tests and by the product-wiring gate in
`crates/neural-app/tests/spec_product_wiring.rs`.

### Decision: model packs are library-only for now

`ModelPackManager` is **not a NeuralIA product feature** in the current
baseline. It is a `neural-core` filesystem utility that can validate manifests,
verify hashes, stage/replace pack files, list them, record benchmarks and
uninstall them. It performs no download, no backend selection, no automatic
activation and no startup hook.

This is intentional. Wiring model packs into the product would require touching
the browser lifecycle and proving lazy load, zero Home residency, failure
fallback and measured resource budgets. Until that work is explicitly scheduled,
the existence of `ModelPackManager` must not be presented as “model packs
supported by NeuralIA”.

Therefore this specification remains **Parcial**. The deterministic local
intelligence is shipped; optional pack lifecycle is library infrastructure only.
The acceptance criteria below that mention enabling/uninstalling a pack remain
open product criteria, not claims about the current browser.

## 9. Acceptance criteria

1. NeuralIA launches normally with no model installed;
2. enabling a pack does not change Home's network-idle guarantee;
3. local embeddings power SPEC-0100 without sending text remotely;
4. uninstalling a pack leaves ordinary browsing intact;
5. model errors degrade gracefully;
6. memory/latency measurements are recorded before enabling a backend by
   default.
