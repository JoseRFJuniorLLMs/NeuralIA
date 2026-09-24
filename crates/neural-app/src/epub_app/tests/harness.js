'use strict';
// Harness dos gates do leitor de EPUB: corre o JavaScript QUE EMBARCA
// (common.js, reader.js, library.js, tal como `epub_asset` os serve) num
// contexto `node:vm`, com um DOM falso construído a partir do HTML que também
// embarca (reader.html, library.html). Nada aqui é de terceiros.
//
// Entrada (stdin, JSON): { assets: {...}, scenarios: "<fonte>", name, data }.
// `scenarios` é o corpo de uma função (h, assert) que devolve uma tabela de
// cenários async; corre o `name` com `data` e devolve o JSON que ele der.
const vm = require('node:vm');
const assert = require('node:assert/strict');

// `input` e `require` chegam como parâmetros (o arranque em js.rs lê o
// stdin): o harness não cabe numa linha de comando do Windows.
const ORIGIN = 'http://neuralia-epub.localhost';
const XHTML = 'http://www.w3.org/1999/xhtml';
const NBSP = String.fromCharCode(160);
const VOID = new Set(['meta', 'link', 'input', 'br', 'img', 'hr', 'area', 'base', 'col',
  'embed', 'source', 'track', 'wbr']);

// ------------------------------------------------------------------ DOM falso

class FakeEvent {
  constructor(type, init) {
    this.type = type;
    this.defaultPrevented = false;
    this.propagationStopped = false;
    this.button = 0;
    this.shiftKey = false;
    this.ctrlKey = false;
    this.altKey = false;
    this.metaKey = false;
    Object.assign(this, init || {});
  }

  preventDefault() {
    this.defaultPrevented = true;
  }

  stopPropagation() {
    this.propagationStopped = true;
  }
}

class FakeNode {
  constructor(doc) {
    this.ownerDocument = doc;
    this.parentNode = null;
    this.childNodes = [];
    this.listeners = new Map();
  }

  addEventListener(type, fn, options) {
    if (typeof fn !== 'function') return;
    const list = this.listeners.get(type) || [];
    list.push({ fn, once: Boolean(options && options.once) });
    this.listeners.set(type, list);
  }

  removeEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    this.listeners.set(type, list.filter((entry) => entry.fn !== fn));
  }

  listenerCount(type) {
    return (this.listeners.get(type) || []).length;
  }

  // Chama os ouvintes deste nó (sem propagação: o leitor ouve no documento).
  fire(type, init) {
    const event = init instanceof FakeEvent ? init : new FakeEvent(type, init);
    if (!event.target) event.target = this;
    for (const entry of (this.listeners.get(type) || []).slice()) {
      if (entry.once) this.removeEventListener(type, entry.fn);
      entry.fn.call(this, event);
    }
    return event;
  }

  appendChild(child) {
    if (child.parentNode) child.parentNode.removeChild(child);
    child.parentNode = this;
    this.childNodes.push(child);
    return child;
  }

  removeChild(child) {
    this.childNodes = this.childNodes.filter((node) => node !== child);
    child.parentNode = null;
    return child;
  }
}

class FakeText {
  constructor(doc, data) {
    this.nodeType = 3;
    this.ownerDocument = doc;
    this.parentNode = null;
    this.data = String(data);
  }

  get textContent() {
    return this.data;
  }
}

class FakeClassList {
  constructor() {
    this.set = new Set();
  }

  add(...names) {
    for (const name of names) this.set.add(name);
  }

  remove(...names) {
    for (const name of names) this.set.delete(name);
  }

  toggle(name, force) {
    const on = force === undefined ? !this.set.has(name) : Boolean(force);
    if (on) this.set.add(name);
    else this.set.delete(name);
    return on;
  }

  contains(name) {
    return this.set.has(name);
  }
}

function makeStyle() {
  const style = {
    props: {},
    setProperty(name, value, priority) {
      this.props[name] = priority ? value + ' !' + priority : value;
    },
    getPropertyValue(name) {
      return String(this.props[name] || '').replace(/\s*!important$/, '');
    },
    getPropertyPriority(name) {
      return /!important$/.test(String(this.props[name] || '')) ? 'important' : '';
    },
    removeProperty(name) {
      delete this.props[name];
    },
  };
  Object.defineProperty(style, 'fontSize', {
    get() {
      return this.getPropertyValue('font-size');
    },
  });
  return style;
}

