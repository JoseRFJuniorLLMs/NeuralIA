// NeuralIA -- Gemini Live: a interface do painel. O protocolo e a sessao
// estao em live-core.js; aqui so se liga o DOM, o navegador e o canal do
// painel (window.ipc, que o wry define so para esta WebView).
(() => {
  'use strict';
  if (window.top !== window) return;
  const core = window.NeuraliaLiveCore;
  if (!core) return;

  const $ = (id) => document.getElementById(id);
  const post = (action, args) =>
    window.ipc.postMessage(JSON.stringify({ action, args: args || {} }));

  const keyScreen = $('key-screen');
  const liveScreen = $('live-screen');
  const keyInput = $('key');
  const saveButton = $('save');
  const keyError = $('key-error');
  const statusLine = $('status');
  const notices = $('notices');
  const errorBox = $('error-box');
  const errorText = $('error');
  const errorDetail = $('error-detail');
  const errorKey = $('error-key');
  const cam = $('cam');
  const log = $('log');
  const toggles = { screen: $('t-screen'), camera: $('t-camera'), mic: $('t-mic') };

  // A tela vai para um <video> fora do DOM; o frame e composto num canvas.
  const screenVideo = document.createElement('video');
  screenVideo.muted = true;
  screenVideo.playsInline = true;
  const canvas = document.createElement('canvas');
  const painter = canvas.getContext('2d');
  const worklets = new WeakMap();

  let session = null;
  let lastRole = null;
  let lastLine = null;

  // Um aviso por motivo, cada um apagado so quando o seu motivo acaba: o da
  // chave que nao foi salva, o de cada fonte que nao ligou e o do som que
  // espera um clique. Uma linha so para todos deixava o ultimo apagar os
  // outros -- e o "clique em Tela" sumia debaixo do aviso do som.
  const NOTICE_KINDS = ['save', 'screen', 'camera', 'mic', 'audio'];
  const noticeText = {};

  function theme(vars) {
    if (!vars || typeof vars !== 'object') return;
    for (const name of Object.keys(vars)) {
      if (name.startsWith('--')) document.documentElement.style.setProperty(name, String(vars[name]));
    }
  }

  function show(which) {
    keyScreen.hidden = which !== 'key';
    liveScreen.hidden = which !== 'live';
  }

  function setStatus(text, tone) {
    statusLine.textContent = text;
    document.body.dataset.state = tone || 'idle';
  }

  function setNotice(kind, text) {
    if (!NOTICE_KINDS.includes(kind)) return;
    noticeText[kind] = text || '';
    while (notices.firstElementChild) notices.removeChild(notices.firstElementChild);
    let shown = 0;
    for (const name of NOTICE_KINDS) {
      if (!noticeText[name]) continue;
      const line = document.createElement('p');
      line.className = 'notice';
      line.dataset.kind = name;
      line.textContent = noticeText[name];
      notices.appendChild(line);
      shown += 1;
    }
    notices.hidden = shown === 0;
  }

  function clearNotices() {
    for (const name of NOTICE_KINDS) noticeText[name] = '';
    setNotice('save', '');
  }

  function showError(message, keyProblem, detail) {
    errorText.textContent = message;
    const extra = detail && detail !== message ? String(detail) : '';
    errorDetail.textContent = extra;
    errorDetail.hidden = !extra;
    errorKey.hidden = !keyProblem;
    errorBox.hidden = false;
    document.body.dataset.state = 'error';
    statusLine.textContent = 'Desligado.';
  }

  function renderSources(sources) {
    for (const name of Object.keys(toggles)) {
      toggles[name].setAttribute('aria-pressed', sources && sources[name] ? 'true' : 'false');
    }
  }

  function transcript(role, text) {
    if (role !== lastRole || !lastLine) {
      lastLine = document.createElement('p');
      lastLine.className = role === 'user' ? 'line user' : 'line model';
      const who = document.createElement('b');
      who.textContent = role === 'user' ? 'Você: ' : 'Gemini: ';
      lastLine.appendChild(who);
      lastLine.appendChild(document.createTextNode(''));
      log.appendChild(lastLine);
      lastRole = role;
      while (log.childElementCount > 200) log.removeChild(log.firstElementChild);
    }
    lastLine.lastChild.textContent += text;
    log.scrollTop = log.scrollHeight;
  }

  function turnComplete() {
    lastRole = null;
    lastLine = null;
  }

  function attach(video, stream) {
    video.srcObject = stream || null;
    if (stream) video.play().catch(() => {});
  }

  async function createMic(context, stream, onChunk) {
    let ready = worklets.get(context);
    if (!ready) {
      const url = URL.createObjectURL(new Blob([core.WORKLET_SOURCE], { type: 'text/javascript' }));
      ready = context.audioWorklet.addModule(url).finally(() => URL.revokeObjectURL(url));
      worklets.set(context, ready);
    }
    await ready;
    const source = context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(context, core.WORKLET_NAME);
    // O worklet so corre se o grafo chegar ao destino; o ganho zero garante
    // que o microfone nao sai nas colunas.
    const sink = context.createGain();
    sink.gain.value = 0;
    node.port.onmessage = (event) => onChunk(event.data);
    source.connect(node);
    node.connect(sink);
    sink.connect(context.destination);
    return {
      rate: context.sampleRate,
      disconnect() {
        node.port.onmessage = null;
        source.disconnect();
        node.disconnect();
        sink.disconnect();
      }
    };
  }

  function grabFrame() {
    const screenOk = !!screenVideo.srcObject && screenVideo.videoWidth > 0;
    const camOk = !!cam.srcObject && cam.videoWidth > 0;
    const main = screenOk ? screenVideo : camOk ? cam : null;
    if (!main) return Promise.resolve(null);
    const size = core.frameSize(main.videoWidth, main.videoHeight);
    if (!size) return Promise.resolve(null);
    canvas.width = size.width;
    canvas.height = size.height;
    painter.drawImage(main, 0, 0, size.width, size.height);
    if (screenOk && camOk) {
      const pip = core.pictureInPicture(size, { width: cam.videoWidth, height: cam.videoHeight });
      if (pip) painter.drawImage(cam, pip.x, pip.y, pip.width, pip.height);
    }
    const url = canvas.toDataURL('image/jpeg', 0.7);
    return Promise.resolve(url.slice(url.indexOf(',') + 1));
  }

  const env = {
    WebSocket: window.WebSocket,
    AudioContext: window.AudioContext,
    getUserMedia: (constraints) => navigator.mediaDevices.getUserMedia(constraints),
    getDisplayMedia: (constraints) => navigator.mediaDevices.getDisplayMedia(constraints),
    createMic,
    grabFrame,
    setInterval: (fn, ms) => window.setInterval(fn, ms),
    clearInterval: (id) => window.clearInterval(id)
  };

  const ui = {
    status: setStatus,
    notice: setNotice,
    error: showError,
    sources: renderSources,
    transcript,
    turnComplete,
    // A sessao acabou sem o utilizador a desligar: o nativo tira o vermelho
    // do olho, porque ja nada sai.
    ended() {
      post('stopped');
    },
    camera(stream) {
      attach(cam, stream);
      cam.hidden = !stream;
    },
    screen(stream) {
      attach(screenVideo, stream);
    }
  };

  function stopSession() {
    if (session) {
      session.stop();
      session = null;
    }
  }

  function start(config) {
    theme(config && config.theme);
    stopSession();
    show('live');
    errorBox.hidden = true;
    clearNotices();
    setNotice('save', (config && config.notice) || '');
    log.textContent = '';
    turnComplete();
    const current = core.createSession({ env, ui, key: config.key });
    session = current;
    current.start().then(() => {
      if (session === current && current.audioSuspended) {
        setNotice('audio', 'Clique no painel para ativar o som e o microfone.');
      }
    });
  }

  function askKey(config) {
    theme(config && config.theme);
    stopSession();
    show('key');
    saveButton.disabled = false;
    const error = config && config.error;
    keyError.textContent = error || '';
    keyError.hidden = !error;
    keyInput.value = '';
    keyInput.focus();
  }

  function saveKey() {
    const value = keyInput.value.trim();
    // A chave sai do DOM ja: nao fica no campo enquanto o nativo responde.
    keyInput.value = '';
    if (!value) {
      keyError.textContent = 'Cole a chave primeiro.';
      keyError.hidden = false;
      return;
    }
    // Maior do que qualquer chave nem cabe no canal: o nativo largava-a sem
    // resposta e o botao ficava desativado para sempre.
    if (value.length > core.KEY_MAX_CHARS) {
      keyError.textContent = core.INVALID_KEY_NOTICE;
      keyError.hidden = false;
      return;
    }
    keyError.hidden = true;
    saveButton.disabled = true;
    post('save_key', { key: value });
  }

  saveButton.addEventListener('click', saveKey);
  keyInput.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      saveKey();
    }
  });
  $('off').addEventListener('click', () => {
    stopSession();
    post('close');
  });
  // Depois de a sessao cair: pede ao nativo para arrancar de novo com a
  // chave salva (ou pedir uma), sem apagar nada.
  $('restart').addEventListener('click', () => {
    stopSession();
    errorBox.hidden = true;
    setStatus('Conectando…', 'connecting');
    post('ready');
  });
  for (const button of [$('change-key'), errorKey]) {
    button.addEventListener('click', () => {
      stopSession();
      post('forget_key');
    });
  }
  const setters = { screen: 'setScreen', camera: 'setCamera', mic: 'setMic' };
  for (const name of Object.keys(toggles)) {
    toggles[name].addEventListener('click', () => {
      if (!session) return;
      const on = toggles[name].getAttribute('aria-pressed') !== 'true';
      session[setters[name]](on);
    });
  }
  // O som (e o worklet do microfone) so arranca depois de um gesto na pagina
  // quando o painel abre sem nenhum: o primeiro clique retoma o contexto.
  document.addEventListener(
    'pointerdown',
    () => {
      if (session && session.audioSuspended) {
        session.resumeAudio().then(() => {
          if (session && !session.audioSuspended) setNotice('audio', '');
        });
      }
    },
    true
  );
  window.addEventListener('pagehide', stopSession);

  Object.defineProperty(window, '__neuraliaLive', {
    value: Object.freeze({ theme, start, askKey }),
    writable: false,
    configurable: false
  });
  post('ready');
})();
