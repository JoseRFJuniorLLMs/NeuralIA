import assert from 'node:assert/strict';
import fs from 'node:fs';

const workflow = fs.readFileSync('.github/workflows/release.yml', 'utf8');

assert.match(
  workflow,
  /NeuralIA-Setup-\*-x64\.exe/,
  'release must build the versioned installer family'
);
assert.match(
  workflow,
  /publishing the tested unsigned installer instead of omitting installation entirely/,
  'missing Authenticode credentials must not remove the installer from the release'
);
assert.doesNotMatch(
  workflow,
  /signed installer asset will be omitted/,
  'release must never silently omit installation just because signing secrets are absent'
);
assert.match(
  workflow,
  /subject-path:\s*dist\/NeuralIA-Setup-\$\{\{ steps\.version\.outputs\.version \}\}-x64\.exe/,
  'attestation must follow the public installer'
);
assert.match(
  workflow,
  /gh release create "\$RELEASE_TAG" dist\/NeuralIA-Setup-\*-x64\.exe/,
  'stable release must publish only the installer asset'
);
assert.doesNotMatch(
  workflow,
  /gh release create[^\n]*dist\/\*/,
  'stable release must not upload every internal CI artifact'
);
// Two release rhythms (AGENTS.md 2.1): X.0.Z is LTS (Latest), everything else
// is a preview published as a pre-release. Run the shipped bash pattern on a
// table of tags instead of trusting its text.
const channelRule = workflow.match(
  /if \[\[ "\$RELEASE_TAG" =~ (\S+) \]\]; then\s+channel_flag="--latest"\s+else\s+channel_flag="--prerelease"\s+fi/
);
assert.ok(channelRule, 'publish must pick --latest or --prerelease from the release tag');
// The table below runs the pattern with JavaScript's engine, so pin the exact
// bash text too: an edit both engines read differently (\d, classes) must fail
// here, and nothing may change the flag after the if/else picks it.
assert.equal(channelRule[1], '^v[0-9]+\\.0\\.[0-9]+$', 'the LTS pattern is exactly ^v[0-9]+\\.0\\.[0-9]+$');
assert.equal((workflow.match(/channel_flag=/g) || []).length, 2, 'channel_flag is set only by the if/else');
assert.equal((workflow.match(/--latest\b/g) || []).length, 1, '--latest appears only in the LTS branch');
assert.equal((workflow.match(/--prerelease\b/g) || []).length, 1, '--prerelease appears only in the preview branch');
const ltsTag = new RegExp(channelRule[1]);
for (const [tag, lts] of [
  ['v3.0.0', true],
  ['v3.0.7', true],
  ['v4.0.0', true],
  ['v10.0.12', true],
  ['v2.2.0', false],
  ['v2.1.8', false],
  ['v3.1.0', false],
  ['v3.10.0', false],
  ['v30.1.0', false],
  ['v3.0.0-rc1', false],
]) {
  assert.equal(ltsTag.test(tag), lts, `${tag} must be ${lts ? 'LTS (Latest)' : 'a preview (pre-release)'}`);
}
assert.match(
  workflow,
  /gh release create "\$RELEASE_TAG"[^\n]*--generate-notes\s+"\$channel_flag"/,
  'the release is created with the channel the tag picked'
);
assert.doesNotMatch(
  workflow,
  /\$portableName/,
  'portable executable must not be prepared for public release'
);
assert.doesNotMatch(
  workflow,
  /dist\/\$\(\$installer\.Name\)\.sha256|dist\/\$portableName\.sha256/,
  'checksum sidecars stay in logs/internal CI, not as public release assets'
);

