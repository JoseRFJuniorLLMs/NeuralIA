# SPEC-0007 — Local History

**Status:** Normative

History is append-only JSON Lines in the application data directory.

Each record contains a Unix timestamp, kind (Ask/Read/Web), original input, and resolved target.

History write failure MUST NOT prevent successful navigation. Corrupt individual lines are skipped during reads.

The default UI history limit is 250 entries. Cloud synchronization is out of scope.
