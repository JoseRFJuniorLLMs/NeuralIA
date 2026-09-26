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

// The CI-only feature `test-stores` (infra-settings-keys, 2.3 plan): neural-core's
// `StoreRegistry::mint_for_test` (a store registry that does not spend the
// once-per-process mint) compiles only in neural-core's own tests and with this
// feature, and only neural-app's [dev-dependencies] enable it. Behaviour: the
// windows job asks `cargo tree` for the normal build graph (the one the release
// build uses) and fails if it enables the feature. These assertions keep that
// wiring from being edited away.
const manifests = [
  'Cargo.toml',
  'crates/neural-core/Cargo.toml',
  'crates/neural-app/Cargo.toml',
  'crates/neural-setup/Cargo.toml',
];
const testStoresUses = [];
for (const file of manifests) {
  let section = '';
  for (const line of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
    const header = line.match(/^\[(.+)\]\s*$/);
    if (header) {
      section = header[1];
    } else if (!/^\s*#/.test(line) && /test-stores/.test(line)) {
      testStoresUses.push(`${file} [${section}] ${line.trim()}`);
    }
  }
}
assert.deepEqual(
  testStoresUses,
  [
    'crates/neural-core/Cargo.toml [features] test-stores = []',
    'crates/neural-app/Cargo.toml [dev-dependencies] neural-core = { path = "../neural-core", features = ["test-stores"] }',
  ],
  'test-stores is declared by neural-core, pulls no crate, and only neural-app [dev-dependencies] enable it'
);
const coreFeatures = fs
  .readFileSync('crates/neural-core/Cargo.toml', 'utf8')
  .match(/\n\[features\]\n([\s\S]*?)(?=\n\[)/);
assert.ok(coreFeatures, 'neural-core declares its [features] table');
assert.match(coreFeatures[1], /^default = \[\]$/m, 'neural-core has no default features');
const jsonStore = fs.readFileSync('crates/neural-core/src/json_store.rs', 'utf8').replace(/\r\n/g, '\n');
assert.match(
  jsonStore,
  /#\[cfg\(any\(test, feature = "test-stores"\)\)\]\n {4}pub fn mint_for_test\(/,
  'mint_for_test compiles only in tests and with test-stores'
);
assert.equal(
  (jsonStore.match(/^\s*pub fn mint_for_test\(/gm) || []).length,
  1,
  'mint_for_test is declared once'
);
const testStoresStepName = '      - name: Published exe has the test-stores feature off\n';
assert.equal(
  windowsJob.split(testStoresStepName).length - 1,
  1,
  'the windows job checks the test-stores feature once'
);
const testStoresStep = windowsJob
  .slice(windowsJob.indexOf(testStoresStepName) + testStoresStepName.length)
  .split('\n      - ')[0];
assert.doesNotMatch(testStoresStep, /^\s+if:/m, 'the test-stores check has no if');
assert.match(
  testStoresStep,
  /cargo tree --locked -p neural-app --target x86_64-pc-windows-msvc -e features,no-dev -i neural-core/,
  'the test-stores check reads the normal build graph'
);
assert.match(
  testStoresStep,
  /if \(\$normal -match 'test-stores'\) \{\n\s+throw /,
  'the test-stores check fails when the normal build graph enables the feature'
);
assert.ok(
  windowsJob.indexOf(testStoresStepName) >
    windowsJob.indexOf('- run: cargo build --locked -p neural-app --bin NeuralIA --release\n'),
  'the test-stores check follows the release build of the windows job'
);

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

// The sabotage matrix of the windows job (AGENTS.md 4.2): each entry names a
// gate and an Old anchor that Replace-Exact must find exactly once in its
// target file, before the try block that restores the tree. One stale anchor
// throws under ErrorActionPreference=Stop, the step fails, and every entry
// after it never runs. That is what e813ba1 did on 2026-09-25: it removed
// ColumnMenuRequest and left menu-label-frozen-at-registration pointing at
// it. This check reads the matrix the way pwsh will (the run block dedented
// by its ten spaces, here-strings without their closing newline) and holds
// every anchor to exactly one match, every gate to a test that exists, and
// every Ignored flag to a #[ignore] on that test.
const sabotageStepName = '      - name: Prove 100-interaction UI gates reject sabotage\n';
assert.equal(ci.split(sabotageStepName).length - 1, 1, 'the windows job has one sabotage step');
const runMarker = '        run: |\n';
const sabotageStep = ci.slice(ci.indexOf(sabotageStepName) + sabotageStepName.length);
const runAt = sabotageStep.indexOf(runMarker);
assert.ok(runAt >= 0 && runAt < 200, 'the sabotage step is a run: | block');
const runLines = [];
for (const line of sabotageStep.slice(runAt + runMarker.length).split('\n')) {
  if (line === '') {
    runLines.push('');
  } else if (line.startsWith(' '.repeat(10))) {
    runLines.push(line.slice(10));
  } else {
    break;
  }
}
const sabotageScript = runLines.join('\n');
const defaultSourcePath = sabotageScript.match(/^\$defaultSourcePath = "([^"]+)"$/m);
assert.ok(defaultSourcePath, 'the sabotage step declares $defaultSourcePath');

function parseSabotages(list) {
  const entries = [];
  let entry = null;
  let field = null;
  let body = [];
  for (const line of list.split('\n')) {
    if (field) {
      if (line === "'@") {
        entry[field] = body.join('\n');
        field = null;
        body = [];
      } else {
        body.push(line);
      }
      continue;
    }
    let match;
    if (/^ {2}@\{\s*$/.test(line)) {
      assert.equal(entry, null, 'sabotage matrix: an entry opens inside another');
      entry = {};
    } else if (/^ {2}\},?\s*$/.test(line)) {
      assert.ok(entry, 'sabotage matrix: an entry closes without opening');
      entries.push(entry);
      entry = null;
    } else if ((match = line.match(/^ {4}(Label|Test|SourcePath) = "([^"]*)"\s*$/))) {
      entry[match[1]] = match[2];
    } else if (/^ {4}Ignored = \$true\s*$/.test(line)) {
      entry.Ignored = true;
    } else if ((match = line.match(/^ {4}(Old|New) = @'\s*$/))) {
      field = match[1];
    } else if (line.trim() !== '') {
      assert.fail(`sabotage matrix: unrecognised line ${JSON.stringify(line)}`);
    }
  }
  assert.equal(entry, null, 'sabotage matrix: unterminated entry');
  assert.equal(field, null, 'sabotage matrix: unterminated here-string');
  return entries;
}
assert.deepEqual(
  parseSabotages(
    "  @{\n    Label = \"a\"\n    Test = \"t\"\n    Old = @'\n    x == 1\n      && y\n'@\n    New = @'\n    true\n'@\n  },\n" +
      "  @{\n    Label = \"b\"\n    Test = \"u\"\n    SourcePath = \"p.rs\"\n    Ignored = $true\n    Old = @'\n  }\n'@\n    New = @'\n\n'@\n  }\n"
  ),
  [
    { Label: 'a', Test: 't', Old: '    x == 1\n      && y', New: '    true' },
    { Label: 'b', Test: 'u', SourcePath: 'p.rs', Ignored: true, Old: '  }', New: '' },
  ],
  'sabotage parser: two entries, here-strings kept verbatim without the closing newline'
);
const listStart = sabotageScript.indexOf('$sabotages = @(\n');
const listEnd = sabotageScript.indexOf('\n)\n', listStart);
assert.ok(listStart >= 0 && listEnd > listStart, 'the sabotage step declares $sabotages = @( ... )');
const sabotages = parseSabotages(sabotageScript.slice(listStart + '$sabotages = @(\n'.length, listEnd));
assert.ok(sabotages.length >= 20, `the sabotage matrix keeps its entries (found ${sabotages.length})`);
assert.equal(
  new Set(sabotages.map((entry) => entry.Label)).size,
  sabotages.length,
  'sabotage labels are unique'
);
// Every .rs of neural-app/src: a gate may be an inline #[cfg(test)] module of
// any file (src/ is the NeuralIA bin, and the step builds and runs only that
// bin's harness: cargo test -p neural-app --bin NeuralIA <name> finds it), but
// an Ignored gate runs with --exact as windows_app::tests::<name>, so it must
// live in that module.
function rustFilesUnder(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = `${dir}/${entry.name}`;
    if (entry.isDirectory()) return rustFilesUnder(path);
    return entry.name.endsWith('.rs') ? [[path, fs.readFileSync(path, 'utf8').replace(/\r\n/g, '\n')]] : [];
  });
}
const neuralAppSources = rustFilesUnder('crates/neural-app/src');
const sabotageSources = new Map();
for (const entry of sabotages) {
  const label = entry.Label ?? '<no label>';
  assert.ok(entry.Label && entry.Test, `sabotage ${label}: Label and Test are set`);
  assert.ok(typeof entry.Old === 'string' && typeof entry.New === 'string', `sabotage ${label}: Old and New are here-strings`);
  assert.ok(entry.Old.trim() !== '', `sabotage ${label}: Old is not empty`);
  assert.notEqual(entry.Old, entry.New, `sabotage ${label}: New must change something`);
  const path = entry.SourcePath ?? defaultSourcePath[1];
  assert.ok(fs.existsSync(path), `sabotage ${label}: target ${path} exists`);
  if (!sabotageSources.has(path)) {
    sabotageSources.set(path, fs.readFileSync(path, 'utf8').replace(/\r\n/g, '\n'));
  }
  const count = sabotageSources.get(path).split(entry.Old).length - 1;
  assert.equal(
    count,
    1,
    `sabotage ${label}: its Old anchor must match ${path} exactly once (found ${count}); ` +
      'Replace-Exact throws otherwise and every entry after it never runs'
  );
  const declarationRe = new RegExp(`((?:^[ \\t]*#\\[[^\\n]*\\]\\n)+)[ \\t]*fn ${entry.Test}\\(`, 'm');
  const declarations = neuralAppSources
    .map(([path, text]) => [path, text.match(declarationRe)])
    .filter(([, match]) => match);
  assert.equal(
    declarations.length,
    1,
    `sabotage ${label}: its gate ${entry.Test} is declared once in neural-app (found in ${declarations.map(([path]) => path).join(', ') || 'no file'})`
  );
  const [declaredIn, declaration] = declarations[0];
  assert.match(declaration[1], /^[ \t]*#\[test\]$/m, `sabotage ${label}: ${entry.Test} is #[test]`);
  assert.equal(
    /^[ \t]*#\[ignore\b/m.test(declaration[1]),
    Boolean(entry.Ignored),
    `sabotage ${label}: Ignored must match the #[ignore] of ${entry.Test} (the CI-only focus gates run alone with --ignored --exact)`
  );
  if (entry.Ignored) {
    assert.equal(
      declaredIn,
      'crates/neural-app/src/windows_app/tests.rs',
      `sabotage ${label}: the CI-only gate ${entry.Test} runs as windows_app::tests::${entry.Test}, so it lives there`
    );
  }
}

// The LLM transport (infra-llm-transport, 2.3 plan): neural_core::llm reaches
// only the pinned provider hosts. Endpoint::pinned is its only constructor in
// the published build; Endpoint::loopback (the unit tests' stub origin)
// compiles only under cfg(test). Behaviour: the Rust gate
// `release_has_no_endpoint_override` (src/llm/tests.rs) and the windows job,
// which scans the exact ci-tested/NeuralIA.exe bytes for base-URL and fixture
// override environment names. These assertions keep that wiring in place.
const llmTransport = fs
  .readFileSync('crates/neural-core/src/llm/transport.rs', 'utf8')
  .replace(/\r\n/g, '\n');
assert.match(
  llmTransport,
  /\n {4}#\[cfg\(test\)\]\n {4}pub\(crate\) fn loopback\(port: u16\) -> Self \{\n/,
  'Endpoint::loopback compiles only in tests'
);
assert.equal(
  (llmTransport.match(/\bfn loopback\(/g) || []).length,
  1,
  'Endpoint::loopback is declared once'
);
assert.equal(
  (llmTransport.match(/\bpub fn pinned\(provider: Provider\) -> Self \{/g) || []).length,
  1,
  'Endpoint::pinned is the constructor that ships'
);
const overrideStep =
  '      - name: Published exe has no LLM endpoint override\n' +
  '        shell: pwsh\n' +
  '        run: ./scripts/test-exe-no-endpoint-override.ps1 -ExePath ci-tested/NeuralIA.exe\n';
assert.equal(windowsJob.split(overrideStep).length - 1, 1, 'the windows job scans the exe for LLM overrides once');
const overrideAt = windowsJob.indexOf(overrideStep);
assert.ok(overrideAt > stageAt, 'the LLM override scan reads the staged ci-tested bytes');
assert.ok(overrideAt < markerAt, 'the LLM override scan runs before the tested exe is uploaded');
assert.match(
  windowsJob.slice(overrideAt + overrideStep.length),
  /^ {6}[-#]/,
  'the LLM override step is exactly name/shell/run (no if, no continue-on-error)'
);
const overrideScript = fs.readFileSync('scripts/test-exe-no-endpoint-override.ps1', 'utf8');
for (const [pattern, why] of [
  [/BASE_URL/, 'the scan looks for base-URL override names'],
  [/FIXTURE/, 'the scan looks for fixture override names'],
  [/\$canaries = @\(/, 'the scan proves it sees planted names before trusting a clean result'],
  [/\[Text\.Encoding\]::Unicode\.GetString/, 'the scan also reads UTF-16LE strings'],
  [/if \(\$found\.Count -gt 0\) \{\n?\s+throw /, 'the scan fails when the exe carries an override name'],
]) {
  assert.match(overrideScript.replace(/\r\n/g, '\n'), pattern, why);
}

console.log('release contract: single public installer asset (neural-setup around the CI-tested binary)');
console.log('release contract: the published exe is built without the accel-spike feature');
console.log('release contract: the published exe is built without the test-stores feature');
console.log('release contract: the accelerator spike runs outside the CI workflow that release.yml waits for');
console.log('release contract: the LLM transport ships only pinned hosts; the published exe has no endpoint override');
console.log(`release contract: every anchor of the ${sabotages.length} CI sabotages matches its file exactly once`);