// `a: b; c: d !important` -> style.props (o que o navegador faz com style="").
function parseDeclarations(text, style) {
  for (const part of String(text || '').split(';')) {
    const at = part.indexOf(':');
    if (at < 0) continue;
    const name = part.slice(0, at).trim().toLowerCase();
    const raw = part.slice(at + 1).trim();
    if (!name) continue;
    const important = /!\s*important$/i.test(raw);
    style.setProperty(name, raw.replace(/\s*!\s*important$/i, ''), important ? 'important' : '');
  }
}

class FakeElement extends FakeNode {
  constructor(doc, localName, namespaceURI) {
    super(doc);
    this.nodeType = 1;
    this.localName = String(localName).toLowerCase();
    this.tagName = this.localName.toUpperCase();
    this.namespaceURI = namespaceURI || XHTML;
    this.attrs = new Map();
    this.style = makeStyle();
    this.classList = new FakeClassList();
    this.hidden = false;
    this.value = '';
    this.disabled = false;
    this.clientWidth = 0;
    this.clientHeight = 0;
    this.scrollWidth = 0;
    this.scrollHeight = 0;
    this.scrollTop = 0;
  }

  get children() {
    return this.childNodes.filter((node) => node.nodeType === 1);
  }

  get id() {
    return this.getAttribute('id') || '';
  }

  set id(value) {
    this.setAttribute('id', value);
  }

  get className() {
    return Array.from(this.classList.set).join(' ');
  }

  set className(value) {
    this.classList.set = new Set(String(value).split(/\s+/).filter(Boolean));
  }

  setAttribute(name, value) {
    if (name === 'class') {
      this.className = value;
      return;
    }
    this.attrs.set(name, String(value));
    if (name === 'style') parseDeclarations(value, this.style);
  }

  getAttribute(name) {
    if (name === 'class') return this.className || null;
    return this.attrs.has(name) ? this.attrs.get(name) : null;
  }

  hasAttribute(name) {
    return name === 'class' ? this.classList.set.size > 0 : this.attrs.has(name);
  }

  removeAttribute(name) {
    this.attrs.delete(name);
  }

  hasAttributeNS(ns, name) {
    return this.attrs.has('xlink:' + name) && ns === 'http://www.w3.org/1999/xlink';
  }

  getAttributeNS(ns, name) {
    return this.hasAttributeNS(ns, name) ? this.attrs.get('xlink:' + name) : null;
  }

  get textContent() {
    return this.childNodes.map((node) => node.textContent).join('');
  }

  set textContent(value) {
    for (const node of this.childNodes) node.parentNode = null;
    this.childNodes = [];
    const text = value == null ? '' : String(value);
    if (text) this.appendChild(this.ownerDocument.createTextNode(text));
  }

  matches(selector) {
    if (selector.startsWith('.')) return this.classList.contains(selector.slice(1));
    if (selector.startsWith('[') && selector.endsWith(']')) return this.attrs.has(selector.slice(1, -1));
    return this.localName === selector.toLowerCase();
  }

