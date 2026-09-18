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
- Ctrl+L selects omnibox content.
- Ctrl+H opens the recent local-history viewer.
- Ctrl+Shift+Delete clears local history.
- Escape clears/returns Home from native or Web content.

Controls require visible text/accessible labels; essential state must not rely on color alone.
