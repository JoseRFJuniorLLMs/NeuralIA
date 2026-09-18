# Changelog

All notable changes to NeuralIA are documented here.

## [1.1.3] - 2026-09-18

Auditoria recursiva, trilhas independentes de rolagem e hardening do comparador.

### Added
- **Três trilhas de rolagem independentes:** cada painel do Google, ChatGPT e Claude recebe uma navegação vertical compacta inspirada em linha do tempo, com uma espinha fina, quatro marcas curtas e uma marca longa móvel; clicar ou arrastar afeta apenas aquela IA.
- Probe interno de lifecycle para o CI medir controladores WebView reais em vez de inferir por processos `msedgewebview2.exe`.
- PDF.js vendorizado passa a ser componente explícito do SBOM CycloneDX da release.

### Security
- Ações `neuralia:` injetadas em páginas remotas exigem token aleatório por WebView e eventos de usuário confiáveis; páginas não podem mais navegar diretamente para ações privilegiadas como limpar histórico ou abrir DevTools.
- Navegação pública do Full Web, comparador e links de PDF bloqueia pivôs sintaticamente locais/privados; navegação local digitada diretamente pelo usuário continua permitida.
- Reader aplica teto explícito aos bytes pós-decodificação e PDFs exigem `Content-Type: application/pdf` e assinatura `%PDF-`.

### Fixed
- Downloads de PDF usam um único worker coalescente em vez de criar uma thread do sistema por pedido.
- O PDF é entregue uma única vez ao protocolo interno, eliminando a cópia Rust persistente de até 64 MiB após o carregamento.
- O viewer PDF cancela renders obsoletos e libera canvases de páginas distantes.
- Reader não usa mais `root.text()` como fallback de páginas JS-heavy, evitando expor scripts/JSON de hidratação como conteúdo.
- Histórico nunca volta a fazer leitura/fsync na thread da UI quando a fila enche.
- Timeout da pergunta de auto-scroll passa a contar como Não e não contamina avisos posteriores.
- O aviso de auto-scroll passa a mostrar os mesmos 30 s configurados no runtime.
- A Home deixa de disparar qualquer consulta automática em produção.
- `Ctrl+F` + Escape fecha a busca sem voltar a página.
- O gate de lifecycle agora é bloqueante e mede 3 WebViews no comparador e 0 na Home.

## [1.1.2] - 2026-09-18

Visualizador embutido de PDF offline com Mozilla PDF.js e acabamento transparente na tela inicial.

### Added
- **Visualizador embutido de PDF offline:** Documentos `.pdf` abrem diretamente num visualizador embutido em memória (`http://neuralia-pdf.localhost`) alimentado pelo PDF.js da Mozilla, com renderização em canvas, zoom, auto-scroll sincronizado e atalhos completos de navegação.
- **Descarregador seguro de documentos binários (`fetch_document`):** Suporta até 64 MiB com validação estrita de cabeçalho `Content-Type`, controle de redirecionamentos com filtro de rede (SSRF) e verificação de cancelamento/deadline em streaming.

### Changed
- **Arte da marca na tela inicial com transparência pura:** Composição alfa dos pixels sobre a cor de fundo do tema do Windows, eliminando a caixa escura residual no tema claro.

## [1.1.1] - 2026-09-18

Correcoes de congelamento, teclado e tela inicial, mais rolagem de leitura.