  querySelectorAll(selector) {
    return descendants(this).filter((node) => node.matches(selector));
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  getElementsByTagName(name) {
    const all = descendants(this);
    const wanted = String(name).toLowerCase();
    return wanted === '*' ? all : all.filter((node) => node.localName === wanted);
  }

  contains(node) {
    for (let current = node; current; current = current.parentNode) {
      if (current === this) return true;
    }
    return false;
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }

  select() {
    this.selected = true;
  }

  scrollIntoView() {
    this.scrolledIntoView = (this.scrolledIntoView || 0) + 1;
  }

  // Posição "de layout" que o teste escolhe (absX/absY), vista a partir do
  // scroll atual da janela do documento, como no navegador. Com o modelo de
  // fluxo (`flow`), a posição sai do texto e do CSS em vigor.
  getBoundingClientRect() {
    const flowed = this.ownerDocument.flowRectOf ? this.ownerDocument.flowRectOf(this) : null;
    if (flowed) return flowed;
    const win = this.ownerDocument.defaultView;
    const x = (this.absX || 0) - (win ? win.scrollX || 0 : 0);
    const y = (this.absY || 0) - (win ? win.scrollY || 0 : 0);
    return { left: x, top: y, right: x + 10, bottom: y + 10, width: this.rectWidth || 10, height: 10 };
  }

  getClientRects() {
    return [this.getBoundingClientRect()];
  }
}

class FakeFrame extends FakeElement {
  constructor(doc) {
    super(doc, 'iframe');
    this.loads = [];
    // Como cada carga foi pedida: 'src' (atributo) ou 'replace'
    // (contentWindow.location.replace).
    this.how = [];
    // O sandbox em vigor no momento de cada carga.
    this.sandboxAtLoad = [];
    this.contentDocument = null;
    this.contentWindow = null;
    this.srcValue = '';
    // O endereço que o iframe está a carregar (o último pedido).
    this.current = '';
    // Entradas que as cargas acrescentaram ao histórico da página: mudar o
    // `src` de um iframe que já mostra um documento acrescenta uma, como no
    // Chromium; `location.replace` não.
    this.historyAdded = 0;
  }

  get src() {
    return this.srcValue;
  }

  set src(value) {
    this.srcValue = String(value);
    this.navigate(this.srcValue, 'src');
  }

  navigate(url, how) {
    if (how === 'src' && this.contentDocument) this.historyAdded++;
    this.current = String(url);
    this.loads.push(this.current);
    this.how.push(how);
    this.sandboxAtLoad.push(this.getAttribute('sandbox'));
  }
}

function descendants(root) {
  const out = [];
  const stack = root.childNodes.slice().reverse();
  while (stack.length) {
    const node = stack.pop();
    if (node.nodeType !== 1) continue;
    out.push(node);
    for (let i = node.childNodes.length - 1; i >= 0; i--) stack.push(node.childNodes[i]);
  }
  return out;
}

function textNodes(root) {
  const out = [];
  const stack = [root];
  while (stack.length) {
    const node = stack.pop();
    if (node.nodeType === 3) {
      out.push(node);
      continue;
    }
    for (let i = node.childNodes.length - 1; i >= 0; i--) stack.push(node.childNodes[i]);
  }
  return out;
}

class FakeRange {
  constructor(doc) {
    this.doc = doc;
    this.startContainer = null;
    this.startOffset = 0;
    this.endContainer = null;
    this.endOffset = 0;
  }

  setStart(node, offset) {
    this.startContainer = node;
    this.startOffset = offset;
  }

  setEnd(node, offset) {
    this.endContainer = node;
    this.endOffset = offset;
  }

  // O texto coberto (os nós de texto entre o início e o fim).
  collapse(toStart) {
    if (toStart) {
      this.endContainer = this.startContainer;
      this.endOffset = this.startOffset;
    } else {
      this.startContainer = this.endContainer;
      this.startOffset = this.endOffset;
    }
  }

  toString() {
    const nodes = textNodes(this.doc.documentElement);
    const from = nodes.indexOf(this.startContainer);
    const to = nodes.indexOf(this.endContainer);
    if (from < 0 || to < 0) return '';
    if (from === to) return this.startContainer.data.slice(this.startOffset, this.endOffset);
    let text = this.startContainer.data.slice(this.startOffset);
    for (let i = from + 1; i < to; i++) text += nodes[i].data;
    return text + this.endContainer.data.slice(0, this.endOffset);
  }

  getClientRects() {
    const flowed = this.doc.flowRectAt ? this.doc.flowRectAt(this.startContainer, this.startOffset) : null;
    if (flowed) return [flowed];
    const element = this.startContainer && this.startContainer.parentNode;
    return element && element.getBoundingClientRect ? [element.getBoundingClientRect()] : [];
  }

  getBoundingClientRect() {
    return this.getClientRects()[0] || { left: 0, top: 0, width: 0, height: 0 };
  }
}

class FakeDocument extends FakeNode {
  constructor() {
    super(null);
    this.nodeType = 9;
    this.documentElement = null;
    this.activeElement = null;
    this.title = '';
    this.defaultView = null;
    this.readyState = 'complete';
    this.baseURI = ORIGIN + '/';
  }