// The public installer is neural-setup (crates/neural-setup) wrapped around
// the exact executable CI measured, and it is smoke-tested before publishing.
const packageJob = workflow.slice(
  workflow.indexOf('\n  package:'),
  workflow.indexOf('\n  publish:')
);
assert.ok(packageJob.length > 0, 'release.yml must keep a package job before publish');
assert.match(
  packageJob,
  /Verify the CI-tested binary checksum/,
  'release must verify the downloaded CI-tested binary before packaging it'
);
assert.match(
  packageJob,
  /\.\/scripts\/build-windows-installer\.ps1 -ExePath ci-tested\/NeuralIA\.exe /,
  'the release installer must wrap the CI-tested NeuralIA.exe, never a rebuild'
);
assert.match(
  packageJob,
  /test-windows-installer\.ps1 -InstallerPath \$installer\.FullName -ExpectedExePath ci-tested\/NeuralIA\.exe/,
  'the release installer must be smoke-tested against the CI-tested binary'
);
assert.match(
  packageJob,
  /dtolnay\/rust-toolchain@[0-9a-f]{40} # 1\.98\.1/,
  'the package job compiles neural-setup and needs the pinned toolchain'
);

const ci = fs.readFileSync('.github/workflows/ci.yml', 'utf8');
const smokeJob = ci.slice(
  ci.indexOf('\n  installer-smoke:'),
  ci.indexOf('\n  attest-tested-binary:')
);
assert.ok(smokeJob.length > 0, 'ci.yml must keep the installer-smoke job');
assert.match(
  smokeJob,
  /dtolnay\/rust-toolchain@[0-9a-f]{40} # 1\.98\.1/,
  'installer-smoke compiles neural-setup and needs the pinned toolchain'
);
for (const [pattern, why] of [
  [/does not match the workspace version/, 'CI must prove a mismatched -Version is refused'],
  [/differs from the CI-tested payload/, 'CI must prove a substituted payload is rejected'],
  [/Authenticode gate stayed green after the signed installer was tampered/, 'CI must prove a tampered signature is rejected'],
  [
    /\.\/scripts\/test-build-installer-isolation\.ps1/,
    'CI must prove cargo never runs with the signing secrets and packs the exact payload',
  ],
]) {
  assert.match(smokeJob, pattern, why);
}
// The isolation gate must run before any step that builds an installer, so a
// leak is caught before the secrets could reach a build script.
assert.ok(
  smokeJob.indexOf('test-build-installer-isolation.ps1') <
    smokeJob.indexOf('./scripts/build-windows-installer.ps1'),
  'the secret-isolation gate runs before the first installer build'
);

const buildScript = fs.readFileSync('scripts/build-windows-installer.ps1', 'utf8');
assert.match(
  buildScript,
  /Invoke-Cargo build --release --locked -p neural-setup/,
  'the installer is neural-setup, built from the locked workspace'
);
// Every cargo call goes through Invoke-Cargo, which refuses to run with a
// signing secret in the environment (behaviour: test-build-installer-isolation.ps1).
assert.equal(
  (buildScript.match(/&\s*cargo\b/g) || []).length,
  1,
  'cargo is called only from Invoke-Cargo'
);
assert.match(buildScript, /NEURALIA_PAYLOAD_DIR/, 'the payload reaches neural-setup through NEURALIA_PAYLOAD_DIR');
assert.match(
  buildScript,
  /does not match the workspace version/,
  'the build refuses a -Version the installer would not register'
);
assert.doesNotMatch(buildScript, /ISCC|NeuralIA\.iss/, 'the Inno Setup path is gone');
assert.ok(!fs.existsSync('installer/NeuralIA.iss'), 'the dead Inno Setup script must not come back');
// A second installer script rebuilt NeuralIA.exe locally, without --locked,
// a version check or the smoke harness: an installer from it would not carry
// the CI-tested bytes. scripts/build-windows-installer.ps1 is the only way.
assert.ok(
  !fs.existsSync('packaging/build-installer.ps1'),
  'the ungated packaging/build-installer.ps1 must not come back'
);

console.log('release contract: single public installer asset (neural-setup around the CI-tested binary)');
