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

console.log('release contract: single public installer asset');
