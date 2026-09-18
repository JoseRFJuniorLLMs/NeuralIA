# SPEC-0000 — Product Charter

**Status:** Normative  
**Version:** 1.1.3

## Thesis

NeuralIA is an AI-first information client that can temporarily become a browser. Its default job is to help a user ask a question or read a document, not execute an arbitrary web application.

## Primary flow

1. Natural-language input routes to the bounded three-provider comparator; `ask:` / `?` selects one provider.
2. HTTP(S) URLs route to Reader.
3. Full web rendering is explicit through `web:` or the Reader escape hatch.
4. The user can always return to the NeuralIA home surface.

## Non-goals

The default distribution MUST NOT ship Chromium/CEF/Electron, a NeuralIA JavaScript engine, a NeuralIA CSS compatibility engine, a local LLM, browser extensions, cloud profile sync, background agents, or speculative prefetch.

## Product invariant

Any feature that materially increases idle memory, startup latency, resident background activity, network authority, or web-platform scope MUST justify itself against SPEC-0008 and SPEC-0015 before merge.

## v1 baseline

A Windows user can launch NeuralIA into an idle native Home, use a native omnibox, submit an AI query, read a normal article without executing site JavaScript, explicitly open the full page, return home, retain bounded local history, clear that history, and receive stable releases produced only from CI-green source.
