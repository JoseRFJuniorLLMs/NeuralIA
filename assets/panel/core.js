(() => {
  if (window.top !== window) return;
  const post = (action, args) => window.ipc.postMessage(JSON.stringify({ action, args: args || {} }));
  const byId = (id) => document.getElementById(id);
  const make = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  const q = byId('q');
  q.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && q.value.trim()) { e.preventDefault(); post('search', { query: q.value.trim() }); }
  });
  const theme = (vars) => { for (const k of Object.keys(vars)) document.documentElement.style.setProperty(k, vars[k]); };
