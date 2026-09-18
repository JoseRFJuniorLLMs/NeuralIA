# SPEC-0012 — Testing and CI

**Status:** Normative

Every intent grammar rule, URL policy rule, search URL transformation, Reader extraction behavior, HTML escaping rule, redirect boundary, and history serialization/concurrency path requires automated tests in `neural-core`.

Required CI:
1. Linux: `cargo fmt --check`, core tests, core clippy.
2. Windows: workspace compile, core tests, desktop clippy, release build.
3. Dependency audit: RustSec advisory check.

A change that breaks formatting, Windows compilation, the security tests, or dependency audit is not releasable.
