import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const source = fs.readFileSync('crates/neural-app/src/windows_app/page_scripts.rs', 'utf8');
const script = source.match(/const SPLIT_SCROLL_RAIL_SCRIPT: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(script, 'shipped scroll rail missing');

function fixture({ nested = false, anchors = [], iframe = false } = {}) {
  const calls = [];
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
    contains(child) { return this.children.includes(child); }
    getBoundingClientRect() { return { top: this.top - (nested ? main.scrollTop : window.scrollY) }; }
    scrollTo({ top }) { calls.push(top); this.scrollTop = top; }
    addEventListener() {}
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
  const document = {
    documentElement: html, scrollingElement: html, body: new Element('body'), readyState: 'complete',
    getElementById: () => null, createElement: tag => new Element(tag), addEventListener() {},
    querySelectorAll: selector => selector.startsWith('main,') ? [main, other] : headings,
  };
  const window = {
    innerHeight: 600, scrollY: 1000, addEventListener() {},
    scrollTo({ top }) { calls.push(top); this.scrollY = top; },
  };
  window.top = iframe ? {} : window;
  vm.runInNewContext(script, {
    window, document,
    getComputedStyle: el => ({ display: 'block', visibility: 'visible', overflowY: el === other ? 'hidden' : 'auto' }),
    MutationObserver: class { observe() {} }, requestAnimationFrame: () => 1,
  });
  const rail = html.children.find(el => el.id === 'neuralia-split-scroll-rail');
  function click(direction) {
    const button = direction < 0 ? rail.children[0] : rail.children.at(-1);
    assert.equal(button.type, 'button', 'arrow must never submit a page form');
    let prevented = false; let stopped = false;
    button.onclick({ preventDefault() { prevented = true; }, stopPropagation() { stopped = true; } });
    assert.ok(prevented && stopped);
    return calls.at(-1);
  }
  return { click, calls, window, main, other, html, rail };
}

let cases = 0;
for (const nested of [false, true]) {
  // Several anchors below the current top but inside the old 24%-viewport pivot:
  // the old up handler selected 1100 and moved DOWN from 1000.
  const dense = fixture({ nested, anchors: [800, 1000, 1100, 1200, 1600] });
  assert.equal(dense.click(-1), 800, 'up must select an anchor above the current top');
  assert.equal(dense.click(1), 1000, 'down must select an anchor below the current top');
  const fallback = fixture({ nested });
  assert.ok(Math.abs(fallback.click(-1) - 508) < 1e-6, 'up without headings moves upward');
  assert.equal(fallback.click(1), 1000, 'down without headings moves downward');
  if (nested) fallback.main.scrollTop = 0; else fallback.window.scrollY = 0;
  assert.equal(fallback.click(-1), 0, 'upper bound');
  if (nested) fallback.main.scrollTop = 2400; else fallback.window.scrollY = 2400;
  assert.equal(fallback.click(1), 2400, 'lower bound');
  assert.ok((nested ? fallback.main : fallback.html).classes.has('neuralia-scroll-root'));
  assert.ok(!fallback.other.classes.has('neuralia-scroll-root'), 'independent panel scrollbar preserved');
  cases += 8;
}
assert.equal(fixture({ iframe: true }).rail, undefined, 'child frame must not mount a rail');
console.log(`scroll rail: ${cases + 1} behavior checks passed (document, nested root, direction, bounds, buttons, iframe)`);
