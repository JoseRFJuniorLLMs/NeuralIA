# SPEC-0013 — Packaging and Release

**Status:** Normative

The stable v1 artifact is a Windows x64 executable from the Rust workspace.

A stable release MUST originate from a successful `main` CI run for the exact source SHA that is tagged. The release workflow creates a new `v<workspace-version>` tag only when that version tag does not already exist.

Release jobs are separated by authority:
- building has read-only repository access plus attestation permissions and operates on the exact CI-tested SHA;
- publishing receives the completed artifact, then creates the new version tag and GitHub Release; it does not execute the Rust build.

A version tag MUST NOT be created until the locked tests, strict Clippy, release build, SBOM generation, checksum preparation and provenance step have succeeded. This prevents orphan stable tags when artifact preparation fails.

A release build MUST:
- use the repository's pinned Rust toolchain;
- consume the committed `Cargo.lock` with `--locked`;
- run core tests and strict Clippy;
- compile `neural-app` in release mode;
- publish `NeuralIA.exe`;
- publish a SHA-256 checksum;
- publish a CycloneDX JSON SBOM for the Windows application;
- publish Cargo metadata;
- generate GitHub build-provenance attestation.

Stable release assets MUST be immutable by policy. If the GitHub Release for a version already exists, automation MUST fail rather than overwrite or clobber its assets.

The repository does not redistribute WebView2 unless a later installer specification explicitly adopts Microsoft's supported bootstrapper/runtime distribution model.

A future signed installer SHOULD provide Authenticode signing, per-user installation, uninstall registration, and protocol/file associations only with explicit user choice.
