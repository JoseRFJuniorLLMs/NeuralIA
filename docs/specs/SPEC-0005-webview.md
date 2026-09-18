# SPEC-0005 — System WebView

**Status:** Normative

NeuralIA uses the platform web engine and does not distribute a full browser engine.

On Windows the adapter is WRY backed by Microsoft Edge WebView2.

## Lifecycle invariant

The native home screen MUST NOT instantiate WebView2.

A WebView is created only for:
- Google AI Mode when selected through `ask:` / `?`;
- Reader HTML;
- explicit Full Web mode;
- the default three-provider comparator, which is the single bounded exception below.

Outside the comparator, only one WebView may exist at a time.

The comparator is the one bounded exception: ordinary text (and the explicit `compare:` alias) creates one WebView per provider, exactly three with the current provider set and never more than `COMPARATOR_COLUMNS` (3).

Whatever the surface, returning to Home MUST destroy every live WebView, comparator included, and return focus to the native window. A single exit path (`destroy_web_surfaces`) owns this, and every surface transition goes through it, so entering a new surface can never leave the previous one alive.

## Native bridge boundary

Reader and external pages receive no NeuralIA IPC object.

Reader buttons are static, script-free links intercepted before navigation.
Scripts injected into external/PDF/comparator WebViews do not get an ambient
native bridge: each WebView receives a random per-instance capability token kept
inside the initialization-script closure, native actions require that token, and
keyboard/button handlers reject synthetic events with `isTrusted == false`.
Public Full Web/comparator navigation also rejects syntactically local/private
targets unless the user explicitly opened a local destination as the initial
Full Web URL.

New-window requests MUST NOT create a second WebView. Valid HTTP(S) targets are routed into the existing Full Web surface; other targets are denied.

New WebView permission requests are denied by default.
