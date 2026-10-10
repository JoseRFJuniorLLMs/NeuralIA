# SPEC-0117 — NeuralIA Agentic OS: Orquestração Total por Gemini Live & Agent Tools

**Status:** Proposed / Architecture Blueprint  
**Alvo:** NeuralIA 3.0 (Linha 2.9.x+)  
**Depende de:** SPEC-0104 (Agent Security), SPEC-0105 (Agent Runtime), SPEC-0108 (Secure WebView IPC), SPEC-0109 (WebRTC Media Permissions), SPEC-0116 (Neural Read Aloud)  
**Relacionada com:** SPEC-0007 (History), SPEC-0100 (Memória Semântica), SPEC-0101 (Research Sessions), SPEC-0102 (Local Intelligence), SPEC-0103 (Semantic Timeline), SPEC-0110 (PDF/EPUB Reader), SPEC-0114 (Anti-Distração), SPEC-0115 (Chat Surface)

---

## 1. Propósito e Visão Geral

O **NeuralIA** é um ambiente nativo em Rust projetado para ser **100% customizado, observado e operado por Inteligência Artificial em tempo real** — tanto pela voz e visão contínua do **Gemini Live** (`neuralia-live.localhost`) quanto pelos **Agentes de Execução (`Agent Tools` / `Agents Hub`)**.

Esta especificação define a arquitetura do **NeuralIA Tool Bus (`NeuralIAToolBus`)** e a integração de **Function Calling Bidirecional Assíncrono (`NON_BLOCKING`)** no protocolo `BidiGenerateContent` do Gemini Live. Com isso, o Gemini Live e os Agentes têm acesso estruturado a **100% dos subsistemas do navegador**, podendo operar qualquer função em primeiro ou segundo plano (inclusive quando o painel do Gemini Live está minimizado):

1. **Histórico Completo, Linha do Tempo Semântica e Memória Local (`history.*` & `memory.*`)**
2. **Controle Total do Navegador, Comparador de 3 IAs, Abas e DOM (`browser.*`)**
3. **Motor de Consenso das 3 IAs e Sessões de Pesquisa (`consensus.*` & `research.*`)**
4. **Gravação de Tela, Áudio, Replay Instantâneo e OCR (`recorder.*`)**
5. **Tradução Simultânea em Tempo Real — Páginas, Livros e Voz (`translate.*`)**
6. **Legendagem de Vídeos e Reuniões ao Vivo (`captions.*`)**
7. **Copiloto e Participação Ativa em Reuniões — Meet, Teams, WhatsApp (`meeting.*`)**
8. **Leitura em Voz Alta Interativa de Textos, Modo Leitor, PDFs e EPUBs (`reader.*`)**
9. **Notas Zettelkasten e Grafo Obsidian (`notes.*`)**
10. **Downloads, Biblioteca de Livros/PDFs, Favoritos e Auditoria de Arquivos (`downloads.*` & `bookmarks.*`)**
11. **Serviços em Segundo Plano, Monitor de Gmail e YouTube (`services.*`)**
12. **Central de Agentes (`Agents Hub`), Anti-Distração, Adblock, Pomodoro, Respiração e Sistema (`agents.*`, `focus.*`, `system.*`)**

---

## 2. Arquitetura do Barramento Unificado (`NeuralIAToolBus`)

Tanto o **Gemini Live** (via WebSocket `BidiGenerateContent`) quanto os **Agentes Internos** (`agents_hub.rs` / `decide_agent_step`) convergem para o mesmo barramento nativo em Rust (`NeuralIAToolBus`). Nenhuma funcionalidade do NeuralIA fica presa apenas ao clique do mouse: **tudo o que existe na UI tem uma Tool equivalente no `NeuralIAToolBus`**.

