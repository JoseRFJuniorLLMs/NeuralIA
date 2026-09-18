# SPEC-0011 — Observability

**Status:** Normative

NeuralIA MUST remain diagnosable without becoming telemetry software.

The default build permits local stderr diagnostics for startup and fatal WebView errors. Diagnostics MUST avoid page content, search queries, cookies, authentication data, Reader body text, and other user content by default.

A diagnostic build MAY expose timings for startup, Reader fetch, extraction, render preparation, queue wait, and WebView creation.

Release engineering MAY emit machine-readable build metadata, SBOMs, checksums and provenance. Those artifacts describe the software build and dependencies; they MUST NOT include user browsing data.

The default project has no remote telemetry endpoint.
