# SPEC-0009 — UI/UX

**Status:** Normative

The official identity is the supplied NeuralIA mark: white neural symbol on a black rounded square. The product UI is monochrome, spacious, text-first, and deliberately avoids dashboard clutter.

## Home

The Home surface contains the logo, product name/tagline, one native Windows omnibox, one primary action, explicit IA/Reader/Web actions, concise grammar help, and local status text.

The omnibox MUST be a platform text-edit control rather than a painted text editor. Selection, caret movement, clipboard operations, IME input, Home/End/Delete, and ordinary text-edit shortcuts therefore follow Windows behavior.

## Reader

Reader prioritizes comfortable line length, typography, semantic headings/lists, quotes, and whitespace-preserving code. It always exposes Home and Open full page.

## Keyboard

- Enter submits the omnibox.
Keyboard focus always lives in a child window (the native omnibox or a WebView2),
so the winit window never receives key events. Every shortcut MUST therefore be
delivered through one of two channels: the omnibox subclass (Home) or the keymap
injected into every page in the capture phase (Reader, Full Web, comparator),
which routes through the `neuralia:` action scheme. A shortcut that exists in
only one channel is a bug, not a limitation.

- Ctrl+L returns to the omnibox with its text selected.
- Ctrl+H opens the recent local-history viewer.
- Ctrl+Shift+Delete clears local history.
- Escape goes back one level: fullscreen column → three columns → Home.
- Backspace / Alt+Left / Alt+Right walk the page history.
- Ctrl+R and F5 reload; Ctrl + / - / 0 zoom on Chrome's ladder; Ctrl+F opens an
  in-page find bar; Ctrl+P prints; F12 opens DevTools; Ctrl+U shows the source.
- F11 toggles fullscreen for the current comparator column; F8 toggles auto-scroll;
  1/2/3 expand a column and 0 restores.
- Backspace and the digits are ignored while typing in a field.

## Auto-scroll

Auto-scroll is opt-in per session: opening a document asks Sim/Não at the bottom
centre, and nothing moves without an answer (an ignored prompt counts as Não).
The interval is 30 seconds from the first advance. A single document is advanced
with a synthesized Page Down so that HTML, plain text and the Edge PDF viewer all
behave the same; the synthesized key is only sent while the NeuralIA window is in
the foreground.

Controls require visible text/accessible labels; essential state must not rely on color alone.
