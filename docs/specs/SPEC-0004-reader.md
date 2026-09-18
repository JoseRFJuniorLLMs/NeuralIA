# SPEC-0004 — Reader Engine

**Status:** Normative

Reader turns ordinary HTML documents into a low-noise representation without executing site JavaScript.

Defaults:
- total navigation deadline: 12 seconds across all redirects;
- decoded body limit: 2 MiB;
- redirect limit: 5;
- one active Reader worker plus one replaceable pending job.

Candidate containers include `article`, `main`, `[role=main]`, and common article/content classes. Candidates are ranked by text volume penalized by link text.

Output blocks: headings, paragraphs, quotes, code, and list items. Navigation, footer, aside, script, style, form and hidden containers are excluded.

`<pre>` blocks preserve line structure and indentation. List items render as semantic lists.

Extracted text is escaped before insertion into Reader HTML. Remote scripts and styles are not copied. Reader HTML executes no NeuralIA JavaScript and MUST use `script-src 'none'`.

Every Reader page MUST expose Home and an explicit full-page escape hatch through app-controlled navigation actions.
