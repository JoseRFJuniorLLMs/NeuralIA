use super::*;

use std::collections::BTreeMap;
use std::sync::{Mutex, RwLock};

use neural_core::distraction::{
    DistractionPolicy, DistractionSurface, distraction_config, distraction_site, script_config,
};

// ===================== anti-distracao (plano 2.4, §7) =====================
//
// Modulo de feature: `UserEvent::Distraction(DistractionEvent)`, o braco
// `distraction_event` no event loop, o slot `distraction` da tabela dos
// ganchos (`webview_hooks.rs`) e o item «Ocultar distrações neste site» do
// botao direito. A politica e do bloqueio de anuncios: vive no campo
// `distraction` do `adblock-settings.json` (loja `Setting`, aberta pelo
// grant do `AdblockState`); as escolhas feitas no Split privado ficam so em
// memoria e nunca chegam ao ficheiro.
//
// O script (`NEURALIA_DISTRACTION_SCRIPT`, um script injetado NOVO -- §7):
//
// - so no documento de topo, e so nas colunas, na fonte ao lado (normal ou
//   privada) e na Web completa -- a tabela liga-o nesses hospedeiros e em
//   nenhum outro (servicos, Leitor, PDF, livros, paineis, Home);
// - leva gravada a politica e as regras do registo dos provedores
//   (`neural_core::distraction::script_config`): a decisao por site e no
//   inicio do documento, com o endereco dele, e repete a do
//   `distraction_config` (nunca nas IAs, nos logins, nas nossas origens,
//   nem num site desligado -- ai nao regista nada);
// - captura no document-created, antes de qualquer script da pagina, cada
//   primitiva que usa (querySelectorAll, click, getComputedStyle,
//   MutationObserver, as strings...): uma pagina que as troque depois nao
//   muda o que ele ve nem o que ele faz;
// - num CMP conhecido so age com o aviso VISIVEL agora (um centro de
//   preferencias escondido, de quem ja respondeu, nao e um aviso) e so
//   esconde o aviso, nunca o anfitriao que o CMP reusa para as definicoes
//   do rodape; so CLICA num botao visivel, quando o seletor de recusa casa
//   E o texto dele tem uma frase de recusa E nenhuma palavra de aceitar;
//   de resto so esconde (`display: none`);
// - uma moldura de outra origem (Sourcepoint, TrustArc) nunca e clicada
//   nem lida: a que tapa a pagina (metade da altura ou mais) pode ser um
//   "pague ou aceite" e fica, e com ela a rolagem e os fundos ficam como a
//   pagina os pos; a pequena fica numa pagina presa e, numa pagina que
//   rola, sai sem mexer na rolagem nem nos fundos;
// - esconde avisos de cookies, janelas de newsletter (no id/class; pelo
//   texto so um dialogo cujo unico campo e um e-mail) e barras fixas na
//   janela, encostadas ao topo ou ao fundo, com pelo menos 25% da altura
//   dela; o que aparece ate 1,5 s depois de um clique ou de uma tecla do
//   utilizador (ele e que o abriu) ou tem um campo de senha fica;
// - um paywall (marcador no id/class ou no texto de um candidato ou de um
//   aviso) fica, e depois de o ver o script nunca mais devolve a rolagem
//   nem esconde um fundo nessa pagina (e desfaz a rolagem que ja tinha
//   devolvido);
// - devolve a rolagem so quando escondeu a causa e o documento rola mesmo;
//   um fundo vazio de ecra inteiro so sai ao lado do que escondeu (nunca
//   uma moldura, canvas ou video da pagina);
// - o exame dos elementos cede a vez depois de 8 ms; a procura dos CMPs
//   (umas consultas ao documento inteiro) corre no maximo a cada 250 ms;
//   de cada elemento acrescentado olha no maximo 60 descendentes; para de
//   observar a pagina depois de 30 s sem mudancas (uma pagina que nunca
//   para de mudar e observada enquanto estiver aberta);
// - nunca usa o `postMessage` e nunca recebe a capability do canal.
//
// A ligacao a WebView: o `AddScriptToExecuteOnDocumentCreated` do COM (e
// nao o `with_initialization_script` do wry) porque devolve um id, e uma
// escolha no menu volta a ligar o script com a politica nova
// (`distraction_rebind`: tira o anterior pelo id, poe o novo) e recarrega a
// pagina onde a escolha foi feita. `DistractionSlot` garante que um script
// cujo registo ainda nao acabou quando chega outro e tirado quando acaba.
// O registo e assincrono e entra depois do `build` (a metade COM dos
// ganchos): o primeiro documento, que o builder ja pediu, pode nascer
// antes dele e ficar sem o script. So se perde o esconder dessa pagina --
// o script que entra ja traz a politica de agora, e nenhuma escolha
// desligada fica para tras.

/// O que o texto do script tem no lugar da configuracao gravada.
pub(in crate::windows_app) const DISTRACTION_CONFIG_PLACEHOLDER: &str =
    "__NEURALIA_DISTRACTION_CONFIG__";

