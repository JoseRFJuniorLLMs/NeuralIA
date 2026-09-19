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

There is no IPC object. Reader, comparator and Full Web pages receive no bridge object, no filesystem access and no native API. The only page-to-native channel is the internal `neuralia:` scheme, which is intercepted before navigation and translated into a fixed list of UI events.

The accepted actions are exactly: `home`, `back`, `restore`, `autoscroll`, `zoomin`, `zoomout`, `zoomreset`, `reload`, `print`, `omnibox`, `history`, `clearhistory`, `fullscreen`, `devtools`, `viewsource`, `newtab`, `expand`, `minimize`, `split`, `split-close`, `split-expand`, `palette` and `gmail-state`. Any other name is rejected. Every one of them drives the application's own user interface: none reads files, reaches the local network, or touches credentials, and the only destructive one (`clearhistory`) erases local history and nothing else.

Every action MUST carry a per-WebView capability token, and the native side MUST reject an action whose token is absent or wrong. The token:

- comes from the operating-system CSPRNG (`BCryptGenRandom`), never from a clock, a hash of a clock, or a process-local PRNG;
- is generated per WebView, so a token leaked from one surface is useless in another;
- exists only inside the closure of the injected scripts, and is never written to the DOM, to a global, to an attribute or to `window.name`;
- is compared in constant time, so a page cannot recover it byte by byte through timing;
- is carried using native functions (`encodeURIComponent` and the rest) captured at document-created time, before page script runs, so poisoning globals neither steals the token nor corrupts the URL that carries it.

Handlers on injected controls MUST require `event.isTrusted`, for pointer and keyboard events alike; synthesized events are ignored. A page may therefore *ask* for a UI action only by clicking the control NeuralIA injected, never by dispatching one.

The floating omnibox (palette) is a native Win32 control, not an `<input>` injected into the page's DOM. Through `neuralia:palette` a page may only request that the palette open; the text is typed into the native control and read from it natively, so a page can neither read nor submit what the user types.

New-window requests MUST NOT create a second WebView. Valid HTTP(S) targets are routed into the existing Full Web surface; other targets are denied.

New WebView permission requests are denied by default.
