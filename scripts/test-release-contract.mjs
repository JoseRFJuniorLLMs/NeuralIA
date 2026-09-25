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

// The accelerator spike (infra-accel-spike, 2.3 plan) is CI-only: its code
// compiles only with the neural-app feature `accel-spike`, and the exe that
// release.yml packages must be built without it. Behaviour: the `windows` job
// of ci.yml proves the spike marker is absent from the exact
// ci-tested/NeuralIA.exe bytes, and the accel-spike.yml workflow proves the
// same check sees the marker in a spike build. These assertions keep that
// wiring from being edited away, and the spike out of the workflow that
// release.yml waits for.
const appManifest = fs.readFileSync('crates/neural-app/Cargo.toml', 'utf8');
const features = appManifest.match(/\n\[features\]\n([\s\S]*?)(?=\n\[)/);
assert.ok(features, 'neural-app declares its [features] table');
assert.match(features[1], /^default = \[\]$/m, 'neural-app has no default features');
assert.match(features[1], /^accel-spike = \[\]$/m, 'accel-spike exists and pulls no crate');
assert.doesNotMatch(
  features[1].replace(/^#.*$/gm, ''),
  /default\s*=\s*\[[^\]]*accel-spike/,
  'accel-spike is never a default feature'
);

const mainRs = fs.readFileSync('crates/neural-app/src/main.rs', 'utf8');
assert.match(
  mainRs,
  /#\[cfg\(any\(test, feature = "accel-spike"\)\)\]\nmod accel_spike;/,
  'the spike module compiles only in tests and in the accel-spike build'
);
assert.equal((mainRs.match(/\bmod accel_spike\b/g) || []).length, 1, 'accel_spike is declared once');
const windowsAppRs = fs.readFileSync('crates/neural-app/src/windows_app.rs', 'utf8');
assert.match(
  windowsAppRs,
  /#\[cfg\(feature = "accel-spike"\)\]\n#\[path = "accel_spike_app\.rs"\]\npub\(in crate::windows_app\) mod accel_spike_app;/,
  'the spike glue compiles only in the accel-spike build'
);
assert.equal(
  (windowsAppRs.match(/\bmod accel_spike_app\b/g) || []).length,
  1,
  'accel_spike_app is declared once'
);

const markerRust = fs
  .readFileSync('crates/neural-app/src/accel_spike.rs', 'utf8')
  .match(/pub\(crate\) const SPIKE_BUILD_MARKER: &str = "([^"]+)";/);
const markerScript = fs
  .readFileSync('scripts/test-accel-spike-marker.ps1', 'utf8')
  .match(/^\$marker = "([^"]+)"$/m);
assert.ok(markerRust && markerScript, 'the spike marker is declared in the module and in the gate');
assert.equal(markerScript[1], markerRust[1], 'the release gate looks for the marker the spike writes');

