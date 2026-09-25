# SPEC-0013 — Packaging and Release

**Status:** Normative

The stable v1 artifact is a Windows x64 executable from the Rust workspace.

A stable release MUST originate from a successful `main` CI run for the exact source SHA that is tagged. The release workflow creates a new `v<workspace-version>` tag only when that version tag does not already exist.

Here "stable release" means a published, tagged release, as opposed to CI artifacts. Its GitHub channel follows the version (AGENTS.md 2.1). A tag matching `^v[0-9]+\.0\.[0-9]+$` is an LTS release, published with `--latest`. Every other tag is a preview, published with `--prerelease`, which leaves the current Latest unchanged. `scripts/test-release-contract.mjs` pins that pattern, runs it over a table of tags, and checks that the flag it picks is the only one `gh release create` receives.

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
- ship that same `NeuralIA.exe`, without rebuilding it, as the payload of the installer described below;
- keep the CI-tested executable's SHA-256, a CycloneDX JSON SBOM for the Windows application, Cargo metadata and the Home and WebView lifecycle measurement JSON in the release workflow artifact `NeuralIA-windows-x64-<tag>`; the public GitHub Release carries exactly one asset, `NeuralIA-Setup-<version>-x64.exe` (`scripts/test-release-contract.mjs`);
- generate GitHub build-provenance attestation in CI for the same tested executable, before release packaging;
- generate a second provenance attestation in the release workflow for the published `dist/NeuralIA-Setup-<version>-x64.exe`, after the installer smoke gate and immediately before publication.

Stable release assets MUST be immutable by policy. When both the version tag
and its GitHub Release already exist, a later main push with the same workspace
version is a no-op. If only one of tag/release exists, automation MUST fail
closed rather than repair, overwrite or clobber stable state.

## Windows installer and Authenticode

The installer is `neural-setup` (`crates/neural-setup`): the project's own
installer, with the brand and the animated neural tissue of the Home.
`scripts/build-windows-installer.ps1` builds it around the measured payload:

- it copies the exact `-ExePath` bytes (the CI-tested `NeuralIA.exe`) as the
  only file of a fresh payload folder, checks the copy's SHA-256, and compiles
  `neural-setup` with `cargo build --release --locked` and
  `NEURALIA_PAYLOAD_DIR`. `build.rs` packs that folder with the same
  `archive.rs` the installer unpacks with, and the installer verifies the
  package digest before writing anything;
- it refuses a `-Version` other than the workspace version, because the
  installer registers its own `CARGO_PKG_VERSION` in Windows "Apps";
- it reads `NEURALIA_AUTHENTICODE_PFX_B64`/`_PASSWORD` once and removes them
  from its environment before any cargo call, and every cargo call refuses to
  run with them set: cargo hands its environment to every build script and
  proc-macro it compiles. `scripts/test-build-installer-isolation.ps1` (CI
  `installer-smoke`, before any installer build) runs the script against a
  fake `cargo` and fails if any call sees a secret, if the packed payload is
  not exactly the `-ExePath` bytes, or if `-Sign` no longer receives the
  secrets. `scripts/build-windows-installer.ps1` is the only installer build
  script;
- the payload is that single file: `NeuralIA.exe` links the WebView2 loader
  statically and embeds PDF.js, Live and the Home art. The system WebView2
  runtime remains a prerequisite.

The installer:

- installs per-user under `%LOCALAPPDATA%\Programs\NeuralIA` as `asInvoker`
  (no elevation), creates the Start menu shortcut (a Desktop one is optional)
  and the HKCU entry
  `Software\Microsoft\Windows\CurrentVersion\Uninstall\NeuralIA`; it does not
  create protocol or file associations;
- accepts `/S` (silent; exit code 0 success, 1 failure, 2 refused arguments or
  install folder, 3 `NeuralIA.exe` in use, 4 built without payload),
  `--uninstall`, and `/D=<folder>` (the last argument, NSIS-style: the folder
  runs to the end of the command line, spaces included);
- installs all-or-nothing. A `NeuralIA.exe` in use stops the install before
  anything is written (the window asks to close NeuralIA; `/S` exits with 3).
  The payload and the uninstaller copy are all written under temporary names
  before anything is swapped, so a full disk fails with nothing changed. If a
  later step fails (a shortcut, the "Apps" entry), the previous files, the
  shortcuts as they were and the previous entry are put back, and a fresh
  install is removed with the folders it created;
