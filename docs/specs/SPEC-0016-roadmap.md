# SPEC-0016 — Roadmap

**Status:** Normative planning document

## Phase 1 — Foundation (complete)

Rust workspace, intent engine, Google AI routing, Reader, safe renderer, bounded local history, logo, CI, and specifications.

## Phase 2 — Actually light (implemented, measurement pending)

Native Windows home, native Windows omnibox, zero WebView instances while idle, lazy WebView creation, one-WebView ceiling, no external IPC, coalescing Reader worker, background history writer, and size-oriented release profile.

Remaining work in this phase is empirical: measure cold start, idle RSS, first-query latency, and Reader latency and enforce SPEC-0008 with benchmark gates.

## Phase 3 — Reader quality

Improve Readability scoring, preserve selected safe links/images, join article pagination only when explicit, add print/export, and expand fixtures from diverse news/docs/blog sites.

## Phase 4 — Distribution

Windows icon resources, Authenticode-signed installer, update metadata, checksums, SBOM, and stronger dependency-policy tooling. The release workflow is already CI-gated, SHA-pinned, immutable and provenance-attested.

## Phase 5 — Optional platforms

macOS with WKWebView and a native shell. Linux with an explicitly chosen system-WebView strategy. Platform dependencies MUST NOT leak into `neural-core`.

## Forbidden shortcut

Do not solve compatibility complaints by embedding Chromium. That would be another product and should be another repository.
