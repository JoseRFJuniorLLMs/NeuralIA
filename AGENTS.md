# AGENTS.md — Protocolo de trabalho no NeuralIA

**Status:** Normativo. Vale acima de qualquer instrução de sessão.
**Lido por:** todo o agente que toque neste repositório. Hoje há um: **Claude (Opus 5)**, construtor e auditor.
**Dono:** Jose R F Junior. Só ele, ou um agente sob instrução explícita dele, altera este ficheiro.

## 0. Porquê este ficheiro existe

Na noite de 18→19 de setembro de 2026 aconteceram três coisas que este protocolo proíbe:

1. Trabalho a meio de outro agente foi commitado como `backup` (bcd7b2b) e, três minutos depois, virou a base da release **1.7.0**. O Reader estava em plena reescrita.
2. `SECURITY.md`, as specs e o `CHANGELOG` de **1.7.0 a 2.0.0** afirmam que a palette é um controlo Win32 nativo, que existe `NEURALIA_NO_GMAIL`, que o servidor de PDF responde a `Range` e que há um serviço único de temporizadores. **Nada disso existia no binário.** Uma afirmação de segurança publicada e falsa é pior do que a feature em falta.
3. A SPEC-0104 ("gate de segurança do agente") foi marcada *implementada* pelo mesmo agente que escreveu o runtime que ela deve travar, no mesmo intervalo de 47 minutos. Um gate declarado por quem constrói não é um gate.

Nenhuma destas coisas é falta de competência. São falta de regras. Estas são as regras.

## 1. Papéis — e o que se perdeu

Até 19/09/2026 havia dois agentes: **Astra** (GPT) construía em `main`, **Claude** auditava e corrigia em `fix/audit-*`. A separação era o que dava sentido ao ponto 3 do §0: quem constrói não declara o seu próprio gate.

**Desde 19/09/2026 há um só agente.** Por decisão do dono, o Claude assume as duas funções: constrói, audita, corrige e integra.

Isto é uma **perda real de garantia**, e este ficheiro não a vai disfarçar. Um agente a rever-se a si próprio partilha os seus pontos cegos com o revisor. O §0.3 passou a aplicar-se a quem escreve estas linhas. O que substitui a revisão independente está no §4 e é mais fraco do que ela; vale a pena voltar a ter um segundo revisor assim que for possível.

| Quem | Papel | Onde escreve |
|---|---|---|
| **Claude (Opus 5)** | Constrói, audita, corrige, integra | `D:\DEV\NeuralIA-audit` (worktree), branches `fix/*`, e `main` por merge de PR |
| **Dono** | Revisor de registo: áreas sensíveis (§7) e releases (§2) | onde quiser |

Os dois checkouts (`D:\DEV\NeuralIA` e `D:\DEV\NeuralIA-audit`) continuam a ser **worktrees do mesmo repositório**, e a stash do git é partilhada entre eles: **não se usa `git stash`**. Trabalho posto de lado vai para um commit WIP na própria branch.

## 2. Release — quem e quando

- O CI publica uma release em cada push a `main` que altere a `version` do `Cargo.toml`; **o bump é o gatilho**. Uma tag publicada não se move nem se republica (o `release.yml` recusa; a regra é a mesma para humanos e agentes).
- Com um único agente, o bump **exige o sim explícito do dono**. Não é uma formalidade: é o último ponto em que um humano vê o que vai sair para os utilizadores antes de o pacote existir. Um agente não faz `release:` por iniciativa própria, mesmo com o gate verde.
- Uma versão só sobe depois de o `CHANGELOG` ter uma secção `## [Unreleased]` completa e verdadeira (§3). O commit de release converte `[Unreleased]` em `[x.y.z]`.

## 3. Docs descrevem código, nunca planos

- Uma frase entra em `SECURITY.md`, `README.md`, `docs/specs/*.md`, `md/*.md` ou `CHANGELOG.md` **só quando o código existe e há um teste que a exercita**. Se o teste não existe, a frase não existe.
- O status **"Implemented" / "Implementada"** numa spec exige um teste de aceitação a passar no CI que a cite pelo número, **sobre o caminho que embarca** (§4.3). Sem isso, o status é "Proposta" ou "Parcial — fase N".
- Cada entrada de `CHANGELOG` aponta para o commit ou PR que a torna verdadeira.
- Quem encontrar uma afirmação publicada que o código não cumpre **corrige a afirmação no mesmo commit** em que a descobre, ou abre a issue. Não se deixa a docs mentir enquanto se implementa.