- upgrades a 2.1.x Inno Setup install in place. Without `/D=` it reuses the
  folder of its own previous registration, else the one of the Inno
  registration. Only after its own registration is written does it delete
  that folder's `unins???.*` files whose uninstall log names the Inno `AppId`
  `{8B2A98F4-7D55-4C43-ABF0-0D7D1A02C4B9}` (or that the registration names),
  and the `{AppId}_is1` entry, so Windows "Apps" lists one NeuralIA. The Start
  menu shortcut, which Inno created at the same path, is rewritten to the new
  executable;
- never deletes or writes the user's data folder (`%LOCALAPPDATA%\NeuralIA`,
  or `NEURALIA_DATA_DIR`: history, memory, notes, Gemini key, WebView2
  profile). An install folder that is, contains or lies inside it is refused
  (exit 2), and every path install, upgrade or uninstall deletes or replaces is
  checked against it first (`guard_user_data`);
- uninstalls only the folder its uninstaller lives in, and removes only
  shortcuts that open that folder's `NeuralIA.exe` and only a registration
  that points at that folder. Both are decided by file identity (volume and
  file index) before anything is deleted, so an 8.3 path such as
  `C:\Users\RUNNER~1\...` and its long form are the same folder. It first
  moves its working directory out of the folder (Explorer starts it inside);
  it keeps the uninstaller while any payload file is still in use, so the
  "Apps" entry keeps working; the running uninstaller moves itself to
  `%TEMP%` (or next to the folder when `%TEMP%` is on another disk) and a
  windowless `cmd` deletes that copy once the process has exited;
- shows its texts in pt-BR with accents, like the application;
- animates the neural tissue: each timer tick advances the tissue clock
  (`TissueClock`) with wall time only while the window is visible, so the
  neurons move while it is shown and stop while it is minimized.

These behaviours are gated by the `neural-setup` tests (the upgrade, in-use,
data-folder, failure-rollback, 8.3-path and uninstall flows run against a
sandboxed registry branch).

CI (`installer-smoke`) builds an unsigned installer candidate and runs
`scripts/test-windows-installer.ps1` against the runner's real per-user
registration:

- silent install with `/S /D=<temporary folder with a space>`; the installed
  `NeuralIA.exe` has the SHA-256 of the CI-tested executable; the
  registration (`DisplayName`, `DisplayVersion`, `InstallLocation`, uninstall
  commands), the uninstaller and the shortcuts exist; with `NeuralIA.exe` held
  open a reinstall exits 3 and changes nothing; the data folder as install
  folder exits 2; silent uninstall, started with the install folder as its
  working directory, removes files, folder, shortcuts and registration, and
  the uninstaller's parked copy is gone within 60 s;
- an upgrade without `/D=` over a simulated Inno Setup 2.1.x install, then
  uninstall;
- the data folder is byte-identical at the end.

The harness refuses to run where NeuralIA is already registered; `-Isolated`
(`NEURALIA_SETUP_SANDBOX`) keeps the registration and shortcuts inside a
private sandbox for local runs. The job also proves that a mismatched
`-Version` is refused, that a substituted payload is rejected and that a
tampered signature is rejected.

- Authenticode signs the **installer only**. The CI-tested
  `NeuralIA.exe` is not signed after measurement because Authenticode changes
  its bytes and would invalidate the byte-for-byte guarantee above;
- stable release packaging signs the installer when both
  `NEURALIA_AUTHENTICODE_PFX_B64` and
  `NEURALIA_AUTHENTICODE_PFX_PASSWORD` are configured. If neither exists, the
  tested unsigned installer is published rather than omitting installation.
  If exactly one exists, packaging fails closed;
- PR CI exercises the Authenticode sign/verify path with an ephemeral
  self-signed code-signing certificate. It verifies the cryptographic
  signature and expected signer thumbprint while allowing only the expected
  trust-chain failure, and skips the external timestamp authority so a
  third-party outage cannot turn the product gate red;
- stable release signing does **not** skip timestamping: it uses an RFC3161
  SHA-256 timestamp before `signtool verify /pa`, PowerShell Authenticode
  verification, and the same installer smoke gate; all must pass before the
  setup executable enters `dist/`.

The certificate itself is an owner-controlled release credential, not repository
content. The README signed-installer milestone remains incomplete until a real
trusted certificate is configured and a stable signed installer is actually
published.

The repository does not redistribute WebView2. Any future protocol or file
association remains opt-in and must not be registered silently.
