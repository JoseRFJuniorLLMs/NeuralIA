# SPEC-0107 — Fase 1: desenho do índice V01 e baseline

**Documento auxiliar de implementação.** Não altera o status da SPEC-0107.

Este documento fixa o desenho da Fase 1 antes de qualquer mudança no runtime de
memória. A regra é **migrar o `memory.rs` atual, não reescrevê-lo**.

## 1. Restrição vinculante de runtime

A Fase 1 usa SQLite de forma deliberadamente pequena:

- `rusqlite` **síncrono**;
- **sem Tokio** e sem runtime assíncrono residente;
- conexão aberta **lazy**, somente quando memória/indexação/busca exige;
- nenhuma conexão SQLite, worker ou pool é criado na Home;
- nenhum thread pool é criado pelo subsistema de memória;
- operações potencialmente longas saem do event loop por meio da disciplina de
  trabalho já existente no aplicativo, não por um runtime Tokio escondido;
- WAL é habilitado na primeira abertura da base;
- o gate da SPEC-0008 de **≤ 16 threads em idle** continua vinculante.

O `ai-memory-store` upstream não deve ser importado como dependência de runtime.
Ele traz `tokio` com feature `full`, `refinery`, `argon2` e outras escolhas
adequadas ao produto upstream, mas inadequadas à Home leve do NeuralIA. O
upstream é referência de schema, retrieval e migrações; a implementação NeuralIA
é própria e mínima.

## 2. Fonte de verdade e papel do SQLite

Os arquivos atuais continuam sendo a fonte durável durante a migração:

- JSON em `memory/documents`;
- Markdown em `memory/wiki`;
- sessões em `memory/sessions`.

O SQLite V01 é **índice derivado e reconstruível**. A Fase 1 só pode inverter
essa relação depois que migração, rebuild, delete, private mode e recuperação
forem provados por testes.

## 3. Schema V01 único

O V01 nasce como um schema completo para os conceitos já previstos pela
SPEC-0107. Não haverá uma cadeia de mini-migrações V01a/V01b criadas durante o
mesmo trabalho.

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE schema_version (
    version       INTEGER PRIMARY KEY,
    applied_at    INTEGER NOT NULL,
    app_version   TEXT NOT NULL,
    schema_sha256 TEXT NOT NULL
);

