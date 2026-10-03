# Relatório Técnico de Auditoria Geral — 4 Iterações de Varredura
**Projeto:** NeuralIA (Navegador Multi-IA e Agente Local em Rust)  
**Diretório:** `D:\DEV\NeuralIA`  
**Versão Auditada:** NeuralIA v2.7.1 (commit `83c9048`)  
**Data:** 02 de Outubro de 2026  
**Auditor:** Antigravity (Google DeepMind Agentic Coding Assistant)  
**Metodologia:** Varredura exaustiva em 4 iterações analíticas cobrindo Concorrência/Ciclo de Vida, Recursos/GDI/Memória, Lógica/Algoritmos/Segurança e UI/UX/Flickering.

---

## Sumário Executivo

Foi executada uma auditoria geral aprofundada de **4 iterações** sobre todo o ecossistema de código Rust do projeto NeuralIA (`crates/neural-core`, `crates/neural-app`, `crates/neural-setup` e documentação de especificações normativas).

### Visão Geral do Ecossistema:
- **Baseline de Compilação:** 100% aprovada (`cargo test`: 73 testes unitários + 21 doc-tests sem nenhuma falha).
- **Análise Estática Rigorosa:** Aprovada sem warnings (`cargo clippy --workspace --all-targets -- -D warnings`).
- **Arquitetura Geral:** Código de alta performance, sem dependências infladas de runtime, uso correto de segurança em Named Pipes e respeito ao modelo de isolamento de dados do usuário (SPEC-0006).

Apesar do estado avançado de maturidade, a presente auditoria identificou **12 pontos críticos de atenção, bugs lógicos e oportunidades de refinamento estrutural**, distribuídos nas 4 frentes normativas.

---

## Iteração 1: Concorrência, Sincronização, Ciclo de Vida e Processos Órfãos (Win32 & WebView2)

