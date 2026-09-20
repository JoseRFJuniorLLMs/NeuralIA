import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
if (typeof Promise.try !== 'function') {
  Promise.try = (fn, ...args) => Promise.resolve().then(() => fn(...args));
}
if (typeof Uint8Array.prototype.toHex !== 'function') {
  Uint8Array.prototype.toHex = function () {
    return Array.from(this, byte => byte.toString(16).padStart(2, '0')).join('');
  };
}
if (typeof Uint8Array.fromHex !== 'function') {
  Uint8Array.fromHex = function (hex) {
    if (hex.length % 2 !== 0) throw new SyntaxError('hex string must have even length');
    return Uint8Array.from(hex.match(/../g) || [], pair => Number.parseInt(pair, 16));
  };
}
if (typeof Map.prototype.getOrInsertComputed !== 'function') {
  Map.prototype.getOrInsertComputed = function (key, callback) {
    if (this.has(key)) return this.get(key);
    const value = callback(key);
    this.set(key, value);
    return value;
  };
}
const pdfjsLib = await import('../assets/pdfjs/pdf.mjs');

const pdfjsRoot = fileURLToPath(new URL('../assets/pdfjs/', import.meta.url));
const missing = path.join(pdfjsRoot, '__missing_aux__') + path.sep;
const breakKind = process.env.NEURALIA_PDF_AUX_BREAK || '';
const only = process.env.NEURALIA_PDF_AUX_CASE || '';

function base(name) {
  const dir = path.join(pdfjsRoot, name) + path.sep;
  if (
    (breakKind === 'wasm' && name === 'wasm') ||
    (breakKind === 'cmap' && name === 'cmaps') ||
    (breakKind === 'font' && name === 'standard_fonts') ||
    (breakKind === 'icc' && name === 'icc')
  ) {
    return missing;
  }
  return dir;
}

const cases = [
  ['jpx', 'bug_jpx.pdf'],
  ['jbig2', 'jbig2_file_header.pdf'],
  ['cid-cmap', 'arial_unicode_ab_cidfont.pdf'],
  ['standard-fonts', 'standard_fonts.pdf'],
  ['cmyk', 'cmykjpeg.pdf'],
];

let passed = 0;
for (const [name, file] of cases) {
  if (only && only !== name) continue;

  const data = new Uint8Array(
    await readFile(path.join(pdfjsRoot, 'fixtures', file))
  );
  const task = pdfjsLib.getDocument({
    data,
    cMapUrl: base('cmaps'),
    cMapPacked: true,
    standardFontDataUrl: base('standard_fonts'),
    wasmUrl: base('wasm'),
    iccUrl: base('icc'),
    useWasm: true,
    useWorkerFetch: false,
    useSystemFonts: false,
    stopAtErrors: true,
  });

  const doc = await task.promise;
  assert.ok(doc.numPages > 0, name + ': PDF sem páginas');
  for (let pageNumber = 1; pageNumber <= doc.numPages; pageNumber++) {
    const page = await doc.getPage(pageNumber);
    const operators = await page.getOperatorList();
    assert.ok(operators.fnArray.length > 0, name + ': operator list vazia');
    await page.getTextContent();
    page.cleanup();
  }
  await doc.destroy();
  passed++;
  console.log('ok - aux ' + name);
}

assert.ok(passed > 0, 'nenhum caso auxiliar foi executado');
console.log('pdfjs auxiliary runtime: ' + passed + ' case(s) passed');
