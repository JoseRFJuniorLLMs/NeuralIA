# SPEC-0006 — Security and Privacy

**Status:** Normative

Defaults:
- HTTP(S) only;
- no embedded URL credentials;
- no site JavaScript in Reader;
- bounded Reader body, redirects, and timeout;
- no NeuralIA cloud account;
- local history only;
- no default remote telemetry.

Remote HTML is untrusted input. Reader output is generated from escaped text blocks, not copied raw markup.

Full Web mode inherits the patch level and sandbox model of the installed system WebView runtime. NeuralIA MUST document that boundary rather than claim its wrapper is a new browser sandbox.

The repository MUST NOT contain secrets, cookies, tokens, private certificates, or user history.
