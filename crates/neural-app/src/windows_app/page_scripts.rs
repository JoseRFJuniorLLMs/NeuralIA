/// Avanca uma pagina, parando no fim em vez de dar a volta. Usa a altura visivel
/// menos uma faixa de sobreposicao, para nao se perder a linha que se estava a
/// ler. Corre no documento e tambem nos frames a que conseguimos chegar.
pub(in crate::windows_app) const AUTO_SCROLL_SCRIPT: &str = r#"
(function () {
  // O nosso visualizador de PDF sabe avancar uma pagina inteira.
  if (typeof window.__neuralia_next_page === 'function') {
    window.__neuralia_next_page();
    return;
  }

  function scrollRoot(doc) {
    var root = doc.scrollingElement || doc.documentElement || doc.body;
    var best = root;
    var bestRange = best ? Math.max(0, best.scrollHeight - best.clientHeight) : 0;
    var candidates = doc.querySelectorAll(
      'main,[role="main"],[data-radix-scroll-area-viewport],'
      + '[data-testid*="scroll"],[class*="overflow"],[class*="scroll"]'
    );

    for (var i = 0; i < candidates.length; i++) {
      var el = candidates[i];
      if (!el || el === doc.body || el === doc.documentElement) continue;
      var range = Math.max(0, el.scrollHeight - el.clientHeight);
      if (range <= bestRange + 24) continue;
      var css = doc.defaultView.getComputedStyle(el);
      if (css.display === 'none' || css.visibility === 'hidden' || css.overflowY === 'hidden') {
        continue;
      }
      best = el;
      bestRange = range;
    }
    return best;
  }

  function step(win) {
    try {
      var doc = win.document;
      var el = scrollRoot(doc);
      if (!el) return false;

      var docLike = el === doc.scrollingElement
        || el === doc.documentElement || el === doc.body;
      var view = el.clientHeight || win.innerHeight || 0;
      var top = docLike ? win.scrollY : el.scrollTop;
      var max = Math.max(0, el.scrollHeight - el.clientHeight);
      if (view <= 0 || max <= 4 || top >= max - 2) return false;

      var amount = Math.max(view - 72, 120);
      if (docLike) {
        win.scrollBy({ top: amount, left: 0, behavior: 'smooth' });
      } else {
        el.scrollBy({ top: amount, left: 0, behavior: 'smooth' });
      }
      return true;
    } catch (err) {
      return false;
    }
  }

  if (step(window)) return;

  // Alguns leitores desenham o conteudo dentro de um frame proprio.
  var frames = document.querySelectorAll('iframe, frame');
  for (var i = 0; i < frames.length; i++) {
    try {
      if (frames[i].contentWindow && step(frames[i].contentWindow)) return;
    } catch (err) { /* outra origem: nao ha nada a fazer daqui */ }
  }
})();
"#;

/// Corre no painel do Ctrl+H quando ele foi aberto pelo botao Notas. A
/// guarda deixa-o inofensivo enquanto o `PANEL_HTML` nao tiver a secao.
pub(in crate::windows_app) const PANEL_SHOW_NOTES_SCRIPT: &str =
    "window.neuraliaShowSection && window.neuraliaShowSection('notes')";

/// O botao Notas com o painel ja aberto: nas Notas fecha (pelo mesmo
/// caminho do X, que salva o editor antes), no Historico mostra as Notas.
pub(in crate::windows_app) const PANEL_NOTES_BUTTON_SCRIPT: &str =
    "window.__neuraliaNotes && window.__neuraliaNotes.button()";

/// Corre no painel quando o Ctrl+Shift+Z e dado na Home (omnibox) ou com o
/// teclado na barra: uma nota nova, em branco, no editor. So chega ao disco
/// quando se salva -- um atalho nao enche a pasta de ficheiros vazios.
pub(in crate::windows_app) const PANEL_NEW_NOTE_SCRIPT: &str =
    "window.__neuraliaNotes && window.__neuraliaNotes.newNote()";

/// Corre na WebView que pediu a nota -- nunca numa privada. O que devolve e
/// dado da pagina (ela pode ter trocado o `getSelection`), nao uma ordem:
/// `note_draft_from_capture` volta a cortar e a validar tudo.
pub(in crate::windows_app) const NOTE_CAPTURE_SCRIPT: &str = r#"(function () {
  var text = '';
  try { text = String(window.getSelection ? window.getSelection() : ''); } catch (e) {}
  return { text: text.slice(0, 20000), url: String(location.href), title: String(document.title || '') };
})()"#;

/// Visualizador de PDF proprio: o do Edge corre noutro processo e nao aceita
/// nem script nem teclado nosso; este e uma pagina nossa, com o PDF.js da
/// Mozilla (Apache-2.0, assets/pdfjs/LICENSE) a desenhar as paginas em canvas.
pub(in crate::windows_app) const PDF_VIEWER_HTML: &[u8] =
    include_bytes!("../../../../assets/pdfjs/viewer.html");

