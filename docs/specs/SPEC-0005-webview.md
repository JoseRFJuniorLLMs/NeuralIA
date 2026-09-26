# SPEC-0005 — System WebView

**Status:** Normative

NeuralIA uses the platform web engine and does not distribute a full browser engine.

On Windows the adapter is WRY backed by Microsoft Edge WebView2.

## Lifecycle invariant

The native home screen MUST NOT instantiate WebView2 to draw itself or to answer anything the user has not asked for. Home renders through GDI.

A visible WebView is created only for:
- Google AI Mode;
- Reader HTML;
- explicit Full Web mode;
- the comparator, which is the bounded exception below.

Outside the comparator, only one visible WebView may exist at a time.

The comparator is the one bounded exception: ordinary questions (and explicit `compare:` requests) create one WebView per provider, at most `COMPARATOR_COLUMNS` (3). Each column owns an independent response timeline.

Whatever the surface, returning to Home MUST destroy every live surface WebView, comparator included, and return focus to the native window. A single exit path (`destroy_web_surfaces`) owns this, and every surface transition goes through it, so entering a new surface can never leave the previous one alive.

### Gmail monitor exception

The Gmail monitor is the one WebView that Home is allowed to keep alive, and it is a deliberate product decision rather than a lifecycle leak:

- it is created only when a Google session already exists in the WebView2 profile, and never creates or prompts for one;
- it is hidden: no window area, no focus, no user-visible surface, nothing to return to;
- it survives the return to Home by design. A notifier destroyed every time the user goes Home would not notify, so `destroy_web_surfaces` deliberately does not own it;
- `NEURALIA_NO_GMAIL=1` suppresses it entirely: with that variable set, Home instantiates no WebView2 at all.

The monitor is therefore counted apart from the surface budget: Home has zero surface WebViews plus at most this one hidden monitor. Lifecycle measurement (`scripts/measure-cycles.ps1`) counts processes, not intent, so it MUST set `NEURALIA_NO_GMAIL=1`; otherwise the gate would fail on any machine with a Google session and pass only by accident on a clean runner.

## Native bridge boundary

There is no page-to-native IPC with ambient authority. Remote WebView surfaces
(external web, comparator, split, Gmail monitor and the PDF surface) receive a
message transport through WebView2, but every accepted message is a bounded JSON
envelope authenticated with a per-WebView capability. The native side exposes
no filesystem API, credential API or arbitrary native-call object.

The closed action set has 31 names: `home`, `back`, `restore`,
`autoscroll`, `zoomin`, `zoomout`, `zoomreset`, `reload`, `print`,
`omnibox`, `history`, `clearhistory`, `fullscreen`, `devtools`,
`viewsource`, `newtab`, `expand`, `shortcut-expand`, `minimize`, `split`,
`link`, `ask`, `search`, `split-close`, `split-expand`, `palette`, `gmail-state`,
`research-answer`, `agent-observation`, `hint` and `note`. Unknown actions, extra fields,
wrong types, oversized messages and invalid per-action arguments are rejected.
The parser gate `protocol_accepts_exactly_the_published_actions`
(`crates/neural-app/src/ipc.rs`) reads this list and fails when it differs from
the set the shipped parser accepts; the count is the test constant
`PUBLISHED_ACTION_COUNT`, and the same gate holds the list and count repeated in
SPEC-0108 and the count in SPEC-0015 to it.

