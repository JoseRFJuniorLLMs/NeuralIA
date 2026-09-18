// Visualizador PDF offline do NeuralIA. Mantem uma janela pequena de canvases
// renderizados para que documentos longos nao consumam memoria sem limite.
import * as pdfjsLib from './pdf.mjs';

pdfjsLib.GlobalWorkerOptions.workerSrc = './pdf.worker.mjs';

const status = document.getElementById('status');
const hud = document.getElementById('hud');
const pagesEl = document.getElementById('pages');
const MAX_WIDTH = 960;
const KEEP_RADIUS = 3;

const slots = [];
let doc = null;
let scale = 1;
let current = 1;
let renderGeneration = 0;
let resizeTimer = 0;

function pixelRatio() {
  return Math.max(1, Math.min(window.devicePixelRatio || 1, 3));
}

function fail(message) {
  status.textContent = message;
  status.classList.add('error');
  status.hidden = false;
}

function targetWidth() {
  return Math.min(MAX_WIDTH, Math.max(320, window.innerWidth - 48));
}

function applyViewport(slot, viewport) {
  slot.canvas.style.width = Math.floor(viewport.width) + 'px';
  slot.canvas.style.height = Math.floor(viewport.height) + 'px';
  slot.el.style.width = slot.canvas.style.width;
  slot.el.style.height = slot.canvas.style.height;
}

async function render(index) {
  const slot = slots[index];
  if (!slot || slot.rendered || slot.rendering || !doc) return;

  const generation = renderGeneration;
  slot.rendering = true;
  slot.generation = generation;
  try {
    const page = slot.page || (slot.page = await doc.getPage(index + 1));
    if (generation !== renderGeneration) return;

    const viewport = page.getViewport({ scale });
    const dpr = pixelRatio();
    applyViewport(slot, viewport);
    slot.canvas.width = Math.max(1, Math.floor(viewport.width * dpr));
    slot.canvas.height = Math.max(1, Math.floor(viewport.height * dpr));

    const ctx = slot.canvas.getContext('2d', { alpha: false });
    const task = page.render({
      canvasContext: ctx,
      viewport,
      transform: [dpr, 0, 0, dpr, 0, 0]
    });
    slot.task = task;
    await task.promise;
    if (generation === renderGeneration) slot.rendered = true;
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

function evictFarPages() {
  const center = current - 1;
  for (let i = 0; i < slots.length; i++) {
    const slot = slots[i];
    if (Math.abs(i - center) <= KEEP_RADIUS || slot.rendering || !slot.rendered) continue;
    slot.canvas.width = 0;
    slot.canvas.height = 0;
    slot.rendered = false;
    if (slot.page && typeof slot.page.cleanup === 'function') slot.page.cleanup();
  }
}

const observer = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (entry.isIntersecting) render(Number(entry.target.dataset.index));
  }
}, { rootMargin: '1200px 0px' });

function updateHud() {
  const top = window.scrollY + window.innerHeight * 0.35;
  let best = 1;
  for (let i = 0; i < slots.length; i++) {
    if (slots[i].el.offsetTop <= top) best = i + 1;
    else break;
  }
  current = best;
  hud.textContent = current + ' / ' + slots.length;
  for (let i = Math.max(0, current - 2); i <= Math.min(slots.length - 1, current); i++) render(i);
  evictFarPages();
}

function scrollToPage(number) {
  const slot = slots[Math.max(0, Math.min(slots.length, number) - 1)];
  if (!slot) return;
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
    const response = await fetch('./document.pdf');
    if (!response.ok) throw new Error('HTTP ' + response.status);
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
    el.style.width = Math.floor(base.width * scale) + 'px';
    el.style.height = Math.floor(base.height * scale) + 'px';

    const canvas = document.createElement('canvas');
    const num = document.createElement('span');
    num.className = 'num';
    num.textContent = String(i + 1);
    el.appendChild(canvas);
    el.appendChild(num);
    pagesEl.appendChild(el);

    slots.push({
      el, canvas, page: i === 0 ? first : null,
      rendered: false, rendering: false, task: null, generation: renderGeneration
    });
    observer.observe(el);
  }

  status.hidden = true;
  hud.hidden = false;
  updateHud();
  document.title = 'NeuralIA · PDF · ' + doc.numPages + ' páginas';
}

window.addEventListener('scroll', updateHud, { passive: true });
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    if (!doc || !slots[0] || !slots[0].page) return;

    renderGeneration++;
    for (const slot of slots) {
      if (slot.task && typeof slot.task.cancel === 'function') slot.task.cancel();
      slot.rendering = false;
      slot.rendered = false;
      slot.task = null;
    }

    scale = targetWidth() / slots[0].page.getViewport({ scale: 1 }).width;
    for (const slot of slots) {
      if (slot.page) applyViewport(slot, slot.page.getViewport({ scale }));
    }
    updateHud();

    slots.forEach((slot, index) => {
      const rect = slot.el.getBoundingClientRect();
      if (rect.bottom > -1200 && rect.top < window.innerHeight + 1200) render(index);
    });
  }, 120);
});

load();
