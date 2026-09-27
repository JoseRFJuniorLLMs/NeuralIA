#!/usr/bin/env node
// Gate do MCP dos agentes externos: um cliente MCP (stdio, JSON-RPC 2.0, uma
// mensagem por linha) conduz o `NeuralIA.exe --mcp` QUE EMBARCA.
//
//   node scripts/test-agents-mcp.mjs [--exe <NeuralIA.exe>]
//     Sem NeuralIA aberto (pasta de dados vazia): ciclo de vida, versoes,
//     tools/list e os seus schemas, ping, codigos de erro, validacao das
//     ferramentas, o erro «Abra o NeuralIA», pureza do stdout, saida quando o
//     stdin fecha e argumentos invalidos.
//
//   node scripts/test-agents-mcp.mjs --with-hub --exe <NeuralIA.exe>
//     Com um hub a correr sobre NEURALIA_DATA_DIR. Quem o arranca e o teste
//     Rust `agents::pipe::tests::shipped_bridge_exe_speaks_mcp_to_the_hub_over_the_pipe`,
//     que faz de utilizador: responde «Sim» a «responda Sim», escreve «olá do
//     teste» a «responda por texto» e deixa «deixe expirar» passar o prazo.
//
// Termina com codigo 0 e «agents-mcp: ok», ou com codigo 1 e o motivo.

import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const MODERN = '2026-07-28';
const argv = process.argv.slice(2);
const withHub = argv.includes('--with-hub');

if (process.platform !== 'win32') {
  console.error('agents-mcp: este gate corre o NeuralIA.exe e so existe no Windows.');
  process.exit(2);
}

function resolveExe() {
  const at = argv.indexOf('--exe');
  if (at >= 0 && argv[at + 1]) return path.resolve(argv[at + 1]);
  if (process.env.NEURALIA_EXE) return path.resolve(process.env.NEURALIA_EXE);
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const targets = [process.env.CARGO_TARGET_DIR, path.join(root, 'target')].filter(Boolean);
  for (const target of targets) {
    for (const profile of ['release', 'debug']) {
      const candidate = path.join(target, profile, 'NeuralIA.exe');
      if (fs.existsSync(candidate)) return candidate;
    }
  }
  throw new Error('NeuralIA.exe nao encontrado: passe --exe <caminho>');
}

class McpProcess {
  constructor(exe, args, env) {
    this.child = spawn(exe, args, { env, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    this.raw = '';
    this.pending = '';
    this.messages = [];
    this.waiters = [];
    this.violations = [];
    this.stderr = '';
    this.child.stdout.setEncoding('utf8');
    this.child.stdout.on('data', (chunk) => {
      this.raw += chunk;
      this.pending += chunk;
      let at;
      while ((at = this.pending.indexOf('\n')) >= 0) {
        const line = this.pending.slice(0, at);
        this.pending = this.pending.slice(at + 1);
        this.onLine(line);
      }
    });
    this.child.stderr.setEncoding('utf8');
    this.child.stderr.on('data', (chunk) => {
      this.stderr += chunk;
    });
    this.exited = new Promise((resolve) => {
      this.child.on('exit', (code, signal) => resolve({ code, signal }));
    });
    this.child.stdin.on('error', () => {});
  }

  onLine(line) {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      this.violations.push(`linha do stdout que nao e JSON: ${JSON.stringify(line)}`);
      return;
    }
    if (message === null || typeof message !== 'object' || message.jsonrpc !== '2.0') {
      this.violations.push(`linha do stdout que nao e JSON-RPC 2.0: ${line}`);
    }
    this.messages.push(message);
    this.deliver();
  }

  deliver() {
    for (const waiter of [...this.waiters]) {
      const index = this.messages.findIndex(waiter.predicate);
      if (index >= 0) {
        const [message] = this.messages.splice(index, 1);
        this.waiters.splice(this.waiters.indexOf(waiter), 1);
        clearTimeout(waiter.timer);
        waiter.resolve(message);
      }
    }
  }

  next(predicate, what, timeoutMs = 15000) {
    return new Promise((resolve, reject) => {
      const waiter = { predicate, resolve };
      waiter.timer = setTimeout(() => {
        this.waiters.splice(this.waiters.indexOf(waiter), 1);
        reject(new Error(`sem resposta: ${what}\nstderr: ${this.stderr}`));
      }, timeoutMs);
      this.waiters.push(waiter);
      this.deliver();
    });
  }

  send(message) {
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  line(text) {
    this.child.stdin.write(`${text}\n`);
  }

  reply(id) {
    return this.next((m) => m.id === id && !('method' in m), `resposta ao id ${JSON.stringify(id)}`);
  }

  async request(id, method, params) {
    this.send({ jsonrpc: '2.0', id, method, params });
    return this.reply(id);
  }

  async call(id, name, args, extra = {}) {
    return this.request(id, 'tools/call', { name, arguments: args, ...extra });
  }

  async close(timeoutMs = 8000) {
    this.child.stdin.end();
    const timer = new Promise((_, reject) =>
      setTimeout(() => reject(new Error('a ponte nao saiu depois de o stdin fechar')), timeoutMs),
    );
    return Promise.race([this.exited, timer]);
  }

  assertPure() {
    assert.deepEqual(this.violations, [], 'stdout so pode ter mensagens MCP');
    assert.ok(this.raw === '' || this.raw.endsWith('\n'), 'cada mensagem termina em \\n');
    assert.equal(this.pending, '', 'nada a meio de uma linha');
  }
}

function toolResult(reply) {
  assert.ok(reply.result, `esperava resultado: ${JSON.stringify(reply)}`);
  return reply.result;
}

function structured(reply) {
  const result = toolResult(reply);
  assert.equal(result.isError, false, JSON.stringify(result));
  const text = JSON.parse(result.content[0].text);
  assert.deepEqual(result.structuredContent, text, 'structuredContent == texto');
  return result.structuredContent;
}

function toolError(reply) {
  const result = toolResult(reply);
  assert.equal(result.isError, true, JSON.stringify(result));
  assert.equal(result.content[0].type, 'text');
  return result.content[0].text;
}

function freshDataDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'neuralia-agents-mcp-'));
}

