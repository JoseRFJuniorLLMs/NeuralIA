import http from 'node:http';
import fs from 'node:fs';

const portFile = process.argv[2];
if (!portFile) throw new Error('usage: node research-e2e-fixture.mjs <port-file>');

const ANSWER = 'NEURALIA_SPEC0101_ANSWER_7F9C';
const SOURCE = 'https://example.test/spec0101-source';

const server = http.createServer((req, res) => {
  if (req.url === '/health') {
    res.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' });
    res.end('ok');
    return;
  }
  if (req.url?.startsWith('/search')) {
    res.writeHead(200, {
      'content-type': 'text/html; charset=utf-8',
      'cache-control': 'no-store',
    });
    res.end(`<!doctype html>
<meta charset="utf-8">
<title>SPEC-0101 E2E</title>
<main>
  <h1>Resposta de pesquisa</h1>
  <p>${ANSWER}</p>
  <p>Proveniência: <a href="${SOURCE}">fonte SPEC-0101</a></p>
</main>`);
    return;
  }
  res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
  res.end('not found');
});

server.listen(0, '127.0.0.1', () => {
  const address = server.address();
  fs.writeFileSync(portFile, String(address.port), 'utf8');
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
