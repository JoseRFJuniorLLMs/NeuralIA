# SPEC-0118 — NeuralIA Kokoro: voz neural livre/offline e botão “Ler PDF”

**Status:** PROPOSTA — integração Kokoro não implementada.
**Escopo:** visualizador PDF.js, Reader e EPUB; interface Windows/Rust.
**Motor de navegação:** manter WRY + WebView2; nenhuma migração para CEF/Chrome/Gecko.
**Relacionadas:** SPEC-0102, SPEC-0104, SPEC-0108, SPEC-0110, SPEC-0116 e AGENTS.md.
**Licença:** manter o código principal MIT; aprovação de cada dependência e modelo antes da distribuição.

## 1. Resultado esperado

O PDFView da NeuralIA deve apresentar, de forma visível, um botão **🔊 Ler PDF** no canto superior direito, com acesso ao player de leitura. A primeira entrega pode usar a síntese atual `window.speechSynthesis`; a entrega completa acrescenta a opção **Kokoro (voz neural local)**.

O Kokoro **não substitui o WebView2**. É apenas o mecanismo opcional de síntese de fala, executado fora do JavaScript da página. O visualizador atual já usa `assets/pdfjs/viewer.html`, `viewer.mjs`, `read-aloud.js` e PDF.js para texto, páginas e realce.

### UX obrigatória

- **🔊 Ler PDF**: botão textual visível e acessível no PDFView (não somente ícone sem legenda). Mesmo botão alterna para **■ Parar PDF** quando a leitura está ativa/pausada.
- Clique inicia a leitura a partir da seleção atual ou página visível; se já lê, para. Barra atual de controles: anterior, reproduzir/pausar, parar, próxima, velocidade, voz, fechar.
- Atalho **Ctrl+Shift+U** preservado. Botão e atalho chamam **o mesmo controller** `NeuralIAReadAloud.attachPdf()`, sem segunda implementação ou dois fluxos de áudio.
- Contraste nos temas claro/escuro, foco visível, `aria-label`/`aria-pressed` corretos, compatível com barra de busca Ctrl+F e zoom.
- Documento sem camada de texto deve informar “Não há texto para ler aqui (página digitalizada?)”; não fazer OCR automático.
- **Antes de Kokoro estar implementado**, o botão usa o motor existente e NÃO pode anunciar “voz neural Kokoro” nem tentar baixar modelos.
- **Depois da implementação**, seletor de motor: “Voz do Windows (atual)” e “Kokoro neural offline (instalar/ativar)”; indicar claramente qual está ativo.

## 2. Modelo e vozes

**Candidato-base:** Kokoro-82M em formato ONNX, versão/revisão congelada após validação, vozes pt-BR inicialmente `pf_dora`, `pm_alex` e `pm_santa`. Não presumir que pt-BR já foi aprovado em qualidade: executar avaliação de pronúncia de números, datas, documentos jurídicos, siglas e nomes.

- Modelo e vozes opcionais por download/importação **explícitos**.
- O instalador-base não contém pesos; a NeuralIA inicia e navega mesmo sem pack.
- A síntese deve funcionar **100% offline** depois da instalação, sem depender de uma API do Edge, Azure, de WebSocket externo ou de um serviço local aberto à rede.
- A opção Kokoro só é selecionável quando runtime, fonemizador e voz aprovados estiverem íntegros e instalados.

### Licenças — condição bloqueante

Pesos Kokoro publicados sob Apache-2.0 **não bastam** para declarar a distribuição permissiva. A fonemização de pt-BR frequentemente usa **eSpeak NG (GPL-3.0-or-later)**. Selecionar e auditar o adaptador/fonemizador efetivamente distribuído, suas licenças e obrigações, inclusive dependências transitivas, sem pressupor que subprocesso resolve o problema de licença. Verificar também modelo ONNX convertido, vozes, dados, runtime de inferência e arquivos de aviso. Não redistribuir itens sem parecer/documento de conformidade.

Fontes para congelamento da revisão:
- https://huggingface.co/hexgrad/Kokoro-82M
- https://huggingface.co/litert-community/Kokoro-82M
- https://github.com/k2-fsa/sherpa-onnx
- https://github.com/espeak-ng/espeak-ng

## 3. Arquitetura normativa

`NeuralIA.exe` (Rust/Win32)
  → PDF.js/Reader/EPUB extrai os blocos autorizados
  → controller único valida origem, sessão e gesto do usuário
  → `LocalTtsService`/worker isolado, iniciado sob demanda
  → normalizador + fonemizador pt-BR (licenças aprovadas)
  → Kokoro/ONNX em CPU com fallback determinístico
  → PCM para saída de áudio local, filas limitadas
  → eventos de progresso/destaque → página confiável

- Manter `wry = 0.57` e `webview2-com` em toda a implementação desta SPEC.
- `neural-core` deve definir tipos do serviço, estados, comandos e erros; `neural-app` deve possuir inicialização lazy, áudio, threads e ciclo de vida.
- O worker não pode acessar cookies, navegador, agente, sessão das IAs, chaves, memória semântica, cliente HTTP ou URLs.
- Integrar ao contrato de IPC da SPEC-0108: origem `neuralia-pdf.localhost` e demais superfícies internas explicitamente confiáveis, capability e esquemas fechados; não inventar ações sem atualizar o registro e os testes de contrato. Frame remoto não aciona TTS.
- Preservar o controle e os realces existentes; se Kokoro não entregar marcação palavra a palavra verificável, fazer realce **por frase/bloco reproduzido**, sem alegar precisão inexistente.
- O PDF.js deve fornecer somente trechos bounded, nunca todo o PDF em memória para o worker.

