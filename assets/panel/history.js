  window.__neuraliaPanel = {
    theme,
    render(data) {
      const section = byId(data.id);
      if (!section) return;
      section.hidden = false;
      section.querySelector('h2').textContent = data.title;
      const box = section.querySelector('div');
      box.textContent = '';
      if (!data.items.length) {
        box.appendChild(make('div', 'empty', data.empty));
        return;
      }
      for (const item of data.items) {
        const button = make('button', 'item');
        button.append(make('span', 'title', item.title), make('span', 'detail', item.detail));
        button.addEventListener('click', () => post('open', { input: item.input }));
        box.appendChild(button);
      }
    }
  };