/// O script contra distracoes (§7). Ver o cabecalho do modulo.
pub(in crate::windows_app) const NEURALIA_DISTRACTION_SCRIPT: &str = r##"(function () {
  'use strict';
  if (window.top !== window) return;
  var W = window;
  var C = __NEURALIA_DISTRACTION_CONFIG__;

  // ---- primitivas, capturadas no document-created (antes da pagina) ----
  var FP = Function.prototype;
  var uncurry = FP.bind.bind(FP.call);
  var gOPD = Object.getOwnPropertyDescriptor;
  var create = Object.create;
  function getter(proto, name) {
    var d = proto ? gOPD(proto, name) : null;
    return d && d.get ? uncurry(d.get) : null;
  }
  var D = W.document;
  var DocP = W.Document.prototype, ElP = W.Element.prototype;
  var HtmlP = W.HTMLElement.prototype, NodeP = W.Node.prototype;
  var qsaDoc = uncurry(DocP.querySelectorAll);
  var qsaEl = uncurry(ElP.querySelectorAll);
  var qsaFrag = uncurry(W.DocumentFragment.prototype.querySelectorAll);
  var nlLength = getter(W.NodeList.prototype, 'length');
  var nlItem = uncurry(W.NodeList.prototype.item);
  var shadowOf = getter(ElP, 'shadowRoot');
  var textOf = getter(HtmlP, 'innerText');
  var styleOf = getter(HtmlP, 'style');
  var getAttr = uncurry(ElP.getAttribute);
  var tagOf = getter(ElP, 'localName');
  var nodeTypeOf = getter(NodeP, 'nodeType');
  var parentOf = getter(NodeP, 'parentNode');
  var firstChildOf = getter(ElP, 'firstElementChild');
  var nextOf = getter(ElP, 'nextElementSibling');
  var containsNode = uncurry(NodeP.contains);
  var docElOf = getter(DocP, 'documentElement');
  var bodyOf = getter(DocP, 'body');
  var readyOf = getter(DocP, 'readyState');
  var activeOf = getter(DocP, 'activeElement');
  var scrollHOf = getter(ElP, 'scrollHeight');
  var clientHOf = getter(ElP, 'clientHeight');
  var clientWOf = getter(ElP, 'clientWidth');
  var rectOf = uncurry(ElP.getBoundingClientRect);
  var RectP = W.DOMRectReadOnly.prototype;
  var rTop = getter(RectP, 'top'), rBottom = getter(RectP, 'bottom');
  var rWidth = getter(RectP, 'width'), rHeight = getter(RectP, 'height');
  var clickEl = uncurry(HtmlP.click);
  var CssP = W.CSSStyleDeclaration.prototype;
  var setProp = uncurry(CssP.setProperty);
  var getProp = uncurry(CssP.getPropertyValue);
  var getPri = uncurry(CssP.getPropertyPriority);
  var removeProp = uncurry(CssP.removeProperty);
  var computed = uncurry(W.getComputedStyle);
  var listen = uncurry(W.EventTarget.prototype.addEventListener);
  var MO = W.MutationObserver;
  var moObserve = uncurry(MO.prototype.observe);
  var moDisconnect = uncurry(MO.prototype.disconnect);
  var addedOf = getter(W.MutationRecord.prototype, 'addedNodes');
  var later = uncurry(W.setTimeout);
  var perf = W.performance;
  var perfNow = uncurry(W.Performance.prototype.now);
  var WS = W.WeakSet;
  var wsAdd = uncurry(WS.prototype.add), wsHas = uncurry(WS.prototype.has);
  var lower = uncurry(String.prototype.toLowerCase);
  var upper = uncurry(String.prototype.toUpperCase);
  var indexOf = uncurry(String.prototype.indexOf);
  var slice = uncurry(String.prototype.slice);
  var hasOwn = uncurry(Object.prototype.hasOwnProperty);

  // ---- a decisao por site, no inicio do documento ----
  var loc = W.location;
  var protocol = loc.protocol;
  if (protocol !== 'http:' && protocol !== 'https:') return;
  var host = lower('' + (loc.hostname || ''));
  if (host.length && host[host.length - 1] === '.') host = slice(host, 0, host.length - 1);
  function endsWith(text, suffix) {
    return text.length >= suffix.length && slice(text, text.length - suffix.length) === suffix;
  }
  function numeric(label) {
    if (!label.length) return false;
    for (var i = 0; i < label.length; i++) {
      if (label[i] < '0' || label[i] > '9') return false;
    }
    return true;
  }
  if (!host || host === 'localhost' || endsWith(host, '.localhost') || host[0] === '[') return;
  var lastDot = -1;
  for (var h = 0; h < host.length; h++) if (host[h] === '.') lastDot = h;
  if (lastDot < 0 || numeric(slice(host, lastDot + 1))) return;
  var udm = null;
  try { udm = new W.URLSearchParams(loc.search).get('udm'); } catch (e) { udm = null; }
  for (var n = 0; n < C.never.length; n++) {
    var rule = C.never[n];
    var hit = host === rule[0] ||
      (rule[1] === true && host.length > rule[0].length + 1 && endsWith(host, '.' + rule[0]));
    if (hit && (rule[2] !== true || udm === '50')) return;
  }
  for (var l = 0; l < C.login.length; l++) {
    var login = C.login[l];
    if (host === login || endsWith(host, '.' + login)) return;
  }
  var site = host;
  if (indexOf(host, 'www.') === 0 && indexOf(slice(host, 4), '.') >= 0) site = slice(host, 4);
  var on = hasOwn(C.sites, site) ? C.sites[site] === true : C.on === true;
  if (!on) return;

  // ---- o texto ----
  function norm(text, max) {
    if (typeof text !== 'string') return '';
    var out = '', gap = false, end = text.length < max ? text.length : max;
    for (var i = 0; i < end; i++) {
      var ch = text[i], lo = lower(ch);
      if (lo !== upper(ch) || (ch >= '0' && ch <= '9')) {
        if (gap && out.length) out += ' ';
        gap = false;
        out += lo;
      } else {
        gap = true;
      }
    }
    return out;
  }
  function hasPhrase(padded, list) {
    for (var i = 0; i < list.length; i++) {
      if (indexOf(padded, ' ' + list[i] + ' ') >= 0) return true;
    }
    return false;
  }
  function hasAccept(padded) {
    for (var i = 0; i < C.accept.length; i++) {
      var word = C.accept[i];
      if (indexOf(padded, word.length >= 4 ? ' ' + word : ' ' + word + ' ') >= 0) return true;
    }
    return false;
  }
  // So um botao de recusa: uma frase de REJECT_WORDS e nenhuma ACCEPT_WORDS.
  function safeReject(label) {
    var text = norm(label, 400);
    if (!text.length || text.length > C.maxLabel) return false;
    var padded = ' ' + text + ' ';
    return hasPhrase(padded, C.reject) && !hasAccept(padded);
  }
  function hasMarker(text, markers) {
    for (var i = 0; i < markers.length; i++) {
      if (indexOf(text, markers[i]) >= 0) return true;
    }
    return false;
  }

  // ---- o DOM, so pelas primitivas capturadas ----
  function buffer() { var b = create(null); b.n = 0; return b; }
  function add(b, value) { b[b.n] = value; b.n = b.n + 1; }
  function query(root, selector) {
    var out = buffer();
    var list;
    try {
      list = root === D ? qsaDoc(D, selector)
        : (nodeTypeOf(root) === 11 ? qsaFrag(root, selector) : qsaEl(root, selector));
    } catch (e) { return out; }
    var count = nlLength(list);
    for (var i = 0; i < count; i++) add(out, nlItem(list, i));
    return out;
  }
  function labelOf(el) {
    try { return textOf(el); } catch (e) { return ''; }
  }
  function idClass(el) {
    try { return norm((getAttr(el, 'id') || '') + ' ' + (getAttr(el, 'class') || ''), 400); }
    catch (e) { return ''; }
  }
  var hidden = new WS(), seen = new WS();
  function hide(el) {
    if (wsHas(hidden, el)) return false;
    try { setProp(styleOf(el), 'display', 'none', 'important'); } catch (e) { return false; }
    wsAdd(hidden, el);
    return true;
  }
  function paywallish(el) {
    if (hasMarker(idClass(el), C.paywall)) return true;
    if (query(el, C.paywallSel).n) return true;
    return hasMarker(norm(labelOf(el), 4000), C.paywall);
  }
  function viewport() {
    var de = docElOf(D);
    return de ? [clientWOf(de), clientHOf(de)] : [0, 0];
  }
  // Mostrado agora: com caixa, e nem ele nem um antepassado escondido.
  function visible(el) {
    try {
      var style = computed(W, el);
      if (getProp(style, 'display') === 'none' || getProp(style, 'visibility') === 'hidden') return false;
      var rect = rectOf(el);
      return rWidth(rect) > 0 && rHeight(rect) > 0;
    } catch (e) { return false; }
  }
  function scrollLocked() {
    var targets = [docElOf(D), bodyOf(D)];
    for (var t = 0; t < 2; t++) {
      if (!targets[t]) continue;
      var overflow = getProp(computed(W, targets[t]), 'overflow-y');
      if (overflow === 'hidden' || overflow === 'clip') return true;
    }
    return false;
  }
  // Uma camada da propria pagina (a app numa moldura, um canvas, um video).
  function media(tag) {
    return tag === 'iframe' || tag === 'canvas' || tag === 'video' || tag === 'img' ||
      tag === 'picture' || tag === 'svg' || tag === 'object' || tag === 'embed';
  }

  var clicked = create(null);
  // hidCause: escondemos o que prende a pagina (um aviso, uma janela).
  // walled: a pagina tem um paywall, ou uma moldura que nao se le e a
  // tapa; a partir dai a rolagem e os fundos ficam como a pagina os pos.
  var hidCause = false, restored = false, walled = false;
  var causes = buffer(), backdrops = buffer(), saved = buffer();
  function cause(el) { hidCause = true; add(causes, el); }

  // O que aparece logo depois de um clique ou de uma tecla do utilizador
  // foi ele que abriu (as definicoes de cookies do rodape, uma pesquisa, um
  // menu): fica, e fica para sempre.
  var lastInput = -1e12;
  var userOpened = new WS(), seenBanner = new WS();
  function markInput() { lastInput = now(); }
  function justOpened() { return now() - lastInput < C.userMs; }
  function openedByUser(el) {
    if (wsHas(userOpened, el)) return true;
    if (wsHas(hidden, el) || wsHas(seenBanner, el)) return false;
    wsAdd(seenBanner, el);
    if (!justOpened()) return false;
    wsAdd(userOpened, el);
    return true;
  }

  // Uma moldura de outra origem (Sourcepoint, TrustArc): nunca se le o
  // que ela diz. A que tapa a pagina pode ser um "pague ou aceite": fica,
  // e a pagina passa a ter um muro. A pequena fica numa pagina presa (sem
  // ela nao havia como sair); numa pagina que rola sai -- sem contar como
  // causa (nem rolagem, nem fundos).
  function frameOnly(shown) {
    var vh = viewport()[1], wall = !(vh > 0);
    for (var i = 0; i < shown.n && !wall; i++) {
      if (rHeight(rectOf(shown[i])) >= vh * C.frameWall) wall = true;
    }
    if (wall) {
      walled = true;
      for (var j = 0; j < shown.n; j++) wsAdd(seen, shown[j]);
      return;
    }
    if (scrollLocked()) return;
    for (var k = 0; k < shown.n; k++) hide(shown[k]);
  }

  // Os CMPs conhecidos, so com um aviso visivel agora: recusa (so num
  // botao visivel com o texto certo), depois esconder o aviso. Sem aviso
  // visivel (ja respondido, ou so o centro de preferencias escondido),
  // nada: nem clique, nem esconder.
  function runCmps() {
    for (var i = 0; i < C.cmp.length; i++) {
      var cmp = C.cmp[i];
      if (!query(D, cmp.detect).n) continue;
      var root = D;
      if (cmp.shadow) {
        var hosts = query(D, cmp.shadow);
        root = null;
        try { root = hosts.n ? shadowOf(hosts[0]) : null; } catch (e) { root = null; }
      }
      var buttons = cmp.reject && root && cmp.frame !== true ? query(root, cmp.reject) : buffer();
      // Sem `banner` (consent.google.com, Usercentrics): o sinal e o botao.
      var candidates = cmp.banner ? query(D, cmp.banner) : buttons;
      var shown = buffer();
      for (var c = 0; c < candidates.n; c++) {
        if (!wsHas(hidden, candidates[c]) && visible(candidates[c])) add(shown, candidates[c]);
      }
      if (!shown.n) continue;
      var paid = false, mine = false;
      for (var b = 0; b < shown.n; b++) {
        if (paywallish(shown[b])) paid = true;
        if (openedByUser(shown[b])) mine = true;
      }
      if (paid) { walled = true; continue; }
      if (mine) continue;
      if (cmp.frame === true) { frameOnly(shown); continue; }
      if (clicked[cmp.id] !== true) {
        for (var j = 0; j < buttons.n; j++) {
          if (!visible(buttons[j]) || !safeReject(labelOf(buttons[j]))) continue;
          clicked[cmp.id] = true;
          try { clickEl(buttons[j]); } catch (e) {}
          break;
        }
      }
      if (!cmp.banner) continue;
      for (var k = 0; k < shown.n; k++) if (hide(shown[k])) cause(shown[k]);
    }
  }

  // Um dialogo que so fala de newsletter no texto so sai quando o unico
  // campo dele e um e-mail (um checkout com a caixa «receber a newsletter»
  // tem outros campos, e fica).
  function emailOnly(el) {
    var fields = query(el, 'input, select, textarea');
    var email = false;
    for (var i = 0; i < fields.n; i++) {
      var field = fields[i];
      if (tagOf(field) !== 'input') return false;
      var type = lower('' + (getAttr(field, 'type') || 'text'));
      if (type === 'hidden' || type === 'submit' || type === 'button' || type === 'image') continue;
      if (type === 'email') { email = true; continue; }
      if (type === 'text') {
        var hint = norm((getAttr(field, 'name') || '') + ' ' + (getAttr(field, 'id') || '') + ' ' +
          (getAttr(field, 'autocomplete') || '') + ' ' + (getAttr(field, 'placeholder') || ''), 200);
        if (indexOf(hint, 'mail') >= 0) { email = true; continue; }
      }
      return false;
    }
    return email;
  }

  // Um candidato: aviso de cookies sem CMP conhecido, janela de newsletter,
  // barra fixa grande, ou o fundo escuro de uma janela que ja escondemos.
  function examine(el) {
    if (wsHas(hidden, el) || wsHas(seen, el)) return;
    wsAdd(seen, el);
    var tag = tagOf(el);
    if (tag === 'html' || tag === 'body' || tag === 'head' || tag === 'script' || tag === 'style') return;
    var style = computed(W, el);
    var position = getProp(style, 'position');
    var fixed = position === 'fixed' || position === 'sticky';
    var role = getAttr(el, 'role');
    var modal = tag === 'dialog' || role === 'dialog' || role === 'alertdialog' ||
      getAttr(el, 'aria-modal') === 'true';
    if (!fixed && !modal) return;
    if (getProp(style, 'display') === 'none' || getProp(style, 'visibility') === 'hidden') return;
    // Um paywall (marcador no id/class ou no texto) fica, e a pagina passa
    // a ter um muro: a rolagem e os fundos ficam como ela os pos.
    var names = idClass(el);
    var text = norm(labelOf(el), 4000);
    if (hasMarker(names, C.paywall) || hasMarker(text, C.paywall) || query(el, C.paywallSel).n) {
      walled = true;
      return;
    }
    var active = activeOf(D);
    if (active && active !== bodyOf(D) && containsNode(el, active)) return;
    if (justOpened()) { wsAdd(userOpened, el); return; }
    // Um login ou um registo (tem um campo de senha) nunca sai.
    if (query(el, 'input[type="password"]').n) return;
    if (fixed && (indexOf(names, 'cookie') >= 0 || indexOf(names, 'consent') >= 0 ||
        indexOf(names, 'gdpr') >= 0)) {
      if (hide(el)) cause(el);
      return;
    }
    if (hasMarker(names, C.newsletter) ||
        (modal && hasMarker(text, C.newsletter) && emailOnly(el))) {
      if (hide(el)) cause(el);
      return;
    }
    if (!fixed) return;
    var view = viewport(), vw = view[0], vh = view[1];
    if (!(vh > 0)) return;
    var rect = rectOf(el);
    var top = rTop(rect), bottom = rBottom(rect), width = rWidth(rect), height = rHeight(rect);
    if (width >= vw * 0.9 && height >= vh * 0.9) {
      // So um fundo vazio (nem texto, nem filhos, nem uma moldura, canvas
      // ou video da pagina); sai so ao lado do que escondemos (`settle`).
      var z = getProp(style, 'z-index');
      if (text.length < 2 && z !== 'auto' && +z > 0 && !media(tag) && !firstChildOf(el)) {
        add(backdrops, el);
      }
      return;
    }
    // Uma barra NA janela, encostada ao topo ou ao fundo: nem uma seccao
    // presa mais abaixo na pagina, nem um menu arrumado fora do ecra.
    if (width >= vw * 0.5 && height >= vh * C.stickyMin && height < vh * C.stickyMax &&
        top < vh && bottom > 0 &&
        ((top >= -1 && top <= 1) || (bottom >= vh - 1 && bottom <= vh + 1))) {
      hide(el);
    }
  }

  // O fundo e irmao do que escondemos, ou do embrulho dele.
  function nextToCause(el) {
    var parent = parentOf(el);
    if (!parent) return false;
    for (var i = 0; i < causes.n; i++) {
      var up = parentOf(causes[i]);
      if (up === parent || (up && parentOf(up) === parent)) return true;
    }
    return false;
  }
  function unrestore() {
    for (var i = 0; i < saved.n; i++) {
      var record = saved[i];
      try {
        if (record[1]) setProp(record[0], 'overflow-y', record[1], record[2]);
        else removeProp(record[0], 'overflow-y');
      } catch (e) {}
    }
    saved = buffer();
  }

  // Um marcador de paywall no id/class de algum elemento do documento: 2 se
  // um deles esta visivel (um muro), 1 se so ha escondidos (um molde).
  function paywallOnPage() {
    var found = query(D, C.paywallSel);
    for (var i = 0; i < found.n && i < 20; i++) if (visible(found[i])) return 2;
    return found.n ? 1 : 0;
  }

  function settle() {
    var pay = walled || !hidCause ? 0 : paywallOnPage();
    if (pay === 2) walled = true;
    // Com um muro na pagina: nada de fundos, e a rolagem que devolvemos
    // antes de ele chegar volta a ser a da pagina.
    if (walled) { unrestore(); backdrops = buffer(); return; }
    if (!hidCause) return;
    var keep = buffer();
    for (var i = 0; i < backdrops.n; i++) {
      if (nextToCause(backdrops[i])) hide(backdrops[i]);
      else add(keep, backdrops[i]);
    }
    backdrops = keep;
    // Um paywall escondido no documento tambem segura a rolagem.
    if (restored || pay) return;
    var de = docElOf(D), body = bodyOf(D);
    if (!de || !body) return;
    var viewH = clientHOf(de);
    if (!(scrollHOf(de) > viewH + 1 || scrollHOf(body) > viewH + 1)) return;
    var targets = [de, body];
    for (var t = 0; t < 2; t++) {
      var overflow = getProp(computed(W, targets[t]), 'overflow-y');
      if (overflow === 'hidden' || overflow === 'clip') {
        var inline = styleOf(targets[t]);
        add(saved, [inline, getProp(inline, 'overflow-y'), getPri(inline, 'overflow-y')]);
        setProp(inline, 'overflow-y', 'auto', 'important');
        restored = true;
      }
    }
  }

  // ---- o ritmo: o exame cede a vez depois de 8 ms; os CMPs no maximo a
  // cada 250 ms; para depois de 30 s sem mudancas ----
  var queue = buffer(), head = 0;
  var scheduled = false, stopped = false, idleArmed = false, lastChange = 0, observer = null;
  var lastCmps = -1e12, cmpsDue = false;
  function now() { return perfNow(perf); }
  function schedule(ms) {
    if (scheduled || stopped) return;
    scheduled = true;
    later(W, pump, ms);
  }
  function enqueue(list) {
    for (var i = 0; i < list.n; i++) add(queue, list[i]);
  }
  // Um elemento acrescentado e ate 60 descendentes dele, sem copiar a
  // arvore inteira.
  function enqueueTree(node) {
    add(queue, node);
    var count = 0, cur = firstChildOf(node);
    while (cur && count < 60) {
      add(queue, cur);
      count = count + 1;
      var next = firstChildOf(cur);
      while (!next && cur) {
        next = nextOf(cur);
        if (!next) {
          cur = parentOf(cur);
          if (cur === node) cur = null;
        }
      }
      cur = next;
    }
  }
  function pump() {
    scheduled = false;
    if (stopped) return;
    var start = now();
    if (start - lastCmps >= C.cmpMs) {
      lastCmps = start;
      cmpsDue = false;
      try { runCmps(); } catch (e) {}
    } else {
      cmpsDue = true;
    }
    while (head < queue.n) {
      if (now() - start > C.budgetMs) { schedule(16); return; }
      var el = queue[head];
      queue[head] = null;
      head = head + 1;
      try { examine(el); } catch (e) {}
    }
    queue = buffer();
    head = 0;
    // `settle` so logo depois de uma procura dos CMPs: nunca devolve a
    // rolagem sem ter visto o aviso (ou o muro) que chegou entretanto.
    if (cmpsDue) { schedule(lastCmps + C.cmpMs - now()); return; }
    try { settle(); } catch (e) {}
    armIdle();
  }
  function armIdle() {
    if (idleArmed || stopped) return;
    idleArmed = true;
    later(W, idleCheck, C.idleMs);
  }
  function idleCheck() {
    idleArmed = false;
    if (stopped) return;
    var quiet = now() - lastChange;
    if (quiet >= C.idleMs) {
      stopped = true;
      try { if (observer) moDisconnect(observer); } catch (e) {}
      return;
    }
    idleArmed = true;
    later(W, idleCheck, C.idleMs - quiet);
  }
  function onMutations(records) {
    if (stopped) return;
    lastChange = now();
    for (var i = 0; i < records.length; i++) {
      var added;
      try { added = addedOf(records[i]); } catch (e) { continue; }
      var count = nlLength(added);
      for (var j = 0; j < count; j++) {
        var node = nlItem(added, j);
        if (!node || nodeTypeOf(node) !== 1) continue;
        enqueueTree(node);
      }
    }
    schedule(50);
  }
  listen(W, 'pointerdown', markInput, true);
  listen(W, 'keydown', markInput, true);
  var begun = false;
  function begin() {
    if (begun) return;
    begun = true;
    lastChange = now();
    var de = docElOf(D);
    if (!de) return;
    enqueue(query(D, 'body > *, body > * > *, body > * > * > *, [role="dialog"], [aria-modal="true"], dialog'));
    observer = new MO(onMutations);
    var init = create(null);
    init.childList = true;
    init.subtree = true;
    try { moObserve(observer, de, init); } catch (e) {}
    pump();
  }
  if (readyOf(D) === 'loading') listen(D, 'DOMContentLoaded', begin);
  else begin();
})();"##;