`note` is the request "make a note of what I selected". It never carries an
address or a title from the page. With no arguments it is Ctrl+Shift+Z: the
page only asks. With exactly `via` set to the closed name `bar` and `text` it
is the selection toolbar's "📝 Salvar nota", and `text` is the text the
toolbar shows, read through the primitives it captured when the document was
created (the same text path as `search`): 1..=5000 characters after trimming
(`NOTE_TEXT_MAX_CHARS`), no control characters except newline and tab, and
the whole message still within 8 KiB -- the toolbar does not send a text that
does not fit and says so ("Seleção grande demais para Salvar nota"). Any
other key, value, case or type (a `url` or `title`, `via: "bar"` without
`text`, a `text` without `via`, `via: "page"`, `via: "Bar"`, `via: true`) is
rejected. Inside an editable field (input, textarea, contenteditable) the
keymap leaves Ctrl+Shift+Z to the page as redo, and a held Ctrl+Shift+Z asks
once (key repeats do not ask again). For Ctrl+Shift+Z the native side then
reads the selection (capped at 20 000 characters, again natively),
`location.href` and `document.title` from the WebView that sent the request
-- the emitting comparator column, the split, or the single
External/Reader/PDF WebView -- and treats that reply as page data: only an
http(s) address becomes the note's source, and on the Reader and PDF surfaces
the source is the address the native side opened. Salvar nota does not read
the page again: the note is the `text` of the request, so a page that swaps
`getSelection`, `Selection.prototype.toString` or `String` after the document
was created changes neither the note nor its notice. Its source is an address
the native side knows -- the Reader article or the PDF it opened, otherwise
the WebView's own `Source` (`webview.url()`), kept only when it is http(s) --
and its title is the first 60 characters of the text on one line, with "…"
when it goes on (`bar_note_step`, `note_capture_source`). Salvar nota is
local: there is no card and the notes panel does not open; a native centred
notice says "Nota salva: <title>". The same text saved again within 2 s is
not another note, also with another text saved in between (`BarNoteGuard`);
the same holds for the note of a Ctrl+Shift+Z (`shortcut_note_step`, its own
guard). The note is written under `<data_dir>/zettel` by a worker thread, and
the Salvar nota path writes neither history nor memory. A Ctrl+Shift+Z from
the private split is refused ("Modo privado: notas não são criadas") without
reading the page, and the split's privacy is checked again when the read would
happen. A Salvar nota from the private split is an explicit request from the
reader and is saved; its notice then says "Modo privado: a nota foi
guardada". The choice of WebView, that check and whether the WebView is the
private split's (which picks that notice) all come from `note_read_view`,
which the shipped `request_note_from_page` calls. Gates:
`note_is_a_bare_request_and_carries_no_page_data` (`ipc.rs`),
`ctrl_shift_z_on_a_page_posts_a_bare_note_request`,
`a_held_ctrl_shift_z_saves_one_note`,
`a_note_request_reads_its_own_webview_and_never_the_private_split`,
`a_selection_becomes_a_quoted_note_with_its_source`,
`salvar_nota_saves_one_note_with_the_native_source_and_never_twice_in_two_seconds`,
`salvar_nota_saves_the_readers_text_even_when_the_page_swaps_get_selection` and
`salvar_nota_sends_only_what_fits_in_the_channel`
(`crates/neural-app/src/windows_app/tests.rs`).

`link` reports a click on a link inside a comparator column. Its arguments are
exactly `col`, `url` and `aside` (boolean); `url` goes through the same
validation and local-network rejection as `split`. With `aside=true` the URL
opens in the Split panel beside the emitting column. With `aside=false` it
navigates all three comparator columns to that URL. It is the one action whose
effect reaches columns other than the one that emitted it (gate:
`a_plain_click_opens_in_all_three_panels_and_ctrl_click_opens_beside` in
`crates/neural-app/src/windows_app/tests.rs`; argument policy:
`a_link_click_obeys_the_same_policy_as_the_split` in `ipc.rs`).

`hint` reports that the pointer entered (or left) one of the controls the
comparator injects into a column (the `−` minimize and `⛶ <AI>` expand
buttons). Its arguments are exactly `col` and `id`, and `id` is one of the
closed names `minimize`, `expand` or `none`; free text is rejected. Only
trusted `mouseenter`/`mouseleave` events post it, a column only speaks for
itself, and the native side picks the text ("Minimizar <AI>", "Expandir <AI>")
and shows it in the app's centered hint. Gates:
`hint_names_one_of_three_closed_hints_for_its_own_column` (`ipc.rs`) and
`column_controls_ask_for_the_centered_hint_on_trusted_hover`
(`windows_app/tests.rs`, runs the shipped script under Node).

