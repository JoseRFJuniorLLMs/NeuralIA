'use strict';
// Cenários dos gates do leitor e da biblioteca de EPUB. Corpo de uma função
// (h, assert) que devolve a tabela de cenários; cada cenário recebe `data`
// (o livro servido pelo EpubServer do Rust) e devolve JSON que o Rust confere
// (por exemplo, as mensagens que a página mandou ao nativo, que o teste passa
// ao parser do IPC).

const WIDTH = 1200;
const HEIGHT = 800;

async function openReader(data, options) {
  const opts = options || {};
  const page = h.makePage(h.asset('reader.html'), {
    path: '/reader.html',
    search: '?book=' + data.id,
    routes: Object.assign({}, data.routes, opts.routes || {}),
    scripts: ['common.js', 'reader.js'],
    storage: opts.storage,
    speech: opts.speech,
  });
  const stage = page.doc.getElementById('stage');
  stage.clientWidth = opts.width || WIDTH;
  stage.clientHeight = opts.height || HEIGHT;
  await h.settle();
  const frame = page.doc.querySelector('iframe');
  assert.ok(frame, 'o leitor cria o iframe do livro');
  return Object.assign(page, { frame, R: page.win.NeuraliaReader });
}

// O capítulo que o iframe pediu, servido pelo Rust.
function chapterFor(data, src) {
  const bare = String(src).split('?')[0];
  const route = data.routes[bare];
  assert.ok(route, 'o iframe pediu um capítulo que o servidor serve: ' + src);
  return route.body;
}

function load(page, data, layout) {
  return h.loadFrame(page.frame, chapterFor(data, page.frame.src), layout);
}

function key(target, name, extra) {
  return target.fire('keydown', Object.assign({ key: name, target: target.body || target }, extra || {}));
}

function lastScroll(book) {
  return book.win.scrolls[book.win.scrolls.length - 1];
}

function byText(doc, tag, text) {
  return doc.querySelectorAll(tag).find((node) => node.textContent.trim() === text) || null;
}

const PAGES = (count) => count * WIDTH - 40;