async function withoutHub(exe) {
  const dataDir = freshDataDir();
  const env = { ...process.env, NEURALIA_DATA_DIR: dataDir };
  const bridge = new McpProcess(exe, ['--mcp', '--agent', 'claude'], env);
  try {
    // So ping antes do initialize.
    const early = await bridge.request(1, 'tools/list', {});
    assert.equal(early.error?.code, -32600, JSON.stringify(early));
    assert.deepEqual((await bridge.request(2, 'ping', {})).result, {});

    const init = await bridge.request(3, 'initialize', {
      protocolVersion: '2025-11-25',
      capabilities: {},
      clientInfo: { name: 'test-agents-mcp', version: '1' },
    });
    assert.equal(init.result.protocolVersion, '2025-11-25');
    assert.deepEqual(init.result.capabilities, { tools: { listChanged: false } });
    assert.equal(init.result.serverInfo.name, 'neuralia');
    assert.match(init.result.instructions, /ask_user/);
    bridge.send({ jsonrpc: '2.0', method: 'notifications/initialized' });

    const list = await bridge.request(4, 'tools/list', {});
    const tools = list.result.tools;
    assert.deepEqual(
      tools.map((tool) => tool.name),
      ['send_message', 'ask_user', 'get_user_messages', 'set_status'],
    );
    for (const tool of tools) {
      assert.equal(tool.inputSchema.type, 'object', tool.name);
      assert.equal(tool.inputSchema.additionalProperties, false, tool.name);
      assert.match(tool.description, / \/ /, `${tool.name}: pt-BR / en`);
    }
    const byName = Object.fromEntries(tools.map((tool) => [tool.name, tool.inputSchema]));
    assert.deepEqual(byName.send_message.required, ['text']);
    assert.equal(byName.send_message.properties.text.maxLength, 4000);
    assert.equal(byName.ask_user.properties.question.maxLength, 1000);
    assert.equal(byName.ask_user.properties.options.maxItems, 4);
    assert.equal(byName.ask_user.properties.options.items.maxLength, 40);
    assert.equal(byName.ask_user.properties.timeout_seconds.minimum, 10);
    assert.equal(byName.ask_user.properties.timeout_seconds.maximum, 3600);
    assert.equal(byName.set_status.properties.text.maxLength, 200);
    assert.equal(byName.set_status.properties.progress.maximum, 100);

    // Codigos de erro JSON-RPC.
    bridge.line('{isto nao e json');
    const parse = await bridge.next((m) => m.error?.code === -32700, 'parse error');
    assert.equal(parse.id, null);
    bridge.line('[{"jsonrpc":"2.0","id":90,"method":"ping"}]');
    await bridge.next((m) => m.error?.code === -32600 && m.id === null, 'batch recusado');
    assert.equal((await bridge.request(5, 'resources/list', {})).error.code, -32601);
    assert.equal((await bridge.call(6, 'apagar_tudo', {})).error.code, -32602);

    // Validacao: erro de execucao, para o modelo corrigir.
    assert.match(toolError(await bridge.call(7, 'send_message', { text: 'x', extra: 1 })), /extra/);
    assert.match(toolError(await bridge.call(8, 'send_message', { text: 'y'.repeat(4001) })), /4000/);
    assert.match(
      toolError(await bridge.call(9, 'ask_user', { question: '?', options: ['a', 'b', 'c', 'd', 'e'] })),
      /options/,
    );
    toolError(await bridge.call(10, 'ask_user', { question: '?', timeout_seconds: 5 }));
    toolError(await bridge.call(11, 'set_status', { text: 's', progress: 101 }));

    // Sem NeuralIA: erro claro, depressa, nunca pendurado.
    const started = Date.now();
    for (const [id, name, args] of [
      [12, 'send_message', { text: 'olá' }],
      [13, 'ask_user', { question: 'Posso?' }],
      [14, 'get_user_messages', {}],
      [15, 'set_status', { text: 'a trabalhar' }],
    ]) {
      assert.match(toolError(await bridge.call(id, name, args)), /Abra o NeuralIA/);
    }
    assert.ok(Date.now() - started < 8000, 'o erro de NeuralIA fechado demorou demais');

    // Era moderna (2026-07-28): sem initialize, versao em cada pedido.
    const meta = {
      'io.modelcontextprotocol/protocolVersion': MODERN,
      'io.modelcontextprotocol/clientCapabilities': {},
    };
    const discover = await bridge.request(16, 'server/discover', { _meta: meta });
    assert.equal(discover.result.resultType, 'complete');
    assert.ok(discover.result.supportedVersions.includes(MODERN));
    assert.ok(discover.result.supportedVersions.includes('2025-11-25'));
    const unsupported = await bridge.request(17, 'tools/list', {
      _meta: { ...meta, 'io.modelcontextprotocol/protocolVersion': '2099-12-31' },
    });
    assert.equal(unsupported.error.code, -32022);
    assert.equal(unsupported.error.data.requested, '2099-12-31');

    const exit = await bridge.close();
    assert.equal(exit.code, 0, `codigo de saida ${exit.code}\n${bridge.stderr}`);
    bridge.assertPure();
  } finally {
    bridge.child.kill();
    fs.rmSync(dataDir, { recursive: true, force: true });
  }

  // Argumentos invalidos: codigo 2, stdout vazio, nenhuma janela.
  const bad = new McpProcess(exe, ['--mcp', '--agent', 'nul'], {
    ...process.env,
    NEURALIA_DATA_DIR: freshDataDir(),
  });
  const exit = await bad.close();
  assert.equal(exit.code, 2);
  assert.equal(bad.raw, '');
  assert.match(bad.stderr, /--mcp/);
}