/// O script com a politica e as regras gravadas (`script_config`): o que
/// cada WebView recebe e o que os gates correm.
pub(in crate::windows_app) fn fill_distraction_script(
    template: &str,
    policy: &DistractionPolicy,
) -> String {
    template.replace(
        DISTRACTION_CONFIG_PLACEHOLDER,
        &script_config(policy).to_string(),
    )
}

/// `fill_distraction_script` sobre o script que embarca.
#[cfg(test)]
pub(in crate::windows_app) fn distraction_script(policy: &DistractionPolicy) -> String {
    fill_distraction_script(NEURALIA_DISTRACTION_SCRIPT, policy)
}

/// Onde a pagina de um hospedeiro esta, para a decisao do nucleo.
pub(in crate::windows_app) fn distraction_surface(host: WebViewHost) -> DistractionSurface {
    match host {
        WebViewHost::Column(_)
        | WebViewHost::Split(_)
        | WebViewHost::PrivateSplit(_)
        | WebViewHost::External => DistractionSurface::Web,
        WebViewHost::Reader => DistractionSurface::Reader,
        WebViewHost::Pdf => DistractionSurface::Pdf,
        WebViewHost::Epub => DistractionSurface::Books,
        WebViewHost::Live | WebViewHost::SidePanel => DistractionSurface::AppPanel,
        WebViewHost::GmailMonitor | WebViewHost::Service(_) => DistractionSurface::ServicePanel,
    }
}

