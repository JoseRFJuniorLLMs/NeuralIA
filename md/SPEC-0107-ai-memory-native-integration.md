# SPEC-0107 — Integração Nativa do ai-memory no NeuralIA

**Status:** Fase 1 parcial — proveniência, SQLite/FTS5, retrieval, tombstones e rebuild operacionais; integração/UX/performance completas pendentes  
**Alvo:** integração incremental pós-2.0; conclusão após os gates de produto/performance  
**Upstream:** akitaonrails/ai-memory  
**Licença upstream:** MIT  
**Estratégia:** importar e adaptar código selecionado; não criar dependência obrigatória do produto ai-memory

## 1. Objetivo

O NeuralIA deve aproveitar a arquitetura madura do ai-memory em vez de
reimplementar do zero armazenamento, busca híbrida, migrações, sanitização,
retenção e embeddings locais.

A meta não é copiar o produto inteiro.

A meta é trazer para dentro do NeuralIA apenas os subsistemas úteis para memória
de navegação e pesquisa:

- conhecimento durável em arquivos;
- SQLite como índice derivado;
- FTS5;
- entidades;
- relações/grafo;
- embeddings locais opcionais;
- busca híbrida com fusão de ranking;
- sanitização antes da persistência;
- consolidação determinística sem obrigar uso de LLM;
- migrações, purge, retenção e auditoria.

O NeuralIA continua sendo uma aplicação Rust nativa usando WRY e WebView do
sistema.

## 1.1. Estado verificado em 19/09/2026

O repositório já passou da Fase 0. Estão em `main` um índice SQLite derivado,
FTS5 para shortlist, rerank semântico local, tombstones duráveis, rebuild
atômico/validado e integração assíncrona pelo `MemoryWorker` do produto.

O teste `spec_0107_phase1_sqlite_retrieval_forget_and_rebuild_are_operational`
prende num mesmo fluxo captura, consulta híbrida, forget por domínio,
tombstone, bloqueio de recaptura e reconstrução do SQLite sem ressuscitar dados
esquecidos. Testes adicionais cobrem custo de captura, ausência de vestígios
após forget e recall PT/EN através do atalho FTS.

Isso **não** torna esta especificação inteira concluída. Permanecem, entre
outros, os gates de lifecycle de model packs, UX completa, benchmarks de
escala/idle e a decomposição arquitetural futura descrita abaixo. O status é
portanto Fase 1 parcial, não "Implementada".

## 2. Não objetivos

Esta integração não deve importar como requisito de produto:

- hooks de Claude Code;
- hooks de Codex;
- integrações de Cursor, Gemini CLI, Kimi, Devin e similares;
- managed coding workstreams;
- instaladores de MCP;
- servidor HTTP obrigatório;
- autenticação multiusuário do ai-memory;
- UI web administrativa do ai-memory;
- Docker como requisito;
- segundo executável obrigatório em background;
- CLI genérica de memória para agentes de programação.

MCP pode existir futuramente como interoperabilidade opcional.

## 3. Licença e proveniência

O ai-memory usa licença MIT.

Todo arquivo copiado ou substancialmente derivado deve preservar o aviso de
copyright e a licença aplicável.

O NeuralIA deve adicionar:

    third_party/
    └── ai-memory/
        ├── LICENSE
        ├── UPSTREAM.md
        └── PATCHES.md

UPSTREAM.md registra:

- URL do repositório upstream;
- commit SHA fixado;
- data da importação;
- caminhos importados;
- caminhos excluídos;
- crates NeuralIA de destino.

PATCHES.md registra diferenças arquiteturais e alterações feitas após a
importação.

Autoria upstream não pode ser apagada.

## 4. Estratégia de importação

É proibido copiar cegamente o repositório inteiro.

A integração tem duas camadas.

### 4.1 Snapshot upstream imutável

Manter uma cópia fixada apenas do material realmente usado como referência:

    third_party/ai-memory/upstream/

Esse snapshot só muda quando houver uma atualização upstream deliberada.

### 4.2 Crates nativos do NeuralIA

Código de produção deve morar em crates do próprio NeuralIA:

    crates/
    ├── neural-memory-core/
    ├── neural-memory-store/
    ├── neural-memory-wiki/
    ├── neural-memory-retrieval/
    ├── neural-memory-local/
    └── neural-memory-ingest/

