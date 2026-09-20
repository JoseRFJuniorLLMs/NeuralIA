# SPEC-0103 — Semantic Timeline

**Status:** Implementada — timeline JS embarcada, aceitação por fornecedor e rácio de performance cobertos em CI  
**Target:** NeuralIA 1.9

## 1. Purpose

The vertical NeuralIA response rail should evolve from a prettier scrollbar
into a **semantic map of the current conversation, page or research source**.

## 2. Marker types

The timeline MAY represent:

- user question;
- AI answer boundary;
- heading/section;
- code block;
- table;
- quoted source;
- opened external source;
- important conclusion;
- error/retry;
- user note.

Example:

```text
⌃
┃
━━  question
━   explanation
━━  code
━   source
━   comparison
━━  conclusion
┃
⌄
```

## 3. Extraction strategy

Use the cheapest trustworthy signal first:

1. known provider DOM semantics;
2. accessibility/landmark roles;
3. Reader semantic structure;
4. generic headings/articles/code/table detection;
5. proportional fallback.

A local model is optional for labeling or importance scoring. It is not needed
for basic operation.

## 4. Behavior

- Each provider panel has an independent timeline.
- Split View uses its own timeline.
- Clicking a marker scrolls to the corresponding semantic anchor.
- Auto-scroll uses the real scroll container and MAY advance marker-to-marker
  rather than a fixed viewport distance.
- The active marker updates as the user scrolls.
- Marker layout must remain useful after responsive resize.

## 5. Stability

Provider-specific selectors are adapters, not core logic.

A broken provider selector MUST fall back to generic semantic detection rather
than disabling navigation.

MutationObserver work MUST be coalesced and bounded.

## 6. Privacy

Timeline semantics are computed locally.

No page content is sent to a separate service merely to draw navigation
markers.

## 7. Accessibility

The timeline controls need accessible labels and keyboard equivalents.

A user must be able to navigate previous/next semantic section without a mouse.

## 7.1. Shipped implementation boundary

The product path is the JavaScript timeline injected by `neural-app`:
`SPLIT_SCROLL_RAIL_SCRIPT` and `COMPARATOR_INJECT_SCRIPT`, each with its own
`semanticAnchors()` implementation. The Rust helper
`neural_core::semantic_anchors_html` is useful as a parser/reference, but it is
**not** the implementation that draws the shipped rail.

For that reason, a test that only calls `semantic_anchors_html` is not an
acceptance gate for this specification. The product gate lives in
`crates/neural-app/tests/spec_product_wiring.rs` and checks the scripts that
actually ship: question/answer roles, headings, code/table/quote/source
selectors, deterministic proportional fallback, Reader/Split wiring and
coalesced animation-frame updates.

Acceptance is deliberately composite instead of pretending that the Rust
reference parser is the product. The `Semantic timeline gates` workflow:

- executes the **exact two `semanticAnchors()` functions extracted from
  `windows_app.rs`** against ChatGPT, Gemini and Claude fixtures;
- ratio-gates those shipped functions with paired 16 -> 64 node measurements;
- runs `timeline_acceptance.rs` against the Rust parser/reference with the
  same provider vocabulary and a paired 256 -> 1024 section ratio;
- proves all new gates with deliberate role and superlinear-work mutations,
  restores the source, and reruns green.

The existing product-wiring gate continues to prove that Reader/Split and the
three comparator columns actually receive the shipped timeline scripts.

## 8. Acceptance criteria

1. ChatGPT, Gemini and Claude each expose independent active markers;
2. Reader headings/code/table blocks create meaningful anchors;
3. Split View uses the same mechanism;
4. generic pages fall back cleanly;
5. timeline work remains bounded and approximately linear by paired-ratio CI gates; visible layout updates stay coalesced behind `requestAnimationFrame`;
6. no native scrollbar is required for normal NeuralIA navigation.


## 9. Acceptance evidence

The acceptance gate is `.github/workflows/semantic-timeline.yml` plus
`crates/neural-core/tests/timeline_acceptance.rs`.

No absolute millisecond budget is normative. Performance is evaluated as a
ratio inside the same CI round, because runner speed is not an algorithmic
property. For a 4x input increase the gate allows less than 8x work; a
deliberate O(n^2) mutation must turn the gate red.
