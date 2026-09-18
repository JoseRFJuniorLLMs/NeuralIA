// O visualizador e NOSSO: corre na nossa origem, com o nosso script. E por
// isso que a rolagem automatica e o teclado funcionam aqui e nao no
// visualizador do Edge, que vive noutro processo e nao nos obedece.
//
// Fica num ficheiro proprio, e nao inline, para a CSP da pagina poder ser
// `script-src 'self'` sem `'unsafe-inline'`.
import * as pdfjsLib from './pdf.mjs';

pdfjsLib.GlobalWorkerOptions.workerSrc = './pdf.worker.mjs';

const status = document.getElementById('status');
const hud = document.getElementById('hud');
const pagesEl = document.getElementById('pages');
const MAX_WIDTH = 960;
const dpr = Math.max(1, Math.min(window.devicePixelRatio || 1, 3));

const slots = []; // { el, canvas, page, rendered, rendering }
let doc = null;
let scale = 1;
let current = 1;

function fail(message) {
  status.textContent = message;
  status.classList.add('error');
  status.hidden = false;
}

function targetWidth() {
  return Math.min(MAX_WIDTH, Math.max(320, window.innerWidth - 48));
}

async function render(index) {
  const slot = slots[index];
  if (!slot || slot.rendered || slot.rendering) { return; }
  slot.rendering = true;
  try {
    const page = slot.page || (slot.page = await doc.getPage(index + 1));
    const viewport = page.getViewport({ scale });
    slot.canvas.width = Math.floor(viewport.width * dpr);
    slot.canvas.height = Math.floor(viewport.height * dpr);
    slot.canvas.style.width = Math.floor(viewport.width) + 'px';
    slot.canvas.style.height = Math.floor(viewport.height) + 'px';
    slot.el.style.width = slot.canvas.style.width;
    slot.el.style.height = slot.canvas.style.height;
    const ctx = slot.canvas.getContext('2d', { alpha: false });
    await page.render({ canvasContext: ctx, viewport, transform: [dpr, 0, 0, dpr, 0, 0] }).promise;
    slot.rendered = true;
  } catch (err) {
    console.error('pagina', index + 1, err);
  } finally {
    slot.rendering = false;
  }
}

const observer = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (entry.isIntersecting) { render(Number(entry.target.dataset.index)); }
  }
}, { rootMargin: '1400px 0px' });

function updateHud() {
  const top = window.scrollY + window.innerHeight * 0.35;
  let best = 1;
  for (let i = 0; i < slots.length; i++) {
    if (slots[i].el.offsetTop <= top) { best = i + 1; } else { break; }
  }
  current = best;
  hud.textContent = current + ' / ' + slots.length;
}

function scrollToPage(number) {
  const slot = slots[Math.max(0, Math.min(slots.length, number) - 1)];
  if (!slot) { return; }
  window.scrollTo({ top: Math.max(0, slot.el.offsetTop - 24), behavior: 'smooth' });
}

// A aplicacao chama isto a cada 30s quando a rolagem automatica esta ligada:
// uma pagina inteira de cada vez, e para na ultima.
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
    el.style.width = Math.floor(base.width * scale) + 'px';
    el.style.height = Math.floor(base.height * scale) + 'px';
    const canvas = document.createElement('canvas');
    const num = document.createElement('span');
    num.className = 'num';
    num.textContent = String(i + 1);
    el.appendChild(canvas);
    el.appendChild(num);
    pagesEl.appendChild(el);
    slots.push({ el, canvas, page: i === 0 ? first : null, rendered: false, rendering: false });
    observer.observe(el);
  }

  status.hidden = true;
  hud.hidden = false;
  updateHud();
  document.title = 'NeuralIA · PDF · ' + doc.numPages + ' páginas';
}

window.addEventListener('scroll', updateHud, { passive: true });
window.addEventListener('resize', () => {
  if (!doc || !slots[0] || !slots[0].page) { return; }
  scale = targetWidth() / slots[0].page.getViewport({ scale: 1 }).width;
  for (const slot of slots) { slot.rendered = false; }
  slots.forEach((slot, index) => {
    const rect = slot.el.getBoundingClientRect();
    if (rect.bottom > -1400 && rect.top < window.innerHeight + 1400) { render(index); }
  });
});

load();
