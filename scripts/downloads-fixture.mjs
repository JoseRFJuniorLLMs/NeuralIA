// Fixture do E2E de downloads (downloads-manager, plano 2.3). So CI:
// scripts/test-downloads.ps1 arranca-o e abre /files/<nome> no exe.
//
// Uso: node scripts/downloads-fixture.mjs <pasta-de-estado>
// Escuta so em 127.0.0.1, numa porta livre; escreve a porta em
// <pasta>/port e cada pedido e cada fim de resposta em <pasta>/events.jsonl
// (uma linha JSON por evento, com o instante em ms).
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const stateDir = process.argv[2];
if (!stateDir) {
  console.error('uso: node scripts/downloads-fixture.mjs <pasta-de-estado>');
  process.exit(2);
}
fs.mkdirSync(stateDir, { recursive: true });
const eventsFile = path.join(stateDir, 'events.jsonl');

function record(event) {
  fs.appendFileSync(eventsFile, JSON.stringify({ t: Date.now(), ...event }) + '\n');
}

// Um PE minimo: MZ, o deslocamento do cabecalho em 0x3C e PE\0\0 em 0x80.
// Nunca corre: so tem de parecer um programa ao sniff e ao nome.
function peBytes() {
  const bytes = Buffer.alloc(4096);
  bytes.write('MZ', 0, 'latin1');
  bytes.writeUInt32LE(0x80, 0x3c);
  bytes.write('PE\0\0', 0x80, 'latin1');
  return bytes;
}

const PDF = Buffer.from(
  '%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n' +
    '2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n' +
    'trailer\n<< /Root 1 0 R >>\n%%EOF\n',
  'latin1',
);

function attachment(response, name, type, length) {
  response.writeHead(200, {
    'content-type': type,
    'content-disposition': `attachment; filename="${name}"`,
    'content-length': String(length),
    'cache-control': 'no-store',
  });
}

// O download lento do spike: 8 MiB, 64 KiB a cada 200 ms (~26 s). Se o
// cliente fecha a ligacao antes do fim, fica `aborted` com o que ja foi.
const SLOW_TOTAL = 8 * 1024 * 1024;
const SLOW_CHUNK = 64 * 1024;
const SLOW_EVERY_MS = 200;

function slow(request, response) {
  attachment(response, 'lento.bin', 'application/octet-stream', SLOW_TOTAL);
  let sent = 0;
  let done = false;
  const chunk = Buffer.alloc(SLOW_CHUNK, 0x61);
  const timer = setInterval(() => {
    if (done) {
      return;
    }
    const size = Math.min(SLOW_CHUNK, SLOW_TOTAL - sent);
    response.write(chunk.subarray(0, size));
    sent += size;
    if (sent >= SLOW_TOTAL) {
      done = true;
      clearInterval(timer);
      response.end();
      record({ kind: 'finished', path: '/files/lento', sent });
    }
  }, SLOW_EVERY_MS);
  response.on('close', () => {
    clearInterval(timer);
    if (!done) {
      done = true;
      record({ kind: 'aborted', path: '/files/lento', sent });
    }
  });
}

const server = http.createServer((request, response) => {
  const url = (request.url || '/').split('?')[0];
  record({ kind: 'request', method: request.method, path: url });
  if (request.method !== 'GET') {
    response.writeHead(405).end();
    return;
  }
  switch (url) {
    case '/files/setup.exe': {
      const bytes = peBytes();
      attachment(response, 'setup.exe', 'application/octet-stream', bytes.length);
      response.end(bytes);
      return;
    }
    // Sem `.pdf` no caminho: um endereco que acaba em .pdf abre no leitor
    // de PDF do NeuralIA, que recusa downloads.
    case '/files/relatorio': {
      attachment(response, 'relatorio.pdf', 'application/pdf', PDF.length);
      response.end(PDF);
      return;
    }
    case '/files/lento':
      slow(request, response);
      return;
    default:
      response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
      response.end('nao existe');
  }
});

server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  fs.writeFileSync(path.join(stateDir, 'port'), String(port));
  console.log(`downloads fixture em http://127.0.0.1:${port}/`);
});
