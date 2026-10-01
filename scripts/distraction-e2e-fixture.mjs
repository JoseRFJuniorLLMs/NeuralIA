// SPEC-0114 WebView2 E2E fixture. CI only.
// The browser loads this server through news.distraction.test -> 127.0.0.1.
// Pass 1 reloads once so async AddScriptToExecuteOnDocumentCreated can settle.
import http from 'node:http';
import fs from 'node:fs';

const portFile = process.argv[2];
if (!portFile) throw new Error('usage: node distraction-e2e-fixture.mjs <port-file>');
const seen = [];

function page(pass) {
  if (pass !== '2') {
    return `<!doctype html>
<meta charset="utf-8">
<title>NeuralIA distraction E2E bootstrap</title>
<p>bootstrap</p>
<script>setTimeout(() => location.replace('/page.html?pass=2'), 750);</script>`;
  }
  return `<!doctype html>
<html lang="pt-BR"><head><meta charset="utf-8"><title>NeuralIA distraction E2E</title>
<style>
html,body { min-height: 200vh; margin: 0; }
#cookie-banner { position: fixed; left: 0; top: 0; width: 100%; height: 120px; background: white; z-index: 999999; }
</style></head><body>
<div id="cookie-banner" role="dialog">Preferências de cookies</div>
<main style="padding-top:160px">Conteúdo normal</main>
<script>
(() => {
  const report = (what) => fetch('/report?what=' + encodeURIComponent(what), {cache:'no-store'}).catch(() => {});
  let tries = 0;
  const timer = setInterval(() => {
    const banner = document.getElementById('cookie-banner');
    const hidden = !!banner && (banner.style.display === 'none' || getComputedStyle(banner).display === 'none');
    if (hidden) { clearInterval(timer); report('hidden'); return; }
    tries += 1;
    if (tries >= 30) { clearInterval(timer); report('visible'); }
  }, 100);
})();
</script></body></html>`;
}

const server = http.createServer((request, response) => {
  const url = new URL(request.url || '/', 'http://fixture.invalid');
  seen.push({host:String(request.headers.host || ''), path:url.pathname, query:url.searchParams.toString()});
  const send = (status, type, body) => {
    response.writeHead(status, {'content-type':type, 'cache-control':'no-store'});
    response.end(body);
  };
  if (request.method !== 'GET') return send(405, 'text/plain; charset=utf-8', 'method');
  if (url.pathname === '/page.html') return send(200, 'text/html; charset=utf-8', page(url.searchParams.get('pass')));
  if (url.pathname === '/report') return send(204, 'text/plain; charset=utf-8', '');
  if (url.pathname === '/__log') return send(200, 'application/json; charset=utf-8', JSON.stringify(seen));
  return send(404, 'text/plain; charset=utf-8', 'not found');
});

server.listen(0, '127.0.0.1', () => fs.writeFileSync(portFile, String(server.address().port), 'utf8'));
for (const signal of ['SIGINT','SIGTERM']) process.on(signal, () => server.close(() => process.exit(0)));
