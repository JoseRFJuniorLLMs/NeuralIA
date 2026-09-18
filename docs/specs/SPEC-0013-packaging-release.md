# SPEC-0013 — Packaging and Release

**Status:** Normative

The first release artifact is a Windows x64 executable from the Rust workspace.

The repository does not redistribute WebView2 unless a later installer specification explicitly adopts Microsoft's supported bootstrapper/runtime distribution model.

A future signed installer SHOULD provide Authenticode signing, per-user installation, uninstall registration, checksums, SBOM, and protocol/file associations only with explicit user choice.

Tags matching `v*` trigger the release-build workflow.
