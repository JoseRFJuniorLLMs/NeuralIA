// Fixture do spike de aceleradores (infra-accel-spike, plano 2.3). So CI:
// scripts/test-accel-spike.ps1 arranca-o, abre /fixture.html nos hospedeiros
// web do exe de spike e le o que a pagina viu pela sonda do exe.
//
// Uso: node scripts/accel-spike-fixture.mjs <ficheiro-da-porta>
// Escuta so em 127.0.0.1, numa porta livre, e escreve-a no ficheiro.
import http from 'node:http';
import fs from 'node:fs';

const portFile = process.argv[2];
if (!portFile) {
  console.error('uso: node scripts/accel-spike-fixture.mjs <ficheiro-da-porta>');
  process.exit(2);
}

// A pagina e "a pagina" do brief: ouvintes de keydown e keypress seus, no
// document (fase de bolha, como um site comum), que guardam tudo o que nao e
// modificador. O foco fica no body para a tecla cair no documento de topo.
const FIXTURE_HTML = `<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<title>NeuralIA accel spike fixture</title>
</head>
<body tabindex="-1" style="margin:0;min-height:100vh;font:16px system-ui,sans-serif">
<p style="padding:24px">Fixture do spike de aceleradores (CI).</p>
<script>
(function () {
  var seen = { down: [], press: [] };
  window.__fixtureSeen = seen;
  var skip = { Control: true, Shift: true, Alt: true, Meta: true };
  function record(list) {
    return function (e) {
      if (skip[e.key] === true) { return; }
      if (list.length >= 200) { return; }
      list.push({ key: String(e.key), code: String(e.code), ctrl: !!e.ctrlKey, shift: !!e.shiftKey, repeat: !!e.repeat, trusted: !!e.isTrusted });
    };
  }
  document.addEventListener('keydown', record(seen.down));
  document.addEventListener('keypress', record(seen.press));
  function focusBody() { try { document.body.focus(); } catch (e) {} }
  window.addEventListener('load', focusBody);
  window.addEventListener('focus', focusBody);
})();
</script>
</body>
</html>
`;

const server = http.createServer((request, response) => {
  const path = (request.url || '/').split('?')[0];
  if (request.method === 'GET' && path === '/fixture.html') {
    response.writeHead(200, {
      'content-type': 'text/html; charset=utf-8',
      'cache-control': 'no-store',
    });
    response.end(FIXTURE_HTML);
    return;
  }
  response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
  response.end('not found');
});

server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  fs.writeFileSync(portFile, String(port));
  console.log(`accel spike fixture: http://127.0.0.1:${port}/fixture.html`);
});
