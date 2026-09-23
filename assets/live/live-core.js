// NeuralIA -- Gemini Live: protocolo e sessao, sem DOM.
//
// Servido ao painel em http://neuralia-live.localhost/live-core.js e corrido
// TAL COMO ESTA pelos gates em Node (crates/neural-app/src/gemini_live.rs):
// o que se testa e o texto que embarca, nao uma copia.
//
// Tudo o que toca no navegador (WebSocket, AudioContext, getUserMedia,
// getDisplayMedia, o worklet do microfone, o canvas dos frames e os
// temporizadores) chega por `env`; a interface chega por `ui`. Assim o ciclo
// de vida inteiro -- ligar, enviar, tocar, interromper, retomar, desligar --
// corre em Node com dublês e fica provado sem um ecra.
(function (root) {
  'use strict';

  // O modelo e INTOCAVEL (regra do dono): e este id, exatamente, e mais nenhum.
  const MODEL = 'models/gemini-2.5-flash-native-audio-preview-12-2025';
  const ENDPOINT =
    'wss://generativelanguage.googleapis.com/ws/' +
    'google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent';
  const INPUT_RATE = 16000;
  const OUTPUT_RATE = 24000;
  const FRAME_MAX_SIDE = 1024;
  const FRAME_INTERVAL_MS = 1000;
  /// 100 ms de voz a 16 kHz por mensagem.
  const MIC_CHUNK_SAMPLES = 1600;
  /// O mesmo tecto do nativo (LIVE_KEY_MAX_CHARS em gemini_live.rs). Uma
  /// colagem maior nem cabe no canal do painel: a pagina recusa-a sozinha.
  const KEY_MAX_CHARS = 256;
  /// O mesmo texto que o nativo manda para uma chave com forma errada.
  const INVALID_KEY_NOTICE =
    'Isso não parece uma chave da API do Gemini. Copie de novo da AI Studio.';
  /// Religacoes seguidas sem um setupComplete pelo meio antes de desistir.
  const MAX_RESUMES = 3;
  /// Tectos por SESSAO, que o setupComplete nao zera: um servidor que aceita
  /// o setup e volta a fechar religaria sem fim, a gastar a cota do dono.
  /// Quedas inesperadas: 6. Rotacoes avisadas (goAway, ~1 a cada 10 min): 36.
  const MAX_DROP_RESUMES = 6;
  const MAX_GOAWAY_RESUMES = 36;
  const TOO_MANY_DROPS =
    'A conexão caiu várias vezes e a sessão foi encerrada para não gastar a sua cota. Ligue de novo quando quiser.';
  const SYSTEM_INSTRUCTION =
    'Você é a assistente de voz do NeuralIA, um navegador para Windows. ' +
    'Você vê a tela do usuário (e a câmera, quando ligada) e ouve o microfone. ' +
    'Responda sempre em português do Brasil, de forma breve e natural. ' +
    'Fale só do que for útil para a pergunta e diga quando algo na tela não estiver legível.';

  const WORKLET_NAME = 'neuralia-mic';
  // O processador do microfone: junta blocos de 128 amostras em 2048 e manda
  // cada bloco cheio para a thread principal. Corre no AudioWorkletGlobalScope.
  const WORKLET_SOURCE =
    'class NeuraliaMic extends AudioWorkletProcessor {\n' +
    '  constructor() { super(); this.buffer = new Float32Array(2048); this.used = 0; }\n' +
    '  process(inputs) {\n' +
    '    const channel = inputs[0] && inputs[0][0];\n' +
    '    if (channel) {\n' +
    '      for (let i = 0; i < channel.length; i++) {\n' +
    '        this.buffer[this.used++] = channel[i];\n' +
    '        if (this.used === this.buffer.length) {\n' +
    '          this.port.postMessage(this.buffer.slice(0));\n' +
    '          this.used = 0;\n' +
    '        }\n' +
    '      }\n' +
    '    }\n' +
    '    return true;\n' +
    '  }\n' +
    '}\n' +
    "registerProcessor('" + WORKLET_NAME + "', NeuraliaMic);\n";

  function socketUrl(key) {
    return ENDPOINT + '?key=' + encodeURIComponent(String(key));
  }

  /// `handle` retoma uma sessao anterior (o ultimo sessionResumptionUpdate);
  /// sem ele a sessao e nova.
  function setupMessage(handle) {
    return {
      setup: {
        model: MODEL,
        generationConfig: { responseModalities: ['AUDIO'] },
        systemInstruction: { parts: [{ text: SYSTEM_INSTRUCTION }] },
        inputAudioTranscription: {},
        outputAudioTranscription: {},
        // Sem compressao do contexto, o Google corta uma sessao com video aos
        // ~2 minutos (so audio, aos 15). A janela deslizante tira o tecto.
        contextWindowCompression: { slidingWindow: {} },
        // Cada ligacao dura ~10 minutos: com isto o servidor manda handles
        // para a sessao continuar numa ligacao nova.
        sessionResumption: handle ? { handle: String(handle) } : {}
      }
    };
  }

  /// [-1, 1] -> PCM de 16 bits. Fora do intervalo corta; NaN vale silencio.
  function floatToInt16(value) {
    const v = value > 1 ? 1 : value < -1 ? -1 : value || 0;
    return v < 0 ? Math.round(v * 32768) : Math.round(v * 32767);
  }

  /// Reamostragem com estado: cada saida e a media das entradas que cobre
  /// (filtro em caixa, que ja corta o grosso do aliasing a 48k -> 16k). O que
  /// sobra de um bloco -- as amostras E a fracao de amostra onde a proxima
  /// saida comeca -- passa para o seguinte. So com as amostras, a 44,1 kHz
  /// cada bloco arredondava a fase para baixo e o audio ia derivando (16 004
  /// saidas por segundo em vez de 16 000).
  function createDownsampler(inRate, outRate) {
    const ratio = inRate / (outRate || INPUT_RATE);
    if (!(ratio > 0) || !isFinite(ratio)) {
      throw new RangeError('taxa de amostragem inválida');
    }
    let carry = new Float32Array(0);
    let phase = 0;
    return function push(chunk) {
      const data = new Float32Array(carry.length + chunk.length);
      data.set(carry, 0);
      data.set(chunk, carry.length);
      const count = Math.max(0, Math.floor((data.length - phase) / ratio));
      const out = new Int16Array(count);
      for (let i = 0; i < count; i++) {
        const start = Math.floor(phase + i * ratio);
        const end = Math.min(
          data.length,
          Math.max(start + 1, Math.floor(phase + (i + 1) * ratio))
        );
        let sum = 0;
        for (let j = start; j < end; j++) sum += data[j];
        out[i] = floatToInt16(sum / (end - start));
      }
      const next = phase + count * ratio;
      const keep = Math.floor(next);
      carry = data.slice(keep);
      phase = next - keep;
      return out;
    };
  }

  /// PCM de 16 bits em little-endian, em base64 -- o formato do realtimeInput.
  function pcm16ToBase64(samples) {
    const bytes = new Uint8Array(samples.length * 2);
    const view = new DataView(bytes.buffer);
    for (let i = 0; i < samples.length; i++) view.setInt16(i * 2, samples[i], true);
    let binary = '';
    for (let i = 0; i < bytes.length; i += 0x8000) {
      binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    return btoa(binary);
  }

  function base64ToPcm16(data) {
    const binary = atob(data);
    const out = new Int16Array(binary.length >> 1);
    for (let i = 0; i < out.length; i++) {
      const value = binary.charCodeAt(2 * i) | (binary.charCodeAt(2 * i + 1) << 8);
      out[i] = value >= 0x8000 ? value - 0x10000 : value;
    }
    return out;
  }

  function pcm16ToFloat32(samples) {
    const out = new Float32Array(samples.length);
    for (let i = 0; i < samples.length; i++) out[i] = samples[i] / 32768;
    return out;
  }

  function audioMessage(samples) {
    return {
      realtimeInput: {
        audio: { data: pcm16ToBase64(samples), mimeType: 'audio/pcm;rate=' + INPUT_RATE }
      }
    };
  }

  /// O microfone desligou: o servidor deixa de esperar pelo resto da frase.
  function audioStreamEndMessage() {
    return { realtimeInput: { audioStreamEnd: true } };
  }

  function videoMessage(jpegBase64) {
    return { realtimeInput: { video: { data: jpegBase64, mimeType: 'image/jpeg' } } };
  }

  /// O frame nunca passa de `maxSide` no lado maior e nunca e ampliado.
  function frameSize(width, height, maxSide) {
    const max = maxSide || FRAME_MAX_SIDE;
    if (!(width > 0) || !(height > 0)) return null;
    const scale = Math.min(1, max / Math.max(width, height));
    return {
      width: Math.max(1, Math.round(width * scale)),
      height: Math.max(1, Math.round(height * scale))
    };
  }

  /// A camera por cima da tela: um quarto da largura, no canto inferior
  /// direito, com a proporcao dela.
  function pictureInPicture(frame, camera) {
    if (!frame || !camera || !(camera.width > 0) || !(camera.height > 0)) return null;
    const width = Math.round(frame.width / 4);
    const height = Math.round((width * camera.height) / camera.width);
    const margin = Math.round(frame.width / 64);
    return {
      x: frame.width - width - margin,
      y: frame.height - height - margin,
      width,
      height
    };
  }

  /// O servidor manda os frames como Blob no navegador (e ArrayBuffer ou
  /// texto noutros sitios). JSON.parse de um Blob da "[object Blob]".
  function frameText(data) {
    if (typeof data === 'string') return Promise.resolve(data);
    if (data && typeof data.text === 'function') return data.text();
    if (data && (data instanceof ArrayBuffer || ArrayBuffer.isView(data))) {
      return Promise.resolve(new TextDecoder().decode(data));
    }
    return Promise.resolve('');
  }

  function parseRate(mimeType) {
    const match = /rate=(\d+)/i.exec(mimeType || '');
    return match ? Number(match[1]) : OUTPUT_RATE;
  }

  /// Uma mensagem do servidor, normalizada. `null` quando nao e JSON.
  function parseServerMessage(text) {
    let message;
    try {
      message = JSON.parse(text);
    } catch (_) {
      return null;
    }
    if (!message || typeof message !== 'object') return null;
    const out = {
      setupComplete: !!message.setupComplete,
      goAway: !!message.goAway,
      resumeHandle: '',
      error: '',
      audio: [],
      inputText: '',
      outputText: '',
      interrupted: false,
      turnComplete: false
    };
    // So um handle "resumable" serve para retomar; os outros nao se guardam.
    const update = message.sessionResumptionUpdate;
    if (
      update &&
      update.resumable === true &&
      typeof update.newHandle === 'string' &&
      update.newHandle
    ) {
      out.resumeHandle = update.newHandle;
    }
    if (message.error) {
      out.error = String(message.error.message || message.error.status || 'erro do servidor');
    }
    const content = message.serverContent;
    if (content && typeof content === 'object') {
      const turn = content.modelTurn;
      const parts = turn && Array.isArray(turn.parts) ? turn.parts : [];
      for (const part of parts) {
        const inline = part && part.inlineData;
        if (inline && typeof inline.data === 'string' && /^audio\/pcm/i.test(inline.mimeType || '')) {
          out.audio.push({ data: inline.data, rate: parseRate(inline.mimeType) });
        }
      }
      if (content.inputTranscription && typeof content.inputTranscription.text === 'string') {
        out.inputText = content.inputTranscription.text;
      }
      if (content.outputTranscription && typeof content.outputTranscription.text === 'string') {
        out.outputText = content.outputTranscription.text;
      }
      out.interrupted = content.interrupted === true;
      out.turnComplete = content.turnComplete === true;
    }
    return out;
  }

  /// Porque a sessao fechou, em portugues do Brasil, se a culpa e da chave, e
  /// o motivo cru do servidor (em ingles) como detalhe a parte.
  function describeClose(code, reason) {
    const text = String(reason || '').trim();
    const detail = text.replace(/[\s.]+$/, '');
    const keyProblem = /api[ _-]?key|permission|unauthori[sz]ed|forbidden|unregistered/i.test(text);
    let message;
    if (keyProblem) {
      message = 'O Google recusou a chave. Use «Trocar chave» e tente de novo.';
    } else if (/quota|resource[ _-]?exhausted|billing|rate[ _-]?limit/i.test(text)) {
      message =
        'A cota da API do Gemini acabou ou falta configurar o faturamento. ' +
        'Confira o plano em aistudio.google.com.';
    } else if (/model/i.test(text) && /not (found|supported)|unsupported/i.test(text)) {
      message = 'O modelo do Gemini Live não está disponível para esta chave.';
    } else if (code === 1000) {
      message = 'Sessão encerrada pelo servidor.';
    } else if (code === 1006) {
      message = 'A conexão caiu (rede ou servidor indisponível).';
    } else {
      message = 'O servidor encerrou a sessão (código ' + code + ').';
    }
    return { message, keyProblem, detail };
  }

  function stopStream(stream) {
    if (!stream || typeof stream.getTracks !== 'function') return;
    for (const track of stream.getTracks()) {
      try {
        track.stop();
      } catch (_) {
        // Uma faixa que ja parou nao impede as outras de pararem.
      }
    }
  }

  function mediaError(what, error) {
    const name = (error && error.name) || '';
    if (name === 'NotAllowedError' || name === 'SecurityError' || name === 'InvalidStateError') {
      return what === 'screen'
        ? 'Tela não compartilhada. Clique em «Tela» para escolher uma janela ou a tela inteira.'
        : 'Sem permissão para ' + (what === 'camera' ? 'a câmera' : 'o microfone') + '.';
    }
    if (name === 'NotFoundError' || name === 'OverconstrainedError') {
      return what === 'camera' ? 'Nenhuma câmera encontrada.' : 'Nenhum microfone encontrado.';
    }
    const label = what === 'screen' ? 'a tela' : what === 'camera' ? 'a câmera' : 'o microfone';
    return 'Não foi possível ativar ' + label + '.';
  }

  const MIC_CONSTRAINTS = {
    audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 }
  };
  const CAMERA_CONSTRAINTS = { video: { width: { ideal: 640 }, height: { ideal: 480 } } };
  const SCREEN_CONSTRAINTS = { video: true, audio: false };
  const SOURCES = ['screen', 'camera', 'mic'];

  /// Uma sessao ao vivo. `options.env` traz o navegador (ou os dublês dos
  /// testes), `options.ui` recebe o que mostrar, `options.key` e a chave.
  ///
  /// `ui.notice(fonte, texto)` guarda um aviso por fonte ('screen', 'camera',
  /// 'mic'); texto vazio apaga so o dessa fonte. `ui.ended()` diz que a sessao
  /// acabou sem o utilizador a desligar.
  function createSession(options) {
    const env = options.env;
    const ui = options.ui || {};
    const key = options.key;
    const notify = (name, ...args) => {
      if (typeof ui[name] === 'function') ui[name](...args);
    };

    const state = {
      started: false,
      closed: false,
      ready: false,
      socket: null,
      resumeHandle: null,
      resumes: 0,
      dropResumes: 0,
      goAwayResumes: 0,
      context: null,
      frameTimer: null,
      grabbing: false,
      tickets: { mic: 0, camera: 0, screen: 0 },
      streams: { mic: null, camera: null, screen: null },
      mic: null,
      downsample: null,
      micQueue: [],
      micQueued: 0,
      playing: [],
      playhead: 0
    };

    function sources() {
      return {
        screen: !!state.streams.screen,
        camera: !!state.streams.camera,
        mic: !!state.streams.mic
      };
    }

    function send(message) {
      const socket = state.socket;
      if (!socket || state.closed || !state.ready || socket.readyState !== 1) return false;
      socket.send(JSON.stringify(message));
      return true;
    }

    function takeMicQueue() {
      const joined = new Int16Array(state.micQueued);
      let at = 0;
      for (const part of state.micQueue) {
        joined.set(part, at);
        at += part.length;
      }
      state.micQueue = [];
      state.micQueued = 0;
      return joined;
    }

    function onMicChunk(chunk) {
      if (state.closed || !state.ready || !state.downsample || !state.streams.mic) return;
      const samples = state.downsample(chunk);
      if (!samples.length) return;
      state.micQueue.push(samples);
      state.micQueued += samples.length;
      if (state.micQueued < MIC_CHUNK_SAMPLES) return;
      send(audioMessage(takeMicQueue()));
    }

    /// O microfone desligado a meio de uma frase: o que estava na fila sai, e
    /// o servidor fica a saber que o audio acabou. Sem isto a deteccao de voz
    /// dele fica a espera do resto e a IA nao responde.
    function endAudioStream() {
      if (state.micQueued > 0) send(audioMessage(takeMicQueue()));
      send(audioStreamEndMessage());
    }

    function flushPlayback() {
      for (const source of state.playing) {
        try {
          source.stop();
        } catch (_) {
          // Ja tinha acabado.
        }
      }
      state.playing = [];
      state.playhead = state.context ? state.context.currentTime : 0;
    }

    function play(chunk) {
      const context = state.context;
      if (!context || state.closed) return;
      const samples = pcm16ToFloat32(base64ToPcm16(chunk.data));
      if (!samples.length) return;
      const rate = chunk.rate || OUTPUT_RATE;
      const buffer = context.createBuffer(1, samples.length, rate);
      buffer.getChannelData(0).set(samples);
      const source = context.createBufferSource();
      source.buffer = buffer;
      source.connect(context.destination);
      // Sem buracos: cada bloco comeca onde o anterior acaba, ou agora se o
      // anterior ja acabou.
      const at = Math.max(state.playhead, context.currentTime);
      source.start(at);
      state.playhead = at + samples.length / rate;
      state.playing.push(source);
      source.onended = () => {
        const index = state.playing.indexOf(source);
        if (index >= 0) state.playing.splice(index, 1);
      };
    }

    function handleText(text) {
      if (state.closed) return;
      const message = parseServerMessage(text);
      if (!message) return;
      if (message.resumeHandle) state.resumeHandle = message.resumeHandle;
      if (message.setupComplete) {
        state.ready = true;
        state.resumes = 0;
        notify('status', 'Ao vivo: a IA vê e ouve o que está ligado.', 'live');
      }
      if (message.interrupted) flushPlayback();
      for (const chunk of message.audio) play(chunk);
      if (message.inputText) notify('transcript', 'user', message.inputText);
      if (message.outputText) notify('transcript', 'model', message.outputText);
      if (message.turnComplete) notify('turnComplete');
      if (message.error) notify('error', 'Erro do servidor: ' + message.error, false, '');
      // Por ultimo: retomar troca o socket, e o resto desta mensagem ainda
      // era da ligacao velha.
      if (message.goAway && !resume('goaway')) {
        notify('status', 'O servidor vai encerrar a sessão em breve…', 'warn');
      }
    }

    function sendFrame() {
      if (state.closed || !state.ready || state.grabbing) return;
      if (!state.streams.screen && !state.streams.camera) return;
      state.grabbing = true;
      Promise.resolve()
        .then(() => env.grabFrame({ screen: state.streams.screen, camera: state.streams.camera }))
        .then((jpeg) => {
          if (jpeg) send(videoMessage(jpeg));
        })
        .catch(() => {})
        .then(() => {
          state.grabbing = false;
        });
    }

    function release(which) {
      const stream = state.streams[which];
      state.streams[which] = null;
      if (which === 'mic' && state.mic) {
        try {
          state.mic.disconnect();
        } catch (_) {
          // O grafo ja estava desfeito.
        }
        state.mic = null;
        state.downsample = null;
        state.micQueue = [];
        state.micQueued = 0;
      }
      stopStream(stream);
      if (which === 'camera') notify('camera', null);
      if (which === 'screen') notify('screen', null);
    }

    // Liga ou desliga uma fonte. Cada pedido leva um bilhete: uma permissao
    // que chega depois de desligar (ou de parar a sessao, ou de um pedido mais
    // novo) larga logo a faixa. A camera nao pode ficar acesa so porque o
    // utilizador foi mais rapido do que o aviso de permissao.
    async function toggle(which, on) {
      const ticket = ++state.tickets[which];
      const current = () => !state.closed && state.tickets[which] === ticket;
      if (!on || state.closed) {
        if (which === 'mic' && state.streams.mic && !state.closed) endAudioStream();
        release(which);
        notify('sources', sources());
        return false;
      }
      if (state.streams[which]) return true;
      let stream;
      try {
        if (which === 'screen') stream = await env.getDisplayMedia(SCREEN_CONSTRAINTS);
        else stream = await env.getUserMedia(which === 'mic' ? MIC_CONSTRAINTS : CAMERA_CONSTRAINTS);
      } catch (error) {
        if (current()) notify('notice', which, mediaError(which, error));
        notify('sources', sources());
        return false;
      }
      if (!current() || state.streams[which]) {
        stopStream(stream);
        return !!state.streams[which] && !state.closed;
      }
      if (which === 'mic') {
        let mic;
        try {
          mic = await env.createMic(state.context, stream, onMicChunk);
        } catch (_) {
          stopStream(stream);
          if (current()) notify('notice', 'mic', mediaError('mic', null));
          notify('sources', sources());
          return false;
        }
        if (!current() || state.streams.mic) {
          try {
            mic.disconnect();
          } catch (_) {
            // Grafo ja desfeito.
          }
          stopStream(stream);
          return !!state.streams.mic && !state.closed;
        }
        state.mic = mic;
        state.downsample = createDownsampler(mic.rate || state.context.sampleRate, INPUT_RATE);
      }
      state.streams[which] = stream;
      // A fonte ligou: o aviso dela (se havia) ja nao e verdade.
      notify('notice', which, '');
      if (which === 'camera') notify('camera', stream);
      if (which === 'screen') {
        notify('screen', stream);
        // "Parar compartilhamento" na barra do Windows termina a faixa por fora.
        const [track] = typeof stream.getVideoTracks === 'function' ? stream.getVideoTracks() : [];
        if (track) {
          track.onended = () => {
            if (state.streams.screen === stream) toggle('screen', false);
          };
        }
      }
      notify('sources', sources());
      return true;
    }

    /// Larga um socket sem o tratar como o fim da sessao.
    function detach(socket) {
      socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
      if (socket.readyState === 0 || socket.readyState === 1) {
        try {
          socket.close(1000);
        } catch (_) {
          // Ja estava a fechar.
        }
      }
    }

    /// Abre uma ligacao. So a ligacao atual (`state.socket`) conta: as
    /// mensagens e o fecho de uma que ja foi substituida sao ignorados.
    function connect(handle) {
      const socket = new env.WebSocket(socketUrl(key));
      state.socket = socket;
      state.ready = false;
      socket.onopen = () => {
        if (state.closed || state.socket !== socket) return;
        socket.send(JSON.stringify(setupMessage(handle)));
        notify('status', 'Preparando a sessão…', 'connecting');
      };
      socket.onmessage = (event) => {
        frameText(event.data).then((text) => {
          if (state.socket === socket) handleText(text);
        }, () => {});
      };
      socket.onclose = (event) => {
        if (state.closed || state.socket !== socket) return;
        lost(event && event.code, event && event.reason);
      };
    }

    /// Continua a mesma sessao numa ligacao nova, com o ultimo handle. A tela,
    /// a camera e o microfone ficam como estao: so o socket muda.
    function resume(kind) {
      if (state.closed || !state.resumeHandle || state.resumes >= MAX_RESUMES) return false;
      if (kind === 'drop' ? state.dropResumes >= MAX_DROP_RESUMES : state.goAwayResumes >= MAX_GOAWAY_RESUMES) {
        return false;
      }
      state.resumes += 1;
      if (kind === 'drop') state.dropResumes += 1;
      else state.goAwayResumes += 1;
      const old = state.socket;
      state.socket = null;
      state.ready = false;
      if (old) detach(old);
      // O audio a meio da fila era da ligacao velha.
      state.micQueue = [];
      state.micQueued = 0;
      notify('status', 'Reconectando ao Gemini…', 'connecting');
      connect(state.resumeHandle);
      return true;
    }

    /// A ligacao fechou sem o utilizador pedir. Retoma se puder; senao para
    /// tudo e diz porque.
    function lost(code, reason) {
      const closed = describeClose(code, reason);
      if (!closed.keyProblem && resume('drop')) return;
      const exhausted = !closed.keyProblem && state.dropResumes >= MAX_DROP_RESUMES;
      stop();
      notify('error', exhausted ? TOO_MANY_DROPS : closed.message, closed.keyProblem, closed.detail);
      notify('ended');
    }

    async function start() {
      if (state.started) return;
      state.started = true;
      notify('status', 'Conectando ao Gemini…', 'connecting');
      state.context = new env.AudioContext();
      connect(null);
      state.frameTimer = env.setInterval(sendFrame, FRAME_INTERVAL_MS);
      await Promise.all([toggle('mic', true), toggle('camera', true), toggle('screen', true)]);
    }

    /// Desliga tudo: faixas, microfone, som, temporizador, socket e contexto.
    function stop() {
      if (state.closed) return;
      state.closed = true;
      state.ready = false;
      if (state.frameTimer !== null) {
        env.clearInterval(state.frameTimer);
        state.frameTimer = null;
      }
      release('mic');
      release('camera');
      release('screen');
      flushPlayback();
      const socket = state.socket;
      state.socket = null;
      if (socket) detach(socket);
      const context = state.context;
      state.context = null;
      if (context && context.state !== 'closed') {
        try {
          const closing = context.close();
          if (closing && typeof closing.catch === 'function') closing.catch(() => {});
        } catch (_) {
          // Contexto ja fechado.
        }
      }
      // Com tudo desligado, os avisos das fontes ja nao dizem nada.
      for (const which of SOURCES) notify('notice', which, '');
      notify('sources', sources());
      notify('status', 'Desligado.', 'idle');
    }

    function resumeAudio() {
      const context = state.context;
      if (context && context.state === 'suspended' && typeof context.resume === 'function') {
        return Promise.resolve(context.resume()).catch(() => {});
      }
      return Promise.resolve();
    }

    return {
      start,
      stop,
      resumeAudio,
      setMic: (on) => toggle('mic', on),
      setCamera: (on) => toggle('camera', on),
      setScreen: (on) => toggle('screen', on),
      sources,
      get audioSuspended() {
        return !!state.context && state.context.state === 'suspended';
      },
      get live() {
        return state.ready && !state.closed;
      }
    };
  }

  root.NeuraliaLiveCore = Object.freeze({
    MODEL,
    ENDPOINT,
    INPUT_RATE,
    OUTPUT_RATE,
    FRAME_MAX_SIDE,
    FRAME_INTERVAL_MS,
    MIC_CHUNK_SAMPLES,
    KEY_MAX_CHARS,
    INVALID_KEY_NOTICE,
    MAX_RESUMES,
    MAX_DROP_RESUMES,
    MAX_GOAWAY_RESUMES,
    SYSTEM_INSTRUCTION,
    WORKLET_NAME,
    WORKLET_SOURCE,
    socketUrl,
    setupMessage,
    floatToInt16,
    createDownsampler,
    pcm16ToBase64,
    base64ToPcm16,
    pcm16ToFloat32,
    audioMessage,
    audioStreamEndMessage,
    videoMessage,
    frameSize,
    pictureInPicture,
    frameText,
    parseServerMessage,
    describeClose,
    createSession
  });
})(typeof globalThis !== 'undefined' ? globalThis : this);
