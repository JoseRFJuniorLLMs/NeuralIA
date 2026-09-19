# SPEC-0101 — Research Sessions and Cross-Source Synthesis

**Status:** Proposed  
**Target:** NeuralIA 1.8

## 1. Purpose

The unit of work in NeuralIA should become a **research session**, not an
accidental pile of tabs.

A session connects:

- the user question;
- Gemini, ChatGPT and Claude responses;
- sources opened from those responses;
- Reader/PDF documents;
- notes;
- later synthesis.

## 2. Session model

```text
Research Session
├── question / intent
├── Gemini
│   ├── answer
│   └── sources
├── ChatGPT
│   ├── answer
│   └── sources
├── Claude
│   ├── answer
│   └── sources
├── Reader/PDF sources
├── saved notes
└── synthesis snapshots
```

The existing provider-grouped tabs SHOULD feed this model rather than being
replaced by a generic Chrome-like tab strip.

## 3. Required capabilities

### 3.1 Create session

A normal three-provider query MAY automatically start a lightweight research
session.

The user can also explicitly create or rename one.

### 3.2 Attach source

Opening a source from a provider associates it with:

- session ID;
- provider of origin;
- canonical URL;
- title;
- capture timestamp.

### 3.3 Multi-source selection

The user can select multiple session items and request:

- compare;
- summarize;
- find disagreements;
- extract common facts;
- build a technical table;
- produce a source-backed outline.

### 3.4 Synthesis artifact

A synthesis is stored as a snapshot containing:

- selected source IDs;
- generated output;
- provider/model used;
- timestamp;
- citations/provenance.

It MUST be reproducible enough to show which sources were used even if the
generation model changes later.

## 4. Cross-source comparison

The comparison engine SHOULD create a structured intermediate representation
before prose generation.

Example:

```text
SourceFacts
├── source_id
├── claims[]
├── named_entities[]
├── numbers[]
├── dates[]
└── technical_attributes{}
```

For product/technical comparisons this enables deterministic tables instead of
asking a model to rediscover every field from raw pages on every request.

## 5. UI

Research sessions SHOULD remain visually lightweight.

Recommended interaction:

- current provider tabs stay in the native title bar;
- a session indicator/name is compact;
- a command palette action opens session contents;
- sources can be checked for synthesis;
- no permanent dashboard is required.

The product should not become a project-management app wearing a browser icon.

## 6. Semantic grouping

When SPEC-0100 is available, sources MAY be clustered by semantic similarity
and intent.

Suggested labels can be generated locally, but the user remains able to rename
or merge groups.

Automatic grouping MUST NOT move private items into persistent sessions.

## 7. Export

A session MAY export to Markdown containing:

- session title/question;
- selected notes;
- synthesis;
- source list with URLs;
- optional quoted excerpts within safe limits.

PDF/HTML export can follow later.

## 8. Performance and storage

- Session metadata is small and local.
- Source content SHOULD reference semantic-memory documents rather than copy
  entire page bodies.
- Opening or closing tabs MUST NOT synchronously rewrite the whole session.
- Session persistence uses the same off-UI-thread discipline as history.

## 9. Acceptance criteria

1. a three-provider query creates or joins a session;
2. sources retain their provider-of-origin relationship;
3. at least five sources can be selected and compared;
4. synthesis includes source provenance;
5. closing visual tabs does not destroy the session;
6. private sources never persist;
7. export produces a self-contained Markdown research record.
