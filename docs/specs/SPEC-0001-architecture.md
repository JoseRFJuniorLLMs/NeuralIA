# SPEC-0001 — Architecture

**Status:** Normative

```text
neural-app
   ├── native Windows shell
   ├── native Windows EDIT omnibox / palette
   ├── bounded Reader, history and memory workers
   ├── lazy system-WebView surfaces + bounded comparator
   ├── authenticated WebView2 message parser/dispatch
   └── GUI event routing
          │
          ▼
neural-core
   ├── intent + routing primitives
   ├── Reader HTTP/extractor + safe renderer
   ├── URL + resolved-IP policy
   ├── bounded local history
   ├── semantic memory + research sessions
   ├── deterministic local intelligence
   └── agent policy/runtime reference primitives
```

`neural-core` MUST NOT depend on UI, WRY, WebView2, Win32, or platform GUI
libraries. Dependency direction is one-way from app to core.

WebView lifecycle follows SPEC-0005. The native Home surface is drawn without a
surface WebView. Outside the comparator, at most one visible surface WebView is
active; the comparator is a bounded exception with at most three provider
surfaces. Returning Home destroys surface WebViews. The hidden Gmail monitor is
the sole deliberate Home exception after a Google session exists, and
`NEURALIA_NO_GMAIL=1` removes that exception for lifecycle measurement.

Reader work executes on one worker. At most one request is running and at most
one newer request is pending; replacing the pending request prevents unbounded
thread/request growth. Results re-enter through the GUI event loop and carry a
navigation generation; stale results are discarded.

History and memory persistence execute away from the UI thread through bounded
workers/channels so filesystem work does not become the GUI event loop.

Reader internal navigation may use the private `neuralia:` scheme because
Reader output is script-free trusted application content. Remote WebView
surfaces MUST reject `neuralia:` navigation. Their page-to-native control path
is the bounded authenticated WebView2 message channel from SPEC-0005/SPEC-0108:
closed action schema, message-size limit, exact argument validation and a
per-WebView capability. No remote page receives ambient filesystem, credential
or arbitrary native-call authority.
