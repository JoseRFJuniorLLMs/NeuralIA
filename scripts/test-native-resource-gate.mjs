import assert from 'node:assert/strict';
import fs from 'node:fs';

const gate = fs.readFileSync('scripts/measure-cycles.ps1', 'utf8');

for (const name of ['MaxHandleGrowth', 'MaxThreadGrowth', 'MaxGdiGrowth']) {
  assert.match(
    gate,
    new RegExp('\\[int\\]\\

assert.match(gate, /GetGuiResources/, 'GDI objects must be measured through GetGuiResources');
assert.match(gate, /handle_growth\s*=\s*\$handleGrowth/, 'handle growth must be exported');
assert.match(gate, /thread_growth\s*=\s*\$threadGrowth/, 'thread growth must be exported');
assert.match(gate, /gdi_growth\s*=\s*\$gdiGrowth/, 'GDI growth must be exported');
assert.match(gate, /handles cresceram/, 'handle growth must fail the gate');
assert.match(gate, /threads cresceram/, 'thread growth must fail the gate');
assert.match(gate, /objetos GDI cresceram/, 'GDI growth must fail the gate');

console.log('native resource lifecycle gate: handles + threads + GDI enforced');
 + name + '\\s*='),
    name + ' must remain an explicit integer threshold parameter'
  );
}

assert.match(gate, /GetGuiResources/, 'GDI objects must be measured through GetGuiResources');
assert.match(gate, /handle_growth\s*=\s*\$handleGrowth/, 'handle growth must be exported');
assert.match(gate, /thread_growth\s*=\s*\$threadGrowth/, 'thread growth must be exported');
assert.match(gate, /gdi_growth\s*=\s*\$gdiGrowth/, 'GDI growth must be exported');
assert.match(gate, /handles cresceram/, 'handle growth must fail the gate');
assert.match(gate, /threads cresceram/, 'thread growth must fail the gate');
assert.match(gate, /objetos GDI cresceram/, 'GDI growth must fail the gate');

console.log('native resource lifecycle gate: handles + threads + GDI enforced');
