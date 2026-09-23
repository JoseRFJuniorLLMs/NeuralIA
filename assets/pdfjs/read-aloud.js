// Leitura em voz alta do NeuralIA (SPEC-0110, fase offline). Ficheiro do
// NeuralIA, sem upstream.
//
// Um so script para dois sitios: o visualizador de PDF carrega-o de
// http://neuralia-pdf.localhost/read-aloud.js (serve_pdf_asset) e o Modo
// Leitura recebe-o como initialization script. O motor e o
// window.speechSynthesis do WebView2:
//   - vozes do Windows (voice.localService === true) sintetizam no proprio
//     computador -- o texto nao sai dele;
//   - vozes "online" que o runtime expuser (localService === false) mandam o
//     texto ao provedor da voz. Nunca sao escolhidas por omissao: so quando o
//     utilizador as escolhe no seletor, que o diz por extenso.
// Este ficheiro nao abre ligacoes de rede; o CSP do visualizador continua com
// connect-src 'self' e o do Modo Leitura com connect-src 'none'.
//
// Porque frase a frase: o Chromium corta utterances longas (em algumas vozes a
// sintese para ao fim de ~15 s e o 'end' nunca chega). Uma fila de frases
// curtas -- cada uma so e entregue depois do 'end' da anterior -- deixa
// realcar, saltar e mudar a velocidade sem perder o sitio.
(function () {
  'use strict';
  if (typeof window === 'undefined' || window.top !== window) return;
  if (window.NeuralIAReadAloud) return;

  const RATES = [0.75, 1, 1.25, 1.5, 1.75, 2];
  const MIN_RATE = 0.75;
  const MAX_RATE = 2;
  // Teto de uma utterance em caracteres: ~12 s a 1x, abaixo do corte do
  // Chromium. Frases maiores partem-se em virgulas ou espacos.
  const MAX_UTTERANCE = 180;
  const STORE_KEY = 'neuralia.readAloud.v1';
  const HIGHLIGHT_NAME = 'neuralia-read-aloud';
  const HIGHLIGHT_CLASS = 'neuralia-ra-hl';
  const HINT_DELAY_MS = 450;
  const NOTICE_MS = 3200;
  const WATCHDOG_MS = 1000;
  const VOICE_WAIT_MS = 1500;

  const OFFLINE_GROUP = 'Offline (Windows) — o texto fica no computador';
  const ONLINE_GROUP = 'Online — precisa de internet; o texto vai para o provedor da voz';
  const NO_VOICE = 'Nenhuma voz do Windows está disponível.\nInstale uma em Configurações › Hora e idioma › Fala.';
  const NO_OFFLINE_VOICE = 'Não há voz offline do Windows.\nEscolha uma voz online no seletor (o texto sai do computador)\nou instale uma em Configurações › Hora e idioma › Fala.';
  const NO_SPEECH = 'Este WebView não oferece síntese de voz.';
  const NO_ONLINE_VOICE = 'Nenhuma voz online neste Windows — lê com as vozes instaladas';
  const NOTHING_TO_READ = 'Não há texto para ler aqui (página digitalizada?).';
  const END_OF_TEXT = 'Leitura concluída.';

  // ---------------------------------------------------------------- texto

  const TERMINAL = /[.!?…]/;
  const CLOSER = /[)\]}"'”’»]/;
  const SPACE = /\s/;
  const LOWER = /\p{Ll}/u;
  const WORDISH = /[\p{L}\p{N}.ºª]/u;
  const SPEAKABLE = /[\p{L}\p{N}]/u;
  const SOFT_CUT = /[,;:–—)]/;
  // Abreviaturas que nunca fecham frase quando a palavra seguinte comeca por
  // maiuscula ou algarismo ("Sr. Silva", "pág. 12"). Palavras comuns que
  // tambem sao abreviaturas (mar, set, out, dez, ver) ficam de fora: "olhou o
  // mar. Depois" tem de partir. "etc." tem regra propria.
  const ABBREVIATIONS = new Set((
    'sr sra srs sras srta srtas dr dra drs dras prof profa profs profas exmo exma exmos exmas ' +
    'ilmo ilma revmo revma sto sta eng enga arq adv gen cel ten sgt gov pres dir dep sen av ' +
    'pça pca rod nº núm num pág pag págs pags pp fl fls fig figs tab cap caps vol ' +
    'vols ed eds org orgs coord trad obs art arts inc ltda cia séc sec aprox ex cf op cit ibid ' +
    'id vs var min máx max mín tel dept depto univ jan fev abr jun jul ago nov jr mr mrs ms st'
  ).split(' '));

  function wordBefore(text, from, i) {
    let k = i;
    while (k > from && WORDISH.test(text[k - 1])) k--;
    return text.slice(k, i);
  }

  // `i` e o primeiro terminal da corrida, `j` o primeiro caracter depois dela.
  function isBoundary(text, from, i, j, to) {
    let k = j;
    while (k < to && SPACE.test(text[k])) k++;
    if (k >= to) return true;
    // Minuscula a seguir nunca abre frase: "etc. e", "aprox. dez", "Ah! disse".
    if (LOWER.test(text[k])) return false;
    if (text[i] !== '.' || (j > i + 1 && TERMINAL.test(text[i + 1]))) return true;
    const word = wordBefore(text, from, i);
    const bare = word.toLowerCase();
    if (!bare || bare === 'etc') return true;
    if (ABBREVIATIONS.has(bare)) return false;
    // Iniciais ("J. R. Tolkien") e siglas com pontos ("e.g.", "a.C.", "S.A.",
    // "Ph.D."): so letras entre os pontos. Um numero ("12.000", "R$ 1.500",
    // "3.14") ou um endereco ("www.exemplo.pt") no fim da frase fecha-a.
    if (/^\p{L}$/u.test(word) || /^(?:\p{L}{1,3}\.)+\p{L}{1,3}$/u.test(word)) return false;
    return true;
  }

  function trimRange(text, a, b) {
    while (a < b && SPACE.test(text[a])) a++;
    while (b > a && SPACE.test(text[b - 1])) b--;
    return [a, b];
  }

  function cutPoint(text, a, limit) {
    const floor = a + Math.floor(MAX_UTTERANCE * 0.4);
    for (let k = limit; k > floor; k--) {
      if (SPACE.test(text[k]) && SOFT_CUT.test(text[k - 1])) return k;
    }
    for (let k = limit; k > floor; k--) {
      if (SPACE.test(text[k])) return k;
    }
    return limit;
  }

  function pushSentence(text, a, b, out) {
    [a, b] = trimRange(text, a, b);
    while (b - a > MAX_UTTERANCE) {
      const cut = cutPoint(text, a, a + MAX_UTTERANCE);
      const [x, y] = trimRange(text, a, cut);
      if (y > x && SPEAKABLE.test(text.slice(x, y))) out.push({ start: x, end: y });
      a = cut;
      while (a < b && SPACE.test(text[a])) a++;
    }
    if (b > a && SPEAKABLE.test(text.slice(a, b))) out.push({ start: a, end: b });
  }

  function splitBlock(text, from, to, out) {
    let start = from;
    let i = from;
    while (i < to) {
      if (!TERMINAL.test(text[i])) {
        i++;
        continue;
      }
      let j = i + 1;
      while (j < to && (TERMINAL.test(text[j]) || CLOSER.test(text[j]))) j++;
      // "3.14", "1.000,50", "www.exemplo.pt": o ponto sem espaco a seguir nao
      // fecha nada.
      if (j < to && !SPACE.test(text[j])) {
        i = j;
        continue;
      }
      if (isBoundary(text, from, i, j, to)) {
        pushSentence(text, start, j, out);
        start = j;
      }
      i = j;
    }
    pushSentence(text, start, to, out);
  }

  // Frases de `text` como intervalos [start, end). `breaks` sao fronteiras
  // duras (fim de bloco no Modo Leitura): nenhuma frase as atravessa.
  function segmentSentences(text, breaks) {
    const out = [];
    const bounds = [0].concat(breaks || [], [text.length]);
    for (let b = 0; b + 1 < bounds.length; b++) {
      if (bounds[b + 1] > bounds[b]) splitBlock(text, bounds[b], bounds[b + 1], out);
    }
    return out;
  }

  // Junta as unidades de uma pagina (spans do PDF.js, blocos do Modo Leitura)
  // num so texto e guarda onde cada uma comeca, para a frase lida voltar a ser
  // posta nos nos certos. Como no PDF.js, as unidades de uma linha juntam-se
  // sem separador (o proprio PDF.js poe os espacos) e o fim de linha vale um
  // espaco -- salvo a palavra partida por hifen, que se cola e perde o hifen
  // so na fala.
  function buildPageText(units, hard) {
    let text = '';
    const starts = [];
    const ends = [];
    const drops = [];
    const breaks = [];
    for (let k = 0; k < units.length; k++) {
      const unit = units[k] || {};
      const piece = typeof unit.text === 'string' ? unit.text : '';
      starts.push(text.length);
      text += piece;
      ends.push(text.length);
      if (hard) {
        breaks.push(text.length);
        text += '\n';
        continue;
      }
      if (unit.eol) {
        // Num PDF com tags (Word, InDesign, PDF/UA) o PDF.js fecha o texto em
        // cada conteudo marcado e o fim de linha chega num item VAZIO: o
        // hifen fica no pedaco anterior e o resto da palavra pode vir depois
        // de outros vazios. Olha-se o texto ja junto e salta-se o que e vazio.
        let following = '';
        for (let n = k + 1; n < units.length && !following; n++) {
          const next = units[n];
          following = next && typeof next.text === 'string' ? next.text : '';
        }
        if (/\p{L}-$/u.test(text.slice(-3)) && /^\p{Ll}/u.test(following)) {
          drops.push(text.length - 1);
        } else {
          text += '\n';
        }
      }
    }
    return { text, starts, ends, drops, breaks };
  }

  function pageModel(units, hard) {
    const model = buildPageText(units || [], !!hard);
    model.sentences = segmentSentences(model.text, model.breaks);
    model.dropSet = new Set(model.drops);
    return model;
  }

  function spokenText(model, sentence) {
    let out = '';
    for (let k = sentence.start; k < sentence.end; k++) {
      if (!model.dropSet.has(k)) out += model.text[k];
    }
    return out.replace(/\s+/g, ' ').trim();
  }

  // Unidades tocadas por [start, end), com o trecho de cada uma.
  function segmentsFor(model, start, end) {
    const out = [];
    let lo = 0;
    let hi = model.ends.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (model.ends[mid] <= start) lo = mid + 1;
      else hi = mid;
    }
    for (let k = lo; k < model.starts.length && model.starts[k] < end; k++) {
      if (model.ends[k] <= model.starts[k]) continue;
      out.push({
        unit: k,
        from: Math.max(start, model.starts[k]) - model.starts[k],
        to: Math.min(end, model.ends[k]) - model.starts[k],
      });
    }
    return out;
  }

  function sentenceAt(model, position) {
    const list = model.sentences;
    for (let i = 0; i < list.length; i++) {
      if (list[i].end > position) return i;
    }
    return list.length;
  }

  // ---------------------------------------------------------------- vozes

  function isLocal(voice) {
    return !!voice && voice.localService === true;
  }

  function normLang(value) {
    return String(value || '').replace(/_/g, '-').toLowerCase();
  }

  function preferredLangs(lang) {
    const wanted = normLang(lang) || 'pt-br';
    const primary = wanted.split('-')[0];
    // O dono e brasileiro: um documento "pt" sem regiao le-se em pt-BR.
    if (primary === 'pt') return ['pt-br', 'pt'];
    return wanted === primary ? [primary] : [wanted, primary];
  }

  function primaryOf(lang) {
    return normLang(lang).split('-')[0];
  }

  // Palavras que so uma das linguas usa a toda a hora. Contam-se no texto da
  // pagina: o idioma que o documento nao declara (o Modo Leitura e o
  // visualizador sao paginas NOSSAS, em pt -- o lang delas nao diz nada do
  // artigo) sai daqui. Sem confianca, nada: fica o pt-BR.
  const STOPWORDS = {
    pt: 'não uma os com é são do da dos das nos nas pelo pela também foi ao aos seu sua isso muito já quando então',
    en: 'the and of to is in that it was for with as are on be this by not or have from which were their has but',
    es: 'el los las del y en es con una lo al más pero está son fue esta',
    fr: 'le les des du et est une dans pour pas qui sur au avec il ce ne sont nous vous mais cette',
    de: 'der die das und ist nicht ein eine zu den mit von auf für sich dem auch im sind werden',
  };
  const STOPWORD_SETS = Object.fromEntries(
    Object.entries(STOPWORDS).map(([code, words]) => [code, new Set(words.split(' '))])
  );
  const GUESS_MIN_HITS = 4;

  function guessLang(text) {
    const counts = {};
    for (const code of Object.keys(STOPWORD_SETS)) counts[code] = 0;
    const words = String(text || '').toLowerCase().match(/\p{L}+/gu) || [];
    for (const word of words.slice(0, 4000)) {
      for (const code of Object.keys(STOPWORD_SETS)) {
        if (STOPWORD_SETS[code].has(word)) counts[code] += 1;
      }
    }
    const ranked = Object.entries(counts).sort((a, b) => b[1] - a[1]);
    const [best, second] = ranked;
    if (best[1] < GUESS_MIN_HITS || best[1] < second[1] * 1.5) return '';
    return best[0];
  }

  // A voz a usar. Uma escolha guardada vale enquanto existir (foi o
  // utilizador que a fez, online incluida). Sem ela: so vozes locais -- a do
  // idioma do documento (pt-BR primeiro), depois a predefinida do Windows,
  // depois qualquer local. Uma voz online nunca e escolhida por omissao.
  function chooseVoice(voices, stored, lang) {
    const list = Array.prototype.filter.call(voices || [], (v) => v && typeof v.voiceURI === 'string');
    if (stored) {
      const kept = list.find((v) => v.voiceURI === stored);
      if (kept) return kept;
    }
    const local = list.filter(isLocal);
    for (const wanted of preferredLangs(lang)) {
      const exact = local.filter((v) => normLang(v.lang) === wanted);
      if (exact.length) return exact.find((v) => v.default) || exact[0];
      const regional = local.filter((v) => normLang(v.lang).split('-')[0] === wanted);
      if (regional.length) return regional.find((v) => v.default) || regional[0];
    }
    return local.find((v) => v.default) || local[0] || null;
  }

  function groupVoices(voices, lang) {
    const primary = preferredLangs(lang)[0].split('-')[0];
    const rank = (v) => (normLang(v.lang).split('-')[0] === primary ? 0 : 1);
    const order = (a, b) => rank(a) - rank(b) || String(a.name).localeCompare(String(b.name));
    const offline = [];
    const online = [];
    for (const v of voices || []) {
      if (!v || typeof v.voiceURI !== 'string') continue;
      (isLocal(v) ? offline : online).push(v);
    }
    return { offline: offline.sort(order), online: online.sort(order) };
  }

  // A velocidade e sempre uma das opcoes do seletor: um valor guardado fora da
  // lista (outra versao, armazenamento mexido) vai para a mais proxima, para o
  // seletor nunca ficar em branco.
  function clampRate(value) {
    const n = Number(value);
    if (!Number.isFinite(n)) return 1;
    const bounded = Math.min(MAX_RATE, Math.max(MIN_RATE, n));
    let best = RATES[0];
    for (const option of RATES) {
      if (Math.abs(option - bounded) < Math.abs(best - bounded)) best = option;
    }
    return best;
  }

  function prefsStore(win) {
    // Origem local do visualizador: o localStorage basta. No Modo Leitura a
    // origem e opaca e o acesso lanca, e cada artigo e uma WebView nova: a
    // escolha vale so para o artigo aberto.
    return {
      load() {
        try {
          const raw = win.localStorage.getItem(STORE_KEY);
          const value = raw ? JSON.parse(raw) : null;
          return value && typeof value === 'object' ? value : {};
        } catch (e) {
          return {};
        }
      },
      save(value) {
        try {
          win.localStorage.setItem(STORE_KEY, JSON.stringify(value));
        } catch (e) {
          /* sem armazenamento: a escolha vale so para esta pagina */
        }
      },
    };
  }

  // ---------------------------------------------------------------- DOM

  function textNodesOf(el, out) {
    const kids = (el && el.childNodes) || [];
    for (let i = 0; i < kids.length; i++) {
      const child = kids[i];
      if (child.nodeType === 3) out.push(child);
      else if (child.nodeType === 1) textNodesOf(child, out);
    }
    return out;
  }

  function offsetWithin(el, node, offset) {
    let acc = 0;
    for (const text of textNodesOf(el, [])) {
      if (text === node) return acc + Math.min(offset, text.data.length);
      acc += text.data.length;
    }
    return 0;
  }

  function pointAt(el, offset) {
    const nodes = textNodesOf(el, []);
    let acc = 0;
    for (let i = 0; i < nodes.length; i++) {
      const length = nodes[i].data.length;
      if (offset < acc + length || (i === nodes.length - 1 && offset <= acc + length)) {
        return { node: nodes[i], offset: Math.max(0, offset - acc) };
      }
      acc += length;
    }
    return null;
  }

  // Onde comeca a selecao, como (pagina, unidade, deslocamento); null se ela
  // nao estiver no texto que se le.
  function selectionStart(win, locate) {
    let selection = null;
    try {
      selection = win.getSelection ? win.getSelection() : null;
    } catch (e) {
      return null;
    }
    if (!selection || selection.isCollapsed || !selection.rangeCount) return null;
    const range = selection.getRangeAt(0);
    let node = range.startContainer;
    let offset = range.startOffset;
    if (node && node.nodeType === 1 && node.childNodes && node.childNodes[offset]) {
      node = node.childNodes[offset];
      offset = 0;
    }
    const hit = node ? locate(node) : null;
    if (!hit) return null;
    const within = node.nodeType === 3 ? offsetWithin(hit.el, node, offset) : 0;
    return { page: hit.page, unit: hit.unit, offset: within };
  }

  function createPainter(win, doc) {
    let marked = [];

    function registry() {
      const css = win.CSS;
      return css && css.highlights && typeof win.Highlight === 'function' && doc.createRange
        ? css.highlights
        : null;
    }

    function clear() {
      const highlights = registry();
      if (highlights) highlights.delete(HIGHLIGHT_NAME);
      for (const el of marked) el.classList.remove(HIGHLIGHT_CLASS);
      marked = [];
    }

    function reveal(node) {
      let rect = null;
      try {
        rect = node.getBoundingClientRect();
      } catch (e) {
        return;
      }
      const height = win.innerHeight || 0;
      if (!rect || (rect.top >= 72 && rect.bottom <= height - 96)) return;
      try {
        node.scrollIntoView({ block: 'center', behavior: 'smooth' });
      } catch (e) {
        /* no ecra ou nao, a leitura continua */
      }
    }

    // `segments`: [{node, from, to}] com deslocamentos no texto de cada no.
    // Com a CSS Custom Highlight API o realce e exato ao caracter e o DOM nao
    // muda; sem ela marca-se o no inteiro.
    function paint(segments, scroll) {
      clear();
      if (!segments.length) return;
      const highlights = registry();
      if (highlights) {
        const ranges = [];
        for (const seg of segments) {
          const a = pointAt(seg.node, seg.from);
          const b = pointAt(seg.node, seg.to);
          if (!a || !b) continue;
          const range = doc.createRange();
          range.setStart(a.node, a.offset);
          range.setEnd(b.node, b.offset);
          ranges.push(range);
        }
        if (ranges.length) highlights.set(HIGHLIGHT_NAME, new win.Highlight(...ranges));
      } else {
        for (const seg of segments) {
          seg.node.classList.add(HIGHLIGHT_CLASS);
          marked.push(seg.node);
        }
      }
      if (scroll) reveal(segments[0].node);
    }

    return { paint, clear };
  }

  // ---------------------------------------------------------------- motor

  function createEngine(options) {
    const source = options.source;
    const speech = options.speech;
    const Utterance = options.Utterance;
    const painter = options.painter;
    const notify = options.notify;
    // O idioma que o DOCUMENTO declara (o /Lang do PDF); vazio quando nao
    // declara nada -- ai vale o que o texto da pagina diz (`guessLang`).
    const declared = normLang(options.lang);
    const prefs = options.prefs;
    const saved = prefs.load();

    let rate = clampRate(saved.rate);
    // A voz escolhida POR IDIOMA: a inglesa escolhida para um artigo em
    // ingles nao passa a ler os documentos em portugues.
    const byLang = {};
    if (saved.voices && typeof saved.voices === 'object') {
      for (const [code, uri] of Object.entries(saved.voices)) {
        if (typeof uri === 'string' && /^[a-z]{2,3}$/.test(code)) byLang[code] = uri;
      }
    }
    // A escolha de antes desta versao (uma so para tudo) vale para o idioma
    // da propria voz.
    const legacy = typeof saved.voice === 'string' ? saved.voice : '';
    // O ultimo idioma que o texto disse com confianca: uma pagina curta
    // (titulo, figura) nao troca a voz a meio do documento.
    let heard = '';
    let state = 'idle';
    // Cada operacao que invalida o que estava em curso incrementa o token:
    // leituras de pagina assincronas que voltam depois disso nao fazem nada.
    let token = 0;
    let page = 0;
    let index = 0;
    let model = null;
    // A utterance viva. Guardada tambem porque o Chromium recolhe utterances
    // sem referencia antes do 'end'. So ela pode fazer a fila andar.
    let current = null;
    let spokeAny = false;
    let watchdog = 0;
    let quiet = 0;
    const cache = new Map();
    const changeListeners = [];
    const voiceListeners = [];
    const voiceWaiters = [];

    function safe(fn) {
      try {
        return fn();
      } catch (e) {
        return null;
      }
    }

    function voices() {
      return safe(() => Array.prototype.slice.call(speech.getVoices() || [])) || [];
    }

    // O idioma da leitura agora: o declarado pelo documento; senao o da
    // pagina a ser lida; senao o ultimo ouvido; senao pt-BR.
    function language() {
      return declared || (model && model.lang) || heard || 'pt-br';
    }

    function storedVoice(primary) {
      if (byLang[primary]) return byLang[primary];
      if (!legacy) return '';
      const kept = voices().find((v) => v && v.voiceURI === legacy);
      return kept && primaryOf(kept.lang) === primary ? legacy : '';
    }

    function voice() {
      const lang = language();
      return chooseVoice(voices(), storedVoice(primaryOf(lang)), lang);
    }

    function pageCount() {
      return Math.max(0, Number(safe(() => source.pageCount())) || 0);
    }

    function snapshot() {
      return {
        state,
        page,
        index,
        sentences: model ? model.sentences.length : 0,
        pages: pageCount(),
        rate,
        voice: voice(),
      };
    }

    function emit() {
      const snap = snapshot();
      for (const fn of changeListeners) safe(() => fn(snap));
    }

    function save() {
      prefs.save({ voices: byLang, voice: legacy, rate });
    }

    function onVoicesChanged() {
      for (const resolve of voiceWaiters.splice(0)) resolve();
      for (const fn of voiceListeners) safe(fn);
      emit();
    }

    if (typeof speech.addEventListener === 'function') {
      speech.addEventListener('voiceschanged', onVoicesChanged);
    } else {
      speech.onvoiceschanged = onVoicesChanged;
    }

    // O Chromium so enche getVoices() depois de 'voiceschanged' -- e o Edge
    // (o motor do WebView2) enche-o em duas levas: primeiro as vozes online,
    // dezenas de milissegundos depois as do Windows. Espera-se por uma voz
    // utilizavel (a guardada ou uma local), nao pela primeira leva: senao um
    // Ctrl+Shift+U logo a abrir dizia "nao ha voz offline" com ela a caminho.
    function waitForVoices() {
      if (voice()) return Promise.resolve();
      return new Promise((resolve) => {
        let done = false;
        const finish = () => {
          if (done) return;
          done = true;
          resolve();
        };
        const check = () => {
          if (done) return;
          if (voice()) finish();
          else voiceWaiters.push(check);
        };
        voiceWaiters.push(check);
        setTimeout(finish, VOICE_WAIT_MS);
      });
    }

    function loadModel(p) {
      let pending = cache.get(p);
      if (!pending) {
        pending = Promise.resolve()
          .then(() => source.pageUnits(p))
          .then(
            (units) => pageModel(units || [], !!source.hardBreaks),
            () => pageModel([], false)
          )
          .then((m) => {
            m.lang = guessLang(m.text);
            return m;
          });
        cache.set(p, pending);
        // So a vizinhanca fica em memoria: um livro de 900 paginas lido de
        // ponta a ponta nao acumula o texto todo.
        for (const key of Array.from(cache.keys())) {
          if (key < p - 1 || key > p + 1) cache.delete(key);
        }
      }
      return pending;
    }

    function stopWatchdog() {
      if (watchdog) clearTimeout(watchdog);
      watchdog = 0;
    }

    // Rede de seguranca para o 'end' perdido (bug conhecido do Chromium): se
    // o motor ficar calado dois periodos seguidos com a nossa utterance viva,
    // conta como terminada.
    function checkStall() {
      watchdog = 0;
      const u = current;
      if (!u || state !== 'speaking') return;
      const busy = safe(() => !!(speech.speaking || speech.pending));
      if (busy) {
        quiet = 0;
      } else if (++quiet >= 2) {
        finished(u);
        return;
      }
      watchdog = setTimeout(checkStall, WATCHDOG_MS);
    }

    function armWatchdog() {
      stopWatchdog();
      quiet = 0;
      watchdog = setTimeout(checkStall, WATCHDOG_MS);
    }

    function silence() {
      current = null;
      stopWatchdog();
      safe(() => speech.cancel());
    }

    function paint(scroll) {
      const sentence = model && model.sentences[index];
      if (!sentence) {
        painter.clear();
        return;
      }
      const nodes = safe(() => source.nodes(page));
      if (!nodes) {
        // Camada de texto ainda por desenhar: leva-se a pagina ao ecra e o
        // realce chega com pageReady().
        painter.clear();
        if (scroll) safe(() => source.showPage(page));
        return;
      }
      const segments = [];
      for (const seg of segmentsFor(model, sentence.start, sentence.end)) {
        const node = nodes[seg.unit];
        if (node) segments.push({ node, from: seg.from, to: seg.to });
      }
      painter.paint(segments, scroll);
    }

    function halt() {
      token += 1;
      silence();
      state = 'idle';
      model = null;
      painter.clear();
      emit();
    }

    function finish() {
      const said = spokeAny;
      halt();
      notify(said ? END_OF_TEXT : NOTHING_TO_READ);
    }

    function speakNow() {
      const sentence = model && model.sentences[index];
      if (!sentence) {
        advancePage();
        return;
      }
      if (model.lang) heard = model.lang;
      const chosen = voice();
      if (!chosen) {
        halt();
        notify(voices().length ? NO_OFFLINE_VOICE : NO_VOICE);
        return;
      }
      const u = new Utterance(spokenText(model, sentence));
      u.voice = chosen;
      u.lang = chosen.lang || language();
      u.rate = rate;
      u.onend = () => finished(u);
      u.onerror = (event) => failed(u, event);
      u.onstart = () => {
        if (u === current) quiet = 0;
      };
      u.onboundary = u.onstart;
      current = u;
      spokeAny = true;
      state = 'speaking';
      paint(true);
      emit();
      try {
        if (speech.paused) speech.resume();
        speech.speak(u);
      } catch (e) {
        failed(u, { error: 'speak' });
        return;
      }
      armWatchdog();
      if (index + 2 >= model.sentences.length && page + 1 < pageCount()) loadModel(page + 1);
    }

    function finished(u) {
      if (u !== current || state !== 'speaking') return;
      current = null;
      stopWatchdog();
      index += 1;
      if (model && index < model.sentences.length) speakNow();
      else advancePage();
    }

    function failed(u, event) {
      // Um cancel() nosso chega aqui como 'interrupted' da utterance antiga:
      // ja nao e a viva e nao mexe em nada.
      if (u !== current) return;
      const code = String((event && event.error) || 'erro');
      const chosen = voice();
      halt();
      if (code === 'interrupted' || code === 'canceled') return;
      notify(
        'A voz parou com um erro (' + code + ').' +
          (chosen && !isLocal(chosen) ? '\nA voz online precisa de internet: escolha uma voz offline (Windows).' : '')
      );
    }

    function advancePage() {
      page += 1;
      index = 0;
      model = null;
      run(++token, null, true);
    }

    // Carrega a pagina `page` (e as seguintes, se vierem vazias) e fala -- ou,
    // em pausa, so poe o realce -- na frase `index`, ou na que contem `at`.
    async function run(my, at, speak) {
      if (speak) {
        state = 'loading';
        emit();
      }
      let first = true;
      for (;;) {
        if (page >= pageCount()) {
          if (my === token) finish();
          return;
        }
        const m = await loadModel(page);
        if (my !== token) return;
        let i = index;
        if (first && at) {
          const base = m.starts[at.unit];
          i = sentenceAt(m, (typeof base === 'number' ? base : 0) + (at.offset || 0));
        }
        first = false;
        if (i < m.sentences.length) {
          model = m;
          index = i;
          if (speak) {
            speakNow();
          } else {
            paint(true);
            emit();
          }
          return;
        }
        page += 1;
        index = 0;
      }
    }

    async function start() {
      const my = ++token;
      silence();
      painter.clear();
      model = null;
      spokeAny = false;
      state = 'loading';
      emit();
      await waitForVoices();
      if (my !== token) return;
      if (!voice()) {
        state = 'idle';
        emit();
        notify(voices().length ? NO_OFFLINE_VOICE : NO_VOICE);
        return;
      }
      const count = pageCount();
      const picked = safe(() => (source.selection ? source.selection() : null));
      if (picked && picked.page >= 0 && picked.page < count) {
        page = picked.page;
        index = 0;
        await run(my, { unit: picked.unit, offset: picked.offset }, true);
        return;
      }
      const here = Number(safe(() => source.currentPage())) || 0;
      page = Math.max(0, Math.min(count - 1, here));
      index = 0;
      const unit = safe(() => (source.firstVisibleUnit ? source.firstVisibleUnit(page) : null));
      await run(my, typeof unit === 'number' && unit > 0 ? { unit, offset: 0 } : null, true);
    }

    function pause() {
      if (state !== 'speaking' && state !== 'loading') return;
      // Pausa = cancelar e, ao continuar, repetir a frase. O pause() do
      // Chromium nao retoma em todas as vozes e, pausado muito tempo, perde a
      // utterance sem 'end'.
      token += 1;
      silence();
      state = 'paused';
      emit();
    }

    function resume() {
      if (state !== 'paused') return;
      run(++token, null, true);
    }

    function playPause() {
      if (state === 'idle') start();
      else if (state === 'paused') resume();
      else pause();
    }

    function stop() {
      if (state !== 'idle') halt();
    }

    function settle(speak) {
      if (speak) {
        speakNow();
      } else {
        paint(true);
        emit();
      }
    }

    async function back(my, speak) {
      for (let p = page - 1; p >= 0; p--) {
        const m = await loadModel(p);
        if (my !== token) return;
        if (m.sentences.length) {
          page = p;
          model = m;
          index = m.sentences.length - 1;
          settle(speak);
          return;
        }
      }
      if (my === token) {
        index = 0;
        settle(speak);
      }
    }

    function step(direction) {
      if ((state !== 'speaking' && state !== 'paused') || !model) return;
      const speak = state === 'speaking';
      const my = ++token;
      silence();
      if (direction > 0) {
        if (index + 1 < model.sentences.length) {
          index += 1;
          settle(speak);
          return;
        }
        page += 1;
        index = 0;
        model = null;
        run(my, null, speak);
        return;
      }
      if (index > 0) {
        index -= 1;
        settle(speak);
        return;
      }
      back(my, speak);
    }

    // Velocidade e voz valem ja: a frase em curso recomeca com elas.
    function restart() {
      if (state === 'speaking') {
        token += 1;
        silence();
        speakNow();
      } else {
        emit();
      }
    }

    function setRate(value) {
      rate = clampRate(value);
      save();
      restart();
    }

    function setVoice(uri) {
      if (!uri) return;
      byLang[primaryOf(language())] = String(uri);
      save();
      restart();
    }

    function pageReady(p) {
      if (state !== 'idle' && model && p === page) paint(true);
    }

    return {
      start,
      stop,
      pause,
      resume,
      playPause,
      next: () => step(1),
      prev: () => step(-1),
      setRate,
      setVoice,
      pageReady,
      voices,
      voice,
      language,
      state: () => state,
      rate: () => rate,
      snapshot,
      onChange: (fn) => changeListeners.push(fn),
      onVoices: (fn) => voiceListeners.push(fn),
    };
  }

  // ---------------------------------------------------------------- fontes

  // PDF.js: uma pagina e a lista de itens do getTextContent(); os nos sao os
  // textDivs da TextLayer. A TextLayer so cria um textDiv por item com `str`
  // -- os marcadores de conteudo marcado (beginMarkedContent/endMarkedContent,
  // sem `str`) viram contentores, nao textDivs. Saltam-se aqui pela mesma
  // regra, senao cada marcador desalinhava o realce uma unidade.
  function createPdfSource(hooks) {
    const win = hooks.window || window;
    return {
      hardBreaks: false,
      pageCount: () => hooks.pageCount(),
      currentPage: () => hooks.currentPage(),
      pageUnits: (p) =>
        Promise.resolve(hooks.textContent(p)).then((items) =>
          Array.prototype.filter
            .call(items || [], (item) => !!item && typeof item.str === 'string')
            .map((item) => ({ text: item.str, eol: !!item.hasEOL }))
        ),
      nodes: (p) => {
        const divs = hooks.textDivs(p);
        return divs && divs.length ? divs : null;
      },
      showPage: (p) => hooks.showPage(p),
      selection: () =>
        selectionStart(win, (node) => {
          const el = node.nodeType === 3 ? node.parentNode : node;
          const p = el ? hooks.pageOf(el) : null;
          if (typeof p !== 'number' || !Number.isInteger(p)) return null;
          const divs = hooks.textDivs(p) || [];
          const unit = Array.prototype.indexOf.call(divs, el);
          return unit < 0 ? null : { page: p, unit, el };
        }),
    };
  }

  const READER_BLOCKS = new Set(['H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'P', 'LI', 'BLOCKQUOTE']);

  function readerBlocks(root, out) {
    const kids = (root && root.children) || [];
    for (let i = 0; i < kids.length; i++) {
      const child = kids[i];
      const tag = String(child.tagName || '').toUpperCase();
      if (READER_BLOCKS.has(tag)) out.push(child);
      else if (tag !== 'PRE' && tag !== 'CODE' && tag !== 'SVG') readerBlocks(child, out);
    }
    return out;
  }

  // Modo Leitura: uma so "pagina" cujos nos sao o titulo, o resumo e os
  // blocos do artigo; cada bloco fecha a sua frase. Codigo nao se le.
  function createReaderSource(win, shell) {
    const blocks = [];
    const kids = shell.children || [];
    for (let i = 0; i < kids.length; i++) {
      const tag = String(kids[i].tagName || '').toUpperCase();
      if (tag === 'HEADER' || tag === 'ARTICLE') readerBlocks(kids[i], blocks);
    }
    return {
      hardBreaks: true,
      pageCount: () => 1,
      currentPage: () => 0,
      pageUnits: () => blocks.map((el) => ({ text: el.textContent || '', eol: true })),
      nodes: () => blocks,
      showPage: () => {},
      firstVisibleUnit: () => {
        for (let i = 0; i < blocks.length; i++) {
          if (blocks[i].getBoundingClientRect().bottom > 0) return i;
        }
        return 0;
      },
      selection: () =>
        selectionStart(win, (node) => {
          const el = node.nodeType === 3 ? node.parentNode : node;
          for (let i = 0; i < blocks.length; i++) {
            if (blocks[i] === el || blocks[i].contains(el)) return { page: 0, unit: i, el: blocks[i] };
          }
          return null;
        }),
    };
  }

  // ---------------------------------------------------------------- interface

  const ICONS = {
    speaker:
      '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M4 9.5v5h3.6L12.5 18V6L7.6 9.5z" fill="currentColor"/><path d="M15.5 9a4 4 0 0 1 0 6M17.8 6.6a7.4 7.4 0 0 1 0 10.8" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>',
    play: '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M8 5.5v13l10.5-6.5z" fill="currentColor"/></svg>',
    pause:
      '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M7 5.5h3.6v13H7zM13.4 5.5H17v13h-3.6z" fill="currentColor"/></svg>',
    stop: '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor"/></svg>',
    prev: '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M6 5.5h2.2v13H6zM19 5.5v13L9.5 12z" fill="currentColor"/></svg>',
    next: '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M15.8 5.5H18v13h-2.2zM5 5.5v13L14.5 12z" fill="currentColor"/></svg>',
    close:
      '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M6.5 6.5l11 11M17.5 6.5l-11 11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>',
  };

  // As dicas sao mensagens no meio da tela, letra grande, com as cores do
  // tema -- o padrao das dicas nativas do NeuralIA (HINT_FONT_PX = 20).
  const STYLE = [
    ':root{--neuralia-ra-surface:#1d2023;--neuralia-ra-fg:#e9ecef;--neuralia-ra-line:rgba(255,255,255,.14);--neuralia-ra-accent:#8ab4f8;--neuralia-ra-on-accent:#111314}',
    '@media (prefers-color-scheme: light){:root{--neuralia-ra-surface:#ffffff;--neuralia-ra-fg:#17191b;--neuralia-ra-line:rgba(0,0,0,.12);--neuralia-ra-accent:#1a73e8;--neuralia-ra-on-accent:#ffffff}}',
    '#neuralia-ra-toggle{position:fixed;top:12px;right:16px;z-index:2147483000;width:38px;height:38px;display:grid;place-items:center;padding:0;border:1px solid var(--neuralia-ra-line);border-radius:999px;background:var(--neuralia-ra-surface);color:var(--neuralia-ra-fg);box-shadow:0 6px 24px rgba(0,0,0,.3);cursor:pointer}',
    // A barra de procura do Ctrl+F (#neuralia-find, do mapa de teclas) nasce
    // no mesmo canto: com ela aberta o botao desce para debaixo dela.
    ':root:has(#neuralia-find) #neuralia-ra-toggle{top:60px}',
    '#neuralia-ra-toggle[aria-pressed="true"]{background:var(--neuralia-ra-accent);color:var(--neuralia-ra-on-accent);border-color:transparent}',
    '#neuralia-ra-bar{position:fixed;left:50%;bottom:18px;transform:translateX(-50%);z-index:2147483000;display:flex;align-items:center;gap:4px;padding:6px 8px;max-width:calc(100vw - 32px);border:1px solid var(--neuralia-ra-line);border-radius:999px;background:var(--neuralia-ra-surface);color:var(--neuralia-ra-fg);box-shadow:0 10px 32px rgba(0,0,0,.35);font:600 13px "Segoe UI",system-ui,sans-serif}',
    '#neuralia-ra-bar[hidden],#neuralia-ra-hint[hidden]{display:none}',
    '.neuralia-ra-btn{width:34px;height:34px;flex:none;display:inline-grid;place-items:center;padding:0;border:0;border-radius:999px;background:transparent;color:inherit;cursor:pointer}',
    '.neuralia-ra-btn:hover{background:var(--neuralia-ra-line)}',
    '.neuralia-ra-btn:disabled{opacity:.4;cursor:default;background:transparent}',
    '#neuralia-ra-play{background:var(--neuralia-ra-accent);color:var(--neuralia-ra-on-accent)}',
    '#neuralia-ra-close:hover{background:#e81123;color:#fff}',
    '#neuralia-ra-bar select{height:30px;min-width:0;max-width:260px;padding:0 8px;border:1px solid var(--neuralia-ra-line);border-radius:999px;background:var(--neuralia-ra-surface);color:inherit;font:inherit}',
    '#neuralia-ra-status{padding:0 6px;font-weight:500;opacity:.75;white-space:nowrap}',
    '#neuralia-ra-status:empty{display:none}',
    '#neuralia-ra-hint{position:fixed;left:50%;top:50%;transform:translate(-50%,-50%);z-index:2147483647;max-width:min(640px,calc(100vw - 48px));padding:14px 24px;border:1px solid var(--neuralia-ra-line);border-radius:24px;background:var(--neuralia-ra-surface);color:var(--neuralia-ra-fg);box-shadow:0 12px 40px rgba(0,0,0,.35);font:400 20px/1.35 "Segoe UI",system-ui,sans-serif;text-align:center;white-space:pre-line;pointer-events:none}',
    '::highlight(' + HIGHLIGHT_NAME + '){background-color:rgba(255,196,0,.45)}',
    '.' + HIGHLIGHT_CLASS + '{background-color:rgba(255,196,0,.45);border-radius:2px}',
  ].join('\n');

  function formatRate(rate) {
    return String(rate).replace('.', ',') + '×';
  }

  function createUi(doc) {
    const handlers = {};
    let hintTimer = 0;
    let noticeTimer = 0;

    function make(tag, id) {
      const el = doc.createElement(tag);
      if (id) el.id = id;
      return el;
    }

    const style = make('style', 'neuralia-ra-style');
    style.textContent = STYLE;
    (doc.head || doc.documentElement).appendChild(style);

    const hint = make('div', 'neuralia-ra-hint');
    hint.setAttribute('role', 'status');
    hint.hidden = true;

    function showHint(text, ms) {
      clearTimeout(noticeTimer);
      noticeTimer = 0;
      hint.textContent = text;
      hint.hidden = false;
      if (ms) {
        noticeTimer = setTimeout(() => {
          noticeTimer = 0;
          hint.hidden = true;
        }, ms);
      }
    }

    function hideHint() {
      clearTimeout(hintTimer);
      hintTimer = 0;
      if (!noticeTimer) hint.hidden = true;
    }

    function withHint(el, text) {
      el.setAttribute('aria-label', text);
      el.addEventListener('pointerenter', () => {
        clearTimeout(hintTimer);
        hintTimer = setTimeout(() => {
          hintTimer = 0;
          if (!noticeTimer) showHint(el.getAttribute('aria-label') || text, 0);
        }, HINT_DELAY_MS);
      });
      el.addEventListener('pointerleave', hideHint);
      el.addEventListener('pointerdown', hideHint);
      return el;
    }

    function button(id, icon, label, action) {
      const el = make('button', id);
      el.type = 'button';
      el.className = 'neuralia-ra-btn';
      el.innerHTML = icon;
      withHint(el, label);
      el.addEventListener('click', (event) => {
        if (event && event.isTrusted === false) return;
        const fn = handlers[action];
        if (fn) fn();
      });
      return el;
    }

    const toggle = button('neuralia-ra-toggle', ICONS.speaker, 'Ler em voz alta (Ctrl+Shift+U)', 'toggle');
    toggle.className = '';
    toggle.setAttribute('aria-pressed', 'false');

    const bar = make('div', 'neuralia-ra-bar');
    bar.setAttribute('role', 'toolbar');
    bar.setAttribute('aria-label', 'Leitura em voz alta');
    bar.hidden = true;

    const prev = button('neuralia-ra-prev', ICONS.prev, 'Frase anterior', 'prev');
    const play = button('neuralia-ra-play', ICONS.play, 'Ler', 'play');
    const stop = button('neuralia-ra-stop', ICONS.stop, 'Parar', 'stop');
    const next = button('neuralia-ra-next', ICONS.next, 'Próxima frase', 'next');

    const rate = withHint(make('select', 'neuralia-ra-rate'), 'Velocidade da leitura');
    for (const value of RATES) {
      const option = make('option');
      option.value = String(value);
      option.textContent = formatRate(value);
      rate.appendChild(option);
    }
    rate.addEventListener('change', () => {
      if (handlers.rate) handlers.rate(rate.value);
    });

    const voice = withHint(
      make('select', 'neuralia-ra-voice'),
      'Voz. Offline (Windows): o texto fica no computador.\nOnline: precisa de internet e o texto vai para o provedor da voz.'
    );
    voice.addEventListener('change', () => {
      if (handlers.voice && voice.value) handlers.voice(voice.value);
    });

    const status = make('span', 'neuralia-ra-status');
    const close = button('neuralia-ra-close', ICONS.close, 'Fechar a leitura (Esc)', 'close');

    for (const el of [prev, play, stop, next, rate, voice, status, close]) bar.appendChild(el);
    const root = doc.body || doc.documentElement;
    root.appendChild(toggle);
    root.appendChild(bar);
    root.appendChild(hint);

    function fillVoices(list, chosen, lang) {
      voice.textContent = '';
      if (!list.length) {
        const none = make('option');
        none.value = '';
        none.textContent = 'Nenhuma voz encontrada';
        none.disabled = true;
        voice.appendChild(none);
        voice.value = '';
        return;
      }
      if (!chosen) {
        const pick = make('option');
        pick.value = '';
        pick.textContent = 'Escolha uma voz…';
        voice.appendChild(pick);
      }
      const groups = groupVoices(list, lang);
      for (const [label, members] of [
        [OFFLINE_GROUP, groups.offline],
        [ONLINE_GROUP, groups.online],
      ]) {
        if (!members.length && label === ONLINE_GROUP) {
          // O WebView2 deste Windows nao expoe voz online nenhuma: diz-se
          // por extenso, em vez de o grupo simplesmente nao existir.
          const group = make('optgroup');
          group.label = label;
          group.setAttribute('label', label);
          const none = make('option');
          none.value = '';
          none.disabled = true;
          none.textContent = NO_ONLINE_VOICE;
          group.appendChild(none);
          voice.appendChild(group);
          continue;
        }
        if (!members.length) continue;
        const group = make('optgroup');
        group.label = label;
        group.setAttribute('label', label);
        for (const v of members) {
          const option = make('option');
          option.value = v.voiceURI;
          option.textContent = v.name + ' (' + v.lang + ')';
          group.appendChild(option);
        }
        voice.appendChild(group);
      }
      voice.value = chosen ? chosen.voiceURI : '';
    }

    function update(snap) {
      const busy = snap.state !== 'idle';
      const speaking = snap.state === 'speaking' || snap.state === 'loading';
      bar.setAttribute('data-state', snap.state);
      toggle.setAttribute('aria-pressed', busy ? 'true' : 'false');
      play.innerHTML = speaking ? ICONS.pause : ICONS.play;
      play.setAttribute('aria-label', speaking ? 'Pausar' : snap.state === 'paused' ? 'Continuar' : 'Ler');
      prev.disabled = !busy;
      next.disabled = !busy;
      stop.disabled = !busy;
      rate.value = String(snap.rate);
      let text = '';
      if (snap.state === 'loading') text = 'Carregando…';
      else if (busy && snap.pages > 1) text = 'Página ' + (snap.page + 1) + ' de ' + snap.pages;
      if (busy && snap.voice && !isLocal(snap.voice)) text += (text ? ' · ' : '') + 'voz online';
      status.textContent = text;
    }

    return {
      on: (name, fn) => {
        handlers[name] = fn;
      },
      show: () => {
        bar.hidden = false;
      },
      hide: () => {
        bar.hidden = true;
      },
      shown: () => !bar.hidden,
      notify: (text) => showHint(text, NOTICE_MS),
      fillVoices,
      update,
    };
  }

  // ---------------------------------------------------------------- ligacao

  let active = null;

  function attach(source, options) {
    const win = window;
    const doc = document;
    // So o idioma que o DOCUMENTO declara. O lang do <html> e o da pagina
    // do NeuralIA (pt), nao o do artigo nem o do PDF: sem declaracao, o
    // motor ouve o texto (`guessLang`).
    const lang = (options && options.lang) || '';
    const ui = createUi(doc);
    const speech = win.speechSynthesis;
    const Utterance = win.SpeechSynthesisUtterance;
    const engine =
      speech && typeof Utterance === 'function'
        ? createEngine({
            source,
            speech,
            Utterance,
            lang,
            prefs: prefsStore(win),
            painter: createPainter(win, doc),
            notify: ui.notify,
          })
        : null;

    function refreshVoices() {
      if (engine) ui.fillVoices(engine.voices(), engine.voice(), engine.language());
    }

    function busy() {
      return (engine && engine.state() !== 'idle') || ui.shown();
    }

    function open() {
      ui.show();
      if (!engine) {
        ui.notify(NO_SPEECH);
        return;
      }
      refreshVoices();
      engine.start();
    }

    function close() {
      if (engine) engine.stop();
      ui.hide();
    }

    function toggle() {
      if (engine && engine.state() !== 'idle') close();
      else open();
    }

    ui.on('toggle', toggle);
    ui.on('close', close);
    if (engine) {
      ui.on('play', () => engine.playPause());
      ui.on('stop', () => engine.stop());
      ui.on('prev', () => engine.prev());
      ui.on('next', () => engine.next());
      ui.on('rate', (value) => engine.setRate(value));
      ui.on('voice', (uri) => engine.setVoice(uri));
      engine.onChange((snap) => ui.update(snap));
      engine.onVoices(refreshVoices);
      ui.update(engine.snapshot());
      refreshVoices();
    }

    const controller = {
      toggle,
      open,
      close,
      busy,
      pageReady: (p) => {
        if (engine) engine.pageReady(p);
      },
      engine,
    };
    active = controller;
    return controller;
  }

  function attachPdf(hooks) {
    return attach(createPdfSource(hooks), { lang: hooks.lang });
  }

  // Um campo de texto que nao e da leitura (a barra de procura do Ctrl+F, por
  // exemplo) fica com o seu Esc.
  function foreignField(target) {
    if (!target || target.nodeType !== 1) return false;
    const tag = String(target.tagName || '').toUpperCase();
    const editable = tag === 'INPUT' || tag === 'TEXTAREA' || target.isContentEditable === true;
    if (!editable) return false;
    const bar = document.getElementById('neuralia-ra-bar');
    return !(bar && bar.contains(target));
  }

  // Ctrl+Shift+U (o atalho do Edge) liga e desliga; Esc para a leitura. Na
  // fase de captura da janela, antes do mapa de teclas do NeuralIA (captura no
  // document): sem isto o Ctrl+Shift+U seria o Ctrl+U de ver o codigo e o Esc
  // da leitura fechava o documento.
  window.addEventListener(
    'keydown',
    (event) => {
      if (!event.isTrusted) return;
      const key = String(event.key || '').toLowerCase();
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && !event.altKey && key === 'u') {
        event.preventDefault();
        event.stopPropagation();
        if (active) active.toggle();
        return;
      }
      if (key === 'escape' && active && active.busy() && !foreignField(event.target)) {
        event.preventDefault();
        event.stopPropagation();
        active.close();
      }
    },
    true
  );

  // O Modo Leitura liga-se sozinho; o visualizador de PDF chama attachPdf()
  // quando o documento abre.
  document.addEventListener('DOMContentLoaded', () => {
    if (active) return;
    const shell = document.getElementById('neural-shell');
    if (!shell || !shell.classList || !shell.classList.contains('reader')) return;
    attach(createReaderSource(window, shell), {});
  });

  window.NeuralIAReadAloud = Object.freeze({
    attach,
    attachPdf,
    createPdfSource,
    createReaderSource,
    segmentSentences,
    buildPageText,
    pageModel,
    spokenText,
    segmentsFor,
    chooseVoice,
    groupVoices,
    guessLang,
    RATES,
    MAX_UTTERANCE,
    OFFLINE_GROUP,
    ONLINE_GROUP,
    NO_ONLINE_VOICE,
  });
})();
