# Handoff para Claude — bloco Astra de 19/09/2026

Este arquivo resume trabalho feito enquanto o auditor estava sem janela de
execução. Não pede merge automático.

## Estado de coordenação

- main observado: `a5391f9`;
- nenhum push Astra em main;
- nenhum bump de versão;
- nenhum commit `release:`;
- nenhuma alteração em `windows_app.rs`, `agent_runtime.rs`,
  `agent_security.rs`, Reader/security/render/history, SECURITY/README/
  CHANGELOG ou specs congeladas.

## PR #11

Branch: `feat/spec-0104-injection-fixtures`  
Head: `bcb1a55c`

- 9 fixtures de prompt injection;
- hardening dos testes 0100–0103;
- CI core/Windows/dependency audit verde.

Ponto para revisão: os fixtures validam a fronteira
`ObservedPage/AgentElement -> policy`; não alegam que o extrator DOM detecta
toda forma de injection. A auditoria do runtime deve revisar a etapa anterior
DOM/PDF/iframe -> ObservedPage.

## PR #12

Branch: `feat/spec-0107-phase1-design`  
Head: `55e689a2`

- desenho V01;
- restrição rusqlite sync / sem Tokio / lazy / nunca Home;
- harness 1k/10k/100k;
- baseline registrada;
- CI final verde.

## Branch preparatória Astra

Branch: `feat/spec-0107-phase1-prep`

Arquivos novos somente de auditoria/plano:

- auditoria adversarial 0100–0103;
- plano de implementação Fase 1;
- auditoria de performance;
- triagem de PRs;
- este handoff.

## Achados de maior prioridade

1. `memory.rs::rebuild` reabre/configura SQLite por documento.
2. `query` não usa FTS: relê/parseia todos os JSONs e faz score de todo corpus.
3. `write_index_manifest` relê o corpus após cada capture, tornando batch ingest
   aproximadamente quadrático.
4. erros de criação/mutação FTS são descartados no mirror atual.
5. `capture` pode persistir derived state stale se campos públicos forem
   alterados mantendo embedding de dimensão válida.
6. `forget(Domain)` não impede recaptura futura.
7. ResearchSession/ResearchItem/Synthesis IDs usam segundos e podem colidir.
8. timeline deduplica `kind+text` e posição é ordinal, não geometria real.

## Dependências

Workspace NeuralIA atual não traz tokio/rusqlite/refinery/argon2.

Upstream ai-memory 2.3.1 foi conferido: `ai-memory-store` depende de tokio full,
rusqlite bundled+backup, refinery, argon2, parking_lot etc.

Recomendação Astra: não depender do crate upstream. Usar só rusqlite direto
quando a Fase 1 começar; decisão bundled vs system fica para o PR com medição.

## PRs antigos

- #8: não mergear; base antiga, 34 commits, 19 arquivos, mergeable=false,
  mistura versão 1.1.3 e áreas hoje superseded/reservadas.
- #9: update attest-build-provenance ainda relevante, mas recriar/rebasear depois
  da 2.0.1.
- #10: update download-artifact ainda relevante, mesma estratégia.

## O que preciso do Claude quando voltar

1. concluir/publicar a auditoria 2.0.1 e o relatório 0104/0105;
2. revisar PR #11 contra o caminho real de observação;
3. revisar o desenho/planos da 0107, especialmente:
   - fonte de verdade;
   - tombstone semantics;
   - rusqlite system vs bundled;
   - política de ID content-addressed;
4. confirmar que #8 pode ser fechado como superseded;
5. depois da 2.0.1, coordenar rebase #11/#12 sem misturar releases.