`ask` reports a question the user typed and sent in a comparator column (Enter
or the send button, trusted events only, on the column's own AI page). Its
arguments are exactly `col` and `text` (1..=2000 characters after trimming,
no control characters except newline and tab). The OTHER columns load the same
question in their own provider; the emitting column is not touched. Gates:
`ask_carries_the_typed_question_within_bounds` (`ipc.rs`) and
`a_question_typed_in_one_column_goes_to_the_others` (`windows_app/tests.rs`).

`search` reports the "🤖 Mandar para IA" and "🌐 Traduzir" buttons of the
selection toolbar. The toolbar is part of the keyboard-shortcut script and is offered in the comparator
columns, the Split panel, external web and the Reader (top frame only). It
appears when a trusted user gesture that selects text -- a mouse drag, a double
or triple click, Shift+click, Shift with an arrow/Home/End/PgUp/PgDn key, or
Ctrl+A -- leaves a selection of 1..=5000 characters outside editable fields,
and the selection is still the same 200 ms later: a selection the page makes
by itself, or swaps in that interval, does not bring it. After Esc or a scroll
it stays closed until the next such gesture, and it is not shown for a
selection whose end is outside the visible area. It is placed above the whole
selection when there is room, else below its last line. It lives in a closed
shadow root built when the document is created, and offers, in this order,
"🤖 Mandar para IA", "📝 Salvar nota" (`note` with `via: "bar"` and the text, above),
"🌐 Traduzir", "📋 Copiar" (clipboard, no IPC) and "⋯" (`aria-label` and
`title` "Mais"). The "⋯" opens a small menu in the same shadow root
(`role="menu"`, entries `role="menuitem"`) with what does not fit in the
bar: today only "🔊 Falar" (local `speechSynthesis` voices, no IPC). The menu
is the closed list `MORE_MENU` of the injected script, and an entry is offered
only when the document has what it needs: a document without speech synthesis
(`speechSynthesis` and the voice accessors) has no "⋯" at all, never a dead
button; with speech synthesis but no local voice, Falar is offered and says
"Nenhuma voz local disponível". The menu floats next to the "⋯", outside the
bar's box (`position:absolute`), so opening or closing it neither resizes nor
moves the toolbar: the "⋯" stays under the pointer and a second click on the
same spot closes the menu. It opens away from the selection -- above the bar
when the bar is above the text, below when the bar is below it or when there
is no room above -- aligned with the bar's right edge (its left edge when that
would leave the screen). Clicking "⋯" opens the menu
(`aria-expanded="true"`) and moves the focus to its first entry; the arrow
keys (wrapping), Home and End move inside it; a trusted Enter or Space runs
the focused entry once; Tab closes it; clicking "⋯" again, clicking outside
or a new selection close it. While nothing is being read, the first Esc
closes only the menu and gives the focus back to the page, the next closes
the toolbar, and only the one after that goes back; while Falar reads, the
first Esc stops the reading and closes the toolbar. The bar's buttons stay out of the page's Tab order
(`tabindex="-1"`). Falar never picks an online voice: it reads `localService`, `lang` and `default`
through the `SpeechSynthesisVoice` accessors, and sets the utterance's
`voice` and `lang` through the `SpeechSynthesisUtterance` setters, captured
when the document is created, so a page that redefines them cannot pass an
online voice off as local. It picks the voice in one pass over
`getVoices()`, with no intermediate list, and checks `localService` again
right before speaking; the toolbar's own lists grow through the
`Object.defineProperty` captured when the document is created, never through
`[[Set]]`, so an index accessor the page puts on `Array.prototype` or
`Object.prototype` changes neither the voice nor the sentences read. It
prefers the page language, then pt-BR, then the system language, then the
default local voice, and reads one sentence per utterance; while it reads,
Falar in the menu reads "⏹ Parar" and the menu stays open, a new selection
moves the toolbar and becomes the text Mandar para IA, Salvar nota, Traduzir
and Copiar use, and without one only the "⋯" remains, its menu open on Parar
without taking the focus from the page. A double click on a word in a
comparator column selects it and brings the toolbar instead of expanding the
column; a double click that leaves no text selected still expands it.