// Which workflow runs can hold a release back: release.yml packages only after
// a workflow_run of the workflows it lists concludes with success. The spike
// must never be one of them: a failing chord is an outcome the spike expects
// (fallback 1/2 of the brief), and an invalid run (no input or no focus on the
// runner) says nothing about the product. Neither may skip a release.
const unquote = (text) => text.trim().replace(/^(["'])(.*)\1$/, '$2');
const watched = workflow.match(/\non:\n {2}workflow_run:\n {4}workflows: \[([^\]\n]*)\]\n/);
assert.ok(watched, 'release.yml is triggered by workflow_run on a listed set of workflows');
const watchedNames = watched[1].split(',').map(unquote);
assert.deepEqual(watchedNames, ['CI'], 'release.yml waits only for the CI workflow');
const workflowDir = '.github/workflows';
const workflows = new Map(
  fs
    .readdirSync(workflowDir)
    .filter((file) => /\.ya?ml$/.test(file))
    .map((file) => [file, fs.readFileSync(`${workflowDir}/${file}`, 'utf8')])
);
const nameOf = (text) => {
  const name = text.match(/^name:\s*(.+?)\s*$/m);
  return name ? unquote(name[1]) : undefined;
};
assert.deepEqual(
  [...workflows].filter(([, text]) => watchedNames.includes(nameOf(text))).map(([file]) => file),
  ['ci.yml'],
  'the CI workflow that release.yml waits for is ci.yml'
);
const noComments = (text) => text.replace(/^[ \t]*#.*\n/gm, '');

// The marker step is exactly name/shell/run and the upload follows it at once:
// no `if:` can skip it and no `continue-on-error:` can turn its red into green.
const markerStep =
  '      - name: Published exe has the accel-spike feature off\n' +
  '        shell: pwsh\n' +
  '        run: ./scripts/test-accel-spike-marker.ps1 -ExePath ci-tested/NeuralIA.exe -Expect Absent\n';
const windowsJob = ci.slice(ci.indexOf('\n  windows:'), ci.indexOf('\n  installer-smoke:'));
assert.ok(windowsJob.length > 0, 'ci.yml keeps the windows job');
assert.match(
  windowsJob,
  /- run: cargo build --locked -p neural-app --bin NeuralIA --release\n/,
  'the published exe is built by the windows job'
);
assert.equal(windowsJob.split(markerStep).length - 1, 1, 'the windows job checks the spike marker once');
const stageAt = windowsJob.indexOf('Copy-Item target/release/NeuralIA.exe ci-tested/NeuralIA.exe');
const markerAt = windowsJob.indexOf(markerStep);
assert.ok(stageAt > 0 && markerAt > stageAt, 'the spike marker is checked on the staged ci-tested bytes');
assert.ok(
  windowsJob.includes(markerStep + '      - uses: actions/upload-artifact@'),
  'the tested exe is uploaded right after the marker check, which has no if and no continue-on-error'
);
assert.doesNotMatch(windowsJob, /continue-on-error/, 'no step of the windows job can fail without failing CI');
const windowsBuild = noComments(windowsJob).replace(markerStep, '');
assert.doesNotMatch(
  windowsBuild,
  /(^|\s)-F(\s|$)|--features|--all-features/m,
  'the windows job never builds with extra features (-F, --features, --all-features)'
);
for (const line of windowsBuild.split('\n').filter((text) => /\bcargo\s/.test(text))) {
  assert.doesNotMatch(line, /(^|\s)-F/, `the windows job never passes -F to cargo: ${line.trim()}`);
}

// The spike runs only in its own workflow, on pull requests and by hand.
const spikeFile = 'accel-spike.yml';
const spike = workflows.get(spikeFile);
assert.ok(spike, 'the spike runs in its own workflow, .github/workflows/accel-spike.yml');
assert.ok(!watchedNames.includes(nameOf(spike)), 'release.yml never waits for the spike workflow');
const triggers = spike.match(/\non:\n((?: {2}.*\n)+)/);
assert.ok(triggers, 'the spike workflow lists its triggers as a block');
assert.deepEqual(
  [...triggers[1].matchAll(/^ {2}([a-z_]+):/gm)].map((trigger) => trigger[1]).sort(),
  ['pull_request', 'workflow_dispatch'],
  'the spike runs on pull requests and by hand, never on the push to main that publishes'
);
assert.match(spike, /\npermissions:\n {2}contents: read\n/, 'the spike workflow only reads the repository');
assert.doesNotMatch(spike, /:\s*write\b/, 'the spike workflow is granted no write permission');
assert.match(
  spike,
  /cargo build --locked -p neural-app --bin NeuralIA --release --features accel-spike/,
  'the spike workflow builds the release exe with the spike feature'
);
assert.match(
  spike,
  /test-accel-spike-marker\.ps1 -ExePath target\/release\/NeuralIA\.exe -Expect Present/,
  'the spike workflow proves the marker check sees a spike build'
);
assert.match(
  spike,
  /\.\/scripts\/test-accel-spike\.ps1 -ExePath target\/release\/NeuralIA\.exe -TablePath /,
  'the spike workflow runs the E2E'
);
assert.doesNotMatch(spike, /actions\/upload-artifact/, 'the spike exe never leaves its workflow');
for (const [file, text] of workflows) {
  if (file === spikeFile) continue;
  assert.doesNotMatch(
    noComments(text).replace(markerStep, ''),
    /accel.spike|accel_spike/i,
    `${file} never builds or runs the spike (only ${spikeFile} does; ci.yml only checks the marker is absent)`
  );
}

// No job consumes the spike job, in any form YAML allows for `needs:`.
function needsOf(text) {
  const out = [];
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const key = lines[i].match(/^\s*needs:\s*(.*)$/);
    if (!key) continue;
    let rest = key[1].replace(/\s+#.*$/, '').trim();
    if (rest.startsWith('[')) {
      while (!rest.includes(']') && i + 1 < lines.length) {
        rest += ' ' + lines[++i].replace(/\s+#.*$/, '').trim();
      }
      out.push(...rest.replace(/^\[/, '').replace(/\].*$/, '').split(',').map(unquote).filter(Boolean));
    } else if (rest) {
      out.push(unquote(rest));
    } else {
      while (i + 1 < lines.length && /^\s*(-\s|#|$)/.test(lines[i + 1])) {
        const item = lines[++i].match(/^\s*-\s+(.*?)(\s+#.*)?$/);
        if (item) out.push(unquote(item[1]));
      }
    }
  }
  return out;
}
assert.deepEqual(needsOf('    needs: a\n'), ['a'], 'needs parser: scalar');
assert.deepEqual(needsOf('    needs: [a, "b"]\n'), ['a', 'b'], 'needs parser: flow list');
assert.deepEqual(needsOf('    needs: [a,\n      b] # why\n'), ['a', 'b'], 'needs parser: flow list over two lines');
assert.deepEqual(
  needsOf("    needs:\n      - a\n\n      # c\n      - 'b' # why\n    runs-on: x\n      - c\n"),
  ['a', 'b'],
  'needs parser: block list'
);
for (const [file, text] of workflows) {
  assert.ok(!needsOf(text).includes('accel-spike'), `no job in ${file} consumes the spike job`);
}
assert.doesNotMatch(workflow, /accel-spike|accel_spike|--features|--all-features/, 'release.yml never builds the spike');
assert.doesNotMatch(buildScript, /accel-spike|--features|--all-features/, 'the installer build never enables features');

console.log('release contract: single public installer asset (neural-setup around the CI-tested binary)');
console.log('release contract: the published exe is built without the accel-spike feature');
console.log('release contract: the accelerator spike runs outside the CI workflow that release.yml waits for');
