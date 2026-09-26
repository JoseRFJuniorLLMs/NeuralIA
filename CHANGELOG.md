# Changelog

All notable changes to NeuralIA are documented here.

## [Unreleased]

### Changed
- **infra-seams — costuras para as features da 2.3 (b8b1a83, e930dac, 80d7be7, ae505d3, 3211b0c):** sem mudança visível. Cada feature passa a viver no seu módulo `windows_app/<feature>.rs` com o seu próprio enum de eventos, uma variante em `UserEvent` e um braço no event loop (o tema é o piloto: `UserEvent::Theme(ThemeEvent)`); o "Apagar histórico" (Ctrl+Shift+Delete) percorre uma tabela com nome dos alvos que apaga (abas, memória, leitura dos livros, histórico), na mesma ordem e com a mesma pergunta de antes; a pintura e a dica da barra leem um `BarState` com nome em vez de parâmetros posicionais; os botões ‹ › de cada coluna e os ícones do canto direito são registos (`ColumnButton`, `RIGHT_CLUSTER`) com a geometria de sempre.
- **Painel lateral — canal por secção e página montada de `assets/panel/` (e930dac):** `parse_panel_message` entrega cada pedido à secção do prefixo dele (`note-`/`notes-` → Notas; sem prefixo → Histórico) com os mesmos tectos de sempre (4 KiB; só o `note-save` e o `note-draft` passam disso), e passa a recusar pedidos com campos a mais nos `args` ou com `args` que não é um objeto (`null`, texto, número; `args` ausente continua a valer como vazio — regra presa em afb8d37) — a página que embarca nunca os manda (`args || {}`); um prefixo que nenhuma secção reclamou morre no delegador. A página do painel (`PANEL_HTML`) é montada em tempo de compilação de `assets/panel/*.css|html|js`, byte a byte igual à de antes (`.gitattributes` fixa LF).
- **Testes (e930dac, 80d7be7, 3211b0c, afb8d37):** gates novos `panel_messages_delegate_by_prefix_and_keep_caps`, `panel_html_is_assembled_from_its_section_assets`, `clear_history_runs_every_registered_target`, `column_buttons_fit_or_vanish`, `right_cluster_slots_never_overlap_and_hit_back` e `panel_bodies_above_the_absolute_cap_are_never_read_as_json` (afb8d37: o tecto absoluto do canal do painel, antes do JSON, provado por um contador de leituras só de testes — tirar a verificação dava o mesmo `None` pelo tecto da secção e nada ficava vermelho); os críticos entram na matriz de sabotagem do CI (`panel-delegator-byte-cap`, `panel-absolute-cap-before-json`, `clear-history-skips-a-registered-target`). Os braços da memória e do histórico do "Apagar histórico" no `App` ficam presos por texto em `the_shipped_paths_are_wired_to_the_tab_session` (afb8d37); `clear_history_runs_every_registered_target` prova só o percurso pela tabela, e a doc dele diz agora isso.
- **infra-settings-keys — lojas com token e cofre de chaves (417c448, f7eec65):** sem mudança visível. `neural_core::json_store` traz o ficheiro JSON versionado (`{"version":N,"data":…}`): o tecto de bytes confere-se antes de ler; sem ficheiro valem os valores por omissão e nada se escreve; estragado, de uma versão mais nova ou grande demais fica só de leitura e nunca é reescrito por cima; o estragado e o de uma versão mais nova ganham uma cópia `.bak` dos bytes lidos, o grande demais não (f7eec65: antes era copiado inteiro para o `.bak` a cada abertura, duplicando no disco um ficheiro de tamanho qualquer); gravar é temporário + `sync_all` + `rename`, e os ficheiros partilhados entre janelas relêem-se debaixo de um trinco antes de mudar. Abrir uma loja pede um token que só o registo das lojas dá; o registo é criado uma vez por processo (no arranque) e cada loja declara o seu tipo (automática, pedida pelo utilizador, definição). Um ficheiro, um tipo, com os nomes como o Windows os vê (f7eec65): o registo compara os nomes sem maiúsculas e recusa partes que acabam em ponto, porque `Panel-Width.json` e `panel-width.json.` são o mesmo `panel-width.json` e deixavam uma loja automática ser escrita no modo privado por outro nome. A cifra DPAPI saiu do `gemini_live.rs` para `secrets.rs`: o `gemini-live.key` mantém o nome, o cabeçalho `NLK1` e a entropia `NeuralIA/gemini-live/v1`, e um ficheiro gravado por versões anteriores abre igual. Novo cofre de chaves por fornecedor (`<data_dir>/keys/<slot>.key`, cabeçalho `NBK1`, entropia própria de cada slot; o Gemini usa o ficheiro do Live) e o pedido de chave nativo (campo de palavra-passe, «Salvar e verificar», «Cancelar», «Esquecer chave»), ainda sem nada no produto que o abra: chega com a tradução e o juiz. As chaves não fazem parte do "Apagar histórico" (Ctrl+Shift+Delete).
- **infra-notify-popups — menus nativos devolvem o teclado (ab80e28, d07b94a):** o menu do botão direito do Pomodoro, o do tema (botão Home), o da aba, o do grupo, a lista "‹N" e o da pílula de uma coluna passam todos por um só caminho (`PopupMenu`, o único `TrackPopupMenu` da app). Antes de abrir, ele regista onde estava o teclado (a janela com o foco e, se ela estiver dentro de uma WebView, qual); ao fechar — com ou sem escolha — devolve-o lá: a uma WebView pela `MoveFocus(PROGRAMMATIC)` do WebView2, a uma janela nossa (a omnibox) por `SetFocus`. A exceção são os comandos que põem o teclado noutro sítio de propósito: "Abrir", "Abrir em tela cheia" e escolher uma aba na lista "‹N". Os itens, as marcas, os cinzentos e as amostras de cor dos menus são os de antes; um item cinzento fica sem clique também quando tem uma amostra de cor (d07b94a — antes, esse ficava clicável; nenhum menu de hoje junta as duas coisas). O gate real Win32 prova-o com um EDIT e com uma vista de teste que recebe o teclado pelo mesmo gancho; a `MoveFocus` de uma WebView verdadeira não é exercitada em teste.
- **infra-notify-popups — centro de avisos e família de popups nativos (ab80e28, d07b94a):** sem mudança visível no aviso do Gmail, que passou a ser um aviso (`Notice`) do centro novo (`notify.rs`): o mesmo título "Gmail · novo e-mail — abrir?", o mesmo corpo (remetente · assunto, ou o que houver), o mesmo tamanho, os mesmos botões "Abrir" e "Não" nos mesmos sítios, os mesmos 12 s, no canto inferior direito da janela da NeuralIA; "Abrir" abre o painel do Gmail e "Não" só esconde. O aviso passa a responder `MA_NOACTIVATE` ao clique, como o cartão do Mandar para IA (antes contava só com o `WS_EX_NOACTIVATE`). O centro decide a entrega de cada aviso: no máximo um à vista, um do mesmo tipo substitui-o, um de outro tipo espera numa fila com um lugar por tipo; com o modo Foco ligado, tudo menos o Pomodoro espera e sai resumido no fim ("Gmail · 3 e-mails durante o foco — abrir?"), e um Pomodoro que chegue com um aviso de outro tipo à vista sai quando esse aviso acaba, sem esperar o fim do Foco (d07b94a); no modo privado, um corpo com conteúdo (o assunto de um e-mail) vira "Conteúdo oculto no modo privado". O modo Foco e o modo privado ainda não existem na app: o centro já os trata e os gates provam-no, e as features que os trazem só os ligam. O cartão de confirmação do Mandar para IA e do Traduzir passa a ser um `NativeCard` com as mesmas regras (token por pedido, 600 ms até aceitar o clique, 12 s até expirar, só confirma o texto que pintou, nunca ativa nem fica acima de outras aplicações), e a pergunta do meio da janela (a da rolagem automática, com Sim e Não iguais aos de antes) passa a ser uma `SplashQuestion` com os seus próprios botões.
- **infra-llm-transport — cliente único das chamadas de IA (867bf3e, ac1078f):** sem mudança visível: nenhuma feature o chama ainda (a Tradução é a primeira, depois da parte 2, `infra-llm-untrusted`). `neural_core::llm` traz o `ApiClient` sobre o ureq 3.4.2 que o Reader já usa (nenhum crate novo; `Cargo.lock` igual): ignora as variáveis de proxy do ambiente, não segue redirects (um 3xx vira erro), usa os certificados do sistema e só HTTPS, tem prazo por chamada (60 s a gerar, 15 s por página da lista de modelos), tecto de 1 MiB no corpo descodificado (um `Content-Length` maior é recusado antes de ler, e uma bomba gzip pequena na rede também), desistência cooperativa e `User-Agent: NeuralIA/<versão>`. Só fala com o host fixado da Gemini (`Endpoint::pinned`); o loopback só compila nos testes. A chave (o `ApiKey` do cofre, que passa a implementar `ApiCredential`) só viaja no cabeçalho `x-goog-api-key`, nunca no URL nem no corpo; os construtores dos pedidos não a recebem. Os erros têm mensagem pt-BR («Chave recusada», «Sem créditos ou limite de gastos», «Muitos pedidos, tente em N s», «Modelo indisponível, escolha outro», «Serviço indisponível», «Demorou demais», «Resposta incompleta», e ainda «Resposta grande demais», «Cancelado» e «Pedido recusado (HTTP N)») e nunca levam a chave nem o corpo da resposta. Um 429 só dá «Sem créditos ou limite de gastos» com uma razão explícita de créditos (`insufficient_quota`, `credit balance`) ou com a quota diária da Gemini esgotada (`quotaId` `…PerDay…`, que só volta no dia seguinte); a quota por minuto da Gemini, cujo texto também fala de faturação, dá «Muitos pedidos, tente em N s» com o `retryDelay` dela, e a faturação só conta como falta de créditos num 400/401/403 (b67d1c1). A Gemini ganha a lista de modelos com páginas e o `generateContent`; o modelo de cada finalidade sai de uma regra (forma permitida, só versões estáveis, termos negados `realtime`, `audio`, `image`, `tts`, `transcribe`, `search`, `codex`, `pro`, `nano`, `oss`, `chat-latest`, `live`, `native-audio` e `embedding`, e a fixação do dono, que ganha quando o modelo está na lista e gera texto), com a faixa de preço para o cartão de consentimento; para a Tradução, o `*-flash-lite` estável mais novo. Nenhum id de modelo está escrito no código; a constante do Gemini Live não foi tocada.
- **infra-webview-hooks — os ganchos de cada WebView (e813ba1, a3c44b8):** sem mudança visível. Toda WebView que a app constrói (as três colunas, a fonte ao lado normal e privada, a Web completa e a página do agente, o Leitor, o PDF, os livros, o Gemini Live, o monitor escondido do Gmail, o painel do Ctrl+H e os painéis de serviço) nasce em dois passos que passam por um só módulo (`windows_app/webview_hooks.rs`, com `UserEvent::WebView(WebViewEvent)` no padrão do infra-seams): antes do `build`, `hooked_builder(builder, hospedeiro, origem local)` instala a trava de navegação do hospedeiro, a recusa de downloads onde a tabela manda e o aviso de página carregada, e devolve um `HookedBuilder` que leva esse hospedeiro; só ele constrói (`build_hooked` / `build_hooked_as_child` são as únicas chamadas ao `build` do wry no produto) e, construída a WebView, regista no WebView2, para o mesmo hospedeiro, os itens do NeuralIA no menu do botão direito e um `AcceleratorKeyPressed` (a3c44b8: antes eram duas chamadas soltas, `hooked_builder` e `install_webview_hooks`, cada uma com o seu hospedeiro, e nada impedia um sítio de dar a cadeia de um hospedeiro ao builder e o menu de outro ao COM, ou de construir sem passar por nenhuma). O que cada hospedeiro recebe está numa tabela pura (`webview_hooks(host)`) com um slot por feature da 2.3 (menu, downloads, despachante de recursos, cadeia de navegação, aceleradores, distrações — este vazio até anti-distração). As onze travas de navegação que cada builder tinha passaram a ser a cadeia do hospedeiro (`web_navigation_verdict`: Web, Pdf, Reader, Epub, Live, Gmail, SidePanel, Service), com as mesmas verificações na mesma ordem e os mesmos veredictos e eventos de antes, byte a byte. O item de rolagem do botão direito das colunas e da fonte ao lado (privada incluída) vem agora de um registo (`WEBVIEW_MENU_ITEMS`), com o mesmo rótulo, o mesmo lugar depois dos itens nativos e o mesmo Ctrl+R; a pílula lê o mesmo registo. O `AcceleratorKeyPressed` de cada WebView lê a tecla como o spike de CI a lia (tecla virtual, descida/subida, modificadores, repetição) e consulta `accelerator_lookup`, que ainda não prende atalho nenhum e por isso não toca no `Handled`: nada muda no teclado até infra-commands-keymap o preencher. Decisão do spike registada: o despacho nativo guarda cada atalho da página e do `act()` do mapa de teclas nos nove hospedeiros, sem fallback; a página continua a ver um `keypress` por toque, que não chega ao `keydown` e não faz diferença aos ganchos. Nenhum código do spike embarca e nada depende da feature `accel-spike`. O despachante de recursos (`resource_gate_answers`) é um esboço que o produto ainda não liga ao `WebResourceRequested` (o bloqueio de anúncios fá-lo): o contrato de nunca responder aos esquemas próprios (`neuralia-pdf`, `neuralia-epub`, `neuralia-live`) já está preso por gate. O evento `WebViewEvent::PageLoaded { page, url }` chega ao event loop no fim de cada carregamento (no Leitor e no PDF o endereço verdadeiro continua a ser `App::page_source`); ninguém o lê ainda além do log de depuração, que regista só o hospedeiro, nunca a URL.
- **infra-llm-untrusted — texto não confiável, resolvedores por localidade e leitura de páginas (21eb543, 6d33111, bbe7056, 107f662, 8590e50, 5f84ea9):** sem mudança visível: nenhuma feature usa isto ainda (a Tradução é a primeira). Toca áreas do §7 (`reader.rs`, `security.rs`, scripts só-leitura) e **espera o sim do dono (OQ2) antes de entrar em `main`**. (1) O `PublicResolver` do Reader passou, sem mudar uma linha, de `reader.rs` para `security.rs` (`pub(crate)`); o Reader continua igual (21eb543). (2) `security::Locality` (`Public`, `Loopback`, `Lan`) decide depois da resolução de nomes, endereço a endereço: `Public` é o de sempre; `Loopback` só 127.0.0.0/8 e `::1` (também mapeado); `Lan` só 10/8, 172.16/12, 192.168/16 e fc00::/7 (também mapeados), nunca link-local (169.254/16, fe80::/10), CGNAT nem túneis 6to4/NAT64. O cliente de IA (`llm::transport`) passa a resolver os hosts fixados como `Public`: um DNS que apontasse o host da Gemini para o loopback, a rede local ou o link-local não recebe o pedido nem a chave (6d33111). (3) `neural_core::untrusted`: `sanitize` tira os invisíveis (U+200B–200D, U+2060, U+FEFF), os controlos bidi (U+202A–202E, U+2066–2069), os caracteres de etiqueta (U+E0000–E007F), os seletores de variação suplementares (U+E0100–E01EF) e os controlos C0/C1 menos a mudança de linha e o tab, e com destino remoto aplica depois a redação de linhas sensíveis (`redact_sensitive_text`); `UntrustedText` só sai sanitizado; o `PromptBuilder` separa instruções (só texto escrito no código), o pedido do utilizador e os dados, que vão sempre numa cerca com um nonce de 128 bits por chamada, que a página não conhece e que sai dos dados se lá estiver — é isso que impede o texto de uma página de fechar a cerca; por cima, em melhor esforço, as sequências de três sinais `<`/`>` (também os parecidos que a tabela conhece, como `＜`, `‹`, `«`, `〈`, `⟨`, `≪`, `ᚲ`) e a palavra do marcador (também em largura total ou com as letras cirílicas, gregas, armênias, Lisu, versaletes e matemáticas que a tabela conhece) são desfeitas, e um parecido fora das tabelas sobrevive, mas sem o nonce não fecha nada (8590e50); o aviso `CONTEXT_DATA_PREAMBLE_PT` vai nas instruções; `injection_signals` só assinala (frases de injeção em pt e en, tokens de modelos de conversa, marcadores parecidos, caracteres escondidos, imagem Markdown com consulta no URL); `Shingles` guarda janelas de 16 caracteres para a deteção de vazamento e a deduplicação (bbe7056). (4) `windows_app/page_eval.rs`: a leitura de uma página por script só-leitura, com um token por leitura, um prazo (serviço `Timers`), um tecto de bytes do JSON cru antes do serde e, na chegada, a geração de navegação da vista e o URL conferidos — uma resposta tardia, grande demais ou de outra página cai. Só corre scripts da lista `READ_ONLY_SCRIPTS` (hoje o da seleção do Ctrl+Shift+Z, cuja chamada não mudou), e um gate recusa no texto de cada um as formas diretas de publicar, buscar, mudar o DOM, escrever HTML, escutar, agendar, navegar (`location` fora das leituras `location.<parte>`, `history`, `reload`), submeter, clicar, mover o foco ou a rolagem, guardar e correr código dinâmico — é uma lista de negação sobre o texto, não uma prova, e cada script novo entra com o sim do dono (5f84ea9); cada consumidor liga a geração ao navigation handler das vistas que lê quando chegar (107f662).
- **infra-egress — portão de saída da IA, consumo mensal e worker preguiçoso (569d80a, 1306765):** sem mudança visível: nenhuma feature o pede ainda (a Tradução é a primeira). `neural_core::ai_policy` diz que dado pode ir para onde: o que o utilizador escreveu, o texto de uma página, respostas das IAs, a memória local e áudio/imagem, para este processo, um servidor deste PC, a rede local ou a Internet; o que fica no PC vai sem perguntar, o que sai precisa de consentimento, e a memória local nunca vai para a Internet. O `EgressGate` (`crates/neural-app/src/egress.rs`) decide cada envio: responde mandar, perguntar (um cartão com os hosts, a estimativa de tokens, o modelo e a faixa de preço) ou recusar com uma razão em pt-BR. O consentimento vale por par (cérebro, site) e fica só em memória até fechar o NeuralIA; sem site (um ficheiro local, um PDF, uma página `data:`, o ditado) vale só para a mesma finalidade com a mesma classe de dado, e o «sim» ao áudio do ditado não manda o texto de um PDF local (1306765). «Sempre neste site» só existe onde o desenho o pede (a Tradução), é gravado numa loja do tipo definição e nunca é oferecido nem lido a partir de um contexto privado; a loja é relida em cada pedido, por isso revogá-lo (`EgressGate::revoke_site_grant`, que também tira o consentimento da sessão desse par) vale já no pedido seguinte de todas as janelas abertas (1306765). Em segundo plano (o Radar, mais tarde) nada é perguntado: só se manda o que o utilizador escreveu, com uma autorização criada num cartão próprio para aquela vigia e aquele cérebro, de no máximo 24 envios por dia e 30 dias, revogável; o texto de uma página e a memória são sempre recusados. No Modo privado (ainda um esboço, até a onda 4) nada sai do PC. Cada chamada paga que o portão deixa sair conta em `<data_dir>/ai/usage.json` (do tipo definição desde 1306765, para o modo privado das lojas não saltar a gravação e o limite não recomeçar: por mês, cérebro e finalidade, compartilhado entre janelas sem perder contas), e acima do limite mensal suave (200 chamadas por omissão, em `<data_dir>/ai/settings.json`, que traz também as finalidades Tradução e Ditado) o clique pergunta «Limite mensal atingido (200 chamadas) — continuar?» e o segundo plano não manda. Antes de decidir, o portão relê o `usage.json`, por isso o que as outras janelas já gravaram conta (1306765); o limite é suave: dois cliques no mesmo instante em duas janelas, antes de qualquer das duas gravar, podem passar os dois, e os dois contam. O mês é o do calendário UTC. A gravação corre na thread `neural-usage`, um `LazyWorker` que só nasce na primeira chamada paga: o portão é criado no primeiro pedido, nunca na inicialização, e a Home fica com as threads de sempre.

