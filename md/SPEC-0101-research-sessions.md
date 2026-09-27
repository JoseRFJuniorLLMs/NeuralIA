# SPEC-0101 — Research Sessions and Cross-Source Synthesis

**Status:** Implemented — NeuralIA 2.0  
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

## 10. Turns and the native answer reader (NeuralIA 2.5)

**Status:** Partial — phase 1 (consensus-reader-turns): the turn model, the
native reader and the `research:compare` path below exist and are gated; the
Consenso panel that will display the readings is a later item.

### 10.1 Turns (SPEC-0109 §5.1 invariants, owner question OQ8)

A session is a list of **turns** (`ResearchTurn` in `neural_core::research`),
one per question sent to a set of providers. The invariants, each pinned by a
test in `research.rs` (`turn_ordinals_are_unique_and_never_reused`,
`begin_turn_is_idempotent_by_operation_key`,
`attempts_append_and_never_overwrite`, `a_session_saved_before_turns_still_loads`):

- `ordinal` is unique within the session and never reused — a new turn takes
  the highest ordinal ever seen plus one, even when the session on disk has a
  gap;
- `begin_turn` is **idempotent by operation key**
  (`neural_core::operation_key(origin, source column, SHA-256 of the text,
  navigation generation of the source column)`): the same question repeated
  from the same column before it navigates returns the same turn; after the
  column navigates it is a new operation;
- the provider order is recorded at creation and never reordered;
- per-provider **attempts** (`ProviderAttempt`) only append: a second reading
  of ChatGPT is attempt 2 next to attempt 1, never over it; each attempt keeps
  its status (`read`, `maybe-incomplete`, `unreadable`, `translated`,
  `other-question`, `failed`) and the id of the item holding the text;
- a text read for a turn is a new `ProviderAnswer` item with `turn` and
  `links` (the cited `http(s)` URLs, in marker order); the legacy
  `upsert_provider_answer` path (the page-pushed `research-answer`) is
  unchanged;
- at most `MAX_CONSENSUS_SNAPSHOTS` (8) `ConsensusSnapshot`s stay in the
  session, the oldest evicted first; a session saved before 2.5 (no `turns`,
  `consensus`, `turn` or `links`) loads unchanged and re-saves byte-identical
  until it gains a turn.

Turns open where the question leaves: `compare` (the three columns, turn 1
of a new session), `ask_other_columns` (a question typed in one column, sent
to the others) and the palette's `LoadProvider` (one column), always before
the columns navigate (`consensus_turns_open_at_every_question_source`).
Sessions and their snapshots are written only through
`PrivacyGuard::save_session` (`memory/sessions/<id>.json`, `Automatic`: a
no-op in the private mode; SPEC-0006 phase-0 table).

### 10.2 The native answer reader (§7: read-only page script)

`research:compare` no longer shows the facts of the answers the page pushed;
`App::read_consensus` (`crates/neural-app/src/windows_app/consensus.rs`)
reads each column natively:

- `ANSWER_READ_SCRIPT` runs through `page_eval` (token, deadline, raw cap,
  navigation generation and URL re-checked on arrival) on `comp.views[col]`
  **only** — never the source opened beside a column, normal or private
  (`consensus_readable`; gate `consensus_reads_only_the_columns_never_the_split`).
  It receives its configuration as the script argument
  (`{selector, busy, markers, max: 24000, maxLinks: 60}`) and returns
  `{v, ok, host, busy, cut, text, links}`: the Markdown of the **last**
  assistant message with each `http(s)` link replaced by a citation marker
  `U+E000 n U+E001` pointing at `links[n]`. It registers no listener and
  posts nothing; the only way back is the `evaluate_script` callback.
- `parse_answer_read` treats the reply as untrusted data: 512 KiB cap before
  serde, exactly the seven keys, `v == 1`, typed fields, `host` equal to the
  column page's host (a login page never passes for an answer), text within
  24 000 characters, at most 60 links, each `http(s)` with a host and within
  2 048 bytes, every citation marker pointing inside `links`
  (`answer_read_parse_caps_raw_and_requires_exact_keys`,
  `answer_read_refuses_a_host_mismatch`).
- Polling every 1.5 s until the answer **settles** (two identical reads
  without the provider's stop control), the column navigates to **another
  question** (its navigation generation moved) or 120 s pass, when the last
  read is kept as `maybe-incomplete`; a late read, one from another page or
  from a finished run is dropped
  (`consensus_polls_until_settled_other_question_or_timeout`,
  `consensus_drops_a_late_or_foreign_read`).
- A column under `TRANSLATE_APPLY` is never read until `TRANSLATE_RESTORE`;
  if the original does not come back it is reported as «traduzida — não
  comparada» and never compared (critique C9;
  `consensus_never_compares_a_translated_column`).
- The comparison of the readings (entities, numbers, dates) runs on the
  `neural-consensus` thread, a `LazyWorker` born on the first report, never
  in `App::new`.

**Selectors (registry `neural_core::search`, `AnswerReadSelector`).** Only
ChatGPT's selector is *grounded*: `[data-message-author-role="assistant"]` is
what the shipped column script's `research-answer` reader has used in
production since 2.0, and the synthetic fixture pins it. **Claude and Google
AI Mode are ASSUMED** (`SelectorGrounding::Assumed`): without a logged-in
session there is no capture of the live DOM, so their selectors (and every
stop-control selector, ChatGPT's included) come from the known page
structure and are pinned by the fixtures in `search.rs` and by the `node:vm`
fixtures of `answer_read_script_reads_the_last_assistant_message_per_selector`
— to be verified with F12 on a logged-in session and re-pinned. Providers
without a selector read as «não lida».
