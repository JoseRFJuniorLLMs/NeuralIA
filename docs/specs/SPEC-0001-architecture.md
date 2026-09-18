# SPEC-0001 — Architecture

**Status:** Normative

```text
neural-app
   ├── native Windows home/omnibox
   ├── lazy system WebView adapter
   ├── lightweight GDI presentation
   └── event routing
          │
          ▼
neural-core
   ├── intent parser
   ├── Google AI URL builder
   ├── Reader HTTP/extractor
   ├── safe HTML renderer
   ├── URL policy
   └── append-only history
```

`neural-core` MUST NOT depend on UI, WRY, WebView2, Win32, or platform GUI libraries. Dependency direction is one-way from app to core.

The Windows home surface is drawn natively and does not create WebView2. AI, Reader, and Full Web create at most one system WebView on demand. Returning Home destroys that WebView.

Blocking Reader work runs away from the UI thread. Results re-enter through the GUI event loop and carry a navigation generation; stale results are discarded.

External pages have an intentionally reduced IPC capability: they may return Home, but may not command Reader or arbitrary native navigation.
