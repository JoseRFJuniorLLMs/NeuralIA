# Relatório da memória semântica do NeuralIA

**Data da revisão:** 2026-09-20  
**Base funcional:** `origin/main` em `f1e1071e`  
**Gate determinístico revisado:** PR #88, SHA `60258a5c`  
**Estado:** funcional no código candidato; ainda não publicado em release estável

## 1. Resumo executivo

A memória semântica está ligada ao produto e já pode ser usada pela interface do
NeuralIA. O usuário pode pesquisar a memória com `Ctrl+H` ou `memory:`, reconstruir
o índice com `memory:rebuild` e apagar histórico e memória com
`Ctrl+Shift+Delete`.

Os dados duráveis são mantidos em documentos JSON e Markdown locais. O SQLite
V01 é um índice derivado: acelera busca e pode ser reconstruído sem ser a única
cópia do conhecimento. A busca combina shortlist textual FTS5, embedding local
determinístico, entidades, relações e recência.

Há limitações importantes:

- a release pública `v2.0.1` ainda não contém esta Fase 1 completa;
- o PR #88 melhora o gate de regressão, mas não muda o comportamento de produção;
- o issue #89 precisa ser corrigido antes da release, pois um teste do instalador
  falha no runner Windows ao comparar caminho longo com alias 8.3;
- respostas dos provedores ficam na `ResearchSession`, mas atualmente não viram
  automaticamente um `MemoryDocument` pesquisável;
- o Memory Doctor e o forget seletivo existem no núcleo, mas ainda não possuem
  comando próprio na interface;
- a ponte português/inglês é uma tabela determinística pequena, não tradução
  geral nem embedding de um modelo grande.

## 2. Como usar hoje

### Pesquisar

Opção 1:

1. Pressione `Ctrl+H`.
2. O NeuralIA preenche `memory:` na omnibox.
3. Escreva o que deseja reencontrar e pressione `Enter`.

Opção 2: escreva diretamente na omnibox:

```text
memory: WebView2 prompt injection
```

O alias curto também funciona:

```text
mem: segurança do navegador
```

Uma consulta vazia, `memory:`, lista até 20 documentos recentes. Uma consulta
normal retorna por padrão até 12 resultados, com título, origem, modos de
correspondência, trecho e URL quando disponível.

### Reconstruir o índice

Digite:

```text
memory:rebuild
```

ou:

```text
mem:rebuild
```

Use isso quando uma busca informar que o índice está ausente, incompatível ou
corrompido. A reconstrução ocorre no worker de memória, fora do event loop da
interface.

### Apagar tudo

Pressione `Ctrl+Shift+Delete`. Na implementação atual esse atalho apaga:

- histórico cronológico local;
- documentos da memória semântica;
- sessões de pesquisa persistidas;
- índice SQLite derivado;
- arquivos derivados de apresentação.

O perfil WebView2, cookies e sessões dos provedores não são apagados por esse
atalho. O registro de tombstones permanece de propósito para impedir que dados
apagados reapareçam durante uma reconstrução ou recaptura atrasada.

### Local dos dados

No Windows, a raiz padrão é:

```text
%LOCALAPPDATA%\NeuralIA\memory
```

A variável `NEURALIA_DATA_DIR` pode substituir a raiz de dados da aplicação.

## 3. O que entra na memória

| Ação | Conteúdo capturado | Observação |
|---|---|---|
| Iniciar comparação multi-IA | pergunta do usuário | vinculada à nova `ResearchSession` |
| Abrir artigo pelo Reader | título, URL e texto extraído/redigido | é o conteúdo mais completo capturado automaticamente |
| Abrir PDF | nome/URL e indicação de que o PDF foi aberto | o texto integral do PDF não é indexado atualmente |
| Abrir página Web pública | host, URL e metadados mínimos | não equivale a capturar todo o corpo da página |
| Abrir fonte no Split View | título, URL e provedor de origem | painel privado produz zero memória |
| Executar `agent: ... | extract` | trecho observado e redigido | segredos são filtrados antes da persistência |
| Receber resposta de Gemini/ChatGPT/Claude | resposta salva na `ResearchSession` | ainda não é transformada automaticamente em `MemoryDocument` pesquisável |