/// Os hospedeiros com o slot `distraction` da tabela: as colunas, a fonte
/// ao lado (normal ou privada) e a Web completa.
pub(in crate::windows_app) fn distraction_host(host: WebViewHost) -> bool {
    distraction_surface(host) == DistractionSurface::Web
}

// ===================== a politica partilhada e as ligacoes =====================

/// O que os handlers (o menu do botao direito, o registo de cada WebView)
/// leem da politica: a gravada e as escolhas do Split privado, publicadas
/// pelo `AdblockState` a cada mudanca, e o script ligado a cada WebView.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct DistractionShared {
    saved: RwLock<DistractionPolicy>,
    private: RwLock<BTreeMap<String, bool>>,
    bound: Mutex<Vec<BoundWebView>>,
}

#[derive(Debug)]
struct BoundWebView {
    key: usize,
    host: WebViewHost,
    slot: Arc<DistractionSlot>,
}

impl DistractionShared {
    pub(in crate::windows_app) fn new(saved: DistractionPolicy) -> Self {
        Self {
            saved: RwLock::new(saved),
            ..Self::default()
        }
    }

    pub(in crate::windows_app) fn publish(
        &self,
        saved: &DistractionPolicy,
        private: &BTreeMap<String, bool>,
    ) {
        *self
            .saved
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = saved.clone();
        *self
            .private
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = private.clone();
    }

