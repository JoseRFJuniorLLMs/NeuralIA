# SPEC-0008 — Performance Budgets

**Status:** Normative

Product targets and CI regression ceilings are intentionally distinct. Product targets describe the desired user experience on a defined reference PC. CI ceilings exist to catch architectural regressions without pretending that a shared hosted runner is a laboratory instrument.

| Metric | product target | CI regression ceiling |
|---|---:|---:|
| time to native shell | < 200 ms | < 1000 ms |
| idle RAM before WebView | < 50 MiB | < 64 MiB |
| idle threads | minimal | <= 16 |
| idle CPU | ~0% | no sustained background work |
| background network while Home | 0 requests | 0 requests |
| WebView instances on Home | 0 | 0 |
| WebView instances while browsing | <= 1 | <= 1 |
| WebView instances in the opt-in comparator | <= 3 | <= 3 |
| Reader decoded HTML | <= 2 MiB | <= 2 MiB |
| Reader extraction | < 100 ms for 2 MiB fixture | tested independently |
| local LLM resident memory | 0 | 0 |

The Windows CI launches the release binary with `NEURALIA_NO_STARTUP=1` so the measured Home is genuinely idle, waits for the native Home window, records startup latency, working set, thread count and binary size, uploads the JSON measurement, and fails on regression ceilings.

Startup measurement alone cannot observe a lifecycle leak, so a second gate (`scripts/measure-cycles.ps1`) drives comparator -> Home cycles and counts the `msedgewebview2.exe` processes descending from the app. Any WebView surviving the return to Home fails the build, as does working-set growth across cycles.

The stricter product targets are not published as achieved values until measurements are recorded on a defined reference machine.
