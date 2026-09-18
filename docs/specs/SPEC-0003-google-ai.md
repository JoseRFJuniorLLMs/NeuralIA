# SPEC-0003 — Google AI Mode Integration

**Status:** Normative

NeuralIA uses Google's public search AI Mode surface rather than a paid model API in the default build.

Queries are URL-encoded and routed to:

```text
https://www.google.com/search?q=<query>&udm=50&hl=<language>
```

No AI API key is stored by NeuralIA.

Google authentication, availability, cookies, consent, regional eligibility, and account state belong to Google and the system WebView profile. NeuralIA MUST NOT scrape passwords or promise universal AI Mode availability.

If Google changes this route, NeuralIA SHOULD expose an explicit fallback in a future spec. It MUST NOT silently switch to a paid provider.
