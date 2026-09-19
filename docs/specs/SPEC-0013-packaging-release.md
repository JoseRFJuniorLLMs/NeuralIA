# SPEC-0013 — Packaging and Release

**Status:** Normative

The stable v1 artifact is a Windows x64 executable from the Rust workspace.

A stable release MUST originate from a successful `main` CI run for the exact source SHA that is tagged. The release workflow creates a new `v<workspace-version>` tag only when that version tag does not already exist.

Release jobs are separated by authority:
- CI builds `NeuralIA.exe`; when repository variable `NEURALIA_AUTHENTICODE_ENABLED=true`, the main-branch Windows job signs that exact executable before product measurement using the configured PFX secret, requires a valid timestamped signature, then runs the Home/lifecycle gates against the signed file. With signing disabled, the same path measures the unsigned executable;
- the Windows job records the SHA-256 of the post-signing candidate and uploads it as a short-lived artifact named for the tested commit SHA;
- after the Windows gate, a separate main-only CI job downloads that immutable artifact and creates the GitHub build-provenance attestation for the exact executable that was measured;
- the release packaging job has read-only repository and Actions access, downloads the artifact from the exact CI run that triggered it and verifies the recorded checksum. That checksum proves integrity of the downloaded artifact against the digest recorded by the producing CI job; it is not provenance by itself. Run identity plus GitHub build-provenance attestation bind the artifact to the producing workflow and source SHA;
- publishing receives the packaged artifact, then creates the new version tag and GitHub Release; it does not execute the Rust build.

**The release workflow MUST NOT rebuild or post-process `NeuralIA.exe`.** A
second build from the same source is not the binary that passed the
performance/lifecycle gates, and signing it after those gates would change its
bytes. If Authenticode is enabled, signing therefore happens in the Windows CI
job *before* Home/lifecycle measurement, hashing and attestation. The stable
executable is byte-for-byte the CI candidate measured before publication.

A version tag MUST NOT be created until the locked tests, strict Clippy, CI release build, performance/lifecycle gates, artifact checksum verification, SBOM generation, checksum preparation and provenance step have succeeded. This prevents orphan stable tags when artifact preparation fails.

A release candidate MUST:
- use the repository's pinned Rust toolchain;
- consume the committed `Cargo.lock` with `--locked`;
- run core/application tests and strict Clippy in CI;
- compile `neural-app` in release mode in CI;
- when Authenticode is enabled, fail closed on missing/invalid certificate material, sign with SHA-256, require a timestamp, and verify the resulting signature before product measurement;
- pass the Home and WebView lifecycle gates as that exact post-signing executable;
- publish that same `NeuralIA.exe` without rebuilding, re-signing or otherwise modifying it;
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

The Authenticode executable-signing path is implemented but opt-in: the repository owner must provide the signing certificate and explicitly enable it. The certificate itself is not stored in the repository. A future Windows installer SHOULD preserve this sign-before-measure invariant and provide per-user installation, uninstall registration, and protocol/file associations only with explicit user choice.
