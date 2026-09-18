# Changelog

All notable changes to NeuralIA are documented here.

## [1.0.1] - 2026-09-18

Security and reliability hardening release after the second recursive audit.

### Security
- Public Reader DNS resolution filters loopback, private, link-local, documentation, benchmark, multicast and reserved IP ranges before connection.
- Reader uses the operating-system TLS certificate verifier.
- The full Reader redirect chain shares one hard deadline.
- Reader-generated pages contain no NeuralIA JavaScript and use `script-src 'none'`.
- External pages still receive no NeuralIA IPC.
- Stable release assets cannot be overwritten by later `main` pushes.
- Release publication only follows a successful `main` CI run.
- GitHub Actions used by CI/release are pinned to commit SHAs and build jobs do not receive release-write credentials.

### Fixed
- Replaced unbounded Reader thread spawning with one coalescing Reader worker.
- Bounded local history to the configured retention limit.
- Moved history writes off the UI thread and added a clear-history action.
- Preserved whitespace in `<pre>` code blocks.
- Rendered list items as semantic HTML lists.
- Added explicit handling for `target="_blank"`/new-window requests while retaining the one-WebView invariant.
- Replaced the painted omnibox editor with a native Windows `EDIT` control.
- CI now tests exactly the committed `Cargo.lock` with `--locked`.

### Testing
- Added end-to-end local HTTP tests for redirects, redirect limits, body limits, charset decoding, non-HTML responses and total deadline behavior.
- Added additional security and history retention tests.

### Distribution
- Release workflow emits SHA-256, Cargo metadata and GitHub build-provenance attestation.
- Stable release publication refuses to overwrite an existing release.

### Remaining
- Automated startup/RSS performance gates.
- Authenticode signing and installer.
- Optional macOS/Linux shells.

## [1.0.0] - 2026-09-18

First stable release.

### Added
- Native Windows home surface with lazy WebView2 creation.
- Google AI Mode routing for ordinary questions.
- HTTP Reader with bounded body, timeout and semantic article extraction.
- Explicit Reader and Full Web modes.
- Local append-only history.
- Windows x64 release workflow with SHA-256 artifact.
- RustSec dependency audit in CI.

### Security
- Unsupported schemes and local filesystem paths are rejected instead of being sent to remote AI search.
- Embedded URL credentials are rejected.
- Reader redirects are followed manually and validated before each request.
- Public Reader navigation cannot redirect into obvious loopback/private/link-local targets.
- External Web pages do not receive the NeuralIA IPC bridge.
- Sensitive WebView permissions are denied by default.
- Reader HTML is escaped and protected by a restrictive CSP.
- History writes use file locking and durable flushes to avoid interleaved records.
