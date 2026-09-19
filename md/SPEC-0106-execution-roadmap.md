# SPEC-0106 — NeuralIA Execution Roadmap

**Status:** Completed baseline — NeuralIA 2.0

## Objective

Evolve NeuralIA toward semantic and agentic navigation without sacrificing the
properties that make it distinct: native shell, low idle cost, system WebView,
local-first privacy and explicit security boundaries.

## Phase A — 1.7 Semantic Memory

Implement SPEC-0100 first.

Deliverables:

- SQLite semantic store;
- FTS5 lexical retrieval;
- model-independent embedding interface;
- optional local embedding pack;
- conceptual Ctrl+H/search surface;
- privacy exclusions;
- migration and clear-history semantics.

Release gate:

- Home still loads with zero model memory;
- private browsing produces zero semantic records;
- indexing stays off the UI thread.

## Phase B — 1.8 Research Sessions

Implement SPEC-0101.

Deliverables:

- persistent session entity;
- provider/source relationships;
- multi-source selection;
- structured comparison;
- Markdown export;
- provenance.

Release gate:

- closing a visual tab does not lose research;
- synthesis identifies exact source set;
- no generic dashboard dependency.

## Phase C — 1.9 Local Intelligence + Semantic Timeline

Implement SPEC-0102 and SPEC-0103.

Deliverables:

- optional model-pack manager;
- local embeddings/classification;
- bounded local summaries;
- semantic timeline anchors;
- marker-based navigation/auto-scroll.

Release gate:

- base installer works without models;
- model loading is lazy;
- timeline has deterministic fallback;
- no background remote inference by default.

## Phase D — 2.0 Agent Security

Implement SPEC-0104 **before** autonomous execution.

Deliverables:

- action capability classes;
- native permission engine;
- sensitive-data filtering;
- origin policy;
- adversarial injection fixtures;
- local audit log;
- kill switch.

No web agent ships before this gate is green.

## Phase E — 2.0 Agent Runtime

Implement SPEC-0105.

Initial supported workflows should be deliberately boring:

1. search;
2. filter;
3. inspect results;
4. extract structured data;
5. compare;
6. pause before sensitive commitment.

Do not start with airline purchasing, banking or account administration.

Those workflows combine every hard problem at once and are poor first tests.

## Explicitly deferred

- embedded Chromium/CEF;
- Electron rewrite;
- browser-extension ecosystem;
- cloud profile sync;
- always-running background agent;
- autonomous payment;
- CAPTCHA bypass;
- unrestricted filesystem access;
- unrestricted arbitrary JavaScript tools;
- general 3B+ local chat model in the base installer.

## Success criterion

NeuralIA 2.0 should be able to:

> understand a research goal, remember relevant prior work, organize multiple
> AI answers and sources, navigate the Web with bounded tools, and stop for the
> human whenever authority or risk increases.

That is a meaningful agentic browser.

Calling a sidebar chatbot an operating system is not.