### Security
- **Log de depuração (417c448):** a redação do log ligado por `NEURALIA_DEBUG_LOG` passa a tapar também chaves `sk-`, `sk-proj-` e `sk-ant-`, tokens `Bearer` e os valores dos cabeçalhos `x-api-key` e `x-goog-api-key`, além das formas que já tapava (`key=`, `"key":"…"` e chaves `AIza…`).
- **Downloads recusados nas páginas locais e no monitor do Gmail (e813ba1):** além dos livros, que já recusavam, o Leitor, o PDF, o Gemini Live e o painel do Ctrl+H recusam qualquer download antes de começar (`DownloadPolicy::Deny` na tabela dos ganchos), e o monitor escondido do Gmail — uma WebView de 1×1 fora do ecrã que ninguém vê — também: um download começado por ele seria gravado em silêncio. As colunas, a fonte ao lado, a Web completa e os painéis de serviço ficam com os downloads do WebView2 como antes (o gestor de downloads da 2.3 entra por aí). Gate: `the_webview_hooks_table`; e, desde a3c44b8, `every_webview_gets_the_hooks` prende o hospedeiro que cada um dos 11 sítios de nascimento declara — a revisão de e813ba1 mostrou que o monitor do Gmail nascido como `External` (a cadeia da Web inteira e os downloads do WebView2) ficava verde.

