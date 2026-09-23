// Harness dos gates da leitura em voz alta (so testes).
//
// Corre os scripts QUE EMBARCAM (INPUT.scripts, texto tal e qual o produto os
// serve ou injeta) num contexto node:vm com um DOM pequeno mas com propagacao
// de eventos real (captura na janela -> document -> alvo -> bolha), um
// speechSynthesis falso que regista cada utterance, a CSS Custom Highlight API,
// temporizadores virtuais e armadilhas em todas as APIs de rede. Depois corre
// INPUT.drive (corpo de uma funcao async) e escreve em JSON o que ele devolve,
// mais os erros, os acessos a rede e as mensagens postadas pelo mapa de teclas.
'use strict';
const vm = require('node:vm');

// Corre DENTRO do contexto (via toString): tudo o que cria pertence ao realm
// da "pagina".
function setup() {
  const g = globalThis;
  g.window = g;
  g.self = g;
  g.top = g;
  g.__errors = [];
  g.__net = [];
  g.__posted = [];
  g.__scrolls = [];
  g.__nav = [];
  g.__stale = [];

  // ------------------------------------------------------------ temporizadores
  let now = 0;
  let seq = 0;
  const timers = new Map();
  g.setTimeout = (fn, ms) => {
    const id = ++seq;
    timers.set(id, { fn, at: now + Math.max(0, Number(ms) || 0) });
    return id;
  };
  g.clearTimeout = (id) => {
    timers.delete(id);
  };
  g.setInterval = g.setTimeout;
  g.clearInterval = g.clearTimeout;
  g.requestAnimationFrame = (fn) => g.setTimeout(fn, 16);
  g.cancelAnimationFrame = g.clearTimeout;
  g.__tick = (ms) => {
    const until = now + ms;
    for (;;) {
      let pick = null;
      for (const [id, t] of timers) {
        if (t.at > until) continue;
        if (!pick || t.at < pick[1].at || (t.at === pick[1].at && id < pick[0])) pick = [id, t];
      }
      if (!pick) break;
      timers.delete(pick[0]);
      now = pick[1].at;
      try {
        pick[1].fn();
      } catch (e) {
        g.__errors.push('timer: ' + e.message);
      }
    }
    now = until;
  };

  // ------------------------------------------------------------ eventos
  function add(target, type, fn, opts) {
    const capture = opts === true || !!(opts && opts.capture);
    const once = !!(opts && typeof opts === 'object' && opts.once);
    (target.__l || (target.__l = [])).push({ type: String(type), fn, capture, once });
  }
  function remove(target, type, fn, opts) {
    const capture = opts === true || !!(opts && opts.capture);
    const list = target.__l || [];
    const at = list.findIndex((l) => l.type === type && l.fn === fn && l.capture === capture);
    if (at >= 0) list.splice(at, 1);
  }
  function invoke(node, ev, capture, flags) {
    for (const l of (node.__l || []).slice()) {
      if (l.type !== ev.type || l.capture !== capture) continue;
      if (l.once) remove(node, l.type, l.fn, l.capture);
      ev.currentTarget = node;
      try {
        if (typeof l.fn === 'function') l.fn.call(node, ev);
        else l.fn.handleEvent(ev);
      } catch (e) {
        g.__errors.push(ev.type + ': ' + e.message);
      }
      if (flags.immediate) return;
    }
  }
  g.__dispatch = (target, type, init) => {
    const flags = { stopped: false, immediate: false };
    const ev = Object.assign(
      { type, bubbles: true, isTrusted: true, key: '', ctrlKey: false, metaKey: false, altKey: false, shiftKey: false },
      init || {}
    );
    ev.target = target;
    ev.defaultPrevented = false;
    ev.preventDefault = () => {
      ev.defaultPrevented = true;
    };
    ev.stopPropagation = () => {
      flags.stopped = true;
    };
    ev.stopImmediatePropagation = () => {
      flags.stopped = true;
      flags.immediate = true;
    };
    const path = [];
    for (let n = target; n; n = n.parentNode) path.push(n);
    if (path[path.length - 1] === g.document) path.push(g);
    ev.composedPath = () => path.slice();
    for (let i = path.length - 1; i >= 0 && !flags.stopped; i--) invoke(path[i], ev, true, flags);
    if (ev.bubbles) {
      for (let i = 0; i < path.length && !flags.stopped; i++) invoke(path[i], ev, false, flags);
    } else if (!flags.stopped) {
      invoke(target, ev, false, flags);
    }
    return ev;
  };
  g.addEventListener = (t, f, o) => add(g, t, f, o);
  g.removeEventListener = (t, f, o) => remove(g, t, f, o);

  class EventTarget {
    addEventListener(t, f, o) {
      add(this, t, f, o);
    }
    removeEventListener(t, f, o) {
      remove(this, t, f, o);
    }
    dispatchEvent(ev) {
      g.__dispatch(this, ev.type, ev);
      return true;
    }
  }
  g.EventTarget = EventTarget;

  // ------------------------------------------------------------ DOM
  class Node extends EventTarget {
    constructor() {
      super();
      this.parentNode = null;
      this.childNodes = [];
    }
    get firstChild() {
      return this.childNodes[0] || null;
    }
    appendChild(child) {
      if (child.parentNode) child.parentNode.removeChild(child);
      child.parentNode = this;
      this.childNodes.push(child);
      return child;
    }
    append(...children) {
      for (const c of children) this.appendChild(typeof c === 'string' ? new Text(c) : c);
    }
    prepend(...children) {
      for (const c of children.reverse()) this.insertBefore(typeof c === 'string' ? new Text(c) : c, this.firstChild);
    }
    insertBefore(child, ref) {
      if (child.parentNode) child.parentNode.removeChild(child);
      const at = ref ? this.childNodes.indexOf(ref) : -1;
      child.parentNode = this;
      if (at < 0) this.childNodes.push(child);
      else this.childNodes.splice(at, 0, child);
      return child;
    }
    removeChild(child) {
      const at = this.childNodes.indexOf(child);
      if (at >= 0) this.childNodes.splice(at, 1);
      child.parentNode = null;
      return child;
    }
    remove() {
      if (this.parentNode) this.parentNode.removeChild(this);
    }
    contains(node) {
      for (let n = node; n; n = n.parentNode) if (n === this) return true;
      return false;
    }
    get textContent() {
      return this.childNodes.map((c) => c.textContent).join('');
    }
    set textContent(value) {
      for (const c of this.childNodes) c.parentNode = null;
      this.childNodes = [];
      if (value !== '' && value !== null && value !== undefined) this.appendChild(new Text(String(value)));
    }
  }

  class Text extends Node {
    constructor(data) {
      super();
      this.nodeType = 3;
      this.data = String(data);
    }
    get textContent() {
      return this.data;
    }
    set textContent(value) {
      this.data = String(value);
    }
    get nodeValue() {
      return this.data;
    }
    get length() {
      return this.data.length;
    }
  }

  class Style {
    setProperty(k, v) {
      this[k] = String(v);
    }
    getPropertyValue(k) {
      return this[k] || '';
    }
    removeProperty(k) {
      delete this[k];
    }
  }

  class ClassList {
    constructor() {
      this.items = new Set();
    }
    add(...names) {
      for (const n of names) this.items.add(String(n));
    }
    remove(...names) {
      for (const n of names) this.items.delete(String(n));
    }
    contains(name) {
      return this.items.has(String(name));
    }
    toggle(name, force) {
      const on = force === undefined ? !this.items.has(name) : !!force;
      if (on) this.items.add(name);
      else this.items.delete(name);
      return on;
    }
    get value() {
      return Array.from(this.items).join(' ');
    }
  }

  function parseCompound(sel) {
    const m = /^([a-zA-Z0-9*]+)?((?:[#.][\w-]+)*)$/.exec(sel.trim());
    if (!m) return null;
    const out = { tag: m[1] ? m[1].toUpperCase() : null, id: null, classes: [] };
    for (const part of m[2].match(/[#.][\w-]+/g) || []) {
      if (part[0] === '#') out.id = part.slice(1);
      else out.classes.push(part.slice(1));
    }
    return out;
  }
  function matches(el, selector) {
    if (!el || el.nodeType !== 1) return false;
    return String(selector)
      .split(',')
      .some((one) => {
        const c = parseCompound(one);
        if (!c) return false;
        if (c.tag && c.tag !== '*' && c.tag !== el.tagName) return false;
        if (c.id && el.id !== c.id) return false;
        return c.classes.every((k) => el.classList.contains(k));
      });
  }
  function descendants(root, out) {
    for (const c of root.childNodes) {
      if (c.nodeType === 1) {
        out.push(c);
        descendants(c, out);
      }
    }
    return out;
  }

  class Element extends Node {
    constructor(tag) {
      super();
      this.nodeType = 1;
      this.tagName = String(tag).toUpperCase();
      this.attributes = {};
      this.style = new Style();
      this.dataset = {};
      this.classList = new ClassList();
      this.hidden = false;
      this.disabled = false;
      this.value = '';
      this.__html = '';
      this.__rect = null;
    }
    get id() {
      return this.attributes.id || '';
    }
    set id(v) {
      this.attributes.id = String(v);
    }
    get className() {
      return this.classList.value;
    }
    set className(v) {
      this.classList = new ClassList();
      for (const n of String(v).split(/\s+/)) if (n) this.classList.add(n);
    }
    setAttribute(k, v) {
      if (k === 'class') this.className = v;
      else this.attributes[k] = String(v);
    }
    getAttribute(k) {
      if (k === 'class') return this.className;
      return Object.prototype.hasOwnProperty.call(this.attributes, k) ? this.attributes[k] : null;
    }
    hasAttribute(k) {
      return this.getAttribute(k) !== null;
    }
    removeAttribute(k) {
      delete this.attributes[k];
    }
    get children() {
      return this.childNodes.filter((c) => c.nodeType === 1);
    }
    get innerHTML() {
      return this.__html;
    }
    set innerHTML(v) {
      this.textContent = '';
      this.__html = String(v);
    }
    get isContentEditable() {
      return false;
    }
    getBoundingClientRect() {
      return this.__rect || { top: 200, bottom: 220, left: 0, right: 100, width: 100, height: 20 };
    }
    scrollIntoView(opts) {
      g.__scrolls.push({ unit: this.getAttribute('data-unit'), opts: opts || null });
    }
    focus() {
      g.document.activeElement = this;
    }
    blur() {}
    select() {}
    click() {
      g.__dispatch(this, 'click', {});
    }
    closest(sel) {
      for (let n = this; n && n.nodeType === 1; n = n.parentNode) if (matches(n, sel)) return n;
      return null;
    }
    matches(sel) {
      return matches(this, sel);
    }
    getElementsByTagName(tag) {
      const want = String(tag).toUpperCase();
      return descendants(this, []).filter((el) => want === '*' || el.tagName === want);
    }
    querySelector(sel) {
      return descendants(this, []).find((el) => matches(el, sel)) || null;
    }
    querySelectorAll(sel) {
      return descendants(this, []).filter((el) => matches(el, sel));
    }
  }

  class Document extends Node {
    constructor() {
      super();
      this.nodeType = 9;
      this.readyState = 'loading';
      this.title = '';
      this.documentElement = new Element('html');
      this.documentElement.lang = 'pt';
      this.head = new Element('head');
      this.body = new Element('body');
      this.documentElement.appendChild(this.head);
      this.documentElement.appendChild(this.body);
      this.appendChild(this.documentElement);
      this.activeElement = this.body;
    }
    get scrollingElement() {
      return this.documentElement;
    }
    createElement(tag) {
      return new Element(tag);
    }
    createTextNode(text) {
      return new Text(text);
    }
    createRange() {
      return new Range();
    }
    getElementById(id) {
      return descendants(this, []).find((el) => el.id === id) || null;
    }
    getElementsByTagName(tag) {
      return this.documentElement.getElementsByTagName(tag);
    }
    querySelector(sel) {
      return descendants(this, []).find((el) => matches(el, sel)) || null;
    }
    querySelectorAll(sel) {
      return descendants(this, []).filter((el) => matches(el, sel));
    }
  }

  class Range {
    setStart(node, offset) {
      this.startContainer = node;
      this.startOffset = offset;
    }
    setEnd(node, offset) {
      this.endContainer = node;
      this.endOffset = offset;
    }
  }
  class Highlight {
    constructor(...ranges) {
      this.ranges = ranges;
    }
  }
  class Observer {
    constructor(callback) {
      this.callback = callback;
    }
    observe() {}
    unobserve() {}
    disconnect() {}
    takeRecords() {
      return [];
    }
  }

  g.Node = Node;
  g.Text = Text;
  g.Element = Element;
  g.HTMLElement = Element;
  g.Document = Document;
  g.Range = Range;
  g.Highlight = Highlight;
  g.CSS = { highlights: new Map() };
  g.MutationObserver = Observer;
  g.ResizeObserver = Observer;
  g.IntersectionObserver = Observer;
  g.document = new Document();
  g.innerHeight = 800;
  g.innerWidth = 1200;
  g.scrollX = 0;
  g.scrollY = 0;
  g.devicePixelRatio = 1;
  g.scrollTo = () => {};
  g.scrollBy = () => {};
  g.find = () => false;
  g.location = new URL(g.__href);
  g.history = {
    back() {
      g.__nav.push('back');
    },
    forward() {
      g.__nav.push('forward');
    },
  };
  g.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
  g.getComputedStyle = () => ({ display: 'block', visibility: 'visible', overflowY: 'auto', rowGap: '0' });
  g.chrome = {
    webview: {
      postMessage(message) {
        g.__posted.push(String(message));
      },
    },
  };

  // ------------------------------------------------------------ selecao
  g.__selection = null;
  g.getSelection = () => g.__selection || { isCollapsed: true, rangeCount: 0 };
  g.__select = (node, offset) => {
    g.__selection = {
      isCollapsed: false,
      rangeCount: 1,
      getRangeAt: () => ({ startContainer: node, startOffset: offset }),
    };
  };

  // ------------------------------------------------------------ armazenamento
  const store = new Map();
  g.__storageThrows = false;
  Object.defineProperty(g, 'localStorage', {
    configurable: true,
    get() {
      if (g.__storageThrows) throw new Error('SecurityError: opaque origin');
      return {
        getItem: (k) => (store.has(k) ? store.get(k) : null),
        setItem: (k, v) => store.set(k, String(v)),
        removeItem: (k) => store.delete(k),
      };
    },
  });
  g.__stored = () => Object.fromEntries(store);

  // ------------------------------------------------------------ rede (armadilhas)
  g.fetch = (url) => {
    g.__net.push('fetch ' + url);
    return Promise.reject(new Error('rede proibida no gate'));
  };
  g.XMLHttpRequest = class {
    constructor() {
      g.__net.push('XMLHttpRequest');
    }
    open(method, url) {
      g.__net.push('xhr ' + url);
    }
    send() {}
    setRequestHeader() {}
  };
  g.WebSocket = class {
    constructor(url) {
      g.__net.push('WebSocket ' + url);
    }
    send() {}
    close() {}
  };
  g.EventSource = class {
    constructor(url) {
      g.__net.push('EventSource ' + url);
    }
  };
  g.Worker = class {
    constructor(url) {
      g.__net.push('Worker ' + url);
    }
  };
  g.Image = class {
    set src(url) {
      g.__net.push('Image ' + url);
    }
  };
  g.navigator = {
    language: 'pt-BR',
    languages: ['pt-BR', 'pt'],
    userAgent: 'harness',
    sendBeacon(url) {
      g.__net.push('sendBeacon ' + url);
      return true;
    },
  };

  // ------------------------------------------------------------ fala
  class FakeSpeech extends EventTarget {
    constructor() {
      super();
      this.voices = [];
      this.queue = [];
      this.log = [];
      this.cancels = 0;
      this.maxQueue = 0;
      this.paused = false;
    }
    get speaking() {
      return this.queue.length > 0;
    }
    get pending() {
      return this.queue.length > 1;
    }
    getVoices() {
      return this.voices.slice();
    }
    speak(u) {
      this.queue.push(u);
      this.maxQueue = Math.max(this.maxQueue, this.queue.length);
      this.log.push({ text: u.text, rate: u.rate, voice: u.voice ? u.voice.voiceURI : null, lang: u.lang });
    }
    cancel() {
      this.cancels++;
      for (const u of this.queue.splice(0)) g.__stale.push(u);
    }
    pause() {
      this.paused = true;
    }
    resume() {
      this.paused = false;
    }
    // Lado do teste --------------------------------------------------
    setVoices(list) {
      this.voices = list.slice();
      for (const l of (this.__l || []).slice()) {
        if (l.type === 'voiceschanged') l.fn.call(this, { type: 'voiceschanged' });
      }
      if (typeof this.onvoiceschanged === 'function') this.onvoiceschanged({ type: 'voiceschanged' });
    }
    // O motor acabou a utterance da cabeca da fila.
    finish() {
      const u = this.queue.shift();
      if (u && typeof u.onend === 'function') u.onend({ type: 'end', utterance: u });
      return u || null;
    }
    // O motor falha a utterance da cabeca da fila.
    fail(error) {
      const u = this.queue.shift();
      if (u && typeof u.onerror === 'function') u.onerror({ type: 'error', error, utterance: u });
      return u || null;
    }
    // Bug do Chromium: a fala para e o 'end' nunca chega.
    lose() {
      return this.queue.shift() || null;
    }
    // Eventos atrasados das utterances canceladas: o Chromium entrega-os
    // DEPOIS do cancel(), como 'end' ou como erro 'interrupted'.
    deliverStale(kind) {
      for (const u of g.__stale.splice(0)) {
        if (kind === 'end' && typeof u.onend === 'function') u.onend({ type: 'end', utterance: u });
        if (kind !== 'end' && typeof u.onerror === 'function') {
          u.onerror({ type: 'error', error: 'interrupted', utterance: u });
        }
      }
    }
  }
  g.speechSynthesis = new FakeSpeech();
  g.SpeechSynthesisUtterance = class {
    constructor(text) {
      this.text = String(text === undefined ? '' : text);
      this.voice = null;
      this.lang = '';
      this.rate = 1;
      this.pitch = 1;
      this.volume = 1;
    }
  };
  g.__speech = g.speechSynthesis;
  g.__said = () => g.__speech.log.map((x) => x.text);
  g.__voice = (voiceURI, lang, local, extra) =>
    Object.assign({ voiceURI, name: voiceURI, lang, localService: local, default: false }, extra || {});

  // ------------------------------------------------------------ ajudas
  g.__settle = async () => {
    for (let i = 0; i < 25; i++) await new Promise((resolve) => g.__immediate(resolve));
  };
  g.__byId = (id) => g.document.getElementById(id);
  g.__key = (init) => g.__dispatch(g.document.activeElement || g.document.body, 'keydown', init);
  g.__click = (el) => g.__dispatch(el, 'click', {});
  g.__change = (el, value) => {
    el.value = value;
    return g.__dispatch(el, 'change', {});
  };
  g.__contentLoaded = () => {
    g.document.readyState = 'interactive';
    g.__dispatch(g.document, 'DOMContentLoaded', { bubbles: false });
  };
  // O realce atual: por cada Range, a unidade (data-unit) e o texto coberto.
  g.__highlight = () => {
    const h = g.CSS.highlights.get('neuralia-read-aloud');
    if (!h) return [];
    return h.ranges.map((r) => {
      const unitOf = (node) => (node.parentNode ? node.parentNode.getAttribute('data-unit') : null);
      return {
        unit: unitOf(r.startContainer),
        endUnit: unitOf(r.endContainer),
        from: r.startOffset,
        to: r.endOffset,
        text: r.startContainer === r.endContainer ? r.startContainer.data.slice(r.startOffset, r.endOffset) : null,
      };
    });
  };
  g.__marked = () =>
    descendants(g.document, [])
      .filter((el) => el.classList.contains('neuralia-ra-hl'))
      .map((el) => el.getAttribute('data-unit'));

  // Um item de conteudo marcado ({ mark: 'begin' | 'end' }) chega ao texto
  // como o PDF.js o da -- sem `str` -- e, como na TextLayer, nao tem textDiv.
  const textItem = (it) =>
    it.mark ? { type: it.mark === 'end' ? 'endMarkedContent' : 'beginMarkedContent', id: 'mc' } : { str: it.str, hasEOL: !!it.eol };

  // Um visualizador de PDF falso com as mesmas ligacoes que o viewer.mjs da
  // ao attachPdf(): paginas por indice, textDivs paralelos aos itens com
  // `str`, e as camadas de texto so existem depois de __renderPage(i).
  g.__pdf = (pages) => {
    const state = { pages, current: 0, shown: [], divs: [], textCalls: [] };
    const container = g.document.createElement('div');
    container.id = 'pages';
    g.document.body.appendChild(container);
    state.els = pages.map((_, i) => {
      const el = g.document.createElement('div');
      el.className = 'page';
      el.dataset.index = String(i);
      container.appendChild(el);
      return el;
    });
    g.__pdfState = state;
    g.__ctl = g.NeuralIAReadAloud.attachPdf({
      window: g,
      lang: 'pt',
      pageCount: () => pages.length,
      currentPage: () => state.current,
      textContent: (i) => {
        state.textCalls.push(i);
        return Promise.resolve(pages[i].map(textItem));
      },
      textDivs: (i) => state.divs[i] || null,
      pageOf: (el) => {
        const page = el && el.closest ? el.closest('.page') : null;
        return page ? Number(page.dataset.index) : null;
      },
      showPage: (i) => {
        state.shown.push(i);
        state.current = i;
      },
    });
    return g.__ctl;
  };
  g.__renderPage = (i) => {
    const state = g.__pdfState;
    const layer = g.document.createElement('div');
    layer.className = 'textLayer';
    state.els[i].appendChild(layer);
    // Como a TextLayer: um marcador abre/fecha um <span class="markedContent">
    // e nao entra nos textDivs; um item vazio tem textDiv mas fica fora do
    // DOM. `data-unit` e a posicao do item no getTextContent().
    const divs = [];
    let parent = layer;
    state.pages[i].forEach((it, k) => {
      if (it.mark === 'end') {
        parent = parent.parentNode || layer;
        return;
      }
      if (it.mark) {
        const marked = g.document.createElement('span');
        marked.className = 'markedContent';
        parent.appendChild(marked);
        parent = marked;
        return;
      }
      const span = g.document.createElement('span');
      span.textContent = it.str;
      span.setAttribute('data-unit', i + ':' + k);
      if (it.str !== '') parent.appendChild(span);
      divs.push(span);
    });
    state.divs[i] = divs;
    g.__ctl.pageReady(i);
    return state.divs[i];
  };

  // O HTML do Modo Leitura (neural-core render.rs), em DOM.
  g.__readerDom = (title, blocks) => {
    const main = g.document.createElement('main');
    main.id = 'neural-shell';
    main.className = 'reader';
    const top = g.document.createElement('div');
    top.className = 'top';
    const home = g.document.createElement('a');
    home.textContent = 'Início';
    top.appendChild(home);
    main.appendChild(top);
    const header = g.document.createElement('header');
    const h1 = g.document.createElement('h1');
    h1.textContent = title;
    h1.setAttribute('data-unit', 'r:title');
    header.appendChild(h1);
    const meta = g.document.createElement('div');
    meta.className = 'meta';
    const source = g.document.createElement('span');
    source.textContent = 'https://exemplo.pt/artigo';
    meta.appendChild(source);
    header.appendChild(meta);
    main.appendChild(header);
    const article = g.document.createElement('article');
    blocks.forEach((b, k) => {
      const el = g.document.createElement(b.tag || 'p');
      el.textContent = b.text;
      el.setAttribute('data-unit', 'r:' + k);
      article.appendChild(el);
    });
    main.appendChild(article);
    g.document.documentElement.lang = 'pt-BR';
    g.document.body.appendChild(main);
    return main;
  };
}

async function main() {
  const context = vm.createContext({ __immediate: setImmediate, __href: INPUT.href || 'http://neuralia-pdf.localhost/viewer.html', URL });
  vm.runInContext('(' + setup.toString() + ')();', context, { filename: 'read_aloud_harness_setup.js' });
  for (const script of INPUT.scripts) {
    vm.runInContext(script.text, context, { filename: script.name });
  }
  let result = null;
  try {
    result = await vm.runInContext('(async () => {\n' + INPUT.drive + '\n})()', context, { filename: 'drive.js' });
  } catch (e) {
    context.__errors.push('drive: ' + (e && e.stack ? e.stack : e));
  }
  process.stdout.write(
    JSON.stringify({
      result: result === undefined ? null : result,
      errors: Array.from(context.__errors, String),
      net: Array.from(context.__net, String),
      posted: Array.from(context.__posted, String),
      nav: Array.from(context.__nav, String),
    })
  );
}

main().catch((e) => {
  process.stderr.write(String(e && e.stack ? e.stack : e));
  process.exit(1);
});
