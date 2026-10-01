# SPEC-0115 — NeuralIA Chat Surface (Multi-Provider Conversation Layer)

**Status:** Proposed (not implemented)  
**Target:** NeuralIA 2.2 or later  
**Depends on:** SPEC-0100, SPEC-0101, SPEC-0104, SPEC-0105, SPEC-0108  
**Related:** SPEC-0103 (timeline), SPEC-0107 (memory)

## 1. Purpose

The NeuralIA chat surface is the NeuralIA-owned conversation layer around the
existing provider WebViews. It lets one research intent span multiple providers
while retaining turn grouping, provenance, local history, export, and optional
semantic-memory integration.

This feature extends the current comparator and research-session flows. It does
not replace provider pages, add a second browser runtime, or create a new action
execution path.

## 2. Normative language

The words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative. Examples
and type sketches are informative unless an acceptance criterion explicitly
requires their behavior.

## 3. Baseline constraints

An implementation MUST preserve these existing properties:

- Provider pages continue to run in the existing system WebViews.
- Page-originated input remains untrusted and crosses the existing bounded IPC
  boundary.
- Agent-initiated actions continue through `decide_agent_step` and
  `AgentPermissionPolicy`; the chat feature does not execute page actions.
- `ResearchSession` remains the authority for research comparison and export.
- Semantic-memory writes continue through the existing `MemoryWorker` path.
- Home, startup, and network-idle guarantees remain unchanged.
- No new async runtime, UUID crate, browser engine, resident model, telemetry, or
  cloud synchronization is introduced solely for this feature.
- The current installer WebView budget MUST NOT increase.

The implementation SHOULD reuse the project's existing string identifier,
synchronous worker, atomic-write, error, and serialization conventions. A new
dependency requires a separate, justified change and is not implied by this
specification.

## 4. Non-goals

This specification does not require:

- replacing provider pages with a custom chat renderer;
- submitting prompts to providers through a hidden or privileged channel;
- unrestricted tool calling or model-provided JavaScript execution;
- cross-device or cloud profile synchronization;
- real-time multi-user collaboration;
- arbitrary conversation branching;
- bundling a local 3B+ chat model;
- migrating the existing `ResearchSession` JSON persistence in this feature.

## 5. Domain model

Identifiers are opaque, non-empty `String` newtypes generated with the existing
project convention. Their serialized representation is stable. Ordering never
depends on lexical identifier order.

```text
ChatThread
├── thread_id: ChatThreadId
├── research_session_id: Option<String>
├── title: String
├── intent: Option<String>
├── privacy: Persistent | Private
├── created_at_ms: u64
├── updated_at_ms: u64
├── next_turn_ordinal: u64
└── providers: ordered unique ProviderId values

ChatTurn
├── turn_id: TurnId
├── thread_id: ChatThreadId
├── ordinal: u64
├── user_message_id: MessageId
├── created_at_ms: u64
└── responses: ordered by provider then attempt

ChatMessage
├── message_id: MessageId
├── thread_id: ChatThreadId
├── turn_id: TurnId
├── ordinal: u64
├── attempt: u32
├── role: User | Assistant | System | Tool | Note
├── provider_id: Option<ProviderId>
├── status: MessageStatus
├── extraction_quality: ExtractionQuality
├── parts: Vec<ContentPart>
├── is_partial: bool
├── created_at_ms: u64
└── updated_at_ms: u64
```

### 5.1 Invariants

1. A thread MAY exist without a research session.
2. A thread is linked to at most one research session.
3. A turn belongs to exactly one thread and has one user message.
4. A provider response belongs to one turn. Retries increment `attempt`; they do
   not overwrite an earlier attempt.
5. `(thread_id, ordinal)` is unique for turns. `(turn_id, provider_id, attempt)`
   is unique for provider responses.
6. Message and turn creation is idempotent for a caller-supplied operation key.
   Replaying the same operation after a crash returns the original entity.
7. Closing or navigating a provider tab MUST NOT delete or invalidate a thread.
8. Provider order is deterministic and recorded when the turn is created.
9. `updated_at_ms` is metadata only; it is never the correctness source for
   ordering or idempotency.

## 6. Message content and limits

```rust
pub enum ContentPart {
    Text { text: String, is_markdown: bool },
    Code { language: Option<String>, text: String },
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    SourceRef {
        url: String,
        title: String,
        provider: ProviderId,
        excerpt: Option<String>,
    },
    ImageRef { local_hash: String, mime_type: String, byte_len: u64 },
    Error { code: String, message: String, retryable: bool },
}

pub enum MessageStatus {
    Pending,
    Streaming,
    Complete,
    NeedsInteraction { reason: String, tab_id: u64 },
    Error { code: String, retryable: bool },
    Cancelled,
}

pub enum ExtractionQuality {
    Structured,
    PlainTextFallback { warning: String },
}
```