Esses crates podem nascer de código derivado do ai-memory, mas suas APIs públicas
devem usar conceitos do domínio NeuralIA.

Não expor tipos como Project, CodingSession, Harness ou Workstream apenas
porque existiam no projeto doador.

## 5. Mapeamento de componentes upstream

### 5.1 Importar/adaptar fortemente

Áreas de maior valor:

    ai-memory-core
    ai-memory-store
    ai-memory-wiki
    ai-memory-llm / embedding
    ai-memory-consolidate / embed
    retrieval_tuning
    fts_query
    migrations
    sanitizer/privacy boundary
    local embeddings

Mapeamento de conceitos:

| ai-memory | NeuralIA |
|---|---|
| page/wiki page | MemoryDocument / KnowledgePage |
| project | ResearchSpace |
| session | ResearchSession |
| observation | NavigationObservation |
| entity | MemoryEntity |
| link neighbor | SourceRelation |
| embedding | MemoryEmbedding |
| page feedback | MemoryFeedback |
| consolidation | ResearchConsolidation |

### 5.2 Adaptar seletivamente

- retenção e decay;
- tombstones;
- reinforcement por acesso;
- page authority;
- evidência/proveniência;
- audit log;
- gerenciamento de modelos locais;
- backup/reindex.

### 5.3 Excluir do core embarcado

    ai-memory-cli
    ai-memory-mcp server surface
    ai-memory-hooks
    managed-workstreams
    client installers
    agent-specific bridges
    human-auth server stack
    web admin UI
    Docker deployment scripts

Esses componentes ficam apenas como referência arquitetural.

## 6. Arquitetura resultante

    Browser / Reader / Gemini / ChatGPT / Claude
                       │
                       ▼
                 Capture Boundary
                       │
                       ▼
                Sanitized Event
                       │
             ┌─────────┴─────────┐
             ▼                   ▼
      Durable Evidence      Knowledge Pages
                                 │
                                 ▼
                              Markdown
                          source of truth
                                 │
                                 ▼
                               SQLite
                    ┌─────┬──────┼──────┬─────┐
                    ▼     ▼      ▼      ▼     ▼
                   FTS  entities graph vectors metadata
                    └─────┴──────┴──────┴─────┘
                                 │
                                 ▼
                            rank fusion
                                 │
                                 ▼
                           semantic recall

SQLite deve ser índice derivado e mecanismo de busca. Conhecimento mantido deve
continuar recuperável a partir da camada durável em arquivos sempre que
praticável.

## 7. Modelo de dados NeuralIA

### 7.1 ResearchSpace

Container lógico de trabalho relacionado.

Exemplos:

- pesquisa HeraclitusDB;
- viagem;
- otimização Rust;
- pesquisa jurídica.

### 7.2 ResearchSession

Uma linha coerente de investigação.

Campos mínimos:

- session ID;
- título;
- intenção/pergunta inicial;
- timestamps;
- provedores envolvidos;
- ResearchSpace opcional.

### 7.3 NavigationObservation

Evento limitado de navegação ou pesquisa:

- consulta submetida;
- página aberta;
- fonte aberta a partir de uma IA;
- Reader extraído;
- PDF aberto;
- resposta de provedor capturada;
- nota criada;
- síntese gerada.

Observações não viram automaticamente conhecimento permanente.

### 7.4 KnowledgePage

Memória consolidada durável.

Tipos sugeridos:

    concept
    decision
    source
    comparison
    procedure
    note
    session-summary
    gotcha
    research-result

### 7.5 SourceRelation

Relações tipadas:

    provider_answer -> cited_source
    source -> supports_claim
    source -> contradicts_claim
    session -> contains_source
    document -> related_to
    decision -> derived_from

## 8. Fronteira de captura e privacidade

Preservar a ideia de fronteira tipada do ai-memory.

Conteúdo remoto deve ser sanitizado antes da persistência.

Nunca persistir:

- cookies;
- Authorization headers;
- senhas;
- campos de pagamento;
- session tokens;
- segredos do perfil do navegador;
- páginas privadas/incógnitas;
- domínios/documentos explicitamente excluídos pelo usuário.

Modo privado deve bloquear captura antes de qualquer gravação.

