import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const defaultScript = fileURLToPath(new URL('../crates/neural-app/src/windows_app/audio_capture.js', import.meta.url));
const source = readFileSync(process.argv[2] || defaultScript, 'utf8');
const CAP = '0123456789abcdef0123456789abcdef';
function harness({ enabled = true, host = 'gemini.google.com', frame = false } = {}) {
  let allocated = 0, originalCalls = 0, timerId = 0;
  const packets = [], timers = new Map(), listeners = new Map();
  class AudioBuffer {
    constructor(data = [0, 0.25, -0.5, 1], sampleRate = 8000) {
      this.channels = [Float32Array.from(data)]; this.numberOfChannels = 1;
      this.length = data.length; this.sampleRate = sampleRate; this.duration = this.length / sampleRate;
    }
    getChannelData(channel) { return this.channels[channel]; }
  }
  class AudioBufferSourceNode {
    constructor(context, buffer) {
      this.context = context; this.buffer = buffer;
      this.playbackRate = { value: 1 }; this.detune = { value: 0 };
    }
    start() { originalCalls++; }
  }
  class TrackedArray extends Float32Array {
    constructor(size) { super(size); allocated += this.length; }
  }
  const browser = {
    AudioBuffer, AudioBufferSourceNode, Float32Array: TrackedArray,
    Uint8Array, DataView,
    location: { hostname: host, protocol: 'https:' },
    setTimeout(fn, delay) { const id = ++timerId; timers.set(id, { fn, delay }); return id; },
    clearTimeout(id) { timers.delete(id); },
    chrome: { webview: {
      postMessage(text) { assert.equal(typeof text, 'string'); assert.ok(text.length < 8192); packets.push(JSON.parse(text)); },
      addEventListener(name, fn) { listeners.set(name, fn); },
    } },
  };
  browser.window = browser; browser.top = frame ? {} : browser;
  const context = vm.createContext(browser);
  vm.runInContext(source.replaceAll('__AUDIO_CAP__', CAP).replaceAll('__AUDIO_ENABLED__', String(enabled)), context);
  return {
    packets, timers, context,
    get allocated() { return allocated; }, get originalCalls() { return originalCalls; },
    buffer: data => new AudioBuffer(data),
    play(buffer, ctx = { currentTime: 0 }, ...args) { new AudioBufferSourceNode(ctx, buffer).start(...args); },
    state(enabled, isTrusted = true) { listeners.get('message')?.({ isTrusted, data: { neuraliaAudioEnabled: enabled } }); },
    async finish() {
      for (let turn = 0; turn < 10000; turn++) {
        await Promise.resolve(); await Promise.resolve();
        const next = timers.entries().next().value;
        if (next) { timers.delete(next[0]); next[1].fn(); }
        else { await Promise.resolve(); if (!timers.size) break; }
      }
    },
  };
}
function clips(packets) {
  const result = []; let state;
  for (const packet of packets) {
    assert.equal(packet.cap, CAP); assert.equal(packet.v, 1);
    if (packet.kind === 'start') state = { id: packet.id, total: packet.total, seq: 0, chunks: [] };
    else if (packet.kind === 'chunk') {
      assert.equal(packet.id, state.id); assert.equal(packet.seq, state.seq++);
      assert.ok(packet.data.length <= 4096); state.chunks.push(Buffer.from(packet.data, 'hex'));
    } else if (packet.kind === 'end') {
      assert.equal(packet.seq, state.seq); const bytes = Buffer.concat(state.chunks);
      assert.equal(bytes.length, state.total); result.push(bytes); state = undefined;
    }
  }
  return result;
}

{
  const h = harness(), buffer = h.buffer([0, 0.25, -0.5, 1]), ctx = { currentTime: 0 };
  h.play(buffer, ctx, 0); h.play(buffer, ctx, 4 / 8000);
  assert.equal(h.originalCalls, 2); await h.finish();
  const [wav] = clips(h.packets); assert.ok(wav); assert.equal(clips(h.packets).length, 1);
  assert.equal(wav.toString('ascii', 0, 4), 'RIFF'); assert.equal(wav.readUInt32LE(24), 8000);
  assert.equal(wav.readUInt32LE(40), 16); // Two scheduled buffers, one clip.
  assert.equal(wav.readInt16LE(48), -16384); assert.equal(wav.readInt16LE(50), 32767);
}
{
  const h = harness(); h.play(h.buffer([0, 0.25, -0.5, 1]), { currentTime: 0 }, 0, 1 / 8000, 2 / 8000);
  await h.finish(); const [wav] = clips(h.packets);
  assert.equal(wav.readUInt32LE(40), 4); assert.equal(wav.readInt16LE(44), 8191);
}
{
  const h = harness(); const buffer = h.buffer(new Float32Array(8192)), ctx = { currentTime: 0 };
  for (let i = 0; i < 2048; i++) h.play(buffer, ctx, 0);
  assert.equal(h.originalCalls, 2048);
  assert.ok(h.allocated <= 8 * 1024 * 1024, `Overlapping buffers retained ${h.allocated} samples`);
}
{
  const h = harness(), buffer = h.buffer([0.5]);
  for (let i = 0; i < 1000; i++) h.play(buffer, { currentTime: 0 }, 0);
  assert.ok(h.allocated <= 16, `Unbounded AudioContext groups: ${h.allocated}`);
}
{
  const h = harness({ enabled: false }), buffer = h.buffer([0.5]);
  h.play(buffer); assert.equal(h.allocated, 0); h.state(true, false); h.play(buffer);
  assert.equal(h.allocated, 0); // Page-crafted state messages have no authority.
  h.state(true); h.play(buffer); assert.equal(h.allocated, 1);
  h.state(false); await h.finish(); assert.equal(h.packets.length, 0);
  assert.equal(h.originalCalls, 3); // Pause never interrupts provider playback.
}
for (const options of [{ frame: true }, { host: 'attacker.test' }]) {
  const h = harness(options); h.play(h.buffer([1])); await h.finish();
  assert.equal(h.packets.length, 0); assert.equal(h.allocated, 0);
}
{
  const h = harness();
  vm.runInContext('globalThis.leaks = 0; Object.prototype.toJSON = function () { globalThis.leaks++; return this; };', h.context);
  h.play(h.buffer([1])); await h.finish(); assert.equal(h.context.leaks, 0);
  assert.equal(clips(h.packets).length, 1); // Capability never reaches page toJSON hooks.
}
console.log('Audio capture: waveform, offsets, grouping, bounded overlap/contexts, pause, origins and capability isolation passed.');
