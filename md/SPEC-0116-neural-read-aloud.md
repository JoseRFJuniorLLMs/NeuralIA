# SPEC-0116 — Leitura em Voz Alta Neural (Protocolo Edge TTS & Pipeline de Áudio Nativo)

**Status:** Proposed

**Alvo:** NeuralIA 2.1–2.2

**Depende de:** SPEC-0104 (Agent Security), SPEC-0108 (Canal Seguro IPC)

**Relacionada com:** SPEC-0100 (Memória Semântica & Privacidade), SPEC-0102 (Inteligência Local), SPEC-0103 (Linha Temporal Semântica)

## 1. Propósito

O NeuralIA necessita de uma funcionalidade de **Leitura em Voz Alta (*Read Aloud*)** para documentos no visualizador de PDF (`neuralia-pdf.localhost`) e no Modo Leitura (Reader) com a mesma qualidade de entonação humana encontrada no Microsoft Edge.

Esta especificação define o consumo do protocolo WebSocket de síntese neural do Azure/Edge diretamente a partir do runtime nativo em Rust, desacoplado da interface visual, reproduzido via pipeline assíncrono de áudio (`rodio`) e sincronizado palavra a palavra com a página através do canal seguro IPC (SPEC-0108).

## 2. Não-objetivos

Esta especificação explicitamente **não** contempla:

* Embutir o binário fechado do navegador Microsoft Edge ou depender do executável `msedge.exe`.
* Carregar modelos locais de TTS com múltiplos gigabytes no instalador base (mantendo o princípio de **0 bytes em Home/idle** da SPEC-0102).
* Enviar o ficheiro PDF completo ou metadados de sessão privada para servidores externos.
* Conceder privilégios de execução de scripts à thread de áudio.
* Implementar ferramentas de clonagem de voz ou gravação para disco sem consentimento explícito do utilizador.

## 3. Arquitetura do Sistema

**Plaintext**

```
[ Documento PDF / Reader ]
        │  (Clique em "Ler em voz alta")
        ▼
[ JavaScript da Página ] ──► Segmentação de parágrafo (< 8 KiB)
        │
        ▼  (SPEC-0108 postMessage com token de capability)
[ neural-app/src/ipc.rs ]
        │
        ▼  (Disparo em background Tokio task)
[ TtsAudioService (Rust) ]
        │
        ├──► [ WebSocket Edge TTS Client ] ──► wss://speech.platform.bing.com/...
        │           │
        │           ├── Stream de Áudio (MP3/Opus) ──► [ rodio::Sink ] ──► Altifalantes
        │           │
        │           └── Metadados WordBoundary
        │                     │
        │                     ▼  (Eval leve no WebView2)
        └──────────────► Destaque visual sincronizado (CSS highlight)
```

## 4. Protocolo Edge TTS (Nuvem / WebSocket)

A comunicação com o endpoint de síntese neural do Edge dispensa chaves pagas de API e opera através de um túnel WebSocket bidirecional autenticado por cabeçalhos de cliente fidedigno (*Trusted Client Token*).

### 4.1 Endpoint e Conexão

* **URL:**`wss://[speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1](https://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1)`
* **Parâmetros de Handshake:**

  * `TrustedClientToken=6A5AA1D4EAFF4E9FB37E23D68491D6F4`
  * `ConnectionId`: UUIDv4 aleatório gerado a cada sessão de leitura.
* **Cabeçalhos Exigidos:**

  * `Origin: chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold`
  * `User-Agent`: String do Microsoft Edge Chromium recente.

### 4.2 Formato das Mensagens de Controlo e Áudio

O protocolo transmite mensagens de texto e dados binários em envelopes delimitados por cabeçalhos internos (`Path`, `X-RequestId`, `Content-Type`):

1. **Configuração de Saída (`Path: speech.config`):** Define a taxa de amostragem e codec:

   **Plaintext**

   ```
   Content-Type: application/json; charset=utf-8
   Path: speech.config

   {"context":{"synthesis":{"audio":{"outputFormat":"audio-24khz-48kbitrate-mono-mp3"}}}}
   ```
2. **Payload SSML (`Path: ssml`):** Envio do parágrafo a ser sintetizado:

   **XML**

   ```
   <speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='pt-BR'>
     <voice name='pt-BR-FranciscaNeural'>
       <prosody pitch='+0Hz' rate='+0%'>
         Texto a ser lido em voz alta.
       </prosody>
     </voice>
   </speak>
   ```