CREATE TABLE research_space (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    description TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE research_session (
    id                TEXT PRIMARY KEY,
    research_space_id TEXT REFERENCES research_space(id) ON DELETE SET NULL,
    title             TEXT NOT NULL,
    initial_intent    TEXT NOT NULL,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

CREATE INDEX research_session_space_idx
    ON research_session(research_space_id, updated_at DESC);

CREATE TABLE navigation_observation (
    id                TEXT PRIMARY KEY,
    research_session_id TEXT REFERENCES research_session(id) ON DELETE SET NULL,
    source_kind       TEXT NOT NULL,
    provider          TEXT,
    url               TEXT,
    title             TEXT,
    query_text        TEXT,
    sanitized_text    TEXT,
    content_hash      TEXT,
    created_at        INTEGER NOT NULL,
    private           INTEGER NOT NULL DEFAULT 0 CHECK (private = 0)
);

CREATE INDEX navigation_observation_session_idx
    ON navigation_observation(research_session_id, created_at DESC);
CREATE INDEX navigation_observation_url_idx
    ON navigation_observation(url);

CREATE TABLE knowledge_page (
    page_pk           INTEGER PRIMARY KEY,
    id                TEXT NOT NULL UNIQUE,
    kind              TEXT NOT NULL,
    source_kind       TEXT NOT NULL,
    research_space_id TEXT REFERENCES research_space(id) ON DELETE SET NULL,
    research_session_id TEXT REFERENCES research_session(id) ON DELETE SET NULL,
    provider          TEXT,
    title             TEXT NOT NULL,
    url               TEXT,
    body              TEXT NOT NULL,
    content_hash      TEXT NOT NULL,
    created_at        INTEGER NOT NULL,
    last_seen_at      INTEGER NOT NULL
);

CREATE INDEX knowledge_page_session_idx
    ON knowledge_page(research_session_id, last_seen_at DESC);
CREATE INDEX knowledge_page_space_idx
    ON knowledge_page(research_space_id, last_seen_at DESC);
CREATE INDEX knowledge_page_provider_idx
    ON knowledge_page(provider);
CREATE INDEX knowledge_page_url_idx
    ON knowledge_page(url);
CREATE INDEX knowledge_page_hash_idx
    ON knowledge_page(content_hash);

CREATE VIRTUAL TABLE knowledge_page_fts USING fts5(
    title,
    body,
    provider,
    content='knowledge_page',
    content_rowid='page_pk',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER knowledge_page_ai AFTER INSERT ON knowledge_page BEGIN
    INSERT INTO knowledge_page_fts(rowid, title, body, provider)
    VALUES (new.page_pk, new.title, new.body, coalesce(new.provider, ''));
END;

CREATE TRIGGER knowledge_page_ad AFTER DELETE ON knowledge_page BEGIN
    INSERT INTO knowledge_page_fts(
        knowledge_page_fts, rowid, title, body, provider
    )
    VALUES (
        'delete', old.page_pk, old.title, old.body, coalesce(old.provider, '')
    );
END;

CREATE TRIGGER knowledge_page_au AFTER UPDATE ON knowledge_page BEGIN
    INSERT INTO knowledge_page_fts(
        knowledge_page_fts, rowid, title, body, provider
    )
    VALUES (
        'delete', old.page_pk, old.title, old.body, coalesce(old.provider, '')
    );
    INSERT INTO knowledge_page_fts(rowid, title, body, provider)
    VALUES (new.page_pk, new.title, new.body, coalesce(new.provider, ''));
END;

CREATE TABLE source_relation (
    id                     INTEGER PRIMARY KEY,
    from_page_id           TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    to_page_id             TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    kind                   TEXT NOT NULL,
    evidence_observation_id TEXT REFERENCES navigation_observation(id) ON DELETE SET NULL,
    created_at             INTEGER NOT NULL,
    UNIQUE(from_page_id, to_page_id, kind)
);

CREATE INDEX source_relation_to_idx
    ON source_relation(to_page_id, kind);

CREATE TABLE memory_entity (
    id              TEXT PRIMARY KEY,
    canonical_name  TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    entity_type     TEXT,
    UNIQUE(normalized_name, entity_type)
);

CREATE TABLE knowledge_page_entity (
    page_id   TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES memory_entity(id) ON DELETE CASCADE,
    weight    REAL NOT NULL DEFAULT 1.0,
    PRIMARY KEY(page_id, entity_id)
);

CREATE INDEX knowledge_page_entity_entity_idx
    ON knowledge_page_entity(entity_id, weight DESC);

CREATE TABLE memory_embedding (
    page_id     TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    model_id    TEXT NOT NULL,
    dimension   INTEGER NOT NULL,
    vector      BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY(page_id, model_id)
);

CREATE TABLE memory_feedback (
    id                  INTEGER PRIMARY KEY,
    page_id             TEXT REFERENCES knowledge_page(id) ON DELETE CASCADE,
    research_session_id TEXT REFERENCES research_session(id) ON DELETE CASCADE,
    signal              TEXT NOT NULL,
    value               REAL NOT NULL,
    created_at          INTEGER NOT NULL
);

CREATE INDEX memory_feedback_page_idx
    ON memory_feedback(page_id, created_at DESC);

CREATE TABLE tombstones (
    id          INTEGER PRIMARY KEY,
    object_type TEXT NOT NULL,
    object_id   TEXT NOT NULL,
    scope       TEXT,
    reason      TEXT,
    created_at  INTEGER NOT NULL,
    UNIQUE(object_type, object_id)
);

CREATE TABLE audit_log (
    id          INTEGER PRIMARY KEY,
    operation   TEXT NOT NULL,
    object_type TEXT NOT NULL,
    object_id   TEXT,
    detail_json TEXT,
    created_at  INTEGER NOT NULL
);

CREATE INDEX audit_log_object_idx
    ON audit_log(object_type, object_id, created_at DESC);

INSERT INTO schema_version(version, applied_at, app_version, schema_sha256)
VALUES (1, :applied_at, :app_version, :schema_sha256);
```

### Fora do V01

O schema não contém:

- users;
- handoffs;
- agent kinds;
- auth/session server;
- managed workstreams;
- tabelas específicas do MCP/CLI upstream.

Esses conceitos não pertencem ao navegador local.

## 4. Mapeamento do `memory.rs` atual

| Estado atual | V01 |
|---|---|
| `MemoryDocument.id` | `knowledge_page.id` |
| `kind` | `knowledge_page.kind` |
| `source_kind` | `knowledge_page.source_kind` |
| `title/url/body` | campos homônimos de `knowledge_page` |
| `provider` | `knowledge_page.provider` |
| `session_id` | `research_session.id` / FK em `knowledge_page` |
| `created_at/last_seen_at` | campos homônimos |
| `content_hash` | `knowledge_page.content_hash` |
| `entities[]` | `memory_entity` + `knowledge_page_entity` |
| `relations[]` | `source_relation` |
| `embedding` | `memory_embedding`, modelo legado `hashing-v1-384` |
| JSON/Markdown | continuam fonte durável durante Fase 1 |
| `index-manifest.json` | permanece como diagnóstico/rebuild até substituição provada |

## 5. Migração: preservar, indexar, verificar

A migração é incremental e reversível:

1. abrir SQLite apenas quando uma operação de memória realmente ocorrer;
2. criar V01 dentro de transação;
3. ler documentos existentes pelos APIs/formatos atuais;
4. criar `research_session` somente para IDs de sessão que realmente existam;
5. importar cada `MemoryDocument` preservando seu ID;
6. normalizar `entities[]` sem apagar o valor original do arquivo-fonte;
7. portar `relations[]` apenas quando os alvos existirem; relações órfãs são
   registradas no audit log e não inventam páginas;
8. portar o embedding atual como `hashing-v1-384`; ausência ou dimensão
   inválida não bloqueia FTS;
9. executar `integrity_check`, contagens e consultas-sentinela;
10. só então marcar `schema_version = 1`.

Falha em qualquer etapa deixa os arquivos atuais intactos e permite reconstruir
o SQLite do zero.

### ResearchSpace

O `memory.rs` atual não possui ResearchSpace. A migração **não fabrica** um
space para dados antigos. `research_space_id` entra como `NULL`; a UI futura
pode agrupar ou mover sessões explicitamente.

### NavigationObservation

Documentos legados não devem ser convertidos em falsa telemetria detalhada. Se
uma linha de proveniência for necessária para auditoria da importação, usa-se
uma observação explícita `source_kind='legacy-import'`, sem fingir uma
navegação que não foi registrada.

## 6. Escrita e concorrência

Fase 1 começa com **single writer** e transações curtas.

- uma conexão de escrita por operação/worker controlado;
- leituras podem abrir conexão separada quando necessário;
- nenhum pool residente;
- `busy_timeout` finito;
- statements preparados para lotes/reindex;
- rebuild deve usar uma transação, não abrir a base uma vez por documento.

O comportamento atual de `sqlite_mirror::rebuild` é baseline a superar, não
arquitetura a perpetuar.

## 7. Delete, private mode e tombstones

- private mode é rejeitado **antes** de arquivo e SQLite;
- delete remove a página e todas as linhas derivadas por FK/trigger;
- delete por domínio/sessão gera tombstone quando a exclusão precisa impedir
  reimport/reindex posterior;
- rebuild consulta tombstones antes de reindexar;
- clear-all pode recriar o índice vazio, mas não pode deixar conteúdo
  pesquisável em FTS, entidades, relações ou embeddings;
- audit log registra operação destrutiva sem corpo, segredo ou credencial.

A ausência atual de uma registry persistente de domínios excluídos é uma lacuna
conhecida e deve ser resolvida com tombstones/política local, não escondida por
um teste que apenas chama `forget(Domain)`.

## 8. Baseline obrigatória antes de implementar V01

O arquivo `crates/neural-core/tests/memory_phase1_baseline.rs` mede o
`memory.rs` **como ele existe antes da Fase 1**.

Tamanhos:

- 1.000 documentos;
- 10.000 documentos;
- 100.000 documentos.

Para cada tamanho registrar:

- query FTS-only no mirror SQLite atual;
- query híbrida pelo `MemoryStore::query` atual;
- `MemoryStore::rebuild`;
- RSS do processo com o store ativo e idle;
- número de threads do processo no mesmo ponto.

Os testes são `#[ignore]` porque 100k documentos e o rebuild atual podem ser
caros demais para o gate normal.

Execução em Windows:

```powershell
cargo test -p neural-core --test memory_phase1_baseline -- --ignored --nocapture
```

Os resultados impressos em JSON são o baseline. A Fase 1 só pode declarar
melhoria de performance comparando a mesma máquina, build mode e corpus.

## 9. Gates para substituir o índice atual

A implementação V01 não substitui o caminho atual até provar:

1. zero escrita em private mode;
2. delete/clear removem FTS, entities, relations e embeddings;
3. domínio excluído não reaparece após rebuild;
4. rebuild completo reconstrói o índice a partir da fonte durável;
5. FTS funciona sem embeddings;
6. schema V01 abre sem Tokio e sem criar threads de runtime;
7. Home continua sem abrir SQLite;
8. idle continua dentro do orçamento de threads da SPEC-0008;
9. baseline de 1k/10k/100k foi registrada antes e depois;
10. falha de migração deixa os arquivos legados recuperáveis.
