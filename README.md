<p align="center"><img src="assets/neuralia-logo.svg" width="150" alt="NeuralIA logo"></p>
<h1 align="center">NeuralIA</h1>
<p align="center"><strong>The browser without the browser.</strong></p>
<p align="center">Search with AI. Read the Web. Open a full page only when you actually need it.</p>

## What NeuralIA is

NeuralIA is an experimental **AI-first, reader-first, system-WebView browser** written in Rust.

It deliberately refuses the usual browser arms race. It does not ship Chromium, does not implement its own JavaScript engine, does not carry a local LLM, and does not try to become an operating system with tabs.

```text
question ───────► Google AI Mode
URL ────────────► Reader
web:<URL> ──────► full system WebView
```

On Windows, the full-web path uses the installed Microsoft Edge WebView2 runtime through WRY. Search uses Google AI Mode and therefore does not require a paid AI API key.

## Design rules

1. One system WebView, never a bundled browser engine.
2. No local LLM in the default build.
3. No custom JavaScript engine.
4. No custom CSS compatibility project.
5. URLs open in Reader by default.
6. Full Web is an escape hatch, not the product center.
7. Performance budgets are architecture requirements.

## Workspace

```text
NeuralIA/
├── crates/neural-core/
├── crates/neural-app/
├── assets/neuralia-logo.svg
├── docs/specs/
└── .github/workflows/
```

## Input grammar

| Input | Result |
|---|---|
| `como funciona Raft?` | Google AI Mode |
| `? MVCC vs OCC` | Google AI Mode |
| `https://example.com/article` | Reader |
| `reader:https://example.com` | Reader |
| `web:https://example.com` | Full WebView |
| `home:` | NeuralIA home |

## Build

```bash
cargo test -p neural-core
```

Windows:

```powershell
cargo run -p neural-app --release
```

## Status

- [x] Rust workspace
- [x] intent parser
- [x] Google AI Mode routing
- [x] bounded HTTP Reader
- [x] semantic article extraction
- [x] safe Reader HTML
- [x] local append-only history
- [x] Windows system-WebView shell
- [x] CI Linux + Windows
- [x] engineering specifications
- [ ] native idle shell with lazy WebView
- [ ] signed installer
- [ ] measured startup/RAM gates

See [the specification index](docs/specs/README.md).

## Non-goals

NeuralIA is not building V8, Blink, an extension ecosystem, or another bundled Chromium distribution. Those are excellent ways to turn a small repository into a hereditary obligation.

## License

MIT.
