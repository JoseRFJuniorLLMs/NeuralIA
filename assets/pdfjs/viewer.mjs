// Visualizador PDF offline do NeuralIA. Documentos longos usam um placeholder
// barato por pagina (da a barra de scroll certa), enquanto canvas/bitmap e
// estado de render sao libertados longe da pagina actual. O PDF.js conserva
// internamente os PDFPageProxy ja pedidos ate destruir o documento, portanto a
// garantia de memoria limitada aqui aplica-se aos recursos pesados de render,
// nao ao numero de proxies. Os bytes chegam por ranges em vez de o ficheiro
// inteiro ser copiado para o worker. A geometria e calculada, nao lida do DOM,
// para o scroll nao forcar layout a cada evento.
import * as pdfjsLib from './pdf.mjs';

pdfjsLib.GlobalWorkerOptions.workerSrc = './pdf.worker.mjs';

export function describeLoadError(err) {
  const name = (err && err.name) || '';
  const text = (err && err.message) || String(err);

  if (
    (typeof pdfjsLib.ResponseException === 'function' && err instanceof pdfjsLib.ResponseException) ||
    name === 'ResponseException'
  ) {
    const status = Number.isFinite(err && err.status) ? ' HTTP ' + err.status : '';
    return 'Não consegui obter o PDF:' + status + (status ? '' : ' ' + text);
  }

  if (
    (typeof pdfjsLib.PasswordException === 'function' && err instanceof pdfjsLib.PasswordException) ||
    name === 'PasswordException'
  ) {
    return 'Este PDF está protegido por senha e o NeuralIA ainda não consegue abri-lo.';
  }

  if (
    (typeof pdfjsLib.InvalidPDFException === 'function' && err instanceof pdfjsLib.InvalidPDFException) ||
    name === 'InvalidPDFException'
  ) {
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
  element.style.display = 'grid';
  element.classList.toggle('error', error);
}

export function hideStatus(element) {
  element.hidden = true;
  // #status tem display:grid em CSS autoral; portanto hidden sozinho pode ser
  // sobreposto. O estilo inline fecha o overlay de forma inequívoca.
  element.style.display = 'none';
  element.classList.remove('error');
}

if (typeof document !== 'undefined') {
const status = document.getElementById('status');
const hud = document.getElementById('hud');
const pagesEl = document.getElementById('pages');
const MAX_WIDTH = 960;
// Paginas com canvas vivo a volta da actual; a referencia local ao proxy da
// pagina sobrevive um pouco mais (EVICT_RADIUS) para o vaivem do scroll nao
// repetir o getPage. O PDF.js pode manter o mesmo proxy no cache interno.
const KEEP_RADIUS = 3;
const EVICT_RADIUS = KEEP_RADIUS + 2;
// Faixa alem do viewport em que o observer pede render. A evicao respeita a
// mesma faixa: o que o observer acabou de pedir nao pode ser apagado logo a
// seguir, porque ele so volta a disparar quando a interseccao mudar.
const BAND = 1200;
const PROBE_PAGES = 8;
const RANGE_CHUNK = 1048576;

const slots = [];
// tops[i] e o offsetTop do placeholder i e heights[i] a altura que lhe
// escrevemos; so top0 e gap vem de uma medicao unica (load e resize).
const tops = [];
const heights = [];
let top0 = 0;
let gap = 0;
// Indices com canvas e/ou proxy vivos: a evicao percorre so estes.
const live = new Set();
let doc = null;
let scale = 1;
let current = 1;
let baseWidth = 1;
let defaultSize = null;
let renderGeneration = 0;
let resizeTimer = 0;
let hudQueued = false;
// Leitura em voz alta (read-aloud.js, carregado antes deste modulo).
let readAloud = null;

function pixelRatio() {
  return Math.max(1, Math.min(window.devicePixelRatio || 1, 3));
}

function fail(message) {
  showStatus(status, message, true);
}

function targetWidth() {
  return Math.min(MAX_WIDTH, Math.max(320, window.innerWidth - 48));
}

// Escreve no placeholder (e no canvas, se existir) o tamanho da pagina a
// escala actual; devolve true se a altura mudou, porque os tops abaixo dela
// ficam errados.
function layoutSlot(index) {
  const slot = slots[index];
  const size = slot.size || defaultSize;
  const w = Math.floor(size.width * scale);
  const h = Math.floor(size.height * scale);
  if (slot.cssWidth !== w) {
    slot.cssWidth = w;
    slot.el.style.width = w + 'px';
  }
  if (slot.canvas) {
    slot.canvas.style.width = w + 'px';
    slot.canvas.style.height = h + 'px';
  }
  if (heights[index] === h) return false;
  heights[index] = h;
  slot.el.style.height = h + 'px';
  return true;
}

function rebuildTops(from) {
  for (let i = from; i < slots.length; i++) {
    tops[i] = i === 0 ? top0 : tops[i - 1] + heights[i - 1] + gap;
  }
}

// Unica leitura de layout: onde comeca o primeiro placeholder e o gap do
// flex. Tudo o resto e aritmetica sobre alturas que nos proprios escrevemos.
function measureLayout() {
  top0 = slots[0].el.getBoundingClientRect().top + window.scrollY;
  gap = parseFloat(getComputedStyle(pagesEl).rowGap) || 0;
  rebuildTops(0);
}

function layoutAll() {
  // A camada de texto do PDF.js dimensiona-se por esta variavel CSS.
  pagesEl.style.setProperty('--scale-factor', String(scale));
  for (let i = 0; i < slots.length; i++) layoutSlot(i);
  measureLayout();
}

// Tamanho real (escala 1) de uma pagina: corrige o placeholder e os tops a
// partir dela. Um PDF com orientacoes mistas so salta ate a pagina ser vista.
function setSize(index, size) {
  const slot = slots[index];
  if (slot.size && slot.size.width === size.width && slot.size.height === size.height) return;
  slot.size = size;
  if (layoutSlot(index)) rebuildTops(index + 1);
}

function attachCanvas(slot, index) {
  const canvas = document.createElement('canvas');
  canvas.style.width = slot.cssWidth + 'px';
  canvas.style.height = heights[index] + 'px';
  slot.el.insertBefore(canvas, slot.el.firstChild);
  slot.canvas = canvas;
  return canvas;
}

// A camada de texto vive e morre com o canvas: mudar a escala ou afastar a
// pagina larga as duas.
function releaseTextLayer(slot) {
  if (slot.textLayer) slot.textLayer.cancel();
  if (slot.textEl) slot.textEl.remove();
  slot.textLayer = null;
  slot.textEl = null;
  slot.textReady = false;
}

// width = 0 liberta o bitmap ja; tirar o no do DOM poupa o resto.
function releaseCanvas(slot) {
  releaseTextLayer(slot);
  const canvas = slot.canvas;
  if (!canvas) return;
  canvas.width = 0;
  canvas.height = 0;
  canvas.remove();
  slot.canvas = null;
  slot.rendered = false;
}

// O texto de uma pagina (getTextContent) serve a camada de texto e a leitura
// em voz alta: pede-se uma vez e partilha-se. Os textDivs da TextLayer ficam
// paralelos a estes itens, que e o que a leitura usa para realcar a frase.
function textContentOf(index) {
  const slot = slots[index];
  if (!slot || !doc) return Promise.resolve(null);
  if (!slot.text) {
    live.add(index);
    const page = slot.page
      ? Promise.resolve(slot.page)
      : doc.getPage(index + 1).then((proxy) => (slot.page = slot.page || proxy));
    const pending = page.then((proxy) => proxy.getTextContent());
    slot.text = pending;
    pending.catch(() => {
      if (slot.text === pending) slot.text = null;
    });
  }
  return slot.text;
}

async function renderTextLayer(index, viewport, generation) {
  const slot = slots[index];
  if (!slot || slot.textLayer || typeof pdfjsLib.TextLayer !== 'function') return;
  let content;
  try {
    content = await textContentOf(index);
  } catch (err) {
    console.error('texto da pagina', index + 1, err);
    return;
  }
  if (!content || generation !== renderGeneration || !slot.canvas || slot.textLayer) return;
  const container = document.createElement('div');
  container.className = 'textLayer';
  slot.el.appendChild(container);
  const layer = new pdfjsLib.TextLayer({ textContentSource: content, container, viewport });
  slot.textLayer = layer;
  slot.textEl = container;
  try {
    await layer.render();
  } catch (err) {
    if (!err || err.name !== 'AbortException') console.error('texto da pagina', index + 1, err);
    return;
  }
  if (slot.textLayer !== layer) return;
  slot.textReady = true;
  if (readAloud) readAloud.pageReady(index);
}

function cleanupPage(slot) {
  if (slot.page && typeof slot.page.cleanup === 'function') slot.page.cleanup();
}

async function render(index) {
  const slot = slots[index];
  if (!slot || slot.rendered || slot.rendering || !doc) return;

  const generation = renderGeneration;
  slot.rendering = true;
  slot.generation = generation;
  live.add(index);
  try {
    const page = slot.page || (slot.page = await doc.getPage(index + 1));
    if (generation !== renderGeneration) return;

    const base = page.getViewport({ scale: 1 });
    setSize(index, { width: base.width, height: base.height });
    const viewport = page.getViewport({ scale });
    const dpr = pixelRatio();
    const canvas = slot.canvas || attachCanvas(slot, index);
    canvas.width = Math.max(1, Math.floor(viewport.width * dpr));
    canvas.height = Math.max(1, Math.floor(viewport.height * dpr));

    const ctx = canvas.getContext('2d', { alpha: false });
    const task = page.render({
      canvasContext: ctx,
      viewport,
      transform: [dpr, 0, 0, dpr, 0, 0]
    });
    slot.task = task;
    await task.promise;
    if (generation === renderGeneration) {
      slot.rendered = true;
      renderTextLayer(index, viewport, generation);
    }
  } catch (err) {
    if (!err || err.name !== 'RenderingCancelledException') {
      console.error('pagina', index + 1, err);
    }
  } finally {
    if (slot.generation === generation) {
      slot.rendering = false;
      slot.task = null;
    }
  }
}

// So visita os indices vivos, nunca todas as paginas. O canvas fica enquanto
// a pagina estiver a KEEP_RADIUS da actual ou dentro da faixa do observer.
// Fora de EVICT_RADIUS largamos a referencia local e chamamos cleanup() para
// libertar estado pesado de render; o WorkerTransport do PDF.js pode continuar
// a reter o PDFPageProxy no cache interno. A pagina 1 fica referenciada porque
// e a base da escala e a primeira a voltar a mostrar.
function evictFarPages() {
  const center = current - 1;
  const bandTop = window.scrollY - BAND;
  const bandBottom = window.scrollY + window.innerHeight + BAND;
  for (const i of live) {
    const slot = slots[i];
    if (slot.rendering) continue;
    const distance = Math.abs(i - center);
    if (distance > KEEP_RADIUS && !(tops[i] + heights[i] >= bandTop && tops[i] <= bandBottom)) {
      if (slot.canvas) {
        releaseCanvas(slot);
        cleanupPage(slot);
      }
      if (distance > EVICT_RADIUS && i !== 0 && slot.page) {
        cleanupPage(slot);
        slot.page = null;
      }
      if (distance > EVICT_RADIUS) slot.text = null;
    }
    if (!slot.canvas && !slot.page && !slot.text) live.delete(i);
  }
}

const observer = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (entry.isIntersecting) render(Number(entry.target.dataset.index));
  }
}, { rootMargin: BAND + 'px 0px' });