Documentos marcados como privados são rejeitados antes de qualquer escrita
persistente. Títulos, URLs e corpos passam por redação de credenciais e outros
segredos antes de serem gravados.

## 4. SQLite V01 transacional

### O que é

É o primeiro schema versionado do índice SQLite da memória. Ele contém páginas
de conhecimento, sessões, relações, entidades, embeddings, feedback,
tombstones e log de auditoria. A tabela FTS5 acompanha inserções, atualizações e
exclusões por triggers.

### Para que serve

- localizar candidatos rapidamente;
- filtrar por provedor ou sessão;
- guardar embeddings locais e relações;
- manter o índice consistente durante cada alteração;
- detectar schema desconhecido ou parcialmente criado.

### Como funciona

Cada `upsert` ocorre em uma transação SQLite `IMMEDIATE`. Criação do schema e
rebuild também são transacionais e validados. Foreign keys são habilitadas e o
hash do schema é conferido ao abrir.

O termo “transacional” deve ser entendido com precisão: a transação protege a
alteração no SQLite. Uma captura também escreve JSON e Markdown no filesystem;
esses três meios não formam uma única transação distribuída. O JSON durável é a
base a partir da qual o índice derivado pode ser reconstruído.

### Como o usuário utiliza

Não há comando para “ligar SQLite”. Ele é criado de forma preguiçosa na primeira
captura e usado automaticamente por `memory:`.

### Estado na candidata

**Usável no código candidato.** Os testes cobrem criação, reabertura, triggers,
filtros, embeddings, schema corrompido, rollback e documentos privados.

## 5. Shortlist FTS5 com reranking semântico

### O que é

A busca não calcula todos os sinais sobre todo o corpus logo de início. O FTS5
seleciona uma shortlist de IDs usando termos da consulta. O limite interno é
derivado do número de resultados pedido e fica limitado a 512 candidatos.

Depois, os candidatos recebem sinais de:

- correspondência lexical;
- similaridade de embedding local;
- entidades em comum;
- relações com outros resultados;
- recência.

Os rankings são combinados por uma forma de reciprocal-rank fusion.

### Para que serve

Reduz o trabalho de consulta e permite que uma correspondência conceitual
melhore a ordem dos resultados sem depender de serviço de nuvem.

### Como usar

Use uma descrição natural:

```text
memory: defesa contra prompt injection no WebView
```

O resultado indica os sinais usados, por exemplo `lexical+semantic+entity`.

### Limite importante

Se o FTS5 funciona mas não encontra nenhum candidato, a implementação atual lê
os documentos duráveis para tentar o fallback semântico. Portanto, a consulta
sem candidato lexical ainda pode custar O(corpus). O que foi removido é o
fallback silencioso quando o índice está quebrado ou indisponível.

### Estado na candidata

**Usável no código candidato.** Não é uma busca neural baseada em modelo grande;
o embedding atual é local, determinístico e baseado em hashing.

## 6. Busca cruzada português/inglês

### O que é

Uma tabela única associa grupos pequenos de equivalentes técnicos, como:

- `memory`, `memória`, `memoria`;
- `browser`, `navegador`, `navegadores`;
- `research`, `pesquisa`;
- `agent`, `agente`;
- `security`, `segurança`;
- `source`, `fonte`;
- `answer`, `resposta`;
- `code`, `código`;
- variantes de vetor/SIMD, CPU/processador e otimização.

A mesma tabela alimenta a expansão lexical do FTS e as features do embedding.
Isso evita que o FTS descarte um documento em inglês antes do reranking
semântico de uma consulta em português, ou vice-versa.

### Para que serve

Permite, por exemplo, encontrar um documento com “Browser engines” usando:

```text
memory: navegadores
```

### Limite importante

É uma ponte técnica deliberadamente pequena. Ela não traduz frases livres e não
promete equivalência multilíngue geral.

