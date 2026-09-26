# SPEC-0114 — Anti-distração (avisos de cookies, newsletter e barras fixas)

**Status:** Proposta. Código e gates na branch `feat/anti-distracao` (plano
2.4, item anti-distracao, cortável); passa a «Implementada» só com os gates
verdes no CI do PR e o sim do dono no §7 do `AGENTS.md` (script injetado
novo).

## Objetivo

Esconder, nas páginas da internet, avisos de cookies, janelas de newsletter e
barras fixas grandes. Nos CMPs conhecidos, recusar em vez de só esconder —
e **nunca aceitar**.

## O que o código faz

- `neural_core::distraction` (puro, portátil): `CMP_RULES` (OneTrust,
  Cookiebot, Didomi, Usercentrics, Quantcast/InMobi Choice, CookieYes,
  Complianz, iubenda, Osano, Axeptio, Termly, consent.google.com; Sourcepoint
  e TrustArc só escondidos, porque o aviso vive numa moldura de outra origem),
  `REJECT_WORDS` e `ACCEPT_WORDS` (pt, en, es, fr, de, it),
  `PAYWALL_MARKERS`, `NEWSLETTER_MARKERS`, a política `DistractionPolicy`
  e a decisão `distraction_config(url, política, superfície)`.
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
- Um clique só acontece num CMP conhecido quando o seletor de recusa casa **e**
  o texto visível do botão tem uma frase de `REJECT_WORDS` **e** não tem
  nenhuma palavra de `ACCEPT_WORDS`. Faltando uma, o aviso só é escondido.
- Um marcador de paywall deixa o que o tem intocado. A rolagem só volta
  quando o script escondeu a causa e o documento rola mesmo.
- O que aparece até 1,5 s depois de um clique ou de uma tecla do utilizador
  (foi ele que o abriu: as definições de cookies do rodapé, uma pesquisa) e
  um diálogo com um campo de senha nunca são escondidos nem clicados.
- Barras fixas ou presas, encostadas ao topo ou ao fundo, com pelo menos
  metade da largura e 25% da altura da janela (e menos de 90%), saem; a que
  contém o foco fica.
- No máximo 8 ms de trabalho de cada vez; pára de observar a página depois de
  30 s sem mudanças.
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
- `paywall_overlays_are_never_touched`;
- `per_site_toggle_off_injects_nothing` (neural-core e o caminho do app);
- `a_toggle_in_private_is_never_written`;
- `distraction_script_posts_nothing_and_survives_a_poisoned_page`.

Amostrados: `scroll_is_restored_only_when_we_hid_the_cause`,
`the_sticky_threshold_is_25_percent`,
`the_script_yields_after_8_ms_and_stops_after_30_s_idle`,
`what_the_user_just_opened_stays`.

Os gates correm o texto que embarca num DOM falso (`node:vm`), não num
WebView2: a ligação pelo COM (`AddScriptToExecuteOnDocumentCreated`, o
recarregar depois de uma escolha) não tem teste que corra o exe.

## Fora desta fase

- A secção de definições «Ocultar distrações (cookies, newsletter, barras
  fixas)» (o `default_on` já é lido e gravado, mas não há controlo que o
  mude) e o comando `/distracoes` da paleta e da Home.
- Um teste E2E no exe (CI) que confirme a ligação pelo COM numa página real.
