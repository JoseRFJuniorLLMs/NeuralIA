# SPEC-0004 — Reader Engine

**Status:** Normative

Reader turns ordinary HTML documents into a low-noise representation without executing site JavaScript.

Defaults:
- total request timeout: 12 seconds;
- decoded body limit: 2 MiB;
- redirect limit: 5.

Candidate containers include `article`, `main`, `[role=main]`, and common article/content classes. Candidates are ranked by text volume penalized by link text.

Output blocks: headings, paragraphs, quotes, code, and list items. Navigation, footer, aside, script, style, and form containers are excluded.

Extracted text is escaped before insertion into Reader HTML. Remote scripts and styles are not copied.

Every Reader page MUST expose an explicit full-page escape hatch.
