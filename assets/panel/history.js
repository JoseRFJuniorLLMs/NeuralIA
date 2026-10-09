  window.__neuraliaPanel = {
    theme,
    render(data) {
      const section = byId(data.id);
      if (!section) return;
      section.hidden = false;
      section.querySelector('h2').textContent = data.title;
      const box = section.querySelector('div');
      box.textContent = '';
      const timeline = data.id === 'recentes';
      box.className = timeline ? 'zk-timeline' : '';
      if (!data.items.length) {
        box.appendChild(make('div', 'empty', data.empty));
        return;
      }
      const items = timeline ? [...data.items].sort((a, b) => (b.timestamp || 0) - (a.timestamp || 0)) : data.items;
      for (const item of items) {
        const button = make('button', 'item');
        let timestamp = '';
        if (item.timestamp) {
          try { timestamp = new Date(item.timestamp * 1000).toLocaleString('pt-BR') + ' · '; } catch (_) {}
        }
        button.append(make('span', 'title', item.title), make('span', 'detail', timestamp + item.detail));
        button.addEventListener('click', () => post('open', { input: item.input }));
        box.appendChild(button);
      }
    }
  };

