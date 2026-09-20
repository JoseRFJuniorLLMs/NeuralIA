import assert from 'node:assert/strict';
import {
  PDF_AUX_BASE,
  describeLoadError,
  hideStatus,
  makePdfLoadOptions,
  showStatus
} from '../assets/pdfjs/viewer-runtime.mjs';

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

test('load options never ask PDF.js for missing WASM binaries', () => {
  const options = makePdfLoadOptions('./document.pdf', 1048576);
  assert.equal(options.url, './document.pdf');
  assert.equal(options.rangeChunkSize, 1048576);
  assert.equal(options.disableStream, true);
  assert.equal(options.disableAutoFetch, true);
  assert.equal(options.wasmUrl, './wasm/');
  assert.equal(options.useWasm, false);
  assert.equal(PDF_AUX_BASE, './wasm/');
});

test('ResponseException is classified as transport failure', () => {
  class ResponseException extends Error {
    constructor(message, status) {
      super(message);
      this.name = 'SomethingElse';
      this.status = status;
    }
  }
  const err = new ResponseException('Service unavailable', 503);
  assert.equal(
    describeLoadError(err, { ResponseException }),
    'Não consegui obter o PDF: HTTP 503'
  );
});

test('InvalidPDFException is classified as document failure', () => {
  class InvalidPDFException extends Error {}
  const err = new InvalidPDFException('bad xref');
  assert.equal(
    describeLoadError(err, { InvalidPDFException }),
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