### Fixed
- **A aplicacao congelava ao voltar de um login.** Um popup de uma coluna destruia as tres colunas para abrir um WebView unico; construir WebViews corre um ciclo de mensagens ANINHADO dentro do nosso callback, durante o qual o winit deixa de entregar redraws -- a janela ficava com os pixeis das janelas mortas, a queimar CPU e surda ao teclado. O popup passa a carregar na coluna que o pediu, o fundo e apagado no `WM_ERASEBKGND` (o unico ponto de pintura que ainda corre nesse ciclo) e a limpeza acontece antes de entrar nele.
- **Esc e Backspace nunca funcionavam fora da tela inicial.** O foco do teclado vive sempre numa janela filha (o WebView2 ou a omnibox), por isso o ramo de teclado do winit era codigo morto. As teclas passam a ser apanhadas na fase de captura dentro das paginas: Esc volta um nivel, Backspace volta uma pagina, 1/2/3 expandem colunas e 0 restaura.
- **`reveal_chrome` criava uma thread do sistema operativo por cada movimento do rato** -- centenas vivas ao mesmo tempo. Passa a ser um prazo atomico com uma unica thread de vigia.
- O botao de sair do ecra completo nao respondia: uma janela `STATIC` devolve `HTTRANSPARENT` e o clique atravessava-a ate ao WebView.
- `set_fullscreen(None)` faltava em quase todas as saidas; bastava um login para ficar sem barra de titulo e sem retorno.

### Added
- Rolagem automatica de leitura, com consentimento: ao abrir um documento pergunta-se Sim/Nao uma vez por sessao, e sem resposta nada se mexe. Com Sim, avanca uma pagina a cada 30s e para no fim; F8 liga e desliga. Um documento sozinho (HTML, texto ou PDF) avanca por PageDown sintetizado -- a unica via que chega ao visualizador de PDF do Edge -- e as tres colunas por script.
- Teclado ao estilo do Chrome dentro de todas as paginas: zoom na escada do Chrome (Ctrl +/-/0, herdado pelas paginas novas), barra de procura (Ctrl+F), recarregar (Ctrl+R/F5), historico da pagina (Backspace, Alt+setas), Ctrl+L, Ctrl+P, Ctrl+T/W, F11, DevTools (F12, Ctrl+Shift+I/J/C) e codigo-fonte (Ctrl+U). Ctrl+H e Ctrl+Shift+Delete passam a ser globais a serio, como o README ja prometia.
- URLs `.pdf` abrem no visualizador embutido em vez de irem para o Reader, que so le HTML e as rejeitava -- o PDF nunca chegava a abrir.
- Perfil do WebView2 fixado em `%LOCALAPPDATA%\NeuralIA\WebView2`: as sessoes deixam de depender do sitio de onde o executavel foi corrido.
- Duplo clique em qualquer sitio de um painel expande essa coluna.

### Changed
- A tela inicial ficou so com a arte da marca e a barra arredondada, sem titulo nem slogan em texto.
- O botao de sair do ecra completo mudou para o centro do topo e acompanha o aparecer e desaparecer da barra.

## [1.1.0] - 2026-09-18

Comparador de IAs, tema do sistema e correcoes de ciclo de vida apontadas pela terceira auditoria.

### Added
- Comparador lado a lado: uma pergunta abre o Google AI Mode, o ChatGPT e o Claude em tres colunas. `ask:` ou `?` mantem a pergunta num unico fornecedor.
- Barra de topo com a cor de destaque do Windows e o tema claro/escuro do sistema, pilulas e icones suavizados, e realce sob o rato.
- Ecra completo real por coluna: sem barra de titulo, com botao de saida flutuante sempre visivel e barra que reaparece com o rato no topo.
- Consulta automatica ao arrancar, configuravel por `NEURALIA_STARTUP_INPUT` e desligavel por `NEURALIA_NO_STARTUP`.
- Gate de ciclo de vida (`scripts/measure-cycles.ps1`) que conta WebViews em ciclos comparador -> Home.

### Fixed
- **Os WebViews do comparador sobreviviam ao regresso a Home.** Todas as saidas passam agora por `destroy_web_surfaces`, que destroi o comparador e o WebView unico.
- `Ctrl+Shift+Delete` dizia que tinha apagado o historico sem confirmacao do disco, e a mensagem era escrita antes de `show_home` a apagar. Agora o worker confirma e a mensagem sobrevive.
- Entradas de historico deixavam de ser gravadas em silencio com a fila cheia; passam a ser escritas na hora.
- O Reader continuava a puxar bytes da rede depois de o utilizador voltar a Home; passa a desistir entre blocos.
- O historico e substituido de forma atomica (ficheiro temporario + `rename`) em vez de truncado no lugar.
- CI e release apontavam para `neural-app.exe` quando o binario passou a chamar-se `NeuralIA.exe`.
- `cargo clippy` e `cargo fmt --check` falhavam em `main`; o CI passa agora a correr tambem `cargo test -p neural-app`.
- A omnibox nao tinha tipo de letra definido e herdava a fonte de sistema minuscula.

