# SPEC-0119 — PDF Routing + Kokoro Offline (Leitura Neural 100% Offline)

**Status:** Proposed / Implementation Pending

**Alvo:** NeuralIA 2.3–2.4

**PR:** [#255 — SPEC-0119 PDF Routing + Kokoro Offline](https://github.com/JoseRFJuniorLLMs/NeuralIA/pull/255)

**Depende de:** SPEC-0104 (Agent Security), SPEC-0108 (Secure WebView IPC), SPEC-0110 (PDF Text Reader), SPEC-0116 (Neural Read Aloud)

**Relacionada com:** SPEC-0102 (Local Intelligence / Model Packs), SPEC-0100 (Memória Semântica), SPEC-0117 (Gemini Live Agent Tools)

---

## 1. Propósito

Esta especificação resolve dois problemas distintos mas interdependentes:

1. **PDF Routing** — O roteamento atual de abertura de PDFs no painel lateral (`SidePanel`) e em tela cheia (`Fullscreen`) apresenta falhas: PDFs clicados no histórico, downloads ou links externos não chegam consistentemente ao `neuralia-pdf.localhost`. O botão **"Ler PDF"** (`reader_read_aloud` via SPEC-0117) depende de uma superfície PDF ativa e falha silenciosamente quando o roteamento está quebrado.

2. **Kokoro Offline TTS** — A SPEC-0116 cobre síntese neural via Edge TTS (nuvem). Esta spec adiciona o **motor Kokoro** como backend TTS 100% offline, eliminando a dependência de rede e o envio de texto a servidores externos, tornando a leitura de PDFs privados completamente local.

---

## 2. Não-objetivos

* Substituir o Edge TTS (SPEC-0116) — Kokoro é o backend offline; Edge TTS permanece o padrão online.
* Carregar modelos Kokoro maiores que **~80 MB** no instalador base (princípio 0-bytes idle da SPEC-0102).
* Suportar vozes Kokoro em idiomas além de `pt-BR` e `en-US` nesta fase.
* Implementar clonagem de voz ou fine-tuning local.
* Alterar o pipeline de segurança do IPC (SPEC-0108) além das ações já previstas (`tts-read`, `tts-control`).

---

## 3. Parte A — Correção do PDF Routing

### 3.1 Diagnóstico dos Problemas Atuais

O método `open_pdf` em `pages.rs` é chamado corretamente pelo `event_loop.rs` quando bytes já estão disponíveis, mas falha nos seguintes caminhos:

| Cenário | Comportamento atual | Comportamento esperado |
|---------|--------------------|-----------------------|
| Clique num link `.pdf` numa coluna de IA | Abre no WebView2 nativo (download do sistema) | Interceptado → `neuralia-pdf.localhost` |
| PDF vindo de `downloads.rs` (biblioteca) | Sem rota para o viewer interno | Abrir no painel lateral PDF |
| Botão "Ler PDF" sem superfície PDF ativa | `reader_read_aloud` falha silenciosamente | Notificação `HintTone::Warning` + fallback |
| PDF em fullscreen → minimize → reabrir | Superfície destruída, `open_pdf` não reconstrói | Recriar superfície e restaurar posição de página |

### 3.2 Correção do Roteamento no `event_loop.rs`

O `event_loop.rs` deve interceptar navegações com extensão `.pdf` em **todas as superfícies** (colunas 0/1/2, split, web completa) antes que o WebView2 faça o download nativo:

```rust
// neural-app/src/windows_app/event_loop.rs
fn handle_navigation_starting(url: &str, surface: Surface) -> NavigationDecision {
    if url.ends_with(".pdf") || url.contains(".pdf?") || is_pdf_content_type(&url) {
        // Cancela navegação nativa e roteia para o viewer interno
        NavigationDecision::Cancel {
            redirect: Some(PdfRoute::from_url(url, surface)),
        }
    } else {
        NavigationDecision::Allow
    }
}

pub enum PdfRoute {
    /// PDF remoto: fetch async + open_pdf
    Remote { url: String, target: PdfTarget },
    /// PDF local (arquivo em disco via library.rs)
    Local  { path: PathBuf, target: PdfTarget },
}

pub enum PdfTarget {
    SidePanel,   // painel lateral (comportamento padrão)
    Fullscreen,  // tela cheia (quando coluna em fullscreen)
}
```

### 3.3 Botão "Ler PDF" — Guard de Superfície Ativa

O handler `reader_read_aloud` em `panels.rs` deve verificar se existe uma superfície PDF ativa antes de despachar o `tts-read`:

```rust
// neural-app/src/windows_app/panels.rs — extensão do handler §3.8 da SPEC-0117
"reader_read_aloud" => {
    match self.active_pdf_surface() {
        Some(surface) => {
            // Continua com o pipeline existente (SPEC-0116 / SPEC-0119 Kokoro)
            self.dispatch_tts_read(surface, args)
        }
        None => {
            // Avisa o usuário e não falha silenciosamente
            self.notify(HintTone::Warning, "Nenhum PDF aberto. Abra um documento primeiro.");
            Ok(ToolResponse::noop())
        }
    }
}
```

### 3.4 Restauração de Superfície em Fullscreen após Minimize

Ao minimizar o painel PDF em fullscreen e reabri-lo, o `WindowsApp` deve:

1. Preservar `pdf_scroll_position` e `pdf_current_page` no `TabSession`.
2. Ao recriar a superfície, injetar via IPC a restauração de página antes do primeiro render.

---

## 4. Parte B — Kokoro Offline TTS

### 4.1 O que é o Kokoro

**Kokoro** é um modelo TTS neural de arquitetura leve (~82 MB em `int8`), Apache 2.0, que gera áudio de qualidade comparável ao Edge TTS com latência de primeiro token < 200 ms em CPU moderna — sem nenhuma chamada de rede.

Repositório de referência: `hexgrad/Kokoro-82M`

Formatos de runtime suportados: **ONNX** (target principal para integração Rust via `ort`).

### 4.2 Arquitetura do Motor Offline

```
[ Botão "Ler PDF" / tts-read via IPC ]
        │
        ▼
[ TtsEngineSelector (neural-app) ]
        │
        ├── online?  ──► [ EdgeTtsEngine (SPEC-0116) ] ──► wss://speech.platform.bing.com
        │
        └── offline / modo privado / sem rede
                │
                ▼
        [ KokoroEngine (neural-core/tts/kokoro.rs) ]
                │
                ├── Sessão ONNX Runtime (ort::Session)
                │       └── modelo: assets/models/kokoro-v1.0.int8.onnx (~82 MB)
                │
                ├── Tokenizer (G2P pt-BR / en-US via phonemizer-rs)
                │
                ├── Áudio PCM 24kHz f32 → MP3 (symphonia-encode)
                │
                └── [ rodio::Sink ] ──► Altifalantes
                        │
                        └── WordBoundary sintético (por fonema estimado)
                                │
                                ▼
                        Destaque visual sincronizado (CSS highlight)
```

### 4.3 Integração com o `TtsEngine` Trait (SPEC-0116)

O Kokoro implementa o mesmo trait `TtsEngine` definido na SPEC-0116, garantindo substituição transparente:

```rust
// neural-core/src/tts/kokoro.rs

use crate::tts::{AudioStreamEvent, TtsConfig, TtsEngine, TtsError, WordBoundary};
use ort::{Environment, Session, SessionBuilder};
use tokio::sync::mpsc;

pub struct KokoroEngine {
    session: Arc<Session>,
    voice_style: KokoroVoice,
}

#[derive(Debug, Clone, Copy)]
pub enum KokoroVoice {
    PtBrFemale,   // voz padrão pt-BR (equivalente a FranciscaNeural)
    EnUsNeutral,  // voz padrão en-US
}

#[async_trait]
impl TtsEngine for KokoroEngine {
    async fn synthesize_stream(
        &self,
        text: String,
        config: TtsConfig,
    ) -> Result<mpsc::Receiver<AudioStreamEvent>, TtsError> {
        let (tx, rx) = mpsc::channel(64);
        let session  = Arc::clone(&self.session);

        tokio::task::spawn_blocking(move || {
            // 1. G2P: texto → fonemas
            let phonemes = phonemize(&text, config.lang());
            // 2. Inferência ONNX em blocos de até 200 tokens
            for chunk in phonemes.chunks(200) {
                let audio_pcm = run_kokoro(&session, chunk)?;
                // 3. Emite WordBoundary sintéticos (1 por token de fonema)
                for (i, _phoneme) in chunk.iter().enumerate() {
                    let _ = tx.blocking_send(AudioStreamEvent::Boundary(WordBoundary {
                        offset_ms: estimate_offset(i, &audio_pcm),
                        duration_ms: 80,
                        text: chunk[i].surface.clone(),
                        word_index: i,
                    }));
                }
                // 4. Emite chunk de áudio PCM
                let _ = tx.blocking_send(AudioStreamEvent::AudioChunk(pcm_to_mp3(&audio_pcm)));
            }
            let _ = tx.blocking_send(AudioStreamEvent::Complete);
            Ok::<_, TtsError>(())
        });

        Ok(rx)
    }

    fn is_offline(&self) -> bool { true }
}
```

### 4.4 Seletor de Motor (`TtsEngineSelector`)

```rust
// neural-app/src/windows_app/tts_selector.rs

pub struct TtsEngineSelector {
    edge:   Arc<dyn TtsEngine>,  // EdgeTtsEngine (SPEC-0116)
    kokoro: Option<Arc<dyn TtsEngine>>, // None se model pack não instalado
}

impl TtsEngineSelector {
    pub fn select(&self, session_is_private: bool) -> Arc<dyn TtsEngine> {
        // Modo privado → sempre offline (invariante SPEC-0116 §7)
        if session_is_private {
            return self.kokoro_or_winrt();
        }
        // Sem rede detectada → fallback offline
        if !network_available() {
            return self.kokoro_or_winrt();
        }
        // Padrão → Edge TTS (online, melhor qualidade)
        Arc::clone(&self.edge)
    }

    fn kokoro_or_winrt(&self) -> Arc<dyn TtsEngine> {
        self.kokoro
            .clone()
            .unwrap_or_else(|| Arc::new(WinRtTtsEngine::default()))
    }
}
```

### 4.5 Ciclo de Vida do Model Pack Kokoro (SPEC-0102)

O modelo Kokoro segue o sistema de **Model Packs** da SPEC-0102:

| Estado | Comportamento |
|--------|--------------|
| Pack **não instalado** | `KokoroEngine` não existe; fallback para WinRT TTS (sistema) |
| Pack **instalado, não ativo** | Modelo em disco mas não carregado em memória |
| Pack **ativo** | Sessão ONNX carregada; `is_offline() == true` |
| **Idle** (nenhum PDF sendo lido) | Sessão mantida em memória (< 150 MB RSS); sem threads ativas |

Comando de instalação via barra de endereço:

```
/modelo instalar kokoro-pt-br-v1
/modelo ativar kokoro-pt-br-v1
/modelo status
```

### 4.6 Manifesto do Model Pack

```json
{
  "id": "kokoro-pt-br-v1",
  "version": "1.0.0",
  "description": "Kokoro TTS offline — voz feminina pt-BR (~82 MB)",
  "license": "Apache-2.0",
  "files": [
    {
      "name": "kokoro-v1.0.int8.onnx",
      "url": "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/kokoro-v1.0.int8.onnx",
      "sha256": "<hash_verificado_em_ci>",
      "size_bytes": 85983232,
      "dest": "models/kokoro/"
    },
    {
      "name": "voices/pt_br_female.bin",
      "url": "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/voices/af_heart.pt",
      "sha256": "<hash_verificado_em_ci>",
      "size_bytes": 395264,
      "dest": "models/kokoro/voices/"
    }
  ],
  "runtime": "onnx",
  "capabilities": ["tts"],
  "languages": ["pt-BR", "en-US"]
}
```

---

## 5. Integração com o Canal IPC (SPEC-0108) e Agent Tool (SPEC-0117)

Nenhuma nova ação IPC é necessária. O seletor de motor é transparente para o handler `tts-read`:

```json
{
  "v": 1,
  "cap": "<token_32_hex>",
  "action": "tts-read",
  "args": {
    "block_id": "p_042",
    "voice": "pt-BR-FranciscaNeural",
    "rate": "+0%",
    "text": "Conteúdo do parágrafo extraído pelo PDF.js..."
  }
}
```

O campo `voice` é mapeado pelo `TtsEngineSelector`:
- Online → Edge TTS usa `pt-BR-FranciscaNeural` diretamente.
- Offline → Kokoro usa `KokoroVoice::PtBrFemale` (ignora o nome de voz de nuvem).

A tool `reader_read_aloud` da SPEC-0117 ganha o campo opcional `engine`:

```json
{ "name": "reader_read_aloud", "args": { "engine": "auto" | "online" | "offline" } }
```

---

## 6. Fronteira de Segurança e Privacidade

| Invariante | Regra |
|-----------|-------|
| **Modo Privado** | `TtsEngineSelector` **nunca** usa Edge TTS. Kokoro ou WinRT. |
| **Model Pack hash** | SHA-256 verificado antes da extração (CI gate). |
| **Sessão ONNX** | Corre em `spawn_blocking`; nunca bloqueia a thread de UI. |
| **Texto para inferência** | Nunca gravado em disco; processado em memória e descartado. |
| **0 bytes em idle** | Sessão ONNX pode ser descarregada após 5 min sem uso (`ModelPackManager::deactivate_idle`). |

---

## 7. Orçamentos de Desempenho

| Métrica | Orçamento | Verificação |
|--------|-----------|------------|
| **TTFS offline (Kokoro)** | ≤ 700 ms (CPU 4 núcleos) | Benchmark `cargo bench tts_kokoro_ttfs` |
| **TTFS online (Edge TTS)** | ≤ 400 ms (SPEC-0116) | Inalterado |
| **CPU em inferência Kokoro** | < 60% em 4 núcleos (chunk 200 tokens) | CI gate no benchmark |
| **RSS com pack ativo** | < 150 MB adicional | Monitorado via `HeapProfiler` |
| **RSS com pack idle** | 0 MB (sessão descarregada) | `ModelPackManager` com timer 5 min |
| **Latência de stop** | < 20 ms | Inalterado (SPEC-0116 `Sink::stop()`) |
| **PDF routing intercept** | < 5 ms (cancelamento de navegação) | Teste E2E `pdf_routing_intercepts_all_surfaces` |

---

## 8. Contrato de API no `neural-core`

```rust
// neural-core/src/tts/mod.rs — extensão da SPEC-0116

pub mod edge;    // EdgeTtsEngine (SPEC-0116)
pub mod kokoro;  // KokoroEngine (SPEC-0119) ← novo
pub mod winrt;   // WinRtTtsEngine (fallback sistema)

/// Retorna o engine recomendado para a sessão atual.
/// Usado exclusivamente pelo TtsEngineSelector em neural-app.
pub fn recommend_engine(
    kokoro: Option<Arc<dyn TtsEngine>>,
    session_private: bool,
    has_network: bool,
) -> Arc<dyn TtsEngine> { /* ... */ }
```

---

## 9. Critérios de Aceitação e Gates de CI

| Gate | Descrição |
|------|-----------|
| `pdf_routing_intercepts_all_surfaces` | Link `.pdf` clicado em qualquer coluna abre em `neuralia-pdf.localhost`, não baixa pelo sistema. |
| `pdf_routing_side_panel_default` | PDF remoto abre no painel lateral por padrão. |
| `pdf_routing_fullscreen_restores_page` | Minimize + reabrir fullscreen restaura a página correta. |
| `reader_read_aloud_without_active_pdf_emits_warning` | Botão "Ler PDF" sem PDF aberto emite `HintTone::Warning`, não panic. |
| `kokoro_model_hash_verified_before_extraction` | CI rejeita pack com hash divergente. |
| `kokoro_never_used_in_private_session_if_edge_tts_attempted` | Guard de modo privado verificado em teste de sabotagem. |
| `kokoro_ttfs_under_700ms_on_4_core_cpu` | Benchmark de latência no CI. |
| `kokoro_session_unloaded_after_5min_idle` | Verificação de RSS após timer de idle. |
| `tts_selector_falls_back_to_winrt_if_kokoro_not_installed` | `KokoroEngine::None` → WinRT TTS. |

---

## 10. Fases de Implementação

### Fase 1 — Correção do PDF Routing (`neural-app`)
- Interceptar `NavigationStarting` em todas as superfícies para URLs `.pdf`.
- Implementar `PdfRoute::Remote` com fetch assíncrono e `open_pdf`.
- Guard de superfície ativa no handler `reader_read_aloud`.
- Restauração de `pdf_current_page` no ciclo fullscreen → minimize → reabrir.
- Gates: `pdf_routing_*` e `reader_read_aloud_without_active_pdf_emits_warning`.

### Fase 2 — Kokoro Engine (`neural-core/tts/kokoro.rs`)
- Implementação do `KokoroEngine` com `ort` (ONNX Runtime).
- G2P simples para `pt-BR` e `en-US` via `phonemizer-rs`.
- Pipeline PCM → MP3 via `symphonia`.
- WordBoundary sintético por bloco de fonemas.
- Gates: `kokoro_ttfs_under_700ms_on_4_core_cpu`.

### Fase 3 — TtsEngineSelector e Integração
- `TtsEngineSelector` com lógica online/offline/privado.
- Mapeamento de voz de nuvem para voz Kokoro.
- Manifesto do Model Pack e verificação SHA-256 em CI.
- Gates: `kokoro_never_used_in_private_session_*`, `tts_selector_*`.

### Fase 4 — Idle Management e Polimento
- Timer de desativação de sessão Kokoro após 5 min idle.
- Comando `/modelo status` mostrando engine ativo na sessão atual.
- Benchmark automatizado no CI e coleta de RSS.
- Gates: `kokoro_session_unloaded_after_5min_idle`.
