  // Grafo do Obsidian (Segundo Cérebro): correlação de notas, sites, histórico e tags.
  // Desenho em HTML5 Canvas nativo com filtros dinâmicos e controlos HUD.
  const obsidian = (() => {
    const oq = byId('oq');
    const canvas = byId('obsidian-canvas');
    const container = byId('obsidian-container');
    const card = byId('obsidian-card');
    const cardTitle = byId('obsidian-card-title');
    const cardDetail = byId('obsidian-card-detail');
    const cardAction = byId('obsidian-card-action');
    const cardClose = byId('obsidian-card-close');
    const countsEl = byId('obsidian-counts');
    const resetBtn = byId('obsidian-reset');
    const newNoteBtn = byId('obsidian-new-note');
    const refreshBtn = byId('obsidian-refresh');
    const orphanBtn = byId('filter-orphans');
    const zoomInBtn = byId('hud-zoom-in');
    const zoomOutBtn = byId('hud-zoom-out');
    const centerBtn = byId('hud-center');
    const freezeBtn = byId('hud-freeze');
    const labelsBtn = byId('hud-labels');

    const raf = typeof requestAnimationFrame === 'function' ? requestAnimationFrame : (cb) => setTimeout(cb, 16);
    function getCtx() {
      return canvas && typeof canvas.getContext === 'function' ? canvas.getContext('2d') : null;
    }

    let nodes = [];
    let edges = [];
    let activeNode = null;
    let hoveredNode = null;
    let panX = 0;
    let panY = 0;
    let zoom = 1;
    let isDragging = false;
    let isPanning = false;
    let dragNode = null;
    let lastMouseX = 0;
    let lastMouseY = 0;
    let animId = 0;
    let energy = 0;
    let physicsPaused = false;
    let showLabels = true;
    let hideOrphans = false;
    let needsCenter = false;
    let viewportWidth = 0;
    let viewportHeight = 0;
    const activeKinds = new Set(['note', 'site', 'history', 'tag']);

    const COLORS = {
      note: '#a855f7',
      site: '#3b82f6',
      history: '#10b981',
      tag: '#f59e0b'
    };

    const RADIUS = {
      note: 9,
      site: 7,
      history: 6,
      tag: 5
    };

    function resize() {
      if (!container || !canvas || typeof container.getBoundingClientRect !== 'function') return false;
      const rect = container.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return false;
      const dpr = window.devicePixelRatio || 1;
      const width = Math.max(1, Math.floor(rect.width * dpr));
      const height = Math.max(1, Math.floor(rect.height * dpr));
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      // Keep the current view centered when the panel width changes.
      panX += (rect.width - viewportWidth) / 2;
      panY += (rect.height - viewportHeight) / 2;
      viewportWidth = rect.width;
      viewportHeight = rect.height;
      renderCanvas();
      if (needsCenter) { needsCenter = false; center(); }
      return true;
    }

    function center() {
      needsCenter = false;
      if (!resize()) { needsCenter = true; return; }
      const visible = nodes.filter(isNodeVisible);
      let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
      for (const node of visible) {
        minX = Math.min(minX, node.x - 15);
        maxX = Math.max(maxX, node.x + 15);
        minY = Math.min(minY, node.y - 15);
        maxY = Math.max(maxY, node.y + 15);
      }
      // Leave room for labels and the HUD, including on narrow panels.
      zoom = visible.length ? Math.max(0.2, Math.min(1,
        Math.max(40, viewportWidth - 140) / Math.max(1, maxX - minX),
        Math.max(40, viewportHeight - 70) / Math.max(1, maxY - minY))) : 1;
      panX = (viewportWidth - 80) / 2 - (visible.length ? (minX + maxX) * zoom / 2 : 0);
      panY = viewportHeight / 2 - (visible.length ? (minY + maxY) * zoom / 2 : 0);
      renderCanvas();
      wake();
    }

    function normalized(text) {
      return String(text || '').normalize('NFD').replace(/[\u0300-\u036f]/g, '').toLowerCase();
    }

    function zoomStep(factor) {
      if (!container || typeof container.getBoundingClientRect !== 'function') return;
      const rect = container.getBoundingClientRect();
      const cx = (rect ? rect.width : 400) / 2;
      const cy = (rect ? rect.height : 400) / 2;
      const newZoom = Math.max(0.2, Math.min(3.5, zoom * factor));
      panX = cx - (cx - panX) * (newZoom / zoom);
      panY = cy - (cy - panY) * (newZoom / zoom);
      zoom = newZoom;
      wake();
    }

    function isNodeVisible(node) {
      if (!node) return false;
      if (!activeKinds.has(node.kind)) return false;
      if (hideOrphans && (node.degree || 0) === 0) return false;
      const query = normalized(oq && oq.value).trim();
      return !query || normalized([node.label, node.subtitle, node.target].join(' ')).includes(query);
    }

    function wake() {
      if (physicsPaused) {
        renderCanvas();
        return;
      }
      energy = 250;
      if (!animId) {
        animId = raf(tick);
      }
    }

    function tick() {
      const view = byId('view-obsidian');
      if (view && view.hidden) {
        animId = 0;
        return;
      }
      if (!physicsPaused) {
        stepPhysics();
      }
      renderCanvas();
      if (!physicsPaused && (energy > 0.05 || isDragging)) {
        animId = raf(tick);
      } else {
        animId = 0;
      }
    }

    function stepPhysics() {
      const visibleNodes = nodes.filter(isNodeVisible);
      const n = visibleNodes.length;
      if (n === 0) { energy = 0; return; }

      let currentEnergy = 0;

      // 1. Repulsão entre todos os nós visíveis (Coulomb)
      const repulsion = 1200;
      for (let i = 0; i < n; i++) {
        const a = visibleNodes[i];
        for (let j = i + 1; j < n; j++) {
          const b = visibleNodes[j];
          let dx = b.x - a.x;
          let dy = b.y - a.y;
          let distSq = dx * dx + dy * dy;
          if (distSq < 1) distSq = 1;
          const dist = Math.sqrt(distSq);
          if (dist > 350) continue;

          const force = repulsion / distSq;
          const fx = (dx / dist) * force;
          const fy = (dy / dist) * force;

          if (a !== dragNode) {
            a.vx -= fx;
            a.vy -= fy;
          }
          if (b !== dragNode) {
            b.vx += fx;
            b.vy += fy;
          }
        }
      }

      // 2. Atração pelas arestas visíveis (Hooke / mola)
      const springLength = 85;
      const stiffness = 0.045;
      for (let i = 0; i < edges.length; i++) {
        const edge = edges[i];
        const a = edge.sourceNode;
        const b = edge.targetNode;
        if (!a || !b || !isNodeVisible(a) || !isNodeVisible(b)) continue;

        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const dist = Math.sqrt(dx * dx + dy * dy) || 1;
        const displacement = dist - springLength;
        const fx = (dx / dist) * displacement * stiffness;
        const fy = (dy / dist) * displacement * stiffness;

        if (a !== dragNode) {
          a.vx += fx;
          a.vy += fy;
        }
        if (b !== dragNode) {
          b.vx += fx;
          b.vy += fy;
        }
      }

      // 3. Gravidade central e amortecimento
      const damping = 0.88;
      for (let i = 0; i < n; i++) {
        const node = visibleNodes[i];
        if (node === dragNode) continue;
        node.vx -= node.x * 0.003;
        node.vy -= node.y * 0.003;
        node.vx *= damping;
        node.vy *= damping;
        node.x += node.vx;
        node.y += node.vy;
        currentEnergy += Math.abs(node.vx) + Math.abs(node.vy);
      }

      energy = currentEnergy;
    }

    function updateCounts() {
      if (!countsEl) return;
      const visibleNodes = nodes.filter(isNodeVisible);
      let visibleEdgesCount = 0;
      for (let i = 0; i < edges.length; i++) {
        const e = edges[i];
        if (isNodeVisible(e.sourceNode) && isNodeVisible(e.targetNode)) {
          visibleEdgesCount++;
        }
      }
      const hiddenCount = nodes.length - visibleNodes.length;
      if (hiddenCount > 0) {
        countsEl.textContent = visibleNodes.length + ' nós (' + hiddenCount + ' ocultos) · ' + visibleEdgesCount + ' conexões';
      } else {
        countsEl.textContent = visibleNodes.length + ' nós · ' + visibleEdgesCount + ' conexões';
      }
    }

    function renderCanvas() {
      const ctx = getCtx();
      if (!ctx || !container || !canvas || typeof container.getBoundingClientRect !== 'function') return;
      const dpr = typeof window !== 'undefined' && window.devicePixelRatio ? window.devicePixelRatio : 1;
      ctx.clearRect(0, 0, canvas.width, canvas.height);

      ctx.save();
      ctx.scale(dpr, dpr);

      if (!nodes.some(isNodeVisible)) {
        ctx.fillStyle = '#999999';
        ctx.font = '13px "Segoe UI", system-ui, sans-serif';
        ctx.fillText(nodes.length ? 'Nenhum nó corresponde aos filtros.' : 'Nenhum nó ainda. Crie uma nota na aba Notas.',
          16, 30);
      }

      ctx.translate(panX, panY);
      ctx.scale(zoom, zoom);

      // Mapa de conexões do nó sob hover
      const connectedIds = new Set();
      if (hoveredNode && isNodeVisible(hoveredNode)) {
        connectedIds.add(hoveredNode.id);
        for (let i = 0; i < edges.length; i++) {
          const e = edges[i];
          if (e.sourceNode === hoveredNode && isNodeVisible(e.targetNode)) connectedIds.add(e.targetNode.id);
          if (e.targetNode === hoveredNode && isNodeVisible(e.sourceNode)) connectedIds.add(e.sourceNode.id);
        }
      }

      // 1. Desenhar arestas visíveis
      ctx.lineWidth = 1;
      for (let i = 0; i < edges.length; i++) {
        const e = edges[i];
        const a = e.sourceNode;
        const b = e.targetNode;
        if (!a || !b || !isNodeVisible(a) || !isNodeVisible(b)) continue;

        const isHovered = hoveredNode && (a === hoveredNode || b === hoveredNode);
        if (hoveredNode && !isHovered) {
          ctx.strokeStyle = 'rgba(120, 120, 120, 0.08)';
        } else if (isHovered) {
          ctx.strokeStyle = 'rgba(168, 85, 247, 0.85)';
          ctx.lineWidth = 1.8;
        } else {
          ctx.strokeStyle = 'rgba(140, 140, 140, 0.28)';
          ctx.lineWidth = 1;
        }

        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }

      // 2. Desenhar nós visíveis
      ctx.font = '11px "Segoe UI", system-ui, sans-serif';
      for (let i = 0; i < nodes.length; i++) {
        const node = nodes[i];
        if (!isNodeVisible(node)) continue;

        const isHovered = node === hoveredNode;
        const isSelected = node === activeNode;
        const isConnected = !hoveredNode || connectedIds.has(node.id);

        let alpha = 1;
        if (hoveredNode && !isConnected) {
          alpha = 0.25;
        }

        const color = COLORS[node.kind] || '#999999';
        const r = (RADIUS[node.kind] || 6) * (isHovered || isSelected ? 1.3 : 1);

        ctx.save();
        ctx.globalAlpha = alpha;

        // Círculo do nó
        ctx.beginPath();
        ctx.arc(node.x, node.y, r, 0, Math.PI * 2);
        ctx.fillStyle = color;
        ctx.fill();

        // Anel de destaque
        if (isHovered || isSelected) {
          ctx.lineWidth = 2;
          ctx.strokeStyle = '#ffffff';
          ctx.stroke();
        }

        // Rótulo de texto (visível se showLabels=true ou em hover/seleção)
        if (showLabels || isHovered || isSelected) {
          if (alpha > 0.4 || zoom >= 0.9 || isHovered || isSelected) {
            ctx.fillStyle = isHovered || isSelected ? '#ffffff' : (node.kind === 'note' ? '#e9d5ff' : '#cccccc');
            ctx.fillText(node.label, node.x + r + 4, node.y + 3.5);
          }
        }

        ctx.restore();
      }

      ctx.restore();
    }

    function getNodeAt(screenX, screenY) {
      if (!container || typeof container.getBoundingClientRect !== 'function') return null;
      const rect = container.getBoundingClientRect();
      if (!rect) return null;
      const localX = (screenX - rect.left - panX) / zoom;
      const localY = (screenY - rect.top - panY) / zoom;

      for (let i = nodes.length - 1; i >= 0; i--) {
        const n = nodes[i];
        if (!isNodeVisible(n)) continue;
        const r = (RADIUS[n.kind] || 6) + 4;
        const dx = localX - n.x;
        const dy = localY - n.y;
        if (dx * dx + dy * dy <= r * r) {
          return n;
        }
      }
      return null;
    }

    function showCard(node) {
      if (!card) return;
      activeNode = node;
      if (!node) {
        card.hidden = true;
        wake();
        return;
      }
      if (cardTitle) cardTitle.textContent = node.label;
      if (cardDetail) cardDetail.textContent = node.subtitle || node.kind;
      if (cardAction) {
        if (node.target) {
          cardAction.hidden = false;
          cardAction.textContent = node.kind === 'note' ? 'Abrir nota' : 'Abrir página';
        } else {
          cardAction.hidden = true;
        }
      }
      card.hidden = false;
      wake();
    }

    function openNode(node) {
      if (!node || !node.target) return;
      if (node.kind === 'note') {
        if (window.neuraliaShowSection && window.neuraliaShowSection('notes')) {
          post('note-open', { id: node.target });
        }
      } else {
        post('open', { input: node.target });
      }
    }

    function render(data) {
      if (!data || !Array.isArray(data.nodes)) return;
      const rawNodes = data.nodes || [];
      const rawEdges = data.edges || [];

      // Mapear nós e calcular posições circulares iniciais
      const nodeMap = new Map();
      const count = rawNodes.length;
      const radius = Math.min(220, Math.max(80, count * 14));

      nodes = rawNodes.map((n, idx) => {
        const angle = (idx / Math.max(1, count)) * Math.PI * 2;
        const node = {
          id: String(n.id),
          label: String(n.label || n.id),
          kind: String(n.kind || 'note'),
          target: n.target ? String(n.target) : null,
          subtitle: n.subtitle ? String(n.subtitle) : null,
          x: Math.cos(angle) * radius + (Math.random() - 0.5) * 20,
          y: Math.sin(angle) * radius + (Math.random() - 0.5) * 20,
          vx: 0,
          vy: 0,
          degree: 0
        };
        nodeMap.set(node.id, node);
        return node;
      });

      // Mapear arestas e calcular o grau dos nós
      edges = [];
      for (let i = 0; i < rawEdges.length; i++) {
        const e = rawEdges[i];
        const s = nodeMap.get(String(e.from));
        const t = nodeMap.get(String(e.to));
        if (s && t && s !== t) {
          s.degree = (s.degree || 0) + 1;
          t.degree = (t.degree || 0) + 1;
          edges.push({
            sourceNode: s,
            targetNode: t,
            kind: String(e.kind || 'link')
          });
        }
      }

      activeNode = null;
      hoveredNode = null;
      if (card) card.hidden = true;
      updateCounts();
      center();
    }

    function refresh() {
      resize();
      center();
      if (countsEl) countsEl.textContent = 'Atualizando grafo…';
      post('obsidian-graph');
    }

    function toggleActive(el, active) {
      if (!el) return;
      if (el.classList && typeof el.classList.toggle === 'function') {
        el.classList.toggle('active', active);
      } else {
        const cls = el.className || '';
        const parts = cls.split(' ').filter((p) => p && p !== 'active');
        if (active) parts.push('active');
        el.className = parts.join(' ');
      }
    }

    // Filtros por tipo de nó (pills)
    const kindChipIds = ['filter-kind-note', 'filter-kind-site', 'filter-kind-history', 'filter-kind-tag'];
    for (let i = 0; i < kindChipIds.length; i++) {
      const chip = byId(kindChipIds[i]);
      if (!chip) continue;
      chip.addEventListener('click', () => {
        const kind = chip.getAttribute('data-kind');
        if (activeKinds.has(kind)) {
          if (activeKinds.size > 1) {
            activeKinds.delete(kind);
            toggleActive(chip, false);
          }
        } else {
          activeKinds.add(kind);
          toggleActive(chip, true);
        }
        updateCounts();
        center();
      });
    }

    // Filtro de órfãos (nós sem links)
    if (orphanBtn) {
      orphanBtn.addEventListener('click', () => {
        hideOrphans = !hideOrphans;
        toggleActive(orphanBtn, hideOrphans);
        updateCounts();
        center();
      });
    }

    // Controles HUD
    if (zoomInBtn) zoomInBtn.addEventListener('click', () => zoomStep(1.25));
    if (zoomOutBtn) zoomOutBtn.addEventListener('click', () => zoomStep(0.8));
    if (centerBtn) centerBtn.addEventListener('click', center);
    if (freezeBtn) {
      freezeBtn.addEventListener('click', () => {
        physicsPaused = !physicsPaused;
        freezeBtn.textContent = physicsPaused ? '▶' : '⏸';
        toggleActive(freezeBtn, physicsPaused);
        if (!physicsPaused) wake();
        else renderCanvas();
      });
    }
    if (labelsBtn) {
      labelsBtn.addEventListener('click', () => {
        showLabels = !showLabels;
        toggleActive(labelsBtn, showLabels);
        renderCanvas();
      });
    }

    // Eventos do mouse e canvas
    if (canvas && typeof canvas.addEventListener === 'function') {
      canvas.addEventListener('mousedown', (e) => {
        if (!container || typeof container.getBoundingClientRect !== 'function') return;
        lastMouseX = e.clientX;
        lastMouseY = e.clientY;
        const hit = getNodeAt(e.clientX, e.clientY);
        if (hit) {
          dragNode = hit;
          isDragging = true;
          showCard(hit);
        } else {
          isPanning = true;
          showCard(null);
        }
      });

      canvas.addEventListener('dblclick', (e) => {
        const hit = getNodeAt(e.clientX, e.clientY);
        if (hit) openNode(hit);
      });

      canvas.addEventListener('wheel', (e) => {
        if (typeof e.preventDefault === 'function') e.preventDefault();
        if (!container || typeof container.getBoundingClientRect !== 'function') return;
        const rect = container.getBoundingClientRect();
        const mouseX = e.clientX - (rect ? rect.left : 0);
        const mouseY = e.clientY - (rect ? rect.top : 0);
        const factor = e.deltaY < 0 ? 1.15 : 0.87;
        const newZoom = Math.max(0.2, Math.min(3.5, zoom * factor));

        panX = mouseX - (mouseX - panX) * (newZoom / zoom);
        panY = mouseY - (mouseY - panY) * (newZoom / zoom);
        zoom = newZoom;
        wake();
      }, { passive: false });
    }

    if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
      const obsidianView = byId('view-obsidian');
      window.addEventListener('mousemove', (e) => {
        if (obsidianView && obsidianView.hidden) return;
        if (isDragging && dragNode) {
          if (!container || typeof container.getBoundingClientRect !== 'function') return;
          const rect = container.getBoundingClientRect();
          const localX = (e.clientX - (rect ? rect.left : 0) - panX) / zoom;
          const localY = (e.clientY - (rect ? rect.top : 0) - panY) / zoom;
          dragNode.x = localX;
          dragNode.y = localY;
          dragNode.vx = 0;
          dragNode.vy = 0;
          wake();
        } else if (isPanning) {
          panX += e.clientX - lastMouseX;
          panY += e.clientY - lastMouseY;
          lastMouseX = e.clientX;
          lastMouseY = e.clientY;
          wake();
        } else {
          const hit = getNodeAt(e.clientX, e.clientY);
          if (hit !== hoveredNode) {
            hoveredNode = hit;
            if (canvas && canvas.style) canvas.style.cursor = hit ? 'pointer' : 'grab';
            wake();
          }
        }
      });

      window.addEventListener('mouseup', () => {
        isDragging = false;
        isPanning = false;
        dragNode = null;
      });

      window.addEventListener('resize', resize);
    }

    if (oq && typeof oq.addEventListener === 'function') {
      const search = () => {
        if (activeNode && !isNodeVisible(activeNode)) showCard(null);
        hoveredNode = null;
        updateCounts();
        // Filtra no lugar. Recentrar a cada tecla fazia os nós saltarem
        // e a busca parecer que não tinha acertado.
        renderCanvas();
        wake();
      };
      oq.addEventListener('input', search);
      oq.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { e.preventDefault(); search(); }
      });
    }

    if (newNoteBtn) newNoteBtn.addEventListener('click', () => {
      if (window.__neuraliaNotes) window.__neuraliaNotes.newNote();
    });

    if (resetBtn && typeof resetBtn.addEventListener === 'function') {
      resetBtn.addEventListener('click', center);
    }
    if (refreshBtn && typeof refreshBtn.addEventListener === 'function') {
      refreshBtn.addEventListener('click', refresh);
    }
    if (cardClose && typeof cardClose.addEventListener === 'function') {
      cardClose.addEventListener('click', () => {
        if (card) card.hidden = true;
        activeNode = null;
        wake();
      });
    }
    if (cardAction && typeof cardAction.addEventListener === 'function') {
      cardAction.addEventListener('click', () => {
        if (activeNode) openNode(activeNode);
      });
    }

    if (typeof ResizeObserver === 'function' && container) {
      new ResizeObserver(resize).observe(container);
    }
    resize();

    return {
      refresh,
      render,
      open: openNode
    };
  })();