Gravar primeiro e apagar depois não satisfaz este requisito.

## 9. Fonte de verdade em arquivos

Layout recomendado:

    <profile>/memory/
    ├── wiki/
    │   ├── concepts/
    │   ├── decisions/
    │   ├── sessions/
    │   ├── sources/
    │   ├── comparisons/
    │   └── gotchas/
    ├── evidence/
    ├── db/
    │   └── neural-memory.sqlite
    ├── models/
    └── logs/

A primeira implementação pode adiar Git automático se isso atrasar demais o
núcleo, mas conhecimento durável deve permanecer legível fora do banco.

Versionamento Git pode ser ativado depois da estabilização do schema.

## 10. SQLite

O índice derivado inicial deve conter:

- documentos/páginas;
- FTS5;
- observations;
- sessions;
- entities;
- entity links;
- source relations;
- embeddings;
- feedback/reinforcement;
- versão de migração;
- tombstones;
- audit log de operações destrutivas.

Usar WAL.

Se a arquitetura single-writer do upstream puder ser portada de forma limpa,
ela deve ser preservada.

## 11. Retrieval híbrido

NeuralIA não deve usar busca somente vetorial.

Streams mínimos:

1. FTS5 lexical;
2. entity match;
3. graph-neighbor;
4. vector similarity opcional.

Pipeline:

    FTS5 -----------┐
    entities -------┤
    graph ----------┼--> RRF/fusão --> authority adjustment --> hits
    vectors --------┘

Embeddings são opcionais.

Sem embedder, memória continua funcionando por FTS, entidades e grafo.

## 12. Embeddings locais

Primeiro baseline pode avaliar o mesmo modelo usado pelo ai-memory:

- all-MiniLM-L6-v2;
- 384 dimensões;
- inferência local;
- sem API key;
- sem data egress.

Porém o NeuralIA deve comparar modelos multilíngues porque o corpus real inclui:

- português;
- inglês;
- código;
- artigos técnicos;
- PDFs;
- possivelmente documentos jurídicos.

A interface Embedder precisa ser independente do modelo.

Modelo padrão só é escolhido depois de benchmark.

## 13. Carregamento de modelo

Embeddings locais são lazy-loaded.

Na Home nativa:

- nenhum modelo carregado;
- nenhum download automático apenas por abrir o NeuralIA;
- nenhuma indexação disputando first paint.

Download de modelo exige consentimento claro ou ativação explícita do recurso.

Arquivos de modelo devem ter hashes fixados.

## 14. Consolidação

Adaptar a distinção do ai-memory entre eventos brutos e páginas mantidas.

Exemplo:

    20 NavigationObservations
              │
              ▼
       session summary
        ├── concept
        ├── decision
        ├── source note
        └── gotcha

Consolidação determinística deve existir.

LLM-assisted consolidation é opcional.

Falha de LLM não pode bloquear memória básica.

## 15. Memória compartilhada entre provedores

Gemini, ChatGPT e Claude podem contribuir para a mesma ResearchSession.

Proveniência é obrigatória.

    ResearchSession
    ├── Gemini observation
    │   └── source A
    ├── ChatGPT observation
    │   └── source B
    └── Claude observation
        └── source C

Busca futura pode combinar tudo, mas deve informar origem de cada resultado.

## 16. Ctrl+H

Ctrl+H deve evoluir de histórico de URLs para duas superfícies relacionadas.

### Histórico cronológico

Navegação exata por tempo.

### Semantic Recall

Exemplos:

    "onde estava aquele artigo sobre AVX-512?"

    "mostre o que pesquisei sobre WebView2 e prompt injection"

Semantic Recall usa retrieval híbrido.

Usuário deve poder apagar:

- um item;
- uma sessão;
- um domínio;
- um período;
- toda a memória.

## 17. Research Sessions

SPEC-0101 deve usar esta camada diretamente.

Sessões referenciam IDs de memória em vez de duplicar conteúdo.

Fechar uma aba visual não apaga pesquisa.

Apagar memória precisa atualizar referências de sessão com segurança.

## 18. Retenção e esquecimento

Adaptar decay do ai-memory só depois de definir semântica específica de browser.

Suportar:

- delete explícito;
- delete por domínio;
- delete por sessão;
- delete por intervalo de data;
- clear semantic index;
- rebuild index;
- TTL;
- importance/reinforcement opcional.

