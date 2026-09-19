# SPEC-0107 Fase 1 — revisão adversarial do schema V01

**Data:** 19/09/2026  
**Schema revisado:** `md/SPEC-0107-phase1-design.md` do PR #12  
**Natureza:** validação preparatória; não altera o desenho original nem implementa runtime.

## Resultado

A estrutura geral do V01 é sólida. Os triggers FTS5 de insert/update/delete foram
validados com SQLite real e mantêm o índice consistente em:

- insert;
- update que remove termos antigos;
- update que adiciona termos novos;
- delete;
- tokenização `unicode61 remove_diacritics 2`.

Há, porém, três correções que devem entrar antes do primeiro commit de storage.

---

## S-01 — PRAGMAs de journal/synchronous não podem ficar dentro da transação de schema

**Severidade:** bloqueante de implementação.

O desenho diz:

1. abrir lazy;
2. criar V01 dentro de transação.

O bloco SQL começa com:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
```

Em SQLite, se esses comandos forem executados depois de `BEGIN`:

- `PRAGMA journal_mode=WAL` falha com `cannot change into wal mode from within a transaction`;
- `PRAGMA synchronous=NORMAL` falha com `Safety level may not be changed inside a transaction`.

`foreign_keys=ON` não dá erro, mas também deve ser política de conexão, não
parte conceitual de uma migration.

### Ordem correta

```text
open connection
  -> busy_timeout
  -> foreign_keys=ON
  -> journal_mode=WAL (writer/open initialization)
  -> synchronous=NORMAL
  -> BEGIN IMMEDIATE
  -> CREATE TABLE / INDEX / TRIGGER
  -> verification
  -> INSERT schema_version
  -> COMMIT