3. **Receção de Metadados (`Path: audio.metadata`):** Pacotes JSON contendo marcações temporais para destaque no ecrã:

   **JSON**

   ```
   {
     "Type": "WordBoundary",
     "Offset": 1250000,
     "Duration": 350000,
     "text": { "Text": "Texto", "Length": 5, "BoundaryType": "WordBoundary" }
   }
   ```
4. **Receção de Chunks Binários (`Path: audio`):** Cada frame binário possui um cabeçalho de 2 bytes indicando o tamanho do header textual, seguido dos bytes brutos MP3.

## 5. Pipeline de Áudio em Rust (`neural-app` / `rodio`)

A reprodução de áudio é tratada como uma operação assíncrona desacoplada da thread de renderização da UI:

1. **Gestor de Áudio (`AudioPlaybackEngine`):**

   * Mantém um `rodio::OutputStream` aberto e um `rodio::Sink` isolado para o canal de TTS.
   * Suporta controlo de reprodução em tempo real: `play()`, `pause()`, `resume()` e `stop()`.
2. **Streaming e Buffer Sem Fricção:**

   * O cliente WebSocket emite blocos binários para um canal `tokio::sync::mpsc`.
   * Um adaptador assíncrono converte o stream de chunks MP3 num leitor contínuo (`std::io::Read`), alimentando o `rodio::Decoder::new_mp3(...)`.
3. **Interrupção Instantânea (*Fail-Closed Drain*):**

   * Ao fechar a aba, teclar `Esc` ou disparar navegação, o Rust invoca imediatamente `sink.stop()`, limpa os buffers em memória e cancela a task do WebSocket, cortando o som em menos de **20 ms**.

## 6. Integração com o Canal Seguro IPC (SPEC-0108)

O suporte à leitura em voz alta adiciona comandos estritos ao handler IPC (`neural-app/src/ipc.rs`):

### 6.1 Extensão do Vocabulário de Ações

A lista de 25 comandos da SPEC-0108 §3.2 é expandida com as ações seguras:

* `tts-read`: Solicita a síntese de um bloco de texto.
* `tts-control`: Envia sinais de navegação (`pause`, `resume`, `stop`, `seek`).

### 6.2 Estrutura do Pacote IPC

Em conformidade com a SPEC-0108, o pacote JSON não pode ultrapassar **8 KiB**:

**JSON**

```
{
  "v": 1,
  "cap": "<token_32_hex>",
  "action": "tts-read",
  "args": {
    "block_id": "p_104",
    "voice": "pt-BR-FranciscaNeural",
    "rate": "+0%",
    "text": "Conteúdo do parágrafo extraído pelo PDF.js..."
  }
}
```

* **Regra de Fatiamento no Cliente:** O script injetado no PDF (`neuralia-pdf.localhost`) deve segmentar blocos longos em múltiplos pedidos sequenciais caso o texto exceda o orçamento de caracteres do IPC.
* **Sincronização Visual:** Conforme o Rust consome os eventos `WordBoundary`, emite via runtime do WebView2 a chamada nativa:

  **JavaScript**

  ```
  window.__neuralia_highlight_word("p_104", 5); // ID do parágrafo e índice da palavra
  ```

## 7. Fronteira de Segurança e Privacidade (SPEC-0100 & SPEC-0104)

1. **Classificação de Autoridade (SPEC-0104):**

   * A extração e leitura de texto de documentos já abertos classifica-se como **Classe A (Read-Only)**.
   * Não altera estado, não submete formulários e não adquire credenciais.
2. **Defesa do Modo Privado / Incógnito (SPEC-0100):**

   * O protocolo Edge TTS envia fragmentos textuais do documento para os servidores da Microsoft.
   * **Invariante Normativo:** Em janelas ou separadores privados, a leitura via WebSocket remoto é **bloqueada por omissão**.
   * O utilizador deve receber um aviso nativo de *Data Egress* para permitir a síntese na nuvem, ou o sistema deve alternar automaticamente para a API offline do sistema operativo (Windows OneCore via WinRT) sem transmissão de rede.
3. **Prevenção de Injeção de Dados:**

   * Textos extraídos do PDF enviados para síntese passam pela sanitização básica da SPEC-0104 para evitar a inserção de comandos SSML maliciosos (escape de tags `<speak>`, `<voice>` e scripts XML).

