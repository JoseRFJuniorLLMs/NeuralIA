# GPT-auditoria.md — Auditoria cruzada do NeuralIA v2.7.1

**Data:** 2026-10-01  
**Auditor:** GPT-5.6 Sol  
**Alvo:** `main@83c9048617e4276bcd0f2628dc8df22396dda02b`  
**Código auditado:** v2.7.1, cujo último commit de código é `f5b711e9a718ee1b5e0904765c94b43d2dfb09c5`  
**Relatórios cruzados:** `GEMINI-auditoria.md` e `Opus-auditoria.md`

> Entre `f5b711e` e `83c9048` existem somente dois arquivos novos: as auditorias Gemini e Opus. Não houve mudança de código Rust nesse intervalo. Portanto, os três relatórios estão avaliando a mesma base executável.

---

## 1. Escopo e método

Esta auditoria não reutiliza como verdade as conclusões dos dois relatórios anteriores. O procedimento foi:

1. ler integralmente `GEMINI-auditoria.md` e `Opus-auditoria.md`;
2. enumerar a árvore Rust atual da `main`;
3. varrer **todos os 120 arquivos Rust first-party de produção** e aprofundar manualmente os módulos de maior risco;
4. cruzar cada afirmação relevante das auditorias com a implementação atual;
5. separar:
   - defeito comprovável;
   - risco arquitetural / de hardening;
   - otimização opcional;
   - afirmação não demonstrada ou falso positivo;
6. ler especialmente as superfícies de confiança: updater, IPC/WebView, agentes/MCP, egress, secrets/DPAPI, downloads, ZIP/EPUB, LLM transport, SQLite/memória, model packs e instalador.

### Cobertura real da árvore

| Métrica | Resultado |
|---|---:|
| Arquivos `.rs` totais | **156** |
| Rust first-party em `crates/` | **150** |
| Rust first-party de produção | **120** |
| Arquivos Rust de teste separados | **29** |
| Rust vendorizado | **3** |
| Rust em scripts | **3** |
| Produção `neural-app` | **66 arquivos** |
| Produção `neural-core` | **46 arquivos** |
| Produção `neural-setup` | **8 arquivos** |
| Tamanho do Rust first-party de produção | **4.383.982 bytes** |
| Tamanho dos arquivos Rust de teste separados | **1.919.271 bytes** |

Isso muda materialmente a leitura da auditoria Opus: ela se declara “exaustiva” sobre **25 arquivos Rust**, mas a mesma árvore executável possui **120 arquivos Rust first-party de produção**. Os 25 analisados cobrem pontos importantes, porém não constituem uma auditoria exaustiva do produto.

Também não encontrei `todo!` / `unimplemented!` evidentes em produção na varredura. Contagens cruas de `unwrap`, `expect` e `panic!` são enganosas aqui porque muitos módulos carregam seus testes em `#[cfg(test)]` no mesmo arquivo.

### Limitação desta execução

A revisão foi estática, sobre a árvore do GitHub. Não executei `cargo fmt`, `cargo clippy`, `cargo test` nem E2E Windows nesta sessão. O commit documental `83c9048` também não expôs checks/statuses pelo conector utilizado. Portanto, este relatório **não repete** a frase “CI verde” sem evidência observada nesta execução.

---

# 2. Resultado executivo

Não encontrei evidência de um vazamento crítico generalizado, RCE vindo de página web, bypass óbvio do IPC ou quebra direta do cofre DPAPI. Ao contrário: as superfícies de IPC, agentes locais, downloads hostis, ZIP e transporte LLM apresentam várias defesas concretas e bem testadas.

O principal problema que as duas auditorias anteriores deixaram passar está no **atualizador automático**.

## ALTO-01 — cadeia de confiança do auto-update é incompleta no cliente

O cliente:

1. consulta a release `latest` no GitHub;
2. aceita o **primeiro asset cujo nome termina em `.exe`** (`neural-core/src/update.rs:63-66`);
3. baixa esse executável para `%TEMP%`;
4. quando o download termina, dispara `Command::new(&installer_path).spawn()` (`windows_app/app/event_loop.rs:207`);
5. o check diário chama o fluxo de aplicação automaticamente (`windows_app/app/chrome.rs:491-502`).

