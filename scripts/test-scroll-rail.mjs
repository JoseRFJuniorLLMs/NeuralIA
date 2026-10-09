import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const source = fs.readFileSync('crates/neural-app/src/windows_app/page_scripts.rs', 'utf8');
const script = source.match(/const SPLIT_SCROLL_RAIL_SCRIPT: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(script, 'shipped scroll rail missing');

function fixture({ nested = false, anchors = [], iframe = false, comparator = false, pdf = false } = {}) {
  const calls = [], scheduled = [], observers = [];
  class Element {
    constructor(tag = 'div') {
      this.tagName = tag.toUpperCase();
      this.style = {}; this.dataset = {}; this.children = [];
      this.scrollHeight = 3000; this.clientHeight = 600; this.scrollTop = 1000;
      this.textContent = ''; this.top = 0;
      this.classes = new Set();
      this.classList = { add: x => this.classes.add(x), remove: x => this.classes.delete(x) };
    }
    appendChild(child) { this.children.push(child); child.parent = this; return child; }
    getAttribute() { return null; }
    setAttribute() {}
    closest() { return null; }
    contains(child) { return this === child || this.children.some(el => el.contains(child)); }
    getBoundingClientRect() { return { top: this.top - (nested ? main.scrollTop : window.scrollY) }; }
    scrollTo({ top }) { calls.push(top); this.scrollTop = top; }
    addEventListener(name, fn) { this.listeners ||= {}; this.listeners[name] = fn; }
  }
  const html = new Element('html');
  const main = new Element('main');
  const other = new Element('aside');
  if (nested) html.scrollHeight = html.clientHeight;
  const headings = anchors.map((top, i) => {
    const el = new Element('h2'); el.top = top; el.textContent = 'Section ' + i;
    main.appendChild(el); return el;
  });
  main.getBoundingClientRect = () => ({ top: 0 });
  const body = new Element('body');
  body.textContent = 'word '.repeat(1000);
  main.textContent = body.textContent;
  const find = (el, id) => el.id === id ? el : el.children.map(child => find(child, id)).find(Boolean);
  const document = {
    documentElement: html, scrollingElement: html, body, readyState: 'complete',
    getElementById: id => find(html, id), createElement: tag => new Element(tag), addEventListener: Element.prototype.addEventListener,
    querySelector: () => null,
    querySelectorAll: selector => selector.startsWith('main,') ? [main, other] : headings,
  };
  const window = {
    innerHeight: 600, scrollY: 1000, addEventListener: Element.prototype.addEventListener,
    chrome: { webview: { postMessage() {} } },
    scrollTo({ top }) { calls.push(top); this.scrollY = top; },
  };
  window.top = iframe ? {} : window;
  const code = comparator ? source.match(/const COMPARATOR_INJECT_SCRIPT: &str = r#"([\s\S]*?)"#;/)[1] : script;
  vm.runInNewContext(code, {
    window, document,
    getComputedStyle: el => ({ display: 'block', visibility: 'visible', overflowY: el === other ? 'hidden' : 'auto' }),
    EventTarget: Element, Node: Element,
    location: new URL(pdf ? 'https://example.org/book.pdf' : 'https://chatgpt.com/'), URL,
    setTimeout: () => 1, clearTimeout() {},
    MutationObserver: class { constructor(fn) { observers.push(fn); } observe() {} },
    requestAnimationFrame: fn => { scheduled.push(fn); return scheduled.length; },
  });
  if (comparator && !iframe) document.listeners.DOMContentLoaded();
  const rail = find(html, comparator ? 'neuralia-response-rail' : 'neuralia-split-scroll-rail');
  function click(direction) {
    const button = direction < 0 ? rail.children[0] : rail.children.at(-1);
    assert.equal(button.type, 'button', 'arrow must never submit a page form');
    let prevented = false; let stopped = false;
    button.onclick({ preventDefault() { prevented = true; }, stopPropagation() { stopped = true; } });
    assert.ok(prevented && stopped);
    return calls.at(-1);
  }
  return { click, calls, window, main, other, html, rail, body, observers, scheduled,
    sync() { window.listeners.scroll(); while (scheduled.length) scheduled.shift()(); } };

}

let cases = 0;
for (const nested of [false, true]) {
  for (const comparator of [false, true]) {
    // Several anchors below the current top but inside the old 24%-viewport pivot:
    // the old up handler selected 1100 and moved DOWN from 1000.
    const dense = fixture({ nested, comparator, anchors: [800, 1000, 1100, 1200, 1600] });
    assert.equal(dense.click(-1), 800, 'up must select an anchor above the current top');
    assert.equal(dense.click(1), 1000, 'down must select an anchor below the current top');
    const fallback = fixture({ nested, comparator });
    assert.ok(Math.abs(fallback.click(-1) - 508) < 1e-6, 'up without headings moves upward');
    assert.equal(fallback.click(1), 1000, 'down without headings moves downward');
    if (nested) fallback.main.scrollTop = 0; else fallback.window.scrollY = 0;
    assert.equal(fallback.click(-1), 0, 'upper bound');
    if (nested) fallback.main.scrollTop = 2400; else fallback.window.scrollY = 2400;
    assert.equal(fallback.click(1), 2400, 'lower bound');
    assert.ok((nested ? fallback.main : fallback.html).classes.has('neuralia-scroll-root'));
    assert.ok(!fallback.other.classes.has('neuralia-scroll-root'), 'independent panel scrollbar preserved');
    const ticks = fallback.rail.children[1];
    const clickTrack = (fraction) => {
      const track = ticks.getBoundingClientRect();
      const trackHeight = track.height || ticks.clientHeight;
      ticks.onclick({
        clientY: track.top + trackHeight * fraction,
        preventDefault() {},
        stopPropagation() {},
      });
    };
    clickTrack(0);
    assert.equal(fallback.calls.at(-1), 0, 'a click at the top of the timeline scrolls up');
    clickTrack(1);
    assert.equal(fallback.calls.at(-1), 2400, 'a click at the bottom of the timeline scrolls to the end');
    const up = fallback.rail.children[0];
    up.onfocus();
    assert.equal(up.style.color, '#3d9bff', 'focused arrow glyph turns blue');
    up.onblur();
    assert.equal(up.style.color, 'rgba(255,255,255,.72)');
    cases += 12;
  }
}
assert.equal(fixture({ iframe: true }).rail, undefined, 'child frame must not mount a rail');
console.log(`scroll rail: ${cases + 1} behavior checks passed (document, nested root, direction, bounds, buttons, iframe)`);

for (const comparator of [false, true]) {
  for (const nested of [false, true]) {
    const p = fixture({ comparator, nested });
    const status = p.rail.children.find(el => el.className === 'neuralia-reading-progress');
    const ticks = p.rail.children[1];
    assert.match(status.textContent, /Pág\. 2\/5\n≈3 min restantes/);
    assert.equal(status.style.opacity, '0', 'page and time stay hidden until hover');
    p.rail.onmouseenter();
    assert.equal(status.style.opacity, '1');
    p.rail.onmouseleave();
    assert.equal(status.style.opacity, '0');
    assert.equal(p.rail.style.width, '16px');
    assert.equal(ticks.style.minHeight, '0', 'short windows must allow tick area to shrink');
    assert.ok(ticks.children.every(el => Number.parseFloat(el.style.height) <= 2));
    p.observers.forEach(fn => fn([{ target: status }]));
    assert.equal(p.scheduled.length, 0, 'rail DOM changes must not recursively schedule frames');
    if (nested) p.main.scrollTop = 2400; else p.window.scrollY = 2400;
    p.sync();
    assert.match(status.textContent, /Pág\. 5\/5\n≈0 min restantes/);
    if (nested) p.main.scrollTop = 0; else p.window.scrollY = 0;
    (nested ? p.main : p.body).textContent = 'word '.repeat(2000);
    p.observers.forEach(fn => fn([{ target: nested ? p.main : p.body }]));
    assert.ok(p.scheduled.length > 0, 'new page text must refresh the estimate');
    while (p.scheduled.length) p.scheduled.shift()();
    assert.match(status.textContent, /Pág\. 1\/5\n≈10 min restantes/);
  }
}
for (const comparator of [false, true]) {
  const page = fixture({ comparator });
  if (comparator) {
    const expand = findId(page.html, 'neuralia-comp-expand');
    assert.equal(expand.style.width, 'max-content', 'maximize pill must not stretch across the page');
    assert.equal(expand.style.maxWidth, '240px');
  }
  const pdf = fixture({ comparator, pdf: true });
  assert.equal(pdf.rail, undefined, 'a PDF keeps the viewer scrollbar; the timeline must not cover it');
  assert.equal(pdf.html.children.some(el => el.id === 'neuralia-scroll-style' || el.id === 'neuralia-split-scroll-style'), false, 'PDF scrollbar must stay visible');
  const controls = pdf.html.children.find(el => el.id === 'neuralia-comp-controls');
  assert.equal(controls, undefined, 'PDF viewer must not get the column overlay');
}
console.log('scroll rail: size, page/time, dynamic text, self-mutation and PDF scrollbar checks passed for both rails');

function findId(el, id) {
  if (el.id === id) return el;
  for (const child of el.children || []) {
    const found = findId(child, id);
    if (found) return found;
  }
  return undefined;
}
