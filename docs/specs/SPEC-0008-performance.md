# SPEC-0008 — Performance Budgets

**Status:** Normative

Budgets are release targets, not claims until benchmarked.

| Metric | v0.2 target |
|---|---:|
| time to native shell | < 200 ms |
| idle RAM before WebView | < 50 MiB |
| idle CPU | ~0% |
| background network | 0 requests |
| WebView instances | <= 1 |
| Reader decoded HTML | <= 2 MiB |
| Reader extraction | < 100 ms for 2 MiB fixture on reference PC |
| local LLM resident memory | 0 |

v0.1 intentionally creates the system WebView at startup, so the pre-WebView memory target becomes enforceable only after the native-shell phase.

Features exceeding a budget require a written spec amendment and benchmark evidence.