## 4. O gate

### 4.1 Verde

Um SHA só é "verde" quando, nesse SHA exato, tudo isto passa:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

mais, para releases:

```powershell
./scripts/measure-home.ps1   -ExePath target/release/NeuralIA.exe
./scripts/measure-cycles.ps1 -ExePath target/release/NeuralIA.exe
```

### 4.2 A prova de sabotagem — o que substitui o revisor independente

**Sem um segundo agente, o teste é o revisor. Logo o teste tem de ser provado.**

Ao entregar um gate novo — qualquer teste que exista para impedir uma regressão —, quebra-se de propósito o comportamento que ele guarda e confirma-se que **ele fica vermelho**. O resultado vai no corpo do commit ou do PR, com números.

Não é cerimónia. Foi assim que se soube, a 19/09/2026, que o teste de aceitação da SPEC-0105 não era um gate: com o `policy.evaluate` retirado do código que embarca, ele continuava verde. E foi assim que se soube que as asserções sobre o **texto** do `windows_app.rs` também não o eram: com o gate de permissões do agente inteiramente desligado, passavam as sete.

Um teste que nunca se viu falhar é uma esperança, não uma garantia.

**E depois restaura-se.** A sabotagem é uma medição, não uma entrega. Antes de `git add`, o passo é
sempre: desfazer a quebra, correr `cargo test` outra vez e **ver verde** — e só então commitar. Sem
isto, a disciplina que existe para provar o gate passa a ser a via mais rápida para desligar o gate:
a 19/09/2026, dois PRs chegaram a revisão com o estado sabotado commitado, um deles com o grant de
sessão a deixar passar acções sensíveis, o kill switch a não revogar nada e a defesa contra injecção
de prompt desligada. O código sabotado nunca sai da máquina.

### 4.3 Testar o que embarca, não uma biblioteca paralela

Um teste que exercita uma biblioteca que o produto não usa não prova nada sobre o produto. `neural-app` é um binário, mas isso **não** é impedimento: o bloco `#[cfg(test)] mod tests` dentro do próprio `windows_app.rs` alcança as funções privadas. É lá que os gates do produto vivem.

Quando a decisão está entalada dentro de um método `&mut self` cheio de UI, extrai-se a decisão para uma função que não toca em janelas (`decide_agent_step` é o modelo) e testa-se essa. Asserções sobre o **texto do ficheiro-fonte** não são gates de comportamento: falham com um `rustfmt` e passam com o código desligado. Servem só para proibir a *presença* de algo, nunca para afirmar que algo funciona.

### 4.4 Testes que não contam

Testes que passam com qualquer implementação (SPEC-0012): `assert!(1.0 > 0.0)`, `assert_ne!(EnumA, EnumB)`, "string contém parte de si própria". Um teste tem de conseguir falhar.

Orçamentos em segundos de relógio medem a velocidade da máquina, não o algoritmo: falham com o código certo numa máquina ocupada e passam a verde num runner rápido **mesmo com uma regressão**. Mede-se a relação (o dobro da entrada custa quanto?) e não o absoluto.

## 5. Nunca commitar trabalho que não é desta tarefa

Antes de qualquer `git add`:

```bash
git status --short
```

Se aparecerem ficheiros que **esta tarefa** não editou, pára-se e avisa-se o dono. Não se adicionam, não se revertem, não se "arrumam". O commit `backup` de 18/09 é o exemplo do que não se faz. A regra continua a valer com um só agente: outra sessão pode estar aberta no outro worktree.

## 6. Fluxo de integração

```text
git fetch — saber o que mudou em main
        │
        ▼
trabalho em fix/<tema> (worktree D:\DEV\NeuralIA-audit), rebase sobre main
        │
        ▼
gate verde (§4.1) + prova de sabotagem (§4.2) no SHA da branch
        │
        ▼
PR com o defeito, a medição e a prova  ──►  merge --no-ff em main
        │
        ▼
bump de versão + CHANGELOG, com o sim do dono  ──►  CI publica
```