/// Mapa de teclas injetado em TODAS as paginas. O teclado pertence ao WebView2,
/// que e uma janela filha: a janela nativa nunca ve a tecla, por isso e aqui,
/// na fase de captura, que se apanham os atalhos antes de o site os consumir.
pub(in crate::windows_app) const NEURALIA_KEYMAP_SCRIPT: &str = r#"
(function () {
  // WRY/WebView2 injeta initialization scripts em child frames no Windows.
  // Capability e controles nativos pertencem somente ao documento principal.
  if (window.top !== window) return;
  if (window.__neuralia_keymap) { return; }
  window.__neuralia_keymap = true;

  // Capturas no document-created, antes de a pagina correr: o que os atalhos
  // usam mais tarde com o token nao pode ser um global ja envenenado.
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
  const colIndex = window.__neuralia_col_index;
  function act(action, args) {
    post(envelope(action, args));
  }

  function findBar() {
    var id = 'neuralia-find';
    var box = document.getElementById(id);
    if (box) { box.querySelector('input').focus(); box.querySelector('input').select(); return; }

    box = document.createElement('div');
    box.id = id;
    box.setAttribute('style', [
      'position:fixed', 'top:12px', 'right:14px', 'z-index:2147483647',
      'display:flex', 'align-items:center', 'gap:8px',
      'padding:8px 12px', 'border-radius:999px',
      'background:rgba(17,19,20,.96)', 'box-shadow:0 8px 28px rgba(0,0,0,.4)',
      'font:600 13px Segoe UI, system-ui, sans-serif'
    ].join(';'));

    var input = document.createElement('input');
    input.type = 'text';
    input.placeholder = 'Procurar na pagina';
    input.setAttribute('style', [
      'border:0', 'outline:0', 'background:transparent', 'color:#fff',
      'font:inherit', 'width:190px'
    ].join(';'));

    var close = document.createElement('span');
    close.textContent = '\u2715';
    close.setAttribute('style', 'color:#9aa1a8;cursor:pointer');
    close.onclick = function () { box.remove(); };

    input.addEventListener('keydown', function (e) {
      e.stopPropagation();
      if (e.key === 'Enter') {
        e.preventDefault();
        try { window.find(input.value, false, e.shiftKey, true); } catch (err) {}
      } else if (e.key === 'Escape') {
        e.preventDefault();
        box.remove();
      }
    }, true);

    box.appendChild(input);
    box.appendChild(close);
    document.documentElement.appendChild(box);
    input.focus();
  }

  // Barra de selecao (pedido do dono: "quando eu selecionar um texto, tem
  // que aparecer a pergunta mandar para pesquisa ? ou copiar ? ou falar ?
  // 3 botoes"; na 2.2.0: "mandar para ia ?, salva no zetelkast, traduzir ?,
  // copiar ?" -- quatro botoes e um "⋯" com o resto). Vive neste script
  // porque e o que todas as paginas recebem. Se faltar uma primitiva, a
  // barra fica desligada e os atalhos seguem.
  let selectionBar = null;
  try { selectionBar = createSelectionBar(); } catch (err) { selectionBar = null; }

  function createSelectionBar() {
    // Capturas no document-created, antes de a pagina correr: trocar depois
    // getSelection, Selection/Range/Event.prototype, EventTarget, String ou
    // String.prototype nao desliga a barra, nao lhe muda o texto e nao da a
    // pagina um `this` dentro da shadow root fechada.
    function uncurry(fn) {
      if (typeof fn !== 'function') throw new TypeError('primitiva em falta');
      return Function.prototype.call.bind(fn);
    }
    function getterOf(proto, name) {
      const found = Object.getOwnPropertyDescriptor(proto, name);
      return uncurry(found && found.get);
    }
    function setterOf(proto, name) {
      const found = Object.getOwnPropertyDescriptor(proto, name);
      return uncurry(found && found.set);
    }
    const listen = uncurry(EventTarget.prototype.addEventListener);
    const setAttr = uncurry(Element.prototype.setAttribute);
    const setText = setterOf(Node.prototype, 'textContent');
    const setStyle = uncurry(CSSStyleDeclaration.prototype.setProperty);
    const readStyle = uncurry(CSSStyleDeclaration.prototype.getPropertyValue);
    const computed = uncurry(getComputedStyle);
    const connected = getterOf(Node.prototype, 'isConnected');
    const later = setTimeout;
    const cancelLater = clearTimeout;
    const clock = Date.now;
    const round = Math.round;
    const abs = Math.abs;
    const thenOf = uncurry(Promise.prototype.then);
    // Acrescentar a uma lista sem [[Set]]: o `push` escrevia pelo
    // Array.prototype, e um acessor da pagina no indice 0 engolia a voz local
    // e devolvia a online (ou uma frase dela) a quem lia a lista.
    const defineOwn = Object.defineProperty;
    function pushTo(list, value) {
      defineOwn(list, list.length, {
        __proto__: null, value: value, writable: true, enumerable: true, configurable: true
      });
    }
    const toStr = String;
    const fromCode = String.fromCharCode;
    const codeAt = uncurry(String.prototype.charCodeAt);
    const sliceOf = uncurry(String.prototype.slice);
    const findIn = uncurry(String.prototype.indexOf);
    const findLast = uncurry(String.prototype.lastIndexOf);
    const lower = uncurry(String.prototype.toLowerCase);
    const upper = uncurry(String.prototype.toUpperCase);
    const docSelection = uncurry(Document.prototype.getSelection);
    const selText = uncurry(Selection.prototype.toString);
    const selRange = uncurry(Selection.prototype.getRangeAt);
    const selCount = getterOf(Selection.prototype, 'rangeCount');
    const selCollapsed = getterOf(Selection.prototype, 'isCollapsed');
    const selAnchor = getterOf(Selection.prototype, 'anchorNode');
    const selFocus = getterOf(Selection.prototype, 'focusNode');
    const rangeRects = uncurry(Range.prototype.getClientRects);
    const rangeBox = uncurry(Range.prototype.getBoundingClientRect);
    const rangeCommon = getterOf(Range.prototype, 'commonAncestorContainer');
    const makeElement = uncurry(Document.prototype.createElement);
    const appendTo = uncurry(Node.prototype.appendChild);
    const holds = uncurry(Node.prototype.contains);
    const closestOf = uncurry(Element.prototype.closest);
    const shadowOf = uncurry(Element.prototype.attachShadow);
    const boxOf = uncurry(Element.prototype.getBoundingClientRect);
    // Onde a barra esta: a raiz do documento e o pai do host. A pagina que
    // muda de sitio o host (esta na arvore dela) ou mente sobre a raiz nao
    // o esconde do teste do clique.
    const rootOf = getterOf(Document.prototype, 'documentElement');
    const parentOf = getterOf(Node.prototype, 'parentNode');
    // As medidas tambem: um getter de DOMRectReadOnly da pagina dizia que a
    // barra continuava onde foi posta depois de ela a mover.
    const rectTop = getterOf(DOMRectReadOnly.prototype, 'top');
    const rectLeft = getterOf(DOMRectReadOnly.prototype, 'left');
    const rectRight = getterOf(DOMRectReadOnly.prototype, 'right');
    const rectBottom = getterOf(DOMRectReadOnly.prototype, 'bottom');
    const rectWidth = getterOf(DOMRectReadOnly.prototype, 'width');
    const rectHeight = getterOf(DOMRectReadOnly.prototype, 'height');
    // Os eventos tambem: a pagina que redefine um acessor de Event recebia o
    // evento de um botao da barra como `this`, com os nos de dentro no
    // composedPath; e um `detail` falso fazia de um clique um gesto.
    const targetOf = getterOf(Event.prototype, 'target');
    const prevent = uncurry(Event.prototype.preventDefault);
    const stopHere = uncurry(Event.prototype.stopPropagation);
    const stopAll = uncurry(Event.prototype.stopImmediatePropagation);
    const detailOf = getterOf(UIEvent.prototype, 'detail');
    const buttonOf = getterOf(MouseEvent.prototype, 'button');
    const xOf = getterOf(MouseEvent.prototype, 'clientX');
    const yOf = getterOf(MouseEvent.prototype, 'clientY');
    const mouseShift = getterOf(MouseEvent.prototype, 'shiftKey');
    const keyOf = getterOf(KeyboardEvent.prototype, 'key');
    const keyShift = getterOf(KeyboardEvent.prototype, 'shiftKey');
    const keyCtrl = getterOf(KeyboardEvent.prototype, 'ctrlKey');
    const keyMeta = getterOf(KeyboardEvent.prototype, 'metaKey');
    // O foco do menu "⋯" (teclado): pelos metodos capturados. Sem eles o
    // menu abre na mesma, so o teclado nao entra nele.
    const Html = typeof HTMLElement === 'function' ? HTMLElement : null;
    const focusOn = Html && typeof Html.prototype.focus === 'function'
      ? uncurry(Html.prototype.focus) : null;
    const blurOf = Html && typeof Html.prototype.blur === 'function'
      ? uncurry(Html.prototype.blur) : null;
    const execCommand = typeof Document.prototype.execCommand === 'function'
      ? uncurry(Document.prototype.execCommand) : null;
    const clip = typeof navigator !== 'undefined' ? navigator.clipboard : null;
    const writeText = clip && typeof Clipboard === 'function'
      && typeof Clipboard.prototype.writeText === 'function'
      ? uncurry(Clipboard.prototype.writeText) : null;
    const navLang = typeof navigator !== 'undefined' ? toStr(navigator.language || '') : '';
    const synth = window.speechSynthesis || null;
    const Utterance = window.SpeechSynthesisUtterance || null;
    // A voz tambem se le pelos acessores capturados aqui: a pagina que
    // redefine `localService`, `lang` ou `default` de SpeechSynthesisVoice,
    // ou o setter `voice` da fala, fazia passar uma voz online por local --
    // e o texto saia do dispositivo, mesmo no painel privado. Sem eles nao ha
    // Falar.
    let voiceApi = null;
    try {
      const Voice = typeof SpeechSynthesisVoice === 'function' ? SpeechSynthesisVoice : null;
      if (synth && Utterance && Voice) {
        voiceApi = {
          local: getterOf(Voice.prototype, 'localService'),
          lang: getterOf(Voice.prototype, 'lang'),
          isDefault: getterOf(Voice.prototype, 'default'),
          use: setterOf(Utterance.prototype, 'voice'),
          speakIn: setterOf(Utterance.prototype, 'lang')
        };
      }
    } catch (err) { voiceApi = null; }
    const speakNow = voiceApi ? uncurry(synth.speak) : null;
    const cancelSpeech = speakNow ? uncurry(synth.cancel) : null;
    const listVoices = speakNow ? uncurry(synth.getVoices) : null;
    const Sheet = typeof CSSStyleSheet === 'function' ? CSSStyleSheet : null;
    // IntersectionObserver v2: diz se a barra esta mesmo a vista, sem nada
    // da pagina por cima (mesmo com pointer-events:none), sem opacidade,
    // filtro ou transformacao. Filtra cliques enganados antes de incomodar o
    // utilizador; quem decide a pesquisa e o cartao nativo.
    const Watch = typeof IntersectionObserver === 'function'
      && typeof IntersectionObserverEntry === 'function'
      && Object.getOwnPropertyDescriptor(IntersectionObserverEntry.prototype, 'isVisible')
      ? IntersectionObserver : null;
    const seesIt = Watch ? getterOf(IntersectionObserverEntry.prototype, 'isVisible') : null;
    const watchOn = Watch ? uncurry(Watch.prototype.observe) : null;

    // Painel privado: o texto nunca sai para o comparador (historico e
    // memoria) -- nem Mandar para IA nem Traduzir. O nativo tambem recusa o
    // `search` destes WebViews. Salvar nota fica: e local e pedido por quem
    // le.
    const searchAllowed = '__NEURALIA_PRIVATE__' === 'false';
    const SHOW_MAX = 5000;
    const SEARCH_MAX = 2000;
    // Salvar nota manda o texto no pedido: ate 5000 caracteres
    // (`NOTE_TEXT_MAX_CHARS`) e so se o envelope inteiro couber nos 8 KiB do
    // canal (`IPC_MAX_BYTES`); o que nao cabe nao sai, e a barra diz porque.
    const NOTE_MAX = 5000;
    const IPC_MAX_BYTES = 8192;
    const SHOW_DELAY_MS = 200;
    // Mandar para IA e Traduzir so PEDEM: o nativo mostra o texto num cartao
    // seu ("Mandar para as 3 IAs?", "Traduzir nas 3 IAs?") e so um clique
    // nesse cartao o envia. A pagina pode encolher ou tornar transparente
    // esta barra de formas que daqui nao se veem; o cartao ela nao alcanca.
    // Antes de pedir -- e antes de Salvar nota -- a barra ainda exige estar
    // parada e a vista ha pelo menos isto.
    const ARM_MS = 500;
    // Menos do que isto entre o mousedown e o mouseup e um clique, nao um
    // arrasto: nao escolhe texto.
    const DRAG_MIN = 4;
    const FEEDBACK_MS = 1000;
    const VOICE_WAIT_MS = 1500;
    const SPEECH_CHUNK = 200;
    const SPEECH_ABBREVIATION = 5;
    const MARGIN = 8;
    const TOO_LONG = 'Seleção grande demais para as IAs (máx. 2000 caracteres)';
    const NOTE_TOO_LONG = 'Seleção grande demais para Salvar nota';
    // Clicado cedo demais: o nome do botao que foi.
    const TOO_SOON = {
      ask: 'Clique de novo em Mandar para IA',
      note: 'Clique de novo em Salvar nota',
      translate: 'Clique de novo em Traduzir'
    };
    const TAMPERED = 'A página cobriu ou alterou esta barra: o pedido não foi enviado';
    const LABELS = {
      ask: '\u{1F916} Mandar para IA',
      note: '\u{1F4DD} Salvar nota',
      translate: '\u{1F310} Traduzir',
      copy: '\u{1F4CB} Copiar',
      copied: '✓ Copiado',
      more: '\u{22EF}',
      speak: '\u{1F50A} Falar',
      stop: '⏹ Parar'
    };
    const CSS = [
      '.bar{position:relative;display:flex;flex-wrap:wrap;align-items:center;gap:4px;padding:4px;',
      'border-radius:22px;background:#ffffff;color:#111314;',
      'border:1px solid rgba(0,0,0,.14);box-shadow:0 8px 28px rgba(0,0,0,.22);',
      'font:600 13px "Segoe UI",system-ui,sans-serif;max-width:560px;',
      'user-select:none;-webkit-user-select:none;cursor:default}',
      'button{all:unset;box-sizing:border-box;display:inline-flex;align-items:center;',
      'gap:6px;min-height:34px;padding:0 12px;border-radius:999px;cursor:pointer;',
      'color:inherit;font:inherit;white-space:nowrap}',
      'button:hover{background:rgba(0,0,0,.08)}',
      'button:focus-visible{outline:2px solid #1a73e8;outline-offset:-2px}',
      '.more{min-width:34px;justify-content:center;padding:0 10px}',
      '.solo .act{display:none}',
      // O menu flutua junto do "⋯", fora da caixa da barra: abrir nao muda
      // o tamanho nem o sitio dela, e o "⋯" fica debaixo do rato.
      '.menu{display:none;position:absolute;right:0;min-width:150px;flex-direction:column;',
      'align-items:stretch;gap:2px;padding:4px;border-radius:16px;',
      'background:#ffffff;color:#111314;border:1px solid rgba(0,0,0,.14);',
      'box-shadow:0 8px 28px rgba(0,0,0,.22)}',
      '.menu.up{bottom:calc(100% + 6px)}',
      '.menu.down{top:calc(100% + 6px)}',
      '.menu.start{right:auto;left:0}',
      '.menu.on{display:flex}',
      '.menu button{border-radius:12px}',
      '.msg{display:none;flex-basis:100%;padding:6px 12px;font-weight:500;line-height:1.35}',
      '.msg.on{display:block}',
      '@media (prefers-color-scheme: dark){',
      '.bar,.menu{background:#1c1f22;color:#f1f3f4;border-color:rgba(255,255,255,.16);',
      'box-shadow:0 8px 28px rgba(0,0,0,.55)}',
      'button:hover{background:rgba(255,255,255,.12)}',
      'button:focus-visible{outline-color:#8ab4f8}}'
    ].join('');
    // O menu "⋯": o que nao cabe na barra, pela ordem em que aparece. Lista
    // fechada, e so com o que ja existe -- nunca um botao morto. O Explicar
    // (uma frase de explicacao, pelo modelo local ou pelo fornecedor) e o
    // Extrair (tabela da pagina, Web-to-Data) sao as proximas ondas da 2.2.0
    // e entram AQUI, como entradas desta lista, quando o caminho deles
    // existir. `ready` diz se a entrada tem o que precisa neste documento.
    const MORE_MENU = [
      { name: 'speak', label: LABELS.speak, run: toggleSpeech, ready: !!speakNow }
    ];

    let host = null;
    let bar = null;
    let note = null;
    const buttons = Object.create(null);
    // O "⋯", o menu que ele abre, as entradas (e o que cada uma faz, para o
    // teclado) e a que tem o foco.
    let moreButton = null;
    let menu = null;
    const menuItems = [];
    const menuRuns = [];
    let menuOpen = false;
    let menuAt = 0;
    let menuFocused = null;
    let visible = false;
    // O texto de Mandar, Salvar nota, Traduzir e Copiar: a selecao que a
    // barra mostra agora. Vazio enquanto se le algo que ja nao esta
    // selecionado.
    let text = '';
    let shownAt = null;
    let placed = null;
    let steadyAt = 0;
    let seenAt = 0;
    let watching = false;
    let pressReady = 'alterada';
    let downX = 0;
    let downY = 0;
    let gestureText = null;
    let keySelecting = false;
    let showTimer = 0;
    let feedbackTimer = 0;
    let voiceTimer = 0;
    let waitingVoices = null;
    let voicesHooked = false;
    let speaking = false;
    let speechRun = 0;
    // A fala em curso fica referenciada: o Chromium recolhe uma
    // SpeechSynthesisUtterance sem dono e o 'end' dela nunca chega.
    let speechHold = null;

    function guard(fn) {
      return function (event) {
        try { fn(event); } catch (err) {}
      };
    }

    function important(node, name, value) {
      setStyle(node.style, name, value, 'important');
    }

    function elementOf(node) {
      if (!node) return null;
      return node.nodeType === 1 ? node : node.parentElement || null;
    }

    function editable(node) {
      const el = elementOf(node);
      if (!el) return false;
      const tag = upper(toStr(el.tagName || ''));
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
      if (el.isContentEditable) return true;
      return !!closestOf(el,
        'input,textarea,select,[contenteditable=""],[contenteditable="true"],[contenteditable="plaintext-only"]');
    }

    function focusedDeep() {
      let el = document.activeElement;
      while (el && el.shadowRoot && el.shadowRoot.activeElement) {
        el = el.shadowRoot.activeElement;
      }
      return el;
    }

    function ours(node) {
      return !!host && !!node && (node === host || holds(host, node));
    }

    // Os espacos que o trim() do JavaScript tira, sem passar pelo trim() que
    // a pagina pode trocar.
    function blank(c) {
      return c === 32 || (c >= 9 && c <= 13) || c === 0xA0 || c === 0x1680
        || (c >= 0x2000 && c <= 0x200A) || c === 0x2028 || c === 0x2029
        || c === 0x202F || c === 0x205F || c === 0x3000 || c === 0xFEFF;
    }

    function trimmed(value) {
      let start = 0;
      let end = value.length;
      while (start < end && blank(codeAt(value, start))) start++;
      while (end > start && blank(codeAt(value, end - 1))) end--;
      return sliceOf(value, start, end);
    }

    function codePoints(value) {
      let count = 0;
      for (let i = 0; i < value.length; i++) {
        const high = codeAt(value, i);
        if (high >= 0xD800 && high <= 0xDBFF && i + 1 < value.length) {
          const low = codeAt(value, i + 1);
          if (low >= 0xDC00 && low <= 0xDFFF) i++;
        }
        count++;
      }
      return count;
    }

    // O texto que vai para a pesquisa: o que o parser nativo aceita (CRLF
    // como LF, controlos alem de \n e \t como espaco, UTF-16 bem formado),
    // aparado.
    function searchable(value) {
      const raw = toStr(value);
      let out = '';
      for (let i = 0; i < raw.length; i++) {
        const c = codeAt(raw, i);
        if (c === 13) {
          out += '\n';
          if (codeAt(raw, i + 1) === 10) i++;
        } else if (c <= 8 || (c >= 11 && c <= 31) || (c >= 127 && c <= 159)) {
          out += ' ';
        } else if (c >= 0xD800 && c <= 0xDBFF) {
          const low = codeAt(raw, i + 1);
          if (low >= 0xDC00 && low <= 0xDFFF) {
            out += fromCode(c, low);
            i++;
          } else {
            out += '�';
          }
        } else if (c >= 0xDC00 && c <= 0xDFFF) {
          out += '�';
        } else {
          out += fromCode(c);
        }
      }
      return trimmed(out);
    }

    // A area visivel, sem as barras de rolagem.
    function viewport() {
      const root = rootOf(document);
      let width = window.innerWidth || 0;
      let height = window.innerHeight || 0;
      if (root && root.clientWidth > 0 && root.clientWidth < width) width = root.clientWidth;
      if (root && root.clientHeight > 0 && root.clientHeight < height) height = root.clientHeight;
      return { width: width, height: height };
    }

    // Um DOMRect lido pelos getters capturados, num objeto so nosso.
    function edges(r) {
      if (!r) return null;
      return {
        top: rectTop(r), left: rectLeft(r), right: rectRight(r), bottom: rectBottom(r),
        width: rectWidth(r), height: rectHeight(r)
      };
    }

    function endRect(range) {
      const rects = rangeRects(range);
      for (let i = rects.length - 1; i >= 0; i--) {
        const r = edges(rects[i]);
        if (r && (r.width > 0 || r.height > 0)) return r;
      }
      const whole = edges(rangeBox(range));
      return whole && (whole.width > 0 || whole.height > 0) ? whole : null;
    }

    function selected() {
      const sel = docSelection(document);
      return !!sel && selCount(sel) > 0 && !selCollapsed(sel) ? sel : null;
    }

    // A selecao do utilizador, se for uma que a barra serve; null se nao.
    function snapshot() {
      const sel = selected();
      if (!sel) return null;
      const raw = toStr(selText(sel));
      const size = codePoints(trimmed(raw));
      if (size < 1 || size > SHOW_MAX) return null;
      const range = selRange(sel, 0);
      const anchor = selAnchor(sel);
      const focus = selFocus(sel);
      if (ours(anchor) || ours(focus)) return null;
      if (editable(anchor) || editable(focus) || editable(rangeCommon(range))
          || editable(focusedDeep())) return null;
      const rect = endRect(range);
      if (!rect) return null;
      // Fim da selecao fora da area visivel (rolou para longe): uma barra
      // encostada a borda apontaria para nada.
      const view = viewport();
      if (rect.bottom < 0 || rect.top > view.height || rect.right < 0 || rect.left > view.width) {
        return null;
      }
      const whole = edges(rangeBox(range));
      const top = whole && whole.height > 0 && whole.top < rect.top ? whole.top : rect.top;
      return { text: raw, rect: rect, top: top };
    }

    function stillSelected() {
      const sel = selected();
      return !!sel && toStr(selText(sel)) === text;
    }

    function setLabel(name, label) {
      if (buttons[name]) setText(buttons[name], label);
    }

    // Mensagem dentro da barra; a barra cresce e volta a caber no ecra.
    function say(message) {
      if (!note) return;
      setText(note, message);
      setAttr(note, 'class', message ? 'msg on' : 'msg');
      if (visible && shownAt) place(shownAt);
    }

    function clearFeedback() {
      if (feedbackTimer) { cancelLater(feedbackTimer); feedbackTimer = 0; }
      setLabel('copy', LABELS.copy);
    }

    function clearVoiceWait() {
      waitingVoices = null;
      if (voiceTimer) { cancelLater(voiceTimer); voiceTimer = 0; }
    }

    function button(parent, name, label, run, kind) {
      const b = makeElement(document, 'button');
      setAttr(b, 'type', 'button');
      // Fora da ordem do Tab e sem foco ao clicar: o foco fica na pagina.
      // (O menu "⋯" da o foco as entradas dele ao abrir, para o teclado.)
      setAttr(b, 'tabindex', '-1');
      setAttr(b, 'data-action', name);
      if (kind) setAttr(b, 'class', kind);
      setText(b, label);
      // mousedown sem efeito por omissao: o foco e a selecao ficam na pagina.
      listen(b, 'mousedown', guard(function (e) { prevent(e); }));
      listen(b, 'click', guard(function (e) {
        if (!e.isTrusted) { return; }
        prevent(e);
        stopHere(e);
        run();
      }));
      buttons[name] = b;
      appendTo(parent, b);
      return b;
    }

    // O "⋯" e o menu dele, na mesma shadow root fechada: so com as entradas
    // de MORE_MENU que este documento tem como fazer. Sem nenhuma, nem o
    // "⋯" aparece.
    function buildMenu() {
      const offered = [];
      for (let i = 0; i < MORE_MENU.length; i++) {
        if (MORE_MENU[i].ready) pushTo(offered, MORE_MENU[i]);
      }
      if (!offered.length) return;
      moreButton = button(bar, 'more', LABELS.more, toggleMenu, 'more');
      setAttr(moreButton, 'aria-label', 'Mais');
      setAttr(moreButton, 'title', 'Mais');
      setAttr(moreButton, 'aria-haspopup', 'menu');
      setAttr(moreButton, 'aria-expanded', 'false');
      menu = makeElement(document, 'div');
      setAttr(menu, 'class', 'menu');
      setAttr(menu, 'role', 'menu');
      setAttr(menu, 'aria-label', 'Mais');
      appendTo(bar, menu);
      for (let i = 0; i < offered.length; i++) {
        const item = button(menu, offered[i].name, offered[i].label, offered[i].run, 'item');
        setAttr(item, 'role', 'menuitem');
        pushTo(menuItems, item);
        pushTo(menuRuns, offered[i].run);
      }
      // Setas, Home/End, Enter/Espaco e Tab dentro do menu. O Esc passa
      // antes pelo mapa de teclas (`dismiss`), que fecha o menu.
      listen(menu, 'keydown', guard(menuKey));
    }

    // A entrada `index` (em volta: da ultima volta-se a primeira) passa a
    // ter o foco.
    function focusItem(index) {
      const count = menuItems.length;
      if (!count) return;
      menuAt = ((index % count) + count) % count;
      menuFocused = menuItems[menuAt];
      if (focusOn) { try { focusOn(menuFocused, { preventScroll: true }); } catch (err) {} }
    }

    // O lado da barra onde o menu abre: longe da selecao (por cima quando a
    // barra esta por cima dela, por baixo quando esta por baixo), ou o outro
    // se desse lado nao couber; encostado a direita da barra, onde esta o
    // "⋯", ou a esquerda se assim saisse do ecra. O menu flutua fora da
    // caixa da barra: ela nao muda de tamanho nem de sitio, e o "⋯" fica
    // debaixo do rato -- um segundo clique no mesmo ponto fecha o menu, nunca
    // cai no Falar.
    function fitMenu() {
      setAttr(menu, 'class', 'menu on up');
      let up = true;
      let start = false;
      try {
        const view = viewport();
        const own = edges(boxOf(host));
        const size = edges(boxOf(menu));
        const tall = size && size.height > 0 ? size.height : 0;
        const wide = size && size.width > 0 ? size.width : 0;
        if (own) {
          const above = !placed || !shownAt || placed.top < shownAt.top;
          const roomUp = own.top - 6 - tall >= MARGIN;
          const roomDown = own.bottom + 6 + tall <= view.height - MARGIN;
          up = above ? (roomUp || !roomDown) : (!roomDown && roomUp);
          start = own.right - wide < MARGIN;
        }
      } catch (err) { up = true; start = false; }
      setAttr(menu, 'class', 'menu on ' + (up ? 'up' : 'down') + (start ? ' start' : ''));
    }

    // `focus`: o clique no "⋯" leva o foco a primeira entrada, e dai o
    // teclado anda no menu. Aberto sozinho (a ler, sem selecao) nao tira o
    // foco a pagina.
    function openMenu(focus) {
      if (!menu) return;
      if (!menuOpen) {
        menuOpen = true;
        setAttr(moreButton, 'aria-expanded', 'true');
        fitMenu();
      }
      if (focus) focusItem(0);
    }

    // Fecha o menu; o foco que estava nele volta a pagina. A barra fica
    // onde estava.
    function closeMenu() {
      if (!menu || !menuOpen) return;
      menuOpen = false;
      setAttr(menu, 'class', 'menu');
      setAttr(moreButton, 'aria-expanded', 'false');
      const had = menuFocused;
      menuFocused = null;
      menuAt = 0;
      if (had && blurOf) { try { blurOf(had); } catch (err) {} }
    }

    function toggleMenu() {
      if (menuOpen) closeMenu(); else openMenu(true);
    }

    function menuKey(e) {
      if (!e.isTrusted || !menuOpen) return;
      const key = lower(toStr(keyOf(e) || ''));
      if (key === 'arrowdown') {
        prevent(e); focusItem(menuAt + 1);
      } else if (key === 'arrowup') {
        prevent(e); focusItem(menuAt - 1);
      } else if (key === 'home') {
        prevent(e); focusItem(0);
      } else if (key === 'end') {
        prevent(e); focusItem(menuItems.length - 1);
      } else if (key === 'enter' || key === ' ') {
        // O clique que o navegador faria a seguir nao chega: uma vez so.
        prevent(e);
        stopHere(e);
        const run = menuRuns[menuAt];
        if (run) run();
      } else if (key === 'escape' || key === 'tab') {
        if (key === 'escape') prevent(e);
        closeMenu();
      }
    }

    // Montada ja no document-created (fora da arvore ate a primeira vez):
    // a folha adotada e atribuida antes de a pagina poder trocar o setter
    // de ShadowRoot.prototype.adoptedStyleSheets.
    // O estilo que protege o host; reposto sempre que a barra aparece, por
    // cima do que a pagina lhe tenha escrito entretanto.
    function pin() {
      important(host, 'all', 'initial');
      important(host, 'position', 'fixed');
      important(host, 'z-index', '2147483647');
    }

    function build() {
      host = makeElement(document, 'div');
      pin();
      important(host, 'display', 'none');
      important(host, 'top', '0px');
      important(host, 'left', '0px');
      // Fechada: CSS e JS da pagina nao leem nem restilizam o que ha dentro.
      const root = shadowOf(host, { mode: 'closed' });
      // Folha construida primeiro: um CSP de style-src sem 'unsafe-inline'
      // bloqueia um <style>, nao uma CSSStyleSheet adotada.
      let styled = false;
      if (Sheet) {
        try {
          const sheet = new Sheet();
          sheet.replaceSync(CSS);
          root.adoptedStyleSheets = [sheet];
          styled = true;
        } catch (err) { styled = false; }
      }
      if (!styled) {
        const style = makeElement(document, 'style');
        setText(style, CSS);
        appendTo(root, style);
      }
      bar = makeElement(document, 'div');
      setAttr(bar, 'class', 'bar');
      setAttr(bar, 'role', 'toolbar');
      setAttr(bar, 'aria-label', 'Texto selecionado');
      appendTo(root, bar);
      // Pela ordem do pedido do dono. No painel privado nem Mandar nem
      // Traduzir existem: o texto dele nao sai para as IAs.
      if (searchAllowed) {
        button(bar, 'ask', LABELS.ask, function () { toAis('ask'); }, 'act');
      }
      button(bar, 'note', LABELS.note, saveNote, 'act');
      if (searchAllowed) {
        button(bar, 'translate', LABELS.translate, function () { toAis('translate'); }, 'act');
      }
      button(bar, 'copy', LABELS.copy, copy, 'act');
      buildMenu();
      note = makeElement(document, 'div');
      setAttr(note, 'class', 'msg');
      setAttr(note, 'role', 'status');
      appendTo(bar, note);
      if (Watch) {
        try {
          const watcher = new Watch(guard(function (entries) {
            for (let i = 0; i < entries.length; i++) {
              seenAt = seesIt(entries[i]) ? (seenAt || clock()) : 0;
            }
          }), { threshold: [0, 1], trackVisibility: true, delay: 100 });
          watchOn(watcher, host);
          watching = true;
        } catch (err) { watching = false; }
      }
    }
    build();

    // Perto do fim da selecao na horizontal; por cima da selecao INTEIRA se
    // couber (numa selecao de varias linhas nunca tapa o texto escolhido),
    // senao por baixo da ultima linha; sempre dentro da area visivel.
    function place(snap) {
      const view = viewport();
      const box = edges(boxOf(host));
      const w = box && box.width > 0 ? box.width : 320;
      const h = box && box.height > 0 ? box.height : 44;
      let top = snap.top - h - MARGIN;
      if (top < MARGIN) top = snap.rect.bottom + MARGIN;
      if (top + h > view.height - MARGIN) top = view.height - h - MARGIN;
      if (top < MARGIN) top = MARGIN;
      let left = snap.rect.right - w / 2;
      if (left + w > view.width - MARGIN) left = view.width - w - MARGIN;
      if (left < MARGIN) left = MARGIN;
      const at = { top: round(top), left: round(left) };
      // Mexeu-se: o tempo a vista conta de novo.
      if (!placed || placed.top !== at.top || placed.left !== at.left) steadyAt = clock();
      placed = at;
      important(host, 'top', at.top + 'px');
      important(host, 'left', at.left + 'px');
    }

    function show(snap) {
      const root = rootOf(document);
      // Filho direto da raiz; se a pagina o levou para outro sitio, volta, e
      // o tempo a vista conta de novo.
      if (!connected(host) || parentOf(host) !== root) {
        appendTo(root, host);
        placed = null;
      }
      if (!visible) placed = null;
      text = snap.text;
      shownAt = snap;
      clearFeedback();
      setAttr(bar, 'class', 'bar');
      // Uma selecao nova e uma barra nova; a ler, o menu fica como estava
      // (com o Parar a mao).
      if (!speaking) closeMenu();
      say('');
      pin();
      important(host, 'visibility', 'hidden');
      important(host, 'display', 'block');
      place(snap);
      // A ler, o menu que ficou aberto segue a barra para o lado certo.
      if (menuOpen) fitMenu();
      important(host, 'visibility', 'visible');
      visible = true;
    }

    // Escondida, a barra nao deixa relogio nenhum a correr.
    function hide() {
      if (showTimer) { cancelLater(showTimer); showTimer = 0; }
      clearFeedback();
      if (host) important(host, 'display', 'none');
      visible = false;
      text = '';
      shownAt = null;
      placed = null;
      seenAt = 0;
      gestureText = null;
      pressReady = 'alterada';
      closeMenu();
    }

    // A ler, a barra fica (o Parar tem de estar a mao), mas sem a selecao
    // antiga: Mandar, Salvar nota, Traduzir e Copiar nao agem sobre texto
    // que ja nao esta selecionado. Fica o "⋯" com o menu aberto no Parar,
    // sem tirar o foco a pagina: um clique para calar, como antes.
    function retire() {
      if (showTimer) { cancelLater(showTimer); showTimer = 0; }
      clearFeedback();
      text = '';
      gestureText = null;
      setAttr(bar, 'class', 'bar solo');
      say('');
      openMenu(false);
    }

    function drop() {
      if (speaking) retire(); else hide();
    }

    // So mostra a selecao que o gesto do utilizador deixou, e so se a pagina
    // nao a trocou no intervalo.
    function check() {
      showTimer = 0;
      const snap = snapshot();
      if (snap && snap.text === gestureText) { show(snap); return; }
      drop();
    }

    // Um clique sem selecao nao deixa relogio nenhum a correr.
    function schedule() {
      const sel = selected();
      if (!sel) { drop(); return; }
      gestureText = toStr(selText(sel));
      if (showTimer) cancelLater(showTimer);
      showTimer = later(guard(check), SHOW_DELAY_MS);
    }

    // Esc: fecha a barra (e cala a leitura) em vez de voltar atras. Com o
    // menu "⋯" aberto e nada a ler, fecha so o menu. Sem barra a vista, o
    // Esc segue para o 'back' de sempre.
    function dismiss() {
      if (menuOpen && visible && !speaking) {
        closeMenu();
        return true;
      }
      const busy = visible || speaking;
      stopSpeech();
      hide();
      return busy;
    }

    // Uma propriedade que o motor pode nao conhecer: so conta se tiver valor.
    function unset(style, name, initial) {
      const value = readStyle(style, name);
      return !value || value === initial;
    }

    function untouched(el, own) {
      if (!el) return false;
      const style = computed(window, el);
      if (readStyle(style, 'opacity') !== '1' || readStyle(style, 'filter') !== 'none'
          || readStyle(style, 'transform') !== 'none') return false;
      return !own || (readStyle(style, 'visibility') === 'visible'
        && readStyle(style, 'clip-path') === 'none'
        && readStyle(style, 'mix-blend-mode') === 'normal'
        && unset(style, 'mask-image', 'none')
        && unset(style, '-webkit-mask-image', 'none')
        && unset(style, 'content-visibility', 'visible'));
    }

    // Pronta para um clique em Mandar, Salvar nota ou Traduzir: '' se sim;
    // 'cedo' se ainda nao esta a vista ha ARM_MS; 'alterada' se a pagina a
    // tapou, moveu, mudou de sitio na arvore ou lhe mexeu no estilo (a
    // propria ou a raiz do documento).
    function readiness() {
      if (!visible || !placed) return 'alterada';
      const root = rootOf(document);
      if (!connected(host) || parentOf(host) !== root) return 'alterada';
      const box = edges(boxOf(host));
      if (!box || abs(box.top - placed.top) > 1 || abs(box.left - placed.left) > 1
          || !(box.width > 0) || !(box.height > 0)) return 'alterada';
      if (!untouched(host, true) || !untouched(root, false)) return 'alterada';
      const now = clock();
      if (now - steadyAt < ARM_MS) return 'cedo';
      if (watching) {
        if (!seenAt) return now - steadyAt < 2 * ARM_MS ? 'cedo' : 'alterada';
        if (now - seenAt < ARM_MS) return 'cedo';
      }
      return '';
    }

    // O clique conta com o estado da barra quando o botao desceu e agora.
    // Nao pronta: nada sai, e a barra diz porque (com o nome do botao).
    function armed(action) {
      const ready = pressReady || readiness();
      pressReady = 'alterada';
      if (ready) { say(ready === 'cedo' ? TOO_SOON[action] : TAMPERED); return false; }
      return true;
    }

    // Mandar para IA ('ask') e Traduzir ('translate'). A pergunta vai
    // inteira ou nao vai: acima do tecto nada sai da pagina e a barra diz
    // porque. O `search` abre o cartao nativo de confirmacao; nada chega as
    // IAs sem o clique la -- e o pedido de traducao e escrito pelo nativo,
    // a pagina so diz qual dos dois botoes foi.
    function toAis(intent) {
      if (!text) return;
      if (!armed(intent)) return;
      const question = searchable(text);
      if (!question) { hide(); return; }
      if (codePoints(question) > SEARCH_MAX) { say(TOO_LONG); return; }
      stopSpeech();
      // Envelope montado so com strings: o serializador nunca ve um objeto
      // em que a pagina possa pendurar um toJSON. `intent` e um dos dois
      // literais acima, nunca texto da pagina.
      post('{"v":1,"cap":"' + capability + '","action":"search","args":{"text":'
        + stringify(question) + ',"intent":"' + (intent === 'translate' ? 'translate' : 'ask')
        + '"}}');
      hide();
    }

    // Bytes UTF-8 de uma mensagem que o `stringify` capturado escreveu (um
    // substituto solto sai como \uXXXX: 6 bytes).
    function utf8Bytes(value) {
      let bytes = 0;
      for (let i = 0; i < value.length; i++) {
        const c = codeAt(value, i);
        if (c < 0x80) bytes += 1;
        else if (c < 0x800) bytes += 2;
        else if (c >= 0xD800 && c <= 0xDBFF && i + 1 < value.length
            && codeAt(value, i + 1) >= 0xDC00 && codeAt(value, i + 1) <= 0xDFFF) {
          bytes += 4;
          i++;
        } else if (c >= 0xD800 && c <= 0xDFFF) bytes += 6;
        else bytes += 3;
      }
      return bytes;
    }

    // Salvar nota: local, sem cartao. Manda no pedido (`note` com
    // `via: bar`) o texto que a barra mostra -- lido pelas primitivas
    // capturadas no document-created, como o do Mandar --, e o nativo grava-o
    // sem voltar a perguntar a pagina: um getSelection trocado depois nao
    // muda a nota. A fonte que fica nela e o endereco que o nativo conhece da
    // WebView, nunca um que a pagina diga. O mesmo filtro de clique que o
    // Mandar.
    function saveNote() {
      if (!text) return;
      if (!armed('note')) return;
      const body = searchable(text);
      if (!body) { hide(); return; }
      // Envelope montado so com strings, como o do Mandar.
      const message = '{"v":1,"cap":"' + capability + '","action":"note","args":{"via":"bar","text":'
        + stringify(body) + '}}';
      if (codePoints(body) > NOTE_MAX || utf8Bytes(message) > IPC_MAX_BYTES) {
        say(NOTE_TOO_LONG);
        return;
      }
      post(message);
      drop();
    }

    function copied(ok) {
      if (!visible) return;
      if (!ok) { say('Não foi possível copiar'); return; }
      setLabel('copy', LABELS.copied);
      if (feedbackTimer) cancelLater(feedbackTimer);
      feedbackTimer = later(guard(function () {
        feedbackTimer = 0;
        setLabel('copy', LABELS.copy);
      }), FEEDBACK_MS);
    }

    function copyByCommand() {
      let ok = false;
      try { ok = !!(execCommand && execCommand(document, 'copy')); } catch (err) { ok = false; }
      copied(ok);
    }

    function copy() {
      if (!text) return;
      if (!writeText) { copyByCommand(); return; }
      try {
        thenOf(writeText(clip, text), guard(function () { copied(true); }), guard(copyByCommand));
      } catch (err) {
        copyByCommand();
      }
    }

    function langTag(value) {
      const raw = lower(toStr(value || ''));
      let out = '';
      for (let i = 0; i < raw.length; i++) {
        const c = codeAt(raw, i);
        out += c === 95 ? '-' : fromCode(c);
      }
      return out;
    }

    function baseOf(tag) {
      const at = findIn(tag, '-');
      return at < 0 ? tag : sliceOf(tag, 0, at);
    }

    // Uma voz que o motor diz ser local, lida pelo acessor capturado.
    function isLocal(voice) {
      try { return !!voice && voiceApi.local(voice) === true; } catch (err) { return false; }
    }

    function langOf(voice) {
      try { return voiceApi.lang(voice); } catch (err) { return ''; }
    }

    // Quanto uma voz local serve: 0/1 na lingua da pagina (exata / so a
    // base), 2/3 em pt-BR, 4/5 na do sistema, 6 a de omissao, 7 outra.
    function voiceRank(voice, wanted) {
      const have = langTag(langOf(voice));
      for (let w = 0; w < wanted.length; w++) {
        const tag = wanted[w];
        if (!tag) continue;
        if (have === tag) return 2 * w;
        if (baseOf(have) === baseOf(tag)) return 2 * w + 1;
      }
      let preferred = false;
      try { preferred = voiceApi.isDefault(voice) === true; } catch (err) { preferred = false; }
      return preferred ? 6 : 7;
    }

    // Voz local (offline) na lingua da pagina; senao pt-BR; senao a do
    // sistema; senao a de omissao; senao qualquer voz local. Nunca uma voz
    // online. Uma so passagem, sem listas intermedias: entre ver que a voz e
    // local e devolve-la nao ha nada que a pagina possa trocar.
    function chooseVoice(voices) {
      let pageLang = '';
      try { pageLang = rootOf(document).lang; } catch (err) { pageLang = ''; }
      const wanted = [langTag(pageLang), langTag('pt-BR'), langTag(navLang)];
      let best = null;
      let bestRank = 8;
      const count = voices.length;
      for (let i = 0; i < count; i++) {
        const voice = voices[i];
        if (!isLocal(voice)) continue;
        const rank = voiceRank(voice, wanted);
        if (rank < bestRank) { best = voice; bestRank = rank; }
      }
      return best;
    }

    // . ! ? … ; : -- depois de um destes, um espaco acaba a frase.
    function closes(c) {
      return c === 46 || c === 33 || c === 63 || c === 0x2026 || c === 59 || c === 58;
    }

    // As frases do texto, com os espacos de cada uma reduzidos a um so.
    function pieces(value) {
      const out = [];
      let current = '';
      let gap = false;
      for (let i = 0; i < value.length; i++) {
        const c = codeAt(value, i);
        if (c === 10) {
          pushTo(out, current);
          current = '';
          gap = false;
        } else if (blank(c)) {
          if (current && closes(codeAt(current, current.length - 1))) {
            pushTo(out, current);
            current = '';
            gap = false;
          } else if (current) {
            gap = true;
          }
        } else {
          if (gap) { current += ' '; gap = false; }
          current += fromCode(c);
        }
      }
      pushTo(out, current);
      return out;
    }

    // O Chromium corta falas longas: uma frase por fala. Uma abreviatura
    // solta ("Sr.", "Fig.") cola-se a frase seguinte; uma frase enorme parte
    // em palavras.
    function sentences(value) {
      const out = [];
      let current = '';
      const parts = pieces(toStr(value));
      for (let i = 0; i < parts.length; i++) {
        let piece = parts[i];
        while (piece.length > SPEECH_CHUNK) {
          let cut = findLast(piece, ' ', SPEECH_CHUNK);
          if (cut < SPEECH_CHUNK / 2) {
            // Sem espaco: corte seco, mas nunca a meio de um par UTF-16.
            cut = SPEECH_CHUNK;
            const high = codeAt(piece, cut - 1);
            if (high >= 0xD800 && high <= 0xDBFF) cut--;
          }
          if (current) { pushTo(out, current); current = ''; }
          pushTo(out, trimmed(sliceOf(piece, 0, cut)));
          piece = trimmed(sliceOf(piece, cut));
        }
        if (!piece) continue;
        if (current && current.length <= SPEECH_ABBREVIATION && findIn(current, ' ') < 0
            && current.length + 1 + piece.length <= SPEECH_CHUNK) {
          current = current + ' ' + piece;
        } else {
          if (current) pushTo(out, current);
          current = piece;
        }
      }
      if (current) pushTo(out, current);
      return out;
    }

    function withVoice(run, done) {
      if (listVoices(synth).length) { done(chooseVoice(listVoices(synth))); return; }
      // As vozes chegam depois ('voiceschanged'); a primeira lista vem vazia.
      waitingVoices = function () {
        if (run !== speechRun) return;
        clearVoiceWait();
        done(chooseVoice(listVoices(synth)));
      };
      if (!voicesHooked) {
        voicesHooked = true;
        listen(synth, 'voiceschanged', guard(function () {
          if (waitingVoices) waitingVoices();
        }));
      }
      voiceTimer = later(guard(function () {
        voiceTimer = 0;
        if (waitingVoices) waitingVoices();
      }), VOICE_WAIT_MS);
    }

    function stopSpeech() {
      speechRun++;
      clearVoiceWait();
      speechHold = null;
      if (speaking) {
        speaking = false;
        try { cancelSpeech(synth); } catch (err) {}
      }
      setLabel('speak', LABELS.speak);
    }

    function spoken(run) {
      if (run !== speechRun) return;
      speaking = false;
      speechHold = null;
      setLabel('speak', LABELS.speak);
      if (!text || !stillSelected()) hide();
    }

    function toggleSpeech() {
      if (speaking) {
        stopSpeech();
        // Parada a meio: a barra so fica se ainda mostrar a selecao atual.
        if (!text || !stillSelected()) hide();
        return;
      }
      const parts = sentences(text);
      if (!parts.length) return;
      speechRun++;
      const run = speechRun;
      speaking = true;
      setLabel('speak', LABELS.stop);
      withVoice(run, function (voice) {
        if (run !== speechRun) return;
        // Outra vez, pelo acessor capturado, mesmo antes de usar.
        if (!voice || !isLocal(voice)) {
          speaking = false;
          setLabel('speak', LABELS.speak);
          say('Nenhuma voz local disponível');
          return;
        }
        try { cancelSpeech(synth); } catch (err) {}
        let next = 0;
        function sayNext() {
          if (run !== speechRun) return;
          if (next >= parts.length) { spoken(run); return; }
          const utterance = new Utterance(parts[next++]);
          voiceApi.use(utterance, voice);
          voiceApi.speakIn(utterance, langOf(voice));
          listen(utterance, 'end', guard(sayNext));
          listen(utterance, 'error', guard(function () { spoken(run); }));
          speechHold = utterance;
          speakNow(synth, utterance);
        }
        sayNext();
      });
    }

    // Os ouvintes do window em captura sao registados aqui, antes dos da
    // pagina: correm primeiro e veem a selecao tal como o gesto a deixou.
    listen(window, 'mousedown', guard(function (e) {
      if (ours(targetOf(e))) {
        prevent(e);
        // O clique em Mandar, Salvar nota ou Traduzir conta com o estado da
        // barra quando o botao desceu, e outra vez quando sobe.
        pressReady = e.isTrusted ? readiness() : 'alterada';
        return;
      }
      pressReady = 'alterada';
      if (e.isTrusted) { downX = xOf(e); downY = yOf(e); }
      drop();
    }), true);

    listen(window, 'mouseup', guard(function (e) {
      if (!e.isTrusted || buttonOf(e) !== 0 || ours(targetOf(e))) return;
      // So um arrasto, um duplo/triplo clique ou Shift+clique escolhem
      // texto. Um clique simples nao: uma selecao que a pagina pos sozinha
      // nao traz a barra.
      const moved = abs(xOf(e) - downX) + abs(yOf(e) - downY);
      if (moved < DRAG_MIN && detailOf(e) < 2 && !mouseShift(e)) {
        if (!speaking) hide();
        return;
      }
      schedule();
    }), true);

    function moves(key) {
      return findIn(key, 'arrow') === 0 || key === 'home' || key === 'end'
        || key === 'pageup' || key === 'pagedown';
    }

    // Teclado: so Shift+setas/Home/End/PgUp/PgDn e Ctrl+A escolhem texto.
    // Setas e PgDn sozinhas so rolam; soltar o Ctrl depois de um Ctrl+C
    // tambem nao: nenhum deles traz de volta uma barra fechada com Esc.
    listen(window, 'keydown', guard(function (e) {
      if (!e.isTrusted) return;
      const key = lower(toStr(keyOf(e) || ''));
      if ((keyShift(e) && moves(key)) || ((keyCtrl(e) || keyMeta(e)) && key === 'a')) {
        keySelecting = true;
      } else if (key !== 'shift' && key !== 'control' && key !== 'meta') {
        keySelecting = false;
      }
    }), true);

    listen(window, 'keyup', guard(function (e) {
      if (!e.isTrusted || !keySelecting) return;
      const key = lower(toStr(keyOf(e) || ''));
      if (moves(key) || key === 'a' || key === 'shift' || key === 'control' || key === 'meta') {
        keySelecting = false;
        schedule();
      }
    }), true);

    // Duplo clique nos botoes nao chega a pagina (no comparador expandia a
    // coluna).
    listen(window, 'dblclick', guard(function (e) {
      if (ours(targetOf(e))) { stopAll(e); }
    }), true);

    listen(document, 'selectionchange', guard(function () {
      if (visible && !stillSelected()) drop();
    }));

    const quietHide = guard(function () { if (!speaking) hide(); });
    listen(window, 'scroll', quietHide, true);
    listen(window, 'resize', quietHide);
    listen(window, 'popstate', quietHide);
    listen(window, 'hashchange', quietHide);
    listen(window, 'blur', quietHide);
    listen(window, 'pagehide', guard(function () { stopSpeech(); hide(); }));

    return {
      dismiss: function () {
        try { return dismiss(); } catch (err) { return false; }
      }
    };
  }

  document.addEventListener('keydown', function (e) {
    if (!e.isTrusted) { return; }
    var mod = e.ctrlKey || e.metaKey;
    var key = (e.key || '').toLowerCase();

    // Combinacoes com Ctrl valem mesmo dentro de um campo de texto.
    if (mod && !e.altKey) {
      if (e.shiftKey && key === 'delete') { e.preventDefault(); act('clearhistory'); return; }
      if (e.shiftKey && key === 'r') { e.preventDefault(); act('reload'); return; }
      if (e.shiftKey && (key === 'i' || key === 'j' || key === 'c')) {
        e.preventDefault(); act('devtools'); return;
      }
      // Ctrl+Shift+Z: nota com o texto selecionado. A pagina so pede; a
      // selecao e lida pelo lado nativo. Num campo editavel continua a ser o
      // refazer do editor. A tecla presa repete o keydown: so o primeiro
      // pede a nota (o nativo tambem nao grava a mesma nota duas vezes em
      // 2 s).
      if (e.shiftKey && key === 'z') {
        var field = e.target || {};
        var fieldTag = (field.tagName || '').toUpperCase();
        if (fieldTag === 'INPUT' || fieldTag === 'TEXTAREA' || field.isContentEditable) { return; }
        e.preventDefault();
        if (!e.repeat) { act('note'); }
        return;
      }
      if (key === 'u') { e.preventDefault(); act('viewsource'); return; }
      switch (key) {
        // Ctrl+R liga/desliga a rolagem automatica (pedido do dono);
        // recarregar fica no F5 e no Ctrl+Shift+R.
        case 'r': e.preventDefault(); act('autoscroll'); return;
        case 'l': e.preventDefault(); act('omnibox'); return;
        case 'h': e.preventDefault(); act('history'); return;
        case 'n':
          e.preventDefault();
          if (typeof colIndex === 'number') {
            act('newtab', { col:colIndex });
          } else {
            act('newtab');
          }
          return;
        case 'k':
        case 't':
          e.preventDefault();
          if (typeof colIndex === 'number') {
            // So o pedido de abertura: o texto vai ser escrito num controlo
            // nativo, fora do alcance da pagina.
            act('palette', { col:colIndex });
          } else if (key === 'k') {
            act('omnibox');
          } else {
            act('home');
          }
          return;
        case 'w': e.preventDefault(); act('back'); return;
        case 'p': e.preventDefault(); act('print'); return;
        case 'f': e.preventDefault(); findBar(); return;
        case '+': case '=': e.preventDefault(); act('zoomin'); return;
        case '-': case '_': e.preventDefault(); act('zoomout'); return;
        case '0': e.preventDefault(); act('zoomreset'); return;
      }
    }

    if (e.altKey && key === 'arrowleft') { e.preventDefault(); window.history.back(); return; }
    if (e.altKey && key === 'arrowright') { e.preventDefault(); window.history.forward(); return; }
    var target = e.target || {};
    if (target.closest && target.closest('#neuralia-find')) { return; }

    if (key === 'f5') { e.preventDefault(); act('reload'); return; }
    if (key === 'f12') { e.preventDefault(); act('devtools'); return; }
    if (key === 'f8') { e.preventDefault(); act('autoscroll'); return; }
    if (key === 'f11') { e.preventDefault(); act('fullscreen'); return; }
    // Com a barra de selecao aberta (ou a ler), o Esc fecha-a e fica por ai.
    if (key === 'escape' && selectionBar && selectionBar.dismiss()) {
      e.preventDefault(); e.stopPropagation(); return;
    }
    if (key === 'escape') { e.preventDefault(); e.stopPropagation(); act('back'); return; }

    var tag = (target.tagName || '').toUpperCase();
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable) {
      return;
    }
    if (e.altKey || mod) { return; }

    if (key === 'backspace') { e.preventDefault(); window.history.back(); return; }
    if (key === '1' || key === '2' || key === '3') {
      if (typeof colIndex === 'number') {
        e.preventDefault();
        act('shortcut-expand', { col:(parseInt(key, 10) - 1) });
      }
      return;
    }
    if (key === '0' && typeof colIndex === 'number') {
      e.preventDefault();
      act('restore');
    }
  }, true);

  // Ctrl + roda do rato, e a pinca do touchpad (o Chromium entrega-a como
  // ctrl+wheel), sobem e descem os mesmos degraus do Ctrl+= e do Ctrl+-, com
  // o mesmo aviso. O zoom do proprio WebView2 esta desligado (o wry deixa
  // IsZoomControlEnabled e IsPinchZoomEnabled a false): um mecanismo so, sem
  // zoom a dobrar.
  const defer = setTimeout;
  // Pixels de roda por degrau: um entalhe de rato (100) sobe um degrau; a
  // pinca, que chega em pedacos pequenos, soma ate la.
  const WHEEL_ZOOM_STEP = 30;
  // Uma pausa maior do que isto comeca um gesto novo.
  const WHEEL_ZOOM_IDLE_MS = 400;
  let wheelZoomSum = 0;
  let wheelZoomAt = -Infinity;
  function wheelZoomAction(e) {
    const unit = e.deltaMode === 1 ? 33 : (e.deltaMode === 2 ? 800 : 1);
    const dy = Number(e.deltaY) * unit;
    if (!isFinite(dy) || dy === 0) { return null; }
    const at = Number(e.timeStamp) || 0;
    if (at - wheelZoomAt > WHEEL_ZOOM_IDLE_MS || (wheelZoomSum < 0) !== (dy < 0)) {
      wheelZoomSum = 0;
    }
    wheelZoomAt = at;
    wheelZoomSum += dy;
    if (Math.abs(wheelZoomSum) < WHEEL_ZOOM_STEP) { return null; }
    const action = wheelZoomSum < 0 ? 'zoomin' : 'zoomout';
    wheelZoomSum = 0;
    return action;
  }
  window.addEventListener('wheel', function (e) {
    if (!e.isTrusted || !e.ctrlKey) { return; }
    const wheel = e;
    // Decide-se depois de o evento passar por todos: uma pagina que trata o
    // gesto ela propria (um mapa, um editor) chama preventDefault, e ai o
    // zoom e dela -- como no Chrome.
    defer(function () {
      if (wheel.defaultPrevented) { return; }
      const action = wheelZoomAction(wheel);
      if (action) { act(action); }
    }, 0);
  }, { passive: true });
})();
"#;

