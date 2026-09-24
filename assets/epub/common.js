'use strict';
// NeuralIA — utilitários partilhados pela biblioteca e pelo leitor de EPUB.
// Ficheiro do próprio NeuralIA, servido pela origem neuralia-epub com
// `script-src 'self'`: nada aqui é de terceiros.
(function (root) {
  const ORIGIN = 'http://neuralia-epub.localhost';
  const ID_RE = /^[0-9a-f]{16,64}$/;
  const COMBINING = /[\u0300-\u036f]/g;

  function isBookId(id) {
    return typeof id === 'string' && ID_RE.test(id);
  }

  // Minúsculas e sem acentos, com o mapa de volta ao texto original: para
  // cada unidade do texto dobrado, onde começa e onde acaba o caractere de
  // origem. Assim uma pesquisa por "acao" encontra "Ação" e sabe marcar
  // exatamente "Ação".
  function foldWithMap(text) {
    const source = String(text == null ? '' : text);
    const out = [];
    const starts = [];
    const ends = [];
    let i = 0;
    while (i < source.length) {
      const cp = source.codePointAt(i);
      const ch = String.fromCodePoint(cp);
      const next = i + ch.length;
      const base = ch.normalize('NFD').replace(COMBINING, '').toLowerCase();
      for (let k = 0; k < base.length; k++) {
        out.push(base[k]);
        starts.push(i);
        ends.push(next);
      }
      i = next;
    }
    return { text: out.join(''), starts, ends };
  }

  function fold(text) {
    return foldWithMap(text).text;
  }

  function clamp(value, low, high) {
    const number = Number(value);
    if (!Number.isFinite(number)) return low;
    return Math.min(high, Math.max(low, number));
  }

  function percentLabel(value) {
    return Math.round(clamp(value, 0, 1) * 100) + '%';
  }

  // Só o canal do próprio WebView (`window.ipc`, do wry). A página nunca vê
  // a capability das páginas remotas, e o lado nativo confere a origem.
  function post(message) {
    const ipc = root.ipc;
    if (!ipc || typeof ipc.postMessage !== 'function') return false;
    ipc.postMessage(JSON.stringify(message));
    return true;
  }

  async function getJson(path) {
    const response = await root.fetch(path, { cache: 'no-store', credentials: 'same-origin' });
    let body = null;
    try {
      body = await response.json();
    } catch (_) {
      body = null;
    }
    if (!response.ok) {
      const message = body && typeof body.error === 'string'
        ? body.error
        : 'Não foi possível carregar (' + response.status + ').';
      throw new Error(message);
    }
    return body;
  }

  function el(doc, tag, className, text) {
    const node = doc.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = String(text);
    return node;
  }

  // Avisos do lado nativo (livro adicionado, marcador gravado, erro). O
  // nativo chama `window.neuraliaEpubNotice({...})` com JSON; aqui só se
  // distribui para quem se inscreveu.
  const listeners = [];
  function onNotice(listener) {
    listeners.push(listener);
  }
  function deliverNotice(notice) {
    if (!notice || typeof notice !== 'object') return;
    for (const listener of listeners.slice()) {
      try {
        listener(notice);
      } catch (error) {
        if (root.console) root.console.error(error);
      }
    }
  }
  root.neuraliaEpubNotice = deliverNotice;

  root.NeuraliaEpub = Object.freeze({
    ORIGIN,
    isBookId,
    fold,
    foldWithMap,
    clamp,
    percentLabel,
    post,
    getJson,
    el,
    onNotice,
  });
})(globalThis);
