# SPEC-0014 — Accessibility and Internationalization

**Status:** Normative

The default language is Brazilian Portuguese. Search URL generation accepts a language parameter instead of hard-coding language inside networking logic.

Reader treats content as Unicode and normalizes whitespace without byte slicing.

UI requirements include keyboard-operable actions, visible focus, semantic headings, zoomable system-WebView content, sufficient contrast, and text/accessible labels for icons.

Future localization MUST use resources rather than duplicate application logic.
