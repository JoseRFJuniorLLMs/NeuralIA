# SPEC-0010 — Networking

**Status:** Normative

Reader uses a reusable `ureq::Agent` connection pool.

Requests use:
- a NeuralIA user agent;
- bounded global timeout;
- bounded redirects;
- bounded decoded-body size.

Reader accepts HTML and XHTML. Explicit non-HTML responses are rejected.

Speculative prefetch is prohibited in the default build. NeuralIA performs requests only because of visible user action or because the system WebView needs resources for a page the user explicitly opened.