async function withTheHub(exe) {
  assert.ok(process.env.NEURALIA_DATA_DIR, '--with-hub precisa de NEURALIA_DATA_DIR');
  const bridge = new McpProcess(exe, ['--mcp', '--agent', 'claude'], process.env);
  try {
    const init = await bridge.request(0, 'initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'test-agents-mcp', version: '1' },
    });
    assert.equal(init.result.protocolVersion, '2025-06-18');
    bridge.send({ jsonrpc: '2.0', method: 'notifications/initialized' });

    const sent = structured(await bridge.call(1, 'send_message', { text: 'Olá do Node', title: 'Teste' }));
    assert.equal(sent.delivered, true);
    assert.equal(typeof sent.id, 'number');
    assert.deepEqual(
      structured(await bridge.call(2, 'set_status', { text: 'a trabalhar…', progress: 42 })),
      { ok: true },
    );

    const read = structured(await bridge.call(3, 'get_user_messages', {}));
    const kinds = read.messages.map((message) => message.kind);
    assert.deepEqual(kinds, ['message', 'page'], JSON.stringify(read));
    assert.equal(read.messages[0].text, 'mensagem do usuário');
    assert.equal(read.messages[1].url, 'https://example.com/partilhada');
    assert.match(read.messages[0].at, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/);
    const again = structured(await bridge.call(4, 'get_user_messages', { since_id: read.last_id }));
    assert.deepEqual(again.messages, []);

    assert.deepEqual(
      structured(await bridge.call(5, 'ask_user', { question: 'responda Sim', options: ['Sim', 'Não'] })),
      { answer: 'Sim', via: 'button' },
    );
    assert.deepEqual(
      structured(await bridge.call(6, 'ask_user', { question: 'responda por texto' })),
      { answer: 'olá do teste', via: 'text' },
    );
    assert.deepEqual(
      structured(
        await bridge.call(7, 'ask_user', { question: 'deixe expirar', timeout_seconds: 10 }, {
          _meta: { progressToken: 'p7' },
        }),
      ),
      { timed_out: true },
    );

    // O hub valida outra vez: nada invalido chega ao usuario.
    toolError(await bridge.call(8, 'send_message', { text: '\u0007\u0008' }));

    const exit = await bridge.close();
    assert.equal(exit.code, 0, `codigo de saida ${exit.code}\n${bridge.stderr}`);
    bridge.assertPure();
  } finally {
    bridge.child.kill();
  }
}

try {
  const exe = resolveExe();
  if (withHub) {
    await withTheHub(exe);
  } else {
    await withoutHub(exe);
  }
  console.log(`agents-mcp: ok (${withHub ? 'com hub' : 'sem hub'}; ${exe})`);
} catch (error) {
  console.error(`agents-mcp: FALHOU\n${error?.stack ?? error}`);
  process.exit(1);
}