```text
┌────────────────────────────────────────────────────────────────────────────────────┐
│                       USUÁRIO (Voz, Visão da Tela, Câmera, Mouse)                  │
└───────────────────┬──────────────────────────────────────────────┬─────────────────┘
                    │ (PCM 16kHz + JPEG 1fps + Áudio Sistema)      │
                    ▼                                              ▼
┌───────────────────────────────────────────┐    ┌───────────────────────────────────┐
│      Painel Gemini Live (WebView2)        │    │     Agents Hub & Automação        │
│      http://neuralia-live.localhost       │    │   (agents_hub.rs / SPEC-0105)     │
│                                           │    └─────────────────┬─────────────────┘
│  wss://generativelanguage.googleapis...   │                      │
│  BidiGenerateContent (Áudio + Tools)      │                      │
│   ├─ setup.tools (functionDeclarations)   │                      │
│   ├─ serverContent (Voz 24kHz + Transcr.) │                      │
│   ├─ toolCall (id, name, args)            │                      │
│   └─ toolResponse (id, response)          │                      │
└───────────────────┬───────────────────────┘                      │
                    │ Canal IPC Seguro (SPEC-0108)                 │
                    │ LiveMessage::ToolCall                        │
                    ▼                                              ▼
┌────────────────────────────────────────────────────────────────────────────────────┐
│                       NeuralIAToolBus (Rust Nativo — App)                          │
│  ┌──────────────────────────────────────────────────────────────────────────────┐  │
│  │ Gate de Segurança e Privacidade (SPEC-0104: Tier 0 / Tier 1 / Tier 2)        │  │
│  └──────────────────────────────────────┬───────────────────────────────────────┘  │
│                                         │                                          │
│  ┌────────────┬────────────┬────────────┼────────────┬────────────┬─────────────┐  │
│  ▼            ▼            ▼            ▼            ▼            ▼             ▼  │
│[Histórico &][Comparador ][Gravador,  ][Tradutor & ][Copiloto   ][Leitor PDF/ ][Notas│
│ Memória /  ][3 IAs, Abas][Replay &   ][Legendas   ][Reuniões & ][EPUB, TTS & ][Downl│
│ Timeline   ][& Consenso ][OCR Tela   ][Ao Vivo    ][Gmail/Serv.][Favoritos   ][Foco]│
└────────────┴────────────┴────────────┴────────────┴────────────┴─────────────┴────┘
```

---

## 3. Catálogo Completo de Tools por Subsistema

Todas as ferramentas são declaradas com `behavior: "NON_BLOCKING"`, permitindo que o Gemini Live continue ouvindo e falando naturalmente enquanto executa ações no navegador.

### 3.1 Histórico de Navegação, Memória Semântica e Linha do Tempo (`history.*` & `memory.*`)

Conecta o Gemini Live diretamente a `history.rs`, `clear_history.rs`, `memory.rs` (`SPEC-0100`) e `semantic_timeline.rs` (`SPEC-0103`):

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `history_list_recent` | `limit?: number (1..200)`, `kind_filter?: "all" \| "ask" \| "read" \| "external"` | Retorna as entradas recentes do histórico (`recent_history`), incluindo perguntas feitas às IAs, artigos lidos no Modo Leitor e sites abertos, com timestamps e URLs. |
| `history_search` | `query: string`, `date_from?: string`, `date_to?: string` | Pesquisa no histórico por palavras-chave no título, URL ou pergunta realizada (ex.: *"Qual foi aquele site sobre Rust que eu abri ontem à tarde?"*). |
| `history_reopen` | `entry_id_or_url: string`, `target?: "comparator" \| "reader" \| "split"` | Reabre imediatamente uma pesquisa antiga nas 3 IAs ou uma página do histórico, ou navega `Back`/`Forward` (`navigate_history`). |
| `history_clear` | `scope: "last_hour" \| "today" \| "all" \| "domain"`, `domain?: string`, `include_memory: bool` | Limpa o histórico de navegação e/ou a memória semântica (`clear_history.rs`), respeitando confirmação de segurança. |
| `memory_semantic_query` | `query: string`, `limit?: number` | Consulta o banco SQLite de Memória Semântica Local (`SPEC-0100`) para recuperar trechos exatos de páginas, PDFs e pesquisas que o usuário já leu no passado. |
| `memory_timeline_browse` | `time_window: string`, `topic?: string` | Navega pela Linha do Tempo Semântica (`SPEC-0103`) para reconstruir a sequência cronológica de estudos ou pesquisas do usuário. |

---

### 3.2 Controle do Navegador, Comparador de 3 IAs, Abas e DOM (`browser.*`)