Mandar para IA, Salvar nota and Traduzir count a click only when the toolbar
has been on screen, where it was placed (read through the `DOMRectReadOnly` accessors captured when the
document is created), for 500 ms (also when the button went down), still a
direct child of the document root, with no page-set opacity, filter,
transform, clip-path, mask, blend mode, hidden content or hidden visibility on
it and no opacity, filter or transform on the document root, and --
when the engine provides IntersectionObserver v2 (`isVisible`) -- after that
observer has reported it visible for 500 ms; otherwise nothing is sent and the
toolbar says why ("Clique de novo em <botão>" for a click that came too soon).
These checks only filter clicks before they bother the user: the page can
still shrink or hide the toolbar in ways the page script cannot see, so a
Mandar para IA or Traduzir click only asks. `search` carries exactly `text`
(1..=2000 characters after trimming, no control characters except newline and
tab) and `intent`, one of the closed names `ask` (Mandar para IA) or
`translate` (Traduzir); a missing, unknown, empty, differently cased or
non-string `intent` is rejected, never read as `ask`. A longer selection is
not sent and the toolbar says so. The page only picks which of the two
buttons was clicked: the card's title and confirm button and the translation
request are written by the native side. The text path uses the `String` and
`String.prototype` members captured when the document is created.

The native side never sends anything to the AIs on `search` alone. It shows a
native confirmation card centred in the window -- "Mandar para as 3 IAs?"
with the buttons Mandar and Cancelar for `ask`, "Traduzir nas 3 IAs?" with
Traduzir and Cancelar for `translate` -- an owned, non-activating popup
(`WS_EX_NOACTIVATE`, shown with `SW_SHOWNOACTIVATE`) that the page cannot
cover, move, paint or click, with the text as plain text and the two buttons
handled natively (press and release on the same button, with the mouse
captured by the card). The question is
cleaned natively before the card shows it, and the cleaned text is the
question itself, not only what is drawn: line breaks, tabs and control
characters become one space, and characters that would paint as nothing are
removed -- Unicode Default_Ignorable_Code_Points (tag characters, variation
selectors, zero-width and bidirectional controls, soft hyphen, BOM, Hangul
fillers), the blank braille pattern, interlinear annotation characters and
U+FFFC, private-use code points and noncharacters. The card measures the text
with the font and `DrawTextW` format it draws with (`DT_WORDBREAK`,
`DT_EDITCONTROL`, `DT_NOPREFIX`, measured with `DT_CALCRECT`): what fits is
drawn whole; otherwise it draws the longest start that fits, then "…", and
"+N caracteres ficam de fora" -- and the part left out is not sent. Only a
click on the card's confirm button (Mandar or Traduzir), at least 600 ms after
that text appeared, opens the normal three-AI comparison, with exactly the
text the card last painted as the question -- for Traduzir, inside the fixed
request written natively (`TRANSLATE_PROMPT`): "Traduza para o português do
Brasil (se o texto já estiver em português, traduza para o inglês):", a blank
line, then that text. A Traduzir is named by the reader's text, not by the
fixed request: its research session and memory entry are "Traduzir: <text>"
and history keeps `traduzir:<text>`, an omnibox command that sends the same
request again when the entry is reopened (`CompareRequest`,
`compare_records`); a Mandar keeps the question as name and `compare:<text>`
in history. Cancelar, or 12 s without an answer, drops it. There is
one card at a time: a new request from either button replaces the text, the
title and the confirm button and restarts both clocks, and a click only
counts for the card it had painted. The comparison never goes through the
omnibox command parser or the palette, so a selected `agent:`, `tema:` or URL
is a question, not a command. A private Split has neither Mandar para IA nor
Traduzir (it keeps Salvar nota, Copiar and the "⋯") and its handler refuses
`search` with either `intent`, so it never shows the card;
`open_split_mode` hands its `private` flag, unchanged, to
`open_split_opened_by`, which hands it to `split_open_plan`, and the Split's
incognito profile, injected script, IPC handler and new-window handler (a
private Split's popups open as private Splits; a normal Split's popup opens
as a tab in the group of the tab it came from) are all set by
`configure_split_webview` from the result. Gates: `search_carries_the_selected_text_within_bounds` (`ipc.rs`),
`the_selection_toolbar_offers_four_actions_and_a_menu_for_a_trusted_selection`,
`the_selection_menu_opens_closes_and_is_reachable_by_keyboard`,
`the_more_menu_opens_without_moving_the_bar`,
`the_selection_toolbar_searches_only_what_fits_and_never_from_private`,
`pesquisar_only_counts_a_click_on_a_bar_the_user_really_saw`,
`pesquisar_asks_the_native_card_and_only_its_search_click_compares`,
`traduzir_sends_the_fixed_prompt_only_after_the_native_confirm`,
`each_translation_is_named_by_its_text_and_reopens_from_history`,
`the_search_card_shows_plain_bounded_text_and_answers_only_its_buttons`,
`the_search_card_confirms_only_the_text_it_painted`,
`the_search_card_window_answers_a_native_press_and_release_with_the_painted_token`,
`the_selection_toolbar_copies_and_speaks_with_local_voices`,
`falar_never_takes_an_online_voice_the_page_disguised_as_local`,
`while_reading_a_new_selection_is_what_copy_and_search_take`,
`every_page_surface_gets_the_toolbar_its_privacy_allows`,
`open_split_mode_hands_its_private_flag_to_the_builder`,
`double_clicking_a_word_in_a_column_selects_it_instead_of_expanding`,
`a_selected_search_reaches_the_comparator_from_every_surface_but_the_private_split`
and `a_selected_search_is_a_question_never_an_omnibox_command`
(`windows_app/tests.rs`).

