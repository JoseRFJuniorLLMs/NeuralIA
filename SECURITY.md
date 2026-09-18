# Security Policy

## Supported branch

Security fixes target the current `main` branch until the first stable release.

## Reporting

Please do not publish credentials, cookies, tokens, private browsing data, or working exploit details in a public issue.

For a suspected vulnerability, contact the repository owner through GitHub and provide:
- affected commit/version;
- impact;
- minimal reproduction;
- whether Reader mode or Full Web mode is involved.

## Security boundaries

Reader mode treats remote HTML as untrusted text and does not execute site JavaScript. Full Web mode intentionally delegates web execution and sandboxing to the installed system WebView2 runtime on Windows.

NeuralIA does not claim that Full Web mode is safer than WebView2 itself.
