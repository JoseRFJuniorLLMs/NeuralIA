  // Abas: Historico, Notas, Favoritos e Downloads.
  const tabs = {
    history: byId('tab-history'), notes: byId('tab-notes'),
    bookmarks: byId('tab-bookmarks'), downloads: byId('tab-downloads')
  };
  const views = {
    history: byId('view-history'), notes: byId('view-notes'),
    bookmarks: byId('view-bookmarks'), downloads: byId('view-downloads')
  };
  function showSection(name) {
    const key = name === 'notes' || name === 'notas' ? 'notes'
      : name === 'history' || name === 'historico' ? 'history'
      : name === 'bookmarks' || name === 'favoritos' ? 'bookmarks'
      : name === 'downloads' ? 'downloads' : '';
    if (!key) return false;
    // Sair das Notas salva o editor; se nao der agora, fica-se nas Notas.
    if (key !== 'notes' && !views.notes.hidden && !notes.leave()) return false;
    for (const other of Object.keys(views)) {
      views[other].hidden = other !== key;
      tabs[other].setAttribute('aria-selected', other === key ? 'true' : 'false');
    }
    if (key === 'notes') { notes.refresh(); notes.focus(); }
    else if (key === 'bookmarks') { bookmarks.refresh(); bookmarks.focus(); }
    else if (key === 'downloads') { downloads.refresh(); }
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
  window.__neuraliaBookmarks = { receive: bookmarks.receive };
  window.__neuraliaDownloads = {
    render: downloads.render,
    // A seta da barra e o Ctrl+J com o painel aberto: nos Downloads fecha
    // (como o X), noutra seccao mostra os Downloads.
    button() { if (views.downloads.hidden) showSection('downloads'); else close(); }
  };
  tabs.history.addEventListener('click', () => showSection('history'));
  tabs.notes.addEventListener('click', () => showSection('notes'));
  tabs.bookmarks.addEventListener('click', () => showSection('bookmarks'));
  tabs.downloads.addEventListener('click', () => showSection('downloads'));
  byId('close').addEventListener('click', close);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); close(); }
  });

  theme(__THEME__);
  q.focus();
  post('ready');
})();
