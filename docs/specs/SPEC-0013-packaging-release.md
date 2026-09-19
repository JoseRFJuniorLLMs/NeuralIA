# SPEC-0013 — Packaging and Release

**Status:** Normative

The stable v1 artifact is a Windows x64 executable from the Rust workspace.

A stable release MUST originate from a successful `main` CI run for the exact source SHA that is tagged. The release workflow creates a new `v<workspace-version>` tag only when that version tag does not already exist.

Release jobs are separated by authority:
- CI builds `NeuralIA.exe`, runs the Windows tests plus Home/lifecycle gates against that exact file, records its SHA-256 and uploads it as a short-lived artifact named for the tested commit SHA;
- after the Windows gate, a separate main-only CI job downloads that immutable artifact and creates the GitHub build-provenance attestation for the exact executable that was measured;
- the release packaging job has read-only repository and Actions access, downloads the artifact from the exact CI run that triggered it and verifies the recorded checksum;
- publishing receives the packaged artifact, then creates the new version tag and GitHub Release; it does not execute the Rust build.

**The release workflow MUST NOT rebuild `NeuralIA.exe`.** A second build from the
same source is not the binary that passed the performance/lifecycle gates. The
stable executable is byte-for-byte the CI candidate measured before publication.

A version tag MUST NOT be created until the locked tests, strict Clippy, CI release build, performance/lifecycle gates, artifact checksum verification, SBOM generation, checksum preparation and provenance step have succeeded. This prevents orphan stable tags when artifact preparation fails.

A release candidate MUST:
- use the repository's pinned Rust toolchain;
- consume the committed `Cargo.lock` with `--locked`;
- run core/application tests and strict Clippy in CI;
- compile `neural-app` in release mode in CI;
- pass the Home and WebView lifecycle gates as that exact executable;
- publish that same `NeuralIA.exe` without rebuilding it;
- publish a SHA-256 checksum;
- publish a CycloneDX JSON SBOM for the Windows application;
- publish Cargo metadata;
- publish the Home and WebView lifecycle measurement JSON produced against the released executable;
- generate GitHub build-provenance attestation in CI for the same tested executable, before release packaging.

Stable release assets MUST be immutable by policy. When both the version tag
and its GitHub Release already exist, a later main push with the same workspace
version is a no-op. If only one of tag/release exists, automation MUST fail
closed rather than repair, overwrite or clobber stable state.

The repository does not redistribute WebView2 unless a later installer specification explicitly adopts Microsoft's supported bootstrapper/runtime distribution model.

A future signed installer SHOULD provide Authenticode signing, per-user installation, uninstall registration, and protocol/file associations only with explicit user choice.
