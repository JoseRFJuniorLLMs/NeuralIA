# SPEC-0016 — Roadmap

**Status:** Normative planning document

## Phase 1 — Foundation (complete)

Rust workspace, intent engine, Google AI routing, Reader, safe renderer, bounded local history, logo, CI, and specifications.

## Phase 2 — Actually light (implemented, core gates active)

Native Windows Home, native Windows omnibox, zero **surface** WebViews while
idle, lazy WebView creation, bounded comparator lifecycle, authenticated
closed-schema WebView2 IPC, coalescing Reader worker, background history writer,
and a size-oriented release profile are implemented. A previously authenticated
Google session may keep one hidden Gmail monitor alive by design; lifecycle CI
sets `NEURALIA_NO_GMAIL=1` so that exception cannot masquerade as a leak.

CI now enforces the native-Home startup/RSS/thread/idle-CPU budget and repeated
comparator → Home WebView lifecycle. Broader first-query and Reader-latency
measurement remain performance follow-up under SPEC-0008 rather than an
unmeasured claim about this phase.

## Phase 3 — Reader quality

Improve Readability scoring, preserve selected safe links/images, join article pagination only when explicit, add print/export, and expand fixtures from diverse news/docs/blog sites.

## Phase 4 — Distribution

Windows icon resources, Authenticode-signed installer, update metadata, checksums, SBOM, and stronger dependency-policy tooling. The release workflow is already CI-gated, SHA-pinned, immutable and provenance-attested.

## Phase 5 — Optional platforms

macOS with WKWebView and a native shell. Linux with an explicitly chosen system-WebView strategy. Platform dependencies MUST NOT leak into `neural-core`.

## Forbidden shortcut

Do not solve compatibility complaints by embedding Chromium. That would be another product and should be another repository.
