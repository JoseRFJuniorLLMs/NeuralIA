import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const SOURCES = [
  'crates/neural-app/src/windows_app.rs',
  'crates/neural-app/src/windows_app/page_scripts.rs',
];
const source = SOURCES.filter(fs.existsSync).map(p => fs.readFileSync(p, 'utf8')).join('\n');
const match = source.match(/const AI_AUTO_SUBMIT_SCRIPT: &str = r#"\r?\n([\s\S]*?)\r?\n"#;/);
assert.ok(match, 'AI_AUTO_SUBMIT_SCRIPT must remain extractable');
const script = match[1];

function scenario(host, { formAfter = 0, buttonAfter = Number.POSITIVE_INFINITY } = {}) {
  const query = 'pergunta de regressao';
  let href = host === 'chatgpt.com'
    ? 'https://chatgpt.com/?q=pergunta+de+regressao&hints=search'
    : 'https://claude.ai/new?q=pergunta+de+regressao';
  let tick = 0;
  let now = 20000;
  let clicks = 0;
  let formSubmits = 0;
  let syntheticEnter = 0;
  const queue = [];
  const storage = new Map();

  const form = {
    querySelector() { return null; },
    requestSubmit() {
      formSubmits += 1;
      editor.value = '';
      href = host === 'chatgpt.com' ? 'https://chatgpt.com/c/ok' : 'https://claude.ai/chat/ok';
    }
  };
  const button = {
    disabled: false,
    getAttribute() { return null; },
    getBoundingClientRect() { return { width: 32, height: 32 }; },
    click() {
      clicks += 1;
      editor.value = '';
      href = host === 'chatgpt.com' ? 'https://chatgpt.com/c/ok' : 'https://claude.ai/chat/ok';
    }
  };
  const editor = {
    value: query,
    isContentEditable: false,
    focus() {},
    getBoundingClientRect() { return { width: 500, height: 80 }; },
    closest(name) { return name === 'form' && tick >= formAfter ? form : null; },
    dispatchEvent(event) {
      if ((event.type === 'keydown' || event.type === 'keypress' || event.type === 'keyup')
          && event.key === 'Enter') {
        syntheticEnter += 1;
      }
      return true;
    }
  };

  const document = {
    readyState: 'complete',
    addEventListener() {},
    querySelectorAll() { return [editor]; },
    querySelector(selector) {
      if (tick >= buttonAfter && selector.includes('button')) return button;
      return null;
    },
    execCommand() { return false; },
    createRange() { return { selectNodeContents() {} }; }
  };

  class FakeEvent {
    constructor(type, init = {}) { this.type = type; Object.assign(this, init); }
  }

  const location = {
    hostname: host,
    get href() { return href; }
  };

  const context = {
    URL,
    location,
    document,
    sessionStorage: {
      getItem(key) { return storage.get(key) ?? null; },
      setItem(key, value) { storage.set(key, String(value)); }
    },
    Event: FakeEvent,
    InputEvent: FakeEvent,
    KeyboardEvent: FakeEvent,
    setTimeout(fn) { queue.push(fn); return queue.length; },
    Date: class extends Date {
      static now() { now += 3000; return now; }
    },
    getComputedStyle() { return {}; },
    console
  };
  context.window = context;
  context.window.top = context.window;
  context.window.getSelection = () => ({ removeAllRanges() {}, addRange() {} });

  vm.runInNewContext(script, context, { filename: 'AI_AUTO_SUBMIT_SCRIPT' });
  while (queue.length && tick < 200) {
    tick += 1;
    queue.shift()();
  }
  return { clicks, formSubmits, syntheticEnter, tick, href };
}

for (const host of ['chatgpt.com', 'claude.ai']) {
  const viaForm = scenario(host, { formAfter: 0 });
  assert.equal(viaForm.formSubmits, 1, host + ' must submit through requestSubmit when no button is available');
  assert.ok(!viaForm.href.includes('?q='), host + ' form submission must be acknowledged');

  const afterIgnoredEnter = scenario(host, { formAfter: 14 });
  assert.ok(afterIgnoredEnter.syntheticEnter > 0, host + ' scenario must exercise the ignored synthetic Enter');
  assert.equal(afterIgnoredEnter.formSubmits, 1, host + ' must keep retrying after an ignored synthetic Enter');
  assert.ok(!afterIgnoredEnter.href.includes('?q='), host + ' retry must eventually submit');

  const viaButton = scenario(host, { buttonAfter: 0 });
  assert.equal(viaButton.clicks, 1, host + ' should prefer the provider send button when available');
}

console.log('AI auto-submit runtime: ChatGPT and Claude retry/submit scenarios passed');
