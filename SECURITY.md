# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| 1.x | Yes |
| < 1.0 | No |

Security fixes target the current `main` branch and the latest stable 1.x release.

## Reporting

Please do not publish credentials, cookies, tokens, private browsing data, or working exploit details in a public issue.

For a suspected vulnerability, contact the repository owner through GitHub and provide:
- affected commit/version;
- impact;
- minimal reproduction;
- whether Reader mode or Full Web mode is involved.

## Security boundaries

Reader mode treats remote HTML as untrusted text and does not execute site JavaScript. Redirects are followed manually; one navigation-wide deadline covers the complete redirect chain. Public Reader DNS resolution filters loopback, private, link-local and reserved IP ranges before connection, reducing DNS-rebinding/client-side-SSRF exposure.

Explicit direct navigation to obvious local HTTP(S) targets such as `localhost` remains a user action and uses a separate local Reader path. A public Reader URL is not permitted to pivot into an obvious local target through redirects.

TLS uses the operating-system certificate verifier.

Reader output is generated from escaped text blocks, not copied raw markup. The Reader Content Security Policy has `script-src 'none'`, `connect-src 'none'`, `frame-src 'none'` and `object-src 'none'`.

Full Web mode intentionally delegates web execution and sandboxing to the installed system WebView2 runtime on Windows. External pages are not given NeuralIA IPC. New permission requests are denied by default. Top-level navigation is restricted to HTTP(S) plus the internal controlled return-to-home action.

NeuralIA does not claim that Full Web mode is safer than WebView2 itself.

The omnibox rejects local filesystem paths and unsupported schemes rather than forwarding them to remote AI search.

Local history is bounded and can be cleared with Ctrl+Shift+Delete. NeuralIA has no remote history synchronization or telemetry endpoint.

The repository MUST NOT contain secrets, cookies, tokens, private certificates, or user history.
