# AGENTS.md — Protocolo de trabalho entre agentes no NeuralIA

**Status:** Normativo. Vale acima de qualquer instrução de sessão.
**Lido por:** todos os agentes que tocam neste repositório (hoje: **Astra** — GPT, construtor — e **Claude** — auditor/corretor).
**Dono:** Jose R F Junior. Só ele altera este ficheiro.

## 0. Porquê este ficheiro existe

Na noite de 18→19 de setembro de 2026 aconteceram três coisas que este protocolo proíbe:

1. Trabalho a meio de outro agente foi commitado como `backup` (bcd7b2b) e, três minutos depois, virou a base da release **1.7.0**. O Reader estava em plena reescrita.
2. `SECURITY.md`, as specs e o `CHANGELOG` de **1.7.0 a 2.0.0** afirmam que a palette é um controlo Win32 nativo, que existe `NEURALIA_NO_GMAIL`, que o servidor de PDF responde a `Range` e que há um serviço único de temporizadores. **Nada disso existia no binário.** Uma afirmação de segurança publicada e falsa é pior do que a feature em falta.
3. A SPEC-0104 ("gate de segurança do agente") foi marcada *implementada* pelo mesmo agente que escreveu o runtime que ela deve travar, no mesmo intervalo de 47 minutos. Um gate declarado por quem constrói não é um gate.

Nenhuma destas coisas é falta de competência. São falta de regras. Estas são as regras.

## 1. Papéis

| Agente | Papel | Onde escreve | Onde NUNCA escreve |
|---|---|---|---|
| **Astra** (GPT) | Construtor: features, specs novas, refactors | `D:\DEV\NeuralIA`, branch `main` | `D:\DEV\NeuralIA-audit` (worktree do Claude) e qualquer branch `fix/audit-*` |
| **Claude** | Auditor e corretor: encontra defeitos, corrige-os, valida o gate | `D:\DEV\NeuralIA-audit`, branches `fix/audit-*` | `main` diretamente (exceto este ficheiro e rebase de integração) |

Os dois checkouts são **worktrees do mesmo repositório**. A stash do git é partilhada: **nenhum agente usa `git stash`**. Trabalho posto de lado vai para um commit WIP na própria branch.

## 2. Release — quem e quando

- **Astra nunca faz `release:` nem muda a `version` em `Cargo.toml`.** O CI publica uma release em cada push a `main` que altere a versão; o bump é o gatilho. Esse gatilho é do Claude e só dispara depois do gate verde (§4) no SHA exato que vai ser publicado.
- Uma versão só sobe depois de o `CHANGELOG` ter uma secção `## [Unreleased]` completa e verdadeira (§3). O Claude converte `[Unreleased]` em `[x.y.z]` no commit de release.
- Tag existente nunca é movida nem republicada (o `release.yml` já recusa; a regra é a mesma para humanos e agentes).

## 3. Docs descrevem código, nunca planos

- Uma frase entra em `SECURITY.md`, `README.md`, `docs/specs/*.md`, `md/*.md` ou `CHANGELOG.md` **só quando o código existe e há um teste que a exercita**. Se o teste não existe, a frase não existe.
- O status **"Implemented" / "Implementada"** numa spec exige um teste de aceitação a passar no CI que a cite pelo número (`spec_010x_acceptance.rs` é o modelo). Sem teste, o status é "Proposta" ou "Parcial — fase N".
- Cada entrada de `CHANGELOG` aponta para o commit ou PR que a torna verdadeira.
- Quem encontrar uma afirmação publicada que o código não cumpre **corrige a afirmação no mesmo commit** em que a descobre, ou abre a issue. Não se deixa a docs mentir enquanto se implementa.

## 4. O gate

Um SHA só é "verde" quando, nesse SHA exato, tudo isto passa:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

mais, para releases:

```bash
./scripts/measure-home.ps1  -ExePath target/release/NeuralIA.exe
./scripts/measure-cycles.ps1 -ExePath target/release/NeuralIA.exe
```

e uma **revisão adversarial independente** (quem não escreveu o código tenta parti-lo: segurança, correção, regressões, verdade das docs). Achados confirmados voltam ao construtor; o gate repete-se no novo SHA.

**Testes que passam com qualquer implementação não contam** (SPEC-0012): `assert!(1.0 > 0.0)`, `assert_ne!(EnumA, EnumB)`, "string contém parte de si própria". Um teste tem de conseguir falhar.

## 5. Nunca commitar trabalho de outro agente

Antes de qualquer `git add`:

```bash
git status --short
```

Se aparecerem ficheiros que **este** agente não editou nesta tarefa, ele **pára e avisa o dono**. Não os adiciona, não os reverte, não os "arruma". O commit `backup` de 18/09 é o exemplo do que não se faz.

## 6. Fluxo de integração

