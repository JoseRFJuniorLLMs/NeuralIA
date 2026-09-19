# Changelog

All notable changes to NeuralIA are documented here.

## [1.7.2] - 2026-09-19

Title bar do comparador redesenhada como uma tab strip de navegador.

### Changed
- As abas/fontes abertas pelas IAs ficam agora na mesma faixa visual dos controles de janela, em vez de ao lado dos botões Gemini/ChatGPT/Claude.
- O comparador usa chrome próprio em duas linhas: title bar com abas + minimizar/maximizar/fechar; segunda linha com Home, provedores, +, Privado e controles de Split View.
- Áreas vazias da title bar arrastam a janela; controles de janela e abas mantêm hit-testing próprio.
- O conteúdo, Split View e divisores foram reposicionados para começar abaixo do novo chrome de duas linhas.

## [1.7.1] - 2026-09-19

Correção visual do comparador após mudanças de topologia dos painéis.

### Fixed
- **Divisores duplicados:** minimizar/restaurar uma IA ou abrir o Split View agora ressincroniza imediatamente os splitters nativos.
- Antes de abrir uma fonte lateral, todos os divisores antigos são ocultados; apenas os divisores correspondentes às fronteiras realmente visíveis voltam a aparecer.
- Alternar tela cheia do Split View também força a limpeza da topologia de divisores, evitando janelas Win32 antigas sobrepostas ao conteúdo.

## [1.7.0] - 2026-09-19

Endurecimento do canal entre página e nativo, omnibox flutuante nativa e uma rodada de correções de ciclo de vida, desempenho e testes.

### Security
- **Canal `neuralia:` endurecido:** o token de capacidade de cada WebView passa a vir do CSPRNG do sistema operacional (`BCryptGenRandom`), em vez de ser derivado do relógio, e é comparado em **tempo constante** — não dá mais para descobri-lo byte a byte pelo tempo de resposta.
- O token viaja por funções nativas (`encodeURIComponent` e companhia) capturadas no `document-created`, antes de qualquer script da página rodar: envenenar globais deixa de ser um caminho para roubá-lo ou para adulterar a URL que o carrega. Ele continua existindo apenas dentro do fecho dos scripts injetados, nunca no DOM.
- **`isTrusted` obrigatório** em todos os handlers injetados, de mouse e de teclado: eventos sintetizados pela página são ignorados. A página pode clicar no controle que o NeuralIA injetou; não pode fingir que o usuário clicou.
- **Omnibox flutuante nativa:** a palette deixa de ser um `<input>` injetado no DOM da página e passa a ser um controle Win32 nativo. A página só pode **pedir** que ela abra (`neuralia:palette`); não lê nem submete o texto digitado.
- **O painel privado não sai mais do modo privado:** as páginas abertas a partir dele continuam no perfil incógnito, em vez de cair no perfil normal e deixar rastro em cookies e histórico.
- **Rede local nunca a partir de uma página:** navegação originada em conteúdo remoto não alcança loopback nem rede privada em nenhuma superfície. Destino local explícito continua sendo permitido quando é o usuário que digita — inclusive na palette nativa, que é entrada do usuário e não da página.

### Fixed
- **Negação de serviço no Reader:** a extração passa a ser O(n), com prazo e cancelamento cooperativo. HTML hostil com containers aninhados em profundidade deixa de prender a extração; há fixtures para o caso hostil e para um documento de 2 MiB.
- **Ver código-fonte volta a funcionar:** `view-source:` passa a ser reconhecido como esquema aceito nas superfícies web, em vez de ser barrado pelo próprio filtro de navegação que a ação dispara.
- A barra superior volta a ficar alinhada com os divisores dos painéis; antes os dois sistemas calculavam a mesma borda de formas diferentes e ela aparecia deslocada.
- **Janelas auxiliares comportadas:** deixam de ficar por cima de outras aplicações e passam a acompanhar a janela principal ao mover, redimensionar e minimizar.

### Performance
- **PDF por HTTP Range:** o visualizador passa a carregar o documento por faixas, em vez de exigir o arquivo inteiro em memória antes da primeira página.
- **Animação da Home para quando a janela é minimizada ou tapada:** enquanto está visível e focada ela roda a ~15 FPS; escondida, não desenha nem um quadro.
- **Um só serviço de temporizadores:** os temporizadores da interface passam por uma única fila, em vez de cada recurso criar o seu.
- **Divisores coalescidos:** arrastar um divisor gera uma atualização por quadro, não uma por mensagem do mouse.
- **Cache de ícones limitado** e **tema em cache:** o cache de ícones passa a ter teto, e as cores do tema deixam de ser consultadas ao sistema a cada desenho.
- **Histórico fora do event loop:** gravação e retenção do histórico não fazem I/O na thread da interface, e o número de entradas guardadas é limitado.

