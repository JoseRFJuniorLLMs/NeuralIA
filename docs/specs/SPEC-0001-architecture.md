# SPEC-0001 — Architecture

**Status:** Normative

```text
neural-app
   ├── native Windows shell
   ├── native Windows EDIT omnibox
   ├── one coalescing Reader worker
   ├── asynchronous bounded-history writer
   ├── lazy system WebView adapter
   └── GUI event routing
          │
          ▼
neural-core
   ├── intent parser
   ├── Google AI URL builder
   ├── Reader HTTP/extractor
   ├── safe HTML renderer
   ├── URL + resolved-IP policy
   └── bounded local history
```

`neural-core` MUST NOT depend on UI, WRY, WebView2, Win32, or platform GUI libraries. Dependency direction is one-way from app to core.

The Windows home surface does not create WebView2. AI, Reader, and Full Web create at most one system WebView on demand. Returning Home destroys that WebView.

Reader work executes on one worker. At most one request is running and at most one newer request is pending; replacing the pending request prevents unbounded thread/request growth. Results re-enter through the GUI event loop and carry a navigation generation; stale results are discarded.

History persistence executes away from the UI thread through a bounded channel.

Reader navigation actions use a private `neuralia:` URL intercepted by the app. Reader and external pages receive no NeuralIA IPC object. External pages may return Home only through the controlled navigation scheme.
