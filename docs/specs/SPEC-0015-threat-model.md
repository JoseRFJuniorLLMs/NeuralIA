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
- no object IPC exposed to Reader or external web pages; remote `neuralia:` navigation is limited to a fixed list of user-interface actions, each requiring a per-WebView capability token drawn from the operating-system CSPRNG (`BCryptGenRandom`), held only inside the closure of the injected scripts, carried by native functions captured at document-created time so poisoned globals cannot steal it, and compared in constant time; every injected handler requires a trusted event, for pointer and keyboard alike, so synthetic clicks and key presses are ignored; the floating palette is a native Win32 control rather than an input injected into the page, so a page can request that it open but can never read or submit its text;
- one-WebView ceiling;
- bounded local history with local clear operation;
- locked dependencies;
- SHA-pinned GitHub Actions;
- separate least-privilege release tag/build/publish jobs;
- stable release assets are never overwritten;
- build provenance attestation.

## Accepted boundary

Full Web mode executes arbitrary websites inside the system WebView sandbox. NeuralIA does not claim to make hostile full-web content harmless; it minimizes how often that surface is necessary.

Direct user navigation to an explicitly local HTTP(S) destination is a user-authorized operation and remains distinct from a public page pivoting into the local network. A Full Web surface that starts on a public URL and every comparator column reject obvious local/private navigation targets.

The native palette is user input, not page input: what the user types into a Win32 control never passed through the page, so an explicitly local destination typed there is authorized exactly like one typed into the native omnibox. A page can only ask for the palette to open; it cannot put a destination in it.