  get head() {
    return this.documentElement ? this.documentElement.querySelector('head') : null;
  }

  get body() {
    return this.documentElement ? this.documentElement.querySelector('body') : null;
  }

  get scrollingElement() {
    return this.documentElement;
  }

  createElement(tag) {
    return tag === 'iframe' ? new FakeFrame(this) : new FakeElement(this, tag);
  }

  createElementNS(ns, tag) {
    return new FakeElement(this, tag, ns);
  }

  createTextNode(text) {
    return new FakeText(this, text);
  }

  createRange() {
    return new FakeRange(this);
  }

  getElementById(id) {
    if (!this.documentElement) return null;
    if (this.documentElement.getAttribute('id') === id) return this.documentElement;
    return descendants(this.documentElement).find((node) => node.getAttribute('id') === id) || null;
  }

  getElementsByName(name) {
    return this.documentElement
      ? descendants(this.documentElement).filter((node) => node.getAttribute('name') === name)
      : [];
  }

  getElementsByTagName(name) {
    if (!this.documentElement) return [];
    const all = [this.documentElement].concat(descendants(this.documentElement));
    return all.filter((node) => node.localName === name);
  }

  // As folhas dos <style> do documento, como CSSOM mínimo: regras com
  // `style` (getPropertyValue/setProperty). Uma mudança via CSSOM fica na
  // regra, como no navegador.
  get styleSheets() {
    return this.getElementsByTagName('style').map((element) => {
      const text = element.textContent;
      if (!element.sheetCache || element.sheetCache.text !== text) {
        const cssRules = [];
        for (const chunk of text.split('}')) {
          const open = chunk.indexOf('{');
          if (open < 0) continue;
          const style = makeStyle();
          parseDeclarations(chunk.slice(open + 1), style);
          cssRules.push({ selectorText: chunk.slice(0, open).trim(), style, cssRules: null });
        }
        element.sheetCache = { text, sheet: { cssRules } };
      }
      return element.sheetCache.sheet;
    });
  }

