# SPEC-0010 — Networking

**Status:** Normative

Reader owns reusable `ureq::Agent` connection pools.

Requests use:
- a NeuralIA user agent containing the package version;
- operating-system TLS certificate verification;
- proxies disabled for Reader;
- one deadline spanning the complete navigation and redirect chain;
- at most five redirects;
- manual redirect handling so every `Location` is validated before the next request;
- DNS resolution filtering for public Reader navigation;
- an early `Content-Length` rejection when the declared response is already above the Reader budget;
- a bounded decoded-body size;
- charset decoding when declared by the response.

Public Reader resolution MUST reject loopback, private, link-local and reserved IP ranges before the connector opens a socket. Obvious direct local destinations use a separate local path only when the user explicitly requested them.

Reader accepts `text/html` and `application/xhtml+xml`. Explicit non-HTML responses are rejected using parsed media type, not substring matching.

The final validated redirect URL becomes the canonical Reader source URL.

Speculative prefetch is prohibited in the default build.
