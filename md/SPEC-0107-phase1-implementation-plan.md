# SPEC-0107 Fase 1 — plano de implementação

**Status:** plano preparatório, sem código de runtime.  
**Base:** `main@a5391f9` + desenho do PR #12.  
**Regra:** executar somente depois da 2.0.1 e da ordem definida em `AGENTS.md`.

## Objetivo

Substituir o mirror SQLite experimental por um índice V01 reconstruível,
consistente e rápido, mantendo os arquivos atuais como fonte durável durante a
Fase 1.

Não é uma reescrita do subsistema. É migração incremental do `memory.rs`.

## Restrições vinculantes

- `rusqlite` síncrono;
- zero Tokio;
- zero runtime async residente;
- lazy open;
- Home não abre SQLite;
- nenhum pool de conexões residente;
- single writer;
- WAL;
- FTS5 com `unicode61 remove_diacritics 2`;
- private mode rejeitado antes de qualquer escrita;
- JSON/Markdown continuam recuperáveis durante toda a Fase 1;
- sem users, handoffs, agent kinds, auth server ou workstreams do upstream;
- sem `refinery` e sem `argon2` para este subsistema;
- nenhuma afirmação de spec passa a "Implementada" sem aceitação real.

## Auditoria de dependências

### Workspace NeuralIA atual

No `main@a5391f9` não há:

- `tokio`;
- `rusqlite`;
- `refinery`;
- `argon2`.

O SQLite atual é chamado diretamente via `winsqlite3` no Windows.

### Upstream ai-memory observado em 19/09/2026

O upstream `akitaonrails/ai-memory` está em versão workspace 2.3.1 e declara,
entre outras dependências:

- `tokio = { features = ["full"] }`;
- `rusqlite 0.32` com `bundled` e `backup`;
- `refinery` com integração rusqlite;
- `argon2`;
- `parking_lot`;
- auth, server, MCP, watchers e outras peças que não pertencem à Home.

Conclusão: **não depender de `ai-memory-store`**. O upstream permanece material
de referência e proveniência.

### Dependência mínima proposta

Adicionar apenas `rusqlite` diretamente ao `neural-core` quando a Fase 1
começar. A decisão entre SQLite do sistema e build bundled deve ser tomada no
PR de implementação com medição de:

- tamanho do binário;
- tempo de build;
- disponibilidade de FTS5;
- comportamento no Windows CI;
- CVE/supply-chain.

Não importar um runtime Tokio para resolver I/O síncrono local.

## Estrutura de código proposta

Sem exigir a reorganização imediatamente, o destino lógico é:

```text
crates/neural-core/src/
  memory.rs                 # API pública e compatibilidade
  memory/
    schema_v01.rs           # SQL V01 embutido + validação
    sqlite.rs               # open lazy, PRAGMAs, transações, statements
    migrate.rs              # JSON atual -> V01
    retrieval.rs            # FTS shortlist + fusão híbrida
    tombstones.rs           # exclusão persistente/rebuild policy
    doctor.rs               # integrity/recovery/rebuild
```

A divisão pode ser feita em commits separados; não é requisito quebrar o arquivo
monolítico no primeiro commit.

## API interna sugerida

```rust
struct MemoryIndex {
    db_path: PathBuf,
}

impl MemoryIndex {
    fn new(root: &Path) -> Self;                 // sem abrir DB
    fn with_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T>;
    fn with_write<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T>;
    fn ensure_schema(&self, conn: &Connection) -> Result<()>;
    fn rebuild_from_documents(&self, docs: impl Iterator<Item = MemoryDocument>) -> Result<...>;
}
```

A forma exata pode mudar, mas as propriedades são obrigatórias:

- construir `MemoryIndex` não toca no disco;
- a conexão vive só durante a operação;
- escrita longa usa uma conexão e uma transação;
- nenhuma thread é criada pelo storage.

## Sequência de implementação

### 0107.1 — storage foundation

Criar:

- dependência `rusqlite` mínima;
- lazy open;
- PRAGMAs;
- schema V01;
- `schema_version`;
- helpers de transação;
- teste de create/open idempotente;
- teste que construir o storage não cria DB;
- CI que falha se `tokio` aparecer no grafo do `neural-core`.

Não trocar retrieval ainda.

### 0107.2 — migration e rebuild

Implementar:

