# SPEC-0015 — Threat Model

**Status:** Normative

## Protected assets

User credentials in system WebView, browsing/search history, local filesystem, network identity, and application integrity.

## Adversaries

Malicious sites, malformed HTML, hostile redirects, oversized responses, compromised dependencies, and accidental secret commits.

## Controls

- scheme allowlist;
- rejection of embedded URL credentials;
- bounded redirects/time/body;
- Reader converts untrusted HTML to escaped text blocks;
- no local-file scheme;
- no site JavaScript in Reader;
- least-privilege GitHub Actions.

## Accepted boundary

Full Web mode executes arbitrary websites inside the system WebView sandbox. NeuralIA does not claim to make hostile full-web content harmless; it minimizes how often that surface is necessary.
