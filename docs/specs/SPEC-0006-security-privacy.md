# SPEC-0006 — Security and Privacy

**Status:** Normative

Defaults:
- HTTP(S) only;
- no embedded URL credentials;
- local filesystem paths are rejected and MUST NOT become remote search queries;
- no site JavaScript in Reader;
- bounded Reader body, redirects, deadline and concurrency;
- every Reader redirect is validated before the next request;
- public Reader DNS resolution rejects loopback/private/link-local/reserved IP destinations;
- operating-system TLS certificate verification;
- no NeuralIA cloud account;
- bounded local history only;
- no default remote telemetry.

Remote HTML is untrusted input. Reader output is generated from escaped text blocks, not copied raw markup, and is served with a Content Security Policy that forbids scripts, connections, frames and objects.

Full Web mode inherits the patch level and sandbox model of the installed system WebView runtime. External pages MUST NOT receive NeuralIA IPC. New sensitive WebView permission requests are denied by default, and top-level navigation is restricted to HTTP(S) plus the internal controlled return-to-home navigation.

Local history writes MUST occur off the UI thread and retention MUST be bounded. The user MUST have a local clear-history action.

The repository MUST NOT contain secrets, cookies, tokens, private certificates, or user history.
