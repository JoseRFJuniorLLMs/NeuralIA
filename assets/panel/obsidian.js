  // Grafo do Obsidian (Segundo Cérebro): correlação de notas, sites, histórico e tags.
  // Desenho em HTML5 Canvas nativo, sem bibliotecas externas.
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
    const refreshBtn = byId('obsidian-refresh');

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
      if (!container || !canvas || typeof container.getBoundingClientRect !== 'function') return;
      const rect = container.getBoundingClientRect();
      if (!rect || rect.width <= 0 || rect.height <= 0) return;
      const dpr = typeof window !== 'undefined' && window.devicePixelRatio ? window.devicePixelRatio : 1;
      canvas.width = Math.floor(rect.width * dpr);
      canvas.height = Math.floor(rect.height * dpr);
      if (canvas.style) {
        canvas.style.width = rect.width + 'px';
        canvas.style.height = rect.height + 'px';
      }
      wake();
    }

    function center() {
      if (!container || typeof container.getBoundingClientRect !== 'function') return;
      const rect = container.getBoundingClientRect();
      panX = (rect && rect.width > 0 ? rect.width : 400) / 2;
      panY = (rect && rect.height > 0 ? rect.height : 400) / 2;
      zoom = 1;
      wake();
    }

    function wake() {
      energy = 250;
      if (!animId) {
        animId = raf(tick);
      }
    }

    function tick() {
      stepPhysics();
      renderCanvas();
      if (energy > 0.05 || isDragging) {
        animId = raf(tick);
      } else {
        animId = 0;
      }
    }

    function stepPhysics() {
      const n = nodes.length;
      if (n === 0) { energy = 0; return; }

      let currentEnergy = 0;

      // 1. Repulsão entre todos os nós (Coulomb)
      const repulsion = 1200;
      for (let i = 0; i < n; i++) {
        const a = nodes[i];
        for (let j = i + 1; j < n; j++) {
          const b = nodes[j];
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

      // 2. Atração pelas arestas (Hooke / mola)
      const springLength = 85;
      const stiffness = 0.045;
      for (let i = 0; i < edges.length; i++) {
        const edge = edges[i];
        const a = edge.sourceNode;
        const b = edge.targetNode;
        if (!a || !b) continue;

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
          b.vx -= fx;
          b.vy -= fy;
        }
      }

      // 3. Gravidade central e amortecimento
      const damping = 0.88;
      for (let i = 0; i < n; i++) {
        const node = nodes[i];
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

    function renderCanvas() {
      const ctx = getCtx();
      if (!ctx || !container || !canvas || typeof container.getBoundingClientRect !== 'function') return;
      const dpr = typeof window !== 'undefined' && window.devicePixelRatio ? window.devicePixelRatio : 1;
      ctx.clearRect(0, 0, canvas.width, canvas.height);

      ctx.save();
      ctx.scale(dpr, dpr);
      ctx.translate(panX, panY);
      ctx.scale(zoom, zoom);

      const query = (oq && oq.value ? oq.value : '').trim().toLowerCase();
      const hasFilter = query.length > 0;

      // Mapa de conexões do nó hovered
      const connectedIds = new Set();
      if (hoveredNode) {
        connectedIds.add(hoveredNode.id);
        for (let i = 0; i < edges.length; i++) {
          const e = edges[i];
          if (e.sourceNode === hoveredNode) connectedIds.add(e.targetNode.id);
          if (e.targetNode === hoveredNode) connectedIds.add(e.sourceNode.id);
        }
      }

      // 1. Desenhar arestas
      ctx.lineWidth = 1;
      for (let i = 0; i < edges.length; i++) {
        const e = edges[i];
        const a = e.sourceNode;
        const b = e.targetNode;
        if (!a || !b) continue;

        const isHovered = hoveredNode && (a === hoveredNode || b === hoveredNode);
        if (hoveredNode && !isHovered) {
          ctx.strokeStyle = 'rgba(120, 120, 120, 0.1)';
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

      // 2. Desenhar nós
      ctx.font = '11px "Segoe UI", system-ui, sans-serif';
      for (let i = 0; i < nodes.length; i++) {
        const node = nodes[i];
        const isHovered = node === hoveredNode;
        const isSelected = node === activeNode;
        const isConnected = !hoveredNode || connectedIds.has(node.id);
        const matchesQuery = !hasFilter || node.label.toLowerCase().includes(query);

        let alpha = 1;
        if (hasFilter && !matchesQuery) {
          alpha = 0.2;
        } else if (hoveredNode && !isConnected) {
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

        // Rótulo de texto
        if (alpha > 0.4 || zoom >= 0.9) {
          ctx.fillStyle = isHovered || isSelected ? '#ffffff' : (node.kind === 'note' ? '#e9d5ff' : '#cccccc');
          ctx.fillText(node.label, node.x + r + 4, node.y + 3.5);
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
          vy: 0
        };
        nodeMap.set(node.id, node);
        return node;
      });

      // Mapear arestas
      edges = [];
      for (let i = 0; i < rawEdges.length; i++) {
        const e = rawEdges[i];
        const s = nodeMap.get(String(e.from));
        const t = nodeMap.get(String(e.to));
        if (s && t && s !== t) {
          edges.push({
            sourceNode: s,
            targetNode: t,
            kind: String(e.kind || 'link')
          });
        }
      }

      if (countsEl) countsEl.textContent = nodes.length + ' nós · ' + edges.length + ' conexões';
      center();
    }

    function refresh() {
      if (countsEl) countsEl.textContent = 'Atualizando grafo…';
      post('obsidian-graph');
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
      window.addEventListener('mousemove', (e) => {
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
      oq.addEventListener('input', () => {
        wake();
      });
    }

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

    setTimeout(resize, 100);

    return {
      refresh,
      render,
      open: openNode
    };
  })();
