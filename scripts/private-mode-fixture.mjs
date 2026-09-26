// Fixture do E2E do modo privado (infra-privacy-guard, plano 2.4). So CI:
// scripts/test-private-mode.ps1 arranca-o e abre /page.html na Web completa
// do exe testado, para a sessao ter uma pagina que entre no historico, na
// memoria e nas abas sem nada sair da maquina.
//
// Uso: node scripts/private-mode-fixture.mjs <pasta-de-estado>
// Escuta so em 127.0.0.1, numa porta livre; escreve a porta em
// <pasta>/port e cada pedido em <pasta>/events.jsonl (uma linha JSON por
// pedido, com o instante em ms).
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const stateDir = process.argv[2];
if (!stateDir) {
  console.error('uso: node scripts/private-mode-fixture.mjs <pasta-de-estado>');
  process.exit(2);
}
fs.mkdirSync(stateDir, { recursive: true });
const eventsFile = path.join(stateDir, 'events.jsonl');

function record(event) {
  fs.appendFileSync(eventsFile, JSON.stringify({ t: Date.now(), ...event }) + '\n');
}

const PAGE = `<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<title>NeuralIA private mode fixture</title>
</head>
<body>
<h1>Uma pagina da fixture do modo privado</h1>
<p>Texto que chegue para uma entrada no historico e um documento na memoria local.
O NeuralIA abriu esta pagina numa sessao normal, com uma pasta de dados temporaria,
e o script confere depois que so as lojas da tabela da SPEC-0006 mudaram.</p>
</body>
</html>
`;

const server = http.createServer((request, response) => {
  const url = (request.url || '/').split('?')[0];
  record({ kind: 'request', method: request.method, path: url });
  if (request.method !== 'GET') {
    response.writeHead(405).end();
    return;
  }
  if (url === '/page.html') {
    response.writeHead(200, {
      'content-type': 'text/html; charset=utf-8',
      'cache-control': 'no-store',
    });
    response.end(PAGE);
    return;
  }
  response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
  response.end('nao existe');
});

server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  fs.writeFileSync(path.join(stateDir, 'port'), String(port));
  console.log(`private mode fixture em http://127.0.0.1:${port}/`);
});