Conecta o Gemini Live a `compare.rs`, `tabs.rs`, `split.rs`, `navigation.rs` e `decide_agent_step` (`SPEC-0105`):

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `browser_search_ai` | `query: string`, `mode: "all_3" \| "google_ai" \| "web"` | Dispara uma pesquisa simultânea nas 3 colunas (**Gemini, ChatGPT e Claude**), no Google AI Mode (`ask:`) ou abre busca web. |
| `browser_navigate` | `url: string`, `target: "reader" \| "split" \| "full_web" \| "private"` | Abre qualquer URL no Modo Leitor, em Split View, em página completa ou no painel Privado (`InPrivate`). |
| `browser_layout` | `action: "minimize_col" \| "expand_col" \| "restore_all" \| "close_split" \| "fullscreen_split" \| "go_home"`, `column?: 0..2` | Minimiza, expande em tela cheia ou restaura qualquer coluna de IA (`0=Gemini`, `1=ChatGPT`, `2=Claude`), controla o Split View ou volta para a Home. |
| `browser_tabs` | `action: "new" \| "switch" \| "close" \| "close_others" \| "group" \| "reload"`, `column: 0..2`, `tab_index?: number`, `group_name?: string` | Controla todas as abas e grupos de abas na barra de título nativa de cada IA. |
| `browser_dom_action` | `surface: "col_0" \| "col_1" \| "col_2" \| "split" \| "webview"`, `action: "click" \| "type" \| "scroll" \| "select" \| "extract"`, `target: string`, `value?: string` | Opera os elementos da página (clicar em links/botões, digitar em formulários, rolar a tela, selecionar filtros e extrair textos/tabelas) via `decide_agent_step` (`SPEC-0105`). |

---

### 3.3 Motor de Consenso das 3 IAs e Sessões de Pesquisa (`consensus.*` & `research.*`)

Conecta o Gemini Live a `consensus.rs` e `research.rs` (`SPEC-0101`):

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `consensus_compare_columns` | `focus?: string`, `save_to_notes?: bool` | Lê as respostas atuais das 3 colunas (Gemini, ChatGPT e Claude), executa o motor de **Consenso (`consensus.rs`)** e explica por voz onde as 3 IAs concordam, onde divergem e qual resposta está mais completa. |
| `research_session_manage` | `action: "status" \| "list_turns" \| "export_markdown"` | Consulta a sessão de pesquisa ativa (`current_research`), lista todas as perguntas/turnos feitos na sessão e exporta o dossiê completo para o Zettelkasten. |

---

### 3.4 Gravação de Tela, Clipes Instantâneos e Captura OCR (`recorder.*`)

Usa as permissões de mídia supervisionadas (`SPEC-0109`) para gravar e documentar qualquer atividade:

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `recorder_screen` | `action: "start" \| "pause" \| "resume" \| "stop"`, `include_mic: bool`, `include_system_audio: bool`, `title?: string` | Grava vídeo e áudio da tela ou janela (`MediaRecorder` WebM/MP4), salva em Downloads e gera automaticamente uma nota com resumo e *timestamps* dos momentos importantes. |
| `recorder_clip_last` | `seconds: number (10..300)`, `note_title?: string` | Salva um replay instantâneo (*rolling buffer*) dos últimos $N$ segundos de vídeo + transcrição do que foi dito e anexa a uma nota Zettelkasten. |
| `recorder_snapshot_ocr` | `region?: "full" \| "col_0" \| "col_1" \| "col_2" \| "split"`, `save_to_notes: bool` | Tira um print de alta definição da tela ou coluna, extrai todo o texto, código ou tabela via visão do Gemini e salva nas Notas com o link da fonte. |

---

### 3.5 Tradução Simultânea em Tempo Real — Texto, Páginas e Voz (`translate.*`)

Conecta o Gemini Live a `translation.rs`, `translate.rs` e ao pipeline de áudio:

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `translate_surface` | `target: "col_0" \| "col_1" \| "col_2" \| "split" \| "reader" \| "pdf" \| "epub"`, `target_lang: string` | Aciona a tradução nativa da coluna, página web, PDF ou capítulo EPUB mantendo o layout original. |
| `translate_live_audio` | `action: "start" \| "stop"`, `source_lang: string`, `target_lang: string`, `mode: "voice_and_subtitles" \| "subtitles_only"` | **Intérprete Simultâneo:** ouve o áudio de vídeos (YouTube, aulas) ou reuniões em outro idioma e traduz em tempo real para o usuário (com voz em português e/ou legenda ao vivo). |
| `translate_selection` | `text?: string`, `target_lang: string`, `save_glossary?: bool` | Traduz um trecho selecionado ou visível na tela, explica expressões técnicas e opcionalmente salva no vocabulário/notas do usuário. |