### Estados

`Unavailable` (pack não instalado); `Ready`; `Loading`; `Playing`; `Paused`; `Stopping`; `Error`.
Ação nova incrementa `generation` monotônica; eventos, áudio e progressos de gerações anteriores devem ser descartados após `stop`, troca de página, navegação, fechamento e nova leitura. Uma sessão ativa por vez; não misturar vozes.

### Contrato lógico (ilustrativo; não implementado)

`TtsCommand::Start {source_id, text_block_id, voice_id, rate, generation}`;
`TtsCommand::Pause {generation}`;
`TtsCommand::Resume {generation}`;
`TtsCommand::Stop {generation}`.
`TtsProgress {generation, page, block_id, state, played_ms}`.

A origem do texto deve ser validada no nativo; não aceitar payload arbitrário de site remoto. Filas com limites explícitos de bytes e duração, com backpressure e cancelamento real, não apenas esconder a UI.

## 4. Instalação de pack e orçamento

Integrar um gestor de **pacotes de voz separado** ou uma extensão devidamente isolada da SPEC-0102; o gestor de modelos genérico possui limites próprios e não deve sofrer aumento global silencioso. Pack contém manifesto versionado, ID, origem, revisão, lista de pesos/vozes/fonemizador, comprimentos, hashes SHA-256, licenças e avisos.

- Download só mediante consentimento, em URL aprovada e sob limite de bytes; staging, verificação SHA-256 e ativação atômica.
- Desinstalação voluntária; reversão para `speechSynthesis` atual caso o pack falhe, seja removido ou não carregue.
- Zero modelos residentes ao abrir a Home, sem worker de TTS iniciado e sem tráfego de rede.
- Descarregar modelos após o período de inatividade medido, sem interferir em abas ou WebView2.
- No modo privado, síntese local é permitida; não persistir texto falado ou áudio sintetizado.
- Testar CPU Windows sem GPU dedicada; GPU opcional apenas mediante medição.

## 5. Metas de desempenho (a validar, não resultados)

Ambiente mínimo de comparação: Windows 11 x64, 4 núcleos CPU e 8 GiB RAM. Registrar modelo, runtime, vozes, hardware e p50/p95.

| Critério | Meta de aceitação |
| --- | --- |
| Home sem pedido de voz | 0 bytes de pesos residentes e nenhum worker Kokoro |
| Primeira fala, modelo já aquecido | p95 até 2 s |
| Síntese offline | ritmo pelo menos tempo real na CPU de referência (RTF até 1,0) |
| Parar até silêncio | p95 até 200 ms |
| Memória incremental do pack ativo | até 768 MiB no cenário de referência, com pico registrado |
| Fila de áudio | tamanho/duração finitos; sem crescimento ilimitado |
| Qualidade pt-BR | avaliação cega em texto jurídico, números, siglas, nomes e frases longas |

Metas que se mostrarem irrealistas devem ser revistas antes do aceite, nunca declaradas como medidas sem testes.

## 6. Testes de aceitação obrigatórios

1. **Botão PDF real:** a interface embarcada apresenta “Ler PDF” (não somente tooltip), usa o mesmo controller do atalho, muda para “Parar PDF” durante a leitura e cancela a sessão sem áudio órfão.
2. **Fallback sem modelo:** leitor do Windows ainda inicia/pausa/para/realça, sem qualquer download ou tela quebrada.
3. **Gestos e IPC:** não aceitar TTS vindo de frame/URL de site não confiável, capability inválida ou payload acima de 8 KiB.
4. **Pack inválido:** recusar manifestos com path traversal, hash errado, licença ausente, arquivo além do teto e modelo incompleto.
5. **Privacidade:** síntese Kokoro testada com rede indisponível; nenhum texto, áudio, URL ou credencial trafega; modo privado não cria cache persistente.
6. **Cancelamento:** `generation` antiga não pode reproduzir áudio ou atualizar highlight após `stop`, fechar, navegar ou iniciar outra leitura.
7. **Qualidade PDF/Reader/EPUB:** áudio real pt-BR, seleção/página inicial corretas, sem texto continua com diagnóstico; realce reflete áudio efetivamente tocado.
8. **Performance:** medir TTFS, fator de tempo real, pico RAM, CPU, efeito no arranque/Home, sem regressões nos gates do navegador.
9. **Licença/SBOM:** lista completa de runtime, pesos, vozes, fonemizador e componentes; redistribuição só após aprovação.
10. **Sabotagem:** forçar falhas dos gates críticos no caminho embarcado e provar vermelho; restaurar byte a byte e reexecutar verde, conforme `AGENTS.md`.

## 7. Plano de entrega

- **Fase 0 — UX:** tornar visível o botão “Ler PDF” do player existente, mantendo o motor `speechSynthesis` atual; testar botão, estado, atalho e fallback. **Isso não é implementação Kokoro.**
- **Fase 1 — protótipo offline:** voz pt-BR Kokoro audível no Windows x64/CPU, revisão fixada, métricas e licenças verificadas.
- **Fase 2 — runtime:** worker Rust lazy, PCM com filas limitadas, `generation`/cancelamento, instalador de pack, sem dependência de WebView2 para síntese.
- **Fase 3 — integração:** seletor de motor e vozes no player do PDF e nos leitores Reader/EPUB, progresso e realce.
- **Fase 4 — hardening:** gates reais, sabotagem dos críticos, testes offline, SBOM e revisão de segurança/licenças.

**Critério final:** somente declarar “Kokoro integrado” quando houver voz real pt-BR via worker embarcado e testes Windows completos. A SPEC não autoriza bump, tag, release ou merge automático.