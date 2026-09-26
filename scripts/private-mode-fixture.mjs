// Fixture do E2E do modo privado (infra-privacy-guard, plano 2.4). So CI:
// scripts/test-private-mode.ps1 arranca-o e abre /page.html no Reader do
// exe testado, para a sessao ter uma pagina que entre no historico e na
// memoria sem nada sair da maquina. O artigo tem texto que chegue para o
// Reader o extrair (extract_article).
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
<article>
<h1>Uma pagina da fixture do modo privado</h1>
<p>Texto que chegue para uma entrada no historico e um documento na memoria local.
O NeuralIA abriu esta pagina numa sessao normal, com uma pasta de dados temporaria,
e o script confere depois que so as lojas da tabela da SPEC-0006 mudaram.</p>
<p>O Reader extrai este artigo, grava a visita no historico pelo PrivacyGuard e
captura o texto na memoria semantica local, tambem pelo PrivacyGuard. Nenhum destes
passos sai da maquina: a fixture escuta so em 127.0.0.1 e nao tem ligacoes para fora.</p>
<p>Na fase 0 o script so enumera os ficheiros que uma sessao normal escreve na pasta
de dados e confere que cada um tem uma linha na tabela, com o tipo da loja. As fases
seguintes do modo privado alargam essa lista e passam a exigir que uma sessao privada
deixe as lojas automaticas byte a byte iguais.</p>
<p>Este paragrafo existe so para o artigo ter corpo suficiente para a heuristica de
leitura o escolher como o conteudo principal da pagina, e nao como uma nota curta.</p>
</article>
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
