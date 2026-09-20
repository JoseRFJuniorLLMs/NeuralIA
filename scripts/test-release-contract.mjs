import assert from 'node:assert/strict';
import fs from 'node:fs';

const workflow = fs.readFileSync('.github/workflows/release.yml', 'utf8');

assert.match(
  workflow,
  /\$portableName\s*=\s*"NeuralIA-\$version-x64\.exe"/,
  'portable release executable must include the version in its public filename'
);
assert.match(
  workflow,
  /NeuralIA-Setup-\*-x64\.exe/,
  'release must include the versioned installer family'
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
  /subject-path:\s*dist\/NeuralIA-\*-x64\.exe/,
  'attestation must follow the versioned public executable name'
);

console.log('release contract: versioned portable EXE + mandatory installer');