Every accepted message MUST carry the per-WebView capability token. The token:

- comes from the operating-system CSPRNG (`BCryptGenRandom`);
- is generated per WebView;
- remains inside the closure of injected scripts and is never written to DOM,
  attributes or `window.name`;
- is compared in constant time;
- is sent through a reference to `window.chrome.webview.postMessage` captured
  at document-created time, with `JSON.stringify` captured at the same point;
- is unavailable to child frames: capability-bearing initialization scripts
  return when `window.top !== window` before the token declaration;
- is never carried in a navigation URL.

The native parser rejects messages above 8 KiB before JSON parsing. Column
indices are bounded; split URLs must be valid HTTP(S) targets and obvious
local/private/special targets are rejected before DNS. This WebView boundary
does not claim the Reader's pre-connect DNS-resolution filtering. Surface-specific
handlers accept only the actions meaningful for that WebView.
Actions tied to a comparator column must match the emitting column.

Remote navigation handlers reject the `neuralia:` scheme. The sole navigation
exception is the script-free Reader content: its own internal links may use
`neuralia:home` and `neuralia:web?...` without a capability. Host-injected
keyboard shortcuts in the Reader use the authenticated message channel.

Handlers on injected user controls MUST require `event.isTrusted`; synthesized
pointer and keyboard events are ignored. Automatic observers such as Gmail,
research-answer capture and the bounded agent observer are not user-event
handlers, but their messages still require the private per-WebView capability.
No injected handler acts on a page message instead of a user gesture: a ctrl+wheel
over a child frame does not zoom (the wheel does not cross the frame boundary),
and page `postMessage` traffic never reaches `act` (gate
`a_page_message_never_zooms_the_app_and_frames_forward_nothing`).

