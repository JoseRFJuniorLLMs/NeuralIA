# SPEC-0012 — Testing and CI

**Status:** Normative

Every intent grammar rule, URL policy rule, search URL transformation, Reader extraction behavior, HTML escaping rule, and history serialization path requires automated tests in `neural-core`.

Required CI:
1. Linux: format, core tests, core clippy.
2. Windows: workspace compile, core tests, desktop clippy.

A change that breaks Windows compilation is not releasable even when the portable core passes.