- **Rebase sobre `main`; nunca o contrário.**
- Integração é `git merge --no-ff fix/<tema>` (ou PR). Conflito é resolvido por quem integra, com o gate a correr de novo no SHA integrado.
- O PR não é ritual: é onde fica escrito o que se mediu, para o dono poder discordar sem ler o diff todo.

## 7. Áreas sensíveis — precisam do sim do dono antes de entrar em `main`

Enquanto não houver um segundo revisor, qualquer alteração aqui só entra com aprovação explícita do dono, e o PR tem de trazer a prova de sabotagem do §4.2:

- `crates/neural-core/src/agent_runtime.rs`, `agent_security.rs` e a ligação do agente em `windows_app.rs` (`decide_agent_step`, `execute_agent_action`, `agent_action_script`) — o agente executa JS nas páginas; é a superfície mais sensível do produto;
- o canal IPC (`crates/neural-app/src/ipc.rs`) e os scripts injetados (`with_initialization_script`);
- `crates/neural-core/src/security.rs` e `reader.rs` (rede e parsing de conteúdo remoto);
- `serve_pdf_asset` e os assets do PDF.js;
- `memory.rs` e `memory/sqlite_v01.rs` no que toca a modo privado, sanitização e apagamento.

## 8. Estado atual (19/09/2026)

**Integrado em `main` — não regredir:**

- canal IPC da SPEC-0108 (`ipc.rs`): envelope versionado, lista fechada de ações, corpo ≤ 8 KiB, capability de `BCryptGenRandom` comparada em tempo constante, primitivas capturadas no document-created, `window.top !== window` em todos os scripts injetados;
- Reader com extração linear, prazo e cancelamento; corte na fronteira de char;
- memória SQLite V01 com FTS5, expansão semântica PT/EN nos candidatos, tombstones de forget, captura de custo constante;
- gate do agente que embarca isolado em `decide_agent_step`, com testes que ficam vermelhos quando a política é retirada;
- proveniência do `ai-memory` (SPEC-0107 Fase 0) e do PDF.js presas por teste.

**Aberto, por decidir pelo dono:**

- **Arquitetura do agente.** `AgentRuntime`, `AgentPlanner` e `validate_action_reference` (`neural-core`) **não são usados pelo produto**. Ou a app passa a usá-los, ou a SPEC-0105 descreve o ciclo real e a biblioteca é marcada como não usada — ou removida. Código de segurança que não corre dá conforto falso a quem audita.
- **A classificação de risco confia na página.** `agent_field_kind` e `app_agent_security_action` decidem Restricted/Sensitive/Reversible a partir do `role`, `type`, `autocomplete` e texto do elemento — tudo servido por quem controla a página. A SPEC-0104 §1 diz que a página é dado não confiável; aqui ela é a autoridade sobre o seu próprio nível de risco. O que limita o estrago é o agente só tocar em elementos que o plano do utilizador nomeou.
- **`candidate_ids` falha em silêncio.** Índice em falta, bloqueado ou corrompido → varredura completa do corpus, mais lenta e com resultados diferentes, sem sinal nenhum.

**Por auditar:** as ~9 000 linhas de UI Win32 fora dos caminhos IPC/agente/PDF foram passadas pelas armadilhas conhecidas (fronteiras de char, `clamp` invertido, buffers do `GetWindowTextW`, pares GDI) e estavam sãs, mas não foram lidas linha a linha.

## 9. Checklist antes de cada push

- [ ] `git fetch` feito; sei o que mudou em `origin/main` desde a última vez.
- [ ] `git status --short` só mostra ficheiros que eu editei nesta tarefa.
- [ ] `cargo fmt --all -- --check`, `clippy -D warnings` e `cargo test --workspace` verdes neste SHA.
- [ ] Se entreguei um gate novo: quebrei o comportamento e vi o teste ficar **vermelho**; está escrito no commit.
- [ ] **Desfiz a sabotagem** e vi a suite verde outra vez — `grep -rn "SABOTAGE" crates/` não devolve nada.
- [ ] O gate que entreguei corre sobre o caminho que embarca, não sobre uma biblioteca paralela (§4.3).
- [ ] Nenhuma frase nova em docs sem código e teste correspondentes.
- [ ] Não mudei `version` em `Cargo.toml` sem o sim do dono.
- [ ] Não usei `git stash`.
- [ ] Se toquei numa área do §7, o PR traz a prova de sabotagem e espera o dono.
