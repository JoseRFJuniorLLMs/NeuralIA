# SPEC-0119 — PDF clicável no Split e tela cheia + leitura neural Kokoro 100% offline

**Status:** PROPOSTA — especificação, não implementação.  
**Escopo:** NeuralIA Windows, Rust/WRY/WebView2, PDF.js, interação nos painéis de IA, barra de leitura e síntese local.  
**Complementa:** SPEC-0118 (PR #254, botão "Ler PDF" e projeto do Kokoro), SPEC-0110, SPEC-0116, SPEC-0108, SPEC-0102 e AGENTS.md.  
**Não autoriza release nem bump de versão.** Implementação deve passar pelos gates/revisões do AGENTS.md.

## 1. Defeitos reproduzidos pelo usuário (não confundir com o comportamento desejado)

1. Ao clicar normalmente num link PDF exibido por uma IA, o PDF é encaminhado ao painel lateral, mas não entra corretamente no visualizador PDF.js integrado; não mostra os controles de leitura.
2. Ao manter **Ctrl** e clicar no link PDF, o documento ocupa toda a tela, mas também não oferece o leitor de voz esperado.
3. Não aparece botão claro **"🔊 Ler PDF"** em todos os modos. A lógica de leitura existente depende de `window.speechSynthesis` do WebView2, não de um motor Kokoro realmente offline.
4. Links PDF, navegação/popup, URLs sem extensão, redirecionamentos e PDFs privados podem seguir caminhos distintos, com UI e recursos inconsistentes.

**Contrato de UX (preservar exatamente):**

| Gesto | Resultado exigido |
|---|---|
| Clique normal em PDF numa coluna de IA | Abrir no **Split/painel de documento lateral**, à direita da IA, com PDF.js e "Ler PDF". Não substituir a coluna de resposta da IA. |
| Ctrl + clique em PDF numa coluna de IA | Abrir o **mesmo PDF.js em vista ampla/tela cheia dentro do NeuralIA**, com os mesmos controles, sem depender do plugin PDF do Edge. |
| Botão de ampliar/restaurar PDF | Alternar Split ↔ vista ampla **preservando documento, página, zoom, busca, seleção, voz e estado de leitura**, quando possível sem reconstrução da WebView. |
| Clicar em "Ler PDF" em qualquer modo | Reproduzir **voz neural Kokoro offline** após instalar o pack, ou indicar de forma honesta que o pack está ausente. Nunca iniciar síntese remota silenciosamente. |
| Ctrl+Shift+U | Acionar o mesmo controlador de leitura, tanto no Split quanto na vista ampla. |

O Ctrl+clique não é um bug a remover: **o bug é o fluxo e o leitor inconsistentes**.

## 2. Diagnóstico confirmado na `main` inspecionada

- `crates/neural-app/src/windows_app/app/pages.rs`: `read_pdf()`/ `open_pdf()` criam `WebViewHost::Pdf` após `destroy_web_surfaces()`; o modelo atual de `Surface::Pdf` destrói o comparador, portanto **não serve para exibir PDF ao lado da IA**.
- `crates/neural-app/src/windows_app/app/split.rs`: `open_split_opened_by()` cria `WebViewHost::Split` com `split_webview_builder().with_url(valid)`, isto é, carrega a URL PDF numa WebView web genérica, **não no builder `pdf_webview_builder()`**.
- `crates/neural-core/src/intent.rs`: `is_pdf_url()` considera apenas o sufixo `.pdf` no path; PDF sem extensão não é identificado.
- `assets/pdfjs/viewer.html` já inclui `read-aloud.js`, e `assets/pdfjs/viewer.mjs` chama `NeuralIAReadAloud.attachPdf()` depois do carregamento. A interface só surge quando se chega ao visualizador próprio, o documento carrega e o script consegue criar os controles.
- `assets/pdfjs/read-aloud.js` já cria toggle e barra via DOM e usa `speechSynthesis`; a SPEC-0118 propõe o rótulo visível **Ler PDF**, mas não instala o Kokoro.
- Uma proposta anterior, PR #254, aborda UI e arquitetura Kokoro, **não corrige por si só o roteamento do PDF no Split**.

Esses pontos explicam uma divergência arquitetural provável; o teste E2E Windows ainda deve confirmar a causa final do comportamento reportado.

## 3. Arquitetura mandatória: um PDF document controller, duas apresentações

```text
Clique real no link PDF dentro de IA / WebView2
   └─ NativePdfNavigationRouter
      ├─ click normal  → PdfPresentation::DockedSplit
      └─ Ctrl+click    → PdfPresentation::Expanded
          └─ PdfDocumentSession { document_id, source_url, private, generation }
             └─ PdfJsWebView (WebView2 + PDF.js local)
                 ├─ UI: Ler PDF, Play/Pause/Stop, voz, velocidade, página
                 ├─ extração de texto PDF.js limitada por chunks
                 └─ Secure IPC (capability por instância, origem local)
                     └─ LocalTtsService (Rust, on-demand)
                         └─ Kokoro-82M multilingual pt-BR + fonemização aprovada
                             └─ áudio PCM local → saída Windows
```

### 3.1 Roteamento e identificação

- Introduzir decisão testável pura, p.ex. `route_pdf_link(source, url, modifiers, response_metadata, privacy)`, separada do efeito de UI e sem acoplá-la ao `App`.
- Tratar cliques reais de `<a>`, `target=_blank`, `window.open`, `NewWindowRequested`, `NavigationStarting`, download e redirects sem navegação duplicada; não supor que `NewWindowRequested` sozinho preserva Ctrl. Capturar modificador **no gesto real**, com protocolo mínimo confiável e sem conceder autoridade a JS remoto.
- Resolver PDFs por URL `.pdf` e, após navegação permitida, por `Content-Type: application/pdf`/assinatura validada, **com limite de sniffing**. Evitar assumir que `Content-Disposition: attachment` autoriza abrir; respeitar intenção explícita de download.
- Validação de URL, redirects e bloqueio de destinos não autorizados continuam sob política atual; PDF não pode virar bypass de SSRF, local/private network, data/blob/file nem de download.
- PDF com sessão autenticada: manter a política de perfis/cookies aplicável, sem copiar cookies ou Authorization ao JavaScript ou à URL; documentar limitações e recusar com erro claro quando não houver download seguro.
- Uma operação por evento/geração; evitar dupla abertura, janela fantasma e fallback acidental que troca Split por `Surface::Pdf`.

### 3.2 PDF.js embutido em painel e em vista ampla

- Refatorar o builder PDF.js para ser reutilizável como **child WebView** no Split; manter `WebView2`, `WRY`, PDF.js e protocol handler `neuralia-pdf`. Não incorporar Chromium/CEF/Electron nem usar o leitor PDF proprietário do Edge.
- `SplitView` deve suportar `SplitContent::Pdf(PdfDocumentSession)` além de fonte Web. A vista ampla deve mudar geometria/apresentação do **mesmo documento** sempre que possível; fullscreen deve permitir retorno/fechamento.
- Desacoplar a origem dos bytes: atualmente `self.pdf_bytes: Arc<Mutex<Vec<u8>>>` é um slot compartilhado; para múltiplos documentos/abas, cada viewer deve usar um identificador e buffer/serviço de ranges **por documento** com isolamento e liberação no fechamento.
- Preservar foco/abas/posição e contexto da IA de origem. Não invocar `destroy_web_surfaces()` ao abrir PDF no Split.
- Garantir páginas, zoom, seleção, busca, barra de progresso, atalho de leitura e controles de acessibilidade no Split e na vista ampla.
- O conteúdo remoto do PDF nunca recebe a capability; apenas HTML/JS local de origem segura do viewer. Texto e bytes ficam fora da página da IA.

## 4. UI: botão "Ler PDF" obrigatório nos dois modos

- Mostrar botão **🔊 Ler PDF** imediatamente na estrutura do viewer, mesmo durante carregamento (desabilitado/estado "Carregando PDF..." até ficar pronto), com rótulo visível, foco/teclado, tooltip e `aria-label`.
- A barra abre controles: **Ler/Pausar/Continuar/Parar**, frase anterior/próxima, velocidade (0,75× a 2×), página, progresso e seletor de voz.
- Não mascarar falha de `read-aloud.js`, `viewer.mjs` ou `attachPdf()`: renderizar erro local não intrusivo e botão com estado explicativo, nunca simplesmente desaparecer.
- Botão e `Ctrl+Shift+U` usam um controlador compartilhado, sem duas sessões simultâneas.
- Destaque da frase em leitura; fallback de progresso por bloco se não houver boundaries palavra-a-palavra precisos.
- Se PDF for digitalização sem texto, avisar **"Este PDF não contém texto selecionável; OCR não está instalado"**; não alegar leitura do conteúdo da imagem.
- Incluir em testes escalas 100/125/150/200%, modo claro/escuro e largura estreita do Split.

## 5. Kokoro neural **100% OFFLINE** — não é Edge TTS

**Motor obrigatório:** Kokoro-82M, variante multilingue com português brasileiro e vozes `pf_dora`, `pm_alex`, `pm_santa` quando verificadas no modelo fixado; permitir ampliação de catálogo depois de validação.

- Executar inferência **local** no Windows por worker Rust que usa backend nativo aprovado (preferência inicial a `sherpa-onnx` + Kokoro ONNX multilingue; avaliar alternativa self-contained de front-end sem eSpeak NG). `WebView2` permanece só para exibir PDF e controles.
- **Sem Azure, Edge TTS WebSocket, `speech.platform.bing.com`, API, login, telemetria, servidor HTTP acessível à rede ou serviço remoto.** A SPEC-0116 descreve alternativa cloud; não deve ser fallback da modalidade offline.
- Download/importação de **pack opcional mediante ação explícita**: modelo, vozes, vocabulário/tokens e fonemizador necessários; download fora da sessão de leitura, manifest versionado, URL de origem fixa, hashes SHA-256, limites, validação de archive e instalação atômica. Navegador inicia normalmente sem pack e sem carregar inferência em Home.
- **Licenças são gate bloqueante**: modelo/pesos/vozes, conversão ONNX, runtime e fonemizador. Algumas rotas eSpeak NG são GPL-3.0; registrar as obrigações ou escolher stack permissiva verificada antes de redistribuir. Não confundir licença Apache-2.0 dos pesos com licença do pipeline completo.
- Pack instalado sintetiza sem conexão de rede; permitir `Kokoro (offline)` como padrão depois de ativado. Se indisponível, mostrar `Instalar voz offline` ou opção offline local do Windows **somente mediante escolha explícita**, sem seleção silenciosa de voz online.
- O backend recebe texto normalizado e identificador efêmero, nunca URL do PDF, cookies, histórico, chave da IA ou arquivo integral.
- Normalização pt-BR: pontuação, siglas, numerais, moedas, datas, abreviações, hifenização e nomes jurídicos; não alterar o texto original do documento e testar pronúncia.
- Worker iniciado **lazy**; threads e fila com limites; PCM transmitido ao dispositivo de áudio Windows; controle play, pause, resume, stop e troca de voz/taxa. Mudança de navegação/aba ou fechamento invalida `generation` e cancela o áudio antigo, sem vazamento.
- Exigir medidas reais em Windows CPU-only: tempo até primeira fala, razão duração de geração/duração do áudio, memória idle e pico, CPU, comportamento de PDF 500 páginas; **não inventar metas aprovadas sem baseline**.

## 6. Fronteiras de segurança/privacidade

1. Origem `neuralia-pdf.localhost` e capability por WebView e sessão; IPC schema fechado, payload até 8 KiB, texto particionado e tamanho total de fila limitado.
2. Sem mensagem `tts-read` de páginas remotas/frames não autorizados; toda solicitação passa pelo roteador nativo, vinculada ao viewer e a gesto/estado válido.
3. Modo privado: leitura offline permitida sem criar arquivos, logs textuais, histórico, memória semântica ou cache persistente do texto/áudio; fechamento zera referências e buffers sensíveis na medida viável.
4. Nenhum texto enviado ao Edge/Azure/Bing ou à rede ao ler. Um teste de rede deve provar **zero solicitações de síntese** com o cabo desligado/rede bloqueada.
5. Impor limites de PDF, páginas, ranges, texto e tempo; PDFs malformados e redirects não podem bloquear a thread UI ou criar loops.
6. Atualizar `AGENTS.md` somente por autorização do dono; respeitar revisão, testes de sabotagem e áreas sensíveis.

## 7. Plano de implementação

**Fase A — testes de reprodução (antes da correção):** fixture de página IA com link PDF, `target=_blank`, query URL, headers PDF e PDF sem extensão; instrumentar eventos nativos e provar o desvio para generic Split.

**Fase B — roteamento:** criar decisão `PdfPresentation`, conectar eventos WebView2 e impedir duplo despacho. Ctrl+clique → vista ampla; clique normal → Split com PDF nativo.

**Fase C — viewer por documento:** reutilizar PDF.js child WebView, estado isolado por sessão e bytes/ranges, minimizar/restaurar/ampliar sem destruir a IA.

**Fase D — UI de leitura:** integrar o botão da SPEC-0118/PR #254 em **todas** as apresentações PDF, com diagnóstico de carregamento/erro e `Ctrl+Shift+U`.

**Fase E — engine Kokoro offline:** manifest/packs, fonemização auditada, integração ONNX Rust, áudio, controles e realce; UX de instalação e escolha, nunca cloud implícita.

**Fase F — testes e release:** CI Windows E2E WebView2 real e testes offline; aprovação jurídica, revisão independente e eventual release só com autorização do dono.

## 8. Critérios de aceitação — bloqueantes

| ID | Gate comportamental verificável |
|---|---|
| A01 | Clique simples num link PDF dentro da IA mantém a IA visível e abre PDF.js no Split; página renderiza, texto selecionável e **Ler PDF** visível. |
| A02 | Ctrl+clique real no mesmo link exibe **PDF.js em vista ampla**, sem plugin de Edge; mantém botão **Ler PDF**. |
| A03 | Maximizar/restaurar Split preserva URL, página, zoom, controller de leitura, estado do comparador e ausência de nova aba fantasma. |
| A04 | PDF `.pdf?token=...`, sem extensão com `application/pdf`, redirect permitido, `target=_blank` e `window.open` são roteados uma única vez. |
| A05 | PDF não PDF, bloqueado, malformado ou offline exibe erro honesto; não destrói a IA nem executa conteúdo não autorizado. |
| A06 | Botão sempre visível; `Ctrl+Shift+U` e botão ativam mesma sessão; Pause/Resume/Stop funcionam no Split e fullscreen. |
| A07 | Pack Kokoro pt-BR instalado e rede indisponível: lê PDF com áudio local audível; vozes `pf_dora`, `pm_alex`, `pm_santa` verificadas. |
| A08 | Sem pack, mensagem de instalação/fallback offline explícita; nenhuma requisição TTS à nuvem em qualquer estado. |
| A09 | Em modo privado: sem egress, histórico, arquivo de áudio ou gravação de texto; dados efêmeros isolados. |
| A10 | Ao fechar/navegar/iniciar nova leitura, som anterior para e progresso de `generation` antiga é descartado; nenhuma sobreposição de voz. |
| A11 | Dois PDFs em abas independentes não partilham bytes, URL, posição, texto nem sessão de áudio; nenhum PDF anterior substitui o novo. |
| A12 | PDF 500 páginas, zoom, Ctrl+F, seleção, temas e DPI preservam uso responsivo sem memória crescer por canvases não liberados. |
| A13 | E2E Windows `WebView2` testa o binário real; testes unitários não substituem A01/A02/A07. |
| A14 | Prova de sabotagem: desviar `PdfPresentation::DockedSplit` para `Web`, remover `attachPdf` e permitir chamada de rede devem tornar gates específicos vermelhos; restaurar antes de commit. |

**Condição de concluído:** aprovar somente quando os testes no caminho embarcado provarem **dois modos de PDF com controles idênticos e voz Kokoro offline em ambos**; mudança documental ou botão JS usando a voz atual não satisfaz A07.

## 9. Arquivos-alvo (orientativos)

- `crates/neural-app/src/windows_app/app/split.rs` — `SplitView`, child viewer PDF, apresentação.
- `crates/neural-app/src/windows_app/app/pages.rs` — construir viewer PDF reutilizável, sessões e download.
- `crates/neural-app/src/windows_app/webview_hooks.rs` — interceptação segura de link/popup.
- `crates/neural-app/src/windows_app/app/event_loop.rs` — eventos roteados uma vez.
- `crates/neural-core/src/intent.rs` — classificação sem confundir `PDF` com `Web`.
- `assets/pdfjs/viewer.html`, `viewer.mjs`, `read-aloud.js` — UI e controller únicos.
- `crates/neural-app/src/ipc.rs`, módulo Rust novo `local_tts`, `neural-core::speech` — contratos, worker, segurança.
- Tests Windows WebView2/fixtures, gate de rede e de pack, e manifests de modelo/licenças.

## 10. Referências técnicas

- NeuralIA SPEC-0118 em PR #254: https://github.com/JoseRFJuniorLLMs/NeuralIA/pull/254
- Kokoro-82M oficial: https://huggingface.co/hexgrad/Kokoro-82M
- Kokoro multilingue integrado: https://huggingface.co/litert-community/Kokoro-82M
- sherpa-onnx Rust API: https://github.com/k2-fsa/sherpa-onnx
- sherpa-onnx API Kokoro offline: https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/c-api/docs/tts.dox

**Nota de implementação:** a variante LiteRT citada relata front-ends auto-contidos sem eSpeak NG, mas sua compatibilidade com Windows/Rust precisa ser demonstrada antes de selecioná-la como runtime. A implementação final deve fixar versões, checksums e licenças de cada componente.