### Estado na candidata

**Usável**, com testes nos dois sentidos PT→EN e EN→PT.

## 7. Tombstones persistentes para esquecer

### O que são

Um tombstone é um registro durável de negação: documento, sessão, domínio ou
faixa temporal foi esquecido. Ele é gravado antes da exclusão dos documentos.

### Para que servem

Sem tombstone, um crash no meio do apagamento, um arquivo antigo ou uma
reconstrução poderia importar novamente aquilo que o usuário mandou esquecer.
Com o tombstone, captura, leitura e rebuild recusam o conteúdo antigo.

### Como usar

Na interface atual, `Ctrl+Shift+Delete` usa o escopo `All`. Os escopos seletivos
por documento, sessão, domínio e data existem no núcleo, mas ainda não estão
expostos como comandos de usuário.

### Estado na candidata

**Usável para apagar tudo pela interface.** Forget seletivo está implementado e
testado no núcleo, mas ainda não possui UX pública.

## 8. Exclusão real dos bytes esquecidos

### O que significa

Esquecer não é apenas esconder resultados. O fluxo remove os documentos JSON,
os Markdown derivados, sessões aplicáveis e registros do índice. Depois ele
reconstrói o SQLite somente com o que permaneceu permitido.

Os testes gravam uma string secreta, executam `forget` e percorrem a árvore do
store para provar que a string deixou de existir nos arquivos remanescentes.

### Para que serve

Garante que “esquecer” tenha efeito material no disco, não apenas visual.

### Como usar

Use `Ctrl+Shift+Delete` para o apagamento total atualmente exposto.

### Limite importante

O arquivo de tombstones continua no disco. Isso é intencional: ele registra o
que não pode ser reimportado, sem preservar o conteúdo apagado.

### Estado na candidata

**Usável e coberto por testes de ausência de vestígios.**

## 9. Rebuild atômico e validado

### O que é

O NeuralIA constrói um novo SQLite em arquivo temporário, insere documentos e
relações, executa `PRAGMA integrity_check`, confere a contagem FTS/conteúdo,
fecha o WAL e reabre o arquivo final para nova validação.

Somente depois o índice antigo é substituído. Um backup de nome fixo permite
recuperar interrupção entre renames no Windows. Se a construção falhar, o
índice anterior permanece intacto.

### Para que serve

- recuperar índice ausente ou corrompido;
- aplicar schema correto;
- reconstruir derivados após forget;
- evitar publicar índice pela metade.

### Como usar

Digite `memory:rebuild`. O comando apenas agenda a operação; a interface atual
não oferece barra detalhada de progresso nem relatório final do Memory Doctor.

### Estado na candidata

**Usável.** Há testes de corrupção de schema, falha durante rebuild, recuperação
de backup interrompido e preservação de tombstones.

## 10. Captura incremental sem varrer o corpus

### O que mudou

Uma captura antes executava validação integral do SQLite e recontava o corpus.
O custo crescia com o número de documentos e podia saturar a fila de memória.

Agora a captura:

1. verifica por `stat` apenas se o documento-alvo já existe;
2. grava o documento e seu Markdown atomicamente;
3. faz `upsert` transacional somente das linhas relacionadas;
4. incrementa o manifesto em `0` ou `1` sem listar todo o diretório;
5. deixa validação integral e recontagem para `rebuild`/`forget`.

O PR #88 substitui o teste instável de relógio por contadores de operações sob
`#[cfg(test)]`. O gate exige zero `document_file_count()` e zero
`validate_integrity()` durante uma captura real, inclusive ao regravar o mesmo
documento. A sabotagem que reintroduziu a varredura deixou o teste vermelho.

### Para que serve

Mantém a captura previsível quando a memória cresce e reduz o risco de saturar a
fila limitada de 128 comandos.

### Como usar

É automático. Use normalmente comparação, Reader, PDF, Web/Split View ou
`agent: ... | extract`.

### Limite importante

A fila usa `try_send`. Se ficar cheia, uma captura ainda pode ser descartada e
apenas registrada no stderr. A correção reduz fortemente a causa conhecida da
saturação, mas não transforma a fila em entrega garantida.