## 8. Orçamentos de Desempenho e Recursos


| **Métrica**                                  | **Orçamento**              | **Verificação**                               |
| --------------------------------------------- | --------------------------- | ----------------------------------------------- |
| **Latência até o Primeiro Som (*TTFS*)**    | **\$\\le 400\\text{ ms}\$** | Conexão banda larga, buffer inicial de 1 chunk |
| **Custo de CPU em Reprodução**              | **\$< 3\\%\$**em 4 núcleos | Decodificação MP3 e render via`rodio`         |
| **Custo de Memória em Idle**                 | **0 bytes**de buffers       | Canal fechado quando não há áudio tocando    |
| **Latência de Interrupção (`Stop`/`Esc`)** | **\$< 20\\text{ ms}\$**     | `Sink::stop()`síncrono e drop imediato de task |
| **Impacto na Thread de Interface**            | **\$0\\text{ ms}\$**        | Totalmente isolado no worker assíncrono Tokio  |

## 9. Contrato de API no `neural-core`

**Rust**

```
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordBoundary {
    pub offset_ms: u64,
    pub duration_ms: u64,
    pub text: String,
    pub word_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    pub voice: String,
    pub pitch: String,
    pub rate: String,
}

pub enum AudioStreamEvent {
    AudioChunk(Vec<u8>),
    Boundary(WordBoundary),
    Complete,
    Error(String),
}

#[async_trait]
pub trait TtsEngine: Send + Sync {
    async fn synthesize_stream(
        &self,
        text: String,
        config: TtsConfig,
    ) -> Result<mpsc::Receiver<AudioStreamEvent>, TtsError>;
  
    fn is_offline(&self) -> bool;
}

pub trait AudioOutputDevice: Send + Sync {
    fn play_chunk(&self, chunk: &[u8]) -> Result<(), AudioError>;
    fn pause(&self) -> Result<(), AudioError>;
    fn resume(&self) -> Result<(), AudioError>;
    fn stop(&self) -> Result<(), AudioError>;
}
```

## 10. Critérios de Aceitação e Testes de Sabotagem (CI Gates)

O pull request que introduzir esta funcionalidade deve comprovar:

1. **Gate do Parser SSML:** Verificação de que caracteres especiais (`&`, `<`, `>`, `"`) são devidamente escapados antes da geração do payload XML.
2. **Gate do Envelope IPC (SPEC-0108):** O handler recusa payloads de `tts-read`\$> 8.192\\text{ bytes}\$ ou com tokens inválidos.
3. **Teste de Sabotagem — Fuga em Modo Privado:** Um teste automatizado deve falhar a compilação/teste se uma chamada a `synthesize_stream` com o motor Edge TTS for despachada a partir de uma sessão privada sem o consentimento de *Data Egress*.
4. **Teste de Interrupção com Sucesso:** A invocação de `tts-control` com argumento `stop` deve encerrar a stream do WebSocket e garantir que o canal do `rodio::Sink` fica vazio e silenciado no intervalo de 20 ms.
5. **Teste de Concorrência:** Disparar nova leitura enquanto outra está em curso cancela a primeira de forma determinística, sem sobreposição de vozes (*race condition* de áudio).

## 11. Fases de Implementação

* **Fase 1: Cliente Edge TTS e Parser SSML (`neural-core`)**

  * Implementação da camada de comunicação WebSocket usando `tokio-tungstenite`.
  * Extração e separação dos cabeçalhos binários e eventos JSON `WordBoundary`.
* **Fase 2: Motor de Áudio Nativo (`neural-app`)**

  * Integração da crate `rodio` em thread dedicada.
  * Pipeline de buffering assíncrono para reprodução contínua.
* **Fase 3: Extensão IPC e Integração com PDF.js**

  * Criação dos handlers `tts-read` e `tts-control` em `neural-app/src/ipc.rs`.
  * Criação da barra flutuante de leitura (*Play*, *Pause*, *Stop*, Seletor de Velocidade) na interface do `neuralia-pdf.localhost`.
* **Fase 4: Sincronização de Destaque e Proteções de Privacidade**

  * Highlight visual em tempo real baseado em spans.
  * Bloqueio fail-closed em separadores de navegação privada.
