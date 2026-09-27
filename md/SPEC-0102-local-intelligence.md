# SPEC-0102 — Optional Local Intelligence

**Status:** Parcial — inteligência local determinística integrada; lifecycle de model packs validado no core, ainda não ligado ao produto  
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
baseline. It is a `neural-core` lifecycle utility that validates manifests,
license metadata and hashes, stages/replaces pack files atomically, records
benchmarks, supports explicit activation/deactivation and removes packs.

Activation in the core is deliberately stricter than installation: a pack is
only marked active after the installed artifact verifies and a non-empty
benchmark record exists. The active-state record pins id/version/hash. If that
state becomes stale, the model file is altered or benchmark metadata is
unusable, resolution returns the deterministic fallback plus a diagnostic
warning instead of making ordinary local intelligence unavailable.

It still performs **no download, no inference-backend construction, no
automatic activation and no browser-startup hook**.

This is intentional. Wiring model packs into the product would require touching
the browser lifecycle and proving lazy load, zero Home residency, failure
fallback and measured resource budgets. Until that work is explicitly scheduled,
the existence of `ModelPackManager` must not be presented as “model packs
supported by NeuralIA”.

Therefore this specification remains **Parcial**. The deterministic local
intelligence is shipped; optional pack lifecycle is library infrastructure only.
The acceptance criteria below that mention enabling/uninstalling a pack remain
open product criteria, not claims about the current browser.

## 8.2. Context budget (library-only)

`neural_core::context_budget` decides what enters a prompt before any call is
made, without I/O or network. `build_context(question, sources, spec, embedder)`
takes `ContextSource`s (id `[A-Za-z0-9_-]{1,16}`, label, kind
`Answer|Page|Reader|Pdf|Epub|Transcript|Note|Memory|Selection|ChatTurn|Tab`,
locator `Block|PdfPage|Epub{spine,block}|Time(secs)`, url, text, priority,
private flag) and a `BudgetSpec` (`max_input`, `reserve_output`, per-source
floor, ~160-token chunks, `Destination::Local` or `Destination::Remote { host }`)
and returns a `ContextPack` whose fields are private: it is read through
`rendered()`, `est_tokens()`, `sources()` (byte spans of every header and
passage), `duplicates()` (kept/dropped provenance and similarity) and
`dropped()`, and cannot be built or mutated from outside (`compile_fail`
doctests). The pipeline: sanitize through `untrusted` (a remote destination goes
through the existing `agent_security::redact_sensitive_text` and `redact_url`;
a private source never goes to a remote destination —
«Modo privado: este conteúdo não pode sair do computador.»), chunk on sentence
boundaries (`Dr. Silva` and `3.5 GHz` stay whole), dedupe (exact SHA-256, then
SimHash filtered and Jaccard ≥ 0.8 confirmed, both from `untrusted::Shingles`),
score 0.5 BM25-lite + 0.4 `Embedder` cosine + 0.1 priority, per-source floors,
greedy allocation with extractive compression, and the `untrusted` nonce fence.
`estimate_tokens` is a per-character-class table × 1.10 (+4 per message);
`TokenCalibration` is an EWMA of real/estimated clamped to [0.6, 2.0];
`fit_conversation` drops the oldest turns and never starts on an orphan answer;
`split_for_map_reduce` cuts a long text into pieces that fit one at a time. The
`Embedder` trait and the deterministic `HashingEmbedder` live in
`local_intelligence.rs`. Summary line: «≈ 3 200 tokens · 4 fontes · 2 trechos
repetidos removidos».

Gates in `crates/neural-core/src/context_budget/tests.rs` (Windows and Linux):
`budget_is_never_exceeded_over_200_seeded_cases`,
`estimator_is_conservative_against_the_token_fixture`,
`near_duplicates_are_removed_once_with_provenance`,
`the_relevant_passage_survives_the_cut`, `every_source_keeps_its_floor`,
`remote_destination_redacts_secrets_and_refuses_private_sources`,
`the_pack_is_deterministic_for_the_same_inputs`,
`the_fence_is_the_untrusted_one_and_spans_point_at_the_passages`, and the
`compile_fail` doctests on `ContextPack`. The token fixture
(`context_budget/token_fixture.tsv`) was derived by hand from the o200k and
Llama 3 pre-tokenization rules and rounded up, not measured with a tokenizer;
the gate only requires the estimator to be at or above both columns.

`neural_core::context_budget` is **not wired into the product yet**: no file
under `crates/neural-app/src` calls `build_context` (gate
`context_budget_is_core_library_only_until_consensus_wires_it` in
`crates/neural-app/tests/spec_product_wiring.rs`). The 2.5 consensus work is
its first caller; whoever wires it updates this section and that gate.

## 9. Acceptance criteria

1. NeuralIA launches normally with no model installed;
2. enabling a pack does not change Home's network-idle guarantee;
3. local embeddings power SPEC-0100 without sending text remotely;
4. uninstalling a pack leaves ordinary browsing intact;
5. model errors degrade gracefully;
6. memory/latency measurements are recorded before enabling a backend by
   default.