/// Fechado num IIFE: um `const` de topo seria um binding lexico global, e a
/// pagina lia o token pelo nome. Tudo o que o botao usa depois do
/// DOMContentLoaded e capturado aqui, antes de a pagina correr.
pub(in crate::windows_app) const EXTERNAL_RETURN_BUTTON: &str = r#"
(function () {
  if (window.top !== window) return;
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
  const defer = setTimeout;
  const cancelDefer = clearTimeout;
  const byId = document.getElementById.bind(document);
  const createElement = document.createElement.bind(document);
  const assign = Object.assign;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const append = Function.prototype.call.bind(Node.prototype.appendChild);

  listen(document, 'DOMContentLoaded', () => {
    if (byId('neural-shell') || byId('neuralia-return')) return;
    const b = createElement('button');
    b.id = 'neuralia-return';
    b.textContent = '◀ NeuralIA';
    assign(b.style, {
      position:'fixed', left:'16px', bottom:'16px', zIndex:'2147483647',
      border:'0', borderRadius:'999px', padding:'11px 16px',
      background:'#111314', color:'#fff', font:'600 13px Segoe UI, sans-serif',
      boxShadow:'0 6px 24px rgba(0,0,0,.25)', cursor:'pointer'
    });
    listen(b, 'click', (event) => {
      if (!event.isTrusted) return;
      post(envelope('home', {}));
    });
    append(document.documentElement, b);
  });
})();
"#;