No caminho runtime auditado **não há verificação independente do instalador baixado antes da execução** por:

- SHA-256 esperado;
- campo `digest` do asset;
- provenance/attestation;
- assinatura Authenticode e identidade esperada do signatário;
- nome exato `NeuralIA-Setup-<versão>-x64.exe`.

O pipeline de release é muito melhor que o cliente: `SPEC-0013` exige checksum, SBOM, provenance e pode assinar o instalador. Porém, **produzir provenance no CI não significa que o updater o verifica no computador do usuário**.

A própria `SPEC-0016` ainda registra “explicit update metadata/policy” como trabalho restante. Isso combina com o que o código mostra: há mecanismo de download/aplicação antes de existir uma política de confiança runtime equivalente à qualidade do pipeline.

### Impacto

Não é “qualquer site executa código”. A fonte normal é o repositório oficial via HTTPS. O problema é de **supply chain e fail-closed**: o cliente transfere a autoridade do repositório/release diretamente para execução local sem uma segunda verificação criptográfica/política no endpoint.

### Correção recomendada

Antes de considerar o auto-update fechado:

- aceitar somente o asset de nome exato derivado da versão;
- validar esquema/host do URL recebido;
- carregar metadado de integridade verificável e conferir o arquivo antes de executar;
- quando houver certificado de produção, verificar Authenticode e o signatário esperado;
- idealmente verificar a attestation/provenance ou um manifesto assinado;
- falhar fechado em qualquer discrepância;
- tornar explícita a política de auto-aplicação/consentimento;
- incluir sabotagem que troca o primeiro asset por `evil.exe` e exige recusa.

**Prioridade: alta.**

---

## ALTO-02 — “verificação diária” atualmente também baixa e aplica

`check_daily_update()` parece, pelo nome e comentário, apenas verificar diariamente, mas chama:

`self.check_and_apply_update(false)`

em `windows_app/app/chrome.rs:491-502`.

Ao receber `UpdateAvailable`, o event loop inicia o download; ao receber `UpdateReady`, executa o instalador e pede o fechamento da aplicação.

Isso é tecnicamente coerente com um auto-updater, mas a política está implícita no código e a roadmap ainda diz que “explicit update metadata/policy” está pendente.

**Recomendação:** decidir e documentar uma das duas políticas:

- check automático + consentimento para instalar; ou
- update realmente automático, mas somente depois de ALTO-01 e com política normativa explícita.

---

# 3. Bugs confirmados no updater

## MÉDIO-01 — comparador “SemVer 2.0.0” não implementa SemVer 2.0.0

Opus acertou o caso `rc9` vs `rc10`, mas o problema é maior.

Em `neural-core/src/update.rs:86-110`, o pré-release é comparado como string inteira:

`cand_p.cmp(cur_p)`

Isso viola a ordenação SemVer por identificadores.

Além disso:

- build metadata (`+build`) não é separado;
- `2.7.1+abc` pode fazer o patch deixar de ser parseado e cair para zero;
- componentes inválidos viram silenciosamente zero;
- somente três componentes numéricos são considerados;
- o parser aceita formas que um parser SemVer estrito recusaria.

Exemplos que devem virar testes de regressão:

- `2.7.0-rc.9 < 2.7.0-rc.10`;
- `2.7.0-alpha.2 < 2.7.0-alpha.10`;
- `2.7.1+build.1 == 2.7.1+build.9`;
- versão inválida deve ser erro, não `0.0.0` implícito.

**Recomendação:** usar parser SemVer estrito ou implementar a gramática/precedência completa e testá-la por tabela.

---

## MÉDIO-02 — asset escolhido por extensão, não por contrato

`parse_github_release_json` seleciona:

> o primeiro asset que termina em `.exe`

O contrato de release diz que o asset público deve ser exatamente `NeuralIA-Setup-<version>-x64.exe`. O updater deveria validar esse contrato em vez de presumir que o produtor sempre o respeitará.

Esse defeito reforça ALTO-01.

---

## MÉDIO-03 — download parcial fica com o nome final

Opus está correto aqui.

