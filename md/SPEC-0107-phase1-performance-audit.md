# SPEC-0107 Fase 1 — auditoria de performance do memory.rs atual

**Base:** `main@a5391f9`  
**Objetivo:** explicar o custo observado antes de otimizar.

## Modelo de custo atual

### Capture

`MemoryStore::capture` executa:

1. redaction/hash;
2. serialização JSON + atomic write;
3. geração Markdown + atomic write;
4. `sqlite_mirror::upsert`;
5. `write_index_manifest`;
6. `write_index_manifest` chama `documents()` e relê todo o corpus.

Para um lote de N captures, o passo 6 produz soma de scans de 1 até N.

**Complexidade aproximada do lote:** O(N²) em número de documentos, ignorando
tamanho do corpo.

### Query

`query` começa com `documents()`, portanto cada consulta faz I/O + JSON parse
de todos os documentos.

Depois, para N documentos:

- lexical: tokeniza título, corpo e entidades;
- semantic: cosine 384-D para cada documento;
- entity: percorre entidades;
- graph: percorre relações;
- RRF lexical/semantic/entity: ordenações;
- ranking final: nova ordenação.

**Custo:** O(bytes do corpus + N*384 + tokenização + N log N).

Não há uso do FTS SQLite no caminho real de `MemoryStore::query`.

### Rebuild

`sqlite_mirror::rebuild`:

```text
for document in documents:
    upsert(path, document)
        Database::open(path)
        PRAGMA journal_mode=WAL
        PRAGMA synchronous=NORMAL
        CREATE TABLE IF NOT EXISTS ...
        CREATE VIRTUAL TABLE IF NOT EXISTS ...
        INSERT/REPLACE
        DELETE FTS
        INSERT FTS
        close
```

O custo dominante é lifecycle/DDL repetido, não FTS5.

## Baseline já medida

| Docs | Híbrida | FTS-only | Build FTS 1 conexão | Rebuild atual |
|---:|---:|---:|---:|---:|
| 1k | 74,048 ms | 18 µs | 51 ms | 47,302 s |
| 10k | 524,554 ms | 15 µs | 99 ms | >180 s |
| 100k | 8,514 s | 32 µs | 1,694 s | >180 s |

Os valores são de runner hospedado e têm ruído, mas a diferença de ordens de
grandeza é estrutural.

## Hotspots em ordem

### P0 — rebuild reabre DB N vezes

**Mudança:** uma conexão + transaction + prepared statements.

**Prova:** benchmark rebuild 1k/10k/100k.

### P0 — query ignora o FTS existente

**Mudança:** FTS shortlist + filtros no SQL.

**Prova:** resultado e proveniência iguais/compatíveis, tempo medido.

### P1 — manifest full-scan após cada capture

**Mudança:** contagem transacional/incremental.

**Prova:** benchmark de ingest em lote e manifest correto após crash/rebuild.

### P1 — derivados recomputados durante toda query

Tokenização de corpo e entidades acontece toda vez. Embedding está persistido,
mas cosine continua N vezes.

**Mudança:** derivados indexados; carregar corpo completo só para top candidates.

### P1 — JSON embedding em Vec<f32>

384 floats em JSON ampliam bytes e parse. No V01 o embedding deve ser BLOB
derivado, identificado por modelo/dimensão.

### P2 — RRF ordena streams inteiros

Quando o shortlist vier do SQL, os streams serão menores. Antes disso, cada
stream de até N itens é ordenado.

### P2 — graph scan global

A expansão atual percorre todos os documentos e suas relações para encontrar
links apontando para seeds. No V01 isso vira índice por `to_page_id`.

## Otimizações explicitamente proibidas

Não "resolver" performance com:

- cache de todo corpus residente na Home;
- pool de threads;
- Tokio;
- pool SQLite permanente;
- índice que não respeita delete/private/tombstones;
- benchmark que mede corpus menor e extrapola 100k;
- desativar integridade/foreign keys para ganhar número bonito.

## Métricas adicionais para o PR de implementação

Além da baseline existente, medir:

- ingest 1k/10k em batch;
- tempo de primeira abertura lazy;
- tempo de segunda abertura;
- bytes do DB/WAL após rebuild;
- peak RSS durante rebuild;
- threads antes/durante/depois;
- query com filtro provider;
- query com filtro session;
- delete de domínio de 10% do corpus;
- rebuild após tombstones;
- recovery após DB truncado.

## Resultado esperado da arquitetura

A Fase 1 deve transformar:

```text
filesystem full scan -> parse all -> score all
```

em:

```text
lazy DB open
  -> indexed filters
  -> FTS shortlist
  -> optional entity/vector/graph rerank
  -> load only top documents
```

Sem tornar SQLite ou embeddings residentes na Home.