    /// A politica que vale neste hospedeiro: a gravada; no Split privado,
    /// com as escolhas feitas la por cima.
    pub(in crate::windows_app) fn policy_for(&self, host: WebViewHost) -> DistractionPolicy {
        let saved = self
            .saved
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if matches!(host, WebViewHost::PrivateSplit(_)) {
            saved.with_overlay(
                &self
                    .private
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
        } else {
            saved
        }
    }

    /// A ligacao de uma WebView acabada de construir (`key`: o ponteiro do
    /// `ICoreWebView2` dela). Uma WebView nova no mesmo endereco de uma que
    /// ja morreu fica com uma ligacao nova.
    fn register(&self, key: usize, host: WebViewHost) -> Arc<DistractionSlot> {
        let mut bound = self
            .bound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        bound.retain(|entry| entry.key != key);
        let slot = Arc::new(DistractionSlot::default());
        bound.push(BoundWebView {
            key,
            host,
            slot: Arc::clone(&slot),
        });
        slot
    }

    /// A ligacao de uma WebView aberta, se ela tem o slot.
    fn bound(&self, key: usize) -> Option<(WebViewHost, Arc<DistractionSlot>)> {
        self.bound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| (entry.host, Arc::clone(&entry.slot)))
    }

    /// Esquece as WebViews que ja fecharam.
    fn retain_open(&self, open: &[usize]) {
        self.bound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|entry| open.contains(&entry.key));
    }
}

