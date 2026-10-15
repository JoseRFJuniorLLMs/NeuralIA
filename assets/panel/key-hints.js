  // F2, numero, Enter: acesso a cada botao local, inclusive linhas dinamicas.
  // Dispara o mesmo click da interface, preservando confirmacoes existentes.
  function installPanelKeyHints(doc, changed) {
    const numbers = new WeakMap(), titles = new WeakMap();
    let next = 1, actions = new Map(), signature = '', active = false, digits = '';
    const status = doc.createElement('div');
    status.hidden = true;
    status.setAttribute('role', 'status');
    status.style.cssText = 'position:fixed;bottom:12px;left:12px;right:12px;z-index:1000;padding:12px;border:1px solid currentColor;border-radius:8px;background:var(--bg,#202124);color:var(--fg,#fff)';
    doc.body.appendChild(status);
    const available = node => node && node.isConnected && !node.disabled &&
      !node.closest('[hidden],[inert],[aria-hidden="true"]') && node.getClientRects().length > 0;
    const label = node => (node.getAttribute('aria-label') || (titles.get(node) || {}).base || node.title || node.textContent || node.id || 'Acao').trim().slice(0, 160);
    function refresh() {
      const updated = new Map();
      for (const node of doc.querySelectorAll('button,[role="button"],input[type="button"],input[type="submit"],input[type="checkbox"]')) {
        if (!numbers.has(node)) numbers.set(node, next++);
        const number = numbers.get(node), keys = 'F2, ' + number + ', Enter';
        const previous = titles.get(node);
        const base = previous && node.title === previous.full ? previous.base : (node.title || label(node));
        const full = base + ' (' + keys + ')';
        if (node.title !== full) node.title = full;
        titles.set(node, {base, full});
        updated.set(String(number), { node, label: label(node), keys });
      }
      actions = updated;
      const rows = Array.from(actions.values(), action => ({label: 'Painel: ' + action.label, keys: action.keys}));
      const current = JSON.stringify(rows);
      if (current !== signature) { signature = current; changed(rows); }
    }
    function cancel() { active = false; digits = ''; status.hidden = true; }
    doc.addEventListener('keydown', event => {
      if (event.defaultPrevented || event.repeat || event.isComposing) return;
      if (event.key === 'F2' && !event.ctrlKey && !event.altKey && !event.metaKey) {
        event.preventDefault(); event.stopImmediatePropagation(); refresh();
        active = !active; digits = ''; status.hidden = !active;
        status.textContent = 'Digite o numero mostrado no hint do botao e Enter. Esc cancela.';
        return;
      }
      if (!active) return;
      if (event.ctrlKey || event.altKey || event.metaKey) { cancel(); return; }
      event.preventDefault(); event.stopImmediatePropagation();
      if (event.key === 'Escape') { cancel(); return; }
      if (/^[0-9]$/.test(event.key) && digits.length < 8) digits += event.key;
      else if (event.key === 'Backspace') digits = digits.slice(0, -1);
      else if (event.key === 'Enter') {
        const action = actions.get(digits); cancel();
        if (action && available(action.node)) { action.node.focus(); action.node.click(); }
        return;
      }
      const action = actions.get(digits);
      status.textContent = 'Atalho ' + digits + (action ? ': ' + action.label : '') + ' — Enter executa; Esc cancela.';
    }, true);
    doc.addEventListener('pointerdown', cancel, true);
    refresh();
    const observer = new MutationObserver(refresh);
    observer.observe(doc.body, {subtree:true, childList:true, characterData:true, attributes:true,
      attributeFilter:['title','aria-label']});
    return {refresh};
  }
