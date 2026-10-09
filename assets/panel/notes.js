  // Notas (Zettelkasten). Tudo o que vem de uma nota -- e o corpo pode ser
  // texto copiado de qualquer pagina -- entra como texto: textContent,
  // value e createTextNode. Nunca como HTML.
  const notes = (() => {
    const ID = /^[0-9][0-9-]{0,63}$/;
    const WIKI = /\[\[([^\[\]|\n]+)(?:\|([^\[\]\n]*))?\]\]/g;
    const BODY_MAX_BYTES = 200 * 1024;
    const TITLE_MAX = 300;
    const TAGS_MAX = 20;
    const TAG_MAX = 60;
    const QUERY_MAX = 500;
    const SOURCE_MAX = 2048;
    const nq = byId('nq'), list = byId('notes-list'), msg = byId('notes-msg');
    const browse = byId('notes-browse'), edit = byId('notes-edit');
    const title = byId('note-title'), body = byId('note-body'), tags = byId('note-tags');
    const sourceRow = byId('note-source-row'), source = byId('note-source'), meta = byId('note-meta');
    const previewBox = byId('note-preview-box'), preview = byId('note-preview');
    const backBox = byId('note-backlinks-box'), backlinks = byId('note-backlinks');
    const confirmBox = byId('note-confirm');
    const template = byId('zk-template'), kindField = byId('note-kind'), typeFilter = byId('zk-filter');
    const linkTarget = byId('zk-link-target');
    let lastList = null;
    let timeline = true;
    const templates = {
      rapida: '## Ideia\n\n\n## Próximo passo\n\n',
      leitura: '## Referência\n\n\n## Ideias em minhas palavras\n\n\n## Conexões\n\n',
      permanente: '## Ideia central\n\n\n## Argumento\n\n\n## Fontes\n\n\n## Conexões\n\n',
      indice: '## Tema\n\n\n## Notas relacionadas\n\n\n## Questões abertas\n\n'
    };
    const noteType = (note) => (note.tags || []).find((tag) => /^zk:(rapida|leitura|permanente|indice)$/.test(tag))?.slice(3) || '';
    function linkOptions() {
      if (!linkTarget) return;
      linkTarget.textContent = '';
      const empty = make('option', '', 'Conectar com outra nota…'); empty.value = '';
      linkTarget.appendChild(empty);
      for (const note of lastList?.notes || []) {
        if (note.id === openId) continue;
        const option = make('option', '', note.title + ' · ' + note.id);
        option.value = note.id;
        linkTarget.appendChild(option);
      }
    }
    function changed() {
      dirty = true;
      say('Alterações por salvar.');
      postDraft();
    }
    // Nota no editor: o id dela, ou null para uma nota nova ainda por salvar.
    let openId = null;
    // A revisao da nota que o editor mostra (do disco, ou do nosso ultimo
    // salvar): o salvar leva-a, e o worker nao esmaga uma nota que mudou fora
    // daqui desde entao.
    let openRev = null;
    let openSource = '';
    let dirty = false;
    let savingNew = false;
    // Um note-save a caminho (o id da nota): se voltar "failed", o texto
    // volta a estar por salvar em vez de se perder ao sair do editor.
    let saving = undefined;
    // O lado nativo guarda uma copia do que esta por salvar (note-draft) e
    // grava-a quando o painel fecha por fora -- botao Notas, Ctrl+H, outro
    // painel, Home, fechar a janela --, porque ai esta pagina ja nao corre.
    let draftPosted = false;
    let draftTimer = 0;
    let searchTimer = 0;
    let previewTimer = 0;

    const say = (text) => { msg.textContent = text || ''; };
    const when = (unix) => {
      if (!unix) return '';
      try { return new Date(unix * 1000).toLocaleString('pt-BR', { dateStyle: 'short', timeStyle: 'short' }); } catch (e) { return ''; }
    };
    const query = () => nq.value.trim().slice(0, QUERY_MAX);
    const refresh = () => {
      const text = query();
      if (text) post('notes-search', { query: text }); else post('notes-list');
    };
    const open = (id) => { if (ID.test(id) && leave()) post('note-open', { id }); };
    // Texto que o JSON leva inteiro (sem metades de um par UTF-16, que o
    // parser do lado nativo recusava) e titulo e tags numa linha: o TAB de
    // uma tabela colada ou do titulo de uma nota do Obsidian vira espaco.
    const whole = (text) => (typeof text.toWellFormed === 'function' ? text.toWellFormed() : text);
    const line = (text) => whole(String(text)).replace(/[\u0000-\u001f\u007f-\u009f]+/g, ' ').trim();
    const edited = () => ({
      id: openId,
      rev: openId === null ? null : openRev,
      title: line(title.value),
      body: whole(body.value),
      tags: tags.value.split(',').map(line).filter(Boolean),
    });
    // Manda (ou limpa) a copia do lado nativo.
    function postDraft() {
      clearTimeout(draftTimer);
      draftTimer = 0;
      const note = edited();
      if (dirty && !edit.hidden && (note.title || note.body.trim())) {
        post('note-draft', note);
        draftPosted = true;
      } else if (draftPosted) {
        post('note-draft', {});
        draftPosted = false;
      }
    }
    const showList = () => { confirmBox.hidden = true; edit.hidden = true; browse.hidden = false; };
    const showEditor = () => { browse.hidden = true; edit.hidden = false; };

    function renderList(data) {
      // Resposta a uma busca que ja nao e a da caixa: a seguinte vem a caminho.
      if ((data.query || '') !== query()) return;
      lastList = data;
      linkOptions();
      list.textContent = '';
      list.className = timeline ? 'zk-timeline' : '';
      const visible = data.notes.filter((note) => !typeFilter?.value || noteType(note) === typeFilter.value);
      if (!visible.length) {
        list.appendChild(make('div', 'empty', data.query
          ? 'Nenhuma nota encontrada.'
          : 'Nenhuma nota ainda. Crie uma em Nova nota, ou selecione um texto numa página e tecle Ctrl+Shift+Z.'));
        return;
      }
      if (data.total > data.notes.length) {
        list.appendChild(make('div', 'empty', 'Mostrando ' + data.notes.length + ' de ' + data.total + ' notas. Refine a busca.'));
      }
      for (const note of [...visible].sort((a, b) => b.updated - a.updated)) {
        const button = make('button', 'item');
        const detail = ['Atualizada ' + when(note.updated), 'ID ' + note.id, note.tags.map((tag) => '#' + tag).join(' ')].filter(Boolean).join(' · ');
        button.append(make('span', 'title', note.title), make('span', 'detail', detail));
        button.addEventListener('click', () => open(note.id));
        list.appendChild(button);
      }
    }

    // O corpo com os [[id]] e [[id|nome]] clicaveis; o resto e texto.
    function renderPreview() {
      preview.textContent = '';
      const text = body.value;
      let last = 0;
      let match;
      WIKI.lastIndex = 0;
      while ((match = WIKI.exec(text)) !== null) {
        const id = match[1].trim();
        if (!ID.test(id)) continue;
        if (match.index > last) preview.appendChild(document.createTextNode(text.slice(last, match.index)));
        const link = make('button', 'link', (match[2] || '').trim() || id);
        link.title = 'Abrir a nota ' + id;
        link.addEventListener('click', () => open(id));
        preview.appendChild(link);
        last = match.index + match[0].length;
      }
      if (last < text.length) preview.appendChild(document.createTextNode(text.slice(last)));
      previewBox.hidden = !text.trim();
    }

    // Fonte, datas e backlinks: o que o editor mostra da nota sem ser editavel.
    function describe(note, links) {
      openSource = note && note.source ? note.source : '';
      source.textContent = openSource;
      source.disabled = !/^https?:\/\//i.test(openSource) || openSource.length > SOURCE_MAX;
      sourceRow.hidden = !openSource;
      meta.textContent = note
        ? ['Criada ' + when(note.created), 'atualizada ' + when(note.updated), 'id ' + note.id].join(' · ')
        : 'Nota nova: ainda não foi salva.';
      backlinks.textContent = '';
      for (const other of links || []) {
        const button = make('button', 'item', other.title);
        button.addEventListener('click', () => open(other.id));
        backlinks.appendChild(button);
      }
      backBox.hidden = !(links && links.length);
    }

    function fill(note, links) {
      openId = note ? note.id : null;
      openRev = note && note.rev ? note.rev : null;
      // Outra nota no editor: a resposta ao salvar de uma nota nova que
      // ainda venha a caminho ja nao e desta.
      savingNew = false;
      title.value = note ? note.title : '';
      body.value = note ? note.body : '';
      tags.value = note ? note.tags.join(', ') : '';
      if (kindField) kindField.value = note ? noteType(note) : '';
      linkOptions();
      describe(note, links);
      confirmBox.hidden = true;
      dirty = false;
      saving = undefined;
      postDraft();
      renderPreview();
    }

    function save() {
      const note = edited();
      if (note.tags.length > TAGS_MAX || note.tags.some((tag) => tag.length > TAG_MAX)) {
        say('Até ' + TAGS_MAX + ' tags, cada uma com até ' + TAG_MAX + ' caracteres.');
        return false;
      }
      if (note.title.length > TITLE_MAX) { say('O título passa de ' + TITLE_MAX + ' caracteres.'); return false; }
      if (new TextEncoder().encode(note.body).length > BODY_MAX_BYTES) {
        say('A nota passa de 200 KiB. Divida-a em duas.');
        return false;
      }
      // Uma nota nova ainda sem id: um segundo pedido criava outra nota. O
      // que se escrever entretanto fica por salvar ate o id chegar.
      if (openId === null && savingNew) { say('A salvar…'); return false; }
      clearTimeout(draftTimer);
      draftTimer = 0;
      post('note-save', note);
      savingNew = openId === null;
      saving = openId;
      dirty = false;
      // O note-save leva o texto todo: o lado nativo larga a copia.
      draftPosted = false;
      say('A salvar…');
      return true;
    }

    // Sair do editor nao deita fora o que se escreveu. `false`: nao da para
    // salvar agora (acima dos tectos, ou a nota nova ainda sem id) -- quem
    // ia sair fica, com o aviso a vista.
    function leave() {
      if (!dirty || edit.hidden) return true;
      const note = edited();
      if (!note.title && !note.body.trim()) return true;
      return save();
    }

    function newNote() {
      if (!leave()) return;
      fill(null, []);
      const kind = template?.value || '';
      if (templates[kind]) {
        body.value = templates[kind];
        tags.value = 'zk:' + kind;
        if (kindField) kindField.value = kind;
        dirty = true;
        renderPreview();
      }
      showEditor();
      if (dirty) postDraft();
      say('');
      title.focus();
    }

    function receive(data) {
      switch (data.kind) {
        case 'listed':
          renderList(data);
          break;
        case 'opened': {
          const note = data.note;
          if (data.cause === 'saved') {
            // So mexe no editor se ele ainda mostra esta nota (ou a nova que
            // acabou de ganhar id); senao o utilizador ja seguiu em frente.
            // O texto nao e reescrito: o cursor ficava no fim a cada Ctrl+S.
            const same = openId === note.id || (openId === null && savingNew);
            savingNew = false;
            if (same) {
              saving = undefined;
              openId = note.id;
              openRev = note.rev || null;
              if (!title.value.trim()) title.value = note.title;
              describe(note, data.backlinks);
              // O que se escreveu enquanto a nota nova esperava pelo id: a
              // copia do lado nativo passa a ser a desta nota.
              if (dirty) postDraft();
            }
            say('Nota salva.');
            refresh();
          } else if (openId !== note.id && !leave()) {
            // Uma nota que chega de fora (Ctrl+Shift+Z) com o editor por
            // salvar e que nao da para salvar agora: o editor fica como esta
            // e a nota nova aparece na lista.
            say(data.cause === 'created' ? 'Nota criada a partir da seleção; está na lista.' : '');
            if (data.cause === 'created') refresh();
          } else {
            // Uma nota que chega de fora (Ctrl+Shift+Z) nao deita fora o que
            // estava por salvar no editor: o `leave` acima ja o salvou.
            fill(note, data.backlinks);
            showEditor();
            say(data.cause === 'created' ? 'Nota criada a partir da seleção.' : '');
            if (data.cause === 'created') refresh();
          }
          break;
        }
        case 'deleted':
          if (openId === data.id) { fill(null, []); showList(); }
          say('Nota movida para a lixeira (.trash).');
          refresh();
          break;
        case 'missing':
          say('Essa nota já não existe.');
          refresh();
          break;
        case 'conflict': {
          // A nota mudou fora deste editor (outra janela, o Obsidian): o
          // texto dele ficou numa copia, e o editor passa a mostra-la.
          const note = data.note;
          if (openId === data.original) {
            saving = undefined;
            openId = note.id;
            openRev = note.rev || null;
            title.value = note.title;
            describe(note, []);
          }
          say('Esta nota mudou fora deste editor (outra janela ou o Obsidian). O seu texto ficou numa cópia: ' + note.title);
          refresh();
          break;
        }
        case 'failed':
          savingNew = false;
          // Um salvar que falhou (pasta ocupada, disco cheio, ficheiro preso
          // por outro programa, pedido recusado): o texto continua por salvar
          // -- sair do editor tenta outra vez -- e o lado nativo volta a ter
          // a copia.
          if (saving !== undefined && saving === openId && !edit.hidden) {
            dirty = true;
            postDraft();
          }
          saving = undefined;
          say(data.message);
          break;
      }
    }

    if (kindField) kindField.addEventListener('change', () => {
      const current = tags.value.split(',').map(line).filter((tag) => tag && !/^zk:(rapida|leitura|permanente|indice)$/.test(tag));
      if (templates[kindField.value]) current.push('zk:' + kindField.value);
      tags.value = current.join(', ');
      changed();
    });
    if (typeFilter) typeFilter.addEventListener('change', () => { if (lastList) renderList(lastList); });
    const display = (on) => {
      timeline = on;
      byId('zk-timeline').setAttribute('aria-pressed', on ? 'true' : 'false');
      byId('zk-list').setAttribute('aria-pressed', on ? 'false' : 'true');
      byId('zk-timeline').className = on ? 'btn primary' : 'btn';
      byId('zk-list').className = on ? 'btn' : 'btn primary';
      if (lastList) renderList(lastList);
    };
    byId('zk-timeline').addEventListener('click', () => display(true));
    byId('zk-list').addEventListener('click', () => display(false));
    byId('zk-graph').addEventListener('click', () => window.neuraliaShowSection('obsidian'));
    byId('zk-link-insert').addEventListener('click', () => {
      const note = lastList?.notes.find((note) => note.id === linkTarget.value);
      if (!note || !ID.test(note.id) || note.id === openId) return;
      const label = note.title.replace(/[\[\]|\r\n]/g, ' ').trim();
      const link = '[[' + note.id + (label ? '|' + label : '') + ']]';
      const start = Number.isInteger(body.selectionStart) ? body.selectionStart : body.value.length;
      const end = Number.isInteger(body.selectionEnd) ? body.selectionEnd : start;
      body.value = body.value.slice(0, start) + link + body.value.slice(end);
      body.focus();
      if (typeof body.setSelectionRange === 'function') body.setSelectionRange(start + link.length, start + link.length);
      changed();
      renderPreview();
    });

    nq.addEventListener('input', () => {
      clearTimeout(searchTimer);
      searchTimer = setTimeout(refresh, 250);
    });
    nq.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); clearTimeout(searchTimer); refresh(); }
    });
    byId('note-new').addEventListener('click', newNote);
    byId('note-back').addEventListener('click', () => { if (leave()) { showList(); refresh(); } });
    byId('note-save').addEventListener('click', save);
    byId('note-delete').addEventListener('click', () => { confirmBox.hidden = false; });
    byId('note-confirm-no').addEventListener('click', () => { confirmBox.hidden = true; });
    byId('note-confirm-yes').addEventListener('click', () => {
      confirmBox.hidden = true;
      if (openId === null) { fill(null, []); showList(); return; }
      post('note-delete', { id: openId });
    });
    // A fonte abre fora do painel, que fecha: salva antes, como o X.
    source.addEventListener('click', () => {
      if (!source.disabled && openSource && leave()) post('open', { input: openSource });
    });
    for (const field of [title, body, tags]) {
      field.addEventListener('input', () => {
        dirty = true;
        say('Alterações por salvar.');
        // A copia do lado nativo nunca fica mais de 200 ms atras, mesmo a
        // escrever sem parar: um temporizador que ja corre nao recomeca
        // (recomecar a cada tecla deixava uma rajada inteira sem copia).
        if (!draftTimer) draftTimer = setTimeout(postDraft, 200);
      });
    }
    body.addEventListener('input', () => {
      clearTimeout(previewTimer);
      previewTimer = setTimeout(renderPreview, 200);
    });
    edit.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && (e.key || '').toLowerCase() === 's') {
        e.preventDefault();
        save();
      }
    });
    return { refresh, receive, newNote, leave, focus: () => (edit.hidden ? nq : title).focus() };
  })();

