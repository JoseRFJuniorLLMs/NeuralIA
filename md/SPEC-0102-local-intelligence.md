# SPEC-0102 — Optional Local Intelligence

**Status:** Parcial — inteligência local determinística integrada; lifecycle de model packs ligado ao produto de forma lazy, backend de inferência ainda não integrado  
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

### Decision: explicit product lifecycle, inference remains absent

The product now has a **product-side lazy adapter** for `ModelPackManager`.
`App::new` asks the `PrivacyGuard` for the declared `MODEL_PACKS_STORE`
(`Explicit`) grant, and the adapter keeps that capability plus an empty manager
slot. Requesting the grant performs no pack I/O: there is **no inference-backend construction**, no model-byte allocation and no pack filesystem access during
Home startup. `ModelPackManager` is created lazily from the grant-rooted path on
the first explicit model action. The CI startup E2E launches the full `App`
with an isolated data directory before any model command and requires
`resident_model_bytes=0`, `manager_initialized=false`, and no `model-packs/`
directory. This proves the shipped App path has not initialized or loaded a
model at startup; it is not a claim about unrelated process working-set memory.

Lifecycle actions are reachable only from explicit omnibox commands:

- `model:status` / `modelo:status` resolves the active pack and reports either
  the verified pack or deterministic fallback plus a diagnostic;
- `model:install:<manifest.json>` imports a user-selected local manifest and
  its sibling model file, validates path/hash/license/capabilities, and does
  **not** activate it;
- `model:activate:<id>` requests activation;
- `model:deactivate` removes active selection without deleting the pack;
- `model:uninstall:<id>` removes the pack and clears active state first.

There is no automatic download or activation. Import rejects a manifest whose
model filename can escape the selected bundle before reading that model path.
The lifecycle adapter has no network, WebView, browser-agent or permission
capability. It cannot navigate, click, execute page script or grant tools.

Activation remains deliberately stricter than installation. The installed
artifact must verify and the benchmark record must contain backend identity,
sample count, embedding dimension, measured latency and
**resident-model-byte evidence**. Older benchmark JSON without residency
evidence fails closed.
A backend-specific benchmark harness can record zero resident model bytes when
zero is the measured value, but absence of the measurement is not treated as
zero.

If active state is missing, stale, corrupted, hash-invalid or paired with
invalid benchmark evidence, resolution returns the existing deterministic
fallback plus an observable warning rather than disabling navigation, memory
or ordinary search. Uninstalling/deactivating a pack immediately returns
resolution to that fallback.

What is **not** implemented yet is equally important: there is still no local
inference backend consuming the pack, no automatic pack download, and no pack
becoming the browser/agent authority. Actual local-intelligence tasks therefore
continue to use the deterministic implementation. A future backend must supply
its measured residency/latency evidence before it can become default and must
retain the no-network/no-browser-authority boundary.