---

### 3.6 Legendagem de Vídeos e Áudio em Tempo Real (`captions.*`)

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `captions_live_overlay` | `enable: bool`, `translate_to?: string`, `position?: "bottom" \| "top" \| "floating"` | Exibe uma camada flutuante de **Legendas em Tempo Real (Live Captions)** sobre qualquer vídeo, curso ou chamada, com opção de legenda bilíngue simultânea. |
| `captions_export` | `format: "srt" \| "vtt" \| "markdown_note"`, `include_summary: bool` | Exporta todas as legendas geradas do vídeo ou reunião para arquivo `.srt`/`.vtt` ou salva a transcrição completa nas Notas. |

---

### 3.7 Copiloto e Participação em Reuniões (`meeting.*`)

Integração direta com os painéis de `Google Meet`, `Microsoft Teams` e `WhatsApp`:

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `meeting_open_or_join` | `service: "meet" \| "teams" \| "whatsapp"`, `meeting_url?: string` | Abre o painel lateral do serviço ou entra diretamente na sala de reunião. |
| `meeting_copilot_mode` | `mode: "scribe" \| "whisper_coach" \| "active_assistant" \| "off"` | • **`scribe` (Escrivão):** transcreve a reunião em silêncio e anota decisões, prazos e *action items*.<br>• **`whisper_coach` (Conselheiro Silencioso):** analisa os slides da reunião e mostra dicas discretas na tela quando fazem uma pergunta ao usuário.<br>• **`active_assistant` (Participante Ativo):** responde dúvidas da equipe por voz ou prepara mensagens no chat quando autorizado. |
| `meeting_summarize_so_far` | `focus?: "decisions" \| "action_items" \| "missed_last_5_min"` | Resume instantaneamente o que foi discutido até o momento ou o que o usuário perdeu nos últimos minutos, gerando a ata completa no Zettelkasten. |
| `meeting_send_chat` | `message: string` | Envia (com confirmação Tier 2) uma mensagem, ata ou link no chat da reunião. |

---

### 3.8 Leitura Interativa de Textos, Modo Leitor, PDFs e EPUBs (`reader.*`)

Conecta o Gemini Live a `reader.rs`, `epub.rs`, `SPEC-0110` (PDF Reader) e `SPEC-0116` (Neural Read Aloud):

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `reader_read_aloud` | `action: "start" \| "pause" \| "resume" \| "stop"`, `target: "selection" \| "current_page" \| "full_article" \| "chapter"`, `speed?: number` | Lê em voz alta o artigo no Modo Leitor, página do PDF ou capítulo do EPUB com destaque visual sincronizado. Permite interrupção por voz (*barge-in*) para explicar parágrafos difíceis e retomar a leitura depois. |
| `reader_navigate_doc` | `action: "next_page" \| "prev_page" \| "goto_page" \| "goto_chapter" \| "toc" \| "search_in_doc"`, `value?: string` | Avança/volta páginas do PDF ou EPUB, abre o sumário (TOC), pula para um capítulo ou busca um termo dentro do livro. |
| `reader_highlight_and_note` | `quote: string`, `comment?: string` | Destaca um trecho do livro/PDF/artigo e salva uma nota Zettelkasten já vinculada ao documento e número da página. |

---

### 3.9 Notas Zettelkasten e Grafo Obsidian (`notes.*`)

Conecta o Gemini Live a `zettel.rs` e `notes.rs`:

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `notes_manage` | `action: "create" \| "append" \| "read" \| "search" \| "delete" \| "capture_selection"`, `title?: string`, `content?: string`, `query?: string` | Cria notas atômicas com `[[wikilinks]]`, adiciona conteúdo a notas existentes, captura a seleção atual da página (`request_note_from_page`), pesquisa ou lê notas em voz alta. |
| `notes_obsidian_graph` | `action: "open_graph" \| "find_connections"`, `topic?: string` | Abre o Grafo Obsidian no painel lateral (`show_obsidian_panel`) e identifica conexões entre notas e pesquisas do histórico. |

---

### 3.10 Downloads, Biblioteca Local, Favoritos e Risco de Arquivos (`downloads.*` & `bookmarks.*`)

