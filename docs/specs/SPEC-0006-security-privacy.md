# SPEC-0006 — Security and Privacy

**Status:** Normative

Defaults:
- HTTP(S) only;
- no embedded URL credentials;
- local filesystem paths are rejected and MUST NOT become remote search queries;
- no site JavaScript in Reader;
- bounded Reader body, redirects, and timeout;
- every Reader redirect is validated before the next request;
- a public Reader URL MUST NOT redirect into obvious loopback/private/link-local targets;
- no NeuralIA cloud account;
- local history only;
- no default remote telemetry.

Remote HTML is untrusted input. Reader output is generated from escaped text blocks, not copied raw markup, and is served with a restrictive Content Security Policy.

Full Web mode inherits the patch level and sandbox model of the installed system WebView runtime. External pages MUST NOT receive NeuralIA's IPC object. Sensitive WebView permissions are denied by default, and top-level navigation is restricted to HTTP(S) plus the internal controlled return-to-home navigation.

The repository MUST NOT contain secrets, cookies, tokens, private certificates, or user history.
