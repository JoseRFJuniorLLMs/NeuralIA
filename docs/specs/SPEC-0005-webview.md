# SPEC-0005 — System WebView

**Status:** Normative

NeuralIA uses the platform web engine and does not distribute a full browser engine.

On Windows the adapter is WRY backed by Microsoft Edge WebView2.

## Lifecycle invariant

The native home screen MUST NOT instantiate WebView2.

A WebView is created only for:
- Google AI Mode;
- Reader HTML;
- explicit Full Web mode.

Only one WebView may exist at a time. Returning to Home drops the WebView and returns focus to the native window.

## IPC

Reader content may request Home or explicit opening of its original source. External web pages receive only a return-to-NeuralIA capability. They MUST NOT be allowed to trigger privileged Reader/native-network actions through IPC.

New-window behavior SHOULD reuse the current view or explicitly hand off to the system browser rather than multiply WebViews.