- import de `MemoryDocument` existente;
- preservação de IDs;
- entidades normalizadas;
- embeddings legados em BLOB;
- relações somente quando o alvo existe;
- audit log para órfãos;
- tombstones;
- rebuild em **uma conexão/transação**;
- integrity check;
- rollback seguro.

O JSON continua fonte durável.

### 0107.3 — delete/privacy/recovery

Implementar e provar:

- private mode = zero rows + zero files;
- delete por documento;
- delete por sessão;
- delete por domínio;
- clear-all;
- cascade de FTS/entities/relations/embeddings;
- tombstone impede reimport;
- banco corrompido pode ser descartado e reconstruído;
- falha durante migração não altera JSON legado.

### 0107.4 — FTS como shortlist

Trocar o full scan lexical por FTS5:

1. normalizar query;
2. consultar FTS com limit superior controlado;
3. aplicar filtros provider/session no SQL;
4. carregar apenas candidatos;
5. retornar proveniência id/url/provider/session.

FTS deve funcionar mesmo se a tabela de embeddings estiver vazia.

### 0107.5 — retrieval híbrido

Depois do FTS estável:

- FTS shortlist;
- entity hits;
- embedding apenas nos candidatos ou por índice dedicado;
- graph expansion limitada;
- RRF determinístico;
- recency com semântica explicitamente escolhida.

Não fazer cosine de 384 dimensões sobre 100k JSONs.

## Plano de migração do estado atual

Para cada JSON válido:

1. validar `private == false`;
2. preservar `id`;
3. recalcular/verificar `content_hash`;
4. validar derived fields;
5. inserir `knowledge_page`;
6. inserir entidades;
7. inserir embedding legado como `hashing-v1-384`;
8. acumular relações para segunda passagem;
9. inserir relações cujo alvo exista;
10. registrar órfãos no audit log.

A migração não inventa ResearchSpace nem NavigationObservation histórica.

## Política para derivados stale

Antes de gravar no V01, o documento deve passar por uma única função de
normalização que derive de novo:

- body redigido;
- content hash;
- entidades;
- embedding;
- identificador, se a política final de ID continuar content-addressed.

Não aceitar combinação arbitrária de campos públicos stale.

## Tombstones

Escopos mínimos:

- document;
- session;
- domain;
- before/retention quando aplicável.

Rebuild/migration consultam tombstones antes de inserir. Tombstone não contém
corpo nem segredo.

## Test plan

### Schema/lazy

- `memory_index_constructor_does_not_create_sqlite`;
- `v01_schema_create_is_idempotent`;
- `v01_schema_version_is_exactly_one`;
- `fts5_is_available_or_open_fails_explicitly`.

### Migration

- `migration_preserves_document_ids`;
- `migration_preserves_provenance`;
- `migration_recomputes_stale_derived_fields`;
- `migration_records_orphan_relations`;
- `migration_failure_keeps_json_source_intact`;
- `second_migration_is_idempotent`.

### Privacy/delete

- `private_capture_zero_files_zero_rows`;
- `delete_document_cascades_all_derived_rows`;
- `clear_all_leaves_no_searchable_rows`;
- `domain_tombstone_blocks_reimport`;
- `domain_tombstone_blocks_rebuild_resurrection`.

### Retrieval

- `fts_works_without_embeddings`;
- `provider_filter_happens_in_sql`;
- `session_filter_happens_in_sql`;
- `deleted_page_never_appears_in_fts`;
- `hybrid_result_keeps_exact_provenance`.

### Recovery

- `corrupt_index_rebuilds_from_json`;
- `interrupted_rebuild_does_not_replace_good_index`;
- `doctor_reports_corrupt_source_documents`.

### Performance/gates

- repetir harness 1k/10k/100k;
- 10k e 100k rebuild devem terminar dentro da janela de 180 s antes de qualquer
  alegação de melhoria;
- registrar FTS-only e híbrida antes/depois;
- Home: zero SQLite aberto/criado;
- thread gate da SPEC-0008 continua verde;
- `cargo tree` não pode introduzir Tokio no core por causa desta fase.

## Critério de merge por subfase

Cada PR:

1. nasce do `main` atualizado;
2. toca apenas a subfase;
3. fmt/clippy/test verdes;
4. contém teste que falha se a implementação for removida;
5. mede performance quando toca hot-path;
6. recebe revisão independente antes de merge se passar por área sensível;
7. não muda versão nem cria release.