  querySelectorAll(selector) {
    return this.documentElement ? this.documentElement.querySelectorAll(selector) : [];
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  // Só nós de texto (whatToShow = SHOW_TEXT), em ordem de documento.
  createTreeWalker(root) {
    const nodes = textNodes(root);
    let index = -1;
    return {
      nextNode() {
        index++;
        return index < nodes.length ? nodes[index] : null;
      },
    };
  }
}

function decodeEntities(text) {
  const named = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: NBSP };
  return text.replace(/&(#[xX][0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);/g, (whole, name) => {
    if (name[0] === '#') {
      const hex = name[1] === 'x' || name[1] === 'X';
      return String.fromCodePoint(parseInt(name.slice(hex ? 2 : 1), hex ? 16 : 10));
    }
    return Object.prototype.hasOwnProperty.call(named, name) ? named[name] : whole;
  });
}

// Um parser de marcação mínimo, suficiente para o HTML que o NeuralIA embarca
// e para os capítulos escritos nos testes.
function parseMarkup(doc, markup) {
  const holder = new FakeElement(doc, '#holder');
  const stack = [holder];
  const re = /<!--[\s\S]*?-->|<![^>]*>|<\?[\s\S]*?\?>|<(\/?)([A-Za-z][A-Za-z0-9:-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>|([^<]+)/g;
  let match;
  while ((match = re.exec(markup))) {
    const top = stack[stack.length - 1];
    if (match[4] !== undefined) {
      top.appendChild(doc.createTextNode(decodeEntities(match[4])));
      continue;
    }
    if (!match[2]) continue;
    const name = match[2].toLowerCase();
    if (match[1]) {
      for (let i = stack.length - 1; i > 0; i--) {
        if (stack[i].localName === name) {
          stack.length = i;
          break;
        }
      }
      continue;
    }
    const attrText = match[3] || '';
    const element = doc.createElement(name);
    const attrRe = /([^\s=/]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
    let attr;
    while ((attr = attrRe.exec(attrText))) {
      const value = attr[2] !== undefined ? attr[2] : attr[3] !== undefined ? attr[3] : attr[4] || '';
      element.setAttribute(attr[1], decodeEntities(value));
    }
    if (element.hasAttribute('hidden')) element.hidden = true;
    if (element.hasAttribute('value')) element.value = element.getAttribute('value');
    top.appendChild(element);
    if (!/\/\s*$/.test(attrText) && !VOID.has(name)) stack.push(element);
  }
  for (const select of holder.querySelectorAll('select')) {
    const options = select.querySelectorAll('option');
    const chosen = options.find((option) => option.hasAttribute('selected')) || options[0];
    if (chosen) select.value = chosen.getAttribute('value') || '';
  }
  return holder;
}

function documentFrom(markup, url) {
  const doc = new FakeDocument();
  const holder = parseMarkup(doc, markup);
  let html = holder.children.find((node) => node.localName === 'html');
  if (!html) {
    html = doc.createElement('html');
    const body = doc.createElement('body');
    for (const node of holder.childNodes.slice()) body.appendChild(node);
    html.appendChild(body);
  }
  doc.documentElement = html;
  html.parentNode = doc;
  if (url) doc.baseURI = url;
  return doc;
}

// ------------------------------------------------------------- janelas falsas

function makeTimers() {
  const pending = new Map();
  let next = 1;
  return {
    pending,
    setTimeout(fn, delay) {
      const id = next++;
      pending.set(id, { fn, delay });
      return id;
    },
    clearTimeout(id) {
      pending.delete(id);
    },
    // Corre o que está armado (e o que isso armar), sem relógio.
    run() {
      let rounds = 0;
      while (pending.size && rounds++ < 100) {
        const [id, entry] = pending.entries().next().value;
        pending.delete(id);
        entry.fn();
      }
    },
  };
}

function makeStorage(initial) {
  const map = new Map(Object.entries(initial || {}));
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
    map,
  };
}

function response(status, body, contentType) {
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => (String(name).toLowerCase() === 'content-type' ? contentType || 'application/json' : null) },
    json: async () => JSON.parse(text),
    text: async () => text,
  };
}

// A janela de uma das nossas páginas: o global onde os scripts embarcados
// correm. `routes` responde ao fetch; `ipc` recolhe o que a página manda ao
// nativo.
function makePage(html, options) {
  const opts = options || {};
  const doc = documentFrom(html, ORIGIN + (opts.path || '/'));
  const timers = makeTimers();
  const posts = [];
  const navigations = [];
  const fetched = [];
  const routes = opts.routes || {};
  const win = {
    document: doc,
    console,
    URL,
    URLSearchParams,
    timers,
    posts,
    navigations,
    fetched,
    setTimeout: (fn, delay) => timers.setTimeout(fn, delay),
    clearTimeout: (id) => timers.clearTimeout(id),
    requestAnimationFrame: (fn) => timers.setTimeout(fn, 16),
    localStorage: makeStorage(opts.storage),
    matchMedia: () => ({ matches: Boolean(opts.dark), addEventListener() {} }),
    ipc: { postMessage: (text) => posts.push(JSON.parse(text)) },
    fetch: async (path) => {
      fetched.push(String(path));
      const route = routes[String(path)];
      if (typeof route === 'function') return route();
      if (!route) return response(404, { error: 'não encontrado' });
      return response(route.status || 200, route.body, route.type);
    },
    listeners: new Map(),
    addEventListener(type, fn) {
      const list = this.listeners.get(type) || [];
      list.push(fn);
      this.listeners.set(type, list);
    },
    fireWindow(type) {
      for (const fn of this.listeners.get(type) || []) fn({ type });
    },
    location: null,
    DOMParser: class {
      parseFromString(source, type) {
        return parseChapterMarkup(source, type);
      }
    },
  };
  const location = {
    search: opts.search || '',
    get href() {
      return ORIGIN + (opts.path || '/') + (opts.search || '');
    },
    set href(value) {
      navigations.push(String(value));
    },
  };
  win.location = location;
  win.window = win;
  if (opts.speech) {
    win.speechSynthesis = opts.speech.synth;
    win.SpeechSynthesisUtterance = opts.speech.Utterance;
  }
  doc.defaultView = win;
  const context = vm.createContext(win);
  for (const name of opts.scripts || []) {
    vm.runInContext(input.assets[name], context, { filename: name });
  }
  return { win, doc, context, timers, posts, navigations, fetched };
}

// O parser XML do navegador recusa entidades HTML sem DTD: o documento sai
// com um <parsererror>. O de HTML aceita.
function parseChapterMarkup(source, type) {
  const doc = documentFrom(source);
  if (type !== 'text/html' && /&nbsp;/.test(source)) {
    const error = doc.createElement('parsererror');
    error.appendChild(doc.createTextNode('Entity nbsp not defined'));
    const root = doc.documentElement;
    root.childNodes.unshift(error);
    error.parentNode = root;
  }
  return doc;
}

// Um modelo de paginação para os cenários que mudam a tipografia: o texto
// corre em colunas de `flow.chars` letras (a 100% e com coluna de 520 px;
// menos com a fonte maior, mais com a coluna mais larga), e no modo de
// rolagem cada letra ocupa `flow.px` píxeis de altura. Lê o CSS que o leitor
// injetou, como o navegador.
function flowState(doc, flow) {
  const style = doc.getElementById('neuralia-reader-style');
  const css = style ? style.textContent : '';
  const num = (re, fallback) => {
    const match = css.match(re);
    return match ? Number(match[1]) : fallback;
  };
  const fontSize = num(/font-size: (\d+)% !important/, 100);
  const paged = /column-width: [\d.]+px/.test(css);
  const pageWidth = num(/html \{ width: (\d+)px !important/, flow.width || 1200);
  const columnWidth = num(/column-width: ([\d.]+)px/, pageWidth);
  const gap = num(/column-gap: ([\d.]+)px/, 0);
  const perPage = paged ? Math.max(1, Math.round(pageWidth / (columnWidth + gap))) : 1;
  const step = pageWidth / perPage;
  const capacity = Math.max(1, Math.floor(flow.chars * (100 / fontSize) * (columnWidth / 520)));
  const pxPerChar = (flow.px || 2) * fontSize / 100;
  const runs = [];
  let total = 0;
  for (const node of textNodes(doc.body || doc.documentElement)) {
    runs.push({ node, start: total });
    total += node.data.length;
  }
  const columns = Math.max(1, Math.ceil(total / capacity));
  return {
    paged, step, gap, capacity, pxPerChar, runs, total, pageWidth,
    contentWidth: paged ? columns * step - gap / 2 : pageWidth,
    contentHeight: Math.ceil(total * pxPerChar),
  };
}

// O documento do capítulo dentro do iframe, e a sua janela.
function makeBookDocument(markup, url, layout) {
  const opts = layout || {};
  const doc = parseChapterMarkup(markup, opts.type || 'application/xhtml+xml');
  doc.baseURI = url;
  doc.readyState = opts.ready || 'complete';
  const root = doc.documentElement;
  const rtl = String(root.getAttribute('dir') || '').toLowerCase() === 'rtl';
  const flow = opts.flow || null;
  const state = () => flowState(doc, flow);
  // Como o Chromium: o que se pode rolar vai até ao fim do conteúdo OU de um
  // elemento posicionado (o marcador de fim de página do leitor).
  const extent = (content) => {
    let far = content;
    for (const node of descendants(root)) {
      const props = node.style && node.style.props;
      if (!props || !/absolute/.test(props.position || '') || /none/.test(props.display || '')) continue;
      const left = parseFloat(props.left);
      const offset = left >= 0 ? left : parseFloat(props.right);
      if (offset >= 0) far = Math.max(far, offset + 1);
    }
    return far;
  };
  Object.defineProperty(root, 'scrollWidth', {
    configurable: true,
    get: () => extent(flow ? state().contentWidth : opts.scrollWidth || 0),
  });
  Object.defineProperty(root, 'scrollHeight', {
    configurable: true,
    get: () => (flow ? state().contentHeight : opts.scrollHeight || 0),
  });
  root.clientHeight = opts.clientHeight || 0;
  const selection = { isCollapsed: true, ranges: [], removeAllRanges() { this.ranges = []; }, addRange(range) { this.ranges.push(range); } };
  const viewport = () => (opts.frame ? parseFloat(opts.frame.style.width) : NaN);
  const win = {
    location: {
      href: url,
      replace(next) {
        if (opts.frame) opts.frame.navigate(next, 'replace');
      },
    },
    scrollX: 0,
    scrollY: 0,
    scrolls: [],
    // O navegador não rola além do conteúdo: [0, scrollWidth - largura], e
    // da direita para a esquerda [-(scrollWidth - largura), 0].
    scrollTo(x, y) {
      let left = x;
      const width = viewport();
      if (width > 0) {
        const max = Math.max(0, root.scrollWidth - width);
        left = rtl ? Math.min(0, Math.max(-max, x)) : Math.max(0, Math.min(max, x));
      }
      this.scrollX = left;
      this.scrollY = y;
      root.scrollTop = y;
      this.scrolls.push([left, y]);
    },
    scrollBy(dx, dy) {
      this.scrollTo(this.scrollX + dx, this.scrollY + dy);
    },
    getSelection: () => selection,
    CSS: { highlights: new Map() },
    Highlight: class {
      constructor(...ranges) {
        this.ranges = ranges;
      }
    },
    // O estilo calculado que o leitor consulta: direção (atributo `dir`) e
    // quebras (do `style=""`; `page-break-*: always` vira `break-*: page`).
    getComputedStyle(element) {
      let direction = 'ltr';
      for (let node = element; node && node.nodeType === 1; node = node.parentNode) {
        const dir = node.getAttribute('dir');
        if (dir) {
          direction = dir.toLowerCase() === 'rtl' ? 'rtl' : 'ltr';
          break;
        }
      }
      const read = (name) => (element.style ? element.style.getPropertyValue(name) : '');
      const legacy = (name) => read('page-break-' + name) || 'auto';
      const modern = (name) => read('break-' + name) || (legacy(name) === 'always' ? 'page' : 'auto');
      return {
        direction,
        breakBefore: modern('before'),
        breakAfter: modern('after'),
        pageBreakBefore: legacy('before'),
        pageBreakAfter: legacy('after'),
      };
    },
  };
  doc.defaultView = win;
  if (flow) {
    const indexOf = (node, offset) => {
      const s = state();
      const run = s.runs.find((item) => item.node === node);
      if (run) return run.start + Math.max(0, Math.min(offset || 0, node.data.length));
      const first = node && node.nodeType === 1 ? textNodes(node)[0] : null;
      const inner = first ? s.runs.find((item) => item.node === first) : null;
      return inner ? inner.start : null;
    };
    const rectAt = (index) => {
      const s = state();
      let x;
      let y;
      if (s.paged) {
        const column = Math.floor(index / s.capacity);
        // Da direita para a esquerda a coluna 0 é a da direita da página 0.
        x = rtl ? s.pageWidth - (column + 1) * s.step + s.gap / 2 : column * s.step + s.gap / 2;
        y = 40 + (index % s.capacity) / s.capacity * 600;
      } else {
        x = 100;
        y = index * s.pxPerChar;
      }
      const left = x - win.scrollX;
      const top = y - win.scrollY;
      return { left, top, right: left + 8, bottom: top + 16, width: 8, height: 16 };
    };
    doc.flowRectAt = (node, offset) => {
      const index = indexOf(node, offset);
      return index == null ? null : rectAt(index);
    };
    doc.flowRectOf = (element) => {
      const index = indexOf(element, 0);
      return index == null ? null : rectAt(index);
    };
    // O ponto de texto num ponto do iframe: no modo de páginas, o início da
    // coluna que está ali; na rolagem, a letra àquela altura.
    doc.caretRangeFromPoint = (x, y) => {
      const s = state();
      if (!s.total) return null;
      let index;
      if (s.paged) {
        const absolute = x + win.scrollX;
        const column = rtl ? Math.floor((s.pageWidth - absolute) / s.step) : Math.floor(absolute / s.step);
        index = Math.max(0, column) * s.capacity;
      } else {
        index = Math.floor((y + win.scrollY) / s.pxPerChar);
      }
      index = Math.max(0, Math.min(s.total - 1, index));
      const run = s.runs.filter((item) => item.start <= index).pop();
      const range = doc.createRange();
      range.setStart(run.node, index - run.start);
      range.setEnd(run.node, index - run.start);
      return range;
    };
  }
  for (const [id, where] of Object.entries(opts.places || {})) {
    const element = doc.getElementById(id);
    if (!element) throw new Error('sem elemento #' + id);
    element.absX = where.x;
    element.absY = where.y || 0;
  }
  return { doc, win, layout: opts };
}

// O documento novo entra no iframe (lido, "interactive"), sem o `load`: as
// imagens ainda estão a chegar.
function commitFrame(frame, markup, layout) {
  const src = frame.current;
  const url = src.startsWith('http') ? src : ORIGIN + src;
  const book = makeBookDocument(markup, url, Object.assign({ ready: 'interactive' }, layout || {}, { frame }));
  frame.contentDocument = book.doc;
  frame.contentWindow = book.win;
  return book;
}

// Entrega o capítulo ao iframe do leitor e dispara o `load`.
function loadFrame(frame, markup, layout) {
  const book = commitFrame(frame, markup, Object.assign({}, layout || {}, { ready: 'complete' }));
  frame.fire('load');
  return book;
}

function speech(voices) {
  const spoken = [];
  const synth = {
    voices,
    spoken,
    cancelled: 0,
    listeners: [],
    getVoices() {
      return this.voices;
    },
    speak(utterance) {
      spoken.push(utterance);
    },
    // Como no Chromium: cancelar interrompe a frase em curso com um erro.
    cancel() {
      this.cancelled++;
      const current = spoken[spoken.length - 1];
      if (current && !current.done && typeof current.onerror === 'function') {
        current.done = true;
        current.onerror({ error: 'interrupted' });
      }
    },
    addEventListener(type, fn) {
      this.listeners.push(fn);
    },
    removeEventListener() {},
  };
  class Utterance {
    constructor(text) {
      this.text = text;
      this.voice = null;
      this.lang = '';
      this.rate = 1;
    }

    finish() {
      this.done = true;
      if (typeof this.onend === 'function') this.onend({});
    }
  }
  return { synth, Utterance };
}

async function settle() {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
}

const helpers = {
  ORIGIN,
  asset: (name) => {
    if (typeof input.assets[name] !== 'string') throw new Error('asset em falta: ' + name);
    return input.assets[name];
  },
  FakeEvent,
  documentFrom,
  makePage,
  makeBookDocument,
  commitFrame,
  loadFrame,
  parseChapterMarkup,
  speech,
  settle,
  response,
  text: (node) => (node ? node.textContent : null),
  run: (context, source) => vm.runInContext(source, context),
  // Tudo o que a página tem com um atributo `on...` ou um <script> inline.
  inlineCode(doc) {
    const found = [];
    for (const element of [doc.documentElement].concat(descendants(doc.documentElement))) {
      for (const name of element.attrs.keys()) {
        if (/^on/i.test(name)) found.push(element.localName + '[' + name + ']');
      }
      if (element.localName === 'script' && (!element.getAttribute('src') || element.textContent.trim())) {
        found.push('script inline');
      }
      if (element.localName === 'style' || element.hasAttribute('style')) found.push(element.localName + ' style');
    }
    return found;
  },
};

// Relógio falso para o que a página mede com Date.now() (a trava da roda).
helpers.setNow = (context, now) => vm.runInContext('Date.now = () => ' + Number(now) + ';', context);

// Objetos criados dentro do contexto têm outro Object.prototype: o
// deepEqual estrito compara-os pelo JSON.
const plain = (value) => (value === undefined ? value : JSON.parse(JSON.stringify(value)));
const check = Object.assign({}, assert, {
  deepEqual: (actual, expected, message) => assert.deepEqual(plain(actual), plain(expected), message),
});
const table = new Function('h', 'assert', input.scenarios)(helpers, check);
const scenario = table[input.name];
if (typeof scenario !== 'function') throw new Error('cenário desconhecido: ' + input.name);
Promise.resolve()
  .then(() => scenario(input.data || {}))
  .then((out) => process.stdout.write(JSON.stringify(out === undefined ? null : out)))
  .catch((error) => {
    process.stderr.write(String((error && error.stack) || error));
    process.exit(1);
  });
