# SPEC-0108 — Canal seguro página→nativo (IPC do WebView2)

**Status:** Parcial — transporte IPC implementado e coberto por testes no PR #24; revisão adversarial independente e release 2.1 pendentes  
**Alvo:** NeuralIA 2.1  
**Substitui:** o transporte por navegação `neuralia:` descrito em SPEC-0005 §"Native bridge boundary" (o modelo de confiança mantém-se; muda o transporte)  
**Depende de:** SPEC-0005, SPEC-0006, SPEC-0015

## 1. Problema

Na baseline 2.0.1 uma página pedia uma ação de interface navegando para
`neuralia:<ação>?cap=<token>`; o handler de navegação nativo intercepta, valida o
token por WebView e traduz num `UserEvent`. O token vive no closure dos scripts
injetados, é gerado por `BCryptGenRandom`, comparado em tempo constante e
transportado por funções nativas capturadas no *document-created*.

Nada disso chega. O Chromium expõe a **Navigation API**: qualquer script da
página pode registar

```js
navigation.addEventListener('navigate', e => exfiltrate(e.destination.url));
```

e ler a URL completa — token incluído — de **cada** ação que o utilizador
dispara. O vazamento é por desenho do browser, não por erro nosso: um canal
que passa pela barra de navegação é observável pela página. Com o token na mão,
a página forja `neuralia:clearhistory`, `neuralia:devtools`, `neuralia:split`
com qualquer URL, etc.

Alternativa rejeitada: `delete window.navigation` no *document-created*. Parte
SPAs que usem a API e é contornável (um `iframe` `about:blank` criado antes de o
nosso script correr nele, `Object.getOwnPropertyDescriptor` no protótipo,
`document.open()`). Um remendo que se contorna não é uma fronteira.

## 2. Decisão

O transporte passa a ser a **mensagem do WebView2**
(`window.chrome.webview.postMessage`, recebida no nativo pelo
`WebViewBuilder::with_ipc_handler` do wry 0.57, que entrega
`Request<String>` com o corpo e a URL do frame emissor).

Propriedades que motivam a escolha:

- **Fora da Navigation API.** A mensagem não é navegação e, portanto, não
  aparece em `navigation.navigate`/`destination.url`; o transporte capturado
  no *document-created* não põe a capability numa URL observável.
- **Não cria superfície nova.** `chrome.webview` existe em todo o WebView2, com
  ou sem handler; sem handler as mensagens são descartadas. A página já podia
  chamar `postMessage`; continua a poder — e continua sem token.
- **O modelo de confiança é o mesmo.** Token por WebView, no closure, gerado
  pelo CSPRNG do SO, comparado em tempo constante, transportado por uma
  referência capturada no *document-created*. Só muda o meio.

A frase de SPEC-0005 "there is no IPC object" passa a
"there is no IPC with ambient authority": existe um canal de mensagens,
limitado a ações de interface, e cada mensagem tem de provar posse do token.

## 3. Protocolo

### 3.1 Mensagem

Uma string JSON, sempre com estes quatro campos e nenhum outro:

```json
{ "v": 1, "cap": "<32 hex>", "action": "<nome>", "args": { } }
```

- `v` — versão do protocolo; mensagens com outra versão são ignoradas.
- `cap` — o token do WebView (32 hex). Comparação em tempo constante.
- `action` — um nome da lista fechada de SPEC-0005 (`home`, `back`, `restore`,
  `autoscroll`, `zoomin`, `zoomout`, `zoomreset`, `reload`, `print`, `omnibox`,
  `history`, `clearhistory`, `fullscreen`, `devtools`, `viewsource`, `newtab`,
  `expand`, `shortcut-expand`, `minimize`, `split`, `split-close`, `split-expand`, `palette`,
  `gmail-state`, `research-answer`, `agent-observation`). Nome fora da lista →
  ignorado.
- `args` — objeto com os parâmetros exatos da ação (`col`, `url`, `count`,
  `sender`, `subject`, `key`, `text`, `data`). Campos extras ou tipos errados
  são rejeitados; índices são validados contra `COMPARATOR_COLUMNS`; `url`
  passa por `validate_web_url` e rejeita alvos locais/privados/special óbvios
  antes de DNS; a camada IPC não afirma filtragem DNS pré-conexão do WebView2; strings são
  recusadas acima dos limites definidos (180/2048 chars e payload do observer
  limitado antes da serialização).

Tamanho máximo da mensagem: 8 KiB. Acima disso é descartada sem parse.

### 3.2 Lado da página

Cada script injetado captura, no *document-created*, antes de qualquer script
da página. No Windows, onde initialization scripts também alcançam child
frames, o script aborta primeiro com `if (window.top !== window) return;`; a
capability só é declarada/ usada no documento principal:

```js
const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
const capability = '__NEURALIA_CAP__';
function act(action, args) {
  post(JSON.stringify({ v: 1, cap: capability, action, args: args || {} }));
}
```

`JSON.stringify` também é capturado (`const stringify = JSON.stringify`).
Nenhum script usa `window.ipc` (global do wry, envenenável pela página antes
de ser lido tarde) nem `location.href` para ações.

Todos os handlers que chamam `act` continuam a exigir `event.isTrusted`.