### 1.1. [CRÍTICO] Potencial de Processos Órfãos `msedgewebview2.exe` na Saída (`exit_now` & `exiting`)
- **Arquivos e Linhas:** 
  - [`crates/neural-app/src/windows_app/downloads_ui.rs:1608-1611`](file:///D:/DEV/NeuralIA/crates/neural-app/src/windows_app/downloads_ui.rs#L1608-L1611)
  - [`crates/neural-app/src/windows_app/app/event_loop.rs:107-111`](file:///D:/DEV/NeuralIA/crates/neural-app/src/windows_app/app/event_loop.rs#L107-L111)
  - [`crates/neural-app/src/windows_app/app/chrome.rs:304-345`](file:///D:/DEV/NeuralIA/crates/neural-app/src/windows_app/app/chrome.rs#L304-L345)
- **Diagnóstico:**
  O método `destroy_web_surfaces(&mut self)` é o responsável por desmontar os controladores WRY, liberar as webviews das 3 colunas do Comparador, fechar o painel do Gemini Live (que acessa webcam, microfone e compartilhamento de tela) e encerrar o agente com `AgentTermination::UserStopped`.
  No entanto, o ponto único de saída da aplicação (`exit_now`) apenas invoca `save_notes_draft_before_exit()` e chama `event_loop.exit()`. O callback de saída do event loop (`exiting`) salva a sessão de abas e encerra downloads pendentes, mas **não chama** `destroy_web_surfaces()`.
- **Impacto:**
  Embora o encerramento do processo pai normalmente force a limpeza dos processos filhos pelo Windows, em encerramentos rápidos ou parciais processos `msedgewebview2.exe` podem permanecer executando em segundo plano na sessão do usuário, consumindo memória RAM e mantendo dispositivos de mídia abertos.
- **Solução Recomendada:**
  ```rust
  fn exit_now(&mut self, event_loop: &ActiveEventLoop) {
      self.destroy_web_surfaces();
      self.save_notes_draft_before_exit();
      event_loop.exit();
  }
  ```

### 1.2. Robustez no Drop de `OwnedHandle` em IPC (`pipe.rs`)
- **Arquivo e Linha:** [`crates/neural-app/src/agents/pipe.rs:94-100`](file:///D:/DEV/NeuralIA/crates/neural-app/src/agents/pipe.rs#L94-L100)
- **Diagnóstico:**
  O struct `OwnedHandle(HANDLE)` implementa `Drop` chamando diretamente `CloseHandle(self.0)`. Em Rust/Win32, se um handle inválido (`INVALID_HANDLE_VALUE` = `(HANDLE)-1` ou `NULL`) for passado para `CloseHandle`, a API retorna `ERROR_INVALID_HANDLE` e, sob anexação de debugger nativo do Windows, pode gerar exceção `STATUS_INVALID_HANDLE`.
- **Solução Recomendada:**
  Implementar guarda defensiva:
  ```rust
  impl Drop for OwnedHandle {
      fn drop(&mut self) {
          if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
              unsafe { CloseHandle(self.0); }
          }
      }
  }
  ```

---

## Iteração 2: Vazamentos de Recursos, Gerenciamento de Memória, GDI e I/O de Sistema

### 2.1. [CRÍTICO] 14 Ocorrências de `InvalidateRect(..., 1)` com `bErase=TRUE` Causando Flickering
- **Arquivos e Linhas:**
  - `chrome.rs:708` (Splash popup)
  - `chrome.rs:769` (Search card)
  - `chrome.rs:882` (Botão caption)
  - `chrome.rs:1143` (Botão de restauração flutuante)
  - `chrome.rs:1329` (Splitters/divisores)
  - `panels.rs:568` (Handle do painel)
  - `split.rs:641` (Divisor de split)
  - `tools.rs:495` (Palette popup)
  - `downloads_ui.rs:1934` (Cartão de downloads)
  - `native.rs:230` (Botão de saída)
  - `secret_prompt.rs:197` (Prompt secreto)
  - `toast.rs:328` (Notificações toast)
  - `translation.rs:2082` (Cartão de tradução)
  - `windows_app.rs:1170` (Botões de controle)
- **Diagnóstico:**
  Quando `bErase` é passado como `1` (`TRUE`), o Windows gera uma mensagem `WM_ERASEBKGND` prévia, preenchendo a área com o pincel de fundo da classe da janela (fundo branco padrão do Windows) antes de disparar o `WM_PAINT`. Como todas as superfícies acima pintam 100% da sua área interna com `FillRect` utilizando a cor do tema atual (`theme.surface`), o apagamento prévio é redundante e gera um clarão branco/cintilação (*white flash*) perceptível durante movimentação de mouse (hover) e redimensionamento.
  No próprio instalador (`crates/neural-setup/src/app.rs`), o parâmetro já é passado como `0` (`bErase = FALSE`).
- **Solução Recomendada:**
  Substituir o terceiro parâmetro de `1` para `0` em todas as 14 chamadas de produção.

### 2.2. Arquivos Incompletos Abandonados no Disco em Falha de Download (`update.rs`)
- **Arquivos e Linhas:** 
  - [`crates/neural-core/src/update.rs:183-215`](file:///D:/DEV/NeuralIA/crates/neural-core/src/update.rs#L183-L215)
  - [`crates/neural-app/src/windows_app/app/event_loop.rs:161-172`](file:///D:/DEV/NeuralIA/crates/neural-app/src/windows_app/app/event_loop.rs#L161-L172)
- **Diagnóstico:**
  A função `download_installer` cria o arquivo executável em disco via `File::create(target_path)`. Se a conexão de rede for interrompida no meio do download ou o limite de 150 MB for atingido com erro de stream, a função propaga o erro `Err(NeuralError::Config(...))`, mas deixa o arquivo parcial de instalador corrompido gravado na pasta `%TEMP%` do sistema.
- **Solução Recomendada:**
  Adicionar remoção automática do arquivo em caso de erro:
  ```rust
  if let Err(error) = download_stream(...) {
      let _ = std::fs::remove_file(target_path);
      return Err(error);
  }
  ```

### 2.3. Alocação Contínua de Buffers e Processamento em CPU no Hot Path de Pintura de Ícones (`icons.rs`)
- **Arquivo e Linha:** [`crates/neural-app/src/windows_app/icons.rs:163-176`](file:///D:/DEV/NeuralIA/crates/neural-app/src/windows_app/icons.rs#L163-L176)
- **Diagnóstico:**
  A cada redesenho de hover de botão ou ícone de serviço, `draw_icon` aloca um buffer heap `Vec::with_capacity((size * size * 4))` e executa alpha blending em software por CPU pixel a pixel. Embora o tamanho do ícone seja reduzido (18x18 a 32x32), em eventos contínuos de hover e animação, milhares de alocações transitórias são criadas e destruídas no caminho crítico de renderização Win32.
- **Solução Recomendada:**
  Estender o `ICON_SCALE_CACHE` para armazenar a imagem final colorizada combinando a chave `(slot, tint, background, size)`.

### 2.4. Inexistência de Pool de Conexões SQLite na Memória Semântica (`sqlite_v01.rs`)
- **Arquivo e Linha:** [`crates/neural-core/src/memory/sqlite_v01.rs:574-579`](file:///D:/DEV/NeuralIA/crates/neural-core/src/memory/sqlite_v01.rs#L574-L579)
- **Diagnóstico:**
  A função `upsert` e as consultas de entidades invocam `open_ready(path)` a cada operação. Cada chamada realiza `Connection::open`, executa os `PRAGMA`s de configuração do SQLite (WAL, busy_timeout, foreign_keys) e consulta o `sqlite_master` para validar o schema. Em navegação rápida com salvamento contínuo de páginas, a sobrecarga de I/O de arquivo e parsing de schema é substancial.

---

## Iteração 3: Bugs Lógicos, Parsing, Edge Cases Numéricos e Algoritmos (Big-O & Robustez)

### 3.1. [BUG LÓGICO CRÍTICO] Comparador SemVer Falha em Pre-Releases > 9 (`update.rs`)
- **Arquivo e Linha:** [`crates/neural-core/src/update.rs:110`](file:///D:/DEV/NeuralIA/crates/neural-core/src/update.rs#L110)
- **Trecho:**
  ```rust
  (Some(cur_p), Some(cand_p)) => cand_p.cmp(cur_p),
  ```
- **Diagnóstico:**
  A comparação entre dois identificadores de pré-release (ex: `rc1` vs `rc2`) utiliza `cmp` lexicográfico de strings.
  De acordo com a norma **SemVer 2.0.0 (Seção 11, Item 4)**:
  > *"Identifiers consisting of only digits are compared numerically. Identifiers with letters or hyphens are compared lexically in ASCII sort order."*
  Na implementação atual:
  - `"rc10".cmp("rc9")` resulta em `Ordering::Less` (pois o caractere ASCII `'1'` é menor que `'9'`).
  - Consequentemente, para o atualizador do NeuralIA, uma versão `2.8.0-rc10` é considerada **inferior e mais antiga** do que `2.8.0-rc9`.
  - Se o projeto publicar um release candidate de 2 dígitos, usuários em `rc9` não receberão a atualização.
- **Solução Recomendada:**
  Separar sufixos numéricos e compará-los como inteiros (`u64`), aplicando a regra formal do SemVer 2.0.0.

### 3.2. Complexidade O(N² · log N) em Contagem de Pre-Tokens (`context_budget.rs`)
- **Arquivo e Linha:** [`crates/neural-core/src/context_budget.rs:379-380`](file:///D:/DEV/NeuralIA/crates/neural-core/src/context_budget.rs#L379-L380)
- **Diagnóstico:**
  `pretoken_count` coleta o texto inteiro em um `Vec<char>`:
  ```rust
  let chars: Vec<char> = text.chars().collect();
  ```
  Esta função é chamada indiretamente por `weight()`, que é executada dentro de laços de busca binária e bisseção de contexto (`fit_units`). Para um documento de 100 KB, cada probe da busca aloca um buffer de centenas de kilobytes apenas para contar tokens, gerando complexidade de alocação de heap $O(N^2 \log N)$.
- **Solução Recomendada:**
  Adaptar o contador para percorrer diretamente iteradores de caracteres (`Chars` / `char_indices`) ou utilizar array temporário fixo na stack para fatias menores.

### 3.3. Busca Linear O(M) em Detecção de Idioma Português (`translate.rs`)
- **Arquivo e Linha:** [`crates/neural-core/src/translate.rs:194-196`](file:///D:/DEV/NeuralIA/crates/neural-core/src/translate.rs#L194-L196)
- **Diagnóstico:**
  O slice `PORTUGUESE_WORDS` possui 51 palavras e é consultado via `.contains(&lower)`. Para cada palavra coletada da página (até 4.000 palavras por amostragem), o código executa uma varredura sequencial completa em memória. Em páginas grandes, são feitas mais de 200.000 comparações de strings desnecessárias.
- **Solução Recomendada:**
  Ordenar a lista em ordem alfabética e utilizar `binary_search` em $O(\log K)$ ou um `HashSet<&'static str>` estático.

### 3.4. Dupla Alocação na Carga de Domínios do Bloqueador (`adblock.rs`)
- **Arquivo e Linha:** [`crates/neural-core/src/adblock.rs:109-117`](file:///D:/DEV/NeuralIA/crates/neural-core/src/adblock.rs#L109-L117)
- **Diagnóstico:**
  `list_domain(candidate)` cria uma `String` temporária no heap. Ao inserir em `domains.insert(&domain)`, o método recebe `&str` e invoca `domain.into()`, criando um novo `Box<str>` e duplicando a memória antes de liberar a `String`. Em listas de 45.000 domínios, são ~90.000 alocações no heap.

---

## Iteração 4: Interface do Usuário, Responsividade, Redesenho Excessivo e Robustez Win32

### 4.1. [DESPERDÍCIO DE CPU] Redesenhos Excessivos em `UpdateProgress` (`event_loop.rs`)
- **Arquivo e Linha:** [`crates/neural-app/src/windows_app/app/event_loop.rs:188-198`](file:///D:/DEV/NeuralIA/crates/neural-app/src/windows_app/app/event_loop.rs#L188-L198)
- **Diagnóstico:**
  A função `download_installer` invoca o callback de progresso a cada buffer de 32 KB recebido do socket. Para um instalador de 14.6 MB, chegam **456 eventos** `UserEvent::UpdateProgress` na fila de mensagens da UI.
  A cada evento, a thread da janela reconstrói a string da barra de progresso, altera o texto do splash e solicita repintura da janela com `self.request_redraw()`.
  Como a barra visual só possui 20 blocos e o valor percentual é discreto (0% a 100%), mais de 350 redesenhos da janela são disparados com a barra na mesmíssima posição gráfica.
- **Solução Recomendada:**
  Aplicar estrangulamento (*throttling*) por porcentagem inteira:
  ```rust
  let percent = (progress * 100.0).clamp(0.0, 100.0) as usize;
  if percent == self.last_update_percent { return; }
  self.last_update_percent = percent;
  ```

### 4.2. Risco de Overflow em Bisseção de Tokens em Modo Debug (`context_budget.rs`)
- **Arquivo e Linha:** [`crates/neural-core/src/context_budget.rs:1133`](file:///D:/DEV/NeuralIA/crates/neural-core/src/context_budget.rs#L1133)
- **Diagnóstico:**
  A instrução `step *= 2;` em builds de debug ativa o panic nativo do Rust caso o número ultrapasse o limite de representação de `usize`. O uso de `step = step.saturating_mul(2);` garante blindagem matemática estrita.

### 4.3. Z-Order e Não-Interceptação de Cliques em Popups Flutuantes
- **Avaliação:** **Excelente**
- O uso de janelas com classe proprietária (*owner window*), estilo estendido `WS_EX_TOOLWINDOW` e a interceptação de `WM_NCHITTEST` retornando `HTCLIENT` em `splash.rs` e `secret_prompt.rs` protege a interface contra o bug comum do WebView2 roubar o foco de cliques em áreas de borda ou menus flutuantes.

---

## Tabela Consolidada de Riscos e Prioridades

| ID | Severidade | Categoria | Descrição | Componente / Arquivo |
|---|---|---|---|---|
| **BUG-01** | 🔴 Alta | Concorrência | Processos órfãos `msedgewebview2.exe` em `exit_now` | `downloads_ui.rs` / `event_loop.rs` |
| **BUG-02** | 🔴 Alta | UI / GDI | 14× `InvalidateRect(..., 1)` provocando flickering de fundo | `chrome.rs`, `panels.rs`, `split.rs`, etc. |
| **BUG-03** | 🔴 Alta | Lógica | Comparador SemVer quebra para pre-releases (`rc10 < rc9`) | `update.rs:110` |
| **PERF-01**| 🟡 Média | Responsividade | 456 redesenhos redundantes de janela em `UpdateProgress` | `event_loop.rs:188-198` |
| **PERF-02**| 🟡 Média | Big-O / RAM | $O(N^2 \log N)$ com alocação contínua de `Vec<char>` | `context_budget.rs:379` |
| **PERF-03**| 🟡 Média | Algoritmo | Varredura linear $O(M)$ com 200K comparações em detecção de idioma | `translate.rs:194` |
| **CLEAN-01**| 🟢 Baixa | Disco / I/O | Arquivo parcial temporário não é limpo em erro de download | `update.rs:183` |
| **PERF-04**| 🟢 Baixa | GDI / Heap | `draw_icon` aloca `Vec<u8>` de pixels a cada WM_PAINT | `icons.rs:163` |
| **PERF-05**| 🟢 Baixa | Heap | Dupla alocação (`String` → `Box<str>`) na carga de adblock | `adblock.rs:109` |
| **IO-01**  | 🟢 Baixa | I/O / SQLite | Abertura repetida de conexão e verificação de schema a cada upsert | `sqlite_v01.rs:574` |
| **EDGE-01**| 🟢 Baixa | Robustez | `step *= 2` suscetível a panic em modo debug | `context_budget.rs:1133` |
| **SEC-01** | 🟢 Baixa | Win32 Handle | `OwnedHandle` sem guarda de `INVALID_HANDLE_VALUE` no drop | `pipe.rs:94` |

---

## Conclusão da Auditoria Geral

O ecossistema do **NeuralIA v2.7.1** é exemplar em aderência aos padrões de segurança em Rust e integração com a API Win32 do Windows. Os testes e o pipeline de CI estão verdes e limpos. 

A correção pontual dos três itens de alta severidade (**BUG-01: destruição de webviews na saída**, **BUG-02: eliminação das 14 chamadas com `bErase=1`** e **BUG-03: correção da ordenação numérica de pre-releases no SemVer**) elevará a estabilidade, a fidelidade visual e a confiabilidade do navegador para o patamar definitivo de produção comercial.
