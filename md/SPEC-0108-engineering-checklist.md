# SPEC-0108 — checklist de engenharia antes da implementação

**Base:** decisão arquitetural registrada em `md/SPEC-0108-secure-webview-ipc-channel.md`.  
**Estado:** preparação somente. Não implementar antes da ordem definida em `AGENTS.md`.

## Objetivo

Migrar ações página→nativo de URLs `neuralia:*?cap=...` para WebView2 messaging sem alterar o modelo de autoridade:

- token por WebView;
- CSPRNG do SO;
- comparação em tempo constante;
- closure document-created;
- lista fechada de ações;
- `isTrusted`;
- Reader continua exceção sem token.

O problema que a migração resolve é específico: a Navigation API consegue observar URLs `neuralia:` e portanto o token.

## API WRY verificada

O workspace usa:

```toml
wry = "0.57"
```

O upstream WRY expõe:

```rust
WebViewBuilder::with_ipc_handler<F>(handler: F)
where
    F: Fn(Request<String>) + 'static
```

O `Request<String>` fornece corpo e URI do frame emissor.

Não usar `window.ipc` como autoridade. O transporte decidido é a referência capturada de:

```js
window.chrome.webview.postMessage
```

## Estrutura recomendada

Evitar colocar mais parsing dentro do já enorme `windows_app.rs`.

Proposta:

```text
crates/neural-app/src/
  ipc.rs
  windows_app.rs
```

`ipc.rs` deve ser lógica pura e testável:

```rust
const IPC_MAX_BYTES: usize = 8 * 1024;

struct IpcEnvelope {
    v: u8,
    cap: String,
    action: String,
    args: serde_json::Value,
}

enum IpcAction {
    Home,
    Back,
    Restore,
    AutoScroll,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Reload,
    Print,
    Omnibox,
    History,
    ClearHistory,
    Fullscreen,
    DevTools,
    ViewSource,
    NewTab { col: usize },
    Expand { col: usize },
    Minimize { col: usize },
    Split { col: usize, url: String },
    SplitClose,
    SplitExpand,
    Palette { col: usize },
    GmailState { count: u32, sender: String, subject: String, key: String },
    AgentObservation { payload: ... },
    ResearchAnswer { col: usize, text: String },
}
```

A lista final deve ser derivada do inventário real dos `neuralia:` handlers antes da troca. Não confiar em uma lista manual antiga.

## Dependências

`serde` e `serde_json` já existem em `[workspace.dependencies]`.

Adicionar ao `neural-app` somente se o parser for para esse crate:

```toml
serde.workspace = true
serde_json.workspace = true
```

Nenhum runtime async adicional.

## Parser

Assinatura sugerida:

```rust
fn parse_ipc_message(
    body: &str,
    expected_cap: &str,
    context: IpcContext,
) -> Result<IpcAction, IpcReject>
```

Ordem de validação:

1. bytes <= 8192;
2. JSON válido;
3. `v == 1`;
4. cap exatamente 32 ASCII hex;
5. constant-time compare contra expected;
6. action em enum fechada;
7. args com shape exato;
8. índices < `COMPARATOR_COLUMNS`;
9. strings com limites;
10. URL validada;
11. política local/private da superfície;
12. produzir `UserEvent`.

Não desserializar diretamente para `UserEvent`.

## Constant-time

Reutilizar a função atual de comparação, ou mover para helper compartilhado.

Teste deve provar que:

- comprimento diferente falha;
- byte inicial errado percorre a comparação completa;
- byte final errado também falha;
- mixed-case token é rejeitado se protocolo declarar lowercase hex.

## JS document-created

Padrão obrigatório por WebView:

```js
const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
const stringify = JSON.stringify;
const capability = '__NEURALIA_CAP__';

function act(action, args) {
  post(stringify({
    v: 1,
    cap: capability,
    action,
    args: args || {}
  }));
}
```

Capturar também as primitivas necessárias antes de scripts da página.

Não usar depois:

- `window.chrome.webview.postMessage(...)` via lookup tardio;
- `JSON.stringify(...)` via lookup tardio;
- `window.location.href = 'neuralia:...'`;
- `location.assign('neuralia:...')`.

## Inventário real de ações no candidato 2.0.1

Varredura de `feat/astra-finish-audit-2.0@3ec01dd` encontrou **25 ações/canais reais** combinando:

- chamadas `act('...')`;
- handlers `starts_with("neuralia:...")`;
- match nativo `name => UserEvent`.

Lista:

```text
agent-observation
autoscroll
back
clearhistory
devtools
expand
fullscreen
gmail-state
history
home
minimize
newtab
omnibox
palette
print
reload
research-answer
restore
split
split-close
split-expand
viewsource
zoomin
zoomout
zoomreset
```

`auto-submit` apareceu apenas como chave de `sessionStorage`, não como canal.
`inventado` apareceu apenas em teste negativo.

