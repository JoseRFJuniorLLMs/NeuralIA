'use strict';
// NeuralIA — biblioteca de livros (estilo Calibre): capas, busca, ordem,
// "Continuar lendo", adicionar e remover. Ficheiro do próprio NeuralIA.
(function (root) {
  const E = root.NeuraliaEpub;
  const collator = new Intl.Collator('pt-BR', { sensitivity: 'base', numeric: true });

  // ---------------------------------------------------------------- lógica

  // Busca sem distinguir maiúsculas nem acentos, em título e autores; cada
  // palavra da busca tem de aparecer.
  function filterBooks(books, query) {
    const terms = E.fold(query || '').split(/\s+/).filter(Boolean);
    if (!terms.length) return books.slice();
    return books.filter((book) => {
      const hay = E.fold([book.title || ''].concat(book.authors || []).join(' '));
      return terms.every((term) => hay.includes(term));
    });
  }

  function recency(book) {
    return Math.max(Number(book.lastOpened) || 0, Number(book.added) || 0);
  }

  function firstAuthor(book) {
    return (book.authors && book.authors[0]) || '';
  }

  // Recentes: o último aberto ou adicionado primeiro. Título e Autor em
  // ordem alfabética do português (sem acentos a contar).
  function sortBooks(books, mode) {
    const list = books.slice();
    const byTitle = (a, b) => collator.compare(a.title || '', b.title || '');
    if (mode === 'title') {
      list.sort((a, b) => byTitle(a, b) || collator.compare(firstAuthor(a), firstAuthor(b)));
    } else if (mode === 'author') {
      list.sort((a, b) => {
        const left = firstAuthor(a);
        const right = firstAuthor(b);
        if (!left !== !right) return left ? -1 : 1;
        return collator.compare(left, right) || byTitle(a, b);
      });
    } else {
      list.sort((a, b) => recency(b) - recency(a) || byTitle(a, b));
    }
    return list;
  }

  // O livro aberto mais recentemente, se algum já foi aberto.
  function continueReading(books) {
    let best = null;
    for (const book of books) {
      if (book.lastOpened && (!best || book.lastOpened > best.lastOpened)) best = book;
    }
    return best;
  }

  // Cor estável da capa gerada: a mesma para o mesmo título.
  function coverHue(title) {
    let hash = 0;
    for (const ch of String(title || '')) hash = (hash * 31 + ch.codePointAt(0)) >>> 0;
    return hash % 360;
  }

  function authorLine(book) {
    return (book.authors && book.authors.length) ? book.authors.join(', ') : 'Autor desconhecido';
  }

  function readerHref(id) {
    return E.isBookId(id) ? '/reader.html?book=' + id : null;
  }

  // ----------------------------------------------------------------- página

  class LibraryPage {
    constructor(doc, win) {
      this.doc = doc;
      this.win = win;
      this.books = [];
      this.pendingRemoval = null;
      this.highlight = new Set();
      const byId = (id) => doc.getElementById(id);
      this.ui = {
        home: byId('home'),
        search: byId('search'),
        sort: byId('sort'),
        add: byId('add'),
        banner: byId('banner'),
        bannerText: byId('banner-text'),
        bannerClose: byId('banner-close'),
        continueSection: byId('continue'),
        continueCard: byId('continue-card'),
        grid: byId('grid'),
        empty: byId('empty'),
        noMatch: byId('no-match'),
        shelfTitle: byId('shelf-title'),
        confirm: byId('confirm'),
        confirmText: byId('confirm-text'),
        confirmOk: byId('confirm-ok'),
        confirmCancel: byId('confirm-cancel'),
      };
    }

    start() {
      const ui = this.ui;
      try {
        const saved = this.win.localStorage.getItem('neuralia-epub-sort');
        if (saved === 'recent' || saved === 'title' || saved === 'author') ui.sort.value = saved;
      } catch (_) { /* sem armazenamento: fica a ordem padrão */ }
      ui.home.addEventListener('click', () => E.post({ t: 'close' }));
      ui.add.addEventListener('click', () => E.post({ t: 'addBooks' }));
      ui.search.addEventListener('input', () => this.render());
      ui.sort.addEventListener('change', () => {
        try {
          this.win.localStorage.setItem('neuralia-epub-sort', ui.sort.value);
        } catch (_) { /* conveniência apenas */ }
        this.render();
      });
      ui.bannerClose.addEventListener('click', () => this.hideBanner());
      ui.confirmCancel.addEventListener('click', () => this.closeConfirm());
      ui.confirmOk.addEventListener('click', () => this.confirmRemoval());
      this.doc.addEventListener('keydown', (event) => this.onKey(event));
      E.onNotice((notice) => this.onNotice(notice));
      this.refresh();
    }

    async refresh() {
      try {
        const data = await E.getJson('/api/library');
        this.books = Array.isArray(data.books) ? data.books.filter((book) => E.isBookId(book.id)) : [];
        if (data.notice) this.showBanner(data.notice, false);
      } catch (error) {
        this.books = [];
        this.showBanner(error.message, true);
      }
      this.render();
    }

    onNotice(notice) {
      if (notice.kind === 'added') {
        const errors = Array.isArray(notice.errors) ? notice.errors : [];
        this.highlight = new Set(Array.isArray(notice.ids) ? notice.ids : []);
        if (errors.length) {
          const lines = errors.map((error) => '“' + error.file + '”: ' + error.message);
          this.showBanner('Não foi possível adicionar ' + lines.join(' · '), true);
        } else if (this.highlight.size) {
          const count = this.highlight.size;
          this.showBanner(count === 1 ? 'Livro adicionado.' : count + ' livros adicionados.', false);
        }
        this.refresh();
      } else if (notice.kind === 'removed') {
        this.showBanner('“' + (notice.title || 'Livro') + '” foi para a lixeira da biblioteca.', false);
        this.refresh();
      } else if (notice.kind === 'error') {
        this.showBanner(notice.message || 'Algo deu errado.', true);
        this.refresh();
      }
    }

    onKey(event) {
      if (!this.ui.confirm.hidden) {
        if (event.key === 'Escape') {
          event.preventDefault();
          this.closeConfirm();
        }
        return;
      }
      if (event.ctrlKey && !event.altKey && (event.key === 'o' || event.key === 'O')) {
        event.preventDefault();
        E.post({ t: 'addBooks' });
        return;
      }
      if (event.ctrlKey && !event.altKey && (event.key === 'f' || event.key === 'F')) {
        event.preventDefault();
        this.ui.search.focus();
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        if (this.doc.activeElement === this.ui.search && this.ui.search.value) {
          this.ui.search.value = '';
          this.render();
          return;
        }
        E.post({ t: 'close' });
      }
    }

    showBanner(text, isError) {
      this.ui.bannerText.textContent = text;
      this.ui.banner.classList.toggle('error', Boolean(isError));
      this.ui.banner.hidden = false;
    }

    hideBanner() {
      this.ui.banner.hidden = true;
    }

    open(book) {
      const href = readerHref(book.id);
      if (href) this.win.location.href = href;
    }

    askRemoval(book) {
      this.pendingRemoval = book;
      this.ui.confirmText.textContent = 'Remover “' + (book.title || 'este livro') + '” da biblioteca?';
      this.ui.confirm.hidden = false;
      this.ui.confirmCancel.focus();
    }

    closeConfirm() {
      this.pendingRemoval = null;
      this.ui.confirm.hidden = true;
    }

    confirmRemoval() {
      const book = this.pendingRemoval;
      this.closeConfirm();
      if (book && E.isBookId(book.id)) E.post({ t: 'removeBook', id: book.id });
    }

    cover(book, large) {
      const doc = this.doc;
      const frame = E.el(doc, 'div', large ? 'cover large' : 'cover');
      const fallback = () => {
        frame.textContent = '';
        const art = E.el(doc, 'div', 'cover-fallback');
        const hue = coverHue(book.title);
        art.style.background = 'linear-gradient(160deg, hsl(' + hue + ', 45%, 38%), hsl(' + ((hue + 40) % 360) + ', 50%, 22%))';
        art.appendChild(E.el(doc, 'span', 'fallback-title', book.title || 'Sem título'));
        art.appendChild(E.el(doc, 'span', 'fallback-author', authorLine(book)));
        frame.appendChild(art);
      };
      if (book.cover) {
        const img = doc.createElement('img');
        img.alt = '';
        img.loading = 'lazy';
        img.decoding = 'async';
        img.addEventListener('error', fallback, { once: true });
        img.src = book.cover;
        frame.appendChild(img);
      } else {
        fallback();
      }
      return frame;
    }

    progress(book) {
      const doc = this.doc;
      const wrap = E.el(doc, 'div', 'progress-line');
      const track = E.el(doc, 'div', 'progress-track');
      const fill = E.el(doc, 'div', 'progress-fill');
      const value = typeof book.progress === 'number' ? book.progress : 0;
      fill.style.width = (E.clamp(value, 0, 1) * 100).toFixed(1) + '%';
      track.appendChild(fill);
      wrap.appendChild(track);
      wrap.appendChild(E.el(doc, 'span', 'progress-label',
        typeof book.progress === 'number' ? E.percentLabel(value) : 'Não iniciado'));
      return wrap;
    }

    card(book) {
      const doc = this.doc;
      const card = E.el(doc, 'article', 'card');
      card.setAttribute('role', 'listitem');
      card.tabIndex = 0;
      if (this.highlight.has(book.id)) card.classList.add('fresh');
      card.appendChild(this.cover(book, false));
      const meta = E.el(doc, 'div', 'card-meta');
      meta.appendChild(E.el(doc, 'div', 'card-title', book.title || 'Sem título'));
      meta.appendChild(E.el(doc, 'div', 'card-author', authorLine(book)));
      meta.appendChild(this.progress(book));
      card.appendChild(meta);
      const remove = E.el(doc, 'button', 'card-remove', 'Remover');
      remove.type = 'button';
      remove.title = 'Remover da biblioteca';
      remove.addEventListener('click', (event) => {
        event.stopPropagation();
        this.askRemoval(book);
      });
      card.appendChild(remove);
      card.title = (book.title || '') + ' — ' + authorLine(book);
      card.addEventListener('click', () => this.open(book));
      card.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          this.open(book);
        } else if (event.key === 'Delete') {
          event.preventDefault();
          this.askRemoval(book);
        }
      });
      return card;
    }

    renderContinue(book) {
      const ui = this.ui;
      ui.continueCard.textContent = '';
      if (!book) {
        ui.continueSection.hidden = true;
        return;
      }
      const doc = this.doc;
      const row = E.el(doc, 'div', 'continue-row');
      row.appendChild(this.cover(book, true));
      const meta = E.el(doc, 'div', 'continue-meta');
      meta.appendChild(E.el(doc, 'div', 'continue-title', book.title || 'Sem título'));
      meta.appendChild(E.el(doc, 'div', 'card-author', authorLine(book)));
      meta.appendChild(this.progress(book));
      const go = E.el(doc, 'button', 'primary-button', 'Continuar lendo');
      go.type = 'button';
      go.addEventListener('click', () => this.open(book));
      meta.appendChild(go);
      row.appendChild(meta);
      ui.continueCard.appendChild(row);
      ui.continueSection.hidden = false;
    }

    render() {
      const ui = this.ui;
      const query = ui.search.value || '';
      const visible = sortBooks(filterBooks(this.books, query), ui.sort.value);
      ui.grid.textContent = '';
      for (const book of visible) ui.grid.appendChild(this.card(book));
      ui.empty.hidden = this.books.length > 0;
      ui.noMatch.hidden = !(this.books.length > 0 && visible.length === 0);
      ui.shelfTitle.textContent = this.books.length
        ? 'Biblioteca (' + visible.length + (query.trim() ? ' de ' + this.books.length : '') + ')'
        : 'Biblioteca';
      this.renderContinue(query.trim() ? null : continueReading(this.books));
    }
  }

  root.NeuraliaLibrary = Object.freeze({
    filterBooks,
    sortBooks,
    continueReading,
    coverHue,
    readerHref,
    LibraryPage,
  });

  if (typeof document !== 'undefined' && document.documentElement &&
      document.documentElement.getAttribute('data-page') === 'library') {
    const start = () => new LibraryPage(document, root).start();
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
    else start();
  }
})(globalThis);