`download_installer` abre diretamente o destino com `File::create(target_path)` (`update.rs:183`). Se leitura, escrita ou flush falhar, a função retorna erro sem remover o arquivo parcial.

Problemas adicionais:

- o mesmo caminho final é reutilizado por versão;
- não há padrão “`.part` + flush + rename atômico”;
- não há pós-condição criptográfica sobre os bytes gravados.

**Correção:** arquivo temporário exclusivo, cleanup em erro, `sync_all`/flush adequado, verificação de integridade e somente então rename para o nome executável final.

---

## BAIXO-01 — dois `ureq::Agent` por ciclo

Confirmado: `Agent::new_with_defaults()` aparece em `update.rs:149` e `:175`.

É desperdício pequeno. Não é um problema arquitetural comparável aos itens acima.

---

## BAIXO-02 — progresso do updater gera eventos por chunk

Opus está correto.

O downloader chama o callback a cada chunk de 32 KiB. O event loop então recalcula a barra, altera splash/status e pede redraw (`event_loop.rs:155-196`).

A UI só precisa de mudanças observáveis, por exemplo:

- mudança de percentual inteiro; ou
- intervalo de 100–250 ms.

O subsistema normal de downloads já possui `PROGRESS_INTERVAL = 250 ms`; o updater deveria adotar uma política semelhante.

---

# 4. Cruzamento com GEMINI-auditoria.md

| Afirmação Gemini | Veredicto GPT | Evidência |
|---|---|---|
| buffer fixo de 32 KiB + limite 150 MiB no updater | **Confirmado** | `update.rs:189+` |
| “zero leak GDI” | **Não demonstrado** | há RAII/limpeza em muitos caminhos, mas uma afirmação absoluta exige instrumentação/GDI counters e não apenas inspeção pontual |
| cache de tema 500 ms | **Incorreto** | `THEME_CACHE_TTL = Duration::from_secs(1)` em `theme.rs:224` |
| SQLite “pool limitado” | **Incorreto** | o módulo trabalha com `rusqlite::Connection`; não há pool nessa implementação |
| `WM_ERASEBKGND` tratado | **Confirmado** | desenho Win32 possui caminho dedicado |
| “zero flicker” | **Não sustentado** | permanecem várias invalidações com `bErase=TRUE` |
| adblock O(1) | **Simplificação excessiva** | lookup por hash ajuda, mas há trabalho por labels/sufixos; não é literalmente custo constante para qualquer domínio |
| SemVer eficiente | **Custo baixo, correção incompleta** | parser aloca pouco, mas a precedência SemVer está errada |
| código “sem caminhos mortos” | **Não auditável por afirmação absoluta** | há diversos blocos planejados/feature-gated e `allow(dead_code)` intencionais |
| “todos os gates CI passaram” | **Não revalidado nesta sessão** | não foi executado CI local; commit documental atual não expôs statuses pelo conector |

### Conclusão sobre Gemini

A auditoria Gemini encontrou alguns pontos reais, especialmente no desenho Win32, mas encerra com conclusões absolutas demais para a evidência apresentada. “Zero leak”, “zero flicker” e “100%” não são conclusões que uma leitura estática parcial sustenta.

---

# 5. Cruzamento com Opus-auditoria.md

| Achado Opus | Veredicto GPT | Prioridade revisada |
|---|---|---|
| arquivo parcial do update não removido | **Confirmado** | Média |
| dois `ureq::Agent` | **Confirmado** | Baixa |
| `splash_buttons` aloca `Vec<RECT>` | **Confirmado** (`splash.rs:63+`) | Baixa |
| splash clona texto em WM_PAINT | **Confirmado** (`splash.rs:153`) | Baixa |
| `InvalidateRect(..., 1)` | **Confirmado em múltiplos pontos** | Média/baixa, medir antes/depois |
| evento de update por chunk | **Confirmado** | Baixa/média |
| `exit_now` cria WebView2 “órfão” | **Não demonstrado** | Rebaixado |
| duas alocações no lookup adblock | **Plausível/confirmável, mas micro** | Baixa |
| SemVer `rc9`/`rc10` | **Confirmado, e há mais casos** | Média |
| `PORTUGUESE_WORDS.contains` linear | **Confirmado**, mas lista atual tem ~50 entradas, não ~200 | Baixa |
| `pretoken_count` torna packing O(N² log N) | **Não confirmado / contradito pelo desenho atual** | Retirar como bug |
| `sentence_spans` materializa `Vec<(usize,char)>` | **Confirmado** | Baixa |
| `cut_victims` pior caso elevado | **Teórico e com N pequeno** | Informação |
| `draw_icon` aloca pixels por pintura | **Confirmado** (`icons.rs:163`) | Baixa |
| EPUB pode ter picos de memória | **Candidato a profiling, não defeito provado aqui** | Informação |
| `step *= 2` poderia overflow | **Teórico, sem cenário realista dado os limites de memória do Vec** | Não prioritário |

