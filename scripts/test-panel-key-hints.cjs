// Runs the shipping panel keyboard path, with no browser/API account required.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../assets/panel/key-hints.js'), 'utf8');
function exercise(code) {
  const listeners = {}, buttons = [], rows = [];
  let observe;
  function button(text) {
    const node = {textContent:text, title:'', id:'', style:{}, hidden:false, isConnected:true,
      disabled:false, clicks:0, attrs:{}, getClientRects(){return this.noLayout ? [] : [{}];},
      closest(){return this.hidden || this.hiddenAncestor ? {} : null;},
      focus(){this.focused=true;}, click(){this.clicks++;},
      setAttribute(k,v){this.attrs[k]=v;},getAttribute(k){return this.attrs[k] || null;}};
    return node;
  }
  const a=button('Salvar'), b=button('Excluir'), confirm=button('Confirmar exclusao');
  confirm.hiddenAncestor=true; buttons.push(a,b,confirm);
  const document = {createElement:()=>button(''),body:{appendChild(){}},
    querySelectorAll:()=>buttons,
    addEventListener(type,fn){(listeners[type] ||= []).push(fn);}};
  const context={document,MutationObserver:class{constructor(fn){observe=fn;} observe(){}}};
  vm.createContext(context); vm.runInContext(code,context);
  context.installPanelKeyHints(document, value=>rows.splice(0,rows.length,...value));
  function key(k, extra={}) {
    const e={key:k,preventDefault(){this.defaultPrevented=true;},stopImmediatePropagation(){},...extra};
    for(const fn of listeners.keydown)fn(e); return e;
  }
  function run(number){key('F2');for(const digit of String(number))key(digit);key('Enter');}
  assert.match(a.title,/F2, 1, Enter/); assert.match(b.title,/F2, 2, Enter/);
  assert.equal(rows.length,3);run(1);assert.equal(a.clicks,1);
  a.disabled=true;run(1);assert.equal(a.clicks,1);a.disabled=false;
  run(3);assert.equal(confirm.clicks,0,'hidden confirmation must not execute');
  confirm.hiddenAncestor=false;run(3);assert.equal(confirm.clicks,1);
  a.noLayout=true;run(1);assert.equal(a.clicks,1);a.noLayout=false;
  a.isConnected=false;run(1);assert.equal(a.clicks,1);a.isConnected=true;
  key('F2');key('2');key('Escape');key('Enter');assert.equal(b.clicks,0);
  key('F2');key('2');listeners.pointerdown[0]();key('Enter');assert.equal(b.clicks,0);
  key('F2');key('2');key('Enter',{repeat:true});key('Escape');assert.equal(b.clicks,0);
  key('F2',{defaultPrevented:true});key('2');key('Enter');assert.equal(b.clicks,0);
  b.textContent='×';b.title='Excluir nota';observe();assert.equal(b.title,'Excluir nota (F2, 2, Enter)');
  assert.equal(rows[1].label,'Painel: Excluir nota');
  observe();assert.equal((b.title.match(/F2/g)||[]).length,1);
  const dynamic=button('Cancelar download');buttons.push(dynamic);observe();
  assert.equal(rows.length,4);assert.match(dynamic.title,/F2, 4, Enter/);run(4);assert.equal(dynamic.clicks,1);
  buttons.splice(3,1);observe();assert.equal(rows.length,3);run(4);assert.equal(dynamic.clicks,1);
}
exercise(source);
if(process.argv.includes('--mutation-check')){
  const changed=source.replace('action && available(action.node)', 'action');
  assert.notEqual(source,changed);assert.throws(()=>exercise(changed),/hidden confirmation|strictly equal/);
  console.log('PASS mutation: bypassing availability is detected; shipping source unchanged');
}
console.log('PASS panel shortcuts: hints, map, dynamic buttons, dispatch, cancel and hidden/disabled guards');
