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

// CRC-32 do ZIP (polinomio 0xEDB88320), para os ZIPs da inspecao
// (downloads-zip-inspect) serem ZIPs de verdade para o Explorador.
const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

// Um ZIP de entradas *stored*. Com `shared`, as entradas depois da primeira
// apontam para o cabecalho local dela: a bomba de sobreposicao, que a
// inspecao do NeuralIA nao lista (fica "nao inspecionado").
function storedZip(entries, shared = false) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  entries.forEach(([name, data], index) => {
    const nameBytes = Buffer.from(name, 'utf8');
    const crc = crc32(data);
    const reused = shared && index > 0;
    const at = reused ? 0 : offset;
    if (!reused) {
      const local = Buffer.alloc(30);
      local.writeUInt32LE(0x04034b50, 0);
      local.writeUInt16LE(20, 4);
      local.writeUInt16LE(0x21, 12);
      local.writeUInt32LE(crc, 14);
      local.writeUInt32LE(data.length, 18);
      local.writeUInt32LE(data.length, 22);
      local.writeUInt16LE(nameBytes.length, 26);
      locals.push(local, nameBytes, data);
      offset += 30 + nameBytes.length + data.length;
    }
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(0x031e, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x21, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(at, 42);
    centrals.push(central, nameBytes);
  });
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

// O pacote.zip do E2E: um programa e um script dentro.
const PACOTE_ZIP = storedZip([
  ['setup.exe', peBytes()],
  ['run.bat', Buffer.from('@echo off\r\necho oi\r\n', 'latin1')],
]);

// Duas fotos no mesmo cabecalho local: nomes inofensivos, estrutura que o
// NeuralIA recusa listar.
const JPEG = Buffer.from([0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0xff, 0xd9]);
const SOBREPOSTO_ZIP = storedZip(
  [
    ['fotos/a.jpg', JPEG],
    ['fotos/b.jpg', JPEG],
  ],
  true,
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
    // Sem `.zip` no caminho, como o relatorio: o nome vem do
    // content-disposition.
    case '/files/pacote': {
      attachment(response, 'pacote.zip', 'application/zip', PACOTE_ZIP.length);
      response.end(PACOTE_ZIP);
      return;
    }
    case '/files/sobreposto': {
      attachment(response, 'sobreposto.zip', 'application/zip', SOBREPOSTO_ZIP.length);
      response.end(SOBREPOSTO_ZIP);
      return;
    }
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
