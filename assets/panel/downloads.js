  // Downloads (downloads-ui). A lista chega do lado nativo como dados e
  // entra so como texto (textContent). Cada pedido leva so o numero da
  // linha, ou o estado do interruptor: nunca um caminho nem um endereco --
  // quem sabe o arquivo de cada linha e o lado nativo.
  const downloads = (() => {
    const list = byId('dl-list'), empty = byId('dl-empty'), allow = byId('dl-allow');
    const ACTIONS = [
      ['open', 'Abrir', 'downloads-open'],
      ['show', 'Mostrar na pasta', 'downloads-show'],
      ['cancel', 'Cancelar', 'downloads-cancel']
    ];
    // As linhas pelo numero: uma atualizacao muda o texto no sitio, e um
    // botao nao e trocado a meio de um clique.
    const rows = new Map();
    let order = '';

    function row(data) {
      let node = rows.get(data.id);
      if (!node) {
        const id = data.id;
        node = {
          box: make('div', 'dl'),
          name: make('span', 'title'),
          status: make('span', 'detail'),
          bar: make('div', 'dl-bar'),
          fill: make('div', 'dl-fill'),
          actions: make('div', 'dl-actions'),
          buttons: {}
        };
        node.bar.appendChild(node.fill);
        for (const [key, label, action] of ACTIONS) {
          const button = make('button', 'btn', label);
          button.addEventListener('click', () => post(action, { id }));
          node.buttons[key] = button;
          node.actions.appendChild(button);
        }
        node.box.append(node.name, node.status, node.bar, node.actions);
        rows.set(id, node);
      }
      node.box.setAttribute('data-tone', String(data.tone || ''));
      node.name.textContent = String(data.name || '');
      node.status.textContent = String(data.status || '');
      const percent = typeof data.percent === 'number' ? Math.max(0, Math.min(100, data.percent)) : null;
      node.bar.hidden = data.tone !== 'running';
      node.fill.style.width = percent === null ? '100%' : percent + '%';
      node.bar.className = percent === null ? 'dl-bar busy' : 'dl-bar';
      let any = false;
      for (const [key] of ACTIONS) {
        node.buttons[key].hidden = !data[key];
        any = any || !!data[key];
      }
      node.actions.hidden = !any;
      return node;
    }

    function render(data) {
      allow.checked = !!data.allowPrograms;
      const seen = new Set();
      const ids = [];
      for (const item of data.rows || []) {
        if (typeof item.id !== 'number' || seen.has(item.id)) continue;
        row(item);
        seen.add(item.id);
        ids.push(item.id);
      }
      for (const [id, node] of rows) {
        if (!seen.has(id)) {
          if (node.box.parentNode) node.box.parentNode.removeChild(node.box);
          rows.delete(id);
        }
      }
      // So se reordena quando a ordem muda: mover uma linha a meio de um
      // clique perdia-o.
      const next = ids.join(',');
      if (next !== order) {
        for (const id of ids) list.appendChild(rows.get(id).box);
        order = next;
      }
      empty.hidden = ids.length > 0;
    }

    allow.addEventListener('change', () => post('downloads-allow-programs', { on: !!allow.checked }));

    return {
      render,
      // A seccao abriu: a lista, ja.
      refresh() { post('downloads-list'); }
    };
  })();

