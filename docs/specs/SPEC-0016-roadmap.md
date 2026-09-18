# SPEC-0016 — Roadmap

**Status:** Normative planning document

## Phase 1 — Foundation (v0.1)

Rust workspace, intent engine, Google AI routing, Reader, safe renderer, local history, one system WebView, CI, logo, and specifications.

## Phase 2 — Actually light

Replace the WebView-rendered home shell with native Windows controls. Create WebView2 lazily. Measure cold start, idle RSS, and first-query latency. Enforce SPEC-0008 with benchmarks.

## Phase 3 — Reader quality

Improve Readability scoring, preserve safe links/images, join article pagination when explicit, improve code blocks, add print/export, and build fixtures from news/docs/blog sites.

## Phase 4 — Distribution

Windows icon resources based on the official logo, signed installer, update metadata, checksums, SBOM, cargo-deny/audit, and hardened release workflow.

## Phase 5 — Optional platforms

macOS with WKWebView and a native shell. Linux with an explicitly chosen system-WebView strategy. Platform dependencies MUST NOT leak into `neural-core`.

## Forbidden shortcut

Do not solve compatibility complaints by embedding Chromium. That would be another product and should be another repository.