// Maior indice cujo topo esta acima de y: pesquisa binaria em tops.
function pageAt(y) {
  let lo = 0;
  let hi = tops.length - 1;
  let best = 0;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (tops[mid] <= y) {
      best = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return best;
}

function updateHud() {
  if (!slots.length) return;
  const index = pageAt(window.scrollY + window.innerHeight * 0.35);
  current = index + 1;
  hud.textContent = current + ' / ' + slots.length;
  for (let i = Math.max(0, index - 1); i <= Math.min(slots.length - 1, index + 1); i++) render(i);
  evictFarPages();
}

// Um scroll dispara varios eventos por frame; basta um updateHud por frame.
function scheduleHud() {
  if (hudQueued) return;
  hudQueued = true;
  requestAnimationFrame(() => {
    hudQueued = false;
    updateHud();
  });
}

// Pede render a tudo o que esta na faixa do observer, pelos tops calculados:
// o observer nao volta a disparar para o que ja estava a intersectar.
function renderBand() {
  const bottom = window.scrollY + window.innerHeight + BAND;
  for (let i = pageAt(window.scrollY - BAND); i < slots.length && tops[i] <= bottom; i++) render(i);
}

function scrollToPage(number) {
  const index = Math.max(0, Math.min(slots.length, number) - 1);
  if (!slots[index]) return;
  window.scrollTo({ top: Math.max(0, tops[index] - 24), behavior: 'smooth' });
}

window.__neuralia_next_page = function () {
  if (current < slots.length) { scrollToPage(current + 1); return true; }
  return false;
};
window.__neuralia_prev_page = function () {
  if (current > 1) { scrollToPage(current - 1); return true; }
  return false;
};

// Dimensoes (escala 1) das primeiras paginas; getPage e getViewport nao
// renderizam nada. Uma falha individual deixa a pagina com o placeholder
// por omissao.
async function probeSizes(first) {
  const count = Math.min(PROBE_PAGES, doc.numPages);
  const pages = [];
  for (let i = 0; i < count; i++) {
    pages.push(i === 0 ? Promise.resolve(first) : doc.getPage(i + 1).catch(() => null));
  }
  return (await Promise.all(pages)).map((page) => {
    if (!page) return null;
    const v = page.getViewport({ scale: 1 });
    return { width: v.width, height: v.height };
  });
}

function medianSize(sizes) {
  const found = sizes.filter(Boolean).sort((a, b) => a.height - b.height);
  return found[found.length >> 1];
}

async function load() {
  try {
    // url + rangeChunkSize: o PDF.js pede so os bytes de que precisa (Range)
    // e passa-os ao worker sem copia; se o servidor nao anunciar
    // Accept-Ranges cai sozinho no download completo. disableStream impede o
    // pedido inicial de continuar a puxar o ficheiro inteiro depois dos
    // cabecalhos, e disableAutoFetch impede o worker de ir buscar em fundo os
    // chunks que ninguem pediu; sem os dois, o PDF acabava todo em memoria.
    doc = await pdfjsLib.getDocument({
      url: './document.pdf',
      rangeChunkSize: RANGE_CHUNK,
      disableStream: true,
      disableAutoFetch: true,
      cMapUrl: './cmaps/',
      cMapPacked: true,
      standardFontDataUrl: './standard_fonts/',
      wasmUrl: './wasm/',
      iccUrl: './icc/',
      useWasm: true,
      useWorkerFetch: true
    }).promise;
  } catch (err) {
    fail(describeLoadError(err));
    return;
  }

  let first;
  let known;
  try {
    first = await doc.getPage(1);
    baseWidth = first.getViewport({ scale: 1 }).width;
    scale = targetWidth() / baseWidth;
    // A mediana das primeiras paginas e o placeholder por omissao: menos
    // saltos que assumir que todas tem o tamanho da pagina 1.
    known = await probeSizes(first);
    defaultSize = medianSize(known);
  } catch (err) {
    fail(describeLoadError(err));
    return;
  }

  // Placeholders para todas as paginas (baratos e dao a barra de scroll
  // certa); o canvas so nasce em render().
  const fragment = document.createDocumentFragment();
  for (let i = 0; i < doc.numPages; i++) {
    const el = document.createElement('div');
    el.className = 'page';
    el.dataset.index = String(i);
    const num = document.createElement('span');
    num.className = 'num';
    num.textContent = String(i + 1);
    el.appendChild(num);
    fragment.appendChild(el);

    slots.push({
      el, canvas: null, page: i === 0 ? first : null, size: known[i] || null, cssWidth: 0,
      rendered: false, rendering: false, task: null, generation: renderGeneration,
      text: null, textLayer: null, textEl: null, textReady: false
    });
  }
  pagesEl.appendChild(fragment);
  live.add(0);
  layoutAll();
  for (const slot of slots) observer.observe(slot.el);

  hideStatus(status);
  hud.hidden = false;
  updateHud();
  document.title = 'NeuralIA · PDF · ' + doc.numPages + ' páginas';
  attachReadAloud();
}

// Liga o read-aloud.js a este documento. Ele so ve paginas por indice: o
// texto (getTextContent), os spans da camada de texto ja desenhada e um
// pedido para trazer a pagina ao ecra.
function attachReadAloud() {
  const api = window.NeuralIAReadAloud;
  if (!api || readAloud) return;
  readAloud = api.attachPdf({
    window,
    lang: document.documentElement.lang,
    pageCount: () => slots.length,
    currentPage: () => current - 1,
    textContent: (i) => textContentOf(i).then((content) => (content ? content.items : [])),
    textDivs: (i) => {
      const slot = slots[i];
      return slot && slot.textReady && slot.textLayer ? slot.textLayer.textDivs : null;
    },
    pageOf: (el) => {
      const pageEl = el && typeof el.closest === 'function' ? el.closest('.page') : null;
      return pageEl ? Number(pageEl.dataset.index) : null;
    },
    showPage: (i) => scrollToPage(i + 1)
  });
}

// Nova escala: cancela o que estava a desenhar, larga todos os canvases (a
// escala antiga ja nao serve), recalcula a geometria e volta a pedir so o
// que esta na faixa visivel.
function relayout() {
  if (!doc || !slots.length) return;

  renderGeneration++;
  for (const i of live) {
    const slot = slots[i];
    if (slot.task && typeof slot.task.cancel === 'function') slot.task.cancel();
    slot.rendering = false;
    slot.task = null;
    releaseCanvas(slot);
    if (!slot.page && !slot.text) live.delete(i);
  }

  scale = targetWidth() / baseWidth;
  layoutAll();
  updateHud();
  renderBand();
}

window.addEventListener('scroll', scheduleHud, { passive: true });
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(relayout, 120);
});

load();
}
