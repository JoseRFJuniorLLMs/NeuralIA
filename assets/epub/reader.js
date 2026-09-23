'use strict';
// NeuralIA — leitor de EPUB (estilo do visualizador do Calibre).
//
// O livro abre num <iframe sandbox="allow-same-origin"> SEM allow-scripts:
// nenhum script do livro corre. Esta página, na mesma origem, injeta o
// estilo de paginação (colunas CSS), vira as páginas, trata os cliques em
// links, pesquisa, guarda a posição e lê em voz alta. Ficheiro do próprio
// NeuralIA, servido com `script-src 'self'`.
(function (root) {
  const E = root.NeuraliaEpub;
  const XHTML_NS = 'http://www.w3.org/1999/xhtml';
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const XLINK_NS = 'http://www.w3.org/1999/xlink';
  const STYLE_ID = 'neuralia-reader-style';
  const SETTINGS_KEY = 'neuralia-epub-settings';
  const SAVE_DELAY_MS = 800;
  const TWO_COLUMN_MIN_WIDTH = 1000;
  const MIN_COLUMN_WIDTH = 220;
  const SCROLL_MAX_WIDTH = 820;
  const MAX_RESULTS = 300;
  const CONTEXT_CHARS = 48;
  const MAX_SPOKEN_CHARS = 280;
  const MAX_LABEL_CHARS = 200;

  const MARGINS = { narrow: 16, normal: 40, wide: 80 };
  const FONT_STEPS = [60, 70, 80, 90, 100, 110, 120, 135, 150, 170, 200, 240];
  const LINE_HEIGHTS = ['original', '1.3', '1.5', '1.7', '2.0'];
  const RATES = ['0.8', '1', '1.2', '1.5'];
  const FONT_FAMILIES = {
    original: null,
    serif: "Georgia, 'Times New Roman', 'Noto Serif', serif",
    sans: "'Segoe UI', Arial, 'Noto Sans', sans-serif",
  };
  const THEMES = {
    light: { bg: '#ffffff', fg: '#1b1b1b', link: '#1a55c4' },
    sepia: { bg: '#f4ecd8', fg: '#5b4636', link: '#8a4b1f' },
    dark: { bg: '#1c1d20', fg: '#d9d9d6', link: '#8ab4f8' },
  };
  const DEFAULT_SETTINGS = Object.freeze({
    fontSize: 100,
    fontFamily: 'original',
    lineHeight: 'original',
    margin: 'normal',
    align: 'original',
    theme: 'system',
    mode: 'paged',
    columns: 'auto',
    rate: '1',
  });
  const LANGUAGE_ALIASES = { por: 'pt', eng: 'en', spa: 'es', fra: 'fr', fre: 'fr', deu: 'de', ger: 'de', ita: 'it' };

  // ------------------------------------------------------------ paginação

  // Duas colunas quando a largura dá para isso (como o Calibre).
  function columnsFor(width, setting) {
    if (setting === '1' || setting === 1) return 1;
    if (setting === '2' || setting === 2) return width >= 2 * MIN_COLUMN_WIDTH + 120 ? 2 : 1;
    return width >= TWO_COLUMN_MIN_WIDTH ? 2 : 1;
  }

  // A página tem a largura do iframe; cada coluna fica com metade do
  // espaço entre colunas de cada lado, por isso o passo de uma coluna para
  // a seguinte é exatamente pageWidth / columns e a página N começa em
  // N * pageWidth.
  function pageGeometry(width, height, columns, margin) {
    const count = columns === 2 ? 2 : 1;
    const pageWidth = Math.max(count, Math.floor(Math.max(1, width) / count) * count);
    const pageHeight = Math.max(1, Math.floor(height));
    let gap = Math.max(0, Math.round(Number(margin) || 0)) * 2;
    while (gap > 8 && (pageWidth - count * gap) / count < MIN_COLUMN_WIDTH) gap = Math.round(gap / 2);
    const columnWidth = Math.max(1, (pageWidth - count * gap) / count);
    return { pageWidth, pageHeight, columns: count, gap, columnWidth };
  }

  // Quantas páginas cabem numa largura de conteúdo. Dois píxeis de folga:
  // o arredondamento das colunas não inventa uma página vazia no fim.
  function pageCount(scrollWidth, pageWidth) {
    if (!(pageWidth > 0) || !(scrollWidth > 0)) return 1;
    return Math.max(1, Math.ceil((scrollWidth - 2) / pageWidth));
  }

  function fractionForPage(page, pages) {
    if (!(pages > 1)) return 0;
    return E.clamp(page / pages, 0, 1);
  }

  function pageForFraction(fraction, pages) {
    if (!(pages > 1)) return 0;
    return Math.min(pages - 1, Math.max(0, Math.floor(E.clamp(fraction, 0, 1) * pages + 1e-6)));
  }

  function scrollFraction(scrollTop, scrollHeight) {
    if (!(scrollHeight > 0)) return 0;
    return E.clamp(scrollTop / scrollHeight, 0, 1);
  }

  // Um passo para a frente (+1) ou para trás (-1) no modo de páginas: dentro
  // do capítulo, ou para o capítulo vizinho (o anterior abre na última
  // página).
  function stepPaged(page, pages, spine, spineCount, direction) {
    if (direction > 0) {
      if (page < pages - 1) return { kind: 'page', page: page + 1 };
      if (spine < spineCount - 1) return { kind: 'chapter', spine: spine + 1, target: { page: 0 } };
      return { kind: 'none' };
    }
    if (page > 0) return { kind: 'page', page: page - 1 };
    if (spine > 0) return { kind: 'chapter', spine: spine - 1, target: { page: 'last' } };
    return { kind: 'none' };
  }

  // Zonas de clique do Calibre: o terço... aqui 30% de cada lado.
  function clickZone(x, width) {
    if (!(width > 0)) return 0;
    if (x < width * 0.3) return -1;
    if (x > width * 0.7) return 1;
    return 0;
  }

  // Progresso no livro inteiro, pesando cada documento pelo tamanho (a
  // mesma conta que o lado nativo usa na biblioteca).
  function bookProgress(sizes, spine, fraction) {
    const count = Array.isArray(sizes) ? sizes.length : 0;
    const f = E.clamp(fraction, 0, 1);
    if (!count) return 0;
    const weight = (value) => (Number(value) > 0 ? Number(value) : 0);
    const total = sizes.reduce((sum, value) => sum + weight(value), 0);
    if (total > 0 && spine >= 0 && spine < count) {
      let before = 0;
      for (let i = 0; i < spine; i++) before += weight(sizes[i]);
      return E.clamp((before + weight(sizes[spine]) * f) / total, 0, 1);
    }
    return E.clamp((spine + f) / count, 0, 1);
  }

  // O inverso: um ponto da barra de progresso → documento e fração.
  function locateProgress(sizes, value) {
    const count = Array.isArray(sizes) ? sizes.length : 0;
    const p = E.clamp(value, 0, 1);
    if (!count) return { spine: 0, fraction: 0 };
    const weight = (v) => (Number(v) > 0 ? Number(v) : 0);
    const total = sizes.reduce((sum, v) => sum + weight(v), 0);
    if (total <= 0) {
      const x = p * count;
      const spine = Math.min(count - 1, Math.floor(x));
      return { spine, fraction: E.clamp(x - spine, 0, 1) };
    }
    const target = p * total;
    let acc = 0;
    for (let i = 0; i < count; i++) {
      const size = weight(sizes[i]);
      if (target < acc + size || i === count - 1) {
        return { spine: i, fraction: size > 0 ? E.clamp((target - acc) / size, 0, 1) : 0 };
      }
      acc += size;
    }
    return { spine: count - 1, fraction: 1 };
  }

  // Onde reabrir: a posição gravada, se ainda couber no livro.
  function restorePosition(position, spineLength) {
    if (position && Number.isInteger(position.spine) && position.spine >= 0 &&
        position.spine < spineLength) {
      return { spine: position.spine, fraction: E.clamp(position.fraction, 0, 1) };
    }
    return { spine: 0, fraction: 0 };
  }

  // ------------------------------------------------------------- sumário

  // A árvore do sumário em pré-ordem, com a profundidade (sem recursão).
  // Sem sumário, uma entrada por documento do spine.
  function flattenToc(toc, spineLength) {
    const out = [];
    const stack = [{ items: Array.isArray(toc) ? toc : [], index: 0, depth: 0 }];
    while (stack.length) {
      const top = stack[stack.length - 1];
      if (top.index >= top.items.length) {
        stack.pop();
        continue;
      }
      const item = top.items[top.index++];
      if (!item || typeof item !== 'object') continue;
      const spine = Number.isInteger(item.spine) && item.spine >= 0 && item.spine < spineLength
        ? item.spine
        : null;
      out.push({
        index: out.length,
        label: String(item.label || '').trim() || 'Sem título',
        spine,
        fragment: typeof item.fragment === 'string' && item.fragment ? item.fragment : null,
        depth: top.depth,
      });
      if (Array.isArray(item.children) && item.children.length && stack.length < 64) {
        stack.push({ items: item.children, index: 0, depth: top.depth + 1 });
      }
    }
    if (!out.length) {
      for (let i = 0; i < spineLength; i++) {
        out.push({ index: i, label: 'Seção ' + (i + 1), spine: i, fragment: null, depth: 0 });
      }
    }
    return out;
  }

  // A entrada do sumário onde o leitor está: a última (em ordem de leitura)
  // que começa antes do ponto atual. `here` é a página (ou o scroll) no
  // capítulo atual; `anchorAt(fragmento)` diz onde começa uma âncora deste
  // capítulo, na mesma unidade (null se não se sabe: vale o início).
  function currentTocIndex(flat, spine, here, anchorAt) {
    let best = -1;
    let bestSpine = -1;
    let bestAt = -1;
    for (const entry of flat) {
      if (entry.spine == null || entry.spine > spine) continue;
      let at = 0;
      if (entry.spine === spine && entry.fragment) {
        const found = anchorAt(entry.fragment);
        at = typeof found === 'number' && found >= 0 ? found : 0;
        if (at > here) continue;
      }
      if (entry.spine > bestSpine || (entry.spine === bestSpine && at >= bestAt)) {
        best = entry.index;
        bestSpine = entry.spine;
        bestAt = at;
      }
    }
    return best;
  }

  // --------------------------------------------------------------- links

  function safeDecode(text) {
    try {
      return decodeURIComponent(text);
    } catch (_) {
      return null;
    }
  }

  // Para onde vai um link do livro (URL já absoluta). Interno: um
  // documento do spine deste livro (e o fragmento). Externo: http(s) de
  // outra origem, que o lado nativo ainda valida. Tudo o resto
  // (javascript:, data:, mailto:, file:, recursos que não são capítulos) é
  // ignorado.
  function classifyLink(href, bookId, spinePaths) {
    if (typeof href !== 'string' || !href) return { kind: 'ignore' };
    let url;
    try {
      url = new URL(href);
    } catch (_) {
      return { kind: 'ignore' };
    }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return { kind: 'ignore' };
    if (url.origin !== E.ORIGIN) return { kind: 'external', url: url.href };
    const prefix = '/book/' + bookId + '/';
    if (!E.isBookId(bookId) || !url.pathname.startsWith(prefix)) return { kind: 'ignore' };
    const path = safeDecode(url.pathname.slice(prefix.length));
    if (path == null) return { kind: 'ignore' };
    const spine = spinePaths.indexOf(path);
    if (spine < 0) return { kind: 'ignore' };
    const fragment = url.hash.length > 1 ? safeDecode(url.hash.slice(1)) : null;
    return { kind: 'internal', spine, fragment: fragment || null };
  }

  // O que um clique num link faz. O externo guarda a posição antes de
  // sair: a página vai ser trocada pela vista web normal.
  function routeLink(href, context) {
    const link = classifyLink(href, context.bookId, context.spinePaths);
    if (link.kind === 'internal') {
      context.navigate(link.spine, link.fragment);
    } else if (link.kind === 'external') {
      context.flush();
      context.post({ t: 'openExternal', url: link.url });
    }
    return link.kind;
  }

  function findAnchor(node) {
    let current = node;
    while (current && current.nodeType !== 9) {
      if (current.nodeType === 1 && current.localName === 'a' &&
          (current.hasAttribute('href') ||
           (current.hasAttributeNS && current.hasAttributeNS(XLINK_NS, 'href')))) {
        return current;
      }
      current = current.parentNode;
    }
    return null;
  }

  function anchorHref(anchor, base) {
    let raw = anchor.getAttribute('href');
    if (raw == null && anchor.getAttributeNS) raw = anchor.getAttributeNS(XLINK_NS, 'href');
    if (raw == null) return null;
    try {
      return new URL(raw, base).href;
    } catch (_) {
      return null;
    }
  }

  // ------------------------------------------------------------- pesquisa

  function collapse(text) {
    return text.replace(/\s+/g, ' ');
  }

  // Ocorrências de `query` em `text`, sem distinguir maiúsculas nem
  // acentos, com o contexto à volta. `occurrence` é a ordem da ocorrência
  // no documento: é por ela que se volta a encontrar o ponto ao abrir.
  function searchText(text, query, limit, context) {
    const needle = E.fold(String(query || '')).replace(/\s+/g, ' ').trim();
    const results = [];
    if (needle.length < 2 || !(limit > 0)) return results;
    const folded = E.foldWithMap(text);
    const hay = folded.text.replace(/\s/g, ' ');
    let from = 0;
    while (results.length < limit) {
      const at = hay.indexOf(needle, from);
      if (at < 0) break;
      const start = folded.starts[at];
      const end = folded.ends[at + needle.length - 1];
      const ctx = context > 0 ? context : 0;
      const beforeStart = Math.max(0, start - ctx);
      const afterEnd = Math.min(text.length, end + ctx);
      results.push({
        occurrence: results.length,
        start,
        end,
        before: (beforeStart > 0 ? '…' : '') + collapse(text.slice(beforeStart, start)).replace(/^\s+/, ''),
        match: collapse(text.slice(start, end)),
        after: collapse(text.slice(end, afterEnd)).replace(/\s+$/, '') + (afterEnd < text.length ? '…' : ''),
      });
      from = at + needle.length;
    }
    return results;
  }

  const BLOCKS = new Set(['p', 'div', 'li', 'ul', 'ol', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'blockquote', 'section', 'article', 'aside', 'header', 'footer', 'nav', 'figure',
    'figcaption', 'table', 'tr', 'td', 'th', 'dt', 'dd', 'dl', 'pre', 'body', 'main',
    'address', 'caption', 'summary', 'details', 'svg', 'text']);
  const SKIPPED = new Set(['script', 'style', 'title', 'head']);

  function blockOf(node) {
    let element = node.parentNode;
    while (element && element.nodeType === 1) {
      if (BLOCKS.has(element.localName)) return element;
      element = element.parentNode;
    }
    return null;
  }

  // O texto de um documento, com uma quebra entre blocos, e onde começa cada
  // nó de texto. A mesma função serve o documento lido para a pesquisa e o
  // documento aberto no iframe: as ocorrências contam-se igual nos dois.
  function collectText(doc) {
    const segments = [];
    let text = '';
    const start = doc && (doc.body || doc.documentElement);
    if (!start) return { text, segments };
    const walker = doc.createTreeWalker(start, 4);
    let lastBlock;
    let node;
    while ((node = walker.nextNode())) {
      const parent = node.parentNode;
      if (parent && SKIPPED.has(parent.localName)) continue;
      const data = node.data;
      if (!data) continue;
      const block = blockOf(node);
      if (segments.length && block !== lastBlock && !/\s$/.test(text)) text += '\n';
      lastBlock = block;
      segments.push({ node, start: text.length });
      text += data;
    }
    return { text, segments };
  }

  // Posição no texto juntado → nó e deslocamento. `atEnd`: um fim de
  // intervalo prefere o fim do nó anterior ao início do seguinte.
  function locateInSegments(segments, pos, atEnd) {
    if (!segments.length) return null;
    let low = 0;
    let high = segments.length - 1;
    let found = 0;
    while (low <= high) {
      const mid = (low + high) >> 1;
      const ok = atEnd ? segments[mid].start < pos : segments[mid].start <= pos;
      if (ok) {
        found = mid;
        low = mid + 1;
      } else {
        high = mid - 1;
      }
    }
    const segment = segments[found];
    const length = segment.node.data.length;
    return { node: segment.node, offset: Math.max(0, Math.min(length, pos - segment.start)) };
  }

  function rangeFor(doc, segments, start, end) {
    const from = locateInSegments(segments, start, false);
    const to = locateInSegments(segments, end, true);
    if (!from || !to) return null;
    const range = doc.createRange();
    range.setStart(from.node, from.offset);
    range.setEnd(to.node, to.offset);
    return range;
  }

  function parseChapter(win, source, contentType) {
    const Parser = win.DOMParser;
    if (typeof Parser !== 'function') return null;
    const parser = new Parser();
    const type = contentType === 'text/html' ? 'text/html'
      : contentType === 'image/svg+xml' ? 'image/svg+xml'
        : 'application/xhtml+xml';
    let doc = parser.parseFromString(source, type);
    if (type !== 'text/html' && doc.getElementsByTagName('parsererror').length) {
      doc = parser.parseFromString(source, 'text/html');
    }
    return doc;
  }

  // ----------------------------------------------------- posição, marcas

  function roundFraction(value) {
    return Math.round(E.clamp(value, 0, 1) * 10000) / 10000;
  }

  // Guarda a posição sem inundar o disco: a primeira mudança arma um
  // prazo; as seguintes só trocam o que vai ser gravado; `flush` grava já.
  class PositionSaver {
    constructor(bookId, post, timers, delay) {
      this.bookId = bookId;
      this.post = post;
      this.timers = timers;
      this.delay = delay;
      this.pending = null;
      this.timer = null;
      this.last = null;
    }

    note(spine, fraction) {
      this.pending = { spine, fraction: roundFraction(fraction) };
      if (this.timer == null) {
        this.timer = this.timers.setTimeout(() => {
          this.timer = null;
          this.flush();
        }, this.delay);
      }
    }

    flush() {
      if (this.timer != null) {
        this.timers.clearTimeout(this.timer);
        this.timer = null;
      }
      const next = this.pending;
      this.pending = null;
      if (!next) return false;
      if (this.last && this.last.spine === next.spine && this.last.fraction === next.fraction) {
        return false;
      }
      this.last = next;
      this.post({ t: 'savePosition', id: this.bookId, spine: next.spine, fraction: next.fraction });
      return true;
    }
  }

  function cleanLabel(label) {
    const text = String(label == null ? '' : label).replace(/[\u0000-\u001f\u007f-\u009f]/g, ' ')
      .replace(/\s+/g, ' ').trim();
    return Array.from(text).slice(0, MAX_LABEL_CHARS).join('');
  }

  // Marcadores do livro, na ordem de leitura. Pedir para adicionar ou
  // remover vai ao lado nativo; a lista volta de lá (com os ids).
  class BookmarkList {
    constructor(bookId, post) {
      this.bookId = bookId;
      this.post = post;
      this.items = [];
    }

    set(list) {
      this.items = (Array.isArray(list) ? list : [])
        .filter((mark) => mark && Number.isInteger(mark.id) && Number.isInteger(mark.spine))
        .map((mark) => ({
          id: mark.id,
          spine: mark.spine,
          fraction: E.clamp(mark.fraction, 0, 1),
          label: cleanLabel(mark.label) || 'Marcador',
        }))
        .sort((a, b) => a.spine - b.spine || a.fraction - b.fraction || a.id - b.id);
      return this.items;
    }

    // Já há um marcador nesta página?
    at(spine, page, pages) {
      return this.items.find((mark) => mark.spine === spine && pageForFraction(mark.fraction, pages) === page) || null;
    }

    add(spine, fraction, label) {
      if (!Number.isInteger(spine) || spine < 0) return false;
      this.post({
        t: 'addBookmark',
        id: this.bookId,
        spine,
        fraction: roundFraction(fraction),
        label: cleanLabel(label) || 'Marcador',
      });
      return true;
    }

    remove(markId) {
      if (!this.items.some((mark) => mark.id === markId)) return false;
      this.items = this.items.filter((mark) => mark.id !== markId);
      this.post({ t: 'removeBookmark', id: this.bookId, bookmark: markId });
      return true;
    }
  }

  // --------------------------------------------------- leitura em voz alta

  function languageTag(value) {
    const tag = String(value || '').trim().replace(/_/g, '-').toLowerCase();
    const base = tag.split('-')[0];
    const alias = LANGUAGE_ALIASES[base];
    return alias ? alias + tag.slice(base.length) : tag;
  }

  // Uma voz LOCAL (nunca uma voz online) que fale a língua do livro; entre
  // as de português, pt-BR primeiro. Sem língua conhecida, ou sem voz para
  // ela: pt-BR, depois a voz local padrão. `null` se não há voz local.
  function chooseVoice(voices, bookLanguage) {
    const local = (Array.isArray(voices) ? voices : []).filter((voice) => voice && voice.localService === true);
    if (!local.length) return null;
    const lang = languageTag(bookLanguage);
    const base = lang.split('-')[0];
    const rank = (voice) => {
      const tag = languageTag(voice.lang);
      const voiceBase = tag.split('-')[0];
      if (base && voiceBase === base) {
        if (base === 'pt') return tag === 'pt-br' ? 0 : 1;
        if (tag === lang) return 0;
        return 1;
      }
      if (tag === 'pt-br') return 10;
      if (voiceBase === 'pt') return 11;
      return voice.default ? 20 : 21;
    };
    let best = null;
    let bestRank = Infinity;
    for (const voice of local) {
      const value = rank(voice);
      if (value < bestRank) {
        best = voice;
        bestRank = value;
      }
    }
    return best;
  }

  // Frases do texto (por bloco; frases longas partidas perto de 280
  // caracteres, para a voz nunca engasgar numa página inteira sem ponto).
  function splitSentences(text, lang) {
    const spans = [];
    const isSpace = (ch) => /\s/.test(ch);
    const pushChunks = (start, end) => {
      while (end - start > MAX_SPOKEN_CHARS) {
        const limit = start + MAX_SPOKEN_CHARS;
        let cut = -1;
        for (const mark of [',', ';', ':', ' ']) {
          const at = text.lastIndexOf(mark, limit);
          if (at > start + 40) {
            cut = at + 1;
            break;
          }
        }
        if (cut < 0) cut = limit;
        spans.push({ start, end: cut });
        start = cut;
        while (start < end && isSpace(text[start])) start++;
      }
      if (end > start) spans.push({ start, end });
    };
    const add = (start, end) => {
      while (start < end && isSpace(text[start])) start++;
      while (end > start && isSpace(text[end - 1])) end--;
      if (end > start && /[\p{L}\p{N}]/u.test(text.slice(start, end))) pushChunks(start, end);
    };
    const Segmenter = root.Intl && root.Intl.Segmenter;
    let segmenter = null;
    if (typeof Segmenter === 'function') {
      try {
        segmenter = new Segmenter(lang || 'pt-BR', { granularity: 'sentence' });
      } catch (_) {
        segmenter = null;
      }
    }
    let blockStart = 0;
    const source = String(text || '');
    while (blockStart <= source.length) {
      let blockEnd = source.indexOf('\n', blockStart);
      if (blockEnd < 0) blockEnd = source.length;
      const block = source.slice(blockStart, blockEnd);
      if (segmenter) {
        for (const part of segmenter.segment(block)) {
          add(blockStart + part.index, blockStart + part.index + part.segment.length);
        }
      } else {
        const pattern = /[^.!?…]*[.!?…]+["'”’»)\]]*|[^.!?…]+$/g;
        let match;
        while ((match = pattern.exec(block))) {
          if (!match[0].length) {
            pattern.lastIndex++;
            continue;
          }
          add(blockStart + match.index, blockStart + match.index + match[0].length);
        }
      }
      blockStart = blockEnd + 1;
    }
    return spans;
  }

  // A fila de frases faladas. Um `token` por arranque: o `onend` de uma
  // frase cancelada nunca faz avançar a fila seguinte.
  class ReadAloud {
    constructor(options) {
      this.synth = options.synth;
      this.Utterance = options.Utterance;
      this.onSentence = options.onSentence || (() => {});
      this.onEnd = options.onEnd || (() => {});
      this.onStop = options.onStop || (() => {});
      this.onError = options.onError || (() => {});
      this.voice = null;
      this.rate = 1;
      this.queue = [];
      this.index = -1;
      this.active = false;
      this.token = 0;
    }

    start(sentences, from) {
      this.stop(false);
      this.queue = Array.isArray(sentences) ? sentences : [];
      this.index = Math.max(0, Number(from) || 0) - 1;
      this.active = true;
      this.next();
    }

    next() {
      if (!this.active) return;
      this.index++;
      if (this.index >= this.queue.length) {
        this.active = false;
        this.onEnd();
        return;
      }
      const sentence = this.queue[this.index];
      const utterance = new this.Utterance(sentence.text);
      if (this.voice) {
        utterance.voice = this.voice;
        utterance.lang = this.voice.lang;
      }
      utterance.rate = this.rate;
      const token = this.token;
      utterance.onend = () => {
        if (token === this.token && this.active) this.next();
      };
      utterance.onerror = (event) => {
        if (token !== this.token) return;
        const kind = event && event.error;
        if (kind === 'interrupted' || kind === 'canceled') return;
        this.stop(true);
        this.onError(kind || 'erro');
      };
      this.onSentence(sentence, this.index);
      this.synth.speak(utterance);
    }

    stop(notify) {
      const wasActive = this.active;
      this.active = false;
      this.token++;
      this.queue = [];
      this.index = -1;
      if (this.synth && typeof this.synth.cancel === 'function') this.synth.cancel();
      if (wasActive && notify !== false) this.onStop();
    }
  }

  // ------------------------------------------------------ estilo do livro

  function createBookFrame(doc) {
    const frame = doc.createElement('iframe');
    // SÓ allow-same-origin: sem allow-scripts nenhum script do livro (nem de
    // SVG) corre; sem allow-forms, allow-popups nem allow-top-navigation.
    frame.setAttribute('sandbox', 'allow-same-origin');
    frame.setAttribute('title', 'Conteúdo do livro');
    frame.setAttribute('referrerpolicy', 'no-referrer');
    frame.className = 'book-frame';
    return frame;
  }

  function verticalPadding(height) {
    return Math.round(E.clamp(height * 0.04, 12, 40));
  }

  function resolveTheme(setting, prefersDark) {
    if (setting === 'system') return prefersDark ? 'dark' : 'light';
    return THEMES[setting] ? setting : 'light';
  }

  // O estilo injetado no documento do livro: paginação (ou rolagem),
  // tipografia e tema. As regras do utilizador levam !important para
  // ganharem ao CSS do livro, como no Calibre.
  function bookCss(settings, geometry, themeName) {
    const theme = THEMES[themeName] || THEMES.light;
    const lines = [];
    const vpad = verticalPadding(geometry.pageHeight);
    lines.push('html { font-size: ' + settings.fontSize + '% !important; scroll-behavior: auto !important; }');
    if (settings.mode === 'paged') {
      const contentHeight = Math.max(40, geometry.pageHeight - 2 * vpad);
      lines.push('html { width: ' + geometry.pageWidth + 'px !important; height: ' + geometry.pageHeight +
        'px !important; max-width: none !important; max-height: none !important; margin: 0 !important; padding: ' +
        vpad + 'px ' + geometry.gap / 2 + 'px !important; box-sizing: border-box !important; overflow: hidden !important;' +
        ' column-width: ' + geometry.columnWidth + 'px !important; column-gap: ' + geometry.gap +
        'px !important; column-fill: auto !important; column-count: auto !important; }');
      lines.push('body { margin: 0 !important; padding: 0 !important; width: auto !important; max-width: none !important;' +
        ' min-height: 0 !important; height: auto !important; overflow: visible !important; }');
      lines.push('img, svg, video, canvas, object, embed { max-width: 100% !important; max-height: ' + contentHeight +
        'px !important; object-fit: contain; box-sizing: border-box; break-inside: avoid; }');
      lines.push('img { height: auto; }');
      lines.push('p, li, blockquote, h1, h2, h3, h4, h5, h6 { orphans: 2; widows: 2; }');
    } else {
      lines.push('html { width: auto !important; height: auto !important; margin: 0 !important; padding: 0 !important;' +
        ' overflow-x: hidden !important; overflow-y: auto !important; column-width: auto !important; column-count: auto !important; }');
      lines.push('body { box-sizing: border-box !important; max-width: ' + SCROLL_MAX_WIDTH + 'px !important; margin: 0 auto !important; padding: ' +
        vpad + 'px ' + Math.max(12, geometry.gap / 2) + 'px !important; }');
      lines.push('img, svg, video, canvas { max-width: 100% !important; object-fit: contain; }');
      lines.push('img { height: auto; }');
    }
    const family = FONT_FAMILIES[settings.fontFamily];
    if (family) {
      lines.push('body, body * { font-family: ' + family + ' !important; }');
      lines.push('code, pre, kbd, samp, tt { font-family: Consolas, \'Courier New\', monospace !important; }');
    }
    if (settings.lineHeight !== 'original') {
      lines.push('body, body p, body li, body div, body span, body blockquote, body dd { line-height: ' +
        settings.lineHeight + ' !important; }');
    }
    if (settings.align === 'justify') {
      lines.push('body p, body li, body div, body blockquote, body dd { text-align: justify !important; hyphens: auto; }');
    } else if (settings.align === 'left') {
      lines.push('body p, body li, body div, body blockquote, body dd { text-align: start !important; }');
    }
    if (themeName !== 'light') {
      lines.push('html, body { background-color: ' + theme.bg + ' !important; color: ' + theme.fg + ' !important; }');
      lines.push('body *:not(img):not(svg):not(video) { color: inherit !important; background-color: transparent !important; }');
      lines.push('a, a * { color: ' + theme.link + ' !important; }');
    } else {
      lines.push('html { background-color: ' + theme.bg + '; color: ' + theme.fg + '; }');
    }
    lines.push('::highlight(neuralia-search) { background-color: #ffd54f; color: #111111; }');
    lines.push('::highlight(neuralia-speech) { background-color: rgba(255, 193, 7, 0.45); }');
    return lines.join('\n');
  }

  function normalizeSettings(raw) {
    const input = raw && typeof raw === 'object' ? raw : {};
    const pick = (value, allowed, fallback) => (allowed.includes(value) ? value : fallback);
    const size = Number(input.fontSize);
    return {
      fontSize: FONT_STEPS.includes(size) ? size : DEFAULT_SETTINGS.fontSize,
      fontFamily: pick(input.fontFamily, Object.keys(FONT_FAMILIES), DEFAULT_SETTINGS.fontFamily),
      lineHeight: pick(input.lineHeight, LINE_HEIGHTS, DEFAULT_SETTINGS.lineHeight),
      margin: pick(input.margin, Object.keys(MARGINS), DEFAULT_SETTINGS.margin),
      align: pick(input.align, ['original', 'justify', 'left'], DEFAULT_SETTINGS.align),
      theme: pick(input.theme, ['system', 'light', 'sepia', 'dark'], DEFAULT_SETTINGS.theme),
      mode: pick(input.mode, ['paged', 'scroll'], DEFAULT_SETTINGS.mode),
      columns: pick(input.columns, ['auto', '1', '2'], DEFAULT_SETTINGS.columns),
      rate: pick(input.rate, RATES, DEFAULT_SETTINGS.rate),
    };
  }

  function stepFont(current, direction) {
    const index = FONT_STEPS.indexOf(current);
    const at = index < 0 ? FONT_STEPS.indexOf(100) : index;
    return FONT_STEPS[Math.min(FONT_STEPS.length - 1, Math.max(0, at + direction))];
  }

  // Tecla → ação do leitor (fora de campos de texto).
  function keyAction(key, shift) {
    switch (key) {
      case 'ArrowRight':
      case 'PageDown':
        return 'next';
      case 'ArrowLeft':
      case 'PageUp':
        return 'prev';
      case ' ':
      case 'Spacebar':
        return shift ? 'prev' : 'next';
      case 'Home':
        return 'start';
      case 'End':
        return 'end';
      case 't':
      case 'T':
        return 'toc';
      case 'b':
      case 'B':
        return 'bookmarks';
      case 'm':
      case 'M':
        return 'addBookmark';
      case 'l':
      case 'L':
        return 'speak';
      case '+':
      case '=':
        return 'bigger';
      case '-':
      case '_':
        return 'smaller';
      case '?':
        return 'help';
      default:
        return null;
    }
  }

  // --------------------------------------------------------------- página

  class Reader {
    constructor(doc, win) {
      this.doc = doc;
      this.win = win;
      this.settings = this.loadSettings();
      this.book = null;
      this.id = null;
      this.spinePaths = [];
      this.sizes = [];
      this.flatToc = [];
      this.tocItems = [];
      this.spine = -1;
      this.page = 0;
      this.pages = 1;
      this.geometry = pageGeometry(1, 1, 1, 0);
      this.frameReady = false;
      this.pendingTarget = null;
      this.openPanelName = null;
      this.searchToken = 0;
      this.textCache = new Map();
      this.speech = null;
      this.speaking = false;
      this.speechContinues = false;
      this.speechText = null;
      this.wheelLock = 0;
      this.wheelAccum = 0;
      this.scrollQueued = false;
      this.toastTimer = null;
      this.resizeTimer = null;
      const byId = (id) => doc.getElementById(id);
      this.ui = {
        back: byId('back'),
        bookTitle: byId('book-title'),
        chapterTitle: byId('chapter-title'),
        stage: byId('stage'),
        frameHost: byId('frame-host'),
        message: byId('reader-message'),
        messageText: byId('reader-message-text'),
        toc: byId('toc'),
        searchForm: byId('search-form'),
        searchInput: byId('search-input'),
        searchStatus: byId('search-status'),
        searchResults: byId('search-results'),
        bookmarkAdd: byId('bookmark-add'),
        bookmarkList: byId('bookmark-list'),
        bookmarkEmpty: byId('bookmark-empty'),
        fontSmaller: byId('font-smaller'),
        fontLarger: byId('font-larger'),
        fontSizeLabel: byId('font-size-label'),
        fontFamily: byId('font-family'),
        lineHeight: byId('line-height'),
        margins: byId('margins'),
        align: byId('align'),
        mode: byId('mode'),
        columns: byId('columns'),
        rate: byId('rate'),
        voiceName: byId('voice-name'),
        speak: byId('btn-speak'),
        help: byId('help'),
        helpClose: byId('help-close'),
        progressTrack: byId('progress-track'),
        progressFill: byId('progress-fill'),
        progressChapter: byId('progress-chapter'),
        progressPage: byId('progress-page'),
        progressPercent: byId('progress-percent'),
        toast: byId('toast'),
        panels: {
          toc: byId('panel-toc'),
          search: byId('panel-search'),
          bookmarks: byId('panel-bookmarks'),
          settings: byId('panel-settings'),
        },
        buttons: {
          toc: byId('btn-toc'),
          search: byId('btn-search'),
          bookmarks: byId('btn-bookmarks'),
          settings: byId('btn-settings'),
        },
      };
      this.frame = createBookFrame(doc);
      this.ui.frameHost.appendChild(this.frame);
      this.frame.addEventListener('load', () => this.onFrameLoad());
      this.darkQuery = typeof win.matchMedia === 'function' ? win.matchMedia('(prefers-color-scheme: dark)') : null;
    }

    loadSettings() {
      try {
        const raw = this.win.localStorage.getItem(SETTINGS_KEY);
        return normalizeSettings(raw ? JSON.parse(raw) : null);
      } catch (_) {
        return normalizeSettings(null);
      }
    }

    saveSettings() {
      try {
        this.win.localStorage.setItem(SETTINGS_KEY, JSON.stringify(this.settings));
      } catch (_) { /* só conveniência */ }
    }

    frameDoc() {
      try {
        return this.frame.contentDocument;
      } catch (_) {
        return null;
      }
    }

    frameWin() {
      return this.frame.contentWindow;
    }

    scroller() {
      const doc = this.frameDoc();
      return doc ? (doc.scrollingElement || doc.documentElement) : null;
    }

    themeName() {
      return resolveTheme(this.settings.theme, Boolean(this.darkQuery && this.darkQuery.matches));
    }

    async start() {
      this.bindChrome();
      this.syncSettingsUi();
      const id = new URLSearchParams(this.win.location.search).get('book');
      if (!E.isBookId(id)) {
        this.fail('Este endereço não aponta para um livro da biblioteca.');
        return;
      }
      this.id = id;
      let book;
      try {
        book = await E.getJson('/api/book/' + id);
      } catch (error) {
        this.fail(error.message);
        return;
      }
      if (!book || !Array.isArray(book.spine) || !book.spine.length) {
        this.fail('Este livro não tem conteúdo para mostrar.');
        return;
      }
      this.book = book;
      this.spinePaths = book.spine.map((item) => item.path);
      this.sizes = book.spine.map((item) => Number(item.size) || 0);
      this.flatToc = flattenToc(book.toc, book.spine.length);
      this.saver = new PositionSaver(id, E.post, this.win, SAVE_DELAY_MS);
      this.marks = new BookmarkList(id, E.post);
      this.marks.set(book.bookmarks);
      this.ui.bookTitle.textContent = book.title || 'Sem título';
      this.doc.title = (book.title || 'Livro') + ' — NeuralIA';
      this.renderToc();
      this.renderBookmarks();
      E.onNotice((notice) => this.onNotice(notice));
      E.post({ t: 'opened', id });
      const start = restorePosition(book.position, book.spine.length);
      this.navigate(start.spine, { fraction: start.fraction });
    }

    bindChrome() {
      const ui = this.ui;
      ui.back.addEventListener('click', () => this.backToLibrary());
      for (const name of Object.keys(ui.buttons)) {
        ui.buttons[name].addEventListener('click', () => this.togglePanel(name));
      }
      for (const panel of Object.values(ui.panels)) {
        const close = panel.querySelector('.panel-close');
        if (close) close.addEventListener('click', () => this.closePanels());
      }
      ui.searchForm.addEventListener('submit', (event) => {
        event.preventDefault();
        this.runSearch(ui.searchInput.value);
      });
      ui.bookmarkAdd.addEventListener('click', () => this.addBookmarkHere());
      ui.fontSmaller.addEventListener('click', () => this.changeFont(-1));
      ui.fontLarger.addEventListener('click', () => this.changeFont(1));
      const bindSelect = (element, key) => element.addEventListener('change', () => this.setSetting(key, element.value));
      bindSelect(ui.fontFamily, 'fontFamily');
      bindSelect(ui.lineHeight, 'lineHeight');
      bindSelect(ui.margins, 'margin');
      bindSelect(ui.align, 'align');
      bindSelect(ui.mode, 'mode');
      bindSelect(ui.columns, 'columns');
      bindSelect(ui.rate, 'rate');
      for (const button of this.doc.querySelectorAll('[data-theme-choice]')) {
        button.addEventListener('click', () => this.setSetting('theme', button.getAttribute('data-theme-choice')));
      }
      ui.speak.addEventListener('click', () => this.toggleSpeech());
      this.doc.getElementById('btn-help').addEventListener('click', () => this.toggleHelp(true));
      ui.helpClose.addEventListener('click', () => this.toggleHelp(false));
      ui.progressTrack.addEventListener('click', (event) => {
        const rect = ui.progressTrack.getBoundingClientRect();
        if (!this.book || !(rect.width > 0)) return;
        const where = locateProgress(this.sizes, (event.clientX - rect.left) / rect.width);
        this.goTo(where.spine, { fraction: where.fraction });
      });
      this.doc.addEventListener('keydown', (event) => this.onKey(event));
      ui.stage.addEventListener('wheel', (event) => this.onWheel(event), { passive: false });
      this.win.addEventListener('resize', () => {
        if (this.resizeTimer != null) this.win.clearTimeout(this.resizeTimer);
        this.resizeTimer = this.win.setTimeout(() => {
          this.resizeTimer = null;
          this.relayout();
        }, 120);
      });
      this.win.addEventListener('pagehide', () => {
        if (this.saver) this.saver.flush();
        this.stopSpeech();
      });
      if (this.darkQuery && typeof this.darkQuery.addEventListener === 'function') {
        this.darkQuery.addEventListener('change', () => {
          if (this.settings.theme === 'system') {
            this.applyChromeTheme();
            this.relayout();
          }
        });
      }
      this.applyChromeTheme();
    }

    applyChromeTheme() {
      this.doc.documentElement.setAttribute('data-theme', this.themeName());
    }

    syncSettingsUi() {
      const ui = this.ui;
      const s = this.settings;
      ui.fontSizeLabel.textContent = s.fontSize + '%';
      ui.fontFamily.value = s.fontFamily;
      ui.lineHeight.value = s.lineHeight;
      ui.margins.value = s.margin;
      ui.align.value = s.align;
      ui.mode.value = s.mode;
      ui.columns.value = s.columns;
      ui.rate.value = s.rate;
      for (const button of this.doc.querySelectorAll('[data-theme-choice]')) {
        button.classList.toggle('active', button.getAttribute('data-theme-choice') === s.theme);
      }
    }

    setSetting(key, value) {
      const next = normalizeSettings(Object.assign({}, this.settings, { [key]: value }));
      this.settings = next;
      this.saveSettings();
      this.syncSettingsUi();
      this.applyChromeTheme();
      if (key === 'rate' && this.speech) this.speech.rate = Number(next.rate) || 1;
      if (key !== 'rate') this.relayout();
    }

    changeFont(direction) {
      this.setSetting('fontSize', stepFont(this.settings.fontSize, direction));
    }

    fail(message) {
      this.frame.hidden = true;
      this.ui.messageText.textContent = message;
      this.ui.message.hidden = false;
    }

    toast(text, isError) {
      const toast = this.ui.toast;
      toast.textContent = text;
      toast.classList.toggle('error', Boolean(isError));
      toast.hidden = false;
      if (this.toastTimer != null) this.win.clearTimeout(this.toastTimer);
      this.toastTimer = this.win.setTimeout(() => {
        this.toastTimer = null;
        toast.hidden = true;
      }, 3500);
    }

    // ---------------------------------------------------------- navegar

    // Navegação pedida pela pessoa: a leitura em voz alta para.
    goTo(spine, target) {
      this.stopSpeech();
      this.navigate(spine, target);
    }

    navigate(spine, target) {
      if (!this.book) return;
      const index = Math.trunc(Number(spine));
      if (!(index >= 0 && index < this.book.spine.length)) return;
      if (index === this.spine && this.frameReady) {
        this.applyTarget(target || { page: 0 });
        return;
      }
      this.spine = index;
      this.frameReady = false;
      this.pendingTarget = target || { page: 0 };
      this.frame.src = this.book.spine[index].href;
    }

    onFrameLoad() {
      const doc = this.frameDoc();
      if (!doc || !this.book) return;
      let here = { kind: 'ignore' };
      try {
        here = classifyLink(String(this.frameWin().location.href), this.id, this.spinePaths);
      } catch (_) { /* sem location: fica o spine pedido */ }
      if (here.kind === 'internal' && here.spine !== this.spine) {
        // O iframe navegou sozinho (um link seguido sem passar por aqui).
        this.spine = here.spine;
        if (!this.pendingTarget) this.pendingTarget = here.fragment ? { fragment: here.fragment } : { page: 0 };
      }
      this.frameReady = true;
      this.prepareDocument(doc);
      this.layout();
      const target = this.pendingTarget || { page: 0 };
      this.pendingTarget = null;
      this.applyTarget(target);
      if (doc.fonts && doc.fonts.ready && typeof doc.fonts.ready.then === 'function') {
        doc.fonts.ready.then(() => {
          if (this.frameDoc() === doc) this.relayout();
        });
      }
      if (this.speaking && this.speechContinues) {
        this.speechContinues = false;
        this.speakFrom(0);
      }
    }

    prepareDocument(doc) {
      const rootElement = doc.documentElement;
      if (!rootElement) return;
      let style = doc.getElementById(STYLE_ID);
      if (!style) {
        const ns = rootElement.namespaceURI === SVG_NS ? SVG_NS : XHTML_NS;
        style = doc.createElementNS(ns, 'style');
        style.setAttribute('id', STYLE_ID);
        (doc.head || rootElement).appendChild(style);
      }
      doc.addEventListener('click', (event) => this.onBookClick(event), true);
      doc.addEventListener('keydown', (event) => this.onKey(event));
      doc.addEventListener('wheel', (event) => this.onWheel(event), { passive: false });
      doc.addEventListener('scroll', () => this.onScroll(), { passive: true });
      // Formulários do livro nunca enviam nada (o sandbox e a CSP já o
      // impedem; isto só evita o clique morto).
      doc.addEventListener('submit', (event) => event.preventDefault(), true);
    }

    layout() {
      const doc = this.frameDoc();
      if (!doc) return;
      const width = Math.max(1, this.ui.stage.clientWidth);
      const height = Math.max(1, this.ui.stage.clientHeight);
      const columns = this.settings.mode === 'paged' ? columnsFor(width, this.settings.columns) : 1;
      this.geometry = pageGeometry(width, height, columns, MARGINS[this.settings.margin] || MARGINS.normal);
      this.frame.style.width = (this.settings.mode === 'paged' ? this.geometry.pageWidth : width) + 'px';
      this.frame.style.height = this.geometry.pageHeight + 'px';
      const style = doc.getElementById(STYLE_ID);
      if (style) style.textContent = bookCss(this.settings, this.geometry, this.themeName());
      this.fitLoneImage(doc);
      this.measure();
    }

    // Uma página que é só uma imagem (a capa, quase sempre) ocupa a página.
    fitLoneImage(doc) {
      const body = doc.body;
      if (!body || this.settings.mode !== 'paged') return;
      let only = body;
      for (let depth = 0; depth < 3; depth++) {
        const children = Array.from(only.children || []);
        if (children.length !== 1) break;
        only = children[0];
      }
      if (only === body || (only.localName !== 'img' && only.localName !== 'svg')) return;
      if ((body.textContent || '').trim()) return;
      const height = Math.max(40, this.geometry.pageHeight - 2 * verticalPadding(this.geometry.pageHeight));
      only.style.setProperty('width', '100%', 'important');
      only.style.setProperty('height', height + 'px', 'important');
      only.style.setProperty('object-fit', 'contain');
    }

    measure() {
      const doc = this.frameDoc();
      if (!doc || this.settings.mode !== 'paged') {
        this.pages = 1;
        return;
      }
      this.pages = pageCount(doc.documentElement.scrollWidth, this.geometry.pageWidth);
    }

    relayout() {
      if (!this.frameReady) return;
      const fraction = this.currentFraction();
      this.layout();
      this.applyTarget({ fraction });
    }

    currentFraction() {
      if (this.settings.mode === 'paged') return fractionForPage(this.page, this.pages);
      const scroller = this.scroller();
      return scroller ? scrollFraction(scroller.scrollTop, scroller.scrollHeight) : 0;
    }

    pointOf(rect) {
      const win = this.frameWin();
      return { x: rect.left + (win.scrollX || 0), y: rect.top + (win.scrollY || 0) };
    }

    pointOfFragment(fragment) {
      const doc = this.frameDoc();
      if (!doc || !fragment) return null;
      let element = doc.getElementById(fragment);
      if (!element && typeof doc.getElementsByName === 'function') element = doc.getElementsByName(fragment)[0] || null;
      if (!element) return null;
      return this.pointOf(element.getBoundingClientRect());
    }

    pointOfRange(range) {
      const rects = range.getClientRects();
      return this.pointOf(rects.length ? rects[0] : range.getBoundingClientRect());
    }

    // Onde começa uma âncora deste capítulo, na unidade de `here` do
    // sumário (página no modo de páginas, píxel no modo de rolagem).
    anchorAt(fragment) {
      const point = this.pointOfFragment(fragment);
      if (!point) return null;
      if (this.settings.mode === 'paged') return Math.floor(point.x / this.geometry.pageWidth);
      return point.y;
    }

    applyTarget(target) {
      let range = null;
      if (target.search) {
        range = this.findOccurrence(target.search.query, target.search.occurrence);
        if (range) this.setHighlight('neuralia-search', range);
      }
      let where = null;
      if (range) where = this.pointOfRange(range);
      else if (target.fragment) where = this.pointOfFragment(target.fragment);
      if (this.settings.mode === 'paged') {
        let page;
        if (where) page = Math.floor(where.x / this.geometry.pageWidth);
        else if (target.page === 'last') page = this.pages - 1;
        else if (typeof target.fraction === 'number') page = pageForFraction(target.fraction, this.pages);
        else page = typeof target.page === 'number' ? target.page : 0;
        this.showPage(page);
        return;
      }
      const scroller = this.scroller();
      if (!scroller) return;
      let top;
      if (where) top = where.y - 24;
      else if (target.page === 'last') top = scroller.scrollHeight;
      else if (typeof target.fraction === 'number') top = E.clamp(target.fraction, 0, 1) * scroller.scrollHeight;
      else top = 0;
      this.frameWin().scrollTo(0, Math.max(0, top));
      this.afterMove();
    }

    showPage(page) {
      this.page = Math.max(0, Math.min(this.pages - 1, Math.trunc(Number(page)) || 0));
      const win = this.frameWin();
      if (win) win.scrollTo(this.page * this.geometry.pageWidth, 0);
      this.afterMove();
    }

    afterMove() {
      if (!this.book) return;
      const fraction = this.currentFraction();
      const progress = bookProgress(this.sizes, this.spine, fraction);
      this.ui.progressFill.style.width = (progress * 100).toFixed(2) + '%';
      this.ui.progressPercent.textContent = E.percentLabel(progress);
      const here = this.settings.mode === 'paged' ? this.page : (this.scroller() ? this.scroller().scrollTop + 1 : 0);
      const current = currentTocIndex(this.flatToc, this.spine, here, (fragment) => this.anchorAt(fragment));
      const label = current >= 0 ? this.flatToc[current].label : '';
      this.ui.chapterTitle.textContent = label;
      this.ui.progressChapter.textContent = label;
      this.ui.progressPage.textContent = this.settings.mode === 'paged'
        ? 'Página ' + (this.page + 1) + ' de ' + this.pages + ' do capítulo'
        : '';
      this.markCurrentToc(current);
      if (this.saver) this.saver.note(this.spine, fraction);
      const marked = this.marks && this.settings.mode === 'paged' && this.marks.at(this.spine, this.page, this.pages);
      this.ui.bookmarkAdd.textContent = marked ? 'Esta página já tem marcador' : 'Adicionar marcador aqui';
    }

    move(direction) {
      if (!this.frameReady || !this.book) return;
      this.stopSpeech();
      if (this.settings.mode === 'paged') {
        const step = stepPaged(this.page, this.pages, this.spine, this.book.spine.length, direction);
        if (step.kind === 'page') this.showPage(step.page);
        else if (step.kind === 'chapter') this.navigate(step.spine, step.target);
        return;
      }
      const scroller = this.scroller();
      if (!scroller) return;
      const atEnd = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
      const atStart = scroller.scrollTop <= 1;
      if (direction > 0 && atEnd) {
        if (this.spine < this.book.spine.length - 1) this.navigate(this.spine + 1, { page: 0 });
      } else if (direction < 0 && atStart) {
        if (this.spine > 0) this.navigate(this.spine - 1, { page: 'last' });
      } else {
        this.frameWin().scrollBy(0, direction * Math.round(scroller.clientHeight * 0.9));
      }
    }

    next() {
      this.move(1);
    }

    prev() {
      this.move(-1);
    }

    onScroll() {
      if (this.settings.mode !== 'paged' && !this.scrollQueued) {
        this.scrollQueued = true;
        this.win.requestAnimationFrame(() => {
          this.scrollQueued = false;
          this.afterMove();
        });
      }
    }

    onWheel(event) {
      if (event.ctrlKey || !this.frameReady) return;
      if (this.settings.mode !== 'paged') {
        const scroller = this.scroller();
        if (!scroller) return;
        const atEnd = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
        const atStart = scroller.scrollTop <= 1;
        if ((event.deltaY > 0 && atEnd) || (event.deltaY < 0 && atStart)) {
          event.preventDefault();
          const now = Date.now();
          if (now < this.wheelLock) return;
          this.wheelLock = now + 400;
          this.move(event.deltaY > 0 ? 1 : -1);
        }
        return;
      }
      event.preventDefault();
      const now = Date.now();
      if (now < this.wheelLock) return;
      const delta = Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
      this.wheelAccum += event.deltaMode ? delta * 40 : delta;
      if (Math.abs(this.wheelAccum) < 40) return;
      const direction = this.wheelAccum > 0 ? 1 : -1;
      this.wheelAccum = 0;
      this.wheelLock = now + 250;
      this.move(direction);
    }

    onBookClick(event) {
      const anchor = findAnchor(event.target);
      if (anchor) {
        event.preventDefault();
        event.stopPropagation();
        const doc = this.frameDoc();
        this.followLink(anchorHref(anchor, doc ? doc.baseURI : undefined));
        return;
      }
      if (this.settings.mode !== 'paged' || event.button !== 0) return;
      const selection = this.frameWin().getSelection();
      if (selection && !selection.isCollapsed) return;
      const zone = clickZone(event.clientX, this.geometry.pageWidth);
      if (zone < 0) this.prev();
      else if (zone > 0) this.next();
    }

    followLink(href) {
      return routeLink(href, {
        bookId: this.id,
        spinePaths: this.spinePaths,
        navigate: (spine, fragment) => this.goTo(spine, fragment ? { fragment } : { page: 0 }),
        flush: () => {
          if (this.saver) this.saver.flush();
          this.stopSpeech();
        },
        post: E.post,
      });
    }

    onKey(event) {
      const key = event.key;
      const target = event.target;
      const typing = target && target.ownerDocument === this.doc &&
        (target.localName === 'input' || target.localName === 'select' || target.localName === 'textarea');
      if (!this.ui.help.hidden) {
        if (key === 'Escape' || key === '?') {
          event.preventDefault();
          this.toggleHelp(false);
        }
        return;
      }
      if (event.ctrlKey && !event.altKey && !event.metaKey) {
        const lower = String(key).toLowerCase();
        if (lower === 'o') {
          event.preventDefault();
          E.post({ t: 'addBooks' });
        } else if (lower === 'f') {
          event.preventDefault();
          this.openPanel('search');
          this.ui.searchInput.focus();
          this.ui.searchInput.select();
        }
        return;
      }
      if (event.altKey || event.metaKey) return;
      if (key === 'Escape') {
        event.preventDefault();
        if (this.openPanelName) this.closePanels();
        else this.backToLibrary();
        return;
      }
      if (typing) return;
      const action = keyAction(key, event.shiftKey);
      if (!action) return;
      event.preventDefault();
      switch (action) {
        case 'next':
          this.next();
          break;
        case 'prev':
          this.prev();
          break;
        case 'start':
          this.goTo(this.spine, { page: 0 });
          break;
        case 'end':
          this.goTo(this.spine, { page: 'last' });
          break;
        case 'toc':
          this.togglePanel('toc');
          break;
        case 'bookmarks':
          this.togglePanel('bookmarks');
          break;
        case 'addBookmark':
          this.addBookmarkHere();
          break;
        case 'speak':
          this.toggleSpeech();
          break;
        case 'bigger':
          this.changeFont(1);
          break;
        case 'smaller':
          this.changeFont(-1);
          break;
        case 'help':
          this.toggleHelp(true);
          break;
        default:
          break;
      }
    }

    backToLibrary() {
      if (this.saver) this.saver.flush();
      this.stopSpeech();
      this.win.location.href = '/library.html';
    }

    toggleHelp(show) {
      this.ui.help.hidden = !show;
      if (show) this.ui.helpClose.focus();
    }

    openPanel(name) {
      for (const [key, panel] of Object.entries(this.ui.panels)) {
        panel.hidden = key !== name;
        this.ui.buttons[key].classList.toggle('active', key === name);
      }
      this.openPanelName = name;
      if (name === 'toc') this.scrollTocIntoView();
    }

    togglePanel(name) {
      if (this.openPanelName === name) this.closePanels();
      else this.openPanel(name);
    }

    closePanels() {
      for (const [key, panel] of Object.entries(this.ui.panels)) {
        panel.hidden = true;
        this.ui.buttons[key].classList.remove('active');
      }
      this.openPanelName = null;
    }

    // ---------------------------------------------------------- sumário

    renderToc() {
      const list = this.ui.toc;
      list.textContent = '';
      this.tocItems = [];
      for (const entry of this.flatToc) {
        const item = E.el(this.doc, 'li', 'toc-item');
        item.style.paddingLeft = (8 + entry.depth * 14) + 'px';
        const button = E.el(this.doc, 'button', 'toc-link', entry.label);
        button.type = 'button';
        if (entry.spine == null) {
          button.disabled = true;
        } else {
          button.addEventListener('click', () => this.goTo(entry.spine, entry.fragment ? { fragment: entry.fragment } : { page: 0 }));
        }
        item.appendChild(button);
        list.appendChild(item);
        this.tocItems.push(item);
      }
    }

    markCurrentToc(index) {
      this.tocItems.forEach((item, at) => {
        const current = at === index;
        item.classList.toggle('current', current);
        if (current) item.setAttribute('aria-current', 'true');
        else item.removeAttribute('aria-current');
      });
      this.currentTocIndex = index;
    }

    scrollTocIntoView() {
      const item = this.tocItems[this.currentTocIndex];
      if (item && typeof item.scrollIntoView === 'function') item.scrollIntoView({ block: 'center' });
    }

    // --------------------------------------------------------- pesquisa

    async chapterText(spine) {
      if (this.textCache.has(spine)) return this.textCache.get(spine);
      let text = '';
      try {
        const response = await this.win.fetch(this.book.spine[spine].href, { cache: 'no-store' });
        if (response.ok) {
          const source = await response.text();
          const type = (response.headers.get('content-type') || '').split(';')[0].trim();
          const parsed = parseChapter(this.win, source, type);
          text = parsed ? collectText(parsed).text : '';
        }
      } catch (_) {
        text = '';
      }
      this.textCache.set(spine, text);
      return text;
    }

    chapterLabel(spine) {
      let label = '';
      for (const entry of this.flatToc) {
        if (entry.spine === spine && !label) label = entry.label;
      }
      return label || 'Seção ' + (spine + 1);
    }

    async runSearch(query) {
      const token = ++this.searchToken;
      const ui = this.ui;
      ui.searchResults.textContent = '';
      this.clearHighlight('neuralia-search');
      if (!this.book) return;
      if (E.fold(query || '').trim().length < 2) {
        ui.searchStatus.textContent = 'Digite pelo menos 2 letras.';
        return;
      }
      const count = this.book.spine.length;
      let total = 0;
      for (let spine = 0; spine < count && total < MAX_RESULTS; spine++) {
        ui.searchStatus.textContent = 'Pesquisando… ' + (spine + 1) + '/' + count;
        const text = await this.chapterText(spine);
        if (token !== this.searchToken) return;
        const found = searchText(text, query, MAX_RESULTS - total, CONTEXT_CHARS);
        const chapter = this.chapterLabel(spine);
        for (const hit of found) {
          const item = E.el(this.doc, 'li', 'search-result');
          const button = E.el(this.doc, 'button', 'search-hit');
          button.type = 'button';
          button.appendChild(E.el(this.doc, 'span', 'search-chapter', chapter));
          const line = E.el(this.doc, 'span', 'search-context');
          line.appendChild(this.doc.createTextNode(hit.before));
          line.appendChild(E.el(this.doc, 'mark', null, hit.match));
          line.appendChild(this.doc.createTextNode(hit.after));
          button.appendChild(line);
          button.addEventListener('click', () => this.goTo(spine, { search: { query, occurrence: hit.occurrence } }));
          item.appendChild(button);
          ui.searchResults.appendChild(item);
        }
        total += found.length;
      }
      if (token !== this.searchToken) return;
      ui.searchStatus.textContent = total === 0
        ? 'Nada encontrado.'
        : total >= MAX_RESULTS
          ? 'Mostrando os primeiros ' + MAX_RESULTS + ' resultados.'
          : total === 1 ? '1 resultado.' : total + ' resultados.';
    }

    findOccurrence(query, occurrence) {
      const doc = this.frameDoc();
      if (!doc) return null;
      const collected = collectText(doc);
      const hits = searchText(collected.text, query, (Number(occurrence) || 0) + 1, 0);
      const hit = hits[Number(occurrence) || 0];
      return hit ? rangeFor(doc, collected.segments, hit.start, hit.end) : null;
    }

    setHighlight(name, range) {
      const win = this.frameWin();
      if (win && win.CSS && win.CSS.highlights && typeof win.Highlight === 'function') {
        win.CSS.highlights.set(name, new win.Highlight(range));
        return;
      }
      const selection = win && win.getSelection();
      if (selection) {
        selection.removeAllRanges();
        selection.addRange(range);
      }
    }

    clearHighlight(name) {
      const win = this.frameWin();
      if (win && win.CSS && win.CSS.highlights) win.CSS.highlights.delete(name);
    }

    // -------------------------------------------------------- marcadores

    addBookmarkHere() {
      if (!this.book || !this.frameReady) return;
      if (this.settings.mode === 'paged' && this.marks.at(this.spine, this.page, this.pages)) {
        this.toast('Esta página já tem um marcador.');
        return;
      }
      const fraction = this.currentFraction();
      const progress = bookProgress(this.sizes, this.spine, fraction);
      const label = (this.ui.chapterTitle.textContent || this.chapterLabel(this.spine)) + ' — ' + E.percentLabel(progress);
      if (this.marks.add(this.spine, fraction, label)) this.toast('Marcador adicionado.');
    }

    renderBookmarks() {
      const list = this.ui.bookmarkList;
      list.textContent = '';
      for (const mark of this.marks.items) {
        const item = E.el(this.doc, 'li', 'bookmark');
        const go = E.el(this.doc, 'button', 'bookmark-go', mark.label);
        go.type = 'button';
        go.addEventListener('click', () => this.goTo(mark.spine, { fraction: mark.fraction }));
        const remove = E.el(this.doc, 'button', 'icon-button small bookmark-remove', '×');
        remove.type = 'button';
        remove.title = 'Remover marcador';
        remove.setAttribute('aria-label', 'Remover marcador ' + mark.label);
        remove.addEventListener('click', () => {
          if (this.marks.remove(mark.id)) this.renderBookmarks();
        });
        item.appendChild(go);
        item.appendChild(remove);
        list.appendChild(item);
      }
      this.ui.bookmarkEmpty.hidden = this.marks.items.length > 0;
    }

    async reloadBookmarks() {
      try {
        const book = await E.getJson('/api/book/' + this.id);
        this.marks.set(book.bookmarks);
        this.renderBookmarks();
        this.afterMove();
      } catch (_) { /* a lista fica como estava */ }
    }

    onNotice(notice) {
      if (notice.kind === 'bookmarks' && notice.id === this.id) {
        this.reloadBookmarks();
      } else if (notice.kind === 'error') {
        this.toast(notice.message || 'Algo deu errado.', true);
      } else if (notice.kind === 'added') {
        const errors = Array.isArray(notice.errors) ? notice.errors : [];
        if (errors.length) {
          this.toast(errors.map((error) => '“' + error.file + '”: ' + error.message).join(' · '), true);
        } else if (Array.isArray(notice.ids) && notice.ids.length) {
          this.toast(notice.ids.length === 1 ? 'Livro adicionado à biblioteca.' : notice.ids.length + ' livros adicionados.');
        }
      }
    }

    // ---------------------------------------------- leitura em voz alta

    toggleSpeech() {
      if (this.speaking) this.stopSpeech();
      else this.startSpeech();
    }

    startSpeech() {
      const synth = this.win.speechSynthesis;
      const Utterance = this.win.SpeechSynthesisUtterance;
      if (!synth || typeof Utterance !== 'function' || !this.book || !this.frameReady) {
        this.toast('A leitura em voz alta não está disponível.', true);
        return;
      }
      const begin = () => {
        const voice = chooseVoice(synth.getVoices(), this.book.language);
        if (!voice) {
          this.toast('Nenhuma voz instalada neste computador pode ler este livro.', true);
          return;
        }
        if (!this.speech) {
          this.speech = new ReadAloud({
            synth,
            Utterance,
            onSentence: (sentence) => this.onSpokenSentence(sentence),
            onEnd: () => this.onSpeechChapterEnd(),
            onStop: () => this.onSpeechStopped(),
            onError: () => this.toast('A leitura em voz alta parou.', true),
          });
        }
        this.speech.voice = voice;
        this.speech.rate = Number(this.settings.rate) || 1;
        this.ui.voiceName.textContent = 'Voz: ' + voice.name + ' (' + voice.lang + ')';
        this.speaking = true;
        this.ui.speak.textContent = 'Parar leitura';
        this.ui.speak.classList.add('active');
        this.speakFrom(this.firstVisibleOffset());
      };
      if (synth.getVoices().length) {
        begin();
        return;
      }
      this.toast('Carregando as vozes do sistema…');
      const once = () => {
        synth.removeEventListener('voiceschanged', once);
        if (!this.speaking) begin();
      };
      synth.addEventListener('voiceschanged', once);
    }

    firstVisibleOffset() {
      const doc = this.frameDoc();
      if (!doc) return 0;
      const collected = collectText(doc);
      if (typeof doc.caretRangeFromPoint === 'function') {
        const x = this.settings.mode === 'paged' ? this.geometry.gap / 2 + 4 : 24;
        const y = verticalPadding(this.geometry.pageHeight) + 4;
        const caret = doc.caretRangeFromPoint(x, y);
        if (caret && caret.startContainer && caret.startContainer.nodeType === 3) {
          const segment = collected.segments.find((item) => item.node === caret.startContainer);
          if (segment) return segment.start + caret.startOffset;
        }
      }
      return Math.floor(this.currentFraction() * collected.text.length);
    }

    speakFrom(offset) {
      const doc = this.frameDoc();
      if (!doc || !this.speech) return;
      const collected = collectText(doc);
      this.speechText = collected;
      const spans = splitSentences(collected.text, this.book.language);
      let from = spans.findIndex((span) => span.end > offset);
      if (from < 0) from = spans.length;
      const sentences = spans.map((span) => ({
        text: collected.text.slice(span.start, span.end),
        start: span.start,
        end: span.end,
      }));
      this.speech.start(sentences, from);
    }

    onSpokenSentence(sentence) {
      const doc = this.frameDoc();
      if (!doc || !this.speechText) return;
      const range = rangeFor(doc, this.speechText.segments, sentence.start, sentence.end);
      if (!range) return;
      this.setHighlight('neuralia-speech', range);
      const point = this.pointOfRange(range);
      if (this.settings.mode === 'paged') {
        const page = Math.floor(point.x / this.geometry.pageWidth);
        if (page !== this.page) this.showPage(page);
      } else {
        const scroller = this.scroller();
        if (scroller && (point.y < scroller.scrollTop || point.y > scroller.scrollTop + scroller.clientHeight - 40)) {
          this.frameWin().scrollTo(0, Math.max(0, point.y - 40));
        }
      }
    }

    onSpeechChapterEnd() {
      if (!this.speaking || !this.book) return;
      if (this.spine < this.book.spine.length - 1) {
        this.speechContinues = true;
        this.navigate(this.spine + 1, { page: 0 });
      } else {
        this.stopSpeech();
        this.toast('Fim do livro.');
      }
    }

    onSpeechStopped() {
      this.speaking = false;
      this.resetSpeechUi();
    }

    stopSpeech() {
      if (!this.speaking && !this.speechContinues) return;
      this.speaking = false;
      this.speechContinues = false;
      if (this.speech) this.speech.stop(false);
      this.resetSpeechUi();
    }

    resetSpeechUi() {
      this.clearHighlight('neuralia-speech');
      this.ui.speak.textContent = 'Ler em voz alta';
      this.ui.speak.classList.remove('active');
    }
  }

  root.NeuraliaReader = Object.freeze({
    columnsFor,
    pageGeometry,
    pageCount,
    fractionForPage,
    pageForFraction,
    scrollFraction,
    stepPaged,
    clickZone,
    bookProgress,
    locateProgress,
    restorePosition,
    flattenToc,
    currentTocIndex,
    classifyLink,
    routeLink,
    findAnchor,
    anchorHref,
    searchText,
    collectText,
    locateInSegments,
    rangeFor,
    PositionSaver,
    BookmarkList,
    cleanLabel,
    chooseVoice,
    splitSentences,
    ReadAloud,
    createBookFrame,
    bookCss,
    resolveTheme,
    normalizeSettings,
    stepFont,
    keyAction,
    Reader,
  });

  if (typeof document !== 'undefined' && document.documentElement &&
      document.documentElement.getAttribute('data-page') === 'reader') {
    const start = () => new Reader(document, root).start();
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
    else start();
  }
})(globalThis);