### 3.3 Lado nativo

- Um `with_ipc_handler` por WebView (external, comparador ×3, split, Gmail),
  cada um com o seu token no closure, exatamente como hoje os handlers de
  navegação.
- O handler: rejeita corpo > 8 KiB; faz parse; rejeita `v != 1`; compara `cap`
  em tempo constante; mapeia `action`/`args` para o `UserEvent` correspondente
  com as mesmas validações de hoje; qualquer falha é silenciosa (sem log de
  conteúdo da página).
- A URL associada ao `Request<String>` não é autoridade e não participa da
  decisão: o token no closure do WebView é o que conta.
- Os handlers de navegação deixam de aceitar `neuralia:` nas superfícies web
  (external, comparador, split, Gmail e PDF). Um `neuralia:` vindo dessas
  superfícies é negado silenciosamente, sem logar conteúdo da página.

### 3.4 Exceção: Reader e visualizador de PDF

O Reader é HTML nosso, servido com `script-src 'none'` para conteúdo da
página: não executa JavaScript do site. Os seus botões continuam a ser
`href="neuralia:home"` e
`href="neuralia:web?url=…"` **sem token**, aceites apenas pelo handler de
navegação do Reader (que já rejeita tudo o resto). Os atalhos injetados pelo
host no Reader usam IPC, sem mudar a exceção dos links internos. O visualizador
de PDF (`neuralia-pdf.localhost`, PDF.js nosso) usa o canal de mensagens como
as outras superfícies, porque tem script próprio.

## 4. Impacto nas specs existentes

| Spec | Frase de hoje | Passa a |
|---|---|---|
| SPEC-0005 §Native bridge boundary | "There is no IPC object. … The only page-to-native channel is the internal `neuralia:` scheme" | "There is no IPC with ambient authority. The page-to-native channel is a WebView2 message carrying a per-WebView capability; the `neuralia:` scheme survives only inside the script-free Reader" |
| SPEC-0005 (token) | "carried using native functions … so poisoning globals neither steals the token nor corrupts the URL" | "carried by `chrome.webview.postMessage` captured at document-created; the Navigation API cannot observe it" |
| SPEC-0006 §Full Web | "Top-level navigation … plus the internal intercepted `neuralia:` actions" | "`neuralia:` is accepted only from the Reader; web surfaces use the message channel" |
| SPEC-0015 §Controls | linha do capability | acrescentar "message transport not observable by page script (Navigation API leak closed)" |
| SECURITY.md | parágrafo do canal | reescrever conforme acima |

Nenhuma destas frases muda antes de o código e os testes existirem (AGENTS.md §3).

## 5. Critérios de aceitação

A SPEC-0108 só passa a "Implementada" quando, no CI:

1. Teste unitário do parser de mensagens: rejeita corpo > 8 KiB, `v != 1`,
   `cap` ausente/errado/com comprimento diferente, `action` fora da lista,
   `args` com tipos errados; aceita cada uma das 26 ações com `args` válidos.
2. Teste: nenhuma constante de script injetado contém `location.href = 'neuralia:`
   nem `neuralia:` + `?cap=` — exceto no HTML do Reader (`render.rs`), que não
   pode conter `cap` de todo.
3. Teste: todos os scripts que enviam ações abortam em child frames **antes**
   de usar a capability e capturam `chrome.webview.postMessage` e
   `JSON.stringify` no topo do documento principal (padrão verificado por
   `injected_scripts_capture_globals_before_the_page_runs` e testes
   `spec_0108_*`).
4. Teste: os handlers de navegação das superfícies web recusam `neuralia:`.
5. Teste de comparação em tempo constante (já existe; manter).
6. Revisão adversarial independente (AGENTS.md §4 e §7): tentar, com script de
   página, (a) ler o token, (b) forjar uma ação sem token, (c) disparar uma ação
   com evento sintético, (d) usar um `iframe` para contornar a captura. As
   quatro têm de falhar.

## 6. Plano de implementação

1. `fn parse_ipc_message(body: &str, expected_cap: &str) -> Option<IpcAction>`
   pura, em `windows_app.rs` (ou módulo novo `ipc.rs` no `neural-app`), com os
   testes do §5.1.
2. Trocar `act()` e todos os `location.href = 'neuralia:…'` dos scripts
   injetados pelo `post(...)` do §3.2, um script de cada vez, mantendo
   `isTrusted`.
3. Acrescentar `with_ipc_handler` aos builders external/comparator/split/Gmail/
   PDF, reutilizando o token e o `proxy` que hoje vão para o handler de
   navegação.
4. Fechar `neuralia:` nos handlers de navegação dessas superfícies.
5. Atualizar as specs conforme §4, no mesmo PR em que os testes passam.
6. Revisão adversarial; gate; release **2.1.0** (mudança de canal interno —
   *minor*, não *patch*).

### Estado do PR #24

Os passos 1–5 estão implementados no PR #24. O passo 6 permanece pendente;
por isso esta spec não usa o status **Implementada**.

## 7. O que não muda

- Lista de ações e as suas validações.
- Palette nativa (SPEC-0005): a página só pede a abertura.
- `isTrusted` obrigatório.
- Token por WebView, CSPRNG, tempo constante, closure.
- Reader sem script e sem token.
