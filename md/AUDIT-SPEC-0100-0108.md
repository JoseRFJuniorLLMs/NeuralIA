# Auditoria dos gates SPEC-0100 a SPEC-0108

**Estado auditado:** NeuralIA `main` após os PRs #42–#45.  
**Regra:** um teste que cita uma SPEC só conta como gate de produto quando consegue ficar vermelho ao quebrar o caminho que realmente embarca.

## Matriz

| SPEC | Caminho que embarca | Gate atual | Classificação | Lacuna que continua aberta |
|---|---|---|---|---|
| 0100 | `MemoryWorker` → `MemoryStore` em `neural-app` | core + `spec_product_wiring.rs` + testes de custo/forget/recall | **núcleo real + wiring** | comportamento assíncrono da fila e UX de erro/saturação ainda não têm E2E |
| 0101 | `ResearchSession` usado diretamente por `windows_app.rs` | core + wiring de capture/compare/synthesize/export | **núcleo real + wiring** | falta E2E WebView → resposta capturada → sessão persistida → export |
| 0102 | `hashed_embedding` no store; lifecycle de model packs não embarca | core + gate que proíbe afirmar que `ModelPackManager` está ligado ao app | **parcial e honesto** | install/uninstall, lazy load, fallback de backend e benchmark de produto |
| 0103 | `semanticAnchors()` nos scripts JS de Reader/Split/Comparator | gate de produto mira os scripts que embarcam | **caminho real, cobertura estrutural** | falta E2E por fornecedor e medição de jank/performance |
| 0104 | `AgentPermissionPolicy` usado pelo agente embarcado | policy tests + fixtures adversariais + wiring do produto | **núcleo real + wiring** | revisão adversarial independente e prova de UI/consentimento ponta a ponta |
| 0105 | `handle_agent_observation` → `decide_agent_step` | testes comportamentais sobre `decide_agent_step` | **caminho real** | decisão arquitetural sobre o destino do `neural-core::AgentRuntime` |
| 0106 | composição de memória/pesquisa/timeline/IPC/agente | roadmap + wiring; não é um único runtime gate | **roadmap, não feature** | manter cada fase presa ao seu gate comportamental próprio |
| 0107 | store SQLite/FTS5 derivado + tombstones/rebuild/retrieval | testes operacionais de Phase 1 + proveniência vendorizada | **Fase 1 parcial** | integração/UX/performance completas e critérios restantes |
| 0108 | `neural-app/src/ipc.rs` + scripts/builders WebView2 | parser, 25 ações, capability, child-frame guard, navegação remota | **melhor gate do grupo** | revisão adversarial independente/release gate |

## Achados que mudaram o desenho dos gates

### SPEC-0103

O antigo caso em `neural-core/tests/spec_010x_acceptance.rs` exercitava
`semantic_anchors_html`. O browser, porém, desenha a timeline com
`semanticAnchors()` em JavaScript. O helper Rust é referência útil, não prova
o rail que o utilizador recebe. A SPEC passou a declarar esse limite e o gate de
produto passou a mirar os scripts embarcados.

### SPEC-0105

O antigo gate executava `AgentRuntime` com planner/executor mockados. Esse
runtime não era chamado por `neural-app`. O produto usa
`handle_agent_observation` e agora concentra a decisão testável em
`decide_agent_step`. O gate comportamental tem de ser sabotável: retirar
`policy.evaluate`, o orçamento ou a confirmação deve fazê-lo ficar vermelho.

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
