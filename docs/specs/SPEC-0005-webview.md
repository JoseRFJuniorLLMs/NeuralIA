# SPEC-0005 — System WebView

**Status:** Normative

NeuralIA uses the platform web engine and does not distribute a full browser engine.

On Windows the adapter is WRY backed by Microsoft Edge WebView2. A single WebView instance is the v0.1 ceiling.

v0.1 creates one WebView at startup. v0.2 SHOULD create it lazily after the first operation that needs HTML/web content and MAY suspend or destroy it after an idle threshold only when benchmarks show a material benefit.

External pages receive a small NeuralIA return affordance. Future target=_blank/window.open behavior SHOULD reuse the current view or explicitly hand off to the system browser rather than multiply WebViews.
