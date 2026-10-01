# Auditoria dos gates SPEC-0100 a SPEC-0108

**Estado auditado:** NeuralIA `main` em `cae1c27`, após o PR #216 (2026-09-30).  
**Regra:** um teste que cita uma SPEC só conta como gate de produto quando consegue ficar vermelho ao quebrar o caminho que realmente embarca.

## Matriz

| SPEC | Caminho que embarca | Gate atual | Classificação | Lacuna que continua aberta |
|---|---|---|---|---|
| 0100 | `MemoryWorker` → `MemoryStore` em `neural-app` | core + wiring + gates de saturação sobre a fila bounded real, com eventos/UX observáveis | **caminho real + comportamento de saturação** | `capture` segue best-effort por desenho; ampliar stress/performance além dos gates bounded atuais |
| 0101 | `ResearchSession` + leitura das respostas nas WebViews + persistência/export | core + wiring + E2E do PR #216 que captura resposta real da WebView, persiste a sessão e exporta mantendo proveniência; modo privado também é exercitado | **caminho real + E2E de persistência/export** | ampliar fixtures contra mudanças de DOM/provedores e continuar a fase de Consenso 2.5 sem regredir a baseline |
| 0102 | semântica determinística/hashed embeddings no produto; lifecycle de model packs ainda não está em `main` | core valida manifest/hash/licença/benchmark/fallback; `main` ainda guarda a ausência de wiring de produto | **parcial e honesto** | #220/#221: lifecycle lazy de produto; primeiro backend medido continua separado e não pode ser simulado por benchmark fictício |
| 0103 | `semanticAnchors()` nos scripts JS de Reader/Split/Comparator | fixtures por fornecedor + gates sobre o JS que embarca + rácio de performance e sabotagem | **caminho real + comportamento/performance** | acompanhar drift de DOM e jank sem voltar a usar o parser Rust de referência como substituto do JS embarcado |
| 0104 | `AgentPermissionPolicy` no caminho embarcado | policy tests + fixtures adversariais + wiring do produto + sabotagem dos gates críticos | **núcleo real + wiring** | revisão adversarial independente e prova ponta a ponta das confirmações/consentimento |
| 0105 | `handle_agent_observation` → `decide_agent_step` | testes comportamentais e sabotagem sobre a decisão que embarca | **caminho real** | revisão adversarial independente; decidir o destino do `neural-core::AgentRuntime`, que permanece harness de referência |
| 0106 | composição de memória/pesquisa/timeline/IPC/agente | roadmap + wiring; não é um único runtime gate | **roadmap, não feature** | manter cada fase presa ao seu gate comportamental próprio |
| 0107 | store SQLite/FTS5 derivado + retrieval/rerank + tombstones/rebuild | testes operacionais de Fase 1, escala e proveniência vendorizada | **Fase 1 parcial** | integração/UX/performance completas; critérios de startup/idle com backend real permanecem dependentes das fases posteriores |
| 0108 | `neural-app/src/ipc.rs` + scripts/builders WebView2 | parser portátil, schema fechado, capability, limite de payload, child-frame guard e gates do produto | **caminho real + product-tested** | revisão adversarial independente/release gate; não há mais pendência de “release 2.1” como estado atual |

## Achados que mudaram o desenho dos gates

### SPEC-0101

A lacuna antiga “WebView → resposta capturada → sessão persistida → export” foi fechada pelo PR #216. O gate E2E conduz a superfície WebView, observa a resposta, persiste pelo caminho compartilhado da sessão e verifica o export com proveniência. O modo privado tem um E2E separado para impedir persistência. A auditoria anterior continuava descrevendo essa entrega como ausente e, portanto, estava desatualizada.

### SPEC-0103

O antigo caso em `neural-core/tests/spec_010x_acceptance.rs` exercitava
`semantic_anchors_html`. O browser, porém, desenha a timeline com
`semanticAnchors()` em JavaScript. O helper Rust é referência útil, não prova
o rail que o utilizador recebe. A SPEC passou a declarar esse limite e o gate de
produto passou a mirar os scripts embarcados.

### SPEC-0105

O antigo gate executava `AgentRuntime` com planner/executor mockados. Esse
runtime não era chamado por `neural-app`. O produto usa
`handle_agent_observation` e concentra a decisão testável em
`decide_agent_step`. O gate comportamental tem de ser sabotável: retirar
`policy.evaluate`, o orçamento ou a confirmação deve fazê-lo ficar vermelho.

### SPEC-0108

O canal IPC já não é uma entrega “pendente da release 2.1”. Ele está no produto,
é compilado/testado também fora do runner Windows quando a lógica é portátil e
tem gates que prendem o conjunto fechado de ações à documentação. O que permanece
aberto é a revisão adversarial independente/release gate, não a existência do
transporte.

### SPEC-0106

Construir `MemoryStore`, uma implementação de inteligência local e uma policy
no mesmo teste não prova um browser agentic. A SPEC-0106 é um roadmap de
composição; os seus requisitos são provados pelos gates das SPECs que compõem o
produto e pelo wiring entre elas.

## Regra para novos gates

Um gate novo só deve receber o nome de uma SPEC quando satisfizer todos estes
pontos:

1. identifica a função, script, parser ou fluxo que realmente embarca;
2. quebra deliberadamente o comportamento protegido e fica vermelho;
3. não usa uma biblioteca paralela como substituto do caminho do produto;
4. separa teste estrutural de wiring de teste comportamental;
5. não promove a SPEC para `Implemented` quando só uma fase está coberta;
6. registra explicitamente o que o teste **não** prova.

Testes de wiring por texto são úteis para detectar desconexões entre
subsistemas, mas não demonstram comportamento. Quando a regra está presa dentro
de UI/WebView, a decisão deve ser extraída para uma função pequena e testável
(`decide_agent_step` e `route_input` são os modelos), sem transformar o
teste em uma segunda implementação do produto.