return {
  // ------------------------------------------------------------ paginação
  async pagination(data) {
    const page = await openReader(data);
    const R = page.R;
    assert.equal(R.columnsFor(1200, 'auto'), 2);
    assert.equal(R.columnsFor(999, 'auto'), 1);
    assert.equal(R.columnsFor(1600, '1'), 1);
    assert.equal(R.columnsFor(600, '2'), 2);
    assert.equal(R.columnsFor(500, '2'), 1);
    // Cada página começa exatamente em N * pageWidth: as colunas nunca
    // escorregam de uma página para a seguinte.
    for (const width of [320, 777, 1000, 1201, 1920]) {
      for (const columns of [1, 2]) {
        for (const margin of [16, 40, 80]) {
          const g = R.pageGeometry(width, 700, columns, margin);
          assert.equal(g.pageWidth % g.columns, 0);
          assert.ok(g.pageWidth <= width);
          assert.ok(g.columnWidth >= 1);
          assert.ok(Math.abs(g.columns * (g.columnWidth + g.gap) - g.pageWidth) < 1e-9,
            JSON.stringify(g));
        }
      }
    }
    assert.equal(R.pageCount(PAGES(3), WIDTH), 3);
    assert.equal(R.pageCount(3 * WIDTH, WIDTH), 3);
    assert.equal(R.pageCount(3 * WIDTH + 1, WIDTH), 3);
    assert.equal(R.pageCount(3 * WIDTH + 3, WIDTH), 4);
    assert.equal(R.pageCount(0, WIDTH), 1);
    assert.equal(R.pageCount(10, 0), 1);
    // Ida e volta página -> fração -> página, também com a fração arredondada
    // a 4 casas como vai para o disco (1/3 -> 0.3333).
    for (let pages = 1; pages <= 300; pages++) {
      for (let at = 0; at < pages; at++) {
        const fraction = R.fractionForPage(at, pages);
        assert.equal(R.pageForFraction(fraction, pages), at);
        assert.equal(R.pageForFraction(R.roundFraction(fraction), pages), at, at + '/' + pages);
      }
    }
    assert.deepEqual(R.stepPaged(2, 3, 0, 3, 1), { kind: 'chapter', spine: 1, target: { page: 0 } });
    assert.deepEqual(R.stepPaged(0, 3, 1, 3, -1), { kind: 'chapter', spine: 0, target: { page: 'last' } });
    assert.deepEqual(R.stepPaged(0, 3, 0, 3, -1), { kind: 'none' });
    assert.deepEqual(R.stepPaged(2, 3, 2, 3, 1), { kind: 'none' });
    assert.equal(R.clickZone(100, WIDTH), -1);
    assert.equal(R.clickZone(600, WIDTH), 0);
    assert.equal(R.clickZone(1100, WIDTH), 1);

    // O leitor de verdade: três páginas no capítulo 1, duas no 2.
    assert.equal(page.frame.src, data.hrefs[0]);
    let book = load(page, data, { scrollWidth: PAGES(3) });
    const style = book.doc.getElementById('neuralia-reader-style');
    assert.ok(style, 'o estilo de paginação entra no documento do livro');
    const css = style.textContent;
    assert.match(css, /column-width: 520px !important/);
    assert.match(css, /column-gap: 80px !important/);
    assert.match(css, /width: 1200px !important/);
    assert.equal(page.frame.style.width, '1200px');
    const pageLabel = () => page.doc.getElementById('progress-page').textContent;
    assert.equal(pageLabel(), 'Página 1 de 3 do capítulo');

    const moves = [];
    key(page.doc, 'ArrowRight');
    moves.push(lastScroll(book));
    key(page.doc, 'PageDown');
    moves.push(lastScroll(book));
    assert.deepEqual(moves, [[1200, 0], [2400, 0]]);
    assert.equal(pageLabel(), 'Página 3 de 3 do capítulo');
    // Da última página, Espaço atravessa para o capítulo seguinte.
    key(page.doc, ' ');
    assert.equal(page.frame.src, data.hrefs[1]);
    book = load(page, data, { scrollWidth: PAGES(2) });
    assert.deepEqual(lastScroll(book), [0, 0]);
    assert.equal(pageLabel(), 'Página 1 de 2 do capítulo');
    // E Shift+Espaço volta para a ÚLTIMA página do anterior.
    key(page.doc, ' ', { shiftKey: true });
    assert.equal(page.frame.src, data.hrefs[0]);
    book = load(page, data, { scrollWidth: PAGES(3) });
    assert.deepEqual(lastScroll(book), [2400, 0]);
    key(page.doc, 'PageUp');
    key(page.doc, 'ArrowLeft');
    assert.deepEqual(lastScroll(book), [0, 0]);
    // As setas também valem com o foco dentro do livro.
    key(book.doc, 'ArrowRight');
    assert.deepEqual(lastScroll(book), [1200, 0]);

    // Zonas de clique: 30% de cada lado; o meio não vira.
    book.doc.fire('click', { target: book.doc.body, clientX: 100 });
    assert.deepEqual(lastScroll(book), [0, 0]);
    book.doc.fire('click', { target: book.doc.body, clientX: 1100 });
    assert.deepEqual(lastScroll(book), [1200, 0]);
    const before = book.win.scrolls.length;
    book.doc.fire('click', { target: book.doc.body, clientX: 600 });
    assert.equal(book.win.scrolls.length, before);

    // Roda do mouse: um gesto, uma página (a trava usa o relógio falso).
    h.setNow(page.context, 1000);
    const wheel = book.doc.fire('wheel', { deltaY: 120, deltaX: 0, deltaMode: 0 });
    assert.equal(wheel.defaultPrevented, true);
    assert.deepEqual(lastScroll(book), [2400, 0]);
    book.doc.fire('wheel', { deltaY: 120, deltaX: 0, deltaMode: 0 });
    assert.deepEqual(lastScroll(book), [2400, 0], 'o mesmo gesto não vira duas páginas');
    h.setNow(page.context, 5000);
    book.doc.fire('wheel', { deltaY: -120, deltaX: 0, deltaMode: 0 });
    assert.deepEqual(lastScroll(book), [1200, 0]);

    // Janela estreita: uma coluna, e a contagem de páginas refaz-se.
    page.doc.getElementById('stage').clientWidth = 800;
    page.win.fireWindow('resize');
    page.timers.run();
    const current = page.frame.contentDocument.getElementById('neuralia-reader-style');
    assert.match(current.textContent, /column-width: 720px !important/);
    assert.equal(page.frame.style.width, '800px');
    assert.equal(pageLabel(), 'Página 2 de 5 do capítulo');
    return { moves, pages: pageLabel() };
  },

  // ----------------------------------------------------------- sumário
  async toc(data) {
    const page = await openReader(data);
    const R = page.R;
    const flat = R.flattenToc([
      { label: 'Um', spine: 0, fragment: null, children: [{ label: 'Um.2', spine: 0, fragment: 's2', children: [] }] },
      { label: 'Fora', spine: 99, children: [] },
      { label: ' ', spine: 1, children: [] },
    ], 2);
    assert.deepEqual(flat.map((entry) => [entry.label, entry.spine, entry.fragment, entry.depth]), [
      ['Um', 0, null, 0], ['Um.2', 0, 's2', 1], ['Fora', null, null, 0], ['Sem título', 1, null, 0],
    ]);
    assert.deepEqual(R.flattenToc([], 2).map((entry) => entry.label), ['Seção 1', 'Seção 2']);
    const at = (fragment) => (fragment === 's2' ? 1 : null);
    assert.equal(R.currentTocIndex(flat, 0, 0, at), 0);
    assert.equal(R.currentTocIndex(flat, 0, 1, at), 1);
    assert.equal(R.currentTocIndex(flat, 0, 5, at), 1);
    assert.equal(R.currentTocIndex(flat, 1, 0, at), 3);

    const items = page.doc.getElementById('toc').querySelectorAll('.toc-item');
    assert.deepEqual(items.map((item) => item.textContent),
      ['Capítulo Um', 'Seção 1.2', 'Capítulo Dois', 'Capítulo Três']);
    assert.equal(items[1].style.paddingLeft, '22px', 'a subseção fica indentada');
    const current = () => items.findIndex((item) => item.classList.contains('current'));
    const title = () => page.doc.getElementById('chapter-title').textContent;
    const seen = [];
    let book = load(page, data, { scrollWidth: PAGES(3), places: { s2: { x: 1300 } } });
    seen.push([title(), current()]);
    key(page.doc, 'ArrowRight');
    seen.push([title(), current()]);
    key(page.doc, 'ArrowRight');
    seen.push([title(), current()]);
    key(page.doc, 'ArrowRight');
    book = load(page, data, { scrollWidth: PAGES(2) });
    seen.push([title(), current()]);
    assert.deepEqual(seen, [
      ['Capítulo Um', 0], ['Seção 1.2', 1], ['Seção 1.2', 1], ['Capítulo Dois', 2],
    ]);
    assert.equal(items[2].getAttribute('aria-current'), 'true');
    assert.equal(page.doc.getElementById('progress-chapter').textContent, 'Capítulo Dois');

    // Painel: T abre, o atual rola para a vista; clicar salta.
    key(page.doc, 't');
    assert.equal(page.doc.getElementById('panel-toc').hidden, false);
    assert.ok(items[2].scrolledIntoView >= 1);
    items[3].querySelector('button').fire('click');
    assert.equal(page.frame.src, data.hrefs[2]);
    book = load(page, data, { scrollWidth: PAGES(1) });
    assert.equal(title(), 'Capítulo Três');
    items[1].querySelector('button').fire('click');
    assert.equal(page.frame.src, data.hrefs[0]);
    book = load(page, data, { scrollWidth: PAGES(3), places: { s2: { x: 1300 } } });
    assert.deepEqual(lastScroll(book), [1200, 0], 'a entrada com fragmento abre na página da âncora');
    assert.equal(title(), 'Seção 1.2');
    return { seen };
  },

  // ----------------------------------------------- posição: gravar e reabrir
  async position(data) {
    const page = await openReader(data);
    const R = page.R;
    // O gravador: uma mudança arma o prazo, as seguintes só trocam o valor.
    const posted = [];
    const timers = { armed: 0, fn: null, setTimeout(fn) { this.armed++; this.fn = fn; return 1; }, clearTimeout() { this.fn = null; } };
    const saver = new R.PositionSaver('0123456789abcdef', (message) => posted.push(message), timers, 800);
    saver.note(0, 0.1);
    saver.note(0, 0.123456);
    assert.equal(timers.armed, 1);
    timers.fn();
    assert.deepEqual(posted, [{ t: 'savePosition', id: '0123456789abcdef', spine: 0, fraction: 0.1235 }]);
    assert.equal(saver.flush(), false, 'nada pendente, nada gravado');
    saver.note(0, 0.1235);
    assert.equal(saver.flush(), false, 'a mesma posição não se grava duas vezes');
    assert.deepEqual(R.restorePosition({ spine: 1, fraction: 0.5 }, 3), { spine: 1, fraction: 0.5 });
    assert.deepEqual(R.restorePosition({ spine: 7, fraction: 0.5 }, 3), { spine: 0, fraction: 0 });
    assert.deepEqual(R.restorePosition({ spine: 1, fraction: 9 }, 3), { spine: 1, fraction: 1 });
    assert.deepEqual(R.restorePosition(null, 3), { spine: 0, fraction: 0 });

    // Reabrir: a posição que o nativo gravou (capítulo 2, página 2 de 3,
    // guardada como 0.3333).
    assert.deepEqual(data.book.position, { spine: 1, fraction: 0.3333 });
    assert.deepEqual(page.posts[0], { t: 'opened', id: data.id });
    assert.equal(page.frame.src, data.hrefs[1], 'reabre no capítulo gravado');
    const book = load(page, data, { scrollWidth: PAGES(3) });
    assert.deepEqual(lastScroll(book), [1200, 0], 'e na página da fração gravada');
    page.timers.run();
    const afterOpen = page.posts.slice(1);
    // Virar três vezes depressa grava uma vez, a última.
    key(page.doc, 'ArrowRight');
    key(page.doc, 'ArrowLeft');
    key(page.doc, 'ArrowRight');
    const pending = page.posts.length;
    page.timers.run();
    assert.equal(page.posts.length, pending + 1);
    assert.deepEqual(page.posts[page.posts.length - 1], { t: 'savePosition', id: data.id, spine: 1, fraction: 0.6667 });
    // Sair da página grava já, sem esperar o prazo.
    key(page.doc, 'ArrowLeft');
    page.win.fireWindow('pagehide');
    assert.deepEqual(page.posts[page.posts.length - 1], { t: 'savePosition', id: data.id, spine: 1, fraction: 0.3333 });
    // Esc volta à biblioteca.
    key(page.doc, 'Escape');
    assert.deepEqual(page.navigations, ['/library.html']);
    const percent = page.doc.getElementById('progress-percent').textContent;
    return { posts: page.posts, afterOpen, percent };
  },

  // --------------------------------------------------------- pesquisa
  async search(data) {
    const page = await openReader(data);
    const R = page.R;
    const hits = R.searchText('A Ação e a AÇÃO, ação.', 'acao', 10, 4);
    assert.deepEqual(hits.map((hit) => hit.match), ['Ação', 'AÇÃO', 'ação']);
    assert.deepEqual(hits.map((hit) => hit.occurrence), [0, 1, 2]);
    assert.equal(hits[1].before, '…e a ');
    assert.equal(hits[1].after, ', aç…');
    assert.equal(R.searchText('abc', 'a', 10, 4).length, 0, 'menos de 2 letras não pesquisa');
    assert.equal(R.searchText('aa aa aa', 'aa', 2, 0).length, 2, 'o limite vale');

    load(page, data, { scrollWidth: PAGES(3) });
    key(page.doc, 'f', { ctrlKey: true });
    assert.equal(page.doc.getElementById('panel-search').hidden, false);
    assert.equal(page.doc.activeElement, page.doc.getElementById('search-input'));
    page.doc.getElementById('search-input').value = 'coracao';
    const submit = page.doc.getElementById('search-form').fire('submit');
    assert.equal(submit.defaultPrevented, true);
    await h.settle();
    const results = page.doc.getElementById('search-results').querySelectorAll('li');
    const rows = results.map((item) => [
      item.querySelector('.search-chapter').textContent,
      item.querySelector('mark').textContent,
      item.querySelector('.search-context').textContent,
    ]);
    assert.deepEqual(rows.map((row) => row.slice(0, 2)), [
      ['Capítulo Um', 'coração'], ['Capítulo Dois', 'coração'], ['Capítulo Três', 'coração'],
    ]);
    assert.equal(page.doc.getElementById('search-status').textContent, '3 resultados.');
    // Saltar para o resultado do capítulo 2: abre-o, marca a palavra e mostra
    // a página onde ela está.
    results[1].querySelector('button').fire('click');
    assert.equal(page.frame.src, data.hrefs[1]);
    const book = load(page, data, { scrollWidth: PAGES(2), places: { meio: { x: 1250 } } });
    const mark = book.win.CSS.highlights.get('neuralia-search');
    assert.ok(mark, 'a ocorrência fica realçada');
    assert.equal(mark.ranges[0].toString(), 'coração');
    assert.deepEqual(lastScroll(book), [1200, 0]);
    return { rows };
  },

  // ------------------------------------------------------- marcadores
  async bookmark_add(data) {
    const page = await openReader(data);
    const R = page.R;
    const posted = [];
    const list = new R.BookmarkList('0123456789abcdef', (message) => posted.push(message));
    list.set([
      { id: 3, spine: 1, fraction: 0.2, label: 'b' },
      { id: 1, spine: 0, fraction: 0.9, label: 'a' },
      { id: 2, spine: 0, fraction: 0.1, label: 'x'.repeat(300) },
      { id: 'mau', spine: 0, fraction: 0.1, label: 'ignorado' },
    ]);
    assert.deepEqual(list.items.map((mark) => mark.id), [2, 1, 3]);
    assert.equal(list.items[0].label.length, 200);
    assert.equal(R.cleanLabel('um' + String.fromCharCode(7) + 'dois   três'), 'um dois três');
    assert.equal(list.add(-1, 0.5, 'x'), false);
    assert.equal(list.remove(99), false);
    assert.equal(list.remove(1), true);
    assert.deepEqual(posted, [{ t: 'removeBookmark', id: '0123456789abcdef', bookmark: 1 }]);

    load(page, data, { scrollWidth: PAGES(3), places: { s2: { x: 1300 } } });
    key(page.doc, 'ArrowRight');
    key(page.doc, 'm');
    const add = page.posts.filter((message) => message.t === 'addBookmark');
    assert.equal(add.length, 1);
    assert.equal(add[0].id, data.id);
    assert.equal(add[0].spine, 0);
    assert.equal(add[0].fraction, 0.3333);
    assert.match(add[0].label, /^Seção 1\.2 — [0-9]+%$/);
    assert.equal(page.doc.getElementById('toast').textContent, 'Marcador adicionado.');
    return { posts: page.posts };
  },

  async bookmark_list(data) {
    const page = await openReader(data);
    load(page, data, { scrollWidth: PAGES(3), places: { s2: { x: 1300 } } });
    const list = page.doc.getElementById('bookmark-list');
    const labels = () => list.querySelectorAll('.bookmark-go').map((node) => node.textContent);
    assert.deepEqual(labels(), data.labels);
    assert.equal(page.doc.getElementById('bookmark-empty').hidden, true);
    // Com um marcador nesta página o botão di-lo, e M não duplica.
    key(page.doc, 'ArrowRight');
    assert.equal(page.doc.getElementById('bookmark-add').textContent, 'Esta página já tem marcador');
    const before = page.posts.length;
    key(page.doc, 'm');
    assert.equal(page.posts.length, before);
    // O aviso do nativo (o script que o Rust gera) recarrega a lista.
    const fetches = page.fetched.length;
    h.run(page.context, data.noticeScript);
    await h.settle();
    assert.equal(page.fetched.length, fetches + 1);
    assert.equal(page.fetched[page.fetched.length - 1], '/api/book/' + data.id);
    key(page.doc, 'b');
    assert.equal(page.doc.getElementById('panel-bookmarks').hidden, false);
    // Ir ao marcador e removê-lo.
    key(page.doc, 'ArrowLeft');
    const book = page.frame.contentWindow;
    list.querySelector('.bookmark-go').fire('click');
    assert.deepEqual(book.scrolls[book.scrolls.length - 1], [1200, 0]);
    list.querySelector('.bookmark-remove').fire('click');
    assert.deepEqual(labels(), []);
    assert.equal(page.doc.getElementById('bookmark-empty').hidden, false);
    const removal = page.posts.filter((message) => message.t === 'removeBookmark');
    assert.deepEqual(removal, [{ t: 'removeBookmark', id: data.id, bookmark: data.bookmarkId }]);
    return { posts: page.posts };
  },

  // ------------------------------------------------ leitura em voz alta
  async read_aloud(data) {
    const voices = [
      { name: 'Google português do Brasil', lang: 'pt-BR', localService: false },
      { name: 'Microsoft Helia', lang: 'pt-PT', localService: true },
      { name: 'Microsoft Maria', lang: 'pt-BR', localService: true },
      { name: 'Microsoft Zira', lang: 'en-US', localService: true, default: true },
      { name: 'Google US English', lang: 'en-US', localService: false },
    ];
    const tts = h.speech(voices);
    const page = await openReader(data, { speech: tts });
    const R = page.R;
    const pick = (list, lang) => {
      const voice = R.chooseVoice(list, lang);
      return voice ? voice.name : null;
    };
    assert.equal(pick(voices, 'pt-BR'), 'Microsoft Maria');
    assert.equal(pick(voices, 'pt'), 'Microsoft Maria', 'português: pt-BR primeiro');
    assert.equal(pick(voices, 'por'), 'Microsoft Maria');
    assert.equal(pick(voices, 'pt-PT'), 'Microsoft Maria');
    assert.equal(pick(voices, 'en-GB'), 'Microsoft Zira');
    assert.equal(pick(voices, 'fr'), 'Microsoft Maria', 'sem voz da língua: pt-BR');
    assert.equal(pick(voices.filter((voice) => voice.lang !== 'pt-BR'), 'fr'), 'Microsoft Helia');
    assert.equal(pick(voices.filter((voice) => !voice.localService), 'pt-BR'), null, 'nunca uma voz online');
    assert.equal(pick([], 'pt-BR'), null);

    const spans = R.splitSentences('Primeira frase. Segunda frase! Terceira?\nOutro bloco sem ponto', 'pt-BR');
    const text = 'Primeira frase. Segunda frase! Terceira?\nOutro bloco sem ponto';
    assert.deepEqual(spans.map((span) => text.slice(span.start, span.end)),
      ['Primeira frase.', 'Segunda frase!', 'Terceira?', 'Outro bloco sem ponto']);
    const long = ('palavra, ').repeat(80);
    const pieces = R.splitSentences(long, 'pt-BR');
    assert.ok(pieces.length >= 3);
    assert.ok(pieces.every((span) => span.end - span.start <= 280));

    // A fila: uma frase de cada vez; o fim de uma frase cancelada não avança.
    const fake = h.speech(voices);
    const events = [];
    const queue = new R.ReadAloud({
      synth: fake.synth,
      Utterance: fake.Utterance,
      onSentence: (sentence, index) => events.push('frase ' + index),
      onEnd: () => events.push('fim'),
      onStop: () => events.push('parou'),
    });
    queue.voice = voices[2];
    queue.start([{ text: 'a' }, { text: 'b' }, { text: 'c' }], 1);
    assert.deepEqual(fake.synth.spoken.map((u) => u.text), ['b']);
    assert.equal(fake.synth.spoken[0].voice.name, 'Microsoft Maria');
    assert.equal(fake.synth.spoken[0].lang, 'pt-BR');
    fake.synth.spoken[0].finish();
    fake.synth.spoken[1].finish();
    assert.deepEqual(events, ['frase 1', 'frase 2', 'fim']);
    queue.start([{ text: 'x' }, { text: 'y' }], 0);
    const stale = fake.synth.spoken[fake.synth.spoken.length - 1];
    queue.stop();
    stale.onend({});
    assert.equal(fake.synth.spoken[fake.synth.spoken.length - 1], stale, 'uma frase parada não puxa a seguinte');
    assert.equal(events[events.length - 1], 'parou');
    // Recomeçar noutro ponto: o fim atrasado da frase antiga não salta uma
    // frase da fila nova.
    queue.start([{ text: 'p' }, { text: 'q' }], 0);
    const old = fake.synth.spoken[fake.synth.spoken.length - 1];
    queue.start([{ text: 's' }, { text: 't' }], 0);
    old.onend({});
    assert.deepEqual(fake.synth.spoken.slice(-2).map((u) => u.text), ['p', 's']);

    // No leitor: L lê com a voz local do livro (pt-BR), frase a frase, e
    // realça a frase que está a ser lida; L outra vez para.
    const book = load(page, data, { scrollWidth: PAGES(3) });
    key(page.doc, 'l');
    const spoken = tts.synth.spoken;
    assert.equal(spoken.length, 1);
    assert.equal(spoken[0].voice.name, 'Microsoft Maria');
    assert.equal(spoken[0].text, 'Capítulo Um');
    assert.equal(book.win.CSS.highlights.get('neuralia-speech').ranges[0].toString(), 'Capítulo Um');
    assert.equal(page.doc.getElementById('btn-speak').textContent, 'Parar leitura');
    spoken[0].finish();
    assert.equal(spoken[1].text, 'A ação começa aqui.');
    assert.equal(book.win.CSS.highlights.get('neuralia-speech').ranges[0].toString(), 'A ação começa aqui.');
    key(page.doc, 'l');
    assert.ok(tts.synth.cancelled >= 1);
    assert.equal(book.win.CSS.highlights.has('neuralia-speech'), false);
    assert.equal(page.doc.getElementById('btn-speak').textContent, 'Ler em voz alta');
    const count = spoken.length;
    spoken[1].finish();
    assert.equal(spoken.length, count, 'parado, nada mais é lido');

    // Só vozes online: não lê, e diz porquê.
    const online = h.speech(voices.filter((voice) => !voice.localService));
    const other = await openReader(data, { speech: online });
    load(other, data, { scrollWidth: PAGES(3) });
    key(other.doc, 'l');
    assert.equal(online.synth.spoken.length, 0);
    assert.equal(other.doc.getElementById('toast').textContent,
      'Nenhuma voz instalada neste computador pode ler este livro.');
    return { first: spoken[0].text, voice: spoken[0].voice.name };
  },

  // ------------------------------------------------------------ links
  async links(data) {
    const page = await openReader(data);
    const R = page.R;
    const base = h.ORIGIN + '/book/' + data.id + '/';
    const paths = data.book.spine.map((item) => item.path);
    const kind = (href) => R.classifyLink(href, data.id, paths);
    assert.deepEqual(kind('https://example.com/x?y=1'), { kind: 'external', url: 'https://example.com/x?y=1' });
    assert.deepEqual(kind(base + 'OEBPS/Text/ch2.xhtml#meio'), { kind: 'internal', spine: 1, fragment: 'meio' });
    assert.deepEqual(kind(base + 'OEBPS/Text/ch1.xhtml'), { kind: 'internal', spine: 0, fragment: null });
    for (const href of ['javascript:alert(1)', 'data:text/html,oi', 'mailto:a@b.c', 'file:///C:/x.html',
      h.ORIGIN + '/api/library', h.ORIGIN + '/library.html', base + 'OEBPS/Images/capa.png',
      h.ORIGIN + '/book/fedcba9876543210/OEBPS/Text/ch1.xhtml', '', 'not a url']) {
      assert.equal(kind(href).kind, 'ignore', href);
    }

    const book = load(page, data, { scrollWidth: PAGES(3) });
    page.timers.run();
    const anchor = (text) => {
      const node = byText(book.doc, 'a', text);
      assert.ok(node, 'link ' + text);
      return node;
    };
    const click = (target) => book.doc.fire('click', { target, clientX: 600 });
    const posts = page.posts.length;
    // Externo: guarda a posição e pede ao nativo; o iframe não navega.
    key(page.doc, 'ArrowRight');
    const loads = page.frame.loads.length;
    const external = click(anchor('externo').querySelector('em') || anchor('externo'));
    assert.equal(external.defaultPrevented, true);
    const sent = page.posts.slice(posts);
    assert.deepEqual(sent.map((message) => message.t), ['savePosition', 'openExternal']);
    assert.equal(sent[1].url, 'https://example.com/pagina');
    assert.equal(page.frame.loads.length, loads);
    // javascript: e data: nunca fazem nada.
    for (const text of ['perigoso', 'dado']) {
      const event = click(anchor(text));
      assert.equal(event.defaultPrevented, true, text);
      assert.equal(page.posts.length, posts + 2, text);
      assert.equal(page.frame.loads.length, loads, text);
    }
    // Interno: capítulo 2, na âncora.
    click(anchor('interno'));
    assert.equal(page.frame.src, data.hrefs[1]);
    const next = load(page, data, { scrollWidth: PAGES(2), places: { meio: { x: 1250 } } });
    assert.deepEqual(lastScroll(next), [1200, 0]);
    return { posts: page.posts.slice(posts) };
  },

  // ------------------------------------------------------------ sandbox
  async sandbox(data) {
    const page = await openReader(data);
    const R = page.R;
    const made = R.createBookFrame(h.documentFrom('<html><body></body></html>'));
    assert.equal(made.getAttribute('sandbox'), 'allow-same-origin');
    const frame = page.frame;
    assert.equal(frame.getAttribute('sandbox'), 'allow-same-origin');
    assert.equal(frame.parentNode, page.doc.getElementById('frame-host'));
    load(page, data, { scrollWidth: PAGES(3) });
    key(page.doc, ' ');
    key(page.doc, ' ');
    key(page.doc, ' ');
    assert.ok(frame.loads.length >= 2);
    assert.deepEqual(frame.sandboxAtLoad, frame.loads.map(() => 'allow-same-origin'),
      'todo o capítulo carrega com o iframe já no sandbox, sem allow-scripts');
    // As nossas páginas: nenhum script inline, nenhum on*, nenhum style
    // inline (a CSP das páginas não os deixaria correr, e não fazem falta).
    const inline = {};
    const scripts = {};
    for (const name of ['reader.html', 'library.html']) {
      const doc = h.documentFrom(h.asset(name));
      inline[name] = h.inlineCode(doc);
      scripts[name] = doc.querySelectorAll('script').map((node) => node.getAttribute('src'));
    }
    assert.deepEqual(inline, { 'reader.html': [], 'library.html': [] });
    assert.deepEqual(scripts, { 'reader.html': ['/common.js', '/reader.js'], 'library.html': ['/common.js', '/library.js'] });
    return { sandbox: frame.getAttribute('sandbox') };
  },

  // ------------------------------------------- capítulo XHTML mal formado
  async html_fallback(data) {
    const page = await openReader(data);
    const R = page.R;
    assert.equal(R.htmlFallbackHref('/book/x/a.xhtml'), '/book/x/a.xhtml?as=html');
    assert.equal(R.htmlFallbackHref('http://h/book/x/a.xhtml#n'), 'http://h/book/x/a.xhtml?as=html');
    assert.equal(R.htmlFallbackHref('/book/x/a.xhtml?as=html'), null);
    const broken = '<html><head><title>x</title></head><body><p>Um&nbsp;espaço</p></body></html>';
    h.loadFrame(page.frame, broken, { scrollWidth: PAGES(1) });
    assert.equal(page.frame.src, h.ORIGIN + data.hrefs[0] + '?as=html', 'volta a pedir como HTML');
    // Como HTML abre; e um segundo erro não faz ciclo.
    const html = h.loadFrame(page.frame, broken, { scrollWidth: PAGES(2), type: 'text/html' });
    assert.deepEqual(lastScroll(html), [0, 0]);
    assert.equal(page.doc.getElementById('progress-page').textContent, 'Página 1 de 2 do capítulo');
    const loads = page.frame.loads.length;
    h.loadFrame(page.frame, broken, { scrollWidth: PAGES(1) });
    assert.equal(page.frame.loads.length, loads);
    return { loads: page.frame.loads };
  },

  // --------------------------------------------------------- biblioteca
  async library(data) {
    const page = h.makePage(h.asset('library.html'), {
      path: '/library.html',
      routes: data.routes,
      scripts: ['common.js', 'library.js'],
    });
    await h.settle();
    const L = page.win.NeuraliaLibrary;
    const books = data.library.books;
    const titles = (list) => list.map((book) => book.title);
    assert.deepEqual(titles(L.filterBooks(books, 'agata')), ['Ágata e o Mar']);
    assert.deepEqual(titles(L.filterBooks(books, 'ANA autora')), ['O Livro de Teste']);
    assert.deepEqual(titles(L.filterBooks(books, 'ninguem')), ['Ágata e o Mar']);
    assert.deepEqual(titles(L.filterBooks(books, 'xyz')), []);
    assert.equal(L.filterBooks(books, '  ').length, books.length);
    assert.deepEqual(titles(L.sortBooks(books, 'title')), ['Abelha Rainha', 'Ágata e o Mar', 'O Livro de Teste']);
    assert.deepEqual(titles(L.sortBooks(books, 'author')), ['O Livro de Teste', 'Abelha Rainha', 'Ágata e o Mar']);
    assert.deepEqual(titles(L.sortBooks(books, 'recent')).slice(0, 1), ['O Livro de Teste']);
    assert.equal(L.continueReading(books).title, 'O Livro de Teste');
    assert.equal(L.coverHue('Ágata'), L.coverHue('Ágata'));
    assert.equal(L.readerHref('../x'), null);

    const doc = page.doc;
    const grid = doc.getElementById('grid');
    const cards = () => grid.querySelectorAll('.card');
    const cardTitles = () => cards().map((card) => card.querySelector('.card-title').textContent);
    assert.equal(cards().length, 3);
    assert.equal(doc.getElementById('continue').hidden, false);
    assert.equal(doc.getElementById('continue-card').querySelector('.continue-title').textContent, 'O Livro de Teste');
    const agata = cards().find((card) => card.querySelector('.card-title').textContent === 'Ágata e o Mar');
    const fallback = agata.querySelector('.cover-fallback');
    assert.ok(fallback, 'sem capa, uma capa gerada');
    assert.equal(fallback.querySelector('.fallback-title').textContent, 'Ágata e o Mar');
    assert.equal(fallback.querySelector('.fallback-author').textContent, 'Zé Ninguém');
    const withCover = cards().find((card) => card.querySelector('.card-title').textContent === 'O Livro de Teste');
    assert.equal(withCover.querySelector('img').src, '/cover/' + data.readId);
    assert.equal(withCover.querySelector('.progress-label').textContent, data.readPercent);
    assert.equal(agata.querySelector('.progress-label').textContent, 'Não iniciado');

    const search = doc.getElementById('search');
    search.value = 'agata';
    search.fire('input');
    assert.deepEqual(cardTitles(), ['Ágata e o Mar']);
    assert.equal(doc.getElementById('shelf-title').textContent, 'Biblioteca (1 de 3)');
    assert.equal(doc.getElementById('continue').hidden, true);
    search.value = 'nada disto';
    search.fire('input');
    assert.equal(doc.getElementById('no-match').hidden, false);
    search.value = '';
    search.fire('input');
    const sort = doc.getElementById('sort');
    sort.value = 'author';
    sort.fire('change');
    assert.deepEqual(cardTitles(), ['O Livro de Teste', 'Abelha Rainha', 'Ágata e o Mar']);
    assert.equal(page.win.localStorage.getItem('neuralia-epub-sort'), 'author');
    sort.value = 'title';
    sort.fire('change');
    assert.deepEqual(cardTitles(), ['Abelha Rainha', 'Ágata e o Mar', 'O Livro de Teste']);

    // Remover pede confirmação na página; cancelar não manda nada.
    const confirm = doc.getElementById('confirm');
    cards()[0].querySelector('.card-remove').fire('click');
    assert.equal(confirm.hidden, false);
    assert.equal(doc.getElementById('confirm-text').textContent, 'Remover “Abelha Rainha” da biblioteca?');
    doc.fire('keydown', { key: 'Escape', target: doc.body });
    assert.equal(confirm.hidden, true);
    assert.equal(page.posts.length, 0);
    cards()[0].querySelector('.card-remove').fire('click');
    doc.getElementById('confirm-ok').fire('click');
    assert.equal(confirm.hidden, true);
    doc.getElementById('add').fire('click');
    doc.fire('keydown', { key: 'o', ctrlKey: true, target: doc.body });
    // Abrir um livro é navegar para o leitor.
    cards()[1].fire('click');
    assert.deepEqual(page.navigations, ['/reader.html?book=' + data.agataId]);
    // Avisos do nativo, pelo script que o Rust gera.
    const fetches = page.fetched.length;
    h.run(page.context, data.failureScript);
    await h.settle();
    const banner = doc.getElementById('banner');
    assert.equal(banner.hidden, false);
    assert.equal(banner.classList.contains('error'), true);
    assert.equal(doc.getElementById('banner-text').textContent,
      'Não foi possível adicionar “protegido.epub”: Este livro tem DRM e não pode ser aberto.');
    assert.equal(page.fetched.length, fetches + 1, 'a lista é recarregada');
    doc.fire('keydown', { key: 'Escape', target: doc.body });
    doc.getElementById('home').fire('click');
    return { posts: page.posts };
  },
};