### Changed
- **`NEURALIA_NO_GMAIL=1`** desliga por completo o monitor do Gmail. O monitor é uma exceção intencional ao "zero WebViews na tela inicial" — fica vivo na Home porque um notificador que morre ao voltar para casa não notifica — e o gate de ciclo de vida (`scripts/measure-cycles.ps1`) passa a defini-la, já que ele conta processos e não distingue exceção de vazamento.
- O gate da Home passa a medir também **CPU em repouso** (percentual de um núcleo, janela de 3 s) e a registrá-la no JSON, com teto de CI generoso para runner compartilhado.
- **Testes reais:** asserts tautológicos — os que passariam com qualquer implementação — deixam de contar como cobertura, e a lógica pura de interface do `neural-app` (layout da barra, pesos do redimensionamento, rota da palette, parsing de `Range`, fila de temporizadores) passa a ter testes no próprio crate.
- SPEC-0005, SPEC-0006, SPEC-0008, SPEC-0012, SPEC-0015 e `SECURITY.md` passam a descrever o canal `neuralia:` como ele é — sem objeto IPC, lista fechada de ações de interface, capacidade por WebView — e a assumir o monitor do Gmail e seu tráfego em background.

## [1.6.0] - 2026-09-19

Painéis redimensionáveis, controles nativos de Split View e navegação privada.

### Added
- **Redimensionamento por mouse:** divisores nativos entre os painéis permitem ajustar Gemini, ChatGPT e Claude arrastando horizontalmente.
- **Painéis persistentes:** as IAs podem ser minimizadas e maximizadas, mas nunca fechadas; as demais expandem automaticamente e a proporção manual é preservada.
- **Privado:** botão no canto superior direito abre um painel lateral usando `WebViewBuilder::with_incognito(true)`, sem reutilizar cookies da sessão normal nem criar aba de fonte persistente.
- **Ctrl+N:** inicia uma nova aba no grupo da IA atual.
- **Ctrl+H:** histórico local permanece acessível tanto pela omnibox nativa quanto pelas páginas WebView.

### Changed
- Os controles **Fonte · IA / expandir / fechar** do Split View deixam de ser injetados dentro do site e passam para a barra nativa superior.
- Mesmo em tela cheia do Split View, a barra nativa continua acessível e o conteúdo começa abaixo dela.

### Fixed
- O Split View não sobrepõe mais seus botões ao conteúdo de LinkedIn, GitHub ou qualquer outro site.
- Divisores são ocultados automaticamente quando um painel é minimizado, quando há Split View ou quando uma IA está maximizada.

## [1.5.0] - 2026-09-18

Barra de titulo compacta com abas inline, menu de contexto e Split View alinhado ao scroll do NeuralIA.

### Added
- **Hit/hover visual** em Home, nome da IA, botão + e abas de contexto.
- **Menu nativo no clique direito da aba:** Abrir, Abrir em tela cheia, Fechar aba, Fechar outras abas do grupo e Fechar todas do grupo.

### Changed
- As abas deixam a segunda linha e passam para a mesma barra de titulo: `Gemini + [abas]`, `ChatGPT + [abas]`, `Claude + [abas]`.
- A barra superior volta a 44 px logicos para devolver altura util aos painéis.
- O Split View passa a usar a mesma timeline tracejada vertical do NeuralIA e oculta a scrollbar tradicional.

### Fixed
- **Auto-scroll do Split View:** quando uma fonte lateral está aberta, o ciclo automático avança a IA de origem e a página lateral; em tela cheia, avança a própria fonte.
- A timeline da fonte lateral detecta também containers internos com overflow, não apenas o documento principal.

## [1.4.0] - 2026-09-18

Rolagem sincronizada nas tres IAs e notificacao Gmail usando a sessao Google existente.

### Added
- **Gmail toast:** quando o perfil WebView2 já está autenticado no Google, o NeuralIA acompanha a caixa de entrada e mostra novas mensagens no canto inferior direito, com remetente e assunto, sem guardar senha.
- O monitor do Gmail cria uma linha de base inicial para não notificar e-mails antigos como se fossem novos.

### Fixed
- **Auto-scroll das 3 colunas:** Gemini, ChatGPT e Claude agora detectam e rolam seus próprios containers internos; antes apenas páginas que rolavam no `window/document` avançavam corretamente.
- Ajustado o teste geométrico da barra agrupada: o centro relevante agora é o grupo **IA + botão +**, não apenas a pílula do nome.
- Aplicado o formato exigido pelo `cargo fmt` à UI de abas agrupadas da 1.3.0.

## [1.3.0] - 2026-09-18