## Por que o O(N² log N) do Opus não fecha

A implementação atual documenta e implementa `pack_units` com busca exponencial + bissecção (`context_budget.rs:1113+`) e o próprio `chunk_spans` registra o objetivo de O(n log n) em bytes medidos (`:1072-1077`).

`pretoken_count` realmente materializa um `Vec<char>` por medição (`:375-380`), então há espaço para otimização de alocação. Mas isso **não transforma automaticamente** a estratégia de cortes em O(N² log N). O número/tamanho agregado das medições é justamente o que a busca exponencial e os gates CB-9 tentam limitar.

Portanto:

- “há alocação evitável” = correto;
- “há regressão algorítmica O(N² log N)” = não demonstrado.

## Por que o “WebView órfão” foi rebaixado

`exit_now` chama `event_loop.exit()` sem `destroy_web_surfaces()` (`downloads_ui.rs:1608-1610`), mas o ciclo principal é:

`event_loop.run_app(&mut app)?; Ok(())`

Quando o event loop termina, `App` sai de escopo e seus campos são destruídos. Além disso, `ApplicationHandler::exiting` ainda finaliza model packs, salva abas e aguarda downloads.

Pode haver valor em destruir WebViews explicitamente antes da saída por previsibilidade/lifecycle telemetry, mas “vazamento órfão” precisa de uma prova runtime que o relatório Opus não forneceu.

---

# 6. Pintura Win32: problemas reais, mas não devem dominar a fila

A Opus está correta ao apontar chamadas `InvalidateRect(..., TRUE)` em produção. Foram localizadas em módulos como:

- `windows_app.rs`;
- `native.rs`;
- `toast.rs`;
- `app/chrome.rs`;
- `app/split.rs`;
- `app/tools.rs`;
- `app/panels.rs`;
- `secret_prompt.rs`;
- `translation.rs`;
- `downloads_ui.rs`;
- `app/event_loop.rs`.

Se a rotina de `WM_PAINT` já cobre integralmente o fundo, `FALSE` evita erase extra. Isso merece gate visual/medição, mas não é comparável à cadeia de confiança do updater.

Também são reais:

- `splash_buttons()` criando `Vec<RECT>` para 0–2 botões;
- clone do texto do splash durante `WM_PAINT`;
- buffer BGRX novo em `draw_icon()`.

São microcustos em caminho quente. Só devem virar refactor se profiling ou o gate de ciclos apontar benefício mensurável. Trocar código simples por caches complexos sem medir seria a forma clássica de corrigir 200 ns criando um bug de três dias.

---

# 7. Superfícies de segurança que as auditorias anteriores subestimaram

## 7.1 Agentes/MCP e named pipe: desenho forte

`agents/pipe.rs` merece destaque positivo:

- nome de pipe com identificador aleatório;
- `FILE_FLAG_FIRST_PIPE_INSTANCE`;
- `PIPE_REJECT_REMOTE_CLIENTS`;
- DACL protegida para o usuário atual;
- token de 256 bits;
- arquivo token/pipe com ACL privada;
- verificação do owner do pipe antes de entregar token;
- autenticação inicial com timeout de 5 s;
- limite de instâncias;
- tamanho máximo de requisição/resposta.

`agents/mcp.rs` complementa com:

