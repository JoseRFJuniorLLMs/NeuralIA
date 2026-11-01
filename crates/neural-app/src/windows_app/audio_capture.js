(() => {
  'use strict';
  if (window.top !== window || location.protocol !== 'https:' ||
      !['chatgpt.com','www.chatgpt.com','chat.openai.com','gemini.google.com','claude.ai','www.claude.ai'].includes(location.hostname)) return;
  // Capture only decoded playback buffers. Microphone streams and audio input
  // nodes are never observed or connected to this recorder.
  const W = window, Source = W.AudioBufferSourceNode;
  if (!Source || !W.chrome || !W.chrome.webview) return;
  const apply = Reflect.apply, stringify = JSON.stringify, create = Object.create;
  const post = W.chrome.webview.postMessage.bind(W.chrome.webview);
  const later = W.setTimeout.bind(W), cancel = W.clearTimeout.bind(W);
  const start = Source.prototype.start;
  const getData = W.AudioBuffer.prototype.getChannelData;
  const FloatArray = W.Float32Array, ByteArray = W.Uint8Array, Data = W.DataView;
  const groups = new WeakMap(), activeGroups = new Set(), cap = '__AUDIO_CAP__';
  const MAX_SAMPLES = 8 * 1024 * 1024, CHUNK = 2048;
  let nextId = 0, queuedBytes = 0, retainedSamples = 0, queue = Promise.resolve();
  let enabled = __AUDIO_ENABLED__, generation = 0;
  const hex = '0123456789abcdef';
  W.chrome.webview.addEventListener('message', event => {
    if (!event.isTrusted || !event.data || typeof event.data.neuraliaAudioEnabled !== 'boolean') return;
    enabled = event.data.neuraliaAudioEnabled;
    if (!enabled) {
      generation++;
      for (const group of activeGroups) {
        group.cancelled = true; cancel(group.timer); group.segments.length = 0;
      }
      activeGroups.clear(); retainedSamples = 0;
    }
  });

  function packet(kind, id, seq, total, data) {
    const obj = create(null);
    obj.v = 1; obj.cap = cap; obj.kind = kind; obj.id = id;
    obj.seq = seq; obj.total = total; obj.data = data;
    post(stringify(obj));
  }
  function transfer(bytes) {
    if (!enabled || queuedBytes + bytes.length > 32 * 1024 * 1024) return;
    const run = generation;
    queuedBytes += bytes.length;
    queue = queue.then(async () => {
      if (!enabled || run !== generation) return;
      const id = (++nextId) >>> 0; let seq = 0;
      packet('start', id, 0, bytes.length, '');
      for (let at = 0; at < bytes.length; at += CHUNK) {
        if (!enabled || run !== generation) return;
        let text = '';
        for (let i = at; i < Math.min(bytes.length, at + CHUNK); ++i) {
          const byte = bytes[i]; text += hex[byte >>> 4] + hex[byte & 15];
        }
        packet('chunk', id, seq++, bytes.length, text);
        // Bounded packets and a yield keep playback/UI responsive.
        if (seq % 16 === 0) await new Promise(resolve => later(resolve, 0));
      }
      packet('end', id, seq, bytes.length, '');
    }).catch(() => {}).then(() => { queuedBytes -= bytes.length; });
  }
  function flush(ctx, group) {
    if (groups.get(ctx) !== group || !group.samples || group.cancelled || !enabled) return;
    groups.delete(ctx);
    activeGroups.delete(group);
    retainedSamples -= group.retained;
    const pcm = new FloatArray(group.samples);
    for (const segment of group.segments) {
      const at = Math.max(0, Math.round((segment.when - group.base) * group.rate));
      for (let i = 0; i < segment.data.length && at + i < pcm.length; ++i) pcm[at + i] += segment.data[i];
    }
    const bytes = new ByteArray(44 + pcm.length * 2), view = new Data(bytes.buffer);
    const ascii = (at, text) => { for (let i = 0; i < text.length; i++) bytes[at + i] = text.charCodeAt(i); };
    ascii(0, 'RIFF'); view.setUint32(4, bytes.length - 8, true); ascii(8, 'WAVEfmt ');
    view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
    view.setUint32(24, group.rate, true); view.setUint32(28, group.rate * 2, true);
    view.setUint16(32, 2, true); view.setUint16(34, 16, true); ascii(36, 'data');
    view.setUint32(40, pcm.length * 2, true);
    for (let i = 0; i < pcm.length; ++i) {
      const value = Math.max(-1, Math.min(1, pcm[i]));
      view.setInt16(44 + i * 2, value < 0 ? value * 32768 : value * 32767, true);
    }
    transfer(bytes);
  }
  function capture(node, args) {
    if (!enabled) return;
    const buffer = node.buffer, ctx = node.context;
    if (!buffer || !ctx || buffer.numberOfChannels < 1 || buffer.numberOfChannels > 32 ||
        buffer.sampleRate < 8000 || buffer.sampleRate > 192000) return;
    const offset = Math.max(0, Number(args[1]) || 0);
    const available = Math.max(0, buffer.duration - offset);
    const duration = Math.min(available, args.length > 2 ? Math.max(0, Number(args[2])) : available);
    const speed = node.playbackRate.value * Math.pow(2, node.detune.value / 1200);
    if (!Number.isFinite(duration) || duration <= 0 || !Number.isFinite(speed) || speed <= 0) return;
    const when = Math.max(ctx.currentTime, Number(args[0]) || 0);
    let group = groups.get(ctx);
    if (!group || group.cancelled) {
      if (activeGroups.size >= 16) return;
      group = { base: when, rate: Math.round(buffer.sampleRate), samples: 0, retained: 0, segments: [], timer: null };
      groups.set(ctx, group); activeGroups.add(group);
    }
    const count = Math.floor(duration * group.rate / speed);
    const at = Math.max(0, Math.round((when - group.base) * group.rate));
    if (count < 1 || count > MAX_SAMPLES || at + count > MAX_SAMPLES ||
        retainedSamples + count > MAX_SAMPLES || group.segments.length >= 2048) {
      if (!group.samples) { groups.delete(ctx); activeGroups.delete(group); }
      return;
    }
    const data = new FloatArray(count), channels = [];
    for (let channel = 0; channel < buffer.numberOfChannels; ++channel) channels.push(apply(getData, buffer, [channel]));
    for (let i = 0; i < count; ++i) {
      const pos = offset * buffer.sampleRate + i * buffer.sampleRate * speed / group.rate;
      const left = Math.min(buffer.length - 1, Math.floor(pos)), right = Math.min(buffer.length - 1, left + 1);
      const fraction = pos - left; let value = 0;
      for (let channel = 0; channel < channels.length; ++channel) {
        const samples = channels[channel]; value += samples[left] * (1 - fraction) + samples[right] * fraction;
      }
      data[i] = value / channels.length;
    }
    retainedSamples += count; group.retained += count;
    group.segments.push({ when, data }); group.samples = Math.max(group.samples, at + count);
    if (group.timer !== null) cancel(group.timer);
    const end = group.base + group.samples / group.rate;
    group.timer = later(() => flush(ctx, group), Math.min(600000, Math.max(1000, (end - ctx.currentTime) * 1000 + 1000)));
  }
  try {
    Object.defineProperty(Source.prototype, 'start', {
      configurable: true, writable: true,
      value: function () {
        const result = apply(start, this, arguments);
        try { capture(this, arguments); } catch (_) {}
        return result;
      }
    });
  } catch (_) {}
})();