/// O script ligado a UMA WebView: a geracao da ultima ligacao pedida e o id
/// que o WebView2 deu a ela. Pura: o COM so pergunta aqui.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct DistractionSlot {
    state: Mutex<SlotState>,
}

#[derive(Debug, Default)]
struct SlotState {
    generation: u64,
    bound: Option<String>,
}

/// O que fazer com o id que o `AddScriptToExecuteOnDocumentCreated` devolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum SlotCompletion {
    /// E a ligacao em vigor: fica.
    Keep,
    /// Outra ligacao foi pedida entretanto: este script sai logo.
    Remove(String),
}

impl DistractionSlot {
    /// Uma ligacao nova: a geracao dela e o id do script anterior, que tem
    /// de sair antes de o novo entrar.
    pub(in crate::windows_app) fn begin(&self) -> (u64, Option<String>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.generation += 1;
        (state.generation, state.bound.take())
    }

    /// O registo da geracao `generation` acabou com o id `id`.
    pub(in crate::windows_app) fn complete(&self, generation: u64, id: String) -> SlotCompletion {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if generation == state.generation {
            state.bound = Some(id);
            SlotCompletion::Keep
        } else {
            SlotCompletion::Remove(id)
        }
    }

    #[cfg(test)]
    pub(in crate::windows_app) fn bound_id(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .bound
            .clone()
    }
}

