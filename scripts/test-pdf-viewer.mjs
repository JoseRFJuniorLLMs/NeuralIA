import assert from 'node:assert/strict';
import * as pdfjsLib from '../assets/pdfjs/pdf.mjs';

const viewerModule =
  process.env.NEURALIA_PDF_VIEWER_MODULE ||
  new URL('../assets/pdfjs/viewer.mjs', import.meta.url).href;

const {
  chunkPdfText, describeLoadError, extractPdfText, hideStatus, normalizePdfTextItems, showStatus
} = await import(viewerModule);

let passed = 0;

async function test(name, fn) {
  try {
    await fn();
    passed++;
    console.log('ok - ' + name);
  } catch (err) {
    console.error('not ok - ' + name);
    throw err;
  }
}

await test('ResponseException is classified as transport failure', () => {
  const err = new pdfjsLib.ResponseException('Service unavailable', 503, false);
  assert.equal(describeLoadError(err), 'Não consegui obter o PDF: HTTP 503');
});

await test('InvalidPDFException is classified as document failure', () => {
  const err = new pdfjsLib.InvalidPDFException('bad xref');
  assert.equal(
    describeLoadError(err),
    'Este ficheiro não é um PDF que eu consiga abrir: bad xref'
  );
});

await test('PasswordException is classified as unsupported password protection', () => {
  const err = new pdfjsLib.PasswordException('Password required', pdfjsLib.PasswordResponses.NEED_PASSWORD);
  assert.equal(
    describeLoadError(err),
    'Este PDF está protegido por senha e o NeuralIA ainda não consegue abri-lo.'
  );
});

await test('wrapped fetch failure remains a transport failure', () => {
  const err = { name: 'UnknownErrorException', message: 'Failed to fetch document' };
  assert.equal(
    describeLoadError(err),
    'Não consegui obter o PDF: Failed to fetch document'
  );
});

await test('unknown parser failure is not mislabeled as network', () => {
  const err = { name: 'UnknownErrorException', message: 'bad object stream' };
  assert.equal(
    describeLoadError(err),
    'Este ficheiro não é um PDF que eu consiga abrir: bad object stream'
  );
});

await test('hideStatus defeats author CSS display:grid', () => {
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


await test('PDF text normalization and chunking stay bounded', () => {
  assert.equal(
    normalizePdfTextItems([{ str: '  um ' }, { str: '' }, { str: ' dois\n três ' }]),
    'um dois três'
  );
  assert.deepEqual(chunkPdfText('abcdef', 2), ['ab', 'cd', 'ef']);
  assert.deepEqual(chunkPdfText('😀a😀b', 2), ['😀a', '😀b']);
});

await test('PDF extraction emits authenticated-sized chunks and a final marker', async () => {
  const pdf = {
    numPages: 3,
    async getPage(page) {
      return {
        async getTextContent() {
          return { items: [{ str: `pagina ${page}` }, { str: 'conteudo' }] };
        }
      };
    }
  };
  const emitted = [];
  const result = await extractPdfText(pdf, (payload) => emitted.push(payload), {
    maxPages: 2,
    maxChars: 100,
    chunkChars: 5
  });
  assert.equal(result.truncated, true);
  assert.equal(emitted.at(-1).done, true);
  assert.equal(emitted.at(-1).truncated, true);
  assert(emitted.slice(0, -1).every((payload) => Array.from(payload.text).length <= 5));
  assert.deepEqual([...new Set(emitted.slice(0, -1).map((payload) => payload.page))], [1, 2]);
});

await test('PDF extraction hard-stops at the text budget', async () => {
  const pdf = {
    numPages: 1,
    async getPage() {
      return { async getTextContent() { return { items: [{ str: 'abcdefghij' }] }; } };
    }
  };
  const emitted = [];
  await extractPdfText(pdf, (payload) => emitted.push(payload), {
    maxPages: 1,
    maxChars: 4,
    chunkChars: 3
  });
  assert.equal(emitted.filter((payload) => !payload.done).map((p) => p.text).join(''), 'abcd');
  assert.equal(emitted.at(-1).truncated, true);
});

console.log(`pdf-viewer runtime: ${passed} tests passed`);
