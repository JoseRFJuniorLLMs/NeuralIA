# SPEC-0001 — Architecture

**Status:** Normative

```text
neural-app
   ├── home / omnibox shell
   ├── system WebView adapter
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

Windows full-web rendering uses the operating-system WebView2 runtime through WRY. The repository MUST NOT vendor Chromium binaries.

Blocking Reader work runs away from the UI thread. Results re-enter through the GUI event loop.

v0.1 uses one system WebView for shell and content. v0.2 SHOULD move the home/omnibox shell to native controls and create the WebView lazily, without changing `neural-core`.
