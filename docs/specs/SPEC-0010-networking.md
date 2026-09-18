# SPEC-0010 — Networking

**Status:** Normative

Reader uses a reusable `ureq::Agent` connection pool owned by the application and cloned into worker threads.

Requests use:
- a NeuralIA user agent containing the package version;
- bounded global timeout;
- at most five redirects;
- manual redirect handling so every `Location` is validated before the next network request;
- an early `Content-Length` rejection when the declared response is already above the Reader budget;
- a bounded decoded-body size;
- charset decoding when declared by the response.

Reader accepts HTML and XHTML. Explicit non-HTML responses are rejected.

The final validated redirect URL becomes the canonical Reader source URL.

Speculative prefetch is prohibited in the default build. NeuralIA performs requests only because of visible user action or because the system WebView needs resources for a page the user explicitly opened.