`MessageStatus` describes lifecycle. `ExtractionQuality` describes parsing
quality; degraded extraction is not a terminal lifecycle state.

Hard limits MUST be enforced before allocation or persistence:

- existing IPC envelope limit: 8 KiB;
- one streaming-delta payload: at most 6 KiB after serialization overhead;
- assembled message content: at most 2 MiB UTF-8;
- content parts per message: at most 2,048;
- source references per message: at most 256;
- title: at most 512 Unicode scalar values;
- source excerpt: at most 240 Unicode scalar values.

An oversized delta is rejected with a stable error code. An assembled message
that reaches its cap is finalized as `Error { code: "content_limit" }` with
`is_partial = true`; it is not silently truncated and is not memory-eligible.

Raw HTML is never a `ContentPart`. Extracted HTML MUST be sanitized or converted
to text before it crosses the adapter boundary. URLs MUST pass the existing URL
validation and scheme restrictions.

## 7. Lifecycle and state transitions

Allowed transitions are:

```text
Pending -> Streaming | NeedsInteraction | Error | Cancelled
Streaming -> Complete | NeedsInteraction | Error | Cancelled
NeedsInteraction -> Pending | Streaming | Error | Cancelled
Complete, Error, Cancelled -> terminal
```

Terminal messages are immutable. A retry creates a new attempt. Duplicate or
out-of-order deltas are ignored using a monotonically increasing per-message
`delta_seq`; gaps mark the message partial and request a final snapshot when the
provider adapter can supply one.

After restart, persisted `Pending` or `Streaming` messages become
`Error { code: "interrupted", retryable: true }` and `is_partial = true` in one
recovery transaction. They are never presented as complete.

## 8. Provider boundary

Provider adapters are observers and normalizers. They MAY:

- validate and normalize events received from the existing IPC path;
- associate a page event with a known provider, tab, turn, and message;
- extract sanitized text, code, tables, citations, and status changes;
- report that human interaction is required.

They MUST NOT click, navigate, fill, submit, evaluate arbitrary script, or call
the policy engine directly.

There are two distinct prompt paths:

1. **Human direct submission:** the existing UI sends the user's prompt through
   the already-shipped provider interaction path. The chat layer records the
   resulting turn and observes responses.
2. **Agent-proposed submission:** the proposal MUST pass through
   `decide_agent_step`, `AgentPermissionPolicy`, and any required native
   confirmation before the existing execution path performs it.

The implementation MUST NOT simulate an agent action by invoking a provider
adapter method.

An implementation-oriented core boundary is synchronous and WebView-free:

```rust
pub trait ChatStore: Send {
    fn create_thread(&mut self, request: CreateThread) -> Result<ChatThreadId, ChatError>;
    fn create_turn(&mut self, request: CreateTurn) -> Result<TurnId, ChatError>;
    fn append_message(&mut self, request: NewMessage) -> Result<MessageId, ChatError>;
    fn apply_delta(&mut self, delta: MessageDelta) -> Result<(), ChatError>;
    fn finalize_message(&mut self, request: FinalizeMessage) -> Result<(), ChatError>;
    fn get_thread(&self, id: &ChatThreadId) -> Result<ChatThread, ChatError>;
    fn delete_thread(&mut self, id: &ChatThreadId, policy: DeleteThreadPolicy)
        -> Result<DeleteReport, ChatError>;
}

pub trait ProviderEventNormalizer: Send {
    fn provider_id(&self) -> ProviderId;
    fn normalize(&mut self, event: ProviderPageEvent)
        -> Result<Vec<NormalizedChatEvent>, ChatError>;
}
```

Exact names MAY change, but authority separation, synchrony, and absence of
WebView2 types are normative.

## 9. Worker, streaming, and backpressure

Storage operations run on a dedicated `ChatWorker`, following the existing
bounded synchronous-worker pattern.

- Command queue capacity: 256.
- UI update cadence: at most one coalesced update per rendered message per
  animation frame; 60 Hz is a ceiling, not a timer requirement.
- Intermediate deltas stay in memory and do not trigger one transaction per
  token.
- A recovery snapshot MAY be flushed after accumulated content or elapsed work,
  but correctness MUST NOT depend on wall-clock timing.
- `Complete`, `Error`, and `Cancelled` force a final storage command.
- When the worker queue is full, adjacent deltas for the same message are
  coalesced. Terminal commands and delete commands MUST NOT be dropped.
- Heavy parsing, export, embedding, and memory ingestion MUST NOT run on the UI
  thread.

Tests MUST use structural counters, bounded queues, and deterministic hooks—not
absolute elapsed-time assertions.