```

`foreign_keys`, `synchronous` e timeout devem ser reaplicados conforme a
semântica da conexão; não presumir que todas as PRAGMAs são configuração
persistente do arquivo.

### Teste obrigatório

```text
fresh_db_open_applies_pragmas_before_schema_transaction
```

Deve verificar:

- journal_mode retorna `wal`;
- foreign_keys retorna `1`;
- synchronous está no nível escolhido;
- schema transaction completa sem erro.

---

## S-02 — UNIQUE(normalized_name, entity_type) não deduplica entity_type NULL

**Severidade:** alta para consistência de entidades.

Schema atual:

```sql
entity_type TEXT,
UNIQUE(normalized_name, entity_type)
```

SQLite permite múltiplos NULL em UNIQUE.

Logo estas duas linhas são aceitas ao mesmo tempo:

```text
(id=e1, normalized_name=rust, entity_type=NULL)
(id=e2, normalized_name=rust, entity_type=NULL)
```

Isso quebra a promessa de normalização/deduplicação para a categoria mais
comum: entidade sem tipo conhecido.

### Opções

**Preferida para V01 simples:**

```sql
entity_type TEXT NOT NULL DEFAULT '',
UNIQUE(normalized_name, entity_type)
```

ou um tipo explícito como `unknown`.

Alternativa:

```sql
CREATE UNIQUE INDEX memory_entity_identity_idx
ON memory_entity(normalized_name, coalesce(entity_type, ''));
```

A representação escolhida deve ser canônica na migration.

### Teste obrigatório

Inserir a mesma entidade normalizada duas vezes sem tipo deve resultar em um
único identity key, não duas entidades.

---

## S-03 — memory_feedback aceita linha sem page nem session

**Severidade:** média/alta para integridade.

Schema atual permite:

```sql
INSERT INTO memory_feedback(signal, value, created_at)
VALUES ('x', 1.0, 1);
```

porque `page_id` e `research_session_id` são ambos nullable.

Isso cria feedback sem objeto ao qual aplicar o sinal.

### Constraint sugerida

Se feedback pode pertencer a página, sessão ou ambos:

```sql
CHECK (page_id IS NOT NULL OR research_session_id IS NOT NULL)
```

Se deve pertencer exatamente a um:

```sql
CHECK (
  (page_id IS NOT NULL) <> (research_session_id IS NOT NULL)
)
```

A semântica deve ser decidida antes do V01 congelar.

---

## S-04 — schema_version precisa de fluxo idempotente fora do script bruto

**Severidade:** média.

O bloco V01 usa `CREATE TABLE` sem `IF NOT EXISTS`, o que é bom para detectar
estado inesperado, mas significa que o script inteiro não é um "ensure schema".

A implementação deve primeiro ler:

```text
sqlite_master / schema_version
```

e decidir explicitamente:

- DB novo -> criar V01;
- V01 válido -> abrir, não rerodar DDL;
- versão desconhecida -> falhar/migrar;
- DB parcial sem schema_version -> recovery/rebuild;
- schema_version=1 com hash divergente -> integrity/recovery, nunca "seguir em frente".

Não transformar o DDL em uma coleção de `IF NOT EXISTS` que mascara schema
parcial.

---

## S-05 — schema hash deve cobrir uma representação canônica

**Severidade:** média.

`schema_sha256` é útil apenas se dois builds calcularem o mesmo hash.

Definir uma constante única:

```rust
const SCHEMA_V01: &str = include_str!(...);
```

O hash deve ser dos bytes exatos dessa constante.

Não montar schema concatenando fragments em ordem incidental e depois chamar
isso de provenance.

Teste:

```text
v01_schema_hash_is_stable_and_matches_version_row
```

---

## S-06 — tombstone de domínio exige identidade normalizada antes do UNIQUE

**Severidade:** média.

`UNIQUE(object_type, object_id)` funciona somente se `object_id` já estiver
normalizado.

Para domínio, decidir no boundary:

- lowercase;
- remover trailing dot;
- IDNA/punycode consistente;
- regra para subdomínios;
- porta não faz parte da identidade;
- URL inteira nunca é usada como domain tombstone.

Exemplo que deve dar a mesma identidade:

```text
Example.COM
example.com.
https://example.com:443/path
```

O schema não precisa conhecer DNS; o storage precisa receber a chave canônica.

---

## S-07 — audit_log.detail_json não é uma licença para guardar body

**Severidade:** média de privacidade.

A coluna é livre e o SQL não consegue impor redaction.

Criar um tipo Rust fechado para detalhes auditáveis, em vez de aceitar
`serde_json::Value` arbitrário vindo do chamador.

Exemplos permitidos:

- counts;
- IDs;
- scope;
- duration;
- reason code;
- orphan relation IDs.

Proibido:

- body;
- query secreta;
- cookie;
- headers;
- token;
- embedding bruto quando derivado de dado privado.

---

## S-08 — validar encoding do embedding antes de congelar BLOB

**Severidade:** média.

`dimension` + `vector BLOB` não declara encoding.

Antes do V01:

```text
encoding = little-endian f32 contiguous
length = dimension * 4
model_id = hashing-v1-384 para legado
```

Pode ser documentado em código sem adicionar coluna se V01 aceitar apenas esse
encoding.

Se houver intenção real de SQ8/PQ no mesmo schema, adicionar `encoding`
explicitamente agora.

Não guardar bytes cuja interpretação depende de conhecimento tribal de 2026.

---

# FTS5: resultado da validação

O desenho atual usa external-content FTS:

```sql
content='knowledge_page',
content_rowid='page_pk'
```

Os três triggers propostos se comportaram corretamente.

### Insert

Página `Café Rust / texto vetorial`:

```text
MATCH 'cafe' -> row encontrada
```

### Update

Depois de trocar para `Python novo / outro corpus`:

```text
MATCH 'cafe'   -> vazio
MATCH 'python' -> row encontrada
```

### Delete

Depois de apagar a página:

```text
MATCH 'python' -> vazio
```

Portanto não há motivo para mudar a sintaxe dos triggers por especulação.

## Otimização para rebuild

Triggers por linha podem permanecer no caminho incremental.

Para rebuild grande, medir duas estratégias:

**A.** tabela + FTS + triggers criados antes do import, inserts acionam FTS;

**B.** bulk import do content table, depois FTS/rebuild em lote.

Só escolher B se o benchmark demonstrar ganho material e a sequência continuar
atômica/recuperável.

O problema de 180s atual já está explicado pela abertura de conexão/schema por
documento; não otimizar prematuramente o segundo gargalo antes de remover o
primeiro.

---

# Constraints adicionais recomendadas

Sem transformar V01 em uma fortaleza de SQL decorativo, estes checks têm valor:

```sql
CHECK (last_seen_at >= created_at)
CHECK (dimension > 0)
CHECK (weight >= 0.0)
CHECK (private = 0)
CHECK (page_id IS NOT NULL OR research_session_id IS NOT NULL) -- feedback
```

Enums textuais (`kind`, `source_kind`, `signal`) podem permanecer TEXT se
Rust fizer parsing fechado e migration rejeitar valores desconhecidos.

---

# Gate de schema antes do código de retrieval

1. fresh create;
2. reopen sem DDL;
3. versão desconhecida;
4. schema hash divergente;
5. transaction rollback no meio da criação;
6. FTS insert/update/delete;
7. foreign key cascade;
8. entity NULL dedupe;
9. feedback sem owner rejeitado;
10. private=1 rejeitado;
11. tombstone domain normalization;
12. corrupt index -> discard/rebuild a partir do JSON;
13. nenhuma dessas operações abre DB ao construir a Home.

## Conclusão

O V01 não precisa ser redesenhado.

Antes de implementar, corrigir:

1. ordem dos PRAGMAs;
2. identidade de `memory_entity` quando type é desconhecido;
3. ownership constraint de `memory_feedback`;
4. fluxo explícito de schema version/hash.

O restante pode entrar incrementalmente com testes.
