import assert from 'node:assert/strict';
import * as pdfjsLib from '../assets/pdfjs/pdf.mjs';

const viewerModule =
  process.env.NEURALIA_PDF_VIEWER_MODULE ||
  new URL('../assets/pdfjs/viewer.mjs', import.meta.url).href;

const { describeLoadError, hideStatus, showStatus } = await import(viewerModule);

let passed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
    console.log('ok - ' + name);
  } catch (err) {
    console.error('not ok - ' + name);
    throw err;
  }
}

test('ResponseException is classified as transport failure', () => {
  const err = new pdfjsLib.ResponseException('Service unavailable', 503, false);
  assert.equal(describeLoadError(err), 'Não consegui obter o PDF: HTTP 503');
});

test('InvalidPDFException is classified as document failure', () => {
  const err = new pdfjsLib.InvalidPDFException('bad xref');
  assert.equal(
    describeLoadError(err),
    'Este ficheiro não é um PDF que eu consiga abrir: bad xref'
  );
});

test('wrapped fetch failure remains a transport failure', () => {
  const err = { name: 'UnknownErrorException', message: 'Failed to fetch document' };
  assert.equal(
    describeLoadError(err),
    'Não consegui obter o PDF: Failed to fetch document'
  );
});

test('unknown parser failure is not mislabeled as network', () => {
  const err = { name: 'UnknownErrorException', message: 'bad object stream' };
  assert.equal(
    describeLoadError(err),
    'Este ficheiro não é um PDF que eu consiga abrir: bad object stream'
  );
});

test('hideStatus defeats author CSS display:grid', () => {
  const classes = new Set(['error']);
  const element = {
    hidden: false,
    style: { display: 'grid' },
    textContent: 'A carregar',
    classList: {
      toggle(name, enabled) {
        if (enabled) classes.add(name); else classes.delete(name);
      },
      remove(name) {
        classes.delete(name);
      }
    }
  };

  hideStatus(element);
  assert.equal(element.hidden, true);
  assert.equal(element.style.display, 'none');
  assert.equal(classes.has('error'), false);

  showStatus(element, 'erro', true);
  assert.equal(element.hidden, false);
  assert.equal(element.style.display, 'grid');
  assert.equal(element.textContent, 'erro');
  assert.equal(classes.has('error'), true);
});

console.log(`pdf-viewer runtime: ${passed} tests passed`);