**Consequência:** a SPEC-0108 atual fala em "23 ações". Esse número não deve ser
copiado para código. O inventário real do SHA de implementação deve ser a fonte,
e a spec deve ser corrigida no mesmo commit em que o transporte novo ficar
completo.

Os canais `agent-observation`, `research-answer` e `gmail-state` carregam
dados, não apenas comandos de UI, e precisam de schemas próprios em vez de
`args: Value` sem validação.

## Inventário de builders a migrar

Revisar no SHA de implementação:

- external webview;
- comparator Gemini;
- comparator ChatGPT;
- comparator Claude;
- split view;
- private split;
- Gmail monitor;
- PDF viewer;
- agent observation channel;
- research answer channel.

Reader fica separado.

Cada WebView deve ter **seu próprio token/closure**.

## Navigation handlers depois da migração

Para superfícies web:

```text
neuralia: -> deny
```

Não manter compatibilidade silenciosa com o transporte antigo, senão o leak continua existindo.

Reader:

- `neuralia:home`;
- `neuralia:web?url=...`;

sem cap e somente no handler Reader.

## Agent observation

Hoje `AGENT_OBSERVER_SCRIPT` envia a observação via:

```text
neuralia:agent-observation?data=...&cap=...
```

Esse canal também precisa migrar.

Sugestão:

```json
{
  "v": 1,
  "cap": "...",
  "action": "agent-observation",
  "args": {
    "generation": 7,
    "url": "...",
    "title": "...",
    "text": "...",
    "elements": [...]
  }
}
```

Evitar continuar serializando payload tab/newline e depois URL-encoding. JSON já é o envelope.

**Importante:** a sanitização e os limites do observer continuam necessários; IPC não resolve prompt injection.

## Research answer / Gmail state

Mesma migração.

Antes de portar, registrar limites atuais:

- resposta de pesquisa;
- sender/subject/key;
- contagem;
- URLs;
- labels.

O parser nativo deve impor os limites, não confiar no `slice()` JavaScript.

## Testes puros

### Envelope

- corpo vazio;
- JSON inválido;
- >8 KiB;
- v0/v2;
- cap ausente;
- cap curto/longo;
- cap não-hex;
- cap errado;
- unknown action;
- args ausentes/incorretos;
- campos extras tolerados ou rejeitados conforme decisão explícita.

### Cada action

Uma tabela deve provar:

- payload mínimo aceito;
- tipos errados rejeitados;
- bounds de índices;
- bounds de strings;
- URLs inseguras rejeitadas.

### Regressão do leak

Source test deve falhar se scripts injetados contiverem:

```text
neuralia:
?cap=
location.href
location.assign
```

quando usados como transporte.

Exceção explicitamente confinada ao Reader.

### Poisoning

Fixture JS deve redefinir, depois do document-created:

- `window.chrome`;
- `window.chrome.webview`;
- `JSON.stringify`;
- `encodeURIComponent`;
- `EventTarget.prototype.addEventListener`.

A closure capturada continua operando.

### Synthetic events

Todos os handlers de input continuam exigindo `isTrusted`.

## Teste adversarial manual do revisor

Em uma página hostil:

1. listener da Navigation API;
2. monkeypatch de globals;
3. iframe same-origin;
4. iframe cross-origin;
5. synthetic click/key;
6. chamada direta a `chrome.webview.postMessage` sem cap;
7. brute payload >8 KiB;
8. replay de mensagem capturada de outra WebView;
9. action válida com args adulterados.

Nenhuma deve ganhar autoridade.

## Replay / escopo de token

Token deve morrer junto com a WebView.

Não criar token global de aplicação.

Mensagem de uma coluna não pode controlar outra coluna se os closures/tokens forem distintos.

Adicionar teste estrutural que conte criação de capabilities por builder.

## Logging

Rejeições IPC não devem logar body bruto.

Permitido:

```text
ipc reject: bad-cap action=split source=https://...
```

Proibido:

```text
body={...sender,subject,text,password...}
```

## Sequência de commits sugerida

1. `ipc.rs` + parser + testes, sem wiring;
2. helper JS capturado + testes source-level;
3. migrate external/split;
4. migrate comparator;
5. migrate Gmail/research answer;
6. migrate agent observation;
7. migrate PDF viewer;
8. bloquear `neuralia:` em superfícies remotas;
9. atualizar docs/specs somente agora;
10. adversarial review;
11. gate completo;
12. Claude faz bump 2.1.0.

## Não fazer

- não misturar SPEC-0107 no mesmo PR;
- não trocar storage;
- não refatorar o agente junto;
- não manter dois transportes ativos "temporariamente" no merge final;
- não mover token para DOM, dataset ou global JS;
- não usar origem do request como substituto do token;
- não considerar IPC uma defesa contra prompt injection.

## Critério de pronto para implementação

Antes do primeiro commit de código:

- PR #14 integrado/release 2.0.1 resolvida;
- relatório 0104/0105 triado;
- inventário exato de todas as ações `neuralia:` congelado;
- parser API revisada;
- lista de builders confirmada;
- reviewer independente disponível para o SHA final.
