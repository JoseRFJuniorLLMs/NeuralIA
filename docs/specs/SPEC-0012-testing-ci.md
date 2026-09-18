# SPEC-0012 — Testing and CI

**Status:** Normative

Every intent grammar rule, URL policy rule, search URL transformation, Reader extraction behavior, HTML escaping rule, redirect boundary, history retention/concurrency path, and Reader HTTP boundary requires automated tests in `neural-core`.

Required CI:
1. Linux: `cargo fmt --check`, locked core tests, locked core clippy.
2. Windows: locked workspace compile, core tests, desktop clippy and release build.
3. Dependency audit: RustSec against the committed `Cargo.lock`.
4. HTTP integration fixtures: redirect success/limit, declared and streamed size limits, charset, content type and navigation-wide deadline.
5. Windows native-Home regression measurement: startup-to-window, working set, thread count and binary size, with a JSON artifact.

CI and release workflows MUST use SHA-pinned third-party Actions. Normal CI MUST NOT regenerate `Cargo.lock`.

A change that breaks formatting, Windows compilation, security tests, integration tests or dependency audit is not releasable.

A stable release MUST be derived only from a successful `main` CI run for the exact source SHA being tagged.
