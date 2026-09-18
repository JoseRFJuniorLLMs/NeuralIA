# Changelog

All notable changes to NeuralIA are documented here.

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

### Fixed
- Reader now records the final validated URL after redirects.
- HTTP connection pooling is preserved across Reader navigations.
- Early Content-Length rejection avoids unnecessary oversized downloads.
- Hidden and navigation-only content is filtered more aggressively.
- Reader scoring and limits are character-aware for Unicode content.
- History read errors are no longer silently swallowed.
- Desktop version output now follows the package version.

### Known limitations
- Windows is the only desktop shell in v1.0.
- Startup/RAM budgets still need automated benchmark gates.
- The v1 executable is not Authenticode-signed.
- Native-home screen-reader semantics need a dedicated accessibility pass.
