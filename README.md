<p align="center"><img src="assets/logo.ico" width="112" height="112" alt="NeuralIA"></p>

<h1 align="center">NeuralIA</h1>
<p align="center"><strong>The browser without the browser.</strong></p>
<p align="center">Search with AI. Read the Web. Open a full page only when you actually need it.</p>

## What NeuralIA is

NeuralIA **v1.7.0** is an AI-first, reader-first, system-WebView information client written in Rust.

It deliberately refuses the usual browser arms race. It does not ship Chromium, does not implement its own JavaScript engine, does not carry a local LLM, and does not recreate a full browser tab strip just to prove that rectangles can multiply.

```text
native Rust home
      │
      ├── question ─────────► Gemini + ChatGPT + Claude ─┐
      ├── ask:/? ───────────► Google AI Mode ────────────┤
      ├── URL ──────────────► Reader ────────────────────┼► lazy system WebView2
      └── web:<URL> ────────► full page ─────────────────┘
```

Uma pergunta normal abre o **comparador de três IAs** e é enviada de verdade às
três superfícies: Gemini, ChatGPT e Claude. `ask:` ou `?` limita a consulta ao
Google AI Mode quando o utilizador quer um único fornecedor. A Home não dispara
nenhuma consulta sozinha.

No comparador, links externos abrem em **Split View** ao lado da IA que gerou a
fonte; fechar a gaveta devolve a comparação sem perder o contexto. Cada
**Gemini / ChatGPT / Claude** é também um grupo de abas na **mesma barra de
título**: `IA + [aba] [aba]`. O botão **+** abre a omnibox daquela IA, hover
deixa claro o componente atingido e o clique direito numa aba abre um menu
nativo com abrir, tela cheia, fechar, fechar outras e fechar todas do grupo.

O Split View usa a mesma timeline vertical do NeuralIA: a scrollbar nativa da
página lateral é escondida e a trilha tracejada centralizada assume a navegação.
A rolagem automática percorre tanto a IA visível quanto a página lateral,
detectando containers internos de scroll. Os controles **Fonte / tela cheia /
fechar** vivem na barra nativa do NeuralIA, fora do conteúdo do site.

Os três painéis de IA podem ser **redimensionados arrastando os divisores
verticais com o mouse**, maximizados ou minimizados. Eles nunca são fechados:
minimizar só oculta a WebView e as demais ocupam o espaço liberado. As proporções
ajustadas pelo usuário ficam preservadas enquanto a sessão do comparador estiver
aberta.

`Ctrl+K` ou `Ctrl+T` abre a omnibox da IA ativa, `Ctrl+H` mostra o
histórico local e `Ctrl+N` inicia uma nova aba no grupo da IA atual. O botão
**Privado** no canto superior direito abre um Split View em modo incognito do
WebView2; ele não reutiliza cookies e não adiciona a navegação à lista normal de
fontes.

Quando já existe uma sessão Google autenticada no perfil WebView2, o NeuralIA
pode observar o Gmail em background sem armazenar senha. Uma nova mensagem gera
um aviso nativo discreto no **canto inferior direito** com remetente e assunto.
O primeiro estado da caixa de entrada é apenas a linha de base e não dispara
notificação retroativa. O monitor vive numa WebView escondida que continua viva
mesmo na tela inicial — um notificador que morre ao voltar para casa não
notifica —, só existe enquanto existir a sessão Google, e `NEURALIA_NO_GMAIL=1`
o desliga por completo.

On Windows, **no WebView is created while the native home screen is idle**
(the hidden Gmail monitor above is the single intentional exception). The Home background is also native: a low-frequency GDI neural network animation flows toward the brand without video, Canvas, WebView or network access. Set `NEURALIA_REDUCE_MOTION=1` to keep the background static. The omnibox is a native Windows edit control; WebView2 is instantiated only after the user asks, reads, or explicitly opens a page, and it is destroyed when the user returns home.

## Design rules

1. Native idle shell first; system WebView only on demand.
2. One system WebView outside the comparator; the comparator has a hard ceiling of three.
3. No local LLM in the default build.
4. No custom JavaScript engine.
5. No custom CSS compatibility project.
6. URLs open in Reader by default.
7. Full Web is an escape hatch, not the product center.
8. Performance and security budgets are architecture requirements.

## Workspace

```text
NeuralIA/
├── crates/neural-core/     # intent, URL policy, Reader, search, history
├── crates/neural-app/      # native Windows shell + lazy WebView2
├── assets/logo.ico          # application/readme icon
├── assets/neuralia-brand.jpg # official NeuralIA brand artwork
├── assets/neuralia-logo.svg  # compact in-app legacy mark
├── docs/specs/
└── .github/workflows/
```

## Input grammar

