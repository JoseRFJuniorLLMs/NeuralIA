  // Abas: Historico, Notas, Obsidian, Favoritos, Downloads e Sobre.
  const tabs = {
    history: byId('tab-history'), notes: byId('tab-notes'),
    obsidian: byId('tab-obsidian'),
    bookmarks: byId('tab-bookmarks'), downloads: byId('tab-downloads'),
    about: byId('tab-about'), shortcuts: byId('tab-shortcuts')
  };
  const views = {
    history: byId('view-history'), notes: byId('view-notes'),
    obsidian: byId('view-obsidian'),
    bookmarks: byId('view-bookmarks'), downloads: byId('view-downloads'),
    about: byId('view-about'), shortcuts: byId('view-shortcuts')
  };
  function showSection(name) {
    const key = name === 'notes' || name === 'notas' ? 'notes'
      : name === 'obsidian' || name === 'grafo' ? 'obsidian'
      : name === 'history' || name === 'historico' ? 'history'
      : name === 'bookmarks' || name === 'favoritos' ? 'bookmarks'
      : name === 'downloads' ? 'downloads'
      : name === 'shortcuts' ? 'shortcuts'
      : name === 'about' || name === 'sobre' ? 'about' : '';
    if (!key) return false;
    // Sair das Notas salva o editor; se nao der agora, fica-se nas Notas.
    if (key !== 'notes' && !views.notes.hidden && !notes.leave()) return false;
    for (const other of Object.keys(views)) {
      views[other].hidden = other !== key;
      tabs[other].setAttribute('aria-selected', other === key ? 'true' : 'false');
    }
    if (key === 'notes') { notes.refresh(); notes.focus(); }
    else if (key === 'obsidian') { obsidian.refresh(); }
    else if (key === 'bookmarks') { bookmarks.refresh(); bookmarks.focus(); }
    else if (key === 'downloads') { downloads.refresh(); }
    else if (key === 'about') { /* Sobre não precisa focar */ }
    else { q.focus(); }
    return true;
  }
  const close = () => { if (notes.leave()) post('close'); };
  window.neuraliaShowSection = showSection;
  window.__neuraliaNotes = {
    receive: notes.receive,
    newNote() { if (showSection('notes')) notes.newNote(); },
    // O botao Notas da barra/Home com o painel aberto: nas Notas fecha
    // (como o X, salvando antes), noutra seccao mostra as Notas.
    button() { if (views.notes.hidden) showSection('notes'); else close(); }
  };
  window.__neuraliaObsidian = {
    render: obsidian.render,
    refresh: obsidian.refresh,
    open: obsidian.open,
    button() { if (views.obsidian.hidden) showSection('obsidian'); else close(); }
  };
  window.__neuraliaBookmarks = { receive: bookmarks.receive };
  window.__neuraliaDownloads = {
    render: downloads.render,
    // A seta da barra e o Ctrl+J com o painel aberto: nos Downloads fecha
    // (como o X), noutra seccao mostra os Downloads.
    button() { if (views.downloads.hidden) showSection('downloads'); else close(); }
  };
  window.__neuraliaAbout = {
    show() { showSection('about'); },
    button() { if (views.about.hidden) showSection('about'); else close(); }
  };
  tabs.history.addEventListener('click', () => showSection('history'));
  tabs.notes.addEventListener('click', () => showSection('notes'));
  tabs.obsidian.addEventListener('click', () => showSection('obsidian'));
  tabs.bookmarks.addEventListener('click', () => showSection('bookmarks'));
  tabs.downloads.addEventListener('click', () => showSection('downloads'));
  tabs.shortcuts.addEventListener('click', () => showSection('shortcuts'));
  tabs.about.addEventListener('click', () => showSection('about'));
  byId('close').addEventListener('click', close);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); close(); }
  });

  let nativeShortcutRows = [], panelShortcutRows = [];
  function renderShortcutMap() {
    const rows = nativeShortcutRows.concat(panelShortcutRows);
    const body = byId('shortcut-rows'); body.replaceChildren();
    for (const row of rows) {
      const tr = document.createElement('tr');
      for (const value of [row.label, row.keys]) { const td = document.createElement('td'); td.textContent = value; tr.appendChild(td); }
      body.appendChild(tr);
    }
  }
  window.neuraliaSetShortcuts = rows => { nativeShortcutRows = rows; renderShortcutMap(); };
  installPanelKeyHints(document, rows => { panelShortcutRows = rows; renderShortcutMap(); });
  theme(__THEME__);
  q.focus();
  post('ready');
})();
