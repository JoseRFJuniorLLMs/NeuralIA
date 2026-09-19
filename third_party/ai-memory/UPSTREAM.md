# ai-memory upstream snapshot

- Repository: https://github.com/akitaonrails/ai-memory
- Pinned commit: `bbc96c4a93f5dc0473a993a2dab2ab1ac1c33025`
- Imported: 2026-09-19
- License: MIT
- Purpose: implementation reference for SPEC-0107.

## Selected snapshot

- `crates/ai-memory-core/src/sanitize.rs`
- `crates/ai-memory-store/src/fts_query.rs`
- `crates/ai-memory-store/src/retrieval_tuning.rs`
- Additional selected storage/wiki/embedding files may be refreshed in later
  explicit import cycles.

The snapshot is intentionally partial. NeuralIA does not vendor ai-memory's CLI,
HTTP/MCP server, coding-harness hooks, authentication stack, workstreams, web UI
or deployment tooling.

Production code lives in NeuralIA-owned modules. Files under
`third_party/ai-memory/upstream/` are provenance/reference material and are
not compiled into NeuralIA.
