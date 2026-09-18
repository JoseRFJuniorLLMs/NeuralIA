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

Reader mode treats remote HTML as untrusted text and does not execute site JavaScript. Redirects are followed manually so each HTTP(S) destination is validated before a new request is made. Public Reader navigation is prevented from pivoting through redirects into obvious loopback, private or link-local network targets.

Full Web mode intentionally delegates web execution and sandboxing to the installed system WebView2 runtime on Windows. External pages are not given NeuralIA's IPC bridge, top-level navigation is restricted to HTTP(S), and sensitive WebView permissions are denied by default.

NeuralIA does not claim that Full Web mode is safer than WebView2 itself.

The omnibox rejects local filesystem paths and unsupported schemes rather than forwarding them to remote AI search.

The repository MUST NOT contain secrets, cookies, tokens, private certificates, or user history.