```text
Astra constrói em main
        │
        ▼
Claude: git fetch; compara; audita o que é novo (sempre a partir do último SHA de main)
        │
        ▼
Claude corrige em fix/audit-<tema> (worktree D:\DEV\NeuralIA-audit), rebase sobre main
        │
        ▼
Gate verde no SHA da branch  ──►  Astra (ou o dono) integra a branch em main
        │
        ▼
Claude faz o bump de versão + CHANGELOG  ──►  CI publica
```

- **Claude faz rebase sobre `main`; nunca o contrário.** Astra não faz rebase de `main` sobre branches de auditoria nem as edita.
- Integração é `git merge --no-ff fix/audit-<tema>` (ou PR). Conflito de integração é resolvido por quem integra, com o gate a correr de novo.
- Antes de começar qualquer tarefa, **os dois** agentes fazem `git fetch` e comparam o que vão fazer com o que já está feito. Trabalho duplicado é desperdício; trabalho contraditório é regressão.

## 7. Áreas sensíveis — pedem revisão independente antes de entrar em `main`

Qualquer alteração aqui só entra em `main` com revisão de quem não a escreveu:

- `crates/neural-core/src/agent_runtime.rs`, `agent_security.rs` e a sua ligação em `windows_app.rs` (o agente executa JS gerado nas páginas — é a superfície mais sensível do produto);
- o canal `neuralia:` e os scripts injetados (`with_initialization_script`);
- `crates/neural-core/src/security.rs`, `reader.rs` (rede e parsing de conteúdo remoto);
- `serve_pdf_asset` e os assets do PDF.js;
- `memory.rs` no que toca a modo privado, sanitização e apagamento.

## 8. Estado atual (19/09/2026) — para ninguém refazer o que já está em curso

**Em curso na branch `fix/audit-2.0` (Claude) — Astra NÃO implementa isto em `main`:**

- palette nativa Win32 (substitui `NEURALIA_PALETTE_SCRIPT` e o canal `neuralia:palette?q=`; corrige a saída do modo privado e o `allow_local` fixo);
- `NEURALIA_NO_GMAIL` (o monitor do Gmail continua permanente por decisão do dono; a variável só o desliga para o gate `measure-cycles.ps1`);
- serviço único de temporizadores (`Timers`), em vez de uma thread por `show_splash`/toast/probe/autoscroll;
- `Range` no servidor `neuralia-pdf` (o `viewer.mjs` já pede por ranges);
- cache de ícones limitado, tema em cache, animação da Home parada quando minimizada/tapada;
- barra alinhada aos pesos dos divisores, coalescing do resize, popups sem `WS_EX_TOPMOST` + `Moved`/`Focused`;
- histórico sem I/O no event loop; entradas de histórico limitadas a 2048 chars; `escape_html` numa passagem;
- substituição dos três testes tautológicos de 1.6.0 por testes reais;
- alinhamento das docs com o código (incluindo SPEC-0107 → "Fase 0 feita", não "Implementada").

**Já em `main` (não regredir):** canal `neuralia:` com nativos capturados no document-created, `isTrusted` em todos os handlers, token de `BCryptGenRandom` comparado em tempo constante, `view-source:`, `nosniff`/CSP no servidor PDF; Reader com extração O(n), prazo e cancelamento (`tests/extraction_cost.rs`); `viewer.mjs` por ranges com geometria calculada.

**Decisões pendentes do dono (ninguém implementa sem ele decidir):**

- **Navigation API.** Uma página pode fazer `navigation.addEventListener('navigate', e => e.destination.url)` e ler `neuralia:…?cap=TOKEN` de cada ação do utilizador — o token vaza por desenho do Chromium. Opções: migrar o canal para `chrome.webview.postMessage` capturado no document-created (contradiz a letra de SPEC-0005 "no IPC object", cumpre a intenção) ou `delete window.navigation` em todos os frames (pode partir SPAs que a usem).
- **Auditoria do agent runtime** contra SPEC-0104 §11 (classes de capacidade, aprovação por classe, firewall de dados sensíveis, pivot para rede local, fixtures de injeção, audit log, kill switch) por quem não o escreveu.

## 9. Checklist antes de cada push (os dois agentes)

- [ ] `git fetch` feito; sei o que mudou em `origin/main` desde a última vez.
- [ ] `git status --short` só mostra ficheiros que eu editei nesta tarefa.
- [ ] `cargo fmt --all -- --check`, `clippy -D warnings` e `cargo test --workspace` verdes neste SHA.
- [ ] Nenhuma frase nova em docs sem código e teste correspondentes.
- [ ] Não mudei `version` em `Cargo.toml` (só o Claude, depois do gate).
- [ ] Não usei `git stash`.
- [ ] Se toquei numa área do §7, pedi revisão independente.
