# Auditoria adversarial preparatória — SPEC-0100 a SPEC-0103

**Data:** 19/09/2026  
**Base auditada:** `main@a5391f9`  
**Modo:** leitura de código; nenhuma alteração de runtime neste trabalho.

Este documento não muda status de spec. Ele registra o que precisa ser preservado,
corrigido ou provado quando a implementação da SPEC-0107 Fase 1 começar.

## Resumo executivo

Os testes existentes de 0100–0103 exercitam comportamento real, mas o código
atual ainda tem dívidas relevantes de correção e escala. O maior gargalo é a
memória semântica: a consulta relê e desserializa todos os JSONs, e o rebuild do
mirror SQLite reabre/configura o banco uma vez por documento.

Há também riscos de consistência que os testes atuais não cobrem: estado derivado
pode ficar desatualizado se um `MemoryDocument` for mutado antes de `capture`;
falhas SQLite podem ocorrer depois de JSON/Markdown já terem sido persistidos; e
o FTS atual ignora erros silenciosamente.

## SPEC-0100 — memória semântica

### A-0100-01 — query faz full scan do filesystem

**Severidade:** alta para escala.

`MemoryStore::query` chama `documents()`, que:

1. enumera todos os `documents/*.json`;
2. lê cada arquivo;
3. desserializa cada documento;
4. ordena todos por `last_seen_at`.

Depois a query ainda executa, para todos os documentos filtrados:

- tokenização lexical de título, corpo e entidades;
- embedding da query;
- cosine de 384 dimensões;
- matching de entidades;
- três ordenações RRF;
- expansão por relações;
- ordenação final.

A baseline já confirma a consequência: 100k documentos levam segundos na busca
híbrida, enquanto o FTS-only permanece na ordem de microssegundos.

**Fase 1:** usar SQLite/FTS como shortlist e só carregar o corpo completo dos
candidatos necessários.

### A-0100-02 — capture tem semântica de sucesso parcial

**Severidade:** alta para consistência.

A ordem atual de `capture` é:

1. escreve JSON;
2. escreve Markdown;
3. atualiza SQLite;
4. reconta tudo e escreve manifest.

Se 3 ou 4 falhar, o método retorna erro embora a fonte durável já exista. Um
retry pode então parecer uma nova operação, quando na verdade é reconciliação.

**Fase 1:** definir explicitamente a fonte de verdade. Enquanto JSON for a fonte
durável, falha do índice derivado deve marcar/requerer rebuild sem fingir que o
documento não foi salvo.

### A-0100-03 — derivados podem ficar stale

**Severidade:** alta para correção de retrieval.

Os campos de `MemoryDocument` são públicos. `capture` recalcula `body`,
`content_hash` e `last_seen_at`, mas só recalcula:

- embedding se a dimensão estiver errada;
- entidades se a lista estiver vazia.

Ele não recalcula `id` nem invalida embedding/entidades quando título/corpo/URL
foram alterados externamente.

Logo é possível persistir um corpo novo com embedding, entidades e ID derivados
do corpo antigo.

**Fase 1:** centralizar a normalização em um método de preparação imutável ou
tornar derivados não-editáveis por chamadores.

### A-0100-04 — manifest torna ingest incremental O(n²)

**Severidade:** alta para lotes.

Cada `capture` chama `write_index_manifest`, que chama `documents()` e
relê todos os JSONs apenas para contar documentos.

Inserir N documentos individualmente custa aproximadamente a soma de 1..N
full-scans, além do custo SQLite.

**Fase 1:** contagem deve vir da transação/índice ou ser atualizada
incrementalmente; rebuild completo só em doctor/recovery.

### A-0100-05 — FTS atual engole erros

**Severidade:** alta para observabilidade/correção.

No mirror Windows:

- a criação de `memory_fts` descarta o erro;
- delete/insert no FTS durante `upsert` também descarta o erro.

O documento pode existir na tabela normal e o índice FTS estar ausente ou stale
sem o chamador saber.

**Fase 1:** nenhuma mutação de índice derivado relevante pode usar `let _ =`
para apagar erro. A transação deve falhar ou registrar estado `needs_rebuild`.

### A-0100-06 — rebuild reabre SQLite por documento

**Severidade:** crítica para performance.

`sqlite_mirror::rebuild` chama `upsert` em loop. Cada `upsert` chama
`Database::open`, reaplica PRAGMAs e `CREATE TABLE IF NOT EXISTS`.

A baseline mediu:

- 1k: dezenas de segundos;
- 10k: não terminou em 180 s;
- 100k: não terminou em 180 s.

O mesmo FTS de 100k criado numa conexão/transação única levou poucos segundos.

**Fase 1:** uma conexão, prepared statements, uma transação de batch.

### A-0100-07 — exclusão por domínio não é uma política persistente

**Severidade:** média/alta para privacidade.

`forget(Domain)` remove o que existe e o rebuild não ressuscita o JSON apagado,
mas nada impede a mesma origem de ser capturada novamente depois.

**Fase 1:** tombstone/deny registry persistente, consultada antes de captura,
migração e rebuild.

### A-0100-08 — corrupção é silenciosamente omitida em consultas

**Severidade:** média.

`documents()` ignora falhas de leitura e JSON inválido. `doctor` consegue
contar corruptos, mas query/manifest não distinguem "documento inexistente" de
"documento ilegível".

**Fase 1:** manter disponibilidade, porém expor contador/flag de degradação no
doctor/audit log.

### A-0100-09 — temp file pode colidir dentro do mesmo processo

**Severidade:** média.

