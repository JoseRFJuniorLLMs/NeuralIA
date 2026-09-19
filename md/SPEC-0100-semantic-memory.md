# SPEC-0100 — Local Semantic Memory

**Status:** Implemented — NeuralIA 2.0  
**Target:** NeuralIA 1.7

## 1. Purpose

NeuralIA should remember **what the user researched**, not merely which URLs
were visited.

The semantic-memory layer enables questions such as:

> Where was the article about AVX-512 kernel optimization that I read a few
> weeks ago?

without requiring the exact page title, URL or domain.

## 2. Scope

Semantic memory MAY index:

- Reader documents;
- normal browsing history;
- opened source tabs;
- user-submitted research queries;
- AI-provider answers when extraction is available without violating provider
  boundaries;
- user-created notes or saved research-session artifacts.

It MUST NOT index:

- private/incognito browsing;
- passwords, authentication fields or cookies;
- payment-card fields;
- raw session tokens;
- pages explicitly marked by the user as excluded;
- content from a domain excluded by local policy.

## 3. Architecture

```text
Navigation / Reader / AI response
             │
             ▼
        Capture Event
             │
             ▼
      Canonicalization
      URL · title · text
             │
       ┌─────┴─────┐
       ▼           ▼
      FTS        Chunker
                   │
                   ▼
              Embeddings
       ┌───────────┴───────────┐
       ▼                       ▼
 metadata / FTS5        vector index
       └───────────┬───────────┘
                   ▼
                SQLite
```

The semantic store MUST live outside the UI thread.

## 4. Storage

SQLite is the preferred initial store.

Required logical tables:

### documents

- `id`
- `canonical_url`
- `title`
- `source_kind`
- `provider`
- `research_session_id` nullable
- `created_at`
- `last_seen_at`
- `content_hash`
- `private` MUST always be false for persisted records

### chunks

- `id`
- `document_id`
- `ordinal`
- `text`
- `token_or_character_count`
- `section_heading` nullable

### embeddings

- `chunk_id`
- `model_id`
- `dimension`
- vector payload/index representation

SQLite FTS5 SHOULD provide lexical retrieval. A small vector extension such as
sqlite-vec MAY provide ANN/vector search.

DuckDB is not required for v1 of semantic memory. It is better reserved for
later analytical workloads.

## 5. Capture and chunking

The indexer SHOULD prefer already-clean Reader text. Full-Web DOM extraction is
a fallback.

Chunking rules:

- preserve semantic headings where available;
- avoid splitting code blocks mid-block;
- avoid indexing navigation menus and cookie banners;
- deduplicate chunks by normalized content hash;
- cap indexed content per document;
- discard near-empty or boilerplate chunks.

## 6. Retrieval

The default retrieval pipeline SHOULD be hybrid:

```text
query
  │
  ├─► FTS5 / BM25
  │
  ├─► embedding similarity
  │
  └─► recency/project/provider signals
          │
          ▼
        fusion
          │
          ▼
        top K
```

A weighted reciprocal-rank fusion or equivalent deterministic fusion is
preferred over asking an LLM to rank hundreds of candidates.

Results MUST include provenance:

- page title;
- URL/domain;
- timestamp;
- matching excerpt;
- source/provider;
- research session when applicable.

## 7. Privacy

Semantic memory is local-first.

- No semantic history sync is enabled by default.
- Embeddings generated locally MUST remain local.
- If a future remote embedding provider exists, it MUST be opt-in and clearly
  identify which text leaves the machine.
- Clearing history MUST offer an option to clear semantic memory derived from
  that history.
- Private mode bypasses capture before persistence, not by deleting it later.

## 8. Performance requirements

- Home MUST load with no embedding model resident.
- Indexing MUST never block navigation or painting.
- Indexing is queued and bounded.
- Duplicate captures SHOULD be coalesced.
- Search over a normal personal corpus should feel interactive.
- A failed embedding job MUST NOT prevent ordinary history from being written.

Initial engineering targets:

- warm hybrid query over 10k documents: < 200 ms on a reference workstation;
- UI-thread work per capture event: negligible metadata enqueue only;
- no background network caused solely by semantic memory.

## 9. API boundary

`neural-core` SHOULD expose model-independent concepts such as:

```rust
struct MemoryDocument { /* metadata + clean text */ }
struct MemoryQuery { /* query + filters */ }
struct MemoryHit { /* score + provenance + excerpt */ }

trait Embedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, MemoryError>;
}
```

The core must not depend directly on WebView2.

## 10. Acceptance criteria

The feature is ready when:

1. a Reader page can be indexed without UI blocking;
2. a conceptual query can retrieve it without title/domain keywords;
3. every result shows provenance and an excerpt;
4. private browsing leaves no semantic record;
5. clearing semantic history actually removes vectors and text;
6. no local model is loaded on native Home;
7. CI has migration, corruption, deduplication and privacy tests.