/// A chave de uma WebView no registo das ligacoes: o ponteiro do
/// `ICoreWebView2` dela.
fn webview_key(webview: &WebView) -> usize {
    use windows_core::Interface;
    use wry::WebViewExtWindows;
    webview.webview().as_raw() as usize
}

/// Liga `script` a uma WebView pelo COM: tira o anterior (pelo id), poe o
/// novo e, com `reload`, recarrega a pagina quando o novo ja esta registado.
fn bind_distraction_script(
    webview: &WebView,
    slot: &Arc<DistractionSlot>,
    script: String,
    reload: bool,
) -> Result<(), String> {
    use webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler;
    use windows_core::HSTRING;
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let (generation, previous) = slot.begin();
    if let Some(id) = previous {
        unsafe { core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id.as_str())) }
            .map_err(|error| format!("RemoveScriptToExecuteOnDocumentCreated: {error}"))?;
    }
    let completed_slot = Arc::clone(slot);
    let target = core.clone();
    let handler =
        AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |result, id| {
            if let Err(error) = result {
                debug_log(format_args!(
                    "distraction: o script nao foi registado ({error})"
                ));
                return Ok(());
            }
            match completed_slot.complete(generation, id) {
                SlotCompletion::Keep => {
                    if reload {
                        unsafe { target.Reload()? };
                    }
                }
                SlotCompletion::Remove(id) => unsafe {
                    target.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id.as_str()))?
                },
            }
            Ok(())
        }));
    unsafe { core.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(script.as_str()), &handler) }
        .map_err(|error| format!("AddScriptToExecuteOnDocumentCreated: {error}"))
}

/// A metade COM do slot `distraction` de uma WebView acabada de construir:
/// regista-a e liga-lhe o script com a politica do hospedeiro.
pub(in crate::windows_app) fn register_distraction_script(
    webview: &WebView,
    host: WebViewHost,
    template: &'static str,
    shared: &DistractionShared,
) -> Result<(), String> {
    let slot = shared.register(webview_key(webview), host);
    let script = fill_distraction_script(template, &shared.policy_for(host));
    bind_distraction_script(webview, &slot, script, false)
}

