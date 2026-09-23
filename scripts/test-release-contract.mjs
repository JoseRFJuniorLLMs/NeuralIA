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
]) {
  assert.match(smokeJob, pattern, why);
}

const buildScript = fs.readFileSync('scripts/build-windows-installer.ps1', 'utf8');
assert.match(
  buildScript,
  /cargo build --release --locked -p neural-setup/,
  'the installer is neural-setup, built from the locked workspace'
);
assert.match(buildScript, /NEURALIA_PAYLOAD_DIR/, 'the payload reaches neural-setup through NEURALIA_PAYLOAD_DIR');
assert.match(
  buildScript,
  /does not match the workspace version/,
  'the build refuses a -Version the installer would not register'
);
assert.doesNotMatch(buildScript, /ISCC|NeuralIA\.iss/, 'the Inno Setup path is gone');
assert.ok(!fs.existsSync('installer/NeuralIA.iss'), 'the dead Inno Setup script must not come back');

console.log('release contract: single public installer asset (neural-setup around the CI-tested binary)');
