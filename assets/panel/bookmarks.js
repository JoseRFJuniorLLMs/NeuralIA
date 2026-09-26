  // Favoritos. Os titulos e os enderecos vem do lado nativo e entram como
  // texto (textContent), nunca como HTML. Os pedidos levam so o id de um
  // favorito -- nunca um endereco nem um caminho: abrir e remover acham o
  // endereco do lado nativo, e os dialogos de importar e exportar sao
  // nativos.
  const bookmarks = (() => {
    const bq = byId('bq'), list = byId('bookmarks-list'), msg = byId('bookmarks-msg');
    const QUERY_MAX = 500;
    let items = [];
    const say = (text) => { msg.textContent = text || ''; };
    const validId = (id) => Number.isSafeInteger(id) && id > 1;
    const shape = (item) => item && typeof item === 'object' && validId(item.id)
      && (item.kind === 'folder' || item.kind === 'link')
      && typeof item.title === 'string' && typeof item.detail === 'string'
      && Number.isSafeInteger(item.depth) && item.depth >= 1;
    const render = () => {
      const query = bq.value.trim().slice(0, QUERY_MAX).toLowerCase();
      list.textContent = '';
      let shown = 0;
      for (const item of items) {
        if (query && (item.kind === 'folder'
          || !(item.title.toLowerCase().includes(query) || item.detail.toLowerCase().includes(query)))) continue;
        const row = make('div', 'bm-row');
        row.style.setProperty('--depth', String(query ? 0 : item.depth - 1));
        if (item.kind === 'folder') {
          row.appendChild(make('div', 'bm-folder', item.title));
        } else {
          const open = make('button', 'item');
          open.append(make('span', 'title', item.title), make('span', 'detail', item.detail));
          open.addEventListener('click', () => post('bookmark-open', { id: item.id }));
          const remove = make('button', 'bm-remove', '✕');
          remove.title = 'Remover dos favoritos';
          remove.addEventListener('click', () => post('bookmark-remove', { id: item.id }));
          row.append(open, remove);
        }
        list.appendChild(row);
        shown++;
      }
      if (!shown) {
        list.appendChild(make('div', 'empty', query
          ? 'Nenhum favorito com esse texto.'
          : 'Nenhum favorito ainda. Tecle Ctrl+D numa página para guardá-la aqui.'));
      }
    };
    bq.addEventListener('input', render);
    byId('bookmarks-import-chrome').addEventListener('click', () => post('bookmarks-import-chrome'));
    byId('bookmarks-import-file').addEventListener('click', () => post('bookmarks-import-file'));
    byId('bookmarks-export').addEventListener('click', () => post('bookmarks-export'));
    const receive = (data) => {
      if (!data || typeof data !== 'object') return;
      if (Array.isArray(data.items)) {
        items = data.items.filter(shape);
        render();
      }
      if (typeof data.notice === 'string') say(data.notice);
    };
    return { refresh: () => post('bookmarks-list'), receive, focus: () => bq.focus() };
  })();
