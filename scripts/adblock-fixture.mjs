// Fixture do E2E do bloqueio de anuncios (adblock, plano 2.3). So CI:
// scripts/test-adblock.ps1 arranca-o, abre /page.html na Web completa do exe
// testado e le em /__log o que chegou aqui.
//
// Uso: node scripts/adblock-fixture.mjs <ficheiro-da-porta>
// Escuta so em 127.0.0.1, numa porta livre, e escreve-a no ficheiro. Os nomes
// `*.test` da pagina chegam aqui porque o E2E corre o WebView2 com
// `--host-resolver-rules="MAP * 127.0.0.1"`: nada sai da maquina, e o
// cabecalho Host diz de que nome veio cada pedido.
import http from 'node:http';
import fs from 'node:fs';

const portFile = process.argv[2];
if (!portFile) {
  console.error('uso: node scripts/adblock-fixture.mjs <ficheiro-da-porta>');
  process.exit(2);
}

// 1x1 PNG transparente.
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=',
  'base64',
);

const seen = [];

function page(port) {
  // O documento de topo e local (127.0.0.1); o anuncio e um script de
  // ads.blocked.test (na lista); a moldura e um DOCUMENTO do mesmo dominio
  // (nunca bloqueado); a imagem de allowed.test nao esta na lista (o
  // controlo de que os nomes resolvem para aqui).
  return `<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<title>NeuralIA adblock fixture</title>
<script>
function report(what) { new Image().src = '/report?what=' + encodeURIComponent(what); }
</script>
</head>
<body>
<p>Fixture do bloqueio de anuncios (CI).</p>
<iframe src="http://ads.blocked.test:${port}/frame.html" width="80" height="40"></iframe>
<img src="http://allowed.test:${port}/ok.png" onload="report('ok-load')" onerror="report('ok-error')">
<img src="/first.png">
<script src="http://ads.blocked.test:${port}/ad.js" onload="report('ad-load')" onerror="report('ad-error')"></script>
</body>
</html>
`;
}

const server = http.createServer((request, response) => {
  const [path, query = ''] = (request.url || '/').split('?');
  seen.push({ host: String(request.headers.host || ''), path, query });
  const port = server.address().port;
  const send = (status, type, body) => {
    response.writeHead(status, { 'content-type': type, 'cache-control': 'no-store' });
    response.end(body);
  };
  if (request.method !== 'GET') {
    send(405, 'text/plain; charset=utf-8', 'method');
    return;
  }
  switch (path) {
    case '/page.html':
      send(200, 'text/html; charset=utf-8', page(port));
      return;
    case '/frame.html':
      send(200, 'text/html; charset=utf-8', '<!doctype html><p>moldura</p>');
      return;
    case '/ad.js':
      send(200, 'text/javascript; charset=utf-8', 'window.__adLoaded = true;');
      return;
    case '/ok.png':
    case '/first.png':
      send(200, 'image/png', PIXEL);
      return;
    case '/report':
      send(204, 'text/plain; charset=utf-8', '');
      return;
    case '/__log':
      send(200, 'application/json; charset=utf-8', JSON.stringify(seen));
      return;
    default:
      send(404, 'text/plain; charset=utf-8', 'not found');
  }
});

server.listen(0, '127.0.0.1', () => {
  fs.writeFileSync(portFile, String(server.address().port));
});