pub(in crate::windows_app) const GMAIL_MONITOR_SCRIPT: &str = r#"
(function () {
  if (window.top !== window) return;
  if (location.hostname !== 'mail.google.com' || window.__neuralia_gmail_monitor) return;
  window.__neuralia_gmail_monitor = true;
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
  let lastState = '';
  let debounce = 0;

  function clean(value) {
    return String(value || '').replace(/\s+/g, ' ').trim().slice(0, 180);
  }

  function unreadCount() {
    const match = String(document.title || '').match(/\(([\d.,]+)\)/);
    if (!match) return 0;
    const digits = match[1].replace(/\D/g, '');
    return Number(digits || '0');
  }

  function firstUnread() {
    const row = document.querySelector(
      'tr.zE,[role="main"] tr.zE,[role="main"] [data-legacy-thread-id].zE'
    );
    if (!row) return { sender:'', subject:'', key:'' };

    const senderNode = row.querySelector('.zF,.yP,[email]');
    const subjectNode = row.querySelector('.bog,[data-thread-id] .bog');
    const sender = clean(
      senderNode && (senderNode.getAttribute('email')
        || senderNode.getAttribute('name')
        || senderNode.textContent)
    );
    const subject = clean(subjectNode && subjectNode.textContent);
    const key = clean(
      row.getAttribute('data-legacy-thread-id')
        || row.getAttribute('data-thread-id')
        || (sender + '|' + subject)
    );
    return { sender, subject, key };
  }

  function emit() {
    if (location.hostname !== 'mail.google.com') return;
    const first = firstUnread();
    const count = unreadCount();
    const state = count + '|' + first.key;
    if (state === lastState) return;
    lastState = state;

    post(envelope('gmail-state', {
      count, sender:first.sender, subject:first.subject, key:first.key
    }));
  }

  function schedule() {
    clearTimeout(debounce);
    debounce = setTimeout(emit, 450);
  }

  // Como os outros observers deste ficheiro: uma chamada por quadro, nao uma
  // por mutacao. A caixa de entrada muda o DOM em rajadas de centenas de
  // registos e cada um fazia clearTimeout/setTimeout.
  let raf = 0;
  const coalesce = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => { raf = 0; schedule(); });
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', schedule, { once:true });
  } else {
    schedule();
  }

  // So childList+subtree: attributes e characterData disparavam a cada
  // realce de linha e a cada relogio que o Gmail redesenha. O que escapar
  // apanha-se no emit() periodico.
  new MutationObserver(coalesce).observe(document.documentElement, {
    childList:true, subtree:true
  });
  setInterval(emit, 15000);
})();
"#;

