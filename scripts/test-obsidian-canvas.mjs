import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

// Execute the shipped asset with a measurable canvas, including hidden layout
// and asynchronous animation. Assertions inspect actual painted screen points.
function panel() {
  let rect = { width: 0, height: 0, left: 0, top: 0 };
  let transform = { x: 0, y: 0, scale: 1 };
  const stack = [], frames = [], pending = [], observers = [];
  const elements = new Map();
  const ctx = {
    clearRect() { frames.push([]); },
    save() { stack.push({ ...transform }); },
    restore() { transform = stack.pop(); },
    scale(x) { transform.scale *= x; },
    translate(x, y) { transform.x += x * transform.scale; transform.y += y * transform.scale; },
    arc(x, y) { frames.at(-1).push({ x: transform.x + x * transform.scale, y: transform.y + y * transform.scale }); },
    beginPath() {}, fill() {}, stroke() {}, moveTo() {}, lineTo() {}, fillText() {},
  };
  const byId = (id) => {
    if (!elements.has(id)) elements.set(id, {
      value: '', hidden: true, width: 300, height: 150, style: {}, textContent: '',
      classList: { toggle() {} }, handlers: {},
      getBoundingClientRect: () => rect,
      getContext: () => ctx,
      getAttribute: () => id.replace('filter-kind-', ''),
      addEventListener(name, fn) { this.handlers[name] = fn; },
    });
    return elements.get(id);
  };
  const posts = [];
  let newNotes = 0;
  const sandbox = {
    byId, post: (action) => posts.push(action),
    window: { devicePixelRatio: 2, addEventListener() {},
      __neuraliaNotes: { newNote() { newNotes++; } } },
    requestAnimationFrame: (fn) => { pending.push(fn); return pending.length; },
    ResizeObserver: class { constructor(fn) { observers.push(fn); } observe() {} },
    setTimeout: (fn) => { pending.push(fn); return pending.length; },
  };
  vm.runInNewContext(fs.readFileSync(new URL('../assets/panel/obsidian.js', import.meta.url), 'utf8') + '\nglobalThis.graph = obsidian;', sandbox);
  return {
    graph: sandbox.graph, byId, posts,
    newNotes: () => newNotes,
    layout(width, height) { rect = { width, height, left: 0, top: 0 }; observers.forEach(fn => fn()); },
    lastFrame: () => frames.at(-1) || [],
    input(value) { byId('oq').value = value; byId('oq').handlers.input(); },
    drain() { for (let i = 0; i < 800 && pending.length; i++) pending.shift()(); },
  };
}

const data = {
  nodes: [
    { id: 'note:1', label: 'Memória', kind: 'note', target: '1', subtitle: 'Nota Zettelkasten' },
    { id: 'site:1', label: 'Google', kind: 'site', target: 'https://google.com/research' },
    { id: 'hist:1', label: 'Uma busca', kind: 'history', target: 'busca' },
  ],
  edges: [{ from: 'note:1', to: 'site:1' }],
};

function onScreen(p, count, width, height) {
  assert.equal(p.lastFrame().length, count, 'every visible node must paint synchronously');
  for (const point of p.lastFrame()) {
    assert.ok(point.x >= 0 && point.x <= width * 2, `node outside canvas width: ${point.x}`);
    assert.ok(point.y >= 0 && point.y <= height * 2, `node outside canvas height: ${point.y}`);
  }
}

const p = panel();
p.layout(390, 640);
p.graph.render(data);
assert.equal(p.byId('obsidian-canvas').width, 780, 'hidden-tab initial size must be replaced');
assert.equal(p.byId('obsidian-canvas').height, 1280);
onScreen(p, 3, 390, 640);
p.drain(); // Let physics sleep, then change only the container (no window resize).
p.layout(600, 800);
onScreen(p, 3, 600, 800);
p.byId('hud-freeze').handlers.click();
p.layout(320, 450);
p.byId('obsidian-reset').handlers.click();
onScreen(p, 3, 320, 450);
p.input('memoria');
onScreen(p, 1, 320, 450);
assert.match(p.byId('obsidian-counts').textContent, /1 nós \(2 ocultos\)/);
p.input('research');
onScreen(p, 1, 320, 450); // Match URL beyond the displayed host label.
p.input('não existe');
assert.equal(p.lastFrame().length, 0);
assert.match(p.byId('obsidian-counts').textContent, /^0 nós/);
p.input('');
onScreen(p, 3, 320, 450);
p.byId('filter-orphans').handlers.click();
onScreen(p, 2, 320, 450);
p.byId('filter-kind-site').handlers.click();
onScreen(p, 1, 320, 450);
p.byId('obsidian-new-note').handlers.click();
assert.equal(p.newNotes(), 1, 'create note must enter the shipped notes editor');
p.graph.refresh();
assert.equal(p.posts.at(-1), 'obsidian-graph', 'returning from saved notes must reload graph data');

// Data may arrive before the section becomes visible; revealing must repaint.
const hidden = panel();
hidden.graph.render(data);
hidden.drain();
hidden.layout(390, 640);
onScreen(hidden, 3, 390, 640);
hidden.byId('obsidian-reset').handlers.click();
onScreen(hidden, 3, 390, 640);
console.log('Obsidian canvas: hidden/revealed, resize, paused center, search, filters and create-note entry passed.');