// ===================== o menu do botao direito =====================

/// Id da caixa «Ocultar distrações neste site» nos menus das WebViews.
pub(in crate::windows_app) const DISTRACTION_MENU_SITE: usize = 5;

pub(in crate::windows_app) const LABEL_DISTRACTION_SITE: &str = "Ocultar distrações neste site";

/// O que a anti-distracao poe no menu desta pagina: o site e se esta
/// ligada nele.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct DistractionMenu {
    pub(in crate::windows_app) site: String,
    pub(in crate::windows_app) on: bool,
}

/// A caixa para a pagina `page` (o `Source` do WebView no instante do
/// botao direito): nada numa pagina onde o script nunca age (as IAs, os
/// logins, as nossas origens, fora da web).
pub(in crate::windows_app) fn distraction_menu(
    host: WebViewHost,
    page: Option<&str>,
    policy: &DistractionPolicy,
) -> Option<DistractionMenu> {
    let url = Url::parse(page?.trim()).ok()?;
    let surface = distraction_surface(host);
    let site = distraction_site(&url, surface)?;
    Some(DistractionMenu {
        on: distraction_config(&url, policy, surface).is_some(),
        site,
    })
}

/// O item do menu: a caixa marcada com a anti-distracao ligada; escolhida,
/// troca so este site.
pub(in crate::windows_app) fn distraction_site_item(
    host: WebViewHost,
    menu: Option<&DistractionMenu>,
) -> Option<MenuItemView> {
    let menu = menu?;
    Some(MenuItemView {
        label: LABEL_DISTRACTION_SITE.to_string(),
        checked: Some(menu.on),
        enabled: true,
        action: MenuAction::Distraction {
            host,
            site: menu.site.clone(),
            on: !menu.on,
        },
    })
}

// ===================== os eventos e o app =====================

/// O que chega ao event loop da anti-distracao.
#[derive(Debug)]
pub(in crate::windows_app) enum DistractionEvent {
    /// A caixa «Ocultar distrações neste site» de `host`: `on` e o estado
    /// novo de `site`.
    SetSite {
        host: WebViewHost,
        site: String,
        on: bool,
    },
}

/// O que uma escolha fez.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum DistractionToggle {
    /// Gravada no `adblock-settings.json`.
    Saved,
    /// Feita no Split privado: so em memoria.
    MemoryOnly,
    Unchanged,
    /// Um site que nao se guarda, ou a lista cheia.
    Refused,
}

/// O aviso depois de uma escolha.
pub(in crate::windows_app) fn distraction_toggle_message(on: bool, private: bool) -> String {
    let base = if on {
        "Distrações ocultas neste site"
    } else {
        "Distrações visíveis neste site"
    };
    if private {
        format!("{base} · Modo privado: esta escolha não é guardada.")
    } else {
        base.to_string()
    }
}

impl App {
    /// O unico braco da anti-distracao no `user_event`.
    pub(in crate::windows_app) fn distraction_event(&mut self, event: DistractionEvent) {
        match event {
            DistractionEvent::SetSite { host, site, on } => {
                self.distraction_set_site(host, &site, on)
            }
        }
    }

    /// A caixa do menu: grava a escolha (no Split privado, so em memoria),
    /// volta a ligar o script em cada WebView aberta e recarrega a pagina
    /// onde a escolha foi feita.
    fn distraction_set_site(&mut self, host: WebViewHost, site: &str, on: bool) {
        let private = matches!(host, WebViewHost::PrivateSplit(_));
        match self.adblock.set_distraction_site(site, on, private) {
            DistractionToggle::Unchanged => return,
            DistractionToggle::Refused => {
                self.show_splash(
                    format!("Não foi possível guardar a escolha para {site}."),
                    4,
                );
                return;
            }
            DistractionToggle::Saved | DistractionToggle::MemoryOnly => {}
        }
        self.distraction_rebind(Some(host));
        self.show_splash(distraction_toggle_message(on, private), 3);
    }

    /// Volta a ligar o script em cada WebView aberta que tem o slot, com a
    /// politica de agora; a de `reload` recarrega quando o script novo ja
    /// esta registado.
    fn distraction_rebind(&self, reload: Option<WebViewHost>) {
        let mut open: Vec<&WebView> = Vec::new();
        if let Some(comparator) = &self.comparator {
            open.extend(comparator.views.iter().map(|view| &view.webview));
            if let Some(split) = &comparator.split {
                open.push(&split.webview);
            }
        }
        if let Some(webview) = &self.webview {
            open.push(webview);
        }
        let shared = &self.adblock.distraction;
        let keys: Vec<usize> = open.iter().map(|webview| webview_key(webview)).collect();
        shared.retain_open(&keys);
        for webview in open {
            let Some((host, slot)) = shared.bound(webview_key(webview)) else {
                continue;
            };
            let script =
                fill_distraction_script(NEURALIA_DISTRACTION_SCRIPT, &shared.policy_for(host));
            if let Err(error) =
                bind_distraction_script(webview, &slot, script, reload == Some(host))
            {
                debug_log(format_args!(
                    "distraction: {} ficou com o script anterior ({error})",
                    host.describe()
                ));
            }
        }
    }
}
