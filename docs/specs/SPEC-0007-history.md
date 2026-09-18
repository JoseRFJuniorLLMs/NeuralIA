# SPEC-0007 — Local History

**Status:** Normative

History is local JSON Lines in the application data directory and is bounded by retention.

Each record contains a Unix timestamp, kind (Ask/Read/Web), original input, and resolved target. Ask records MUST NOT duplicate the query inside a Google URL target; the target may use a provider identifier such as `google-ai`.

Default retention is 250 entries. Appending beyond the limit rewrites only the retained tail while holding a file lock.

History persistence runs on a bounded background writer so `sync_data()` does not block the UI thread. History write failure MUST NOT prevent successful navigation. Corrupt individual lines are skipped during reads.

Ctrl+Shift+Delete clears local history.

Cloud synchronization is out of scope.
