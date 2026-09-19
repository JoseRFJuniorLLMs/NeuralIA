# SPEC-0008 — Performance Budgets

**Status:** Normative

Product targets and CI regression ceilings are intentionally distinct. Product targets describe the desired user experience on a defined reference PC. CI ceilings exist to catch architectural regressions without pretending that a shared hosted runner is a laboratory instrument.

| Metric | product target | CI regression ceiling |
|---|---:|---:|
| time to native shell | < 200 ms | < 1000 ms |
| idle RAM before WebView | < 50 MiB | < 64 MiB |
| idle threads | minimal | <= 16 |
| native Home animation CPU | low single-digit | bounded ~15 FPS, no WebView/network |
| idle CPU on Home (visible, focused) | low single-digit | <= 25% of one core |
| Home animation when minimized/occluded | 0 frames | 0 frames |
| background network while Home | 0 requests (except the Gmail monitor) | 0 requests (except the Gmail monitor) |
| WebView instances on Home | 0 (+1 Gmail monitor when a Google session exists) | 0 (+1 Gmail monitor when a Google session exists) |
| WebView instances while browsing | <= 1 | <= 1 |
| WebView instances in the comparator | <= 3 (+1 Gmail monitor, +1 Split View) | <= 3 (+1 Gmail monitor, +1 Split View) |
| Reader decoded HTML | <= 2 MiB | <= 2 MiB |
| Reader extraction | < 100 ms for 2 MiB fixture | hostile nested-container fixture must complete in bounded time; extraction honours the navigation deadline and cancellation |
| local LLM resident memory | 0 | 0 |

The Windows CI launches the release binary with `NEURALIA_NO_STARTUP=1` so no provider query is sent; the default native Home animation remains enabled during measurement, waits for the native Home window, records startup latency, working set, thread count and binary size, samples idle CPU over a three-second window with the Home window visible, uploads the JSON measurement, and fails on regression ceilings.

The idle CPU sample is reported as a percentage of one core. The Home animation runs at about 15 FPS while the window is visible and focused, and stops entirely when the window is minimized or occluded, so the measured window is the worst case. The CI ceiling is deliberately generous because a shared hosted runner has scheduling noise and software rendering; the product target stays low single-digit.

Startup measurement alone cannot observe a lifecycle leak, so a second gate (`scripts/measure-cycles.ps1`) drives comparator -> Home cycles and counts the `msedgewebview2.exe` processes descending from the app. That gate sets `NEURALIA_NO_GMAIL=1`, because the hidden Gmail monitor is an intentional exception to "zero WebViews on Home" (SPEC-0005) and counting processes cannot tell intent from leak. The first cycle must prove WebView creation; subsequent cycles account for WebView2 process reuse. Any observable WebView surviving Home or excessive working-set growth fails the build.

The stricter product targets are not published as achieved values until measurements are recorded on a defined reference machine.
