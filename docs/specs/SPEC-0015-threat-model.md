# SPEC-0015 — Threat Model

**Status:** Normative

## Protected assets

User credentials in system WebView, browsing/search history, local filesystem, local-network services, network identity, release integrity and application integrity.

## Adversaries

Malicious sites, malformed HTML, hostile redirects, DNS rebinding/private-network pivots, oversized/slow responses, compromised dependencies/actions, and accidental secret commits.

## Controls

- HTTP(S) scheme allowlist;
- rejection of embedded URL credentials and local-file schemes;
- navigation-wide deadline, bounded redirects/body/concurrency;
- public DNS resolution rejects local/private/reserved destinations before connection;
- Reader converts untrusted HTML to escaped text blocks;
- no site JavaScript in Reader;
- no raw IPC exposed to Reader or external web pages;
- per-WebView random capability token + trusted-event check for injected native actions;
- public external/comparator/PDF navigation rejects syntactically local/private pivots;
- one-WebView ceiling outside the bounded three-column comparator;
- bounded local history with local clear operation;
- locked dependencies;
- SHA-pinned GitHub Actions;
- separate least-privilege release tag/build/publish jobs;
- stable release assets are never overwritten;
- build provenance attestation.

## Accepted boundary

Full Web mode executes arbitrary websites inside the system WebView sandbox. NeuralIA does not claim to make hostile full-web content harmless; it minimizes how often that surface is necessary.

Direct user navigation to an explicitly local HTTP(S) destination is a user-authorized operation and remains distinct from a public page pivoting into the local network. Reader additionally enforces public DNS resolution against private/reserved addresses. Full Web uses the system WebView network stack, so DNS-level Private Network Access enforcement beyond syntactically local targets remains a platform boundary rather than a NeuralIA claim.
