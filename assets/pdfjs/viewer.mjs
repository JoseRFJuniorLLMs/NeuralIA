// Visualizador PDF offline do NeuralIA. Mantem apenas uma pequena janela de
// paginas renderizadas em canvas para que documentos longos nao transformem
// memoria RAM numa colecao permanente de bitmaps.
import * as pdfjsLib from './pdf.mjs';

pdfjsLib.GlobalWorkerOptions.workerSrc = './pdf.worker.mjs';

const status = document.getElementById('status');
const hud = document.getElementById('hud');
const pagesEl = document.getElementById('pages');
const MAX_WIDTH = 960;
const KEEP_RADIUS = 3;
const OBSERVER_MARGIN = 900;

const slots = []; // { el, canvas, page, rendered, renderTask, epoch, baseWidth, baseHeight }
let doc = null;
let scale = 1;
let current = 1;
let renderEpoch = 0;

function fail(message) {
  status.textContent = message;
  status.classList.add('error');
  status.hidden = false;
}

function pixelRatio() {
  return Math.max(1, Math.min(window.devicePixelRatio || 1, 3));
}

function targetWidth() {
  return Math.min(MAX_WIDTH, Math.max(320, window.innerWidth - 48));
}

function setSlotSize(slot) {
  if (!slot.baseWidth || !slot.baseHeight) { return; }
  slot.el.style.width = Math.floor(slot.baseWidth * scale) + 'px';
  slot.el.style.height = Math.floor(slot.baseHeight * scale) + 'px';
}

async function ensurePage(index) {
  const slot = slots[index];
  if (!slot) { return null; }
  if (!slot.page) {
    slot.page = await doc.getPage(index + 1);
    const base = slot.page.getViewport({ scale: 1 });
    slot.baseWidth = base.width;
    slot.baseHeight = base.height;
    setSlotSize(slot);
  }
  return slot.page;
}

function release(index) {
  const slot = slots[index];
  if (!slot) { return; }
  if (slot.renderTask) {
    try { slot.renderTask.cancel(); } catch (_) {}
    slot.renderTask = null;
  }
  if (slot.rendered || slot.canvas.width || slot.canvas.height) {
    slot.canvas.width = 0;
    slot.canvas.height = 0;
    slot.canvas.removeAttribute('style');
  }
  slot.rendered = false;
  slot.epoch = -1;
  try { slot.page && slot.page.cleanup(); } catch (_) {}
}

async function render(index) {
  const slot = slots[index];
  if (!slot || slot.rendered || slot.renderTask) { return; }

  const epoch = renderEpoch;
  try {
    const page = await ensurePage(index);
    if (!page || epoch !== renderEpoch) { return; }

    const viewport = page.getViewport({ scale });
    const dpr = pixelRatio();
    slot.canvas.width = Math.max(1, Math.floor(viewport.width * dpr));
    slot.canvas.height = Math.max(1, Math.floor(viewport.height * dpr));
    slot.canvas.style.width = Math.floor(viewport.width) + 'px';
    slot.canvas.style.height = Math.floor(viewport.height) + 'px';
    slot.el.style.width = slot.canvas.style.width;
    slot.el.style.height = slot.canvas.style.height;

    const ctx = slot.canvas.getContext('2d', { alpha: false });
    const task = page.render({
      canvasContext: ctx,
      viewport,
      transform: [dpr, 0, 0, dpr, 0, 0]
    });
    slot.renderTask = task;
    slot.epoch = epoch;
    await task.promise;

    if (epoch === renderEpoch && slot.epoch === epoch) {
      slot.rendered = true;
    }
  } catch (err) {
    if (!err || err.name !== 'RenderingCancelledException') {
      console.error('pagina', index + 1, err);
    }
  } finally {
    if (slot && slot.epoch === epoch) {
      slot.renderTask = null;
    }
  }
}

const observer = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (entry.isIntersecting) {
      render(Number(entry.target.dataset.index));
    }
  }
}, { rootMargin: OBSERVER_MARGIN + 'px 0px' });

function evictFarPages() {
  const center = Math.max(0, current - 1);
  for (let i = 0; i < slots.length; i++) {
    if (Math.abs(i - center) > KEEP_RADIUS) {
      release(i);
    }
  }
}

function updateHud() {
  const top = window.scrollY + window.innerHeight * 0.35;
  let best = 1;
  for (let i = 0; i < slots.length; i++) {
    if (slots[i].el.offsetTop <= top) { best = i + 1; } else { break; }
  }
  current = best;
  hud.textContent = current + ' / ' + slots.length;
  evictFarPages();
}

function scrollToPage(number) {
  const slot = slots[Math.max(0, Math.min(slots.length, number) - 1)];
  if (!slot) { return; }
  window.scrollTo({ top: Math.max(0, slot.el.offsetTop - 24), behavior: 'smooth' });
}

window.__neuralia_next_page = function () {
  if (current < slots.length) { scrollToPage(current + 1); return true; }
  return false;
};

window.__neuralia_prev_page = function () {
  if (current > 1) { scrollToPage(current - 1); return true; }
  return false;
};

async function load() {
  let data;
  try {
    const response = await fetch('./document.pdf', { cache: 'no-store' });
    if (!response.ok) { throw new Error('HTTP ' + response.status); }
    data = await response.arrayBuffer();
  } catch (err) {
    fail('Não consegui obter o PDF: ' + err.message);
    return;
  }

  try {
    doc = await pdfjsLib.getDocument({ data }).promise;
  } catch (err) {
    fail('Este ficheiro não é um PDF que eu consiga abrir: ' + (err.message || err));
    return;
  }

  const first = await doc.getPage(1);
  const base = first.getViewport({ scale: 1 });
  scale = targetWidth() / base.width;

  for (let i = 0; i < doc.numPages; i++) {
    const el = document.createElement('div');
    el.className = 'page';
    el.dataset.index = String(i);

    const canvas = document.createElement('canvas');
    const num = document.createElement('span');
    num.className = 'num';
    num.textContent = String(i + 1);
    el.appendChild(canvas);
    el.appendChild(num);
    pagesEl.appendChild(el);

    const slot = {
      el,
      canvas,
      page: i === 0 ? first : null,
      rendered: false,
      renderTask: null,
      epoch: -1,
      baseWidth: base.width,
      baseHeight: base.height
    };
    slots.push(slot);
    setSlotSize(slot);
    observer.observe(el);
  }

  status.hidden = true;
  hud.hidden = false;
  updateHud();
  render(0);
  document.title = 'NeuralIA · PDF · ' + doc.numPages + ' páginas';
}

window.addEventListener('scroll', updateHud, { passive: true });

let resizeTimer = 0;
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    if (!doc || !slots[0] || !slots[0].page) { return; }

    renderEpoch++;
    scale = targetWidth() / slots[0].page.getViewport({ scale: 1 }).width;

    for (let i = 0; i < slots.length; i++) {
      release(i);
      setSlotSize(slots[i]);
    }

    const firstVisible = Math.max(0, current - 1 - KEEP_RADIUS);
    const lastVisible = Math.min(slots.length - 1, current - 1 + KEEP_RADIUS);
    for (let i = firstVisible; i <= lastVisible; i++) {
      render(i);
    }
    updateHud();
  }, 120);
});

load();
