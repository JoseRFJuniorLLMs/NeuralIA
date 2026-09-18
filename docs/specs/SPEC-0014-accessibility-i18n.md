# SPEC-0014 — Accessibility and Internationalization

**Status:** Normative

The default language is Brazilian Portuguese. Search URL generation accepts a language parameter instead of hard-coding language inside networking logic.

Reader treats content as Unicode and normalizes ordinary prose without byte slicing. Code/preformatted blocks preserve meaningful whitespace.

The Windows omnibox MUST use a native Windows `EDIT` control rather than a painted text editor. This delegates caret movement, selection, clipboard shortcuts, IME behavior and the base accessibility/UI Automation semantics to the platform.

Reader uses semantic headings, lists, quotes and code elements. Controls require visible text labels and sufficient contrast; essential state must not rely on color alone.

Future localization MUST use resources rather than duplicate application logic.
