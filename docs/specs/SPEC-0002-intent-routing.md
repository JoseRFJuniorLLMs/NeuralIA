# SPEC-0002 — Intent Routing

**Status:** Normative

The omnibox is an intent parser.

- ordinary text → `Compare`
- `? query`, `ask:query` → `Ask`
- `compare:`, `comparar:`, `!compare ` → `Compare`
- absolute HTTP(S) URL → `Read`
- domain-like token without whitespace → `Read`
- `reader:`, `read:`, `!read ` → `Read`
- `web:`, `!web ` → `Web`
- `home:`, `neural:home` → `Home`

Only HTTP and HTTPS are accepted for remote navigation. Embedded credentials are rejected. `file:`, `javascript:`, `data:`, `ftp:`, and arbitrary custom remote schemes are rejected.

Grammar changes are user-facing API changes and require tests.
