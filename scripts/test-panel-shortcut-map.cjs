// Executes the assembled shipping panel and its DOM harness, without accounts.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const root = path.resolve(__dirname, '..');
const panelDir = path.join(root, 'crates/neural-app/src/windows_app');
const source = fs.readFileSync(path.join(panelDir, 'side_panel.rs'), 'utf8');
const concat = /const PANEL_HTML: &str = concat!\(([\s\S]*?)\n\);/.exec(source);
assert.ok(concat, 'shipping PANEL_HTML assembly');
let html = '';
const pieces = /include_str!\("([^"]+)"\)|r#"([\s\S]*?)"#|"((?:\\.|[^"\\])*)"/g;
for (const match of concat[1].matchAll(pieces)) {
  html += match[1] ? fs.readFileSync(path.resolve(panelDir, match[1]), 'utf8')
    : match[2] !== undefined ? match[2] : JSON.parse('"' + match[3] + '"');
}
const nativeRows = [{label: 'Catálogo inicial', keys: 'Ctrl+T'}];
html = html.replace('__THEME__', '{}').replaceAll('__NEURALIA_VERSION__', 'test')
  .replace('__SHORTCUT_ROWS__', JSON.stringify(nativeRows));
const tests = fs.readFileSync(path.join(panelDir, 'tests.rs'), 'utf8');
const harness = /const PANEL_DOM_HARNESS: &str = r##"([\s\S]*?)"##;/.exec(tests);
assert.ok(harness, 'shipping panel DOM harness');
function exercise(page) {
  const input = {html: page, steps: [String.raw`
__out.initialNative = $('shortcut-rows').textContent.includes('Catálogo inicial');
__click($('tab-shortcuts'));
__out.rows = $('shortcut-rows').children.length;
__out.visible = __visible($('view-shortcuts'));
__out.selected = $('tab-shortcuts').getAttribute('aria-selected');
__out.focusVisible = __visible(document.activeElement);
__out.focusTag = document.activeElement.tagName;
const dynamic = document.createElement('button');
dynamic.textContent = 'Botão criado depois';
$('view-history').appendChild(dynamic);
window.neuraliaShowSection('history');
window.neuraliaShowSection('shortcuts');
__out.dynamic = $('shortcut-rows').textContent.includes('Botão criado depois');
window.neuraliaSetShortcuts([{label:'Catálogo atualizado',keys:'Alt+9'}]);
__out.lateNative = $('shortcut-rows').textContent.includes('Catálogo atualizadoAlt+9');
__out.localAfterNative = $('shortcut-rows').textContent.includes('Botão criado depois');
`]};
  let output = '';
  vm.runInNewContext('const INPUT = ' + JSON.stringify(input) + ';\n' + harness[1], {
    require, TextEncoder, URL, process: {stdout: {write(value) { output += value; }}}
  });
  const result = JSON.parse(output);
  assert.deepEqual(result.errors, [], 'assembled panel executes without exceptions');
  assert.deepEqual(result.html, [], 'catalog renders as text');
  assert.equal(result.out.initialNative, true, 'catalog supplied before native ready');
  assert.ok(result.out.rows > 20, 'local button hints are listed');
  assert.equal(result.out.visible, true, 'selected shortcuts section is visible');
  assert.equal(result.out.selected, 'true');
  assert.equal(result.out.focusVisible, true, 'focus remains on visible content');
  assert.equal(result.out.focusTag, 'MAIN');
  assert.equal(result.out.dynamic, true, 'opening shortcuts refreshes dynamic buttons');
  assert.equal(result.out.lateNative, true, 'late native refresh updates catalog');
  assert.equal(result.out.localAfterNative, true, 'native refresh preserves local hints');
}
exercise(html);
function exerciseAudio(page) {
  const input = {html: page, steps: [String.raw`
window.neuraliaShowSection('audio');
__out.audioVisible = __visible($('view-audio'));
__out.initialAudioEnabled = $('audio-enabled').getAttribute('checked') !== null || !!$('audio-enabled').checked;
window.__neuraliaAudio.render({enabled:true,status:'Pronto',items:[{id:17,title:'<img src=x onerror=evil()>',detail:'Voz original',busy:false}]});
__out.audioEnabled = $('audio-enabled').checked;
__out.audioTitle = $('audio-list').children[0].children[0].textContent;
__click($('audio-list').children[0].children[2]);
__click($('audio-refresh'));
$('audio-enabled').checked = false; __fire($('audio-enabled'), 'change');
__click($('audio-clear'));
window.__neuraliaAudio.render({enabled:false,status:'Exportando',items:[{id:18,title:'Trecho',detail:'PCM',busy:true}]});
__out.busy = $('audio-list').children[0].children[2].disabled;
`]};
  let output = '';
  vm.runInNewContext('const INPUT = ' + JSON.stringify(input) + ';\n' + harness[1], {
    require, TextEncoder, URL, process: {stdout: {write(value) { output += value; }}}
  });
  const result = JSON.parse(output), messages = result.posted.map(JSON.parse);
  assert.deepEqual(result.errors, []);
  assert.deepEqual(result.html, [], 'audio data renders as text');
  assert.equal(result.out.audioVisible, true);
  assert.equal(result.out.initialAudioEnabled, false, 'audio capture requires session opt-in');
  assert.equal(result.out.audioEnabled, true);
  assert.equal(result.out.audioTitle, '<img src=x onerror=evil()>');
  assert.equal(result.out.busy, true);
  for (const expected of [
    {action:'audio-export',args:{id:17}}, {action:'audio-list',args:{}},
    {action:'audio-enabled',args:{on:false}}, {action:'audio-clear',args:{}},
  ]) assert.ok(messages.some(message => JSON.stringify(message) === JSON.stringify(expected)), 'panel dispatch ' + expected.action);
}
exerciseAudio(html);
if (process.argv.includes('--mutation-check')) {
  const withoutBootstrap = html.replace(JSON.stringify(nativeRows), '[]');
  assert.notEqual(withoutBootstrap, html);
  assert.throws(() => exercise(withoutBootstrap), /catalog supplied before native ready/);
  const withoutRefresh = html.replace('      panelKeyHints.refresh();', '');
  assert.notEqual(withoutRefresh, html);
  assert.throws(() => exercise(withoutRefresh), /opening shortcuts refreshes dynamic buttons/);
  const autoCapture = html.replace('id="audio-enabled" type="checkbox"', 'id="audio-enabled" type="checkbox" checked');
  assert.notEqual(autoCapture, html);
  assert.throws(() => exerciseAudio(autoCapture), /audio capture requires session opt-in/);
  const withoutExport = html.replace("post('audio-export', {id:item.id})", "post('audio-list')");
  assert.notEqual(withoutExport, html);
  assert.throws(() => exerciseAudio(withoutExport), /panel dispatch audio-export/);
  console.log('PASS mutations: missing initial catalog and missing refresh both fail; files unchanged');
}
console.log('PASS assembled shortcuts: initial catalog, visible tab, focus, dynamic hints and native refresh');
console.log('PASS assembled audio: typed export, text safety, pause and clear');
