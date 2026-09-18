# SPEC-0016 — Roadmap

**Status:** Normative planning document

## Phase 1 — Foundation (complete)

Rust workspace, intent engine, Google AI routing, Reader, safe renderer, local history, logo, CI, and specifications.

## Phase 2 — Actually light (implemented, measurement pending)

Native Windows home/omnibox, zero WebView instances while idle, lazy WebView creation, one-WebView ceiling, IPC isolation, navigation-generation cancellation, and size-oriented release profile.

Remaining work in this phase is empirical: measure cold start, idle RSS, first-query latency, and Reader latency and enforce SPEC-0008 with benchmark gates.

## Phase 3 — Reader quality

Improve Readability scoring, preserve safe links/images, join article pagination when explicit, improve code blocks, add print/export, and build fixtures from diverse news/docs/blog sites.

## Phase 4 — Distribution

Windows icon resources based on the official logo, signed installer, update metadata, checksums, SBOM, cargo-deny/audit, and hardened release workflow.

## Phase 5 — Optional platforms

macOS with WKWebView and a native shell. Linux with an explicitly chosen system-WebView strategy. Platform dependencies MUST NOT leak into `neural-core`.

## Forbidden shortcut

Do not solve compatibility complaints by embedding Chromium. That would be another product and should be another repository.