### Estado na candidata

**Comportamento já presente no `main`; gate determinístico aprovado tecnicamente
no PR #88 e ainda aguardando integração.**

## 11. Falhas do índice não viram full scan silencioso

### O que significa

Se já existe corpus e o SQLite está ausente, corrompido ou impossível de abrir,
a consulta retorna erro indicando `memory:rebuild`. Ela não muda silenciosamente
para uma busca completa nos JSONs, que esconderia a falha e alteraria custo e
semântica.

Um perfil realmente vazio pode não ter SQLite ainda; esse caso continua válido.

### Para que serve

- torna degradação visível;
- evita lentidão inesperada por falha do índice;
- mantém comportamento pesquisável e auditável;
- orienta recuperação explícita.

### Como usar

Se a janela de resultados mostrar índice indisponível, execute:

```text
memory:rebuild
```

e repita a busca depois da reconstrução.

### Estado na candidata

**Usável e fail-closed para falhas reais do índice.** Como explicado na seção 5,
“índice saudável sem candidatos lexicais” é um caso diferente e ainda pode usar
o fallback semântico sobre os documentos.

## 12. Memory Doctor

O núcleo possui `MemoryStore::doctor(rebuild)`. Ele conta documentos válidos e
corrompidos, informa presença do manifesto e do SQLite e pode solicitar rebuild.

Entretanto, não existe `memory:doctor` no roteamento atual da aplicação. Assim:

- **núcleo:** implementado e testado;
- **uso pelo usuário final:** ainda não exposto;
- **recuperação disponível hoje:** `memory:rebuild`.

## 13. Situação por distribuição

| Distribuição | Situação |
|---|---|
| Release pública `v2.0.1` | não contém a Fase 1 completa descrita neste relatório |
| `origin/main` em `f1e1071e` | funcionalidade presente; ainda possui o antigo gate de custo baseado em relógio |
| PR #88 em `60258a5c` | comportamento de produção igual ao `main`; gate determinístico revisado e sem achados bloqueantes |
| Próxima candidata 2.1.0 | deverá conter a memória após integração do #88 e correção do #89 |
| PR antigo #77 | não deve ser tratado como candidato final; precisa ser substituído por release criada do `main` final |

## 14. Parecer de prontidão

### Funcionalidade de memória

Está utilizável para captura automática, consulta local, busca PT/EN limitada,
rebuild e apagamento total. Os testes de produto confirmam que o worker, Reader,
sessões e proteção de navegação privada estão ligados ao binário que embarca.

### Release

Ainda não está liberada. Ordem recomendada:

1. corrigir e revisar o #89;
2. integrar o #89 e obter `main` verde nos três comandos normativos;
3. atualizar/revalidar e integrar o #88;
4. confirmar CI pós-merge verde;
5. atualizar `CHANGELOG`, versão e artefatos a partir do `main` final;
6. obter autorização explícita do dono para o bump/release.

## 15. Referências de implementação

- `crates/neural-core/src/memory.rs` — captura, consulta, forget, tombstones,
  doctor, rebuild e manifesto;
- `crates/neural-core/src/memory/sqlite_v01.rs` — schema, transações, FTS,
  embeddings e swap atômico;
- `crates/neural-core/src/memory/schema_v01.sql` — schema SQLite V01;
- `crates/neural-core/src/local_intelligence.rs` — embedding local e grupos
  semânticos PT/EN;
- `crates/neural-app/src/windows_app.rs` — worker, captura do produto, comandos e
  apresentação dos resultados;
- `crates/neural-core/tests/spec_0107_phase1_acceptance.rs` — fluxo completo da
  Fase 1;
- `crates/neural-core/tests/semantic_recall.rs` — recall PT/EN;
- `crates/neural-core/tests/forget_leaves_no_trace.rs` — exclusão material dos
  bytes;
- PR #88 — gate determinístico da captura;
- issue #89 — equivalência de caminhos Windows no teste do instalador.