- mensagens limitadas a 1 MiB;
- no máximo 8 calls inflight;
- IDs duplicados recusados;
- negociação de protocolo/capabilities;
- cancelamento;
- timeouts no `ask_user`;
- encerramento coordenado.

Isso reduz bastante a superfície de um “MCP local = shell aberto”, que seria uma tragédia surpreendentemente comum.

## 7.2 IPC WebView: autenticação/caps/bounds consistentes

`ipc.rs`:

- `IPC_MAX_BYTES = 8 KiB`;
- capability de 32 caracteres;
- lista fechada de ações;
- chaves/argumentos exatos;
- limites de texto/páginas/colunas;
- URL validada;
- alvo de rede local recusado em ações de link/split.

Os testes incluem capability errada em posições diferentes e URLs locais. É um ponto forte real.

## 7.3 Secrets/DPAPI

`secrets.rs`:

- DPAPI com entropia por uso;
- tamanho máximo de arquivo secreto;
- buffers apagados;
- arquivo com magic;
- validação de formato das chaves;
- redator central de logs.

Não encontrei no fluxo lido uma chave sendo colocada em URL do transporte LLM.

## 7.4 Transporte LLM

`llm/transport.rs` possui política mais rigorosa que o updater:

- `proxy(None)` para impedir proxy de ambiente capturando chave;
- `max_redirects(0)`;
- endpoint pinned;
- resolução pública que rejeita loopback/LAN/link-local;
- chave somente em header;
- limite do corpo decodificado;
- deadline/cancelamento.

Essa diferença é justamente por que o updater merece hardening próprio.

## 7.5 Downloads e ZIP hostil

`file_risk.rs`, `downloads.rs` e `safezip.rs` formam uma defesa bem acima do básico:

- bloqueio/aviso por tipos executáveis, scripts, shell shortcuts, imagens de disco e masquerading;
- sniff dos bytes iniciais;
- inspeção de ZIP sem extrair conteúdo;
- limites de entradas, tamanhos e central directory;
- travessia de caminho recusada;
- ZIP encryption/métodos estranhos recusados;
- detecção de sobreposição;
- MOTW para arquivos mantidos;
- download privado excluído do histórico.

Isso é materialmente mais relevante que várias micro-otimizações dos relatórios anteriores.

---

# 8. Arquitetura “egress”: boa, mas ainda não universal

`egress.rs` implementa um portão cuidadoso:

- `PrivateMode` bloqueia destino que sai do PC;
- classes de dados;
- consentimento de sessão/site;
- grant persistente canônico por site;
- limites de uso;
- background somente com grant e `UserTyped`.

Entretanto, o próprio `main.rs:69-77` deixa claro: **Tradução é a primeira feature a usar o portão; as próximas ainda virão**.

A busca de construções `EgressRequest` na árvore confirma o wiring de produto em `windows_app/translation.rs` e no próprio módulo/testes.

Conclusão: o mecanismo é bom, mas não deve ser descrito como “toda saída de IA passa pelo egress gate” ainda. Isso seria confundir arquitetura preparada com cobertura efetiva.

---

# 9. Correções factuais adicionais

## SQLite

Gemini fala em “pool limitado”. A implementação `memory/sqlite_v01.rs` configura `rusqlite::Connection` e abre/conduz conexões conforme as operações. Não identifiquei um pool de conexões nessa implementação.

Isso não significa automaticamente que SQLite esteja ruim: WAL, busy timeout, integridade/rebuild e transações estão presentes. Apenas a descrição “pool” está errada.

## Tema

Opus corrige Gemini corretamente:

`THEME_CACHE_TTL = Duration::from_secs(1)`

em `windows_app/theme.rs:224`.

## Detecção de português

`PORTUGUESE_WORDS.contains(&lower)` é linear, mas a lista atual tem cerca de cinquenta marcadores e a amostra é limitada a 4.000 palavras. Transformar isso em `HashSet` global pode ser razoável, porém classificar como gargalo médio sem profiling é exagero.

---

# 10. Ordem recomendada de correção

### P1 — antes da próxima release com auto-update confiável