/// ChatGPT e Claude aceitam a consulta por ?q=, mas hoje apenas preenchem o
/// compositor. O comparador tem semântica de "perguntar às três", portanto o
/// NeuralIA confirma o envio assim que o botão real do fornecedor fica pronto.
/// Envia a pergunta no fornecedor em vez de a deixar na caixa.
///
/// O Gemini abre directamente numa pagina de resultados; o ChatGPT e o Claude
/// recebem `?q=` que so PREENCHE a caixa. A versao anterior esperava que
/// `promptText()` devolvesse texto antes de carregar em enviar -- mas lia o
/// PRIMEIRO `textarea` da pagina, que nestes sitios e um campo escondido e
/// vazio. Ficava a tentar 120 vezes e desistia, e a pergunta ficava na barra a
/// espera de um Enter manual: exactamente o que o utilizador via.
///
/// Agora procura o editor que TEM texto, e se nenhum tiver escreve a pergunta
/// ele proprio antes de enviar.
pub(in crate::windows_app) const AI_AUTO_SUBMIT_SCRIPT: &str = r#"
(function () {
  // So no frame de topo. Este script ESCREVE numa caixa de texto, e o WebView2
  // injeta os scripts de inicializacao tambem nos frames filhos: sem esta
  // guarda, um iframe da mesma origem levava com a pergunta escrita dentro.
  if (window.top !== window) return;
  const host = location.hostname.toLowerCase();
  if (host !== 'chatgpt.com' && host !== 'claude.ai') return;

  // Claude pode redireccionar /new?q=... antes de o compositor ficar pronto.
  // Guardamos a consulta no sessionStorage no primeiro documento e retomamos
  // no seguinte. Assim o auto-submit nao depende de o fornecedor preservar o
  // parametro q durante toda a montagem da SPA.
  function storageRead(key) {
    try { return String(sessionStorage.getItem(key) || ''); } catch (_) { return ''; }
  }
  function storageWrite(key, value) {
    try { sessionStorage.setItem(key, String(value)); } catch (_) {}
  }
  function storageRemove(key) {
    try { sessionStorage.removeItem(key); } catch (_) {}
  }
  function stampRead(key) {
    return Number(storageRead(key) || '0');
  }
  function stampWrite(key, value) {
    storageWrite(key, value);
  }

  let urlQuery = '';
  try { urlQuery = String(new URL(location.href).searchParams.get('q') || '').trim(); } catch (_) {}
  const pendingKey = 'neuralia:pending-query:' + host;
  if (urlQuery) storageWrite(pendingKey, urlQuery);
  const query = (urlQuery || storageRead(pendingKey)).trim();
  if (!query) return;

  const stampKey = 'neuralia:auto-submit:' + host + ':' + query;
  if (Date.now() - stampRead(stampKey) < 10000) {
    storageRemove(pendingKey);
    return;
  }

  function finish() {
    stampWrite(stampKey, Date.now());
    storageRemove(pendingKey);
  }

  const EDITORS = 'div[contenteditable="true"][role="textbox"], div[contenteditable="true"], [data-testid="prompt-textarea"], textarea';

  function textOf(el) {
    if (!el) return '';
    return String('value' in el && typeof el.value === 'string' ? el.value : el.innerText || el.textContent || '').trim();
  }

  function visible(el) {
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  // O editor certo e o que ESTA VISIVEL e, de preferencia, o que ja tem texto.
  // Ler so o primeiro `textarea` apanhava um campo escondido e vazio.
  function editor() {
    const all = Array.from(document.querySelectorAll(EDITORS)).filter(visible);
    return all.find((el) => textOf(el)) || all[0] || null;
  }

  function fill(el) {
    el.focus();
    if (el.isContentEditable) {
      // `execCommand` e o que os editores com React por tras aceitam sem
      // reescrever o estado deles por baixo.
      if (!document.execCommand('insertText', false, query)) {
        el.textContent = query;
        el.dispatchEvent(new InputEvent('input', { bubbles:true, data:query, inputType:'insertText' }));
      }
      return;
    }
    const descriptor = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
    if (descriptor && descriptor.set) descriptor.set.call(el, query);
    else el.value = query;
    el.dispatchEvent(new Event('input', { bubbles:true }));
  }

  // Apagar o que NOS escrevemos. So se usa quando desistimos: texto que o
  // utilizador nao escreveu nao pode ficar na caixa de outra pessoa.
  function clear(el) {
    if (!el) return;
    el.focus();
    if (el.isContentEditable) {
      const range = document.createRange();
      range.selectNodeContents(el);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      if (!document.execCommand('insertText', false, '')) {
        el.textContent = '';
        el.dispatchEvent(new InputEvent('input', { bubbles:true, inputType:'deleteContentBackward' }));
      }
      return;
    }
    const descriptor = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
    if (descriptor && descriptor.set) descriptor.set.call(el, '');
    else el.value = '';
    el.dispatchEvent(new Event('input', { bubbles:true }));
  }

  function sendButton() {
    const selectors = host === 'chatgpt.com'
      ? ['button[data-testid="send-button"]', 'button[aria-label*="Send prompt"]', 'button[aria-label*="Send message"]', 'button[aria-label*="Enviar"]', 'form button[type="submit"]']
      : ['button[data-testid="send-button"]', 'button[aria-label*="Send"]', 'button[aria-label*="Enviar"]', 'button[data-testid*="send"]', 'form button[type="submit"]'];
    for (const selector of selectors) {
      const button = document.querySelector(selector);
      if (!button || button.disabled || button.getAttribute('aria-disabled') === 'true') continue;
      if (!visible(button)) continue;
      return button;
    }
    return null;
  }

  // O sitio ja tratou da pergunta sozinho?
  //
  // O ChatGPT, com `?q=...&hints=search`, NAO se limita a preencher a caixa:
  // envia a pergunta e troca a URL para `/uc/<id>` sem recarregar a pagina.
  // A caixa fica entao vazia -- e o script, que guardou a pergunta no
  // arranque, via-a vazia e escrevia-a de volta. Era isso que ficava escrito
  // no ChatGPT depois de a resposta ja estar na tela.
  //
  // O sinal e a propria URL e nao o DOM: o `?q=` desaparece quando o site o
  // consome, em qualquer provedor e em qualquer versao do HTML deles. Ler o
  // DOM obrigava a conhecer os seletores de cada um -- e o ChatGPT tem pelo
  // menos duas variantes (ligado e desligado) com marcadores diferentes.
  function consumed() {
    // No ChatGPT o desaparecimento de q e um sinal real de submissao. No
    // Claude e apenas parte do redirect de /new para a SPA, portanto nao pode
    // encerrar o auto-submit antes de o compositor sequer existir.
    if (host === 'claude.ai') return false;
    try {
      return new URL(location.href).searchParams.get('q') !== query;
    } catch (_) {
      return true;
    }
  }

  // Reaviva o estado do framework quando o proprio ?q= desenhou texto no
  // editor mas ainda nao habilitou o botao de envio.
  function nudge(el) {
    if (!el) return;
    try {
      el.dispatchEvent(new InputEvent('input', { bubbles:true, inputType:'insertText' }));
    } catch (_) {
      el.dispatchEvent(new Event('input', { bubbles:true }));
    }
    el.dispatchEvent(new Event('change', { bubbles:true }));
  }

  // Primeiro tenta o botao real. Se o fornecedor escondeu o botao mas o
  // editor pertence a um form, requestSubmit() percorre o caminho nativo do
  // formulario. O KeyboardEvent sintetico fica apenas como ultimo recurso:
  // Chromium marca-o isTrusted=false e os fornecedores podem ignora-lo.
  function submitEditor(el) {
    const button = sendButton();
    if (button) {
      button.click();
      return 'button';
    }

    const form = el && typeof el.closest === 'function' ? el.closest('form') : null;
    if (form && typeof form.requestSubmit === 'function') {
      try {
        const submitter = form.querySelector(
          'button[type="submit"]:not([disabled]), input[type="submit"]:not([disabled])'
        );
        if (submitter) form.requestSubmit(submitter);
        else form.requestSubmit();
        return 'form';
      } catch (_) {}
    }

    el.focus();
    for (const type of ['keydown', 'keypress', 'keyup']) {
      el.dispatchEvent(new KeyboardEvent(type, {
        key:'Enter', code:'Enter', keyCode:13, which:13,
        bubbles:true, cancelable:true
      }));
    }
    return 'keyboard';
  }

  let attempts = 0;
  let filled = false;
  let nudged = false;
  let lastSubmitAt = 0;

  function submitWhenReady() {
    attempts += 1;

    if (consumed()) {
      finish();
      return;
    }

    const el = editor();

    // Depois de uma tentativa, o compositor vazio e o melhor reconhecimento
    // transversal de que o site aceitou a pergunta. Nao ha novo clique.
    if (lastSubmitAt && el && !textOf(el)) {
      finish();
      return;
    }

    if (el) {
      // Dez tentativas (~1,5 s) para o proprio site preencher o compositor.
      // Depois disso escrevemos nos, caso ele ainda esteja vazio.
      if (!textOf(el) && !filled && attempts > 10) {
        fill(el);
        filled = true;
      }

      if (textOf(el)) {
        // Um ?q= pode pintar a string sem acordar o estado React. Reemitir
        // input/change uma vez deixa o botao real nascer/habilitar.
        if (!nudged && attempts > 8) {
          nudge(el);
          nudged = true;
        }

        const now = Date.now();
        // Nao martelar o endpoint enquanto uma submissao anterior ainda pode
        // estar em voo. Se um Enter sintetico for ignorado, continuamos a
        // observar e tentamos o botao/formulario assim que aparecer.
        if (attempts > 10 && now - lastSubmitAt >= 2500) {
          lastSubmitAt = now;
          submitEditor(el);
        }
      }
    }

    if (attempts < 160) {
      setTimeout(submitWhenReady, 150);
      return;
    }

    // Desistimos ao fim de ~24 s. Se fomos NOS a escrever e nunca chegou a ser
    // enviado, a pergunta nao pode ficar la a fingir que o utilizador a
    // escreveu.
    if (filled) clear(el || editor());
    storageRemove(pendingKey);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => setTimeout(submitWhenReady, 100), { once:true });
  } else {
    setTimeout(submitWhenReady, 100);
  }
})();
"#;