### Testing
- **infra-settings-keys (417c448):** gates novos das lojas (`a_missing_file_gives_defaults_and_writes_nothing`, `a_corrupt_file_is_kept_backed_up_and_never_overwritten`, `a_file_from_a_future_version_is_read_only`, `an_oversize_file_is_never_parsed_and_never_written`, `two_writers_never_lose_an_update`, `store_kind_is_mandatory`, `mint_once`, `a_store_cannot_open_without_a_grant` e oito doctests `compile_fail`), das chaves (`every_slot_round_trips_encrypted_and_forgets`, `a_live_key_file_from_before_the_refactor_still_loads`, `one_slots_entropy_cannot_open_another_slots_file`, `api_keys_have_the_shape_of_their_slot`, `redaction_hides_every_key_shape`) e do produto (`existing_stores_have_a_declared_kind`, `the_store_registry_is_minted_once_in_app_new`, `keys_survive_clear_history`, `no_panel_payload_builder_takes_a_key`), cada um com sabotagem vermelha no commit. O teste de janela do pedido de chave muda o foco, por isso fica fora da suíte local (`#[ignore]`) e corre no passo novo "Focus-affecting window gates (desktop session)" do job `windows`, que exige a linha `ok`; a matriz de sabotagem do CI ganha três entradas contra ele (`secret-prompt-destroyed-with-the-key`, `secret-prompt-enter-keeps-the-key`, `secret-prompt-topmost`). A feature de CI `test-stores` (registo das lojas para testes) só é ligada nas `[dev-dependencies]` do neural-app: o passo "Published exe has the test-stores feature off" prova no grafo de build (`cargo tree -e features,no-dev`) que o exe publicado a tem desligada, e `scripts/test-release-contract.mjs` prende isso.
- **infra-settings-keys — revisão (f7eec65):** o `a_store_cannot_open_without_a_grant` só procurava funções `pub` que devolvessem um grant ou o registo, e um `impl From<PathBuf> for StoreGrant` fabricava um grant com tudo verde. Passa a ler o código do módulo em tokens e exige: nenhum `impl <Trait> for` nas duas (só os dois `impl` inerentes), nenhum atributo além do `#[derive(Debug)]`, um grant construído só no `grant` e um registo só no `build` (que só as duas cunhagens chamam), e nada de `unsafe`, `macro_rules!`, `include!`, submódulos ou apelidos (`type`, `use`, `const`, `static`) com as duas; o módulo ganha `#![forbid(unsafe_code)]`. Dois doctests `compile_fail` novos (`.into()` de um caminho para o grant e para o registo). `store_kind_is_mandatory` e `grant_names_never_leave_the_data_dir` cobrem os nomes com outras maiúsculas e com ponto final, e `an_oversize_file_is_never_parsed_and_never_written` exige que o grande demais não ganhe `.bak`. Sabotagens vermelhas no commit: o `impl From`, um grant forjado noutra função pública, o nome comparado com maiúsculas, o ponto final aceite e a cópia do ficheiro grande demais.
- **infra-notify-popups (ab80e28, d07b94a):** gates novos `notify_route_table`, `queue_order_and_dedupe`, `gmail_toast_behaviour_unchanged` (texto, tamanho, botões em sete escalas, canto, prazo, token e respostas contra as fórmulas da 2.2.0), `popup_menus_keep_their_items_and_decide_the_focus_by_origin` (também: um só `TrackPopupMenu(` no código que embarca), `native_card_arms_expires_and_confirms_only_the_painted` e `splash_question_keeps_the_yes_no_geometry_and_answers_by_button`. Três gates reais Win32 criam uma janela que pode ir para a frente, abrem um menu modal ou mexem no teclado, por isso ficam fora da suíte local (`#[ignore]`) e correm num passo próprio do job `windows` do CI (`--ignored --exact`, um de cada vez; o passo falha se algum não imprimir `ok`): `toast_never_activates`, `native_card_never_activates` e `popup_menu_restores_origin_focus`. Sabotagens vermelhas locais: o `notify_route` a ignorar o Foco, o modo privado a deixar passar o corpo e o "Abrir" do Gmail sem abrir nada; a matriz de sabotagem do CI ganha `toast-shown-with-sw-show` (`SW_SHOW` no lugar de `SW_SHOWNOACTIVATE`), `card-activates-on-click` (`WM_MOUSEACTIVATE` fora da regra dos popups), `menu-skips-focus-restore` e `notify-route-ignores-focus`, e prova de novo os gates só-CI sobre o código restaurado. Em d07b94a, `queue_order_and_dedupe` ganha a ordem Gmail à vista → Foco ligado → Pomodoro à espera (sai no fim do Gmail, à frente de um download que espera o fim do Foco) e `popup_menus_keep_their_items_and_decide_the_focus_by_origin` lê o `GetMenuState` do menu que o `PopupMenu` monta, sem janela (cinzento e marcado, com e sem amostra de cor); os dois ficaram vermelhos no código de antes da correção.
- **infra-accel-spike (693bf7f, 530ef1c, 78b7dc8):** workflow próprio `Accelerator spike` (`.github/workflows/accel-spike.yml`, windows-latest), em cada PR e à mão (`workflow_dispatch`), fora do workflow `CI` de que o `release.yml` depende: um atalho que falhe é um resultado previsto do spike e nunca impede uma release. O exe de release compilado com a feature de CI `accel-spike` instala um `AcceleratorKeyPressed` que marca `Handled` só nos dez atalhos nativos por omissão do plano da 2.3 (Ctrl+D, Ctrl+J, Ctrl+Shift+E/A/N/P/S/F, F1, Ctrl+O) nas WebViews da coluna, do Split, do Split privado, da Web externa, do Leitor, do PDF, dos livros (EPUB), do painel do Ctrl+H e do painel de serviços; `scripts/test-accel-spike.ps1` carrega em cada atalho em cada uma (uma vez e com a tecla presa), com uma fixture em 127.0.0.1 e o `NEURALIA_KEYMAP_SCRIPT` real, e imprime a tabela por hospedeiro (registo e resumo do job) com a regra do spike: a página nunca vê o keydown, nenhum `act()` do mapa de teclas dispara e o lado nativo dispara exatamente uma vez. A coluna carrega a fixture por uma exceção de navegação que só existe no exe do spike (a origem exata que o condutor pediu, `column_fixture_navigation`, com teste e sabotagem vermelha); o gate de navegação da coluna que embarca não muda, e se o runtime não deixar a coluna fica na página ao vivo e a nota da tabela di-lo. O entregável é a tabela: o passo sai com 0 quando ela está completa e a corrida é válida, mesmo com atalhos a falhar a regra (ficam na tabela para a decisão do dono, §7), e só falha por um erro do condutor ou por uma corrida inválida. Uma tentativa cujo `SendInput` falhou, uma tentativa não medida (o hospedeiro não abriu, não ficou com o foco ou deixou de responder — a corrida continua no hospedeiro seguinte) ou uma corrida em que nenhuma tecla chegou a um `AcceleratorKeyPressed` (linha "EXECUCAO INVALIDA" no topo da tabela e do resumo do job) conta como inválida e não sugere fallback; o `-SelfTest` corre o mesmo relatório sobre uma corrida sintética de nove hospedeiros na forma real do registo. O mesmo job sabota `Handled=false` e exige que a tabela mostre a página a ver a tecla. Nada disto chega ao utilizador: o exe publicado é compilado sem a feature — o job `windows` do `ci.yml` prova que o marcador do spike não está no `ci-tested/NeuralIA.exe` (`scripts/test-accel-spike-marker.ps1`) e `scripts/test-release-contract.mjs` prende isso aos workflows, com sabotagens vermelhas em 693bf7f (a feature por omissão, o build do `windows` com a feature, o marcador pedido presente, o módulo sem `cfg`, um upload no job do spike) e em 530ef1c (o spike de volta ao `ci.yml`, o `release.yml` à espera do spike, o spike no push a `main`, o passo do marcador com `if` ou `continue-on-error`, um build do `windows` com `-F accel-spike`, um `needs:` em lista sobre o spike, um upload no workflow do spike). A tabela e a decisão que ela pede ao dono (despacho nativo, guarda de uma linha no mapa de teclas ou número IPC reservado) saem da primeira corrida deste workflow.
- **accel-spike — a coluna volta a ser medida (3d673b5):** na corrida 36197975480 a coluna saiu "inválido" sem ter sido tentada: o exe escreve o `hello` antes de construir o comparador do `NEURALIA_STARTUP_INPUT` e só lê comandos depois dele (~40 s nesse job, ~28 s na corrida da sabotagem), e o prazo de 30 s do `open` do primeiro hospedeiro contava desde o `hello`. O condutor manda agora primeiro um `ping` com prazo próprio (`-StartupMs`, 120 s; sem resposta é um erro do condutor, com o motivo) e anota na tabela a latência do arranque; um `open Column` recusado sai logo com o motivo. O exe do spike escreve no registo o que o WebView2 fez da navegação da coluna para a fixture (`colnav`: o `Cancel` que o gate que embarca deixou, o que ficou depois da exceção e o `NavigationCompleted`); se ela vier sem sucesso o condutor recua logo, e também se a sonda não a vir em 20 s: o exe repõe a página ao vivo do fornecedor e a nota da tabela diz onde a coluna foi medida e porquê. A leitura do registo deixou de contar duas ou três vezes uma linha apanhada antes do fim de linha. Gates: `-SelfTest` (61 casos) e `cargo test -p neural-app [--features accel-spike] accel_spike`, com sabotagens vermelhas (a leitura antiga do registo, a navegação falhada tomada por sucesso, o `aberto=` do `ping` lido à larga, o `NavigationId` ignorado, o `ping` com argumento). O E2E só corre no CI.
- **infra-llm-transport (867bf3e, ac1078f):** gates novos no `neural-core`, sobre um servidor stub em 127.0.0.1 (nenhum teste sai da máquina): `a_302_is_not_followed`, `a_2_mib_body_gives_too_large` (Content-Length declarado, corpo sem tamanho e bomba gzip), `the_key_is_never_in_the_request_line`, `status_mapping`, `errors_never_carry_key_or_body`, `pick_rule_table_over_every_denied_family` (fixture `crates/neural-core/tests/fixtures/llm/pick-rule-models.json`: ids reais de cada família negada e um chamariz `*-flash-lite` mais novo que só o termo negado tira), `no_model_id_is_hard_coded` e `release_has_no_endpoint_override` (ausência no código-fonte de `src/llm/`), mais `a_call_stops_at_its_own_deadline`, `cancel_is_cooperative`, `pinned_urls_stay_on_the_pinned_host`, `generate_content_request_and_answer`, `the_owner_pin_and_the_price_tier` e, num binário próprio, `the_ai_client_ignores_proxy_variables_and_keeps_its_policy`. Sabotagem vermelha nos commits para `max_redirects(5)`, a chave em `?key=`, o corpo no `Display`, o tecto descodificado retirado, o loopback fora de `cfg(test)` (contrato de release e gate Rust), um termo negado retirado e o `proxy(None)` retirado. O job `windows` do CI ganha o passo "Published exe has no LLM endpoint override" (`scripts/test-exe-no-endpoint-override.ps1`): procura no `ci-tested/NeuralIA.exe`, em ASCII e UTF-16LE, nomes de variáveis de ambiente que desviariam o host ou apontariam para um fixture (`NEURALIA_*BASE_URL*`, `*ENDPOINT*`, `*FIXTURE*`, `OPENAI_BASE_URL`, `OLLAMA_HOST`…), depois de provar que vê nomes plantados; o `scripts/test-release-contract.mjs` prende esse passo e o `cfg(test)` do loopback. Enquanto nenhuma feature chamar o transporte, esta varredura guarda sobretudo o que vier depois.
- **infra-llm-transport — revisão (b67d1c1):** o `status_mapping` só tinha um 429 inventado da Gemini ("Resource has been exhausted.", sem a palavra faturação), e por isso passava enquanto o 429 real por minuto ("check your plan and billing details", com `RetryInfo`) saía sempre «Sem créditos ou limite de gastos» e a espera do `retryDelay` nunca chegava ao cartão. A tabela ganha os corpos com a forma dos reais (tirada de relatos públicos, nenhum pedido feito): a quota por minuto (`retryDelay` 37 s → «tente em 37 s»), a diária (`retryDelay` 49 s → sem créditos), o `insufficient_quota` da OpenAI com o texto real (que também fala de faturação) e um 400 `credit balance`. Sabotagens vermelhas no commit: a faturação a contar como créditos num 429, a verificação da quota diária desligada e a razão explícita de créditos ignorada num 429.
- **infra-webview-hooks (e813ba1):** gates novos `the_webview_hooks_table`, `every_webview_gets_the_hooks` (um registador e um builder gravados percorrem cada tipo de hospedeiro: o `AcceleratorKeyPressed` em todos, os itens de menu só nos que rolam, a trava igual à cadeia, os downloads recusados onde a tabela manda, o `PageLoaded` só no fim do carregamento; e cada um dos 11 sítios onde uma WebView nasce passa pelas duas metades com o seu hospedeiro), `navigation_verdicts_are_the_ones_the_builders_gave` (os closures de navegação da 2.2.0, letra por letra, como oráculo sobre 91 alvos × 14 cadeias — veredicto e evento), `custom_schemes_are_never_answered_by_the_resource_gate` e `accelerator_lookup_binds_nothing_today`; `context_menu_commands_are_unique` cobre o registo dos itens das WebViews (ids únicos, nunca 0, encontráveis pelo id); `spec_0108_remote_navigation_handlers_reject_neuralia_scheme` passa a proibir qualquer `with_navigation_handler` fora do módulo dos ganchos e a exigir a recusa do `neuralia:` nas cadeias remotas; `tests/spec_product_wiring.rs` lê o módulo novo. Sabotagens vermelhas locais (uma compilação cada, ficheiro restaurado byte a byte): o Leitor a nascer sem `install_webview_hooks`; o despachante a responder a `neuralia-pdf`; a cadeia Web a abrir a rede local inteira; a cadeia do Leitor a agir sobre `https://…/home`; o Leitor com downloads do WebView2; o `AcceleratorKeyPressed` saltado; a fonte privada sem o item de rolagem. Uma sabotagem ficou verde e fica registada em vez de disfarçada com uma asserção de texto: tirar a recusa explícita do `neuralia:` da cadeia Web não muda veredicto nenhum, porque `remote_web_target` e `is_view_source_target` já o recusam — é defesa em profundidade, não um gate. A matriz de sabotagem do CI ganha `reader-born-without-hooks` e `resource-gate-answers-neuralia-pdf`. Nenhum teste novo precisa do desktop; nenhum toca na rede.
- **infra-webview-hooks — correções da revisão (a3c44b8):** dois achados altos. (1) `every_webview_gets_the_hooks` só prendia o hospedeiro do `hooked_builder` em 6 dos 11 sítios: trocar `GmailMonitor` por `External` no `gmail.rs` (ou `Live` por `External` no `panels.rs`) deixava a suíte inteira verde, com o monitor escondido do Gmail a aceitar qualquer https e a descarregar em silêncio — o contrário do que a entrada de Security dizia. Agora o `hooked_builder` devolve um `HookedBuilder` (campos privados) com o hospedeiro, e só `build_hooked` / `build_hooked_as_child` chamam o `build` do wry e registam a metade do COM para esse mesmo hospedeiro; o gate proíbe o `build` / `build_as_child` do wry fora do módulo, conta os 11 `build_hooked` contra os 11 `hooked_builder`, prende a linha do `hooked_builder` de cada sítio (o hospedeiro literal colado ao builder que embrulha, uma só vez) e só admite um `let host = WebViewHost::` (a fonte ao lado). (2) A entrada `menu-label-frozen-at-registration` da matriz de sabotagem do CI apontava para o `ColumnMenuRequest` que e813ba1 apagou: o `Replace-Exact` lançava antes do `try`, o passo "Prove 100-interaction UI gates reject sabotage" falhava e as duas entradas novas nunca corriam. Reancorada em `webview_menu_responder` (`webview_hooks.rs`); `reader-born-without-hooks` passa a construir o Leitor pelo `build` do wry directamente; entrada nova `gmail-monitor-gets-web-chain`. `scripts/test-release-contract.mjs` lê a matriz como o pwsh a vê (o bloco `run` sem os dez espaços, as here-strings sem a linha final) e exige que cada âncora `Old` case exactamente uma vez no seu ficheiro, que cada `Test` seja um `#[test]` único do neural-app e que cada `Ignored` bata com um `#[ignore]` em `windows_app/tests.rs` — enquanto era escrito apanhou a âncora do Leitor com a indentação errada (0 ocorrências). Sabotagens vermelhas locais (uma compilação cada, três delas aplicadas a partir das próprias entradas do `ci.yml`, ficheiro restaurado byte a byte por SHA-256): o rótulo do menu preso no registo (`Column(0): rotulo preso`), o monitor do Gmail nascido como `External`, o Leitor construído pelo `build` do wry, o Gemini Live nascido como `External`. SPEC-0005 descreve a forma nova.
- **infra-llm-untrusted (21eb543, 6d33111, bbe7056, 107f662):** a suíte do Reader (`reader::tests`, `security::tests`, `tests/http_reader.rs`, `tests/reader_block_scope.rs`, `tests/extraction_cost.rs`) fica verde depois da mudança do `PublicResolver`. Gates novos: `resolver_tables` (27 endereços × 3 localidades, pela `Locality::admits` e pelos resolvedores reais sobre URIs com IP literal, sem DNS), `the_pinned_transport_resolves_only_public_addresses` (uma armadilha em 127.0.0.1 resolvida como `Public` ou `Lan` não recebe ligação nenhuma; o controlo `Loopback` chega), `fence_cannot_be_closed` (22 páginas hostis, incluindo o marcador de fecho com o nonce certo, parecidos, invisíveis e hífens suaves pelo meio, letras cirílicas e de largura total, com tabela de parecidos própria do teste), `bidi_stripped` (os 1 114 112 pontos de código contra o predicado do teste, Trojan Source e etiquetas pelo construtor), `remote_redaction`, `prompt_slots_are_separate`, `fresh_nonces_are_128_bit_and_differ`, `injection_signals_table`, `shingles_carry_and_compare`, dois doctests `compile_fail` (texto de página como instrução, `&str` como dado) e, no `neural-app`, `page_eval_delivers_a_read_that_arrives_in_time_on_the_same_page`, `page_eval_drops_a_late_read`, `page_eval_drops_an_over_cap_read`, `page_eval_drops_a_navigated_read` (recarregar com o mesmo URL, `pushState`, vista fechada), `page_eval_refuses_no_page_a_failed_eval_and_too_many_reads` e `page_eval_scripts_are_read_only`. Sabotagens vermelhas nos commits: o `PublicResolver` a aceitar o loopback (`resolver_tables`), o agente fixado com o resolvedor por omissão do ureq (gate do transporte), os `＜`/`＞` de largura total tirados da tabela de parecidos (`fence_cannot_be_closed`), os controlos bidi fora da limpeza (`bidi_stripped`), o destino remoto sem redação (`remote_redaction`) e a verificação da geração desligada (`page_eval_drops_a_navigated_read`); esta última entra também na matriz de sabotagem do CI (`page-eval-skips-generation`).
- **infra-llm-untrusted — revisão (8590e50, 5f84ea9):** o `page_eval_scripts_are_read_only` ficava verde com um script em `READ_ONLY_SCRIPTS` que recarregava a página (`location.reload()`), andava no histórico (`history.go(-1)`), mudava `location.search`, submetia um formulário (`requestSubmit`) e punha o foco num campo (`focus()`), e o `page_eval.rs` dizia que o gate provava que nenhum navegava. O gate passa a ler o texto com os escapes `\u` desfeitos e um espaço entre identificadores (o `return history` já não vira `returnhistory`), a recusar nomes inteiros por efeito, `location` fora das leituras, chaves de texto entre colchetes (`x['nome']`) e atribuições a `src`, `href`, `name`, `scrollTop`, aos globais e aos `on*`; o gate novo `page_eval_read_only_gate_refuses_acting_scripts` exige que 48 scripts que agem (os cinco da revisão primeiro) fiquem vermelhos e que o de captura de notas, um de leitura do DOM e a troca do `nodeValue` da Tradução passem (5f84ea9). Na cerca, letras Lisu, matemáticas (também o `ℯ` e as gregas), a armênia U+054D e versaletes escreviam a palavra do marcador sem a partir, e três `ᚲ` rúnicos passavam como sinais: o `fence_cannot_be_closed` passa de 22 para 30 páginas hostis e o `injection_signals_table` ganha quatro linhas, com as tabelas próprias do teste alargadas à parte (8590e50). Sabotagens vermelhas nos commits: a sonda da revisão em `READ_ONLY_SCRIPTS`, a regra do `location` desligada, a compactação total dos espaços, os escapes `\u` por desfazer e o `focus` fora da lista (5f84ea9); o Lisu, as letras matemáticas, o rúnico e o armênio fora das tabelas (8590e50).
- **infra-egress (569d80a):** gates novos `decide_table` (clique e segundo plano × classe do dado × autorização × privacidade × limite, 3 840 combinações mais as linhas ditas à mão), `background_never_sends_without_matching_grant` (autorização de outra vigia, de outro cérebro, revogada, esgotada, esticada no arquivo ou ainda não válida nunca manda, e o segundo plano nunca pergunta), `ledger_counts_every_paid_call_and_caps` (duas janelas no mesmo `usage.json` não perdem contas), `lazy_worker_spawns_nothing_until_first_job`, `the_usage_writer_starts_on_the_first_paid_call`, `app_new_starts_no_lazy_worker`, `consent_ledger_is_session_only`, `site_grants_only_where_offered_and_never_from_private`, `standing_grants_are_bounded`, `latest_wins_and_generation_cancel`, `the_card_shows_hosts_tokens_model_and_price_tier` e, no `neural-core`, `may_send_table` e `choose_engine_keeps_the_rules_that_never_change`; `existing_stores_have_a_declared_kind` ganha `ai/settings.json` (definição) e `ai/usage.json` (automático em 569d80a, definição desde 1306765). Sabotagem vermelha no commit: aceitar a autorização de outra vigia, deixar passar o texto de uma página em segundo plano, não contar a chamada paga, criar e usar um `LazyWorker` no `App::new` (e no construtor do portão) e gravar o consentimento da sessão no disco. O neural-app passa a declarar o `serde` que o `neural-core` já usava: nenhum crate novo no `Cargo.lock`. O corpo de 569d80a dá o `egress.rs` restaurado com o SHA-256 `e3657f72`; o ficheiro commitado tem `9af1bbdc` (as âncoras das cinco sabotagens foram reconferidas nele: nenhuma ficou).
- **infra-egress — correções da revisão (1306765):** três achados médios e dois baixos. (1) O limite decidia com o `usage.json` que a janela leu no primeiro pedido: com o limite em 3, a janela A mandava 1, a B mandava 2 e A voltava a mandar sem cartão. Agora cada decisão de um destino pago relê o ficheiro e soma o que a janela contou e ainda não gravou; a releitura e a gravação da thread `neural-usage` não se cruzam (nada conta duas vezes). Gate novo `the_cap_counts_what_other_windows_sent`; `the_usage_writer_starts_on_the_first_paid_call` confere a conta exata em cada um dos 30 envios com a thread a gravar ao lado. (2) O «Sempre nesta sessão» sem site cobria tudo o que não tem site, de qualquer finalidade e dado: o do ditado mandava o texto de `file:///C:/Users/x/contrato.pdf` sem cartão. Gate novo `session_consent_without_a_site_stays_with_its_feature_and_data` (`file:`, `neuralia-pdf:`, `data:`, origem com mais de 256 caracteres). (3) «Sempre neste site» revogado noutra janela continuava a mandar até reiniciar, na janela que o leu e na que o deu (a resposta gravava-o também na sessão). `site_grants_only_where_offered_and_never_from_private` passa a revogar com as janelas abertas e a exercer `revoke_site_grant`. (4) `ai/usage.json` era automático e não definição como o desenho pedia: com o modo privado das lojas ligado, as chamadas pagas ficavam só em memória, nada era gravado e a sessão seguinte recomeçava o limite do zero. Gate novo `usage_survives_the_private_store_mode`. (5) O hash de restauro de 569d80a (acima). Sabotagens vermelhas locais (uma compilação cada, ficheiro restaurado byte a byte por SHA-256): o `usage.json` lido uma vez por portão, o consentimento sem site a ignorar finalidade e dado, o «Sempre» gravado a semear também a sessão, a loja do «Sempre» lida uma vez, a revogação a deixar a sessão, e `ai/usage.json` de volta a automático.

## [2.2.0] - 2026-09-25

### Added
- **feat/tools — Pomodoro (c58783f, fd02f27, 1b9f846, 956e92d, 1278242):** botão na linha do título, na barra e na Home, com o tempo que falta ("mm:ss", "⏸ mm:ss" pausado) sem tirar largura às colunas das IAs. Clique inicia, pausa e retoma; o botão direito abre o menu (Pular fase, Parar, 25/5, 50/10 e 15/3 min); `pomodoro:` funciona na omnibox e na paleta. No fim de cada fase há um som, um aviso no meio da janela e, com ela minimizada ou atrás de outra, o botão a piscar na barra de tarefas sem roubar o foco; o aviso que a janela não viu aparece quando ela volta. As durações ficam em `<data_dir>/pomodoro`.
- **feat/tools — Notas (Zettelkasten) (9603fac, 42653f4, d5c7cd7, 956e92d, bfb8156):** aba Notas no painel do Ctrl+H, com busca, lista, editor (título, corpo, tags), ligações `[[id]]`, notas que ligam para esta e Excluir para `.trash`, em Markdown em `<data_dir>/zettel`. Ctrl+Shift+Z numa página cria uma nota da seleção com a fonte (nunca no Split privado); na Home abre uma nota em branco. O salvar leva a revisão aberta: se a nota mudou fora deste editor (outra janela, o Obsidian), o texto vai para uma cópia "(conflito)" em vez de a esmagar.
- **feat/tools — Respiração (c58783f):** botão que abre o vídeo de respiração guiada (método Wim Hof) no painel lateral, em WebView2 InPrivate, sem câmera nem microfone, sem login e preso ao YouTube.
- **feat/tools — Ler em voz alta no PDF e no Modo Leitura (068bd8f, f334e85, 956e92d; SPEC-0110, fase offline):** Ctrl+Shift+U ou o botão de alto-falante lê frase a frase com as vozes instaladas do Windows, com realce, velocidade e a voz escolhida por língua; a língua é a do documento (`/Lang` do PDF ou o texto). Por omissão só vozes locais: uma voz online só aparece se o runtime do WebView2 a oferecer, e só por escolha.
- **feat/selection-toolbar — barra de seleção (pedido do dono; remodelada na 2.2.0 em eb63a68, 59caccf, f3382c3):** ao selecionar texto numa página (arrastar, duplo ou triplo clique, Shift+clique, Shift+setas, Ctrl+A) aparece uma barra com 🤖 Mandar para IA, 📝 Salvar nota, 🌐 Traduzir, 📋 Copiar e "⋯" (Mais), nas colunas do comparador, no Split, na Web externa e no Reader. O "⋯" abre, junto dele, um menu com 🔊 Falar: abre e fecha no clique, a barra não sai do sítio (o menu abre para o lado longe do texto selecionado, ou para onde couber), dá para navegar nele pelo teclado (setas, Home/End, Enter e Espaço; Tab sai), e o primeiro Esc fecha só o menu — durante a leitura, o primeiro Esc para a leitura e fecha a barra. Copiar usa a área de transferência; Falar lê com uma voz local do sistema (nunca uma voz online), uma frase de cada vez, e o segundo clique para; durante a leitura o Parar continua à mão no menu, também depois de a seleção sumir. Numa página sem síntese de voz não aparece o "⋯"; sem voz local instalada, o Falar avisa "Nenhuma voz local disponível". No Split privado não há Mandar para IA nem Traduzir. Em breve (próximas ondas da 2.2.0): 💡 Explicar e 📊 Extrair entram no menu "⋯" (a lista `MORE_MENU` do script); ainda não existem, e a barra não mostra botões sem ação.
- **feat/selection-toolbar — confirmação nativa do Mandar para IA e do Traduzir:** os dois botões só pedem. A NeuralIA mostra no centro da janela um cartão nativo — "Mandar para as 3 IAs?" com os botões Mandar e Cancelar, ou "Traduzir nas 3 IAs?" com Traduzir e Cancelar — e o texto. O texto aparece até onde cabe na caixa; o que não cabe fica de fora, marcado com "…" e "+N caracteres ficam de fora", e não vai. Só um clique em Mandar (ou Traduzir) no cartão, a partir de 600 ms depois de o texto aparecer, envia às três IAs exatamente o texto que o cartão mostrou; no Traduzir, as três IAs recebem o pedido fixo "Traduza para o português do Brasil (se o texto já estiver em português, traduza para o inglês):", uma linha em branco e esse texto. No Histórico, na memória e na sessão de pesquisa o Traduzir fica com o nome do texto ("Traduzir: <texto>"), e o Histórico guarda `traduzir:<texto>`, que ao ser reaberto refaz o mesmo pedido; `traduzir:` também funciona na omnibox. Cancelar ou 12 s sem resposta descartam o pedido, e um pedido novo (de qualquer um dos dois botões) substitui o anterior. O cartão não tira o foco da página, e a página não pode tapá-lo, movê-lo nem clicá-lo.
- **release/2.2.0 — Salvar nota na barra de seleção (eb63a68, 59caccf):** grava o texto selecionado como nota nova do Zettelkasten (a mesma pasta `<data_dir>/zettel` e o mesmo worker das Notas), sem cartão e sem abrir o painel: um aviso no meio da janela diz "Nota salva: <início do texto>". A nota é o texto que a barra mostra, enviado com o pedido: a NeuralIA não volta a ler a página ao salvar. O título da nota são os primeiros 60 caracteres do texto; a fonte é o endereço que a NeuralIA conhece da própria aba (ou o artigo do Leitor, o PDF aberto), nunca um endereço ou título informados pela página. O mesmo texto de novo em menos de 2 s não vira outra nota, mesmo com outro texto salvo no meio. Uma seleção que não cabe no pedido (8 KiB: em português ou inglês corrente, os 5000 caracteres que a barra aceita; em escritas de 3 bytes por caractere, como chinês ou japonês, cerca de 2700) não é enviada, e a barra diz "Seleção grande demais para Salvar nota". No Split privado funciona, e o aviso diz "Modo privado: a nota foi guardada"; nada vai para o histórico nem para a memória. O Ctrl+Shift+Z continua como antes (e recusado no Split privado); com a tecla presa, faz uma nota só.
- **feat/epub — leitor e biblioteca de livros EPUB (26a9ccf):** suporte a livros `.epub` em biblioteca local (`livros:`, `biblioteca:`, `books:`, `library:`, ou arrastar arquivo para a janela, ou `Ctrl+O` na Home); leitor com navegação por capítulos, paginação, índice, progresso de leitura, busca no livro, modo escuro/claro e leitura em voz alta com síntese de voz local. Parsing seguro com limites rígidos de metadados, recusa de entidades XML e DRM, e renderização em iframe isolado sob a origem `http://neuralia-epub.localhost` com CSP restritivo (`script-src 'none'`). Canal IPC dedicado e isolado (`parse_epub_ipc`) com lista fechada de 8 mensagens sem alterar a contagem de 31 ações de `ipc.rs`.

