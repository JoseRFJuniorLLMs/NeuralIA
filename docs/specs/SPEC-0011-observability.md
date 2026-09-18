# SPEC-0011 — Observability

**Status:** Normative

NeuralIA MUST remain diagnosable without becoming telemetry software.

v0.1 permits stderr diagnostics for startup and fatal WebView errors. Future structured logs SHOULD capture event category, duration, and error class but MUST avoid page content, search queries, cookies, authentication data, and Reader body text by default.

A diagnostic build MAY expose timings for startup, Reader fetch, extraction, and render preparation.

The default project has no remote telemetry endpoint.
