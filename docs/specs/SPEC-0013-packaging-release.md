# SPEC-0013 — Packaging and Release

**Status:** Normative

The stable v1 artifact is a Windows x64 executable from the Rust workspace.

A release build MUST:
- use the repository's pinned Rust toolchain;
- compile `neural-app` in release mode;
- publish `NeuralIA.exe`;
- publish a SHA-256 checksum;
- attach both files to the GitHub Release created from a `v*` tag.

The repository does not redistribute WebView2 unless a later installer specification explicitly adopts Microsoft's supported bootstrapper/runtime distribution model.

A future signed installer SHOULD provide Authenticode signing, per-user installation, uninstall registration, SBOM, and protocol/file associations only with explicit user choice.

Tags matching `v*` trigger the release-build workflow.