1. fechar ALTO-01: verificação runtime do instalador;
2. exigir nome exato do asset;
3. definir política explícita de atualização;
4. trocar o parser SemVer por implementação estrita;
5. baixar em arquivo temporário transacional e limpar falhas;
6. criar testes/sabotagens para asset errado, hash errado, assinatura/digest errado e SemVer com pré-release/build metadata.

### P2 — robustez/performance de baixo risco

7. coalescer `UpdateProgress`;
8. revisar `InvalidateRect(..., TRUE)` com teste visual/gate;
9. eliminar as alocações de pintura somente onde medição provar ganho;
10. reaproveitar `ureq::Agent` no ciclo de update.

### P3 — manter como observabilidade, não “bug” ainda

11. medir memória EPUB em fixture grande;
12. medir GDI handles em ciclo longo antes de afirmar “zero leak”;
13. medir shutdown de WebView antes de alterar `exit_now`;
14. manter benchmarks de `context_budget` e não reescrever o algoritmo baseado no falso positivo O(N² log N).

---

# 11. Gates novos que eu adicionaria

- **`updater_rejects_wrong_asset_name`**
- **`updater_verifies_downloaded_bytes_before_execute`**
- **`updater_never_executes_on_integrity_failure`**
- **`updater_partial_download_is_not_executable_and_is_cleaned`**
- **`semver_precedence_table_matches_2_0_0`**
- **`semver_build_metadata_does_not_change_precedence`**
- **`update_progress_is_coalesced`**
- sabotage: primeiro asset vira `evil.exe` → gate vermelho;
- sabotage: um byte do instalador muda → gate vermelho;
- sabotage: `rc.10` volta a ordenar abaixo de `rc.9` → gate vermelho;
- sabotage: remover verificação de integridade antes do `Command::spawn` → gate vermelho.

---

# 12. Veredicto final

O NeuralIA v2.7.1 está, em várias superfícies sensíveis, **mais defensivo do que as duas auditorias anteriores deixam transparecer**. IPC, named pipe/MCP, DPAPI, LLM transport, downloads e ZIP hostil têm controles concretos e numerosos gates.

Ao mesmo tempo, a conclusão “SOTA / zero leak / zero flicker / tudo verde” da Gemini é ampla demais, e a Opus, embora tecnicamente muito melhor, mistura defeitos reais com micro-otimizações e pelo menos dois diagnósticos que não consegui sustentar (`context_budget` O(N² log N) e WebView “órfão”).

A lacuna que merece prioridade é outra: **o pipeline de release tem provenance e integridade, mas o auto-updater cliente ainda não fecha essa cadeia antes de executar o instalador recebido**.

Esse é o ponto em que eu colocaria o próximo ciclo de engenharia antes de gastar tempo transformando um `Vec<RECT>` de dois elementos em obra de arquitetura.

---

## Apêndice A — arquivos de maior risco aprofundados

Além da varredura dos 120 arquivos de produção, foram aprofundados diretamente, entre outros:

- `crates/neural-core/src/update.rs`
- `crates/neural-core/src/context_budget.rs`
- `crates/neural-core/src/translate.rs`
- `crates/neural-core/src/safezip.rs`
- `crates/neural-core/src/file_risk.rs`
- `crates/neural-core/src/downloads.rs`
- `crates/neural-core/src/llm/transport.rs`
- `crates/neural-core/src/memory/sqlite_v01.rs`
- `crates/neural-app/src/agents/{hub,mcp,pipe,store,tools}.rs`
- `crates/neural-app/src/egress.rs`
- `crates/neural-app/src/ipc.rs`
- `crates/neural-app/src/secrets.rs`
- `crates/neural-app/src/gemini_live.rs`
- `crates/neural-app/src/local_models.rs`
- `crates/neural-app/src/windows_app.rs`
- módulos `windows_app/*` e `windows_app/app/*`
- `crates/neural-setup/src/{app,archive,install,winshell}.rs`
- `AGENTS.md`
- `SPEC-0013` e trechos relevantes da `SPEC-0016`.

## Apêndice B — governança

Conforme `AGENTS.md`, esta auditoria foi criada em branch própria `docs/*` e deve integrar por Pull Request. Nenhum código, versão ou release foi alterado por esta auditoria.
