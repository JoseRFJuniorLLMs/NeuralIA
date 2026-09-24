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
- remote page-to-native control uses a bounded WebView2 message channel with no ambient authority: a closed 31-action schema (SPEC-0005), 8 KiB envelope ceiling, exact argument validation and a per-WebView capability drawn from `BCryptGenRandom` and compared in constant time; `postMessage` and `JSON.stringify` are captured at document-created time, and the capability never enters a navigation URL, closing the Navigation API token leak; remote handlers reject `neuralia:` navigation, while the script-free Reader keeps only its internal links; injected user-event handlers require `isTrusted`; the floating palette remains a native Win32 control whose text is never exposed to the page;
- bounded WebView lifecycle: one visible surface outside the comparator, a hard ceiling of three provider surfaces inside it, and the optional hidden Gmail monitor accounted for separately; lifecycle return-to-Home is regression-tested;
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
