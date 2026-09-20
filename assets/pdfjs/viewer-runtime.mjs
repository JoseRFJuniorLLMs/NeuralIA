// Lógica pura do visualizador PDF do NeuralIA.
// Fica separada do DOM para poder ser testada em Node sem arrancar WebView2.

export const PDF_AUX_BASE = './wasm/';

export function makePdfLoadOptions(url, rangeChunkSize) {
  return {
    url,
    rangeChunkSize,
    disableStream: true,
    disableAutoFetch: true,

    // PDF.js 6.x tenta WASM por omissão. O pacote NeuralIA não distribuía os
    // binários WASM, o que fazia o worker tentar URLs "null*.wasm". Usamos os
    // fallbacks JS oficiais e vendorizados da mesma versão do PDF.js.
    wasmUrl: PDF_AUX_BASE,
    useWasm: false
  };
}

function isInstanceOf(err, Type) {
  return typeof Type === 'function' && err instanceof Type;
}

export function describeLoadError(err, pdfjs = {}) {
  const name = (err && err.name) || '';
  const text = (err && err.message) || String(err);

  if (isInstanceOf(err, pdfjs.ResponseException) || name === 'ResponseException') {
    const status = Number.isFinite(err && err.status) ? ' HTTP ' + err.status : '';
    return 'Não consegui obter o PDF:' + status + (status ? '' : ' ' + text);
  }

  if (isInstanceOf(err, pdfjs.InvalidPDFException) || name === 'InvalidPDFException') {
    return 'Este ficheiro não é um PDF que eu consiga abrir: ' + text;
  }

  if (
    name === 'UnknownErrorException' &&
    /(?:failed to fetch|network(?:error)?|load failed|fetch)/i.test(text)
  ) {
    return 'Não consegui obter o PDF: ' + text;
  }

  return 'Este ficheiro não é um PDF que eu consiga abrir: ' + text;
}

export function showStatus(element, message, error = false) {
  if (message !== undefined && message !== null) element.textContent = message;
  element.hidden = false;
  // CSS autoral pode sobrepor [hidden] { display:none }. Controlar display
  // explicitamente evita o overlay invisivelmente "preso" sobre o documento.
  element.style.display = 'grid';
  element.classList.toggle('error', error);
}

export function hideStatus(element) {
  element.hidden = true;
  element.style.display = 'none';
  element.classList.remove('error');
}