## 10. Persistence and migration

Persistent chat data uses a dedicated SQLite store owned by the chat feature.
It MUST NOT reuse or mutate semantic-memory tables. Private chat uses a distinct
in-memory store and never opens the persistent chat database.

Schema version V01 contains, at minimum:

- `chat_meta(schema_version, created_by_version)`;
- `chat_threads`;
- `chat_thread_providers`;
- `chat_turns`;
- `chat_messages`;
- `chat_content_parts`;
- `chat_operation_keys` for idempotency;
- indexes for thread update order, turn order, and message lookup.

Foreign keys MUST be enabled. Content-part order uses an explicit ordinal. All
creation/finalization/deletion changes are transactional. Startup either opens a
valid known schema, creates V01 atomically, or returns a typed unsupported/corrupt
error; it never silently rebuilds or discards user data.

Migration tests MUST cover:

- fresh V01 creation;
- reopen with data intact;
- unsupported future schema rejection;
- interrupted-message recovery;
- foreign-key and uniqueness enforcement;
- idempotent replay of every mutating command;
- rollback after an injected failure at each transaction boundary.

## 11. Privacy, redaction, and deletion

- Persistent conversation data remains local and is not telemetry.
- Private threads and their operation keys remain memory-only and are wiped when
  the private context closes or the process exits.
- Passwords, payment fields, cookies, authorization headers, session tokens, and
  page storage values MUST NOT be captured.
- Logging MUST contain identifiers and typed error codes, never prompt or answer
  bodies.
- Export is an explicit user action and applies the same redaction rules.

Thread deletion offers:

```rust
pub enum DeleteThreadPolicy {
    PurgeDerivedMemory,
    KeepDerivedMemory,
}
```

Session deletion offers:

```rust
pub enum DeleteSessionPolicy {
    DeleteLinkedThreads { memory: DeleteThreadPolicy },
    DetachLinkedThreads,
}
```

The current `ResearchSession` persistence is JSON, so the session link is a soft
reference validated by the application layer. Detaching MUST clear every linked
thread atomically within the chat store. Deleting a thread MUST produce a
`DeleteReport` listing derived memory IDs requested for purge. A failed memory
purge remains a visible retryable operation; the UI MUST NOT claim it succeeded.

## 12. Semantic-memory hand-off

A message is eligible only when all are true:

- `status == Complete`;
- `is_partial == false`;
- thread privacy is `Persistent`;
- role is `Assistant` or `Note`;
- the user-enabled memory policy permits it.

The memory record preserves thread, turn, message, provider, source, and session
identifiers. The hand-off is idempotent by message ID and uses `MemoryWorker`.
Pending, streaming, interaction-required, errored, cancelled, private, or
partial messages produce zero semantic records.

## 13. Research-session integration

- A multi-provider prompt MAY create a standalone thread or attach to an existing
  `ResearchSession`.
- Sources selected from `SourceRef` are copied into the session with canonical
  URL and provider-of-origin metadata.
- Comparison, synthesis, and research export remain operations on the session,
  not on transient tab state.
- Until `ResearchSession` gains transactional storage, chat/session updates are
  an explicit two-step operation with a repairable pending-link marker. Startup
  reconciliation completes or rolls back incomplete links deterministically.

## 14. UI behavior

- Existing provider tabs remain in the native title bar.
- Thread and turn indicators are compact and keyboard accessible.
- Closing a tab does not close a thread.
- An internal, sanitized archive view MAY render persisted content, but it MUST
  reuse an existing NeuralIA-owned surface and MUST NOT add another WebView.
- The UI distinguishes lifecycle status from extraction quality and visibly marks
  partial or fallback content.
- Source links use the existing routing rules, including side-panel modifiers.
- Destructive deletion and derived-memory purge require a clear native
  confirmation describing both scopes.

Minimum keyboard behavior and accessible names MUST be specified in the UI PR;
no keyboard shortcut in this feature may shadow an existing shortcut.

## 15. IPC additions

Any additions to SPEC-0108 use the closed action set and the existing 8 KiB
envelope. Conceptual actions are:

- `chat-response-start`;
- `chat-response-delta`;
- `chat-response-final`;
- `chat-response-error`;
- `chat-needs-interaction`.

Each event contains only validated scalar metadata plus bounded sanitized text.
The native side validates action, provider/tab association, identifiers, sequence,
size, and state transition before forwarding to `ChatWorker`. Unknown actions,
invalid transitions, oversized values, and cross-tab/provider mismatches are
rejected closed.

## 16. Performance and resource gates

Performance acceptance is structural and relative:

- Creating or appending one chat entity performs a bounded number of SQL
  statements independent of total thread count.
- Capturing a delta performs no directory walk, full-database integrity scan,
  semantic indexing, or synchronous export.