`atomic_write` usa `.<nome>.<pid>.tmp`. Duas escritas concorrentes do mesmo
arquivo no mesmo processo usam o mesmo temp path.

**Fase 1:** single-writer elimina a condição no storage. Para utilitários
genéricos, temp names devem ter nonce/contador.

### A-0100-10 — boost de "recência" é relativo ao item mais novo

**Severidade:** baixa; decisão semântica.

O boost usa `newest - doc.last_seen_at < 7 dias`, não `now - last_seen`.
Se todo o corpus tiver anos, os itens próximos ao mais novo ainda recebem boost.

Isso pode ser desejado como recência relativa do corpus, mas deve ser uma
decisão explícita.

## SPEC-0101 — research sessions

### A-0101-01 — IDs podem colidir no mesmo segundo

**Severidade:** alta.

`ResearchSession::new` deriva ID de `question + unix_seconds`.

`push_item` deriva ID de sessão, kind, título, provider e `unix_seconds`.

Duas operações semanticamente iguais no mesmo segundo podem gerar o mesmo ID.
Isso é especialmente perigoso porque o vetor aceita itens duplicados com ID
igual e sínteses referenciam IDs, não posição.

**Correção futura:** nonce/CSPRNG/UUID ou contador monotônico local. Não usar
segundos como componente de unicidade.

### A-0101-02 — synthesis ID tem a mesma granularidade

**Severidade:** média.

Duas sínteses do mesmo conjunto no mesmo segundo podem ter ID igual.

### A-0101-03 — upsert sobrescreve `created_at`

**Severidade:** baixa/média.

Atualizar uma resposta de provider grava o momento da atualização em
`created_at`. Isso elimina a data de criação original.

**Futuro:** separar `created_at` e `updated_at`.

### A-0101-04 — save tem janela de perda no replace

**Severidade:** média.

O `atomic_write` de research remove o destino antes de renomear o temp.
Uma queda entre as duas operações pode deixar a sessão sem arquivo final.

**Futuro:** replacement atômico específico de plataforma ou estratégia
temp+backup+rename com recovery.

### A-0101-05 — filtro por IDs é linear dentro de loop

**Severidade:** baixa.

`comparison` e `synthesize` usam `item_ids.contains` para cada item.
Para sessões pequenas é irrelevante; se crescerem, converter para `HashSet`.

## SPEC-0102 — inteligência local

### A-0102-01 — embedding hashing é barato de implantar, caro de consultar em massa

**Severidade:** média/alta em 100k+.

Cada documento carrega 384 `f32` no JSON e cada query calcula cosine em todos
os documentos. A desserialização textual do vetor também custa CPU e bytes.

**Fase 1:** embedding vira BLOB/tabela derivada. Retrieval vetorial só roda sobre
shortlist ou índice específico; FTS continua funcional sem embedding.

### A-0102-02 — lexical/token/entity alocam repetidamente

**Severidade:** média.

`hashed_embedding`, `tokenize` e `extract_entities` criam Strings/vetores a
cada query. Isso é aceitável como fallback pequeno, não como scan de 100k corpos.

**Fase 1:** pré-computar derivados; query só normaliza o texto do usuário uma vez.

### A-0102-03 — precedência do classificador é heurística, não composição

**Severidade:** baixa.

A ordem MemoryRecall → Research → Navigate → AgentTask significa que prompts
compostos podem cair na primeira classe que contém uma palavra-chave, mesmo
quando a intenção acionável aparece depois.

Não é falha de segurança porque autorização é outra camada, mas é limitação de
UX/roteamento a manter explícita.

### A-0102-04 — model pack install não é transacional

**Severidade:** baixa/média.

O modelo é renomeado para o path final antes de `manifest.json` ser escrito.
Queda entre as duas etapas deixa arquivo órfão.

Fora do escopo mínimo da Fase 1, mas deve entrar em doctor/cleanup futuro.

## SPEC-0103 — timeline semântica

### A-0103-01 — dedupe pode apagar anchors legítimos repetidos

**Severidade:** média.

A chave é `kind + text`. Duas seções diferentes chamadas "Conclusão", ou duas
respostas iguais em posições diferentes, viram um único anchor.

O teste atual prova ausência de duplicatas; não prova que a deduplicação é
semanticamente correta.

### A-0103-02 — position é ordinal, não geometria do documento

**Severidade:** média.

`position = ordinal / (n-1)`. Isso descreve ordem dos anchors, não posição real
de scroll/DOM. Uma seção enorme e outra minúscula recebem espaçamento igual.

Se a UI usar isso apenas como trilha semântica abstrata, está correto. Se
representar como posição física do documento, precisa de geometria real.

### A-0103-03 — limite de 128 é silencioso

**Severidade:** baixa/média.

Depois de 128 anchors o restante é descartado sem flag de truncamento.

**Futuro:** retornar metadado `truncated` ou priorizar anchors relevantes.

### A-0103-04 — todo link vira Source

**Severidade:** baixa.

`a[href]` inclui navegação, menus e links utilitários. Em páginas densas pode
consumir o orçamento de 128 antes dos anchors mais úteis.

## Prioridade sugerida pós-2.0.1

1. A-0100-06: conexão/transação única no rebuild.
2. A-0100-01/04: parar full scans por query/capture.
3. A-0100-03/05: consistência dos derivados e erros do índice.
4. A-0100-07: tombstones persistentes.
5. A-0101-01/02: IDs robustos.
6. A-0103-01/02: semântica da timeline, sem bloquear a Fase 1.

Nenhuma dessas correções deve ser feita nesta branch preparatória.
