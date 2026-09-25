  // Abas: Historico e Notas.
  const tabs = { history: byId('tab-history'), notes: byId('tab-notes') };
  const views = { history: byId('view-history'), notes: byId('view-notes') };
  function showSection(name) {
    const key = name === 'notes' || name === 'notas' ? 'notes'
      : name === 'history' || name === 'historico' ? 'history' : '';
    if (!key) return false;
    // Sair das Notas salva o editor; se nao der agora, fica-se nas Notas.
    if (key === 'history' && !views.notes.hidden && !notes.leave()) return false;
    for (const other of Object.keys(views)) {
      views[other].hidden = other !== key;
      tabs[other].setAttribute('aria-selected', other === key ? 'true' : 'false');
    }
    if (key === 'notes') { notes.refresh(); notes.focus(); } else { q.focus(); }
    return true;
  }
  const close = () => { if (notes.leave()) post('close'); };
  window.neuraliaShowSection = showSection;
  window.__neuraliaNotes = {
    receive: notes.receive,
    newNote() { if (showSection('notes')) notes.newNote(); },
    // O botao Notas da barra/Home com o painel aberto: nas Notas fecha
    // (como o X, salvando antes), no Historico mostra as Notas.
    button() { if (views.notes.hidden) showSection('notes'); else close(); }
  };
  tabs.history.addEventListener('click', () => showSection('history'));
  tabs.notes.addEventListener('click', () => showSection('notes'));
  byId('close').addEventListener('click', close);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); close(); }
  });

  theme(__THEME__);
  q.focus();
  post('ready');
})();