pub(in crate::windows_app) const SPLIT_SCROLL_RAIL_SCRIPT: &str = r#"
(function () {
  // WRY/WebView2 injeta initialization scripts em child frames no Windows:
  // o rail e o CSS que esconde as barras so pertencem ao documento principal.
  if (window.top !== window) return;
document.addEventListener('DOMContentLoaded', () => {
  if (document.getElementById('neuralia-split-scroll-rail')) return;

  const style = document.createElement('style');
  style.id = 'neuralia-split-scroll-style';
  style.textContent = [
    '*{scrollbar-width:none!important;-ms-overflow-style:none!important;}',
    '*::-webkit-scrollbar{width:0!important;height:0!important;display:none!important;background:transparent!important;}'
  ].join('');
  document.documentElement.appendChild(style);

  let currentRoot = null;
  let semantic = [];

  function scrollRoot() {
    const docRoot = document.scrollingElement || document.documentElement || document.body;
    const candidates = docRoot ? [docRoot] : [];
    document.querySelectorAll(
      'main,[role="main"],[data-radix-scroll-area-viewport],'
      + '[data-testid*="scroll"],[class*="scroll"],[class*="overflow"],[style*="overflow"]'
    ).forEach((el) => candidates.push(el));

    let best = docRoot;
    let bestRange = best ? Math.max(0, best.scrollHeight - best.clientHeight) : 0;
    for (const el of candidates) {
      if (!el || el === document.body) continue;
      const range = Math.max(0, el.scrollHeight - el.clientHeight);
      if (range <= bestRange + 24) continue;
      const css = getComputedStyle(el);
      if (css.display === 'none' || css.visibility === 'hidden' || css.overflowY === 'hidden') continue;
      best = el;
      bestRange = range;
    }
    currentRoot = best || docRoot;
    return currentRoot;
  }

  function metrics() {
    const root = scrollRoot();
    if (!root) return { root:null, top:0, max:0, view:window.innerHeight, docLike:true };
    const docLike = root === document.scrollingElement
      || root === document.documentElement || root === document.body;
    return {
      root,
      docLike,
      top: docLike ? window.scrollY : root.scrollTop,
      max: Math.max(0, root.scrollHeight - root.clientHeight),
      view: root.clientHeight || window.innerHeight
    };
  }

  function scrollToPosition(top) {
    const state = metrics();
    const value = Math.max(0, Math.min(state.max, top));
    if (state.docLike) window.scrollTo({ top:value, behavior:'smooth' });
    else if (state.root) state.root.scrollTo({ top:value, behavior:'smooth' });
  }

  function anchorTop(el, state) {
    const rect = el.getBoundingClientRect();
    if (state.docLike) return state.top + rect.top;
    const rootRect = state.root.getBoundingClientRect();
    return state.top + rect.top - rootRect.top;
  }

  function kindOf(el) {
    const role = el.getAttribute('data-message-author-role');
    const tag = el.tagName ? el.tagName.toLowerCase() : '';
    const text = (el.textContent || '').trim().toLowerCase();
    if (role === 'user') return 'pergunta';
    if (role === 'assistant') return 'resposta';
    if (/^h[1-6]$/.test(tag)) return text.includes('conclus') ? 'conclusão' : 'seção';
    if (tag === 'pre' || tag === 'code') return 'código';
    if (tag === 'table') return 'tabela';
    if (tag === 'blockquote') return 'citação';
    if (tag === 'aside') return 'nota';
    if (tag === 'a') return 'fonte';
    return 'resposta';
  }

  function semanticAnchors() {
    const state = metrics();
    const raw = [];
    const seen = new Set();
    const nodes = document.querySelectorAll(
      '[data-message-author-role="user"],[data-message-author-role="assistant"],'
      + 'h1,h2,h3,h4,h5,h6,pre,table,blockquote,aside,article,[role="article"],a[href]'
    );

    for (const el of nodes) {
      if (raw.length >= 128) break;
      if (!el || el.closest('#neuralia-split-scroll-rail,#neuralia-comp-controls')) continue;
      const css = getComputedStyle(el);
      if (css.display === 'none' || css.visibility === 'hidden') continue;
      const label = (el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 96);
      if (!label) continue;
      const kind = kindOf(el);
      const key = kind + ':' + label;
      if (seen.has(key)) continue;
      seen.add(key);
      raw.push({ el, kind, label, top:anchorTop(el, state) });
    }

    raw.sort((a, b) => a.top - b.top);
    if (raw.length <= 28) return raw;
    const sampled = [];
    for (let i = 0; i < 28; i++) {
      sampled.push(raw[Math.round(i * (raw.length - 1) / 27)]);
    }
    return sampled;
  }

  function semanticStep(direction) {
    semantic = semanticAnchors();
    if (!semantic.length) return false;
    const state = metrics();
    const pivot = state.top + Math.max(24, state.view * .24);
    let current = 0;
    for (let i = 0; i < semantic.length; i++) {
      if (semantic[i].top <= pivot) current = i;
      else break;
    }
    const target = Math.max(0, Math.min(semantic.length - 1, current + direction));
    if (target === current && ((direction < 0 && current === 0)
        || (direction > 0 && current === semantic.length - 1))) return false;
    scrollToPosition(semantic[target].top);
    return true;
  }

  const rail = document.createElement('div');
  rail.id = 'neuralia-split-scroll-rail';
  Object.assign(rail.style, {
    position:'fixed', top:'50%', right:'7px', transform:'translateY(-50%)',
    zIndex:'2147483646', pointerEvents:'auto', width:'44px',
    minHeight:'240px', maxHeight:'58vh',
    display:'flex', flexDirection:'column', alignItems:'center',
    justifyContent:'space-between', opacity:'.68',
    transition:'opacity .18s ease', fontFamily:'Segoe UI, system-ui, sans-serif'
  });
  rail.onmouseenter = () => { rail.style.opacity = '1'; };
  rail.onmouseleave = () => { rail.style.opacity = '.68'; };

  function arrow(symbol, title, direction) {
    const button = document.createElement('button');
    button.textContent = symbol;
    button.title = title;
    button.setAttribute('aria-label', title);
    Object.assign(button.style, {
      width: direction > 0 ? '42px' : '32px',
      height: direction > 0 ? '42px' : '28px',
      border: direction > 0 ? '1px solid rgba(255,255,255,.08)' : '0',
      borderRadius:'50%', padding:'0',
      background: direction > 0 ? 'rgba(38,38,38,.94)' : 'transparent',
      color: direction > 0 ? '#f4f4f4' : 'rgba(255,255,255,.46)',
      boxShadow: direction > 0 ? '0 6px 20px rgba(0,0,0,.28)' : 'none',
      fontSize:'21px', lineHeight: direction > 0 ? '38px' : '26px',
      cursor:'pointer'
    });
    button.onclick = (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (semanticStep(direction)) return;
      const state = metrics();
      scrollToPosition(state.top + Math.max(220, state.view * .82) * direction);
    };
    return button;
  }

  const ticks = document.createElement('div');
  Object.assign(ticks.style, {
    width:'34px', flex:'1', margin:'8px 0 10px',
    display:'flex', flexDirection:'column',
    justifyContent:'space-evenly', alignItems:'flex-end'
  });

  function rebuildTicks() {
    const state = metrics();
    semantic = semanticAnchors();
    const fallbackCount = Math.max(5, Math.min(11,
      Math.ceil((state.max + Math.max(state.view, 1)) / Math.max(state.view, 1))));
    const count = semantic.length || fallbackCount;
    const signature = semantic.length
      ? semantic.map((item) => item.kind + ':' + Math.round(item.top)).join('|')
      : 'fallback:' + count;
    if (ticks.dataset.signature === signature) return;
    ticks.dataset.signature = signature;
    ticks.textContent = '';

    for (let i = 0; i < count; i++) {
      const tick = document.createElement('button');
      tick.type = 'button';
      const item = semantic[i] || null;
      const label = item ? item.kind + ': ' + item.label : 'posição ' + (i + 1);
      tick.title = label;
      tick.setAttribute('aria-label', label);
      tick.dataset.top = item ? String(item.top) : '';
      tick.dataset.fraction = item ? '' : String(count <= 1 ? 0 : i / (count - 1));
      Object.assign(tick.style, {
        display:'block', height:'3px', width:i === 0 ? '30px' : '14px',
        minHeight:'3px', border:'0', borderRadius:'2px', padding:'0',
        background:'rgba(255,255,255,.30)', cursor:'pointer',
        transition:'width .16s ease, background .16s ease, opacity .16s ease'
      });
      tick.onclick = (event) => {
        event.preventDefault();
        event.stopPropagation();
        const top = Number(tick.dataset.top);
        if (tick.dataset.top) scrollToPosition(top);
        else scrollToPosition(metrics().max * Number(tick.dataset.fraction || 0));
      };
      ticks.appendChild(tick);
    }
  }

  function syncTicks() {
    rebuildTicks();
    const state = metrics();
    const children = Array.from(ticks.children);
    let active = 0;
    if (semantic.length) {
      const pivot = state.top + Math.max(24, state.view * .24);
      for (let i = 0; i < children.length; i++) {
        const top = Number(children[i].dataset.top || 0);
        if (top <= pivot) active = i;
        else break;
      }
    } else {
      const progress = state.max <= 0 ? 0 : Math.max(0, Math.min(1, state.top / state.max));
      active = Math.round(progress * Math.max(0, children.length - 1));
    }

    children.forEach((tick, i) => {
      const selected = i === active;
      tick.style.width = selected ? '32px' : (Math.abs(i - active) === 1 ? '22px' : '13px');
      tick.style.background = selected ? '#fff' : 'rgba(255,255,255,.32)';
      tick.style.opacity = selected ? '1' : (Math.abs(i - active) === 1 ? '.78' : '.55');
    });
  }

  rail.appendChild(arrow('⌃', 'Seção semântica anterior', -1));
  rail.appendChild(ticks);
  rail.appendChild(arrow('⌄', 'Próxima seção semântica', 1));
  document.documentElement.appendChild(rail);

  let raf = 0;
  const scheduleSync = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => { raf = 0; syncTicks(); });
  };
  window.addEventListener('scroll', scheduleSync, { passive:true });
  document.addEventListener('scroll', scheduleSync, { passive:true, capture:true });
  window.addEventListener('resize', scheduleSync, { passive:true });
  new MutationObserver(scheduleSync).observe(document.documentElement, {
    childList:true, subtree:true
  });
  syncTicks();
});
})();
"#;

