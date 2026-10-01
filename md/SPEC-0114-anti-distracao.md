# SPEC-0114 — Anti-distração (avisos de cookies, newsletter e barras fixas)

**Status:** Implementada no `main` pela `release/2.4.0` (PR #169, após
#168 e autorização do dono). A extensão do PR #235 acrescenta o controle global
local `/distracoes [on|off|status]`, persistido na política existente, e fecha o
gate da ligação COM com E2E no EXE/WebView2 real e sabotagem determinística.

## Objetivo

Esconder, nas páginas da internet, avisos de cookies, janelas de newsletter e
barras fixas grandes. Nos CMPs conhecidos, recusar em vez de só esconder —
e **nunca aceitar**.

## O que o código faz

- `neural_core::distraction` (puro, portátil): `CMP_RULES` (OneTrust,
  Cookiebot, Didomi, Usercentrics, Quantcast/InMobi Choice, CookieYes,
  Complianz, iubenda, Osano, Axeptio, Termly, consent.google.com; Sourcepoint
  e TrustArc nunca clicados, porque o aviso vive numa moldura de outra
  origem), `REJECT_WORDS` e `ACCEPT_WORDS` (pt, en, es, fr, de, it),
  `PAYWALL_MARKERS`, `NEWSLETTER_MARKERS`, `PERSISTENT_CMP_HOSTS`, a política
  `DistractionPolicy` e a decisão `distraction_config(url, política,
  superfície)`.
- A decisão é `None` nas páginas das IAs e dos logins (registo dos
  provedores: `is_ai_provider_host`, `is_login_host`), nas nossas origens
  (`localhost`, `*.localhost`, IPs), fora da superfície web (painéis de
  serviço, Leitor, PDF, livros, Home) e num site desligado.
- `NEURALIA_DISTRACTION_SCRIPT` (`crates/neural-app/src/windows_app/distraction.rs`)
  é ligado pelo slot `distraction` da tabela dos ganchos só às colunas, às
  fontes ao lado (normal e privada) e à Web completa, pelo
  `AddScriptToExecuteOnDocumentCreated` do WebView2. Leva gravada a política
  e as regras do registo; no início do documento repete a decisão e, onde
  ela é `None`, não regista nada.
- Só no documento de topo. Captura no document-created cada primitiva que
  usa; nunca usa o `postMessage` nem recebe a capability do canal.
- Num CMP conhecido o script só age quando o aviso está visível agora (um
  CMP de quem já respondeu, com o aviso ou o centro de preferências
  escondidos, não recebe clique nem fica escondido) e só esconde o próprio
  aviso: nunca um anfitrião que o CMP reusa para as definições que o
  utilizador abre depois pelo rodapé (`PERSISTENT_CMP_HOSTS`: o centro de
  preferências do OneTrust, o `#didomi-host`, a raiz do Usercentrics — este
  só é clicado, nunca escondido).
- Um clique só acontece num botão visível de um CMP conhecido, quando o
  seletor de recusa casa **e** o texto do botão tem uma frase de
  `REJECT_WORDS` **e** não tem nenhuma palavra de `ACCEPT_WORDS`. Faltando
  uma, o aviso só é escondido.
- Sourcepoint e TrustArc: o script não lê a moldura, onde pode estar um
  «pague ou aceite» (pur abo, contentpass). A moldura que tapa metade da
  janela ou mais fica — e com ela na página a rolagem e os fundos ficam como
  a página os pôs. A pequena fica numa página com a rolagem presa; só numa
  página que rola é escondida, e isso não devolve a rolagem.
- Um marcador de paywall (no id/class, sem olhar a maiúsculas, ou no texto
  de um candidato ou de um aviso) deixa o que o tem intocado; depois de ver
  um, o script nunca mais devolve a rolagem nem esconde um fundo nessa
  página, e desfaz a rolagem que já tinha devolvido. Um paywall só escondido
  no documento (um molde) também segura a rolagem.
- A rolagem só volta quando o script escondeu a causa e o documento rola
  mesmo. Um fundo vazio de ecrã inteiro (sem texto nem filhos; nunca uma
  moldura, canvas ou vídeo da página) só sai quando é irmão do que o script
  escondeu, ou do embrulho dele.
- O que aparece até 1,5 s depois de um clique ou de uma tecla do utilizador
  (foi ele que o abriu: as definições de cookies do rodapé, uma pesquisa) e
  um diálogo com um campo de senha nunca são escondidos nem clicados.
- Janelas de newsletter: pelo id/class; pelo texto, só um diálogo cujo único
  campo é um e-mail (um checkout com a caixa «receber a newsletter» fica; o
  «boletim» do pt não é marcador).
- Barras fixas ou presas na janela, encostadas ao topo ou ao fundo, com pelo
  menos metade da largura e 25% da altura da janela (e menos de 90%), saem;
  a que contém o foco fica, e uma secção presa mais abaixo na página ou um
  menu arrumado fora do ecrã também.
- O exame dos elementos cede a vez depois de 8 ms; a procura dos CMPs
  (algumas consultas ao documento inteiro, fora desse orçamento) corre no
  máximo a cada 250 ms; de cada elemento acrescentado olha no máximo 60
  descendentes, sem copiar a árvore. Pára de observar a página depois de
  30 s sem mudanças; uma página que nunca pára de mudar é observada enquanto
  estiver aberta.
- A política vive no campo `distraction` do `adblock-settings.json` (loja
  `Setting`), com a forma de mapa `site -> bool` que a 2.3 reservou (o padrão
  desligado é `"*": false`). O item «Ocultar distrações neste site» do botão
  direito muda o site e volta a ligar o script (tira o anterior pelo id) e
  recarrega a página. No Split privado a escolha fica só em memória
  («Modo privado: esta escolha não é guardada.»).

## Gates (críticos, com sabotagem)

- `distraction_never_clicks_accept` (node:vm, uma fixture por linha de
  `CMP_RULES`; «Aceitar todos», «Concordo», «Rejeitar e aceitar» e os aceitar
  de cada língua → zero cliques);
- `cmp_rules_reject_selectors_never_name_accept` (neural-core);
- `distraction_leaves_ai_provider_pages_untouched` (neural-core e node:vm, o
  script comparado com `distraction_config` em cada regra do registo);
- `paywall_overlays_are_never_touched` (inclui as molduras do Sourcepoint e
  do TrustArc que tapam, o paywall só pelo texto, o `PaywallModal`, o fundo
  de um paywall e o paywall que chega depois de a rolagem ter voltado);
- `cmp_settings_the_user_opens_stay` e
  `cmp_banners_never_name_a_persistent_host` (neural-core);
- `per_site_toggle_off_injects_nothing` (neural-core e o caminho do app);
- `a_toggle_in_private_is_never_written`;
- `global_distraction_command_changes_the_shared_policy`;
- `global_reload_never_reloads_provider_columns`;
- `distraction_command_stays_local_in_home_and_palette`;
- sabotagem `distraction-global-toggle-does-not-publish`, que remove a
  publicação da política e precisa deixar o gate do produto vermelho;
- `distraction_script_posts_nothing_and_survives_a_poisoned_page`.

Amostrados: `scroll_is_restored_only_when_we_hid_the_cause`,
`the_sticky_threshold_is_25_percent`,
`the_script_yields_after_8_ms_and_stops_after_30_s_idle`,
`what_the_user_just_opened_stays`,
`only_an_empty_backdrop_next_to_what_we_hid_goes`,
`a_dialog_that_only_mentions_a_newsletter_stays`.

Os gates de política continuam correndo o texto que embarca num DOM falso
(`node:vm`), mas a ligação pelo COM deixou de ser inferida por esses testes:
`scripts/test-distraction-e2e.ps1` abre o EXE real em WebView2 contra uma
fixture presa a 127.0.0.1 por `news.distraction.test`. A fixture força uma
segunda navegação para sair da corrida conhecida do primeiro documento e o
próprio DOM informa se um `#cookie-banner` fixo foi realmente escondido. O CI
também sabota `AddScriptToExecuteOnDocumentCreated`, recompila e exige o
veredicto `visible`; depois restaura, recompila e exige verde novamente. O
CI principal repete o E2E sobre `ci-tested/NeuralIA.exe`, exatamente o binário
que `release.yml` empacota.

## Estado do controle global

A Home e a paleta reconhecem localmente:

- `/distracoes` ou `/distracoes status`;
- `/distracoes on` (também `ligar`/`ativar`);
- `/distracoes off` (também `desligar`/`desativar`);
- `distracoes:<ação>` como forma equivalente.

A mudança grava `default_on`, publica a política compartilhada e re-registra
o script. Para fazê-la valer já nas páginas abertas, recarrega somente Split,
Split privado e Web completa; as colunas das IAs nunca são recarregadas por
esse toggle. Escolhas privadas por site continuam somente em memória e podem
sobrepor o padrão salvo durante a sessão.

## Fora desta fase

- A secção visual de Definições «Ocultar distrações (cookies, newsletter,
  barras fixas)». O comando já cobre o controle global sem criar uma segunda
  política.
- A secção visual de Definições continua fora desta fase; o controle global
  já existe pela Home/paleta sem criar uma segunda política.