The floating omnibox (palette) is a native Win32 control, not an `<input>`
inside page DOM. A remote page can only request that it open through the
authenticated `palette` message; the text itself is typed and read natively.


New-window requests MUST NOT create a second WebView. Valid HTTP(S) targets are routed into the existing Full Web surface; other targets are denied.

New WebView permission requests are denied by default.

## Per-WebView hooks

Every WebView the app builds -- the three comparator columns, the split
(normal and private), the full Web surface and the agent's page, the Reader,
the PDF viewer, the EPUB pages, the Gemini Live panel, the hidden Gmail
monitor, the Ctrl+H panel and the service panels -- is born in two steps that
go through one module, `crates/neural-app/src/windows_app/webview_hooks.rs`:

- before `build`, `App::hooked_builder(builder, host, local_origin)` installs
  the host's navigation gate, the download refusal where the table says so,
  and the page-load notice (`WebViewEvent::PageLoaded { page, url }`, sent on
  `Finished` only), and returns a `HookedBuilder` that carries that host;
- `HookedBuilder::build_hooked` / `build_hooked_as_child` are the only calls
  of wry's `build` / `build_as_child` in the product (the gate holds them to
  the module), and after the build they register through WebView2 COM, for
  the same host, the NeuralIA items of the right-click menu
  (`WEBVIEW_MENU_ITEMS`: today only the auto-scroll item, on the columns and
  on the split, private included) and an `AcceleratorKeyPressed` handler that
  consults `accelerator_lookup` (`install_webview_hooks`, private to the
  module).

The host a birth site passes to `hooked_builder` is therefore the only host
that WebView has, in both halves: a site cannot build without the chain, nor
give one host's chain to the builder and another's menu to COM.

What each host receives is a pure table, `webview_hooks(host)`, with one slot
per feature: `menu`, `downloads` (`Managed` on the columns, the split, the
full Web and the service panels; `Deny` on the Reader, the PDF, the EPUB
pages, the Live panel, the Ctrl+H panel and the Gmail monitor),
`resource_gate`, `nav_gate`, `accelerators` (every host) and `distraction`
(empty). Navigation verdicts are the per-host chain `web_navigation_verdict`
(`Web`, `Pdf`, `Reader`, `Epub`, `Live`, `Gmail`, `SidePanel`, `Service`);
the gate `navigation_verdicts_are_the_ones_the_builders_gave` holds every
verdict and every event to the closures the builders had in 2.2.0, and
`spec_0108_remote_navigation_handlers_reject_neuralia_scheme` forbids any
`with_navigation_handler` outside the module. `every_webview_gets_the_hooks`
runs both halves over a recording registrar and builder for every host kind,
forbids wry's `build` / `build_as_child` outside the module, counts the 11
`build_hooked` calls against the 11 `hooked_builder` calls, and pins the
`hooked_builder` line of each of the 11 birth sites (the host literal next to
the builder it wraps, exactly once); `the_webview_hooks_table` pins the table.
The CI sabotage matrix proves it red with `reader-born-without-hooks` (the
Reader built by wry's `build` directly) and `gmail-monitor-gets-web-chain`
(the Gmail monitor born as `External`).

`accelerator_lookup` binds nothing yet (gate
`accelerator_lookup_binds_nothing_today`): the handler only sets `Handled`
when the lookup says so, so no keyboard behaviour changes until the command
registry fills it. The resource dispatcher `resource_gate_answers` is a stub
the product does not yet wire to `WebResourceRequested`; the gate
`custom_schemes_are_never_answered_by_the_resource_gate` already holds it to
never answering a request on `neuralia-pdf`, `neuralia-epub` or
`neuralia-live` (those are served by their wry custom protocols).