Therefore this specification remains **Parcial**: deterministic local
intelligence plus the explicit lazy lifecycle boundary are shipped; real pack
inference, idle backend unload and backend-specific resource budgets remain
future work.

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
passage in the pack, and of every passage inside its source's sanitized text),
`duplicates()` (kept/dropped provenance and similarity) and
`dropped()`, and cannot be built or mutated from outside (`compile_fail`
doctests). The whole prompt fits `BudgetSpec::available()`: the fence
instructions (`ContextPack::fence_instructions()`: `CONTEXT_DATA_PREAMBLE_PT`
and the nonce notice, one message) plus the user message
(`ContextPack::user_message()`: the question, a blank line and the pack), with
the nonce counted at its heaviest; task instructions, if any, belong to
`reserve_output`. The pipeline: sanitize through `untrusted` (a remote
destination goes through the existing `agent_security::redact_sensitive_text`
and `redact_url`: the source URL, every URL inside the text that carries a
credential — a credential parameter such as `sig=` or `code=`, a fragment with
one, or `user:pass@` — and the source label, which goes out in the header; a
URL without a credential stays byte for byte; a private source never goes to a
remote destination — «Modo privado: este conteúdo não pode sair do
computador.»), chunk on sentence boundaries (`Dr. Silva` and `3.5 GHz` stay
whole; the CJK terminals `。！？` close a sentence without a following space),
finding each cut by exponential search and bisection (O(n log n) bytes
measured), dedupe in score order so the kept copy is the highest-scoring one
(exact SHA-256, then SimHash filtered and Jaccard ≥ 0.8 confirmed, both from
`untrusted::Shingles`; across different sources, near copies that disagree on
numbers or negations are both kept), score 0.5 BM25-lite + 0.4 `Embedder`
cosine + 0.1 priority, per-source floors (round-robin, with what the other
sources still need reserved: a whole chunk only goes in when it leaves that
room, otherwise a cut of it — its best sentences, or its beginning; when all
floors and headers fit, each source gets at least min(floor, its text),
otherwise the largest common floor that fits), greedy allocation with
extractive compression, and the `untrusted` nonce fence, where a passage line
that starts with an opening square bracket or a look-alike (`[2] resposta:`,
`［3］`) gets a leading `\` so page text cannot pose as another source's
header. `estimate_tokens` is the larger of a per-character-class table × 1.10
and the pre-token count of the published o200k and Llama 3 split regexes
(simulated by hand; +4 per message). Each regex's pre-token count is a hard
lower bound of that tokenizer's tokens, which covers what the table alone
under-counted (alternating letters and digits, hex ids, case switches, short
lines); the estimate is at or above the fixture, but it is not a guarantee
against the real tokenizers: BPE can split more inside a pre-token (long base64
or random ids), which is what `TokenCalibration` corrects. `TokenCalibration` is
an EWMA of real/estimated clamped to [0.6, 2.0]; `fit_conversation` drops the
oldest turns and never starts on an orphan answer; `split_for_map_reduce` cuts a
long text into pieces that fit one call at a time, each with the fence
instructions and its own fence (`MAP_FENCE_LABEL`). The `Embedder` trait and the
deterministic `HashingEmbedder` live in `local_intelligence.rs`. Summary line:
«≈ 3 200 tokens · 4 fontes · 2 trechos repetidos removidos».

Gates in `crates/neural-core/src/context_budget/tests.rs` (Windows and Linux):
`budget_is_never_exceeded_over_200_seeded_cases` (the whole prompt, and no
secret in a remote pack's labels, URLs or text),
`estimator_is_conservative_against_the_token_fixture`,
`pretokens_follow_the_published_split_regexes`,
`near_duplicates_are_removed_once_with_provenance`,
`near_duplicates_that_disagree_are_both_kept`,
`the_relevant_passage_survives_the_cut`, `every_source_keeps_its_floor`,
`floors_hold_when_a_whole_best_chunk_would_eat_another_floor`,
`remote_destination_redacts_secrets_and_refuses_private_sources`,
`a_passage_cannot_forge_a_source_header`,
`the_pack_is_deterministic_for_the_same_inputs`,
`the_fence_is_the_untrusted_one_and_spans_point_at_the_passages`,
`compressed_spans_stay_inside_a_source_the_fence_rewrote`,
`cjk_text_is_chunked_at_sentence_ends_and_compressible`,
`chunking_matches_the_unit_by_unit_greedy`,
`chunking_work_is_n_log_n_on_megabyte_texts`,
`split_for_map_reduce_pieces_fit_and_cover_the_text`, and the `compile_fail`
doctests on `ContextPack`. The first rows of the token fixture
(`context_budget/token_fixture.tsv`) were derived by hand from the o200k and
Llama 3 pre-tokenization rules and rounded up, not measured with a tokenizer;
the review rows carry the pre-token counts of the published regexes (a hard
lower bound); the gate only requires the estimator to be at or above both
columns.

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