| Input | Result |
|---|---|
| `como funciona Raft?` | Comparador: Gemini + ChatGPT + Claude |
| `? MVCC vs OCC` | Google AI Mode |
| `compare: MVCC vs OCC` | Comparador explícito: Gemini + ChatGPT + Claude |
| `https://example.com/paper.pdf` | Visualizador de PDF embutido (o Reader só lê HTML) |
| `https://example.com/article` | Reader |
| `reader:https://example.com` | Reader |
| `web:https://example.com` | Full WebView |
| `home:` | Native NeuralIA home |

## Build

Portable core:

```bash
cargo test --locked -p neural-core
```

Windows desktop:

```powershell
cargo run --locked -p neural-app --release
```

The desktop app requires the Microsoft Edge WebView2 Runtime only for AI/Reader/Web surfaces.

## Keyboard

The native omnibox inherits Windows text editing, selection, clipboard, IME and accessibility behavior.

Keyboard focus always lives inside a child window (the omnibox or a WebView2),
so the shortcuts below are captured *inside* every page, in the capture phase,
before the site sees the key. They work on every surface unless noted.

| Keys | Action |
|---|---|
| `Enter` | submit the omnibox |
| `Esc` | back one level: fullscreen → 3 columns → Home |
| `Backspace` · `Alt+←` · `Alt+→` | page history back / forward |
| `Ctrl+L` | return to the omnibox with its text selected |
| `Ctrl+T` · `Ctrl+W` | Home · back |
| `Ctrl+R` · `F5` | reload |
| `Ctrl` `+` / `-` / `0` | zoom, on Chrome's ladder (25%–400%), inherited by new pages |
| `Ctrl+F` | in-page find bar (Enter / Shift+Enter / Esc) |
| `Ctrl+P` | print the page you are looking at |
| `Ctrl+H` | the 20 most recent local history entries |
| `Ctrl+Shift+Delete` | clear local history (the WebView2 profile is untouched) |
| `F12` · `Ctrl+Shift+I/J/C` | Chromium DevTools |
| `Ctrl+U` | view page source |
| `F11` | fullscreen for the current comparator column |
| `F8` | auto-scroll on / off |
| `1` `2` `3` · `0` | expand a comparator column · restore three columns |

`Backspace` and the digits are ignored while typing in a field; `Ctrl` combinations are not.

## Comparator timelines

Each AI panel owns its own vertical response timeline. The compact rail on the
right side of Gemini, ChatGPT and Claude has an up control, progress markers and
a down control. It tracks the page as answers grow and scrolls only the panel
where it lives; manual navigation in one provider never moves the other two.

## Reading

Opening a document asks once per session whether to advance the page automatically
(**Sim** / **Não**, bottom centre). With **Sim**, the page moves one screen every
30 seconds and stops at the end. HTML and comparator surfaces advance by script;
PDFs use NeuralIA's bundled offline PDF.js viewer and advance page-by-page. `F8` toggles
it at any time. EPUB is not supported: WebView2 does not open it.

## v1.0.1 hardening baseline

- Reader enforces one deadline across the complete redirect chain.
- Public Reader DNS resolution rejects loopback, private, link-local and reserved IP ranges before connecting.
- Reader uses the operating-system TLS verifier.
- Reader runs on one bounded worker; newer pending reads replace older pending reads.
- Reader HTML executes no NeuralIA JavaScript and has `script-src 'none'`.
- External Web pages receive no NeuralIA IPC.
- New-window requests are reused in the single WebView instead of multiplying WebViews.
- Local history is bounded, written off the UI thread, and can be cleared.
- Release dependencies are locked and CI uses `--locked`.
- GitHub Actions are SHA-pinned.
- Stable release assets are created once and never overwritten.
- Release creation is gated on a successful `main` CI run.

## Status

- [x] Rust workspace
- [x] native Windows idle shell
- [x] native accessible Windows omnibox
- [x] lazy one-WebView lifecycle
- [x] intent parser
- [x] Google AI Mode routing
- [x] bounded HTTP Reader
- [x] DNS/private-network Reader protection
- [x] semantic article extraction
- [x] safe script-free Reader HTML
- [x] bounded local history + native viewer + clear
- [x] AI comparator with system-themed top bar, independent response timelines and real fullscreen
- [x] Chrome-style keyboard, zoom, find bar and DevTools inside every page
- [x] opt-in auto-scroll for reading (HTML, text and PDF)
- [x] external IPC isolation
- [x] HTTP integration tests
- [x] CI Linux + Windows + RustSec
- [x] immutable release workflow with CycloneDX SBOM + provenance attestation
- [x] CI startup/RAM/thread regression gate (product targets still measured separately)
- [ ] Authenticode-signed Windows installer
- [ ] macOS shell
- [ ] Linux shell

See [CHANGELOG.md](CHANGELOG.md) and [the specification index](docs/specs/README.md).

## Non-goals

NeuralIA is not building V8, Blink, an extension ecosystem, or another bundled Chromium distribution. Those are excellent ways to turn a small repository into a hereditary obligation.

## License

MIT.