Abas visiveis agrupadas por IA e botao + em cada grupo.

### Added
- **Grupos de abas por IA:** Gemini, ChatGPT e Claude passam a ter suas proprias abas de fontes no topo; uma fonte nunca e misturada com a sessao de outra IA.
- **Botao + por IA:** ao lado do nome de cada modelo, abre a omnibox flutuante já vinculada àquela IA.
- Ate tres abas recentes ficam visiveis por grupo; contextos mais antigos continuam preservados internamente.
- Clicar numa aba de fonte reabre diretamente o Split View correspondente; a aba ativa recebe destaque.

## [1.2.0] - 2026-09-18

Split View, omnibox flutuante e envio real da consulta para as tres IAs.

### Added
- **Split View por IA:** links externos abrem numa gaveta lateral ao lado da conversa que gerou a fonte, com fechar e expandir/restaurar sem criar uma fileira de abas.
- **Contexto agrupado por IA:** cada coluna preserva um histórico leve das fontes abertas por Gemini, ChatGPT ou Claude.
- **Omnibox Spotlight:** `Ctrl+K` ou `Ctrl+T` abre uma barra flutuante. URL abre no Split View; texto pesquisa diretamente na IA ativa.
- **Timeline como scroll:** a scrollbar nativa da coluna e ocultada e a trilha centralizada detecta o container rolavel real, inclusive em SPAs com scroll interno.

### Fixed
- Enter na barra principal passa a efetivamente enviar a consulta para **Gemini + ChatGPT + Claude**. ChatGPT e Claude antes apenas recebiam o texto preenchido e aguardavam um segundo Enter/clique.
- A timeline deixa de assumir `window.scrollY` e passa a seguir o maior container rolavel da pagina.

## [1.1.4] - 2026-09-18

Polimento da timeline independente do comparador e correcao de sincronizacao visual.

### Fixed
- O estado expandido/restaurado volta a controlar o ID atual `#neuralia-comp-expand`; a implementacao 1.1.3 ainda procurava o ID antigo `#neuralia-comp-btn`.
- A timeline de cada IA fica mais proxima do controle temporal do Grok: trilha transparente, marcador ativo mais longo, vizinhos graduais, seta superior discreta e botao inferior circular.
- Teste de regressao garante que o script injetado e o sincronizador nativo continuem usando os mesmos IDs.

## [1.1.3] - 2026-09-18

Comparador com timelines independentes e hardening de seguranca, memoria e ciclo de vida.

### Added
- **Home neural nativa:** fundo dinamico desenhado por GDI, com neuronios/conexoes a convergir para a logomarca sem criar WebView, sem video e sem trafego de rede. `NEURALIA_REDUCE_MOTION=1` desliga o movimento.
- **Timeline por IA:** cada coluna Gemini/ChatGPT/Claude recebe uma trilha vertical independente, com marcadores de progresso, seta para resposta anterior e proxima resposta. Cada controle rola apenas o seu WebView.
- Testes para impedir comandos privilegiados via `neuralia:`, pivots de pagina publica para rede privada e spoofing da origem interna do PDF.
- O SBOM CycloneDX passa a declarar o PDF.js 6.3.289 vendorizado e a licenca Apache-2.0.

### Fixed
- A Home deixa de enviar `jose r f junior` automaticamente ao arrancar. `NEURALIA_STARTUP_INPUT` fica apenas como entrada explicita de automacao/benchmark.
- Paginas externas nao podem mais disparar historico, limpeza de historico, DevTools, view-source ou impressao atraves do esquema `neuralia:`.
- Navegacao iniciada em pagina publica nao pode pivotar para loopback/rede privada; navegacao local digitada explicitamente continua permitida.
- Downloads de PDF usam um worker coalescente, em vez de criar uma thread de 90 s por pedido.
- O buffer do PDF e libertado ao sair da superficie; o limite de documento baixa para 32 MiB e o viewer remove canvases distantes.
- O PDF viewer cancela renders antigos em resize e deixa de marcar renderizacao obsoleta como atual.
- `fetch_document` passa a exigir `Content-Type` explicito e o Reader aplica um limite adicional aos bytes efetivamente decodificados.
- O fallback do Reader ignora payloads `script/style/template/noscript` em paginas JS-heavy.
- Historico nunca faz escrita/fsync no event loop quando a fila esta saturada.
- Timeout da pergunta de auto-scroll agora equivale a "Nao" e limpa corretamente o estado; o toast mostra os 30 s reais.
- Esc na barra de Ctrl+F fecha a busca em vez de navegar para tras.
- O gate de lifecycle passa a bloquear o CI e reconhece reutilizacao de processos WebView2 entre controllers.
- O teste manual do YouTube foi substituido por fixtures deterministicas/offline.


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
