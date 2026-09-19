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

Windows icon resources, SHA-256 checksums, CycloneDX SBOM, Cargo metadata and provenance are implemented. The CI now also has an opt-in Authenticode path that signs the release candidate before product measurement and fails closed when enabled without valid certificate material; the owner still has to provision the real certificate. The remaining distribution work is the actual per-user installer and explicit update metadata/policy. Dependency policy is enforced in CI with a SHA-pinned `cargo-deny` action: unknown registries/git sources and registry/git wildcard dependencies are denied, licenses are allowlisted explicitly, and duplicate crate versions are surfaced as warnings for cleanup. The release pipeline is CI-gated, SHA-pinned and immutable; build provenance is attested in CI against the exact Windows binary that passed the product gates.

## Phase 5 — Optional platforms

macOS with WKWebView and a native shell. Linux with an explicitly chosen system-WebView strategy. Platform dependencies MUST NOT leak into `neural-core`.

## Forbidden shortcut

Do not solve compatibility complaints by embedding Chromium. That would be another product and should be another repository.
