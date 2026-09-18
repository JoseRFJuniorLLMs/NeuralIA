# SPEC-0003 — Google AI Mode Integration

**Status:** Normative

NeuralIA uses Google's public search AI Mode surface rather than a paid model API in the default build.

Queries are URL-encoded and routed to:

```text
https://www.google.com/search?q=<query>&udm=50&hl=<language>
```

No AI API key is stored by NeuralIA.

## Query fan-out boundary

A plain natural-language question is a deliberate three-provider fan-out: the
same query is sent to Google AI Mode, ChatGPT and Claude in three bounded
WebViews. The UI MUST expose those three panels so this network behavior is
visible to the user.

`ask:<query>` and `?<query>` are the single-provider escape hatch and send the
query only to Google AI Mode. `compare:<query>` remains an explicit alias for
the default comparator behavior.

Google authentication, availability, cookies, consent, regional eligibility, and account state belong to Google and the system WebView profile. NeuralIA MUST NOT scrape passwords or promise universal AI Mode availability.

If Google changes this route, NeuralIA SHOULD expose an explicit fallback in a future spec. It MUST NOT silently switch to a paid provider.