Delete deve ser verdadeiro.

Se vetor ou índice derivado continua existindo após excluir a fonte, a operação
não terminou.

## 19. Reindex e recuperação

Invariante central:

> o banco de busca derivado pode ser reconstruído.

Criar um Memory Doctor:

    NeuralIA Memory Doctor
    ├── verify schema
    ├── verify files
    ├── verify model/index compatibility
    ├── rebuild FTS
    ├── rebuild entities
    ├── rebuild graph
    └── rebuild embeddings

Corrupção do SQLite não pode significar perda irreversível do conhecimento
mantido.

### 19.1 Estado operacional do Doctor

`MemoryStore::doctor(rebuild)` já é API pública do core. O gate
`crates/neural-core/tests/spec_0107_doctor.rs` cria fontes duráveis reais,
injeta um documento JSON corrompido, remove o SQLite derivado e exige que
`doctor(true)`:

- conte separadamente fontes válidas e corrompidas;
- não promova o JSON corrompido a conhecimento;
- recrie o SQLite a partir das fontes válidas;
- devolva busca semântica/lexical funcional depois do rebuild.

O workflow sabota deliberadamente a chamada a `rebuild()` dentro do Doctor e
confirma vermelho antes de restaurar. Compatibilidade de modelo/embedding e
rebuild seletivo de entidades/grafo continuam critérios futuros; o Doctor atual
é de storage/index, não deve ser descrito como mais do que isso.

## 20. Atualização futura do upstream

NeuralIA não faz merge contínuo do ai-memory.

Cada atualização é explícita:

1. registrar SHA antigo;
2. registrar SHA novo;
3. diff apenas das áreas importadas;
4. classificar mudança:
   - segurança;
   - correção;
   - performance;
   - API;
   - irrelevante para browser;
5. portar seletivamente;
6. documentar em PATCHES.md;
7. executar testes NeuralIA.

Depois da divergência, nunca substituir os crates NeuralIA inteiros pelo upstream.

## 21. Interfaces sugeridas

    trait MemoryStore {
        fn capture(
            &self,
            observation: NavigationObservation
        ) -> Result<(), MemoryError>;

        fn query(
            &self,
            query: MemoryQuery
        ) -> Result<Vec<MemoryHit>, MemoryError>;

        fn forget(
            &self,
            scope: ForgetScope
        ) -> Result<ForgetReport, MemoryError>;
    }

    trait Embedder {
        fn identity(&self) -> EmbedderIdentity;

        fn embed(
            &self,
            input: &[String]
        ) -> Result<Vec<Vec<f32>>, MemoryError>;
    }

    trait Consolidator {
        fn consolidate(
            &self,
            session: &ResearchSession,
            observations: &[NavigationObservation],
        ) -> Result<Vec<KnowledgePage>, MemoryError>;
    }

Nenhum tipo WebView pertence a essas interfaces.

## 22. Performance

A integração é rejeitada se tornar o NeuralIA significativamente mais pesado em
idle.

Requisitos:

- Home independente do subsistema de memória;
- sem processo servidor obrigatório;
- sem modelo obrigatório em background;
- writes fora da UI thread;
- indexação limitada;
- erro de indexação não bloqueia navegação;
- startup de memória lazy quando possível;
- private mode sem carga de memória.

Benchmarks:

- 1k documentos;
- 10k documentos;
- 100k documentos;
- FTS-only;
- hybrid retrieval;
- reindex;
- embedding throughput;
- startup com memória ativada porém idle.

### 22.1 Harness e gate de escala

O harness `crates/neural-core/examples/memory_scale.rs` mede, pela API pública
real de `MemoryStore`, captura, 100 queries híbridas e rebuild. Sem argumentos,
ele executa exatamente os corpora de 1k, 10k e 100k documentos previstos acima:

`cargo run -p neural-core --example memory_scale --release -- 1000 10000 100000`

O CI não usa 100k em cada PR. Ele mantém um gate de regressão em
`crates/neural-core/tests/spec_0107_scale.rs`: dois stores independentes com
128 e 512 documentos, mesmas queries e rebuilds, medidos por melhor de múltiplas
rodadas. Para 4x entrada, query e rebuild devem custar menos de 8x.

