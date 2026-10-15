// Runs the shipping Live core against the same fakes used by Rust tests.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
require(path.join(root, 'assets/live/live-core.js'));
const core = globalThis.NeuraliaLiveCore;
const rust = fs.readFileSync(path.join(root, 'crates/neural-app/src/gemini_live.rs'), 'utf8');
const fakes = /const SESSION_FAKES: &str = r#"([\s\S]*?)"#;/.exec(rust)[1];
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
async function scenario(name) {
  const section = rust.slice(rust.indexOf('fn ' + name));
  const body = /r#"([\s\S]*?)"#/.exec(section)[1];
  return new AsyncFunction('core', fakes + body)(core);
}
(async () => {
  const rotations = await scenario('requested_live_persists_across_rotations_and_drops_until_stop');
  assert.equal(rotations.before.sockets, 51);
  assert.equal(rotations.before.ended, 0);
  assert.deepEqual(rotations.before.stopped, [0, 0, 0]);
  assert.equal(rotations.canceled, true);
  assert.deepEqual(rotations.stopped, [1, 1, 1]);
  const retry = await scenario('requested_live_retries_without_handle_with_capped_backoff');
  assert.equal(retry.count, 11);
  assert.deepEqual(retry.delays, [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000, 30000]);
  assert.equal(retry.ended, 1);
  assert.deepEqual(retry.stopped, [1, 1, 1]);
  const recovery = await scenario('requested_live_invalidates_resumption_and_recovers_expired_handle');
  assert.equal(recovery.oldHandle, 'old');
  assert.deepEqual(recovery.freshSetup, {});
  assert.deepEqual(recovery.lastSetup, {});
  assert.equal(recovery.ended, 0);
  const names = core.LIVE_TOOLS.map(tool => tool.name);
  assert.equal(names.length, 44);
  assert.equal(new Set(names).size, 44);
  for (const name of ['desktop_move_mouse', 'browser_maps', 'browser_uber', 'browser_agent_plan']) assert(names.includes(name));
  console.log('PASS: 50 reconnects, expired resume handle recovery, capped backoff, explicit stop, fatal authentication error, 44 unique tools.');
})().catch(error => { console.error(error); process.exitCode = 1; });