/// Tudo o que corre depois do DOMContentLoaded usa as capturas do topo: a
/// pagina ja correu nessa altura e pode ter trocado qualquer global. Os
/// botoes que levam o token ouvem por addEventListener, nao por `onclick`,
/// para a pagina nao poder ler o handler do elemento e chama-lo a mao.
pub(in crate::windows_app) const COMPARATOR_INJECT_SCRIPT: &str = r#"
(function () {
  if (window.top !== window) return;
  const colIndex = window.__neuralia_col_index ?? 0;
  const colName = window.__neuralia_col_name ?? 'IA';
  const capability = '__NEURALIA_CAP__';
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  const stringify = JSON.stringify;
  // O envelope com o token e montado com primitivas. Serializar um objeto
  // que contem o token faz o serializador consultar toJSON pela cadeia de
  // prototipos, que a pagina controla: um getter dela recebia o envelope
  // como `this` e lia `cap`. Strings nao passam por toJSON.
  function envelope(action, args) {
    return '{"v":1,"cap":"' + capability + '","action":' + stringify(action)
      + ',"args":' + stringify(args || {}) + '}';
  }
  const defer = setTimeout;
  const cancelDefer = clearTimeout;
  function act(action, args) {
    post(envelope(action, args));
  }
  const byId = document.getElementById.bind(document);
  const createElement = document.createElement.bind(document);
  const assign = Object.assign;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
  const append = Function.prototype.call.bind(Node.prototype.appendChild);

  // Semantica do Chrome: clique abre onde se esta, Ctrl+clique (ou clique do
  // meio) abre "noutro separador" -- aqui, o painel lateral, com a aba na
  // barra de titulo.
  //
  // Os listeners ficam registados JA, fora do DOMContentLoaded. Os scripts da
  // propria pagina correm durante o parse, ou seja antes desse evento, e
  // registam os deles em captura primeiro; quem chega depois recebe os
  // eventos ja com `defaultPrevented` posto e desiste sem fazer nada.
  const GOOGLE_REDIRECT_PARAMS = ['q', 'url', 'imgurl', 'adurl'];
  const LINK_SELECTOR = 'a[href],area[href],[role="link"],[data-href],[data-url]';

  function linkNodeFromEvent(event) {
    // React/Shadow DOM pode retargetear event.target para um host que nao e o
    // <a> real. composedPath devolve o caminho original atraves das sombras.
    const path = typeof event.composedPath === 'function'
      ? event.composedPath()
      : [event.target];

    for (const candidate of path) {
      if (!candidate || candidate === window || candidate === document) continue;
      if (candidate.matches && candidate.matches(LINK_SELECTOR)) return candidate;
      if (candidate.closest) {
        const found = candidate.closest(LINK_SELECTOR);
        if (found) return found;
      }
    }
    return null;
  }

  function neuraliaControlFromEvent(event) {
    const path = typeof event.composedPath === 'function'
      ? event.composedPath()
      : [event.target];
    return path.some((candidate) => candidate && candidate.closest
      && candidate.closest('#neuralia-comp-controls,#neuralia-palette'));
  }

  function linkUrl(anchor) {
    if (!anchor) return null;

    // href absoluto do DOM ganha de getAttribute: sites React podem montar a
    // URL relativa e trocar <base>. data-* cobre chips de fonte sem <a>.
    const raw = (typeof anchor.href === 'string' && anchor.href)
      || anchor.getAttribute('href')
      || anchor.getAttribute('data-href')
      || anchor.getAttribute('data-url');
    if (!raw) return null;

    let target;
    try { target = new URL(raw, location.href); } catch (_) { return null; }
    if (target.protocol !== 'http:' && target.protocol !== 'https:') return null;

    // O Google embrulha as fontes em /url, /imgres, /aclk etc. O parametro
    // revela o destino real sem depender de um path especifico.
    const host = target.hostname.toLowerCase();
    if (host === 'google.com' || host.endsWith('.google.com')) {
      for (const name of GOOGLE_REDIRECT_PARAMS) {
        const actual = target.searchParams.get(name);
        if (!actual) continue;
        try {
          // So URL absoluta: em /search o `q` e um termo, e resolvido contra a
          // coluna virava uma URL falsa na origem da IA.
          const unwrapped = new URL(actual);
          if (unwrapped.protocol === 'http:' || unwrapped.protocol === 'https:') {
            target = unwrapped;
            break;
          }
        } catch (_) {}
      }
    }
    return target;
  }

  function routeLink(event, aside) {
    if (!event.isTrusted) return false;
    // Alt e Shift continuam reservados ao comportamento nativo do navegador.
    if (event.altKey || event.shiftKey) return false;
    if (neuraliaControlFromEvent(event)) return false;

    const anchor = linkNodeFromEvent(event);
    const target = linkUrl(anchor);
    if (!target) return false;

    // Navegacao interna da propria IA continua com a SPA para nao perder a
    // conversa. Ctrl/meta/meio e links externos sao assumidos pelo NeuralIA.
    if (!aside && target.origin === location.origin) return false;

    // Capturamos no WINDOW, antes de handlers de document/React. Depois que o
    // NeuralIA assume o clique, nenhum listener concorrente pode navegar a
    // coluna ao mesmo tempo e criar click duplo/race com o IPC.
    event.preventDefault();
    event.stopImmediatePropagation();
    act('link', { col:colIndex, url:target.href, aside:aside });
    return true;
  }

  // UM so listener de clique no window (o gate scripts/test-link-routing.mjs
  // exige-o): primeiro o link; se nao era link, o botao de enviar da IA.
  listen(window, 'click', (event) => {
    if (event.button !== 0) return;
    if (routeLink(event, !!(event.ctrlKey || event.metaKey))) return;
    askFromSendButton(event);
  }, true);

  // O botao do meio usa auxclick. Captura no window pela mesma razao: sites de
  // IA costumam instalar handlers de document que chamam window.open primeiro.
  listen(window, 'auxclick', (event) => {
    if (event.button !== 1) return;
    routeLink(event, true);
  }, true);

  // Pergunta escrita numa coluna tambem pesquisa nas outras (pedido do
  // dono: "escrever pesquisar, tem que pesquisar em todos tambem").
  //  - Na pagina da propria IA (Google IA, ChatGPT, Claude, Gemini): o texto
  //    enviado com Enter ou com o botao de enviar vai as OUTRAS colunas
  //    ('ask'); esta segue a conversa dela.
  //  - Num site aberto por um link (as colunas no mesmo site): a pesquisa GET
  //    desse site abre o resultado em todas ('link'), tal como o clique.
  //  POST, senhas, e-mails e eventos sinteticos nunca sao replicados.
  const ASK_MAX = 2000;
  const ASK_REPEAT_MS = 2000;
  const SEND_LABEL = /\bsend\b|enviar|submit/i;
  const clock = Date.now;
  const toArray = Array.from;
  let lastAsk = { text: '', at: 0 };
  let lastComposer = null;

  function onProviderPage() {
    let here;
    try { here = new URL(location.href); } catch (_) { return false; }
    const host = here.hostname.toLowerCase();
    if (host === 'chatgpt.com' || host.endsWith('.chatgpt.com') || host === 'chat.openai.com') return true;
    if (host === 'claude.ai' || host.endsWith('.claude.ai')) return true;
    if (host === 'gemini.google.com') return true;
    return (host === 'google.com' || host.endsWith('.google.com'))
      && here.searchParams.get('udm') === '50';
  }

  // Texto de uma caixa onde se escreve uma pergunta; null para tudo o resto
  // (senhas, e-mails, codigos, botoes...).
  function composerText(node) {
    if (!node || !node.tagName) return null;
    const tag = String(node.tagName).toUpperCase();
    if (tag === 'TEXTAREA') return String(node.value || '');
    if (tag === 'INPUT') {
      const type = String(node.type || 'text').toLowerCase();
      if (type !== 'text' && type !== 'search') return null;
      const auto = String((node.getAttribute && node.getAttribute('autocomplete')) || '').toLowerCase();
      if (/user|mail|pass|code|tel|cc-/.test(auto)) return null;
      return String(node.value || '');
    }
    if (node.isContentEditable) return String(node.innerText || node.textContent || '');
    return null;
  }

  function pathOf(event) {
    return typeof event.composedPath === 'function' ? event.composedPath() : [event.target];
  }

  function composerFromEvent(event) {
    for (const candidate of pathOf(event)) {
      if (composerText(candidate) !== null) return candidate;
    }
    return null;
  }

  function sendAsk(text) {
    const clean = String(text || '').trim();
    if (!clean || clean.length > ASK_MAX) return;
    const at = clock();
    // Enter e o submit do mesmo formulario chegam os dois: uma pergunta so.
    if (clean === lastAsk.text && at - lastAsk.at < ASK_REPEAT_MS) return;
    lastAsk = { text: clean, at: at };
    act('ask', { col:colIndex, text:clean });
  }

  // Um GET de um formulario com texto escrito -> a URL que ele abriria.
  function formSearchUrl(form, submitter) {
    const method = String((form.getAttribute && form.getAttribute('method')) || 'get').toLowerCase();
    if (method !== 'get') return null;
    let target;
    try {
      target = new URL((form.getAttribute && form.getAttribute('action')) || location.href, location.href);
    } catch (_) { return null; }
    if (target.protocol !== 'http:' && target.protocol !== 'https:') return null;
    target.search = '';
    let typed = false;
    for (const field of toArray(form.elements || [])) {
      if (!field || !field.name || field.disabled) continue;
      const type = String(field.type || '').toLowerCase();
      if (type === 'password' || type === 'file' || type === 'email') return null;
      if ((type === 'checkbox' || type === 'radio') && !field.checked) continue;
      if ((type === 'submit' || type === 'button' || type === 'image' || type === 'reset')
          && field !== submitter) continue;
      const value = String(field.value == null ? '' : field.value);
      if ((type === 'search' || type === 'text' || type === 'textarea') && value.trim()) typed = true;
      target.searchParams.append(String(field.name), value);
    }
    return typed ? target : null;
  }

  listen(window, 'focusin', (event) => {
    if (!event.isTrusted) return;
    const node = composerFromEvent(event);
    if (node) lastComposer = node;
  }, true);

  listen(window, 'keydown', (event) => {
    if (!event.isTrusted || event.key !== 'Enter') return;
    // Shift+Enter e quebra de linha; durante a composicao (acentos, IME) o
    // Enter ainda nao e envio.
    if (event.shiftKey || event.ctrlKey || event.altKey || event.metaKey || event.isComposing) return;
    if (!onProviderPage() || neuraliaControlFromEvent(event)) return;
    const node = composerFromEvent(event);
    if (node) sendAsk(composerText(node));
  }, true);

  // Chamado pelo listener de clique do window, depois do encaminhamento de
  // links.
  function askFromSendButton(event) {
    if (!event.isTrusted || event.button !== 0 || !onProviderPage()) return;
    if (neuraliaControlFromEvent(event) || !lastComposer) return;
    const button = pathOf(event).find((node) => node && node.tagName
      && (String(node.tagName).toUpperCase() === 'BUTTON'
        || (node.getAttribute && node.getAttribute('role') === 'button')));
    if (!button || !button.getAttribute) return;
    const label = [
      button.getAttribute('aria-label'),
      button.getAttribute('data-testid'),
      button.getAttribute('title')
    ].join(' ');
    if (SEND_LABEL.test(label)) sendAsk(composerText(lastComposer));
  }

  listen(window, 'submit', (event) => {
    if (!event.isTrusted) return;
    const form = event.target;
    if (!form || !form.tagName || String(form.tagName).toUpperCase() !== 'FORM') return;
    if (onProviderPage()) {
      for (const field of toArray(form.elements || [])) {
        const text = composerText(field);
        if (text && text.trim()) { sendAsk(text); return; }
      }
      return;
    }
    const target = formSearchUrl(form, event.submitter || null);
    if (!target) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    act('link', { col:colIndex, url:target.href, aside:false });
  }, true);

  // Duplo clique numa palavra seleciona-a, e quem responde e a barra de
  // selecao (Pesquisar/Copiar/Falar). Expandir a coluna redimensionava o
  // WebView e o 'resize' levava a barra: so expande o duplo clique que nao
  // deixa texto selecionado. Primitivas capturadas aqui, no document-created.
  const textSelected = (function () {
    try {
      const current = Function.prototype.call.bind(Document.prototype.getSelection);
      const collapsed = Function.prototype.call.bind(
        Object.getOwnPropertyDescriptor(Selection.prototype, 'isCollapsed').get);
      const textOf = Function.prototype.call.bind(Selection.prototype.toString);
      const trim = Function.prototype.call.bind(String.prototype.trim);
      return function () {
        try {
          const selection = current(document);
          return !!selection && !collapsed(selection) && trim('' + textOf(selection)) !== '';
        } catch (_) { return false; }
      };
    } catch (_) {
      return function () { return false; };
    }
  })();

  listen(document, 'dblclick', (event) => {
    if (!event.isTrusted || event.defaultPrevented) return;
    if (event.target && event.target.closest
        && event.target.closest(
          '#neuralia-comp-controls,#neuralia-palette,a[href],button,input,textarea,select,option,label,summary,[role="button"],[role="link"],[contenteditable="true"]'
        )) return;
    const tag = event.target && event.target.tagName
      ? event.target.tagName.toUpperCase() : '';
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (event.target && event.target.isContentEditable) return;
    if (textSelected()) return;
    act('expand', { col:colIndex });
  }, true);

  listen(document, 'DOMContentLoaded', () => {
    let researchAnswerTimer = 0;
    let lastResearchAnswer = '';

    function researchAnswerText() {
      const preferred = Array.from(document.querySelectorAll(
        '[data-message-author-role="assistant"],'
        + '[data-testid*="assistant"],[class*="assistant"],article,[role="article"]'
      ));
      let best = '';
      for (const element of preferred) {
        if (!element || (element.closest && element.closest('#neuralia-comp-controls,#neuralia-palette'))) {
          continue;
        }
        const text = (element.innerText || element.textContent || '')
          .replace(/\s+/g, ' ')
          .trim();
        if (text.length > best.length) best = text;
      }
      if (!best) {
        const main = document.querySelector('main,[role="main"]');
        best = main ? (main.innerText || main.textContent || '').replace(/\s+/g, ' ').trim() : '';
      }
      return best.slice(0, 1800);
    }

    function scheduleResearchAnswer() {
      cancelDefer(researchAnswerTimer);
      researchAnswerTimer = defer(() => {
        const text = researchAnswerText();
        if (text.length < 24 || text === lastResearchAnswer) return;
        lastResearchAnswer = text;
        act('research-answer', { col:colIndex, text });
      }, 1800);
    }

    function mountControls() {
      if (byId('neuralia-comp-controls')) return;

      const style = createElement('style');
      style.id = 'neuralia-scroll-style';
      style.textContent = [
        'html,body,.neuralia-scroll-root{scrollbar-width:none!important;-ms-overflow-style:none!important;}',
        'html::-webkit-scrollbar,body::-webkit-scrollbar,.neuralia-scroll-root::-webkit-scrollbar{width:0!important;height:0!important;display:none!important;}'
      ].join('');
      append(document.documentElement, style);

      let currentRoot = null;
      function scrollRoot() {
        const docRoot = document.scrollingElement || document.documentElement || document.body;
        const candidates = docRoot ? [docRoot] : [];
        document.querySelectorAll(
          'main,[role="main"],[class*="scroll"],[class*="overflow"],[style*="overflow"]'
        ).forEach((el) => candidates.push(el));

        let best = docRoot;
        let bestRange = best ? Math.max(0, best.scrollHeight - best.clientHeight) : 0;
        for (const el of candidates) {
          if (!el || el === document.body) continue;
          const range = Math.max(0, el.scrollHeight - el.clientHeight);
          if (range <= bestRange + 24) continue;
          const css = getComputedStyle(el);
          if (css.overflowY === 'hidden' || css.display === 'none') continue;
          best = el;
          bestRange = range;
        }
        if (currentRoot && currentRoot !== best && currentRoot.classList) {
          currentRoot.classList.remove('neuralia-scroll-root');
        }
        currentRoot = best || docRoot;
        if (currentRoot && currentRoot.classList) currentRoot.classList.add('neuralia-scroll-root');
        return currentRoot;
      }

      function metrics() {
        const root = scrollRoot();
        if (!root) return { root:null, top:0, max:0, docLike:true };
        const docLike = root === document.scrollingElement
          || root === document.documentElement || root === document.body;
        return {
          root,
          docLike,
          top: docLike ? window.scrollY : root.scrollTop,
          max: Math.max(0, root.scrollHeight - root.clientHeight)
        };
      }

      function scrollToPosition(top) {
        const state = metrics();
        const value = Math.max(0, Math.min(state.max, top));
        if (state.docLike) window.scrollTo({ top:value, behavior:'smooth' });
        else if (state.root) state.root.scrollTo({ top:value, behavior:'smooth' });
      }

      const controls = createElement('div');
      controls.id = 'neuralia-comp-controls';
      assign(controls.style, {
        position:'fixed', inset:'0', zIndex:'2147483647',
        pointerEvents:'none', fontFamily:'Segoe UI, system-ui, sans-serif'
      });

      const expand = createElement('button');
      expand.id = 'neuralia-comp-expand';
      expand.textContent = '⛶ ' + colName;
      assign(expand.style, {
        position:'absolute', top:'10px', right:'10px',
        pointerEvents:'auto', border:'1px solid rgba(255,255,255,.12)',
        borderRadius:'999px', padding:'6px 11px', background:'rgba(17,19,20,.90)',
        color:'#fff', fontSize:'11px', fontWeight:'600',
        boxShadow:'0 5px 18px rgba(0,0,0,.28)', cursor:'pointer'
      });
      listen(expand, 'click', (event) => {
        if (!event.isTrusted) return;
        event.preventDefault(); event.stopPropagation();
        act('hint', { col:colIndex, id:'none' });
        act('expand', { col:colIndex });
      });

      const minimize = createElement('button');
      minimize.id = 'neuralia-comp-minimize';
      minimize.textContent = '−';
      // A dica e a centrada do app (canal 'hint'), nao o title do browser:
      // com os dois, apareciam duas dicas diferentes ao mesmo tempo.
      minimize.ariaLabel = 'Minimizar ' + colName;
      assign(minimize.style, {
        position:'absolute', top:'10px', right:'112px',
        pointerEvents:'auto', width:'30px', height:'28px',
        border:'1px solid rgba(255,255,255,.12)',
        borderRadius:'999px', padding:'0',
        background:'rgba(17,19,20,.90)', color:'#fff',
        fontSize:'18px', fontWeight:'600', lineHeight:'24px',
        boxShadow:'0 5px 18px rgba(0,0,0,.28)', cursor:'pointer'
      });
      listen(minimize, 'click', (event) => {
        if (!event.isTrusted) return;
        event.preventDefault(); event.stopPropagation();
        act('hint', { col:colIndex, id:'none' });
        act('minimize', { col:colIndex });
      });

      // Passar o rato pelo "−" e pelo "⛶ <IA>" mostra a dica centrada do
      // app, a mesma da barra. So eventos do utilizador (isTrusted); a pagina
      // escolhe apenas QUAL das dicas, o texto e o nome vem do nativo.
      function hintOn(button, id) {
        listen(button, 'mouseenter', (event) => {
          if (!event.isTrusted) return;
          act('hint', { col:colIndex, id:id });
        });
        listen(button, 'mouseleave', (event) => {
          if (!event.isTrusted) return;
          act('hint', { col:colIndex, id:'none' });
        });
      }
      hintOn(minimize, 'minimize');
      hintOn(expand, 'expand');

      const rail = createElement('div');
      rail.id = 'neuralia-response-rail';
      assign(rail.style, {
        position:'absolute', top:'50%', right:'7px', transform:'translateY(-50%)',
        pointerEvents:'auto', width:'44px', minHeight:'240px', maxHeight:'58vh',
        display:'flex', flexDirection:'column', alignItems:'center',
        justifyContent:'space-between', opacity:'.68',
        transition:'opacity .18s ease'
      });
      rail.onmouseenter = () => { rail.style.opacity = '1'; };
      rail.onmouseleave = () => { rail.style.opacity = '.68'; };

      function arrow(symbol, title, direction) {
        const button = createElement('button');
        button.textContent = symbol;
        button.title = title;
        assign(button.style, {
          width: direction > 0 ? '42px' : '32px',
          height: direction > 0 ? '42px' : '28px',
          border: direction > 0 ? '1px solid rgba(255,255,255,.08)' : '0',
          borderRadius:'50%', padding:'0',
          background: direction > 0 ? 'rgba(38,38,38,.94)' : 'transparent',
          color: direction > 0 ? '#f4f4f4' : 'rgba(255,255,255,.46)',
          boxShadow: direction > 0 ? '0 6px 20px rgba(0,0,0,.28)' : 'none',
          fontSize:'21px', lineHeight: direction > 0 ? '38px' : '26px',
          cursor:'pointer'
        });
        button.onclick = (event) => {
          event.preventDefault(); event.stopPropagation();
          if (semanticStep(direction)) return;
          const state = metrics();
          const view = state.root ? state.root.clientHeight : window.innerHeight;
          scrollToPosition(state.top + Math.max(220, view * .82) * direction);
        };
        return button;
      }

      const ticks = createElement('div');
      ticks.id = 'neuralia-response-ticks';
      assign(ticks.style, {
        width:'34px', flex:'1', margin:'8px 0 10px', display:'flex',
        flexDirection:'column', justifyContent:'space-evenly',
        alignItems:'flex-end', cursor:'pointer'
      });

      let semantic = [];

      function anchorTop(el, state) {
        const rect = el.getBoundingClientRect();
        if (state.docLike) return state.top + rect.top;
        const rootRect = state.root.getBoundingClientRect();
        return state.top + rect.top - rootRect.top;
      }

      function semanticKind(el) {
        const role = el.getAttribute('data-message-author-role');
        const tag = el.tagName ? el.tagName.toLowerCase() : '';
        const text = (el.textContent || '').trim().toLowerCase();
        if (role === 'user') return 'pergunta';
        if (role === 'assistant') return 'resposta';
        if (/^h[1-6]$/.test(tag)) return text.includes('conclus') ? 'conclusão' : 'seção';
        if (tag === 'pre' || tag === 'code') return 'código';
        if (tag === 'table') return 'tabela';
        if (tag === 'blockquote') return 'citação';
        if (tag === 'aside') return 'nota';
        if (tag === 'a') return 'fonte';
        return 'resposta';
      }

      function semanticAnchors() {
        const state = metrics();
        const raw = [];
        const seen = new Set();
        const nodes = document.querySelectorAll(
          '[data-message-author-role="user"],[data-message-author-role="assistant"],'
          + 'h1,h2,h3,h4,h5,h6,pre,table,blockquote,aside,article,[role="article"],a[href]'
        );
        for (const el of nodes) {
          if (raw.length >= 128) break;
          if (!el || (el.closest && el.closest('#neuralia-comp-controls,#neuralia-palette'))) continue;
          const css = getComputedStyle(el);
          if (css.display === 'none' || css.visibility === 'hidden') continue;
          const label = (el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 96);
          if (!label) continue;
          const kind = semanticKind(el);
          const key = kind + ':' + label;
          if (seen.has(key)) continue;
          seen.add(key);
          raw.push({ el, kind, label, top:anchorTop(el, state) });
        }
        raw.sort((a, b) => a.top - b.top);
        if (raw.length <= 28) return raw;
        const sampled = [];
        for (let i = 0; i < 28; i++) {
          sampled.push(raw[Math.round(i * (raw.length - 1) / 27)]);
        }
        return sampled;
      }

      function semanticStep(direction) {
        semantic = semanticAnchors();
        if (!semantic.length) return false;
        const state = metrics();
        const view = state.root ? state.root.clientHeight : window.innerHeight;
        const pivot = state.top + Math.max(24, view * .24);
        let current = 0;
        for (let i = 0; i < semantic.length; i++) {
          if (semantic[i].top <= pivot) current = i;
          else break;
        }
        const target = Math.max(0, Math.min(semantic.length - 1, current + direction));
        if (target === current && ((direction < 0 && current === 0)
            || (direction > 0 && current === semantic.length - 1))) return false;
        scrollToPosition(semantic[target].top);
        return true;
      }

      function rebuildTicks() {
        const state = metrics();
        const view = state.root ? state.root.clientHeight : window.innerHeight;
        semantic = semanticAnchors();
        const fallbackCount = Math.max(5, Math.min(11,
          Math.ceil((state.max + Math.max(view, 1)) / Math.max(view, 1))));
        const count = semantic.length || fallbackCount;
        const signature = semantic.length
          ? semantic.map((item) => item.kind + ':' + Math.round(item.top)).join('|')
          : 'fallback:' + count;
        if (ticks.dataset.signature === signature) return;
        ticks.dataset.signature = signature;
        ticks.textContent = '';

        for (let i = 0; i < count; i++) {
          const tick = createElement('button');
          const item = semantic[i] || null;
          const label = item ? item.kind + ': ' + item.label : 'posição ' + (i + 1);
          tick.type = 'button';
          tick.title = label;
          tick.ariaLabel = label;
          tick.dataset.top = item ? String(item.top) : '';
          tick.dataset.fraction = item ? '' : String(count <= 1 ? 0 : i / (count - 1));
          assign(tick.style, {
            display:'block', height:'3px', minHeight:'3px',
            width:i === 0 ? '30px' : '14px', border:'0', borderRadius:'2px',
            padding:'0', background:'rgba(255,255,255,.30)', cursor:'pointer',
            transition:'width .16s ease, background .16s ease, opacity .16s ease'
          });
          tick.onclick = (event) => {
            event.preventDefault(); event.stopPropagation();
            if (tick.dataset.top) scrollToPosition(Number(tick.dataset.top));
            else scrollToPosition(metrics().max * Number(tick.dataset.fraction || 0));
          };
          append(ticks, tick);
        }
      }

      function syncTicks() {
        rebuildTicks();
        const state = metrics();
        const view = state.root ? state.root.clientHeight : window.innerHeight;
        const children = Array.from(ticks.children);
        let active = 0;

        if (semantic.length) {
          const pivot = state.top + Math.max(24, view * .24);
          for (let i = 0; i < children.length; i++) {
            const top = Number(children[i].dataset.top || 0);
            if (top <= pivot) active = i;
            else break;
          }
        } else {
          const progress = state.max <= 0 ? 0 : Math.max(0, Math.min(1, state.top / state.max));
          active = Math.round(progress * Math.max(0, children.length - 1));
        }

        children.forEach((tick, i) => {
          const selected = i === active;
          tick.style.width = selected ? '32px' : (Math.abs(i - active) === 1 ? '22px' : '13px');
          tick.style.background = selected ? '#fff' : 'rgba(255,255,255,.32)';
          tick.style.opacity = selected ? '1' : (Math.abs(i - active) === 1 ? '.78' : '.55');
        });
      }

      append(rail, arrow('⌃', 'Resposta anterior', -1));
      append(rail, ticks);
      append(rail, arrow('⌄', 'Próxima resposta', 1));
      append(controls, minimize);
      append(controls, expand);
      append(controls, rail);
      append(document.documentElement, controls);

      let raf = 0;
      const scheduleSync = () => {
        if (raf) return;
        raf = requestAnimationFrame(() => { raf = 0; syncTicks(); });
      };
      listen(window, 'scroll', scheduleSync, { passive:true });
      listen(document, 'scroll', scheduleSync, { passive:true, capture:true });
      listen(window, 'resize', scheduleSync, { passive:true });
      new MutationObserver(() => {
        scheduleSync();
        scheduleResearchAnswer();
      }).observe(document.documentElement, {
        childList:true, subtree:true
      });
      syncTicks();
      scheduleResearchAnswer();
    }

    mountControls();
    new MutationObserver(() => {
      if (!byId('neuralia-comp-controls')) mountControls();
    }).observe(document.documentElement, { childList:true, subtree:true });

  });
})();
"#;
