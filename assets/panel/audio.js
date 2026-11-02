  const audio = (() => {
    const list = byId('audio-list'), status = byId('audio-status'), enabled = byId('audio-enabled');
    function render(data) {
      list.replaceChildren();
      enabled.checked = !!data.enabled;
      status.textContent = data.status || (data.items.length ? '' : 'Nenhum áudio detectado. Ative a leitura em voz alta na página da IA.');
      for (const item of data.items) {
        const card = make('section'), title = make('h3'), detail = make('p'), save = make('button');
        title.textContent = item.title;
        detail.textContent = item.detail;
        save.textContent = item.busy ? 'Exportando…' : 'Exportar MP3';
        save.disabled = !!item.busy;
        save.addEventListener('click', () => post('audio-export', {id:item.id}));
        card.appendChild(title); card.appendChild(detail); card.appendChild(save); list.appendChild(card);
      }
    }
    enabled.addEventListener('change', () => post('audio-enabled', {on:enabled.checked}));
    byId('audio-refresh').addEventListener('click', () => post('audio-list'));
    byId('audio-clear').addEventListener('click', () => post('audio-clear'));
    return {render, refresh() { post('audio-list'); }};
  })();