### Changed
- **release/2.2.0 — código da janela dividido em módulos (split-windows-app-a/b: 7080682..2d4f96c; split-windows-app-c: PR #142):** o `windows_app.rs` (cerca de 45 mil linhas) passa a uma árvore de módulos em `crates/neural-app/src/windows_app/` (tema, ícones, barra, linha de abas, scripts das páginas, painel lateral, notas, serviços, cartão de pesquisa, e o `impl App` por domínio em `app/`). Não muda comportamento: a lista de testes é a mesma, e os itens do agente continuam na raiz até decisão do dono.
- **CI — job windows com 35 min de limite (PR #141):** as execuções verdes da 2.2.0 já levavam quase 20 min, e uma execução com todos os passos verdes era cortada no limite antigo de 20.
- **Releases em dois ritmos (PR #141; AGENTS.md §2.1):** as versões `X.0.Z` (3.0.0, 4.0.0…) são LTS e aparecem como a versão mais recente (Latest) no GitHub. As outras versões (2.2.0, 3.1.0…) saem mais depressa como prévia (pre-release): continuam disponíveis para descarregar, mas não substituem a Latest. O `scripts/test-release-contract.mjs` testa a regra do canal sobre uma tabela de versões.
- **feat/selection-toolbar — duplo clique nas colunas:** um duplo clique numa palavra seleciona-a e mostra a barra de seleção em vez de expandir a coluna; um duplo clique que não deixa texto selecionado continua a expandir.

### Fixed
- **release/2.2.0 — "Apagar histórico" nos Livros esquece também os livros removidos (2addad2):** o Ctrl+Shift+Delete promete que nos Livros o registro de quando cada livro foi aberto desaparece. Um livro removido guardava a última abertura no registro da lixeira (`.trash/<sha256>.json`) e, adicionado de novo, voltava ao "Continuar lendo" e à ordem dos recentes. Agora `Library::clear_last_opened` tira a última abertura desses registros (a posição e os marcadores ficam, como nos livros da biblioteca) e vale também para um livro que outra janela acrescentou depois de esta ter lido o índice.
- **release/2.2.0 — "Apagar histórico" nos Livros chega à outra janela aberta (5126bbd):** com duas janelas da NeuralIA na mesma biblioteca, a que não apagou guardava em memória quando cada livro foi aberto e continuava a mostrar o "Continuar lendo" até o índice mudar ou a janela reabrir (no disco já estava apagado). Agora `Library::clear_last_opened` sobe um contador em `state.generation`, na pasta da biblioteca, e o `Library::refresh` que a outra janela faz antes de cada pedido ao worker dos Livros relê o estado dos livros quando ele muda: o "Continuar lendo" sai dela no pedido seguinte (abrir um livro, virar a página, acrescentar ou remover um livro ou um marcador); até lá, continua à vista nela.
- **README — Livros (EPUB) (de3f90d):** a omnibox abre um livro com `epub:<caminho>`; um caminho `.epub` solto é recusado como caminho local, e o README dizia que servia. Os atalhos da tabela do teclado não chegam à biblioteca nem ao leitor de EPUB (a WebView dos Livros não recebe o mapa de teclas): o README dizia "em todas as superfícies" e agora assinala a exceção dos Livros, onde valem `Ctrl+O`, `Ctrl+F` e `Esc`.
- **feat/tools — uma nota a meio nunca se perde quando o painel fecha (bfb8156):** todas as saídas do painel do Ctrl+H (X, Ctrl+H, abrir um item, outro painel, Home, pesquisa nova, a tela de erro, a Web completa, o Leitor, o PDF, um link externo, fechar a janela) passam por uma saída única que grava o que o editor tinha por salvar antes de a página sair, uma vez só. Antes, a troca de superfície (`destroy_web_surfaces`) largava o painel sem gravar. Uma cópia que chega de um painel já fechado é gravada em vez de ser substituída pela do painel seguinte. O painel largado sem passar por essa saída (substituído por outro, largado de vez) ainda grava o rascunho, uma vez (e9926ec). Com a fila das notas cheia (64 pedidos à espera de um disco lento), o salvar da página — o X, o Esc e o botão Notas salvam e fecham — e o rascunho de um fecho entram na fila na mesma, pela ordem e sem travar a janela; só listar, buscar, abrir, excluir e criar da seleção respondem "ocupadas" (e9926ec; antes, o salvar do X era recusado e o texto perdia-se). Ao fechar a janela, a espera vai pela mesma fila, atrás do rascunho: a janela só fecha com ele no disco, no máximo 3 s depois (e9926ec; antes, a espera podia passar à frente do rascunho). A cópia do editor segue a escrita com no máximo 200 ms de atraso, mesmo a escrever sem parar (antes, uma rajada inteira ficava sem cópia até a pausa).
- **feat/tools — teclado de volta ao fechar um painel (d5c7cd7, bfb8156):** fechar a Respiração, outro serviço ou o Ctrl+H na Home devolve o teclado à omnibox; fora da Home, à janela.
- **release/2.2.0 — revisão da barra de seleção (f3382c3):** abrir o "⋯" já não move a barra: antes, com a barra por cima do texto, o menu entrava como uma linha nova, a barra subia e o 🔊 Falar ficava exatamente onde estava o "⋯", e um segundo clique no mesmo ponto (para fechar o menu, ou um duplo clique) começava a ler a seleção em voz alta; agora o menu flutua junto do "⋯", fora da caixa da barra. Cada Traduzir tinha no Histórico e na memória o mesmo título (o início do pedido fixo) e, com um texto acima de ~1940 caracteres, não reabria (a entrada passava do tecto de 2048 do painel); agora o nome é o do texto e a entrada é `traduzir:<texto>`. Segurar Ctrl+Shift+Z fazia uma nota idêntica por cada repetição da tecla, cada uma a abrir o painel das Notas; agora o mapa de teclas ignora as repetições e o nativo não grava a mesma nota duas vezes em 2 s. A dica do botão da Respiração, com o painel dela aberto ou minimizado, diz agora o que o clique faz ("… minimizado · clique para voltar ao painel", "… aberto ao lado · clique para fechar"), como os ícones dos outros serviços.

### Security
- **neural-core — base de downloads, provedores e ZIP (8b965ed, 4908049, 7327872; ainda sem ligação à app):** classificação de risco de ficheiros descarregados e guarda do alvo da app padrão; registo único dos provedores (hosts, nomes, seletores); o leitor de ZIP seguro do EPUB passa a módulo partilhado `safezip`, com os mesmos limites do EPUB.
- **neural-core — risco de downloads (b2bb25d; ainda sem ligação à app):** `classify_download_name` bloqueia tipos que correm com um duplo clique e antes davam Seguro: Python (`.py`, `.pyw`, `.pyz`, `.pyzw`, `.pyc`, `.pyo`), `.wsb`, `.msu`, `.mst`, `.ps1xml`, `.ps2xml`, `.psc1`, `.psc2`, `.sct`, `.shb`, `.shs`, `.rdp`, `.theme`, `.themepack`, `.xbap`, `.jnlp` e as bases do Access (`.mda`, `.mdb`, `.mde`, `.accde`, `.ade`, `.adp`), também disfarçados (`invoice.pdf.pyw`). `.xll` (uma DLL nativa do Excel) passa de aviso a bloqueio, e um documento com macro depois de uma extensão de fachada (`fatura.pdf.docm`) é disfarce e bloqueia. `sniff_download` reconhece `@echo off` sem distinguir maiúsculas e depois de um BOM UTF-8. `default_app_target` recebe só o caminho e lê ele mesmo os primeiros 4 KiB do arquivo; sem os conseguir ler (não existe, é uma pasta) não há alvo. Antes, sem bytes, certificava um arquivo que ninguém tinha lido.
- **neural-core — risco de downloads, revisão (beba7e5; ainda sem ligação à app):** `.ppsm` (apresentação com macros que abre direto no modo de exibição) e os binários antigos que também levam macros, `.xlsb`, `.xlm`, `.xla`, `.ppa` e `.pps`, davam Seguro e passam a aviso (e a bloqueio atrás de uma fachada: `fatura.pdf.ppsm`). O documento com macro disfarçado bloqueia também com segmentos em branco pelo meio (`fatura.pdf. .docm`, `fatura.pdf..docm`), com um ponto que não é o ASCII (`fatura．pdf.docm`) e atrás de tipos que antes não contavam como fachada: os que a própria NeuralIA abre (`livro.epub.docm`, `nota.md.xlsm`), páginas (`fatura.html.docm`) e mais imagens, áudio, vídeo e compactados (`foto.heic.docm`, `digitalizacao.tif.docm`). Um nome com caracteres de formato que não se veem (os Default_Ignorable do Unicode — ZWSP, word joiner, soft hyphen, BOM, preenchimentos Hangul, caracteres tag — e o braille vazio) bloqueia, como os bidi, e o `display_label` marca-os (`‹U+200B›`); o ZWJ, o ZWNJ e os seletores de variação (emoji, persa) não bloqueiam, e a análise da extensão corre sem eles. `default_app_target` deixa de aceitar `.svg`: no Windows abre no navegador, que corre o `<script>` do arquivo a partir de `file://`; fica com "Mostrar na pasta", como o `.html`. `sniff_download` reconhece `@ echo off` e `@@echo off`, que o `cmd.exe` corre como `@echo off`.
- **neural-core — registro de provedores (de3f90d; ainda sem ligação à app):** `is_ai_provider_host` e `ProviderId::from_url` leem as regras de host das linhas do registro (`HostRule`) e aceitam só http(s). O Google IA vale só em `google.com` e `www.google.com` e só com o primeiro `udm` igual a 50 (antes, qualquer `*.google.com`, como `sites.google.com`, e um `udm=50` em qualquer posição); DeepSeek, Mistral e Grok só em `chat.deepseek.com`, `chat.mistral.ai` e `grok.com`; Gemini só em `gemini.google.com`. Só os 3 slots padrão (Google IA, ChatGPT, Claude) são selecionáveis até ao item de provedores; os outros 5 ficam no registro para serem reconhecidos. `all_self_names` é juntado das linhas do registro. O `onProviderPage` do script das colunas não mudou e continua a aceitar `*.google.com` com `udm=50` (divergência escrita no gate `script_hosts_subset_of_registry`).
- **feat/selection-toolbar — IPC (SPEC-0005/SPEC-0108/SPEC-0015):** o conjunto fechado ganha `search` (a 31.ª ação na release/2.2.0; `PUBLISHED_ACTION_COUNT` = 31, gate `protocol_accepts_exactly_the_published_actions`, que lê a SPEC-0005, a SPEC-0108 e a SPEC-0015). `search` leva exatamente `text` (de 1 a 2000 caracteres depois de aparado, sem caracteres de controle além de `\n` e `\t`) e `intent`, um de dois nomes fechados: `ask` (Mandar para IA) ou `translate` (Traduzir); sem `intent`, ou com outro nome, vazio, em maiúsculas ou de outro tipo, é recusado, nunca lido como `ask` (eb63a68). A página só diz qual dos dois botões foi: o título do cartão, o botão de confirmar e o pedido de tradução são escritos pelo nativo. `search` é aceito das colunas, do Split normal, da Web externa e do Reader e recusado pelo Split privado, com qualquer `intent`. O nativo nunca envia nada às IAs só por receber `search`: abre o cartão de confirmação, e o texto segue como pergunta, nunca como comando da omnibox (`agent:`, `tema:` ou uma URL selecionados são perguntas).
- **release/2.2.0 — `note` do Salvar nota (eb63a68; revisto na revisão da barra):** além do pedido sem argumentos do Ctrl+Shift+Z, `note` aceita só `{"via":"bar","text":…}`, com o texto que a barra mostra (de 1 a 5000 caracteres depois de aparado, sem caracteres de controle além de `\n` e `\t`, e o pedido inteiro dentro dos 8 KiB do canal); um endereço ou título, `via` sem `text`, outro `via` ou outro tipo é recusado. Antes, a barra mandava só `{"via":"bar"}` e o nativo voltava a ler a seleção com um script no mundo da página: uma página que trocasse o `window.getSelection` decidia o corpo, o título e o aviso "Nota salva: …" de cada nota, ao lado da fonte verdadeira. Agora o texto é o que a barra leu com as primitivas capturadas no document-created (o mesmo caminho do Mandar), e o nativo não volta a perguntar à página. O Salvar nota só conta um clique confiável numa barra intacta, à vista há 500 ms (o mesmo filtro do Mandar para IA e do Traduzir). A fonte vem do lado nativo (`webview.url()`, ou o artigo do Leitor e o PDF abertos), só se for http(s), e o título vem do texto. No Split privado o Salvar nota grava por pedido explícito de quem lê; `note_read_view` devolve com a WebView se ela é a do Split privado, e é isso que escolhe o aviso "Modo privado: a nota foi guardada".
- **feat/selection-toolbar — o cartão é o que se confirma:** a pergunta é limpa no nativo antes de o cartão a mostrar, e a limpeza vale para a pergunta, não só para o desenho: saem os caracteres que se pintam como nada (Default_Ignorable_Code_Point do Unicode — caracteres tag, o "ASCII smuggling", seletores de variação, zero-width e controlos bidi, soft hyphen, BOM, preenchimentos Hangul —, o braille vazio, as âncoras de anotação e U+FFFC, a área privada e os não-caracteres), e quebras, tabs e controlos viram um espaço. O cartão mede o texto com a fonte e o formato com que o desenha e envia só o que pintou: dois textos que pintam o mesmo cartão levam a mesma pergunta.
- **feat/selection-toolbar — página hostil:** a barra vive numa shadow root fechada montada no document-created e usa primitivas capturadas nesse momento (seleção, eventos, medidas, `String`, e os acessores de `SpeechSynthesisVoice` e `SpeechSynthesisUtterance`): uma página que os redefine não muda o texto enviado nem faz passar uma voz online por local. O Falar escolhe a voz numa só passagem, sem lista intermédia, e volta a confirmar `localService` antes de falar; as listas da barra crescem por `Object.defineProperty` capturado, nunca por `[[Set]]`, e um acessor no índice 0 de `Array.prototype` ou `Object.prototype` não troca a voz nem as frases. No Split privado, `open_split_mode` passa o seu `private` a `split_open_plan`, e `configure_split_webview` monta com o resultado o perfil anônimo, o script sem Mandar para IA nem Traduzir, o handler IPC que recusa `search` e o de popups, que abre os popups de um Split privado como Split privado.

### Known limitations
- Um fecho do painel pelo lado nativo (Ctrl+H, outro painel, Home, troca de superfície, fechar a janela) perde o que se escreveu nos últimos 200 ms antes dele.
- Fechar o painel enquanto o primeiro Salvar de uma nota **nova** ainda está a caminho pode gravá-la em duplicado (nunca perdida).
- Ao fechar a janela, a espera pelas notas é de no máximo 3 s: com um disco que não responde durante mais tempo, o que ainda estiver na fila das notas perde-se.
- Ler em voz alta: a voz online depende do que o runtime do WebView2 oferece; o motor online próprio (API oficial, com chave, custo e envio do texto decididos pelo dono) não está nesta versão.
- Ctrl+Shift+Z: o texto da nota é lido de novo da página no momento de salvar, no mundo dela; uma página hostil que troque o `getSelection` muda o texto (e, fora do Leitor e do PDF, o título e a fonte) dessa nota. O Salvar nota da barra não tem este problema: o texto vai com o pedido.
- Salvar nota: no Split privado a nota guarda o endereço da página como fonte, como nas outras abas (é um pedido explícito, e o aviso diz que foi no modo privado). Uma seleção que não cabe nos 8 KiB do pedido não é salva pela barra (em português ou inglês corrente isso não acontece dentro dos 5000 caracteres da barra; em chinês ou japonês, acima de cerca de 2700).
- Barra de seleção: uma mensagem na barra (por exemplo "Clique de novo em …" ou "Nenhuma voz local disponível") ainda a faz crescer e voltar a posicionar-se; só o menu do "⋯" deixou de a mover.

### Testing
- **release/2.2.0 — gates depois da divisão (PR #142):** os 17 testes que liam só o texto do ficheiro-raiz passam a ler todos os módulos que embarcam (`shipped_source()`); a lista `ALL_SOURCES` do `spec_product_wiring` é conferida contra a árvore no disco; o gate de cada método que troca de superfície desligar o Gemini Live cobre os módulos-filho e a superfície EPUB; gate novo do wiring do EPUB (canal IPC próprio, trava de navegação, recusas, Apagar histórico, arrastar ficheiros). Cada um visto vermelho com a sua sabotagem e restaurado byte a byte.
- **release/2.2.0 — auditoria do neural-core (de3f90d, b2bb25d, 9fa9727, 2addad2):** gates novos ou refeitos, cada um visto vermelho com a sua sabotagem (uma compilação cada, restaurada byte a byte). `script_hosts_subset_of_registry` passa a `crates/neural-app/tests/registry_and_docs_consistency.rs` e lê o `onProviderPage` do `COMPARATOR_INJECT_SCRIPT` que embarca: cada página que ele aceita tem de ser aceita pelo registro (antes era uma lista fixa no neural-core, e um host novo no script ficava verde; como a função é lida está na entrada da revisão, abaixo). Registro: `every_registry_host_refuses_its_lookalikes` (gerado das linhas: `evil<host>`, `<host>.evil.io`, outros esquemas, subdomínios e `udm`), `only_the_default_slots_are_selectable_until_the_providers_item` e `every_provider_has_answer_fixture_or_is_marked_unreadable`, agora com fixtures HTML (sintéticas: provam que o seletor escolhe a resposta e não a pergunta, não o DOM ao vivo). Sabotagens: sufixo sem o ponto e sufixo por `contains` (vermelhos nos dois crates), qualquer esquema, o último `udm` em vez do primeiro, Perplexity selecionável, seletor inexistente e os hosts do ChatGPT trocados. Risco de downloads: a tabela passa de 95 para 135 nomes e `every_listed_dangerous_type_blocks_alone_and_behind_a_decoy`; sabotagens sem `.py`, sem `.xll`, sem a regra da macro disfarçada, com o sniffing opcional, sem tirar o BOM e com `@echo` sensível a maiúsculas. ZIP: `central_directory_cuts_reach_the_directory_parser_and_never_panic` corta o diretório central (clássico e ZIP64) com um fim que declara o tamanho cortado, e cada corte chega ao parser; os cortes no fim do arquivo, que paravam todos na busca do fim, ficam num teste próprio. Sabotagens: a leitura do nome com `expect` (vermelho nos cortes; os cortes no fim ficam verdes) e a truncagem sem o erro próprio. Livros: `clearing_the_history_also_forgets_removed_books_in_the_trash` e `clearing_the_history_reaches_books_another_window_added`; sabotagens sem limpar a lixeira e só com os livros em memória.
- **release/2.2.0 — revisão da auditoria do neural-core (beba7e5, 5126bbd, dba6c5c):** `script_hosts_subset_of_registry` deixa de ler o `onProviderPage` como texto. Parte a função em símbolos e só a aceita inteira dentro de uma gramática fechada (qualquer outro símbolo ou forma — aspas duplas, `!`, `indexOf`, `here.origin`, um comentário — fica vermelho até ser ensinado ao gate), avalia-a com a precedência do JavaScript sobre uma tabela de endereços gerada do registro e dos literais do script (hosts, subdomínios, lookalikes, o Google sem `udm=50`) e exige que o script declare `onProviderPage` uma vez só e só a chame. Antes ficavam verdes quatro derivas que aceitavam páginas que o registro recusa (aspas duplas com `return 1`, `indexOf` com `return !0`, a precedência do `||` e do `&&`, uma segunda declaração); são agora controles negativos em `drifts_that_accept_pages_the_registry_refuses_are_caught`, e `the_reader_gives_and_precedence_over_or` prende a precedência. Risco de downloads: `every_macro_enabled_office_type_warns_alone_and_blocks_behind_a_decoy` (os 10 tipos OOXML com macros escritos no teste, não lidos da lista do produto), `a_macro_behind_a_decoy_blocks_through_blanks_invisibles_and_lookalike_dots`, `invisible_format_characters_block_and_text_joiners_do_not`, `every_default_app_type_is_a_decoy`, `an_svg_never_reaches_the_default_app` e `display_label_marks_invisible_format_characters`. Livros: `clearing_the_history_reaches_what_another_open_window_shows` (duas instâncias de `Library` na mesma pasta). Sabotagens, uma compilação cada, restauradas byte a byte, todas vermelhas: sem `.ppsm`; sem bloquear os invisíveis; sem saltar os segmentos em branco; sem os sósias do ponto; `.svg` de volta ao aplicativo padrão; os joiners dentro da análise; `@ echo off` recusado; sem subir a geração; o registro sem `chat.openai.com` e com o sufixo sem o ponto (os dois vermelhos no gate do neural-app).
- **feat/epub (26a9ccf):** gates de parsing seguro de EPUB, limites rígidos de metadados (`MAX_METADATA_ITEMS = 64`, `MAX_METADATA_REFINES = 512`), teto de alocação de pico de memória medido por alocador de rastreamento (`test_alloc`), recusa de caminhos maliciosos/zip-slip, proteção contra entidades XML e DRM, escrita atômica segura da biblioteca (`library.json`), e isolamento do canal IPC de 8 mensagens da biblioteca e do leitor.
- **feat/tools (bfb8156, 1278242):** gates sobre o caminho que embarca, cada um visto vermelho com a sua sabotagem e restaurado byte a byte: `every_way_out_of_the_side_panel_saves_the_note_being_typed_once` (o editor que embarca, o parser do canal, o `SidePanel` e o `ZettelWorker` real sobre uma pasta temporária, por cada saída), `a_note_sent_by_a_panel_that_already_closed_is_saved_and_never_reaches_the_next_one`, `a_full_notes_queue_still_takes_the_note_of_a_closing_panel`, `the_native_copy_of_a_note_is_never_more_than_200_ms_behind`, `closing_a_panel_gives_the_keyboard_back` (um EDIT Win32 escondido e `GetFocus`), `the_app_tick_announces_a_phase_end_as_the_window_can_see_it` e `a_note_request_reads_its_own_webview_and_never_the_private_split` sobre os tipos do comparador. A saída normal do painel do Ctrl+H é `SidePanel::dismiss` (grava o rascunho, depois larga a página e devolve o teclado), e um `take()` direto da página não compila.
- **feat/tools (e9926ec):** largar o painel do Ctrl+H de outra forma — uma atribuição por cima, `mem::replace`, `drop` — compila (a afirmação anterior, "largá-lo de outra forma já não compila", era falsa), mas passa pelo `Drop` do `SidePanel`, que grava o rascunho uma vez, antes de a página sair; só não devolve o teclado. Só um `mem::forget` do painel ou o processo morto a meio passam por cima. Gates, cada um visto vermelho com a sua sabotagem e restaurado byte a byte: `dropping_or_overwriting_an_open_side_panel_still_saves_the_note_once` (sem o resgate no `Drop`: vermelho; `dismiss` e `Drop` a gravarem os dois: vermelho), `closing_the_panel_by_its_x_saves_the_note_even_with_the_notes_queue_full` (o X, o Esc e o botão Notas com 64 pedidos na fila e o `ZettelWorker` real preso; o salvar pelo tecto da fila: vermelho), `closing_the_window_waits_for_the_rescued_note_behind_a_full_queue` (a espera antes do rascunho: vermelho na ordem da fila e, sozinho, no worker real) e `a_full_notes_queue_still_takes_the_note_of_a_closing_panel`, refeito.
- **feat/selection-toolbar:** gates sobre o caminho que embarca — os scripts injetados correm no Node com a página hostil simulada; o cartão por `pesquisar_asks_the_native_card_and_only_its_search_click_compares`, `the_search_card_shows_plain_bounded_text_and_answers_only_its_buttons`, `the_search_card_confirms_only_the_text_it_painted` (a pintura real num bitmap) e `the_search_card_window_answers_a_native_press_and_release_with_the_painted_token`; o Split privado por `open_split_mode_hands_its_private_flag_to_the_builder` (o builder real, com um registo no lugar do WebViewBuilder); a voz por `falar_never_takes_an_online_voice_the_page_disguised_as_local`; o duplo clique por `double_clicking_a_word_in_a_column_selects_it_instead_of_expanding`. Os gates que leem o `windows_app.rs` normalizam CRLF para LF (checkout Windows com `core.autocrlf=true`). Cada gate foi quebrado de propósito e ficou vermelho (prova de sabotagem nos commits da branch).
- **release/2.2.0 — integração de feat/selection-toolbar:** o canal IPC passa a 31 ações (`search` é a 31.ª; `PUBLISHED_ACTION_COUNT` = 31 e SPEC-0005/SPEC-0108/SPEC-0015 acompanham). O Split continua montado por `configure_split_webview` a partir do `SplitBuild` de `split_open_plan`, agora chamado por `open_split_opened_by` (2.1.7): o popup do Split normal volta a nascer como aba no grupo da aba de onde saiu (`OpenSplitFromSplit`, que a barra de seleção tinha trocado por `OpenSplit`); o do privado continua privado. O Split recusa `search` quando é privado e trata o `note` (Ctrl+Shift+Z) como antes. O Modo Leitura injeta o mapa de teclas com a barra de seleção (`bind_page_script`) e a leitura em voz alta. Gates adaptados sem mudar de nome: `open_split_mode_hands_its_private_flag_to_the_builder` (o popup do Split normal é `OpenSplitFromSplit`, e o texto prende que `open_split_mode` entrega o seu `private` a `open_split_opened_by`). Sabotagens (uma compilação cada, todas vermelhas, restauradas byte a byte): o cartão a confirmar a pergunta inteira em vez do que pintou (`the_search_card_confirms_only_the_text_it_painted`), o popup do Split normal de volta a `OpenSplit` e `open_split_mode` a passar `false` (`open_split_mode_hands_its_private_flag_to_the_builder`), e de novo o `Drop` do painel sem resgate, a dica de outra coluna e a roda desviada.
- **release/2.2.0 — barra de seleção remodelada (eb63a68, 59caccf):** gates sobre o caminho que embarca (o mapa de teclas injetado corre no Node com a página hostil simulada; o cartão, o parser e o worker das notas são os do produto): `the_selection_toolbar_offers_four_actions_and_a_menu_for_a_trusted_selection` (renomeado de `the_selection_toolbar_offers_three_actions_for_a_trusted_selection`: ordem e rótulos dos quatro botões e do "⋯"), `the_selection_menu_opens_closes_and_is_reachable_by_keyboard` (novo), `traduzir_sends_the_fixed_prompt_only_after_the_native_confirm` (novo), `salvar_nota_saves_one_note_with_the_native_source_and_never_twice_in_two_seconds` (novo: o envelope que a barra manda — desde a revisão da barra, `{"via":"bar","text":…}` —, normal e no Split privado; a fonte nativa; uma nota por texto em 2 s pelo `ZettelWorker` real numa pasta temporária; o aviso privado; e a asserção de ausência de histórico, memória e painel no ramo do Salvar nota); adaptados sem mudar de nome: `search_carries_the_selected_text_within_bounds` e `note_is_a_bare_request_and_carries_no_page_data` (`ipc.rs`), `the_selection_toolbar_searches_only_what_fits_and_never_from_private`, `pesquisar_only_counts_a_click_on_a_bar_the_user_really_saw` (Salvar nota e Traduzir com o mesmo filtro), `every_page_surface_gets_the_toolbar_its_privacy_allows`, `open_split_mode_hands_its_private_flag_to_the_builder`, `a_selected_search_reaches_the_comparator_from_every_surface_but_the_private_split` (os dois `intent`), `a_note_request_reads_its_own_webview_and_never_the_private_split` (`note_read_view` devolve também se a WebView é a do Split privado) e os gates do Falar, que passam pelo "⋯". Sabotagens (uma compilação cada, restauradas byte a byte, todas vermelhas): o Traduzir confirmado já no pedido, sem o cartão (`traduzir_sends_the_fixed_prompt_only_after_the_native_confirm`); um `intent` desconhecido lido como `ask` (`search_carries_the_selected_text_within_bounds`); a nota da barra a cair no endereço da página (`salvar_nota_…`); o Split privado a mostrar o Traduzir (`every_page_surface_gets_the_toolbar_its_privacy_allows`, `open_split_mode_hands_its_private_flag_to_the_builder`, `the_selection_toolbar_searches_only_what_fits_and_never_from_private`); o Split privado a aceitar `search` com `translate` (`a_selected_search_reaches_the_comparator_from_every_surface_but_the_private_split` e os dois anteriores); sem supressão de repetidos e com ela só para o último texto (`salvar_nota_…`, 7 e 5 notas em vez de 3); o Salvar nota sem o filtro de clique (`pesquisar_only_counts_a_click_on_a_bar_the_user_really_saw`); o Split privado lido como normal no aviso (`salvar_nota_…`); a recusa do Split privado trocada do Ctrl+Shift+Z para a barra (`a_note_request_reads_its_own_webview_and_never_the_private_split` e `salvar_nota_…`); e, como amostra dos gates não críticos, o Esc a fechar a barra junto com o menu (`the_selection_menu_opens_closes_and_is_reachable_by_keyboard`).
- **release/2.2.0 — revisão da barra de seleção (f3382c3):** gates novos sobre o caminho que embarca: `salvar_nota_saves_the_readers_text_even_when_the_page_swaps_get_selection` (a barra injetada no Node com uma página que troca `getSelection`, `Selection.prototype.toString`, `String` e `JSON.stringify` depois do document-created; o parser do canal, o evento da coluna e `bar_note_step`: corpo, título e aviso são o texto de quem lê, enquanto uma releitura com o `NOTE_CAPTURE_SCRIPT` devolve o da página), `salvar_nota_sends_only_what_fits_in_the_channel` (no limite dos 8 KiB vai e o parser aceita; um byte a mais, a barra não manda), `a_held_ctrl_shift_z_saves_one_note`, `each_translation_is_named_by_its_text_and_reopens_from_history` (dois textos, dois títulos na sessão, na memória e na linha do Histórico; `traduzir:` de 2000 caracteres reabre pelo parser do painel e refaz o mesmo pedido), `the_more_menu_opens_without_moving_the_bar` (o mock faz a barra crescer quando o menu está no fluxo dela, como o navegador) e `the_button_that_opened_a_service_panel_hints_its_state`; adaptados sem mudar de nome: `note_is_a_bare_request_and_carries_no_page_data` (`ipc.rs`), `ctrl_shift_z_on_a_page_posts_a_bare_note_request` (cinco repetições da tecla), `salvar_nota_saves_one_note_with_the_native_source_and_never_twice_in_two_seconds` (o envelope com o texto; a asserção de ausência passa a exigir que o ramo do Salvar nota não use `evaluate_script` nem o `NOTE_CAPTURE_SCRIPT`), `pesquisar_asks_the_native_card_and_only_its_search_click_compares` e `traduzir_sends_the_fixed_prompt_only_after_the_native_confirm` (o cartão confirma um `CompareRequest`), `the_search_card_confirms_only_the_text_it_painted`, `a_selected_search_is_a_question_never_an_omnibox_command` (o `compare` chamado pela omnibox, pelo `traduzir:` e pelo host do cartão), `a_note_request_reads_its_own_webview_and_never_the_private_split`, `open_split_mode_hands_its_private_flag_to_the_builder` e `every_page_surface_gets_the_toolbar_its_privacy_allows`. Sabotagens (uma compilação cada, restauradas byte a byte por sha256, todas vermelhas): a barra a mandar o texto do `window.getSelection` da página (`salvar_nota_saves_the_readers_text_…`: a nota levou "Aviso do banco: … phish.example/login"); o parser do `note` sem a lista exata de chaves (`note_is_a_bare_request_…` aceitou `url`); o mapa de teclas sem o filtro de repetição (`ctrl_shift_z_on_a_page_…`: 6 pedidos em vez de 1); o Ctrl+Shift+Z sem a guarda dos 2 s (`a_held_ctrl_shift_z_saves_one_note`: 7 notas em vez de 3); o Traduzir com o nome do pedido fixo (`each_translation_…`: os dois títulos iguais); e, como amostra dos gates não críticos, o menu de volta ao fluxo da barra e a reposicionar ao abrir (`the_more_menu_opens_without_moving_the_bar`: topo 250 → 205, altura 42 → 87) e o botão da Respiração sem a dica do painel (`the_button_that_opened_a_service_panel_hints_its_state`). Conferido também no Edge 153 sem interface (CDP, cliques confiáveis): com a barra por cima do texto o menu abre por cima e a barra fica no topo 248 com 44 px; por baixo do texto, o menu abre por baixo; o segundo clique no "⋯" fecha o menu sem ler; numa página que troca o `getSelection`, o `note` leva a palavra selecionada.
- **release/2.2.0 — integração de feat/tools sobre a 2.1.8:** o canal IPC passa a 30 ações (`hint` é a 29.ª, `note` a 30.ª; `PUBLISHED_ACTION_COUNT` = 30 e SPEC-0005/SPEC-0108/SPEC-0015 acompanham). O vídeo da Respiração abre num painel de serviços como os outros da 2.1.8 (largura arrastável e gravada em `panel-width.json`, faixa Minimizar/Tela cheia/Fechar, sempre InPrivate, preso ao YouTube e sem câmera nem microfone); minimizado, o ponto aparece no botão dele nas ferramentas (`a_minimized_service_marks_the_button_that_opened_it`). Fechar um serviço devolve o teclado (`close_service_panel_in`, agora sobre o `ServicePanel`) e continua a desfazer a tela cheia, a pega e a roda. O painel do Ctrl+H (Histórico e Notas) usa a largura `PanelKind::History`, a pega, a roda e a barra fina da 2.1.8 e sai sempre por `close_side_panel(exit)`. A largura que as colunas cedem a um painel volta a ser zero fora do comparador (`open_panel_width_for`: a verificação da superfície perdia-se na junção). O menu do Pomodoro termina um arrasto de aba a meio, como o menu das abas. Com as ferramentas na linha do título, as 3 abas e o grupo de cada coluna cabem encolhidos a partir de ~880 px lógicos (antes 800 px); abaixo disso aparece o "‹N". Testes adaptados sem mudar de nome: `closing_a_panel_gives_the_keyboard_back` (a vista do serviço é o próprio painel), `an_open_breath_panel_pushes_the_comparator` (a identidade do serviço já não entra na largura), `panels_opened_from_home_leave_its_top_strip_clear` (`right_panel_top`) e `a_narrow_bar_never_shows_an_open_group_chip_without_its_tabs` (limiar 900 px). Sabotagens (uma compilação cada, todas vermelhas, restauradas byte a byte): o `Drop` do painel sem resgate (`dropping_or_overwriting_an_open_side_panel_still_saves_the_note_once`), a dica de outra coluna aceite (`column_controls_ask_for_the_centered_hint_on_trusted_hover`), a roda desviada para qualquer janela (`the_wheel_hook_only_redirects_when_the_panel_is_under_the_cursor`) e a largura cedida fora do comparador (`an_open_breath_panel_pushes_the_comparator`).
- **release/2.2.0 — gate do conjunto fechado do IPC sem contagem no nome:** `protocol_accepts_exactly_the_twenty_nine_published_actions` passa a `protocol_accepts_exactly_the_published_actions`. A contagem vive numa só constante de teste (`PUBLISHED_ACTION_COUNT`, junto de `wire_name` em `crates/neural-app/src/ipc.rs`) e o gate exige um exemplo aceite por ação e que a lista e a contagem da SPEC-0005, a lista e a contagem da SPEC-0108 e a contagem da SPEC-0015 digam exatamente o que o parser aceita; uma ação nova muda um número e as specs, não o nome do gate. A lista da SPEC-0108 passa a ter `ask` e `hint`, que o parser já aceitava. Sabotagens (uma compilação cada, todas vermelhas): uma ação `fake` com parser, `wire_name`, exemplo e contagem 30 mas fora da SPEC-0005; a SPEC-0108 sem `hint`; a SPEC-0015 a dizer 28.

## [2.1.8] - 2026-09-24

### Fixed
- **Instalador sem quadrado à volta da marca (item 1 do dono, e591f1a):** o `neural-setup` desenha a `assets/neuralia-home.png` (1200×868, sem fundo) com o alfa dela por cima do tecido; o retângulo opaco que apagava os neurónios por trás da marca saiu. Revisão: o quadro inteiro da janela passou a `paint_frame`, e o gate pinta esse quadro numa secção DIB — sem a marca, ou com um quadrado à volta dela, fica vermelho.
- **Marca e ícone novos em todo o lado (item 2, e591f1a):** `neuralia-home.png` é a marca da Home, do instalador e do Reader (neste, uma versão reduzida e recortada, `assets/neuralia-home-reader.png`, para caber no limite de 2 MB do `NavigateToString`); o `assets/logo.ico` é o ícone do projeto, regenerado a partir da arte em 10 tamanhos quadrados (16 a 256 px) por `scripts/brand-assets`, e é ele que o executável, a janela e a barra de tarefas mostram. O ícone original do dono fica em `scripts/assets-src/logo.owner.ico`. Revisão: a janela principal é criada por `main_window_attributes`, e um gate falha se ela perder o ícone da barra de título ou o da barra de tarefas; a verificação de que nada inclui os ficheiros apagados passou a apanhar caminhos com `\`.
- **Home — botões da janela só com o mouse por perto (item 3, 8961c2e):** minimizar, maximizar e fechar ficam invisíveis e sem clique até o mouse chegar a 24 px deles, e somem 300 ms depois de ele sair; o Alt+F4 continua. Revisão: na Home, os painéis da direita (Ctrl+H, serviços, Gemini Live) passam a começar abaixo da fila dos botões — antes o painel tapava a zona que os acorda e, com ele aberto, não havia como minimizar ou fechar com o mouse.
- **Roda do mouse sobre o painel lateral (item 4, 8961c2e):** girar a roda por cima do painel rola o painel, e não as 3 colunas das IAs. Revisão: só quando a janela debaixo do cursor é mesmo a do painel — uma lista aberta, o menu de contexto, o seletor de emojis (Win+.), o histórico do Win+V ou outra aplicação por cima do painel ficam com a roda delas.
- **Barra de rolagem fina (item 5, 8961c2e):** o painel do Ctrl+H e o do Gemini Live usam uma barra fina nas cores do tema, em vez da barra clássica do Windows. As páginas dos serviços (sites de terceiros) mantêm a delas.
- **YouTube em tela cheia no painel (item 6, 8961c2e):** o botão de tela cheia do próprio YouTube (ou "⛶ Tela cheia" na faixa do painel) ocupa a janela inteira; Esc ou a saída da página voltam ao painel. Revisão: em tela cheia, os botões minimizar/maximizar/fechar já não aparecem por cima do canto do vídeo (o × apanhava o clique e fechava a NeuralIA); e sair da tela cheia do YouTube já não tira a tela cheia do split.
- **Redimensionar o painel com o mouse (item 7, 8961c2e):** arrastar a borda esquerda do painel da direita muda a largura dele (entre 300 px e 60% da janela), gravada por tipo de painel em `panel-width.json`; as colunas das IAs acompanham a borda nova.
- **Dicas nos controles das colunas (item 8, 8961c2e):** o "−" e o "⛶ <IA>" de cada coluna mostram a dica grande e centrada do app ("Minimizar <IA>", "Expandir <IA>"). Revisão: sair de um controle direto para a barra já não apaga a dica do botão da barra.
- **Minimizar o YouTube (item 10, 8961c2e):** "— Minimizar" na faixa do painel de serviços tira o painel da frente e o áudio/vídeo continua; as colunas recuperam a largura, o ícone do serviço ganha um ponto (vermelho enquanto toca) e clicar nele traz o painel de volta.
- **Zoom com Ctrl+roda e pinça do touchpad (item 11, 8961c2e):** sobem e descem os mesmos degraus do Ctrl+= e do Ctrl+-, com o mesmo aviso; uma página que trata o gesto ela mesma fica com ele. Por cima de iframes (prévia de artefato, vídeo ou mapa embutidos) o gesto fica com o frame e não faz zoom: reencaminhá-lo deixaria qualquer página mudar o zoom sem um gesto teu.

### Testing
- Revisão da integração 2.1.8 (integrate/2.1.8), cada correção com o seu gate e prova de sabotagem: `the_window_buttons_stay_hidden_under_the_fullscreen_service_panel`, `hidden_caption_buttons_keep_their_place_under_the_raised_panel`, `the_home_panels_leave_the_window_buttons_reachable`, `a_window_over_the_panel_keeps_its_own_wheel`, `the_wheel_hook_only_redirects_when_the_panel_is_under_the_cursor`, `a_page_message_never_zooms_the_app_and_frames_forward_nothing`, `leaving_the_panel_fullscreen_undoes_only_what_the_panel_did`, `a_late_none_from_a_column_does_not_erase_another_hint`, `the_installer_window_paints_the_brand_on_the_tissue`, `the_main_window_is_created_with_the_project_icons`; `nothing_includes_the_brand_files_the_owner_deleted` passa a normalizar separadores e maiúsculas.

## [2.1.7] - 2026-09-23

### Added
- **Grupos de abas como no Chrome:** cada aba tem o seu **×** (vermelho sob o mouse); o grupo mostra a etiqueta na cor dele e uma linha da mesma cor por baixo das abas que lhe pertencem; botão direito no grupo (ou numa aba do grupo) abre as **9 cores**, Recolher/Expandir, Desagrupar e Fechar grupo; o menu da aba ganha "Adicionar a um novo grupo", "Mover para o grupo" e "Remover do grupo".
- **Arrastar abas e grupos com o mouse:** soltar entre membros junta ao grupo, arrastar para fora sai dele, um grupo arrasta-se inteiro e nunca cai dentro de outro; Esc cancela.
- **Abas e grupos guardados:** uma nova pesquisa já não apaga as abas nem os grupos, e eles voltam quando reabres a NeuralIA (`tabs.json`, escrita atómica, um só escritor por pasta de dados); o modo privado nunca é guardado e "Apagar histórico" também apaga as abas guardadas.
- **Botão direito dentro de uma IA:** o menu do WebView2 ganha "Desativar/Ativar rolagem automática (Ctrl+R)", nas colunas e no painel ao lado.

### Fixed
- **Grupos de abas:** uma aba aberta a partir de uma aba agrupada nasce dentro do grupo; tirar ou reagrupar uma aba do meio já não parte o grupo em dois; o limite de 32 abas já não apaga os grupos guardados.
- **Instalador — sem elipse escura:** o `neural-setup` abria uma zona de silêncio em forma de elipse à volta da marca e do texto, e via-se como um buraco escuro no meio do tecido. Saiu: os neurónios passam por trás da logo, como na Home do navegador. O título, a versão, o caminho de instalação, a etapa e a legenda leem-se por cima do tecido graças a uma orla fina na cor da página (`paint::text_on_tissue`); os botões mantêm o fundo próprio.

### Testing
- `the_neurons_pass_behind_the_brand_and_the_text` (sem zona de silêncio; neurónios por trás da marca e do texto ao longo do tempo) e `text_over_the_tissue_gets_a_thin_ring_not_a_box` (anel simétrico nas oito direções). Sabotagens: a elipse de volta e um anel torto ficam vermelhos.

## [2.1.6] - 2026-09-23

### Changed
- **feat/animated-installer — o instalador publicado volta a ser o `neural-setup`:** o `NeuralIA-Setup-<versão>-x64.exe` da release é o instalador próprio, com a marca e o tecido neuronal animado da Home, compilado por `scripts/build-windows-installer.ps1` à volta dos bytes exatos do `NeuralIA.exe` medido pelo CI (cópia conferida por SHA-256, `cargo build --release --locked -p neural-setup`). O script recusa um `-Version` diferente da versão do workspace, que é a que o instalador regista. O Inno Setup (`installer/NeuralIA.iss`) saiu.
- **feat/animated-installer — atualização por cima das 2.1.x:** sem `/D=`, o instalador reutiliza a pasta da instalação anterior (a sua entrada, senão a do Inno), substitui a NeuralIA e só depois de escrever a sua entrada apaga o `unins000.*` do Inno (reconhecido pelo `AppId` no cabeçalho do `unins000.dat`) e a chave `{8B2A98F4-7D55-4C43-ABF0-0D7D1A02C4B9}_is1`: "Aplicativos" passa a mostrar uma só NeuralIA, e o atalho do menu Iniciar abre a versão nova.
- **feat/animated-installer — linha de comandos:** `/D=<pasta>` (último argumento, à NSIS, espaços incluídos) e códigos de saída do modo silencioso: 0 sucesso, 1 falha, 2 argumentos/pasta recusados, 3 `NeuralIA.exe` em uso, 4 instalador sem carga útil.

### Fixed
- **Cliques e digitação mortos (2.1.5):** quatro popups auxiliares (divisores do comparador, botão de saída, splash e aviso do Gmail) eram mostrados com `SW_SHOW` e roubavam o foco da janela; cliques em links e botões não chegavam às páginas e a omnibox deixava de aceitar texto. Agora nascem `WS_EX_NOACTIVATE` e aparecem com `SW_SHOWNOACTIVATE`; o foco volta às páginas e os botões nativos são repintados.
- **Crash com acentos:** texto acentuado na omnibox (por exemplo "ação") fazia o prefixo de um comando cortar a meio de um caractere UTF-8 e derrubava o navegador.
- **Ícones:** os botões Home, minimizar, maximizar e fechar apareciam como quadrados brancos (o DC nascia opaco) e o "+" aparecia como "-." (elipse num botão estreito).
- **Nome no Gerenciador de Tarefas:** o processo aparece como "NeuralIA".
- **Aviso de rolagem:** a pergunta inicial aparece no centro da janela.
- **Painel lateral:** abre ao lado do comparador e empurra as colunas em vez de ficar por cima delas (a lista da paleta já não se sobrepõe).
- **Pesquisa escrita num painel:** uma pergunta escrita e enviada na caixa de uma IA (Enter ou botão de enviar) vai também às outras colunas, cada uma no seu fornecedor; num site aberto por um link, a pesquisa GET do site abre em todas as colunas, como o clique. Senhas, e-mails e formulários POST nunca são replicados.
- **feat/animated-installer — NeuralIA aberta durante a instalação:** a carga útil passa a ser escrita tudo-ou-nada (nomes de passagem, troca com reversão); um `NeuralIA.exe` em uso trava a instalação antes de qualquer escrita — a janela pede para fechar a NeuralIA, o `/S` sai com 3. Antes, um executável a correr podia ser renomeado por baixo da aplicação aberta.
- **feat/animated-installer — o desinstalador só remove a sua própria pasta:** corrido de uma cópia noutra pasta, apagava a instalação em `%LOCALAPPDATA%\Programs\NeuralIA`, o atalho e a entrada de "Aplicativos" da instalação boa. Agora apaga só a pasta onde vive, só atalhos que abram essa pasta e só uma entrada que aponte para ela; com a pasta temporária noutro disco, estaciona-se ao lado da pasta em vez de falhar com "feche a NeuralIA".
- **feat/animated-installer — dados do utilizador:** uma pasta de instalação que seja, contenha ou fique dentro da pasta dos dados (`%LOCALAPPDATA%\NeuralIA` ou `NEURALIA_DATA_DIR`: histórico, memória, notas, chave do Gemini, perfil WebView2) é recusada, e tudo o que instalar, atualizar ou desinstalar apaga é conferido contra ela antes de começar.
- **feat/animated-installer — tecido do instalador:** o relógio do tecido só anda com a janela visível; minimizada, deixa de redesenhar e retoma de onde estava.
- **feat/animated-installer (revisão) — instalação inteira ou nenhuma:** o desinstalador (a cópia do instalador, ~14 MB) passa a ser escrito junto com a carga útil, antes de qualquer troca, e uma falha nos atalhos ou na entrada de "Aplicativos" desfaz tudo: volta a versão anterior com os atalhos e a entrada de antes, ou some a instalação nova. Antes, um disco cheio depois da carga útil deixava o `NeuralIA.exe` e os atalhos sem desinstalador nem entrada.
- **feat/animated-installer (revisão) — caminhos 8.3:** atalhos e entrada de "Aplicativos" são reconhecidos pela identidade no disco, não pelo texto: com a pasta num nome 8.3 (o `%TEMP%` `C:\Users\RUNNER~1\...` dos runners do GitHub), a desinstalação deixava o atalho do menu Iniciar e o CI do Windows ficava vermelho.
- **feat/animated-installer (revisão) — desinstalar pelo Explorador:** o desinstalador sai da própria pasta antes de a apagar (o duplo clique corre-o lá dentro, e a pasta ficava vazia para trás); mantém-se enquanto um arquivo da NeuralIA estiver em uso, para a entrada de "Aplicativos" continuar a funcionar; e a cópia estacionada em `%TEMP%` é apagada por um `cmd` sem janela quando o processo termina, em vez de ficar para sempre.
- **feat/animated-installer (revisão) — textos em pt-BR:** o instalador fala como a NeuralIA ("Não foi possível", "Instalando a NeuralIA", "Criar atalho na área de trabalho", "versão").

### Added
- **Dicas (hints):** todos os botões explicam o que fazem, numa mensagem grande, centrada, em pílula com cantos suaves nas cores do tema.
- **Tema:** Sistema, Claro ou Escuro (menu no botão Home, comando `tema:`); a barra acompanha o sistema.
- **Ctrl+H:** painel lateral com o histórico inteligente, pesquisa na memória e sugestões de sites.
- **Home em tela cheia**, sem barra de título, com os botões da própria app; o × de fechar fica vermelho sob o mouse, como no Chrome, em todos os fechos.
- **Barra de ícones:** Meet, WhatsApp, YouTube, Gmail (liga/desliga avisos de e-mail, com pergunta "abrir?" que abre no painel lateral) e Privado, com o serviço a abrir no painel lateral (câmera e microfone só com o consentimento do WebView2).
- **‹ › por IA:** cada coluna tem os seus botões voltar/avançar logo depois do "+"; a fonte aberta ao lado tem o seu par junto ao rótulo.
- **"Ir" vivo:** sob o mouse o botão vira um degradê em movimento e o cursor passa a mão.
- **Ctrl+R** liga/desliga a rolagem automática com um aviso no meio da janela (recarregar continua no F5 e no Ctrl+Shift+R).
- **Gemini Live** (botão com um olho, vermelho quando ligado): o Gemini vê a tela, a câmera e ouve o microfone, e responde por voz. Pede a chave da API na primeira vez e guarda-a cifrada (DPAPI) em `gemini-live.key`; modelo `gemini-2.5-flash-native-audio-preview-12-2025`. Tudo para ao desligar, ao ir para a Home ou numa tela de erro; sessões longas sobrevivem aos limites de 2 e 10 minutos do servidor; reconexões limitadas por sessão.
- **Log de depuração** em tempo de execução (`NEURALIA_DEBUG_LOG`) para bugs intermitentes de janela.
- **Núcleo:** motores de Pomodoro e Zettelkasten em `neural-core` (a interface chega na próxima versão).

### Security
- **Ctrl+Shift+Delete** pede confirmação ("Não" por omissão) antes de apagar todo o histórico e a memória local.
- **IPC (SPEC-0005/SPEC-0108/SPEC-0015):** o conjunto fechado passa a 28 ações com `ask` (`col`, `text` de 1 a 2000 caracteres, sem caracteres de controlo além de `\n` e `\t`); uma coluna só fala por si. As specs publicavam 26 (ou 25) ações e omitiam `link`; o gate `protocol_accepts_exactly_the_twenty_eight_published_actions` lê a SPEC-0005 e falha se ela divergir do parser.
- **Gemini Live:** destino de rede novo (`generativelanguage.googleapis.com`, só enquanto ligado), documentado no `SECURITY.md`; a chave nunca vai para o log, o histórico, a memória nem para a URL da página.
- 32 correções de auditoria verificadas (instalador, core, CI/docs, agente e canal IPC), cada uma com teste e prova de sabotagem.
- **feat/animated-installer (revisão) — segredos de assinatura fora do cargo:** `scripts/build-windows-installer.ps1` lê `NEURALIA_AUTHENTICODE_PFX_B64`/`_PASSWORD` e tira-os do ambiente antes de chamar o cargo, que os passava a todos os build scripts e proc-macros compilados no passo de release. O gate `scripts/test-build-installer-isolation.ps1` (CI `installer-smoke`) corre o script com um `cargo` falso e fica vermelho se alguma chamada vir um segredo. O `packaging/build-installer.ps1`, que recompilava o `NeuralIA.exe` sem `--locked` nem smoke, saiu.

### Testing
- **fix/audit-cidocs — prova de sabotagem da UI no CI:** o passo aplicava as seis regressões Rust de uma vez e só verificava se o nome de cada gate aparecia no log, o que acontece também quando o teste passa; cinco dos seis gates podiam estar mortos com o passo verde. Agora cada uma das oito sabotagens Rust é aplicada sozinha, corre só o seu gate e exige a linha `test ...::<gate> ... FAILED`; depois de restaurar, exige `... ok` para cada gate.
- **feat/animated-installer — `installer-smoke` sobre o `neural-setup`:** `scripts/test-windows-installer.ps1` instala em silêncio com `/S /D=` numa pasta com espaço, confere o SHA-256 do `NeuralIA.exe` instalado contra o testado pelo CI, a entrada `...\Uninstall\NeuralIA` (nome, versão, pasta, comandos), o desinstalador e os atalhos, o código 3 com o executável aberto, o código 2 para a pasta dos dados e a desinstalação silenciosa; depois atualiza por cima de uma instalação Inno 2.1.x simulada e confere que os dados ficaram byte a byte iguais. Recusa correr onde a NeuralIA já está registada; `-Isolated` usa uma pasta de ensaio (`NEURALIA_SETUP_SANDBOX`). O job passa a instalar a toolchain fixada e prova também que um `-Version` diferente do workspace é recusado; o contrato de release exige o instalador à volta do binário testado.
- Os scripts Node do CI (`scripts/test-*.mjs`) passam a ser corridos localmente antes de cada push (um segundo listener de clique na coluna quebrou o `core` uma vez).

## [2.1.5] - 2026-09-22

### Fixed
- **PR #128 — auditoria recursiva de 20 passagens:** a omnibox Win32 da Home mantém o HWND estável fora da área cliente, mas fica desabilitada no comparador e deixa de poder roubar foco/teclado.
- **PR #128 — grupos de abas:** “Fechar outras abas deste grupo” e “Fechar todas deste grupo” passam a atuar somente no grupo selecionado; grupos vazios são podados e outras coleções permanecem intactas.
- **PR #128 — reagrupamento:** mover a última aba de um grupo para um novo grupo deixa de criar pílulas vazias/fantasmas.
- **PR #128 — Win32:** falha ao instalar a subclass da omnibox destrói o HWND recém-criado em vez de vazá-lo; reparenting passa a verificar o pai efetivo.
- **PR #128 — IPC:** se o `BCryptGenRandom` principal falhar, a capability tenta `RtlGenRandom` antes de falhar fechado, sem degradar para fonte pseudoaleatória.
- **PR #133 — auditoria recursiva de 100 interações:** hit-testing de tela usa bordas half-open, a palette respeita colunas estreitas e minimizar a última IA visível não destrói um Split antes de rejeitar a ação.
- **PR #133 — identidade e grupos:** abas recebem identidade estável independente da URL; URLs iguais em grupos diferentes não fecham nem realçam o Split errado; a poda de 32 abas remove grupos que fiquem órfãos.
- **PR #133 — atalhos por foco:** Ctrl+R, Ctrl+P, F12 e Ctrl+U atuam na WebView que recebeu o gesto; F11 respeita a coluna atual; 1/2/3 continuam globais através de `shortcut-expand` sem ampliar a autoridade do `expand` vindo do DOM; Ctrl+L no Split volta à IA de origem.
- **PR #133 — cliques e Split:** duplo clique em links/controles não expande a coluna junto com a ação do elemento; uma troca de Split só substitui o painel anterior depois de o novo WebView existir, preserva o estado em falha e descarta builds que terminem depois de outra navegação.
- **PR #133 — controles nativos:** minimizar/maximizar/fechar e Home só executam quando press e release pertencem ao mesmo controle; perda/cancelamento de captura limpa o estado pendente. Soltar após `ReleaseCapture` mantém o clique legítimo e arrastar para fora da altura do botão cancela a ação.
- **PR #133 — menu de grupos:** “Juntar ao grupo” deixa de oferecer o próprio grupo atual da aba, evitando reordenação sem efeito.

### Security
- **PR #128 / SPEC-0109:** câmera, microfone e captura de tela passam pelo consentimento nativo do WebView2 apenas em superfícies visíveis; agente e demais permissões continuam `Deny`.
- **PR #133 / SPEC-0108:** o conjunto fechado passa a 27 ações com `shortcut-expand` (as 26 anteriores já incluíam `link`, omitido nesta entrada até à correção em `[Unreleased]`), autenticada pela capability e separada do `expand` de autoridade local à coluna.

### Testing
- **PR #128:** gates de comportamento cobrem escopo de grupos, poda de grupos vazios, omnibox desabilitada fora da Home, fallback CSPRNG e política WebRTC.
- **PR #133:** matriz determinística de 100 cenários cobre 5 larguras × 4 escalas de DPI × 5 topologias, além de gates de bordas de clique, alvo por foco, troca transacional de Split, press→release nativo, menu de grupos e duplo clique em elementos interativos.
- **PR #133 — sabotagem automática:** o job Windows reintroduz temporariamente seis regressões Rust, as falhas de ordem de captura e limite vertical e uma regressão JS no checkout do runner, exige os gates vermelhos (até à correção em `[Unreleased]`, o passo só provava que pelo menos uma das seis regressões Rust ficava vermelha), restaura os bytes originais e repete os gates verdes antes de prosseguir.
- **PR #133 — ciclo de vida:** a medição de cinco ciclos envia press→release ao botão Home nativo, como o produto exige, e verifica zero WebViews visíveis após cada retorno.
- **PR #134 — medição WebView2:** a contagem de processos aguarda até quinze segundos quando ultrapassa o pico de aquecimento; um excesso que persiste continua a reprovar o gate de ciclo de vida.

## [2.1.4] - 2026-09-22

### Fixed
- **Chrome do comparador corrigido após auditoria recursiva.** Expandir uma IA passa a ocupar somente a área de conteúdo; a titlebar do NeuralIA e os controles minimizar, maximizar/restaurar e fechar permanecem visíveis.
- **Omnibox da Home deixa de ocupar visualmente o comparador.** O controle Win32 permanece vivo para estabilidade do HWND, mas é estacionado fora da área cliente; a desativação explícita de foco/teclado fica registrada em `[Unreleased]`.
- **Lifecycle WebView2 endurecido.** O gate só publica o comparador como pronto depois de devolver o controle ao event loop, fora do pump aninhado do WebView2.
- **Isolamento IPC entre painéis.** Uma coluna não pode mais enviar `Expand { col }` para comandar outra coluna.
- **Erros do histórico deixam de ser silenciosos.** Falhas de persistência e saturação da fila passam a ser reportadas pela interface.

### Testing
- O loop de lifecycle passa a medir, além de Working Set e processos WebView2, crescimento de **handles Win32, threads e objetos GDI**.
- Prova de sabotagem do PR #126 reintroduziu fullscreen borderless, readiness precoce e expansão cruzada; os novos gates ficaram vermelhos exatamente nesses pontos.
- O PR #124 passou os gates Windows, core, core-linux, runtime, timeline, dependency policy/audit e installer smoke antes do merge.


## [2.1.3] - 2026-09-22

### Fixed
- **PR #113 — remove regressões visuais e de foco da v2.1.2.** O botão Home volta a ser somente texto, sem ícone inventado; a omnibox deixa de ocupar a titlebar do comparador; os controles nativos de minimizar, maximizar/restaurar e fechar permanecem acessíveis acima das WebViews; e o foco/teclado deixa de depender de reparenting instável do EDIT da Home.
- **PR #113 — Ctrl+L no comparador volta ao fluxo correto.** Em vez de reutilizar a caixa da Home na barra de título, a ação abre a palette da coluna ativa, mantendo a busca fora do chrome da janela.

## [2.1.2] - 2026-09-21

### Fixed
- Auditoria visual pós-2.1.1: a omnibox permanece utilizável no comparador, permitindo novas pesquisas sem regressar à Home.
- Home passou a ter controlo Win32 nativo clicável e os controlos nativos são reparentados quando o Windows troca o HWND após mudanças de decoração.
- Fullscreen usa geometria estável e botão nativo permanente de saída, eliminando o redimensionamento por hover que causava flicker e travamentos.
- Claude preserva a consulta através de redirects e não considera a pesquisa enviada apenas porque o parâmetro `?q=` desapareceu.
- A release pública passa a publicar somente o instalador `NeuralIA-Setup-<versão>-x64.exe`.


## [2.1.1] - 2026-09-21

### Fixed
- **PR #101 — Correção de instalação e primeira pesquisa v2.1.** Corrigidas regressões no ciclo de vida da WebView e primeira instalação.
- Nome dos artefatos de release ajustado para `NeuralIA-v2.1.1-windows-x64.exe`.

## [2.1.0] - 2026-09-20

Auditoria completa do produto e do núcleo, com regressões presas a gates
determinísticos. Esta versão entrega a memória semântica SQLite/FTS5, reforça o
isolamento do agente e da WebView, renova a experiência nativa e publica
exatamente o binário aprovado pelo CI.

### Security
- **PR #69 — uma URL local escrita autoriza somente a sua origem.** A permissão
  deixa de abrir toda a rede local para a página carregada no Split View;
  redireções dentro do mesmo servidor continuam válidas, mas pivôs para
  loopback, router ou outro host voltam a exigir autorização.
- **PR #74 — o redactor deixa de devolver o segredo em linhas sem separador.**
  Entradas como `access_token ya29.SEGREDO` já não reaparecem intactas ao lado
  de um rótulo `[REDACTED]` enganoso.
- **PR #70 — URL e título passam pelo mesmo redactor do corpo.** Tokens em query
  ou fragmento deixam de chegar ao JSON, wiki, SQLite e interface; parâmetros
  não sensíveis continuam preservados.
- **PR #54 — SPEC-0104 passa a aplicar classes A/B/C/D em código nativo.**
  `CapabilityClass` deixa explícito o nível de autoridade; confirmação positiva
  pode autorizar Classe C, mas nunca transforma Classe D em ação autônoma.
  `stop()` revoga o grant reversível e origens extras antes de qualquer
  `resume()`, e o firewall textual cobre formas adicionais de senha, API/client
  secret e dados de cartão.
- **SPEC-0108 / PR #24:** superfícies WebView remotas deixam de transportar a
  capability em URLs `neuralia:?cap=...` e passam a usar mensagens WebView2
  autenticadas por capability por WebView. O parser aceita somente o envelope
  versionado e a lista fechada de 25 ações, limita o corpo a 8 KiB, valida
  argumentos e bloqueia pivô de Split View para rede local.
- Scripts capturam `chrome.webview.postMessage` e `JSON.stringify` no
  document-created; handlers de navegação remotos recusam `neuralia:`.
  Links internos do Reader continuam sendo a exceção sem token.

### Validation
- **PR #88 — o custo da captura de memória passa a ter gate determinístico.**
  O caminho real de `MemoryStore::capture` precisa executar zero varreduras do
  corpus e zero validações integrais do SQLite, mantendo o manifesto incremental
  correto inclusive ao regravar o mesmo documento.
- **PR #90 — atalhos Windows são validados pela identidade do arquivo.** O gate
  compara volume e file index, aceitando alias 8.3 do mesmo objeto e rejeitando
  outro executável mesmo quando a grafia do caminho parece equivalente.
- **PR #91 — os 204 blobs auxiliares do PDF.js têm proveniência verificável.**
  O gate confere tamanho e Git blob SHA-1, a allowlist do host, os bytes servidos
  e o MIME de cada recurso; sabotagens de WASM, CMap e fontes ficam vermelhas.
- **PR #82 — SPEC-0103 fecha a aceitação da timeline sem cronómetro absoluto.** O CI
  executa as duas funções `semanticAnchors()` que realmente embarcam contra
  fixtures ChatGPT/Gemini/Claude e mede 16→64 nós por rácio. O parser Rust de
  referência recebe o mesmo gate por fornecedor e 256→1024 secções. Mutações
  deliberadas de papel e trabalho O(n²) precisam deixar ambos vermelhos.

- **PR #83 — pipeline de release consolidado.** O executável publicado continua sendo o
  mesmo `NeuralIA.exe` medido pelo CI, com checksum verificado antes do
  empacotamento; o release gera SBOM, instalador Authenticode quando as
  credenciais existem e agora também uma attestation sobre o
  `dist/NeuralIA.exe` efetivamente publicado.

- **PR #64 — instalador per-user com caminho de Authenticode preparado.** O CI
  compila um setup Inno a partir do mesmo `NeuralIA.exe` medido, instala em
  diretório temporário, confere SHA-256 do payload, registro de uninstall em
  HKCU e desinstalação. Uma segunda montagem adultera o payload em 1 byte e o
  gate precisa rejeitá-la. O release só publica setup quando o par de segredos
  Authenticode está completo; configuração parcial falha fechado. O executável
  medido não é assinado depois do CI, porque isso mudaria os seus bytes.
- **PR #58 — release publica exatamente o executável medido pelo CI.** O job
  Windows guarda `NeuralIA.exe`, SHA-256 e os JSONs dos gates no artefato
  nomeado pelo SHA; a attestation é criada sobre esse mesmo binário. O workflow
  de release baixa o artefato do run que o disparou, verifica o checksum e
  empacota sem recompilar. PRs também passam a cancelar runs antigos do próprio
  PR, enquanto pushes em `main` permanecem não-canceláveis.
- **PR #56 — dependency policy becomes an enforced CI gate.** A SHA-pinned
  `cargo-deny` checks licenses, dependency sources and wildcard declarations
  on Linux + Windows dependency graphs. Unknown registries/git sources and
  external wildcards fail the build. `neural-app` is explicitly
  `publish = false`, so its workspace-local path dependency is accepted
  without pretending the Windows application is a crates.io library. The
  sabotage commit banned `serde` and CI #384 went red before the policy was
  restored.
- **PR #79 — `neural-core` ganha gate portátil no Linux.** Check, testes e
  clippy passam a rodar fora do Windows; o parser IPC compartilhável também é
  compilado no job principal.
- **PRs #9 e #87 — actions de attestation e download de artefatos atualizadas.**
  O pipeline permanece preso a SHAs e compatível com os formatos atuais dos
  artefatos usados na publicação.
- PR #24 adiciona testes do parser, das 25 ações, bounds, capability incorreta,
  ausência de `?cap=` nos scripts remotos, captura antecipada das primitivas
  IPC e rejeição do esquema `neuralia:` nas superfícies remotas.
- Revisão adversarial independente concluída antes da publicação da 2.1.0.

### Fixed
- **PR #67 — minimizar a janela na Home deixa de derrubar a aplicação.** O
  layout trata dimensões transitórias iguais a zero sem chamar `f64::clamp`
  com limites invertidos.
- **PR #61 — o cromado de cada coluna permanece dentro da sua coluna.** Pílulas,
  chips e controlos da direita reservam espaço antes da distribuição, evitando
  hit-tests na coluna vizinha.
- **PR #75 — capturar um documento deixa de listar todo o diretório.** O
  manifesto é atualizado incrementalmente, eliminando a última operação
  O(corpus) que permanecia em cada gravação.
- **PR #71 — o Reader deixa de duplicar blocos aninhados e de misturar metadata com o corpo.**
  Itens de lista/blocos filhos são emitidos uma vez, metadados vazios deixam a
  cadeia de fallback continuar e o conteúdo de `<head>/<title>` não entra no
  texto de fallback do artigo.
- **PR #73 — a inteligência local respeita budgets e mantém model packs dentro da raiz.**
  Resumos truncados passam a incluir a elipse dentro do próprio limite de
  caracteres, e componentes de model pack rejeitam sintaxe Windows com `:`
  (incluindo caminhos drive-relative e alternate data streams).
- **PR #66 — o visualizador PDF fecha o overlay de carregamento de forma determinística.**
  `hidden` deixa de depender da precedência de `#status { display:grid }`: o
  viewer controla também `display` diretamente. Erros HTTP/PDF passam a usar
  as classes exportadas pelo PDF.js antes do fallback textual, e documentos
  protegidos por senha deixam de ser rotulados incorretamente como PDF inválido.
- **PR #57 — o divisor nativo do comparador volta a receber o rato.** A janela
  `STATIC` do splitter devolvia `HTTRANSPARENT` no `WM_NCHITTEST`, portanto
  `WM_LBUTTONDOWN`/`WM_MOUSEMOVE` nunca chegavam à subclasse e toda a lógica de
  `SetCapture`/resize ficava morta. O gate novo cria a janela real e prova o
  hit-test, em vez de apenas procurar handlers escritos no fonte.
- **PR #54 — falhas do shortlist SQLite deixam de virar full scan silencioso.**
  Com corpus existente, índice ausente, corrompido ou impossível de abrir chega
  ao chamador como erro com instrução para `memory:rebuild`; só um perfil
  realmente vazio aceita ainda não ter índice. O gate inclui falha portátil de
  abertura, sem depender de locking específico de Windows.
- **Reader derrubava a janela** em artigos longos com acentos: o texto que vai
  para a memória era cortado com `String::truncate` num índice de byte, e 512
  KiB caem a meio de um UTF-8 quando o corpo começa em offset ímpar. O corte
  recua até à fronteira de char anterior
  (`reader_memory_text_cuts_on_char_boundary`).
- **Bridge do agente não agia em elementos com espaço no nome**: `js_percent`
  usava o serializador de formulários, que escreve o espaço como `+`, e
  `decodeURIComponent` não o desfaz. O nome esperado nunca batia com o do DOM
  — o guard do script desistia em silêncio — e `TypeText`/`Select` escreviam
  `+` no lugar dos espaços (`agent_script_encodes_spaces_as_percent_twenty`).
- **Captura de memória custava O(corpus)**: cada escrita corria um `PRAGMA
  integrity_check` sobre a base inteira e relia todos os documentos em JSON só
  para contar. Com 1500 páginas, 217 ms por captura contra 10 ms com a base
  vazia; a fila de 128 saturava e as capturas passavam a ser descartadas em
  silêncio. Agora é constante — 9 ms às 1500 páginas
  (`tests/memory_capture_cost.rs`). A verificação da base inteira fica no
  `rebuild`, que já a fazia.
- **`AgentPermissionPolicy::new(None)` desligava o gate de origem**: sem origem
  inicial, toda ação de qualquer origem passava sem confirmação. Sem origem
  inicial nada está aprovado
  (`policy_without_initial_origin_gates_every_origin`).
- **PR #44 — o recall PT/EN voltou a atravessar idiomas.** A shortlist FTS
  introduzida no PR #36 escolhia candidatos por palavra exacta antes de
  pontuar: uma pergunta em português já não encontrava o documento em inglês,
  excepto quando *nenhum* documento batia lexicalmente. Medido com dois
  documentos: `["Navegadores leves"]` com índice contra
  `["Navegadores leves", "Browser engines"]` sem ele. Cada termo passa a entrar
  no `MATCH` com os seus equivalentes, e os sinónimos vivem numa tabela única
  lida pelo embedding e pela pesquisa (`tests/semantic_recall.rs`).
- **PR #46 — um `click=` mal escrito guardava a página na memória.** Comandos de
  agente com valor vazio eram descartados em silêncio; quando eram os únicos, o
  parser punha um `extract` no lugar. E `select=` com rótulo vazio escrevia no
  primeiro `select`/`combobox` da página, escolhido por ordem do DOM. Passam a
  ser erros com a forma correcta.
- **PR #47 — `memory:rebuild` nunca reconstruiu nada.** O prefixo `memory:` era
  testado antes do comando exacto e apanhava-o primeiro: o comando procurava a
  palavra *"rebuild"* na memória e anunciava *"Buscando na memória local…"*. O
  encaminhamento sai para `route_input`, comandos exactos antes de prefixos
  (`route_input_sends_each_command_where_it_belongs`,
  `memory_rebuild_is_not_swallowed_by_the_memory_prefix`).
- **PR #41 — a bonificação de recência media o documento mais novo do corpus**,
  não o relógio. `recency_bonus(last_seen_at, now)` passa a usar a hora real.

### Changed
- **PR #62 — a razão de linearidade é medida dentro da mesma ronda.** O gate
  deixa de comparar janelas de carga distintas sem afrouxar os limites de
  crescimento aceitos.
- **PR #76 — variáveis efémeras de Authenticode deixam de parecer segredos
  persistentes ao scanner.** A mudança de nomes elimina falsos positivos sem
  expor nem reduzir a proteção das credenciais reais.
- **PR #63 — a SPEC-0105 distingue constantes de observações.** `frame="top"`
  e `visible=true` são documentados como garantias por construção do observador.
- **PR #80 — agent runtime morto removido.** O antigo `neural_core::AgentRuntime`,
  `AgentPlanner` e `AgentToolExecutor` não eram chamados pelo binário e
  duplicavam uma superfície de segurança que não protegia o produto. O
  vocabulário/configuração realmente compartilhado passa para
  `agent_protocol`; o único loop que envia ações continua em
  `windows_app.rs` conforme a SPEC-0105.

- O orçamento do caso hostil em `tests/extraction_cost.rs` deixa de ser um teto
  em segundos de relógio — que media a velocidade da máquina: falhava a 4,7 s
  com o código certo numa máquina ocupada e passaria a verde num runner rápido
  mesmo com regressão — e passa a medir o que a SPEC-0004 exige: 4x a entrada,
  no máximo 8x o tempo.
- SPEC-0104 e SPEC-0105 declaram "auditoria independente pendente", como o
  AGENTS.md §8 e o `md/README.md` já diziam.
- **PR #43 — o gate do agente passa a ser testável no código que embarca.** O
  `AgentRuntime` do `neural-core` não é usado pela aplicação: o agente do
  produto é `handle_agent_observation`. A decisão — limites, escolha do
  elemento, gate da política — sai para `decide_agent_step`, sem UI nem
  WebView. Medido: com o `policy.evaluate` retirado do caminho que embarca, o
  teste de aceitação da SPEC-0105 continuava **verde**; os sete testes novos
  ficam vermelhos, 5 de 7. Os limites passam a vir do
  `AgentRuntimeConfig::default()`, num sítio só.
- **PR #47 — a regra "navegação privada nunca entra na memória semântica"
  passa a ser testada por comportamento.** Era guardada por contagens de
  ocorrências de `if !private` no texto do ficheiro, que passam com a condição
  invertida. A decisão vive em `split_source_memory`
  (`private_split_source_never_becomes_a_memory_document`).
- **PR #47 — o orçamento do teste hostil deixa de ser instável.** Media os três
  tamanhos uma vez cada, em momentos diferentes; sob carga paralela a razão
  inflacionava e o teste ficava vermelho com o código correcto. Cada tamanho
  passa a ser medido três vezes, ficando com o mínimo.
- **PR #50 — model packs são biblioteca, não feature.** O `ModelPackManager`
  declara no próprio código o que não faz (sem download, sem escolha de
  backend, sem activação automática, sem gancho no arranque) e a SPEC-0102
  mantém-se **Parcial** com a razão escrita.
- O CI do Windows ganha `timeout-minutes: 20` e o `measure-cycles.ps1` limita a
  consulta CIM a 2 s com aviso; a detecção de fugas continua pelo delta de
  processos `msedgewebview2` contra a baseline.

### Added
- **PR #85 — experiência nativa renovada.** A Home ganha tecido neuronal denso
  e ramificado com custo limitado por contagens, animação mais legível e marca
  com alfa real; grupos de abas, arranque maximizado, auto-submit nos três
  provedores e Ctrl+clique para o painel lateral chegam no mesmo conjunto. O
  instalador nativo per-user passa a reutilizar o tecido compartilhado.
- **PR #91 — visualizador PDF realmente offline para documentos complexos.**
  O pacote passa a incluir 168 CMaps, 14 fontes padrão, OpenJPEG, JBIG2, QCMS,
  perfil ICC, licenças e fixtures. O host serve apenas rotas exatas, com CSP
  same-origin, `nosniff` e MIME explícito; não há fallback para CDN.
- **PR #51 — o parser do canal IPC passa a ser compilado e testado fora do
  Windows.** `src/ipc.rs` não tem uma chamada Win32, mas estava atrás de
  `cfg(target_os = "windows")` por arrastamento: a superfície por onde uma
  página remota fala com o nativo não era sequer compilada noutro sistema. Em
  Linux, `cargo test -p neural-app` passa de zero para 14 testes, e o job
  `core` do CI (ubuntu) passa a testá-lo e a lintá-lo.
- **PR #42 — `tests/forget_leaves_no_trace.rs`**: "esquecer" apaga os bytes, não
  só a vista da API. Os testes leem a árvore inteira do store à procura do
  texto capturado, nos âmbitos `All` e `Domain`.
- **PR #45 — proveniência do PDF.js presa ao SBOM.** O `release.yml` escrevia a
  versão à mão, sem nada a ligá-la ao ficheiro que embarca. `UPSTREAM.md`
  regista origem, versão, licença e SHA-256, e
  `tests/vendored_provenance.rs` obriga três sítios a dizer o mesmo número.
  `.gitattributes` mantém os bytes vendorizados intactos entre plataformas.
- Gates de produto para as SPEC-0100..0107 (`tests/spec_product_wiring.rs`),
  aceitação da Fase 1 da SPEC-0107, fixtures adversariais da SPEC-0104 e o
  documento `md/AUDIT-SPEC-0100-0108.md`, que classifica cada gate por aquilo
  que ele consegue mesmo provar.

## [2.0.1] - 2026-09-19

Correção da baseline 2.0 após auditoria do código real e alinhamento das
afirmações publicadas com o binário.

### Fixed
- Workflow de release trata tag + GitHub Release já existentes como no-op seguro
  quando a versão não mudou; estado parcial continua falhando para nunca mover,
  reparar silenciosamente ou republicar uma versão estável.
- Palette do comparador passa a ser controle Win32 nativo, sem campo de texto
  injetado na página e preservando o perfil privado.
- `NEURALIA_NO_GMAIL=1` desliga o monitor do Gmail para gates de lifecycle.
- Timers de splash, toast, probe e autoscroll passam por um serviço único.
- Servidor interno do PDF responde a HTTP Range para o PDF.js vendorizado.
- Histórico sai do event loop e limita entradas; escaping HTML é feito em uma
  única passagem.
- Layout/splitters, resize coalescido, caches de ícone/tema e janelas auxiliares
  foram alinhados com o comportamento testado.
- Testes tautológicos da baseline anterior foram substituídos por testes reais
  de palette, Range, timers e layout.

### Changed
- SPEC-0107 passa a declarar apenas **Fase 0 concluída**; SQLite/FTS5 continua
  pendente e não é apresentado como parte implementada da baseline.
- SPEC-0108 documenta a migração futura do transporte observável
  `neuralia:?cap=...` para WebView2 IPC; esta release não declara essa migração
  como implementada.

### Validation
- CI completo verde no candidato integrado pelo PR #14, incluindo core,
  Windows, dependency audit, build release, Home performance e cinco ciclos
  de lifecycle WebView.

## [2.0.0] - 2026-09-19

NeuralIA passa de comparador multi-IA para uma base local-first de pesquisa,
memória semântica e automação Web limitada por política.

### Added
- SPEC-0100: memória semântica file-first, SQLite/FTS5 derivado no Windows,
  entidades, relações, busca semântica local, fusão RRF, Memory Doctor e rebuild.
- SPEC-0101: Research Sessions com proveniência por provedor, comparação,
  síntese e exportação Markdown.
- SPEC-0102: inteligência local independente de modelo, embeddings offline,
  classificação, entidades, resumo, model packs, SHA-256 e benchmark.
- SPEC-0103: timeline semântica para perguntas, respostas, headings, código,
  tabelas, citações, fontes, notas e conclusões.
- SPEC-0104: permission engine do agente, política de origem, redaction,
  audit log, confirmação humana e kill switch.
- SPEC-0105: runtime de agente estruturado, limitado por passos/tempo e sem
  ferramenta de JavaScript arbitrário.
- SPEC-0107 **Fase 0**: proveniência MIT do ai-memory vendorizada com SHA upstream,
  LICENSE, UPSTREAM.md e PATCHES.md. As fases de storage/retrieval não fazem parte
  da baseline 2.0.
- Gate integrado `spec_010x_acceptance.rs` com casos numerados SPEC-0100 a
  SPEC-0107; o caso 0107 valida somente a proveniência da Fase 0.

### Changed
- `Ctrl+H` passa a abrir semantic recall local por `memory:`.
- As abas/fontes ficam na title bar superior; provedores permanecem na segunda faixa.
- O fallback semântico local normaliza aliases técnicos PT/EN.

### Security
- Navegação privada não entra na memória persistente.
- Conteúdo remoto nunca concede capacidades.
- Ações sensíveis exigem aprovação pontual; senha, cartão, OTP, CAPTCHA e
  pagamento permanecem human-only.
- Traces e decisões de política são auditáveis localmente sem valores secretos.

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
