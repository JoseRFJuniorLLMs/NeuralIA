import fs from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";

const source = fs.readFileSync("crates/neural-app/src/windows_app.rs", "utf8");
const match = source.match(/const COMPARATOR_INJECT_SCRIPT: &str = r#"([\s\S]*?)"#;/);
assert.ok(match, "COMPARATOR_INJECT_SCRIPT not found");
const script = match[1].replaceAll("__NEURALIA_CAP__", "test-cap");

class MockEventTarget {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, handler, capture) {
    const row = { handler, capture: !!capture };
    const list = this.listeners.get(type) ?? [];
    list.push(row);
    this.listeners.set(type, list);
  }
}

class MockNode extends MockEventTarget {
  appendChild(child) { return child; }
}

class MockElement extends MockNode {
  constructor({ href = "", attrs = {}, matchesLink = false, control = false } = {}) {
    super();
    this.href = href;
    this.attrs = attrs;
    this.matchesLink = matchesLink;
    this.control = control;
  }
  matches(selector) {
    return this.matchesLink && selector.includes("a[href]");
  }
  closest(selector) {
    if (selector.includes("#neuralia-comp-controls") && this.control) return this;
    if (this.matchesLink && selector.includes("a[href]")) return this;
    return null;
  }
  getAttribute(name) {
    if (name === "href" && this.href) return this.href;
    return this.attrs[name] ?? null;
  }
}

class MockDocument extends MockEventTarget {
  constructor() {
    super();
    this.documentElement = new MockNode();
  }
  getElementById() { return null; }
  createElement() { return new MockElement(); }
  querySelectorAll() { return []; }
  querySelector() { return null; }
}

const document = new MockDocument();
const window = new MockEventTarget();
window.top = window;
window.__neuralia_col_index = 1;
window.__neuralia_col_name = "ChatGPT";

const posted = [];
window.chrome = {
  webview: {
    postMessage(message) { posted.push(JSON.parse(message)); }
  }
};

const context = vm.createContext({
  window,
  document,
  EventTarget: MockEventTarget,
  Node: MockNode,
  URL,
  location: new URL("https://chatgpt.com/c/test"),
  setTimeout,
  clearTimeout,
  Object,
  Function,
  JSON,
  Array,
  Number,
  String,
  Math,
  MutationObserver: class { observe() {} }
});

vm.runInContext(script, context, { filename: "COMPARATOR_INJECT_SCRIPT" });

function listener(type) {
  const rows = window.listeners.get(type) ?? [];
  assert.equal(rows.length, 1, `expected one window ${type} listener`);
  assert.equal(rows[0].capture, true, `${type} must capture at window`);
  return rows[0].handler;
}

const click = listener("click");
const auxclick = listener("auxclick");

function documentListener(type) {
  const rows = document.listeners.get(type) ?? [];
  assert.equal(rows.length, 1, `expected one document ${type} listener`);
  assert.equal(rows[0].capture, true, `${type} must capture at document`);
  return rows[0].handler;
}

const dblclick = documentListener("dblclick");

function eventFor({ anchor, target = null, button = 0, ctrlKey = false, metaKey = false, altKey = false, shiftKey = false }) {
  let prevented = false;
  let stopped = false;
  const leaf = target ?? new MockElement();
  return {
    isTrusted: true,
    defaultPrevented: false,
    button,
    ctrlKey,
    metaKey,
    altKey,
    shiftKey,
    target: leaf,
    composedPath() { return [leaf, anchor, document, window]; },
    preventDefault() { prevented = true; },
    stopImmediatePropagation() { stopped = true; },
    get prevented() { return prevented; },
    get stopped() { return stopped; }
  };
}

function popMessage() {
  assert.equal(posted.length, 1, "expected exactly one IPC message");
  return posted.pop();
}

// 1. External plain click: three-way routing.
{
  const anchor = new MockElement({ href: "https://example.org/source", matchesLink: true });
  const ev = eventFor({ anchor });
  click(ev);
  const msg = popMessage();
  assert.equal(msg.action, "link");
  assert.deepEqual(msg.args, { col: 1, url: "https://example.org/source", aside: false });
  assert.equal(ev.prevented, true);
  assert.equal(ev.stopped, true);
}

// 2. Ctrl-click: side panel/tab semantics.
{
  const anchor = new MockElement({ href: "https://example.org/ctrl", matchesLink: true });
  const ev = eventFor({ anchor, ctrlKey: true });
  click(ev);
  const msg = popMessage();
  assert.equal(msg.action, "link");
  assert.equal(msg.args.aside, true);
  assert.equal(msg.args.url, "https://example.org/ctrl");
  assert.equal(ev.prevented, true);
  assert.equal(ev.stopped, true);
}

// 3. Middle click uses auxclick and also opens beside.
{
  const anchor = new MockElement({ href: "https://example.org/middle", matchesLink: true });
  const ev = eventFor({ anchor, button: 1 });
  auxclick(ev);
  const msg = popMessage();
  assert.equal(msg.action, "link");
  assert.equal(msg.args.aside, true);
  assert.equal(msg.args.url, "https://example.org/middle");
}

// 4. Same-origin normal navigation is left to the provider SPA.
{
  const anchor = new MockElement({ href: "https://chatgpt.com/c/next", matchesLink: true });
  const ev = eventFor({ anchor });
  click(ev);
  assert.equal(posted.length, 0);
  assert.equal(ev.prevented, false);
  assert.equal(ev.stopped, false);
}

// 5. Shadow-DOM retargeting: event.target is not the link, composedPath is.
{
  const anchor = new MockElement({ href: "https://example.org/shadow", matchesLink: true });
  const host = new MockElement();
  const ev = eventFor({ anchor, target: host });
  click(ev);
  const msg = popMessage();
  assert.equal(msg.args.url, "https://example.org/shadow");
  assert.equal(msg.args.aside, false);
}

// 6. Google redirect wrappers resolve to the actual source URL.
{
  const wrapped = "https://www.google.com/url?q=" + encodeURIComponent("https://example.org/google-source");
  const anchor = new MockElement({ href: wrapped, matchesLink: true });
  const ev = eventFor({ anchor });
  click(ev);
  const msg = popMessage();
  assert.equal(msg.args.url, "https://example.org/google-source");
}

// 7. Double-clicking a real link must not also expand the comparator column.
{
  const anchor = new MockElement({ href: "https://example.org/double", matchesLink: true });
  const ev = eventFor({ anchor, target: anchor });
  dblclick(ev);
  assert.equal(posted.length, 0, "double-click on link must not emit expand");
}

// 8. Double-clicking ordinary panel content still expands the focused column.
{
  const leaf = new MockElement();
  const ev = eventFor({ anchor: null, target: leaf });
  dblclick(ev);
  const msg = popMessage();
  assert.equal(msg.action, "expand");
  assert.deepEqual(msg.args, { col: 1 });
}

console.log("link routing runtime gate: ok");
