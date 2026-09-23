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

The closed action set has 29 names: `home`, `back`, `restore`,
`autoscroll`, `zoomin`, `zoomout`, `zoomreset`, `reload`, `print`,
`omnibox`, `history`, `clearhistory`, `fullscreen`, `devtools`,
`viewsource`, `newtab`, `expand`, `shortcut-expand`, `minimize`, `split`,
`link`, `ask`, `search`, `split-close`, `split-expand`, `palette`,
`gmail-state`, `research-answer` and `agent-observation`. Unknown actions,
extra fields, wrong types, oversized messages and invalid per-action arguments
are rejected.
The parser gate `protocol_accepts_exactly_the_twenty_nine_published_actions`
(`crates/neural-app/src/ipc.rs`) reads this list and fails when it differs from
the set the shipped parser accepts.

`link` reports a click on a link inside a comparator column. Its arguments are
exactly `col`, `url` and `aside` (boolean); `url` goes through the same
validation and local-network rejection as `split`. With `aside=true` the URL
opens in the Split panel beside the emitting column. With `aside=false` it
navigates all three comparator columns to that URL. It is the one action whose
effect reaches columns other than the one that emitted it (gate:
`a_plain_click_opens_in_all_three_panels_and_ctrl_click_opens_beside` in
`crates/neural-app/src/windows_app.rs`; argument policy:
`a_link_click_obeys_the_same_policy_as_the_split` in `ipc.rs`).

`ask` reports a question the user typed and sent in a comparator column (Enter
or the send button, trusted events only, on the column's own AI page). Its
arguments are exactly `col` and `text` (1..=2000 characters after trimming,
no control characters except newline and tab). The OTHER columns load the same
question in their own provider; the emitting column is not touched. Gates:
`ask_carries_the_typed_question_within_bounds` (`ipc.rs`) and
`a_question_typed_in_one_column_goes_to_the_others` (`windows_app.rs`).

`search` reports the "Pesquisar" button of the selection toolbar. The toolbar
is part of the keyboard-shortcut script, so it exists in every WebView that
receives that script: the comparator columns, the Split panel, external web,
the Reader and the PDF viewer (top frame only). It appears after a trusted
mouse or keyboard selection of 1..=5000 characters outside editable fields,
lives in a closed shadow root and offers Pesquisar, Copiar (clipboard, no IPC)
and Falar (local `speechSynthesis` voices, no IPC). `search` carries exactly
`text` (1..=2000 characters after trimming, no control characters except
newline and tab); a longer selection is not sent and the toolbar says so. The
native side opens the normal three-AI comparison with the text as the
question, directly: it never goes through the omnibox command parser, so a
selected `agent:` or `tema:` is a question, not a command. A private Split has
no Pesquisar button and its handler refuses `search`. Gates:
`search_carries_the_selected_text_within_bounds` (`ipc.rs`),
`the_selection_toolbar_offers_three_actions_for_a_trusted_selection`,
`the_selection_toolbar_searches_only_what_fits_and_never_from_private`,
`a_selected_search_reaches_the_comparator_from_every_surface_but_the_private_split`
and `a_selected_search_is_a_question_never_an_omnibox_command`
(`windows_app.rs`).

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

The floating omnibox (palette) is a native Win32 control, not an `<input>`
inside page DOM. A remote page can only request that it open through the
authenticated `palette` message; the text itself is typed and read natively.


New-window requests MUST NOT create a second WebView. Valid HTTP(S) targets are routed into the existing Full Web surface; other targets are denied.

New WebView permission requests are denied by default.
