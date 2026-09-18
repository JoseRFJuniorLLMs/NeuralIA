# SPEC-0008 — Performance Budgets

**Status:** Normative

Budgets are release targets, not claims until benchmarked.

| Metric | target |
|---|---:|
| time to native shell | < 200 ms |
| idle RAM before WebView | < 50 MiB |
| idle CPU | ~0% |
| background network while Home | 0 requests |
| WebView instances on Home | 0 |
| WebView instances while browsing | <= 1 |
| Reader decoded HTML | <= 2 MiB |
| Reader extraction | < 100 ms for 2 MiB fixture on reference PC |
| local LLM resident memory | 0 |

The implementation now satisfies the architectural prerequisite for idle-memory measurement: the Windows Home surface is native and WebView2 is lazy.

Numbers are not published as achieved values until a benchmark workflow records them on a defined reference machine.