- UI-to-worker communication is bounded and non-blocking.
- A benchmark corpus compares the feature branch with its baseline using the same
  machine and fixture. Median creation/finalization work MUST NOT regress by more
  than 20% without an approved exception.
- Memory growth while streaming is bounded by the message and queue limits.
- Installer size and WebView count do not increase because of this feature.

No acceptance test asserts a fixed millisecond threshold.

## 17. Acceptance criteria

The feature is ready only when all of the following are demonstrated through the
shipped product path:

1. One user prompt creates one ordered turn and distinct provider attempts under
   the same turn.
2. Replaying a mutating operation key creates no duplicate thread, turn, message,
   content part, or memory record.
3. Streaming remains responsive under a full worker queue; deltas coalesce and a
   terminal event is retained.
4. Complete answers preserve provider and source provenance and can participate
   in research comparison.
5. Closing or navigating provider tabs does not destroy thread state.
6. Restart recovery marks unfinished messages partial and retryable, never
   complete.
7. Private mode produces zero persistent chat rows, files, logs containing
   content, and semantic-memory records.
8. Every agent-proposed action leaving the conversation crosses
   `decide_agent_step` and `AgentPermissionPolicy`; human direct submission is
   explicitly distinguished in tests.
9. Only eligible messages reach `MemoryWorker`; every excluded state produces
   zero memory writes.
10. Thread/session deletion obeys the selected detach/cascade policy and exposes
    retryable memory-purge failures.
11. Markdown export is deterministic and includes thread intent, ordered roles,
    provider origin, canonical source list, and bounded excerpts.
12. Malformed IPC, invalid transitions, out-of-order deltas, oversized content,
    and cross-provider spoofing fail closed.
13. `cargo fmt --all -- --check`, workspace clippy with warnings denied, and the
    full workspace test suite pass.

Required deliberate sabotage proofs:

- bypass the policy call on the shipped agent-action path: product-wiring test
  turns red;
- persist a private thread: privacy test turns red;
- allow a partial/error message into memory: eligibility test turns red;
- make the worker queue unbounded or drop a terminal command: backpressure test
  turns red;
- remove operation-key uniqueness: replay test turns red;
- perform a full scan in the delta path: structural cost test turns red.

Each sabotage is temporary, reverted before commit, and its failing test output
is recorded in the PR.

## 18. Implementation plan

Each phase is a separate reviewable PR unless maintainers explicitly approve a
smaller grouping. Later phases do not weaken earlier gates.

### Phase 0 — Contract and fixtures

- Freeze serialized domain fixtures and typed errors in `neural-core`.
- Add no product wiring.
- Confirm identifiers, limits, transition table, and SQLite V01 design.

### Phase 1 — Store and recovery

- Implement persistent SQLite and private in-memory `ChatStore` variants.
- Add migration, idempotency, rollback, deletion, and recovery tests.

### Phase 2 — Capture and IPC normalization

- Add closed IPC actions and provider event normalizers.
- Connect existing captured provider answers to `ChatWorker`.
- Prove spoofing, size, state, and tab/provider validation.

### Phase 3 — Thread/archive surface

- Add thread navigation and sanitized archive rendering without another WebView.
- Add accessibility, keyboard, tab-lifecycle, and bounded-stream tests.

### Phase 4 — Research and export

- Add repairable session linking, source attachment, comparison, and deterministic
  Markdown export.

### Phase 5 — Semantic memory

- Add eligible-message hand-off and deletion/purge retry tracking through
  `MemoryWorker`.

### Phase 6 — Agent awareness

- Allow bounded conversation context reads.
- Route every agent-proposed external action through the existing policy and
  execution path, with product-wiring and sabotage proof.

Probable files include new chat modules under `crates/neural-core/src/`, their
unit/integration tests, bounded IPC changes in `crates/neural-app/src/`, and
targeted UI wiring. Exact filenames are implementation choices; edits to
security-sensitive IPC, policy, memory, or persistence code require independent
review.

## 19. Explicitly deferred

- cloud sync and shared threads;
- arbitrary reply trees;
- hidden provider automation outside the existing execution path;
- model-supplied JavaScript;
- large bundled local chat models;
- custom browser engines;
- automatic migration of historical provider-page conversations;
- migration of `ResearchSession` JSON storage.

## 20. Definition of success

A user can start one research question, observe multiple provider answers grouped
under one durable turn, close provider tabs without losing the thread, preserve
provenance, export deterministically, optionally remember only eligible content,
and remain protected by the existing policy boundary whenever the agent acts.

The feature is not complete if it is only a token-streaming sidebar, if it adds a
parallel execution path, or if its privacy and failure guarantees are not proven
by negative tests.