O limite é uma **razão**, não um orçamento absoluto em milissegundos. Uma
sabotagem no workflow injeta atraso O(n²) proporcional ao número de documentos
no rebuild e precisa deixar o gate vermelho antes de restaurar o código. Assim o teste rejeita regressão
algorítmica sem confundir uma VM ocupada com código ruim.

Este gate fecha a infraestrutura de benchmark de escala, mas não conclui a
SPEC-0107: UX, entidades/grafo completos, embeddings opcionais de produto e
startup idle com backend real continuam pendentes.

## 23. Segurança

Código importado não vira confiável apenas por ser Rust.

Revisar:

- SQL boundaries;
- canonicalização de caminhos;
- symlinks;
- texto remoto malicioso;
- prompt injection armazenado;
- integridade de downloads de modelo;
- Markdown não confiável;
- exclusão de dados;
- vazamento do private mode;
- contaminação cross-session.

Conteúdo armazenado continua sendo dado não confiável.

## 24. Testes mínimos

### Storage

- migrations monotônicas;
- WAL recovery;
- reindex a partir dos arquivos;
- duplicate capture;
- corruption recovery.

### Retrieval

- lexical exact match;
- semantic paraphrase;
- entity retrieval;
- graph neighbor;
- hybrid fusion;
- fallback sem vetor;
- corpus multilíngue.

### Privacy

- incognito cria zero rows/files persistentes;
- password nunca entra no capture;
- cookies nunca entram no capture;
- clear history remove índices derivados;
- domínio excluído continua excluído.

### Proveniência

- provider de origem;
- URL;
- ResearchSession;
- source IDs que suportam síntese.

### Licença

CI deve verificar a presença de:

    third_party/ai-memory/LICENSE
    third_party/ai-memory/UPSTREAM.md

## 25. Fases de implementação

### Fase 0 — provenance import

- fixar commit upstream;
- adicionar licença;
- adicionar UPSTREAM.md;
- importar referência selecionada;
- nenhuma mudança de runtime.

### Fase 1 — storage

- novos crates NeuralIA;
- migrations/store;
- file-backed knowledge;
- FTS5.

### Fase 2 — retrieval

- entities;
- graph;
- rank fusion;
- explain/provenance.

### Fase 3 — local embeddings

- interface Embedder;
- backend local;
- vector retrieval;
- verificação de modelos.

### Fase 4 — browser capture

- NavigationObservation;
- Reader;
- provider/source attribution;
- private-mode exclusion.

### Fase 5 — consolidation

- rule-based;
- KnowledgePages;
- LLM opcional.

### Fase 6 — UI

- Ctrl+H semantic recall;
- filtros;
- Research Sessions;
- delete/reindex.

### Fase 7 — manutenção upstream

- relatório de diff;
- selective porting;
- patch lineage.

## 26. Critérios de aceite

A integração está concluída quando:

1. NeuralIA não exige servidor ai-memory separado;
2. código derivado possui atribuição MIT clara;
3. navegação normal gera memória sanitizada de forma assíncrona;
4. private mode gera zero memória persistente;
5. conhecimento sobrevive à reconstrução total do SQLite;
6. Ctrl+H encontra informação conceitualmente;
7. FTS/entity/graph funcionam sem embeddings;
8. embeddings locais acrescentam busca sem data egress;
9. resultados preservam provider/source/session;
10. delete remove fonte e índices derivados;
11. browser base continua leve em idle;
12. melhorias futuras do upstream podem ser portadas a partir de SHA documentado.

## 27. Decisão arquitetural

NeuralIA deve clonar e adaptar o motor útil do ai-memory em crates Rust próprios,
não depender do produto completo ai-memory em runtime.

Isso entrega:

- infraestrutura madura;
- memória local-first;
- retrieval híbrido;
- menos engenharia duplicada;
- conhecimento recuperável;
- caminho para histórico semântico;
- caminho para memória compartilhada entre Gemini, ChatGPT e Claude;

sem carregar:

- pressupostos de coding-agent;
- servidor HTTP permanente;
- integrações irrelevantes;
- complexidade operacional desnecessária.

A diferença é simples: aproveitar o motor do carro doador é sensato. Rebocar o
carro inteiro atrás do navegador seria apenas uma maneira criativa de aumentar
o consumo.
