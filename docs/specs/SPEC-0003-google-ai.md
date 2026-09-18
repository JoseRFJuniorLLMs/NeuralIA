# SPEC-0003 — Google AI Mode Integration

**Status:** Normative

NeuralIA uses Google's public search AI Mode surface rather than a paid model API in the default build.

Queries are URL-encoded and routed to:

```text
https://www.google.com/search?q=<query>&udm=50&hl=<language>
```

No AI API key is stored by NeuralIA.

## Query fan-out boundary

A plain natural-language question goes to Google AI Mode and nowhere else. NeuralIA MUST NOT send it to any other provider by default.

The comparator is opt-in and explicit: only `compare:<query>` (or the Comparar button) sends the same question to Google AI Mode, ChatGPT and Claude at once. The user chooses that per query; there is no setting that makes it the default, and the input grammar states the fan-out.

Google authentication, availability, cookies, consent, regional eligibility, and account state belong to Google and the system WebView profile. NeuralIA MUST NOT scrape passwords or promise universal AI Mode availability.

If Google changes this route, NeuralIA SHOULD expose an explicit fallback in a future spec. It MUST NOT silently switch to a paid provider.
