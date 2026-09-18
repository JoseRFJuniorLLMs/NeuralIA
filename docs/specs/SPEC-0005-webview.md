# SPEC-0005 — System WebView

**Status:** Normative

NeuralIA uses the platform web engine and does not distribute a full browser engine.

On Windows the adapter is WRY backed by Microsoft Edge WebView2.

## Lifecycle invariant

The native home screen MUST NOT instantiate WebView2.

A WebView is created only for:
- Google AI Mode;
- Reader HTML;
- explicit Full Web mode;
- the comparator, which is the single bounded exception below.

Outside the comparator, only one WebView may exist at a time.

The comparator is the one bounded exception: ordinary questions (and explicit `compare:` requests) create one WebView per provider, at most `COMPARATOR_COLUMNS` (3). Each column owns an independent response timeline.

Whatever the surface, returning to Home MUST destroy every live WebView, comparator included, and return focus to the native window. A single exit path (`destroy_web_surfaces`) owns this, and every surface transition goes through it, so entering a new surface can never leave the previous one alive.

## Native bridge boundary

Reader and external pages receive no NeuralIA IPC object. Remote pages receive no ambient authority through the intercepted `neuralia:` scheme. Injected controls use a per-WebView unguessable capability carried only in their closure; unsigned page-initiated actions are rejected.

Reader buttons navigate to the internal `neuralia:` action scheme, which is intercepted before navigation. Comparator and Full Web pages receive small injected navigation controls, but no object bridge or filesystem/native API.

New-window requests MUST NOT create a second WebView. Valid HTTP(S) targets are routed into the existing Full Web surface; other targets are denied.

New WebView permission requests are denied by default.