### Security
- Filtro de IPv6 desempacota 6to4 e NAT64 antes de decidir, fechando o tunel para 127.0.0.1 e para a rede privada; cobre ainda site-local, discard-only, Teredo e benchmarking.
- `build.rs` falha a compilacao em release quando o recurso PE nao compila, em vez de continuar com um aviso.
- O workflow de release exige `push` e repositorio de origem proprio, alem de CI verde em `main`.

### Changed
- A tela inicial ficou so com a marca, a barra arredondada e o botao Ir; os modos vivem na gramatica da omnibox.
- SPEC-0003, SPEC-0005 e SPEC-0008 passam a descrever o leque para tres fornecedores e o tecto de WebViews do comparador.

## [1.0.1] - 2026-09-18

Security and reliability hardening release after the second recursive audit.

### Security
- Public Reader DNS resolution filters loopback, private, link-local, documentation, benchmark, multicast and reserved IP ranges before connection.
- Reader uses the operating-system TLS certificate verifier.
- The full Reader redirect chain shares one hard deadline.
- Reader-generated pages contain no NeuralIA JavaScript and use `script-src 'none'`.
- External pages still receive no NeuralIA IPC.
- Stable release assets cannot be overwritten by later `main` pushes.
- Release publication only follows a successful `main` CI run.
- GitHub Actions used by CI/release are pinned to commit SHAs and build jobs do not receive release-write credentials.

### Fixed
- Replaced unbounded Reader thread spawning with one coalescing Reader worker.
- Bounded local history to the configured retention limit.
- Moved history writes off the UI thread, added a native Ctrl+H viewer, and added a clear-history action.
- Preserved whitespace in `<pre>` code blocks.
- Rendered list items as semantic HTML lists.
- Added explicit handling for `target="_blank"`/new-window requests while retaining the one-WebView invariant.
- Replaced the painted omnibox editor with a native Windows `EDIT` control.
- CI now tests exactly the committed `Cargo.lock` with `--locked`.
- Added a Windows native-Home startup/RSS/thread regression gate with a JSON artifact.

### Testing
- Added end-to-end local HTTP tests for redirects, redirect limits, body limits, charset decoding, non-HTML responses and total deadline behavior.
- Added additional security and history retention tests.

### Distribution
- Release workflow emits SHA-256, a CycloneDX JSON SBOM, Cargo metadata and GitHub build-provenance attestation.
- Stable release publication refuses to overwrite an existing release.

### Remaining
- Automated startup/RSS performance gates.
- Authenticode signing and installer.
- Optional macOS/Linux shells.

## [1.0.0] - 2026-09-18

First stable release.

### Added
- Native Windows home surface with lazy WebView2 creation.
- Google AI Mode routing for ordinary questions.
- HTTP Reader with bounded body, timeout and semantic article extraction.
- Explicit Reader and Full Web modes.
- Local append-only history.
- Windows x64 release workflow with SHA-256 artifact.
- RustSec dependency audit in CI.

### Security
- Unsupported schemes and local filesystem paths are rejected instead of being sent to remote AI search.
- Embedded URL credentials are rejected.
- Reader redirects are followed manually and validated before each request.
- Public Reader navigation cannot redirect into obvious loopback/private/link-local targets.
- External Web pages do not receive the NeuralIA IPC bridge.
- Sensitive WebView permissions are denied by default.
- Reader HTML is escaped and protected by a restrictive CSP.
- History writes use file locking and durable flushes to avoid interleaved records.