Conecta o Gemini Live a `downloads.rs`, `downloads_ui.rs`, `library.rs`, `file_risk.rs` e `bookmarks.rs`:

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `downloads_manage` | `action: "list" \| "open_panel" \| "open_file" \| "verify_risk"`, `download_id_or_name?: string` | Lista os downloads realizados, abre a aba de Downloads (`Ctrl+J`), abre um PDF/EPUB baixado no leitor interno e informa a análise de segurança do arquivo (`file_risk.rs`). |
| `library_documents` | `action: "list_books" \| "open_book" \| "import"`, `query_or_id?: string` | Lista todos os livros EPUB e documentos PDF da Biblioteca Local (`library.rs`) e abre qualquer livro por comando de voz. |
| `bookmarks_manage` | `action: "add_current" \| "list" \| "search" \| "open" \| "remove"`, `query_or_url?: string`, `folder?: string` | Favorita a página/coluna/livro atual (`bookmarks.rs`), pesquisa nos favoritos salvos ou abre um favorito. |

---

### 3.11 Serviços em Segundo Plano, Monitor de Gmail e YouTube (`services.*`)

Conecta o Gemini Live a `services.rs` e `gmail.rs`:

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `services_panel` | `service: "whatsapp" \| "youtube" \| "gmail" \| "outlook" \| "teams" \| "meet" \| "breath"`, `action: "open" \| "minimize" \| "close"` | Abre, minimiza para segundo plano ou fecha qualquer painel de serviço lateral. |
| `services_gmail_status` | `action: "check_unread" \| "toggle_notifications"` | Consulta o monitor de Gmail em segundo plano (`gmail.rs`) para informar se chegaram novos e-mails ou alterna os avisos de área de trabalho. |
| `services_media_control` | `action: "play_pause" \| "mute_unmute"` | Controla vídeos ou áudios tocando em segundo plano no painel do YouTube sem o usuário precisar sair da pesquisa atual. |

---

### 3.12 Central de Agentes (`Agents Hub`), Anti-Distração, Pomodoro e Sistema (`agents.*`, `focus.*`, `system.*`)

Conecta o Gemini Live a `agents_hub.rs`, `distraction.rs` (`SPEC-0114`), `adblock.rs`, `pomodoro.rs` e `update.rs`:

| Nome da Tool | Argumentos Principais | O que faz no NeuralIA |
| :--- | :--- | :--- |
| `agents_hub_manage` | `action: "list" \| "run" \| "stop"`, `agent_name?: string`, `goal?: string` | Lista os agentes configurados no `Agents Hub` (`agents_hub.rs`) e dispara ou interrompe a execução de um agente especializado. |
| `focus_pomodoro` | `action: "start" \| "pause" \| "reset" \| "status"` | Inicia, pausa ou consulta o tempo restante do ciclo Pomodoro na barra de título. |
| `focus_anti_distraction` | `action: "status" \| "enable_focus_guard" \| "open_breath"` | Consulta ou ativa a proteção Anti-Distração (`SPEC-0114` / `distraction.rs`), informa estatísticas de bloqueio (`adblock.rs`) ou abre a Respiração Guiada Wim Hof (`Service::Breath`). |
| `system_control` | `action: "check_update" \| "open_about" \| "window_fullscreen" \| "window_minimize"` | Verifica e aplica atualizações do NeuralIA (`check_and_apply_update`), abre a aba Sobre com a versão compilada ou ajusta o estado da janela. |

---

## 4. Protocolo WebSocket e Canal IPC (`live-core.js` <-> `gemini_live.rs`)

### 4.1 Declaração de Tools no `setupMessage` (`live-core.js`)

No handshake do WebSocket `BidiGenerateContent`, `setupMessage` registra todas as declarações do `NeuralIAToolBus` com `behavior: "NON_BLOCKING"`:

```json
{
  "setup": {
    "model": "models/gemini-2.5-flash-native-audio-preview-12-2025",
    "generationConfig": { "responseModalities": ["AUDIO"] },
    "systemInstruction": {
      "parts": [{
        "text": "Você é o agente multimodal central do NeuralIA. Você vê a tela, ouve o usuário e controla 100% das ferramentas do navegador (histórico, memória semântica, linha do tempo, comparador de 3 IAs, consenso, abas, gravação de tela, tradução simultânea, legendas ao vivo, copiloto de reuniões, leitura de PDFs/EPUBs, notas Zettelkasten, grafo Obsidian, downloads, favoritos, serviços em segundo plano, Agents Hub e Pomodoro). Execute as ferramentas sempre que o usuário pedir ou para auxiliá-lo proativamente."
      }]
    },
    "tools": [
      {
        "functionDeclarations": [
          { "name": "history_list_recent", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "history_search", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "history_reopen", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "memory_semantic_query", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "browser_search_ai", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "browser_navigate", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "browser_layout", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "browser_tabs", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "browser_dom_action", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "consensus_compare_columns", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "recorder_screen", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "translate_surface", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "translate_live_audio", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "captions_live_overlay", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "meeting_copilot_mode", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "reader_read_aloud", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "notes_manage", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "notes_obsidian_graph", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "downloads_manage", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "bookmarks_manage", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "services_panel", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "agents_hub_manage", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "focus_pomodoro", "behavior": "NON_BLOCKING", "description": "..." },
          { "name": "system_control", "behavior": "NON_BLOCKING", "description": "..." }
        ]
      }
    ],
    "inputAudioTranscription": {},
    "outputAudioTranscription": {},
    "contextWindowCompression": { "slidingWindow": {} },
    "sessionResumption": {}
  }
}
```

### 4.2 Fluxo `toolCall` -> IPC -> `NeuralIAToolBus` -> `toolResponse`

1. **Do Servidor Gemini para `live-core.js`:**
   `parseServerMessage` extrai `message.toolCall.functionCalls` e `message.toolCallCancellation.ids`.
2. **De `live.js` para o Rust Nativo via IPC (`SPEC-0108`):**
   ```json
   {
     "action": "tool_call",
     "args": {
       "id": "call_42",
       "name": "history_search",
       "params": { "query": "arquitetura rust webview2" }
     }
   }
   ```
3. **Do Rust Nativo (`NeuralIAToolBus`) de volta para `live.js` -> WebSocket:**
   ```json
   {
     "toolResponse": {
       "functionResponses": [
         {
           "id": "call_42",
           "name": "history_search",
           "response": {
             "result": {
               "ok": true,
               "entries": [
                 { "title": "SPEC-0001 Architecture", "url": "https://...", "kind": "Ask" }
               ],
               "scheduling": "WHEN_IDLE"
             }
           }
         }
       ]
     }
   }
   ```

---

## 5. Modelo de Segurança, Privacidade e Governança (`SPEC-0104`)

1. **Três Níveis de Autoridade (`ToolSecurityTier`):**
   - **Tier 0 — Leitura e Consulta Local (Execução Imediata):** consultar histórico, memória semântica, linha do tempo, listar downloads/favoritos/notas, comparar consenso das 3 IAs, ativar legendas, traduzir na tela, verificar status do Gmail ou Pomodoro.
   - **Tier 1 — Ação Reversível no Workspace (Execução Imediata com Aviso Visual `HintTone`):** pesquisar nas 3 IAs, reabrir item do histórico, alternar abas/colunas/Split View, criar ou adicionar notas no Zettelkasten, abrir grafo Obsidian, favoritar página, iniciar leitura em voz alta ou gravação solicitada pelo usuário.
   - **Tier 2 — Ação Externa ou Destrutiva (Requer Confirmação via `AgentPermissionPolicy`):** limpar histórico/memória (`history_clear`), apagar notas permanentemente, enviar mensagens em chats de reunião/WhatsApp/e-mail ou submeter formulários sensíveis.
2. **Continuidade em Segundo Plano (Painel Minimizado):**
   - Com o Gemini Live minimizado (`LivePanel::is_minimized() == true`), o comparador ocupa 100% da janela, o ícone do olho permanece vermelho (`LiveIndicator::Live`) e **todas as Tools continuam operacionais** por comando de voz.
   - Cada ação executada em segundo plano exibe uma notificação arredondada semântica (`HintTone::Info` / `HintTone::Success`).
3. **Proteção de Sessões Privadas (`InPrivate`) e Segredos (`DPAPI`):**
   - Nenhuma ferramenta grava ou lê histórico/memória a partir de superfícies `InPrivate` (Painel Privado ou Respiração Guiada).
   - A chave da API do Gemini permanece protegida com `DPAPI` + entropia exclusiva e nunca aparece em logs (`redact_debug_secrets`).
