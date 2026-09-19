# SPEC-0013 — Packaging and Release

**Status:** Normative

The stable v1 artifact is a Windows x64 executable from the Rust workspace.

A stable release MUST originate from a successful `main` CI run for the exact source SHA that is tagged. The release workflow creates a new `v<workspace-version>` tag only when that version tag does not already exist.

Release jobs are separated by authority:
- CI builds `NeuralIA.exe`, runs the Windows tests plus Home/lifecycle gates against that exact file, records its SHA-256 and uploads it as a short-lived artifact named for the tested commit SHA;
- after the Windows gate, a separate main-only CI job downloads that immutable artifact and creates the GitHub build-provenance attestation for the exact executable that was measured;
- the release packaging job has read-only repository and Actions access, downloads the artifact from the exact CI run that triggered it and verifies the recorded checksum;
- publishing receives the packaged artifact, then creates the new version tag and GitHub Release; it does not execute the Rust build.

The SHA-256 check proves integrity of the downloaded CI artifact: the bytes received by the packaging job match the bytes staged by the Windows CI job. It is not, by itself, a provenance proof. Provenance comes from binding the download to the triggering workflow `run-id`/source SHA and from the build-provenance attestation created for the CI-tested executable.

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

## Windows installer and Authenticode

The installer path is implemented with Inno Setup and is deliberately separate
from the measured payload:

- Windows CI builds an **unsigned installer candidate**, installs it silently
  into a temporary per-user directory, verifies that the installed
  `NeuralIA.exe` has the same SHA-256 as the CI-tested executable, verifies
  the HKCU uninstall registration, then uninstalls it;
- the installer uses `PrivilegesRequired=lowest` and installs under
  `%LOCALAPPDATA%\Programs\NeuralIA`; it does not create protocol or file
  associations;
- Authenticode signs the **installer only**. The CI-tested
  `NeuralIA.exe` is not signed after measurement because Authenticode changes
  its bytes and would invalidate the byte-for-byte guarantee above;
- stable release packaging emits a signed installer only when both
  `NEURALIA_AUTHENTICODE_PFX_B64` and
  `NEURALIA_AUTHENTICODE_PFX_PASSWORD` are configured. If neither exists,
  the installer asset is omitted and the existing executable release remains
  valid. If exactly one exists, packaging fails closed;
- PR CI exercises the Authenticode sign/verify path with an ephemeral trusted
  code-signing certificate but skips the external timestamp authority, so a
  third-party outage cannot turn the product gate red;
- stable release signing does **not** skip timestamping: it uses an RFC3161
  SHA-256 timestamp before `signtool verify /pa`, PowerShell Authenticode
  verification, and the same install/hash/uninstall smoke gate; all must pass
  before the setup executable enters `dist/`.

The certificate itself is an owner-controlled release credential, not repository
content. The README signed-installer milestone remains incomplete until a real
trusted certificate is configured and a stable signed installer is actually
published.

The repository does not redistribute WebView2. Any future protocol or file
association remains opt-in and must not be registered silently.
