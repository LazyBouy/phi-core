# Changelog

All notable changes to phi-core are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Forward markers

- **Async parity for tool-update hooks** — `BeforeToolExecutionUpdateFn` and
  `AfterToolExecutionUpdateFn` remain sync in 0.9.0 (see the §0.9.0 "Breaking"
  notes below for rationale). Making them async would cascade into the
  `ToolUpdateFn` callback type and every `AgentTool::execute` body that calls
  `ctx.on_update(...)` — a wider migration deferred to a future release.

---

## [0.9.0] — 2026-05-24

**Breaking-change release.** Ships two bundled surfaces:

1. **Per-turn debug capture.** A new `AgentEvent::TurnRequest` variant
   carries the fully-assembled LLM request — system prompt, post-
   `convert_to_llm()` `Vec<Message>` array, tool definitions, and
   parallel-indexed per-block provenance — exactly once per turn. Opt-in
   persistence onto `Turn::request_payload` via the new
   `SessionRecorderConfig::capture_turn_requests` flag (default `false`).
   Closes the gap where the wire-format payload sent to the model was
   never recoverable post-hoc.

2. **Async-trait migration.** `BlockCompactionStrategy` and 9 of the 11
   `AgentLoopConfig` lifecycle Fns + the `InputFilter` trait become async.
   Custom impls and hook closures can now `.await` LLM calls and other
   async work inside compaction bodies and lifecycle hooks without
   `block_in_place` workarounds. Tool-update hooks
   (`BeforeToolExecutionUpdateFn` / `AfterToolExecutionUpdateFn`) remain
   sync — see the migration notes for the rationale.

Concept source: [`docs/concepts/debugging.md`](docs/concepts/debugging.md)
(new — covers existing debugging surfaces plus the 0.9.0 capture). Plan
archive: i-phi
`docs/v0/proposal/plan/build/phi-core-0.9.0/plan.md`.

### Breaking

- **`AgentEvent::TurnRequest` variant added.** `AgentEvent` is already
  `#[non_exhaustive]` since 0.8.0, so wildcard `_ => …` arms keep
  compiling unchanged. Exhaustive matchers without a wildcard must add a
  `TurnRequest { .. } => …` arm.
- **`LlmMessage` gains `provenance_hint: Option<Box<BlockProvenance>>`.**
  `LlmMessage::new(...)`, `LlmMessage::with_turn(...)`,
  `LlmMessage::with_provenance_hint(...)`, and `LlmMessage::with_node_identity(...)`
  fill the field automatically. Direct struct-literal construction breaks —
  add `provenance_hint: None`. Serialization is `#[serde(default)]` /
  omitted-when-`None`, so old session JSON loads cleanly.
- **`BlockCompactionStrategy` is now `#[async_trait]`.** All four methods
  (`keep_first`, `keep_recent`, `keep_compacted`, `compact`) are `async fn`.
  Sync impls migrate mechanically: prepend `#[async_trait::async_trait]`
  to the impl block and `async` to each method signature. Bodies need no
  changes if they don't `.await` anything. `compact_session_loops` (and the
  `BasicAgent::compact_context*` wrappers) is now `async fn` as well.
- **9 of 11 `AgentLoopConfig` lifecycle Fns become async.**
  `BeforeLoopFn`, `AfterLoopFn`, `BeforeTurnFn`, `AfterTurnFn`, `OnErrorFn`,
  `BeforeToolExecutionFn`, `AfterToolExecutionFn`, `BeforeCompactionStartFn`,
  `AfterCompactionEndFn` switch from `Fn(...) -> T` to
  `Fn(...) -> HookFuture<'_, T>` (alias for `Pin<Box<dyn Future<Output = T> + Send>>`).
  Sync hook bodies migrate by wrapping in `Box::pin(async move { ... })`.
  `BeforeToolExecutionUpdateFn` and `AfterToolExecutionUpdateFn` **stay sync**
  — see migration notes.
- **`InputFilter::filter()` is now `async fn`** (via `#[async_trait]`).
  CPU-bound filters should wrap their work in `tokio::task::spawn_blocking`
  to avoid stalling the runtime.
- **`AgentLoopConfig` gains no new fields** — async-fication is contained
  in the existing hook fields' type aliases.

### Added

- **`AgentEvent::TurnRequest`** variant with fields
  `{ loop_id, turn_index, payload: AnnotatedRequestPayload, timestamp }`.
  Emitted exactly once per turn (before the retry loop's first
  `provider.stream()` call) regardless of recorder configuration.
- **`BlockProvenance`** enum (`#[non_exhaustive]`) with variants
  `SystemPrompt`, `IdentityBlock { name, order }`,
  `MemoryTier { tier, record_id }`,
  `LoopTurn { turn_index, role, message_index }`, `Steering`, `FollowUp`,
  `Unknown`.
- **`ProvenanceRole`** enum: `UserMessage`, `AssistantResponse`,
  `ToolCallRequest`, `ToolCallResult`.
- **`AnnotatedRequestPayload`** struct mirroring the provider wire format
  (`system_prompt` + `messages` + `tools` + model identity / thinking_level
  / max_tokens / temperature / response_format) with a parallel-indexed
  `provenance` vec.
- **`SessionRecorderConfig::capture_turn_requests: bool`** (default
  `false`) — opt-in flag that mirrors `include_streaming_events`.
- **`Turn::request_payload: Option<AnnotatedRequestPayload>`** —
  `#[serde(default, skip_serializing_if = "Option::is_none")]` so existing
  session JSON loads unchanged.
- **`LlmMessage::with_provenance_hint(BlockProvenance)`** consuming
  builder — used by upstream consumers (identity loaders, memory stores)
  to stamp non-loop-history provenance before emitting messages.
- **`HookFuture<'a, T>`** type alias for `Pin<Box<dyn Future<Output = T> + Send + 'a>>`
  in `phi_core::agent_loop` — short hand for async hook return types.
- **`phi-core/docs/concepts/debugging.md`** — new concept doc covering all
  debug surfaces (`AgentEvent` stream / `SessionRecorder` JSON / `tracing`
  integration) plus the new per-turn capture flow.

### Migration

**Custom `BlockCompactionStrategy` implementations** — add
`#[async_trait::async_trait]` to the impl block and `async` to each method.
If your bodies don't `.await` anything, no further changes are needed:

```rust
use async_trait::async_trait;
use phi_core::context::{BlockCompactionStrategy, CompactedSection, TurnMap, TurnRange};
use phi_core::session::LoopRecord;

struct MyStrategy;

#[async_trait]
impl BlockCompactionStrategy for MyStrategy {
    async fn keep_first(
        &self,
        record: &LoopRecord,
        turn_map: &TurnMap,
        config: &phi_core::context::CompactionConfig,
    ) -> Option<TurnRange> {
        // Sync body — unchanged. Or await an LLM call here.
        None
    }
    // ... keep_recent / keep_compacted similarly
}
```

**Lifecycle hook closures** — wrap the sync body in
`Box::pin(async move { ... })`:

```rust
use std::sync::Arc;
use phi_core::agent_loop::BeforeTurnFn;

let hook: BeforeTurnFn = Arc::new(|messages, turn_index| {
    Box::pin(async move {
        println!("turn {} starting with {} messages", turn_index, messages.len());
        true // false to abort the turn
    })
});
```

Closures that previously did `tokio::task::block_on(async { llm_call().await })`
can drop the bridge and just `.await` directly inside the `async move` block.

**`LlmMessage` struct-literal construction** — add `provenance_hint: None`:

```rust
let lm = phi_core::LlmMessage {
    message: phi_core::Message::user("hi"),
    turn_id: None,
    node_id: None,
    parent_id: None,
    tags: vec![],
    provenance_hint: None, // <-- 0.9.0 addition
};
```

Or prefer the constructor: `LlmMessage::new(Message::user("hi"))`.

**`InputFilter::filter()` is now `async fn`** — prepend
`#[async_trait::async_trait]` to the impl block + `async` to the method.
For CPU-bound filters:

```rust
async fn filter(&self, input: &str) -> FilterDecision {
    let owned = input.to_string();
    tokio::task::spawn_blocking(move || expensive_sync_scan(&owned))
        .await
        .unwrap_or(FilterDecision::Allow)
}
```

**Exhaustive `match AgentEvent` arms** — `#[non_exhaustive]` shielded the
type since 0.8.0, so wildcard arms compile unchanged. Otherwise add:

```rust
AgentEvent::TurnRequest { loop_id, turn_index, payload, timestamp } => {
    // per-turn debug payload available here
}
```

**Pre-existing-behaviour preservation note —
`BeforeToolExecutionUpdateFn` + `AfterToolExecutionUpdateFn` stay sync.**
Making them async would cascade into the `ToolUpdateFn` callback type and
every `AgentTool::execute` body that invokes `ctx.on_update(...)` —
materially wider than the 0.9.0 cycle's scope. The veto decision in
`BeforeToolExecutionUpdateFn` must be synchronous so the surrounding
emit-gate works without an `.await` suspension point at every streamed
tool-update; consumers that want async work at update-time should
dispatch via `tokio::spawn(...)` inside the sync closure body. Tracked
under the `[Unreleased]` "Forward markers" section for a future release.

### Internal

- New `phi-core/src/types/provenance.rs` (`BlockProvenance` + `ProvenanceRole`
  + `AnnotatedRequestPayload` with `serde` round-trip including a
  `response_format` proxy because `ResponseFormat` does not derive serde
  natively).
- `stream_assistant_response()` derives a parallel `Vec<BlockProvenance>` for
  the wire-format `messages` vec, reading `LlmMessage::provenance_hint`
  when set and falling back to `turn_id` + role-derivation otherwise.
- `compact_session_loops` is `async fn`; in-loop call sites in
  `agent_loop/run.rs` adopt `.await`.
- Test count: 461 → 470 (+9 across new integration tests
  `tests/turn_request_capture_test.rs` + `tests/async_compaction_strategy_test.rs`).

---

## [0.8.0] — 2026-05-23

**Breaking-change release.** Ships **Composition I** — an opt-in tree-structured
"braking" layer on top of the agent's conversation. The agent can now call a
new `revert_to_state` tool to abandon failed or finished branches between
turns; the next prompt is rebuilt by walking parent-id links from the active
node, so abandoned spans drop out of context while the forensic record stays
intact. Composition I sits **above** `BlockCompactionStrategy` /
`compact_messages` / episodic memory — it does NOT replace them; it delays
how often they must run.

The braking layer is **opt-in**. A consumer that upgrades 0.7.1 → 0.8.0
without changing how it constructs its agent sees no behavioural change —
the new tool is not registered, the parent-chain walk does not activate, and
`build_working_context` takes the byte-identical linear path it always did.
Enable via `BasicAgent::with_revert_tool()` (one line on the builder).

Concept source: [`docs/concepts/concept-brake.md`](docs/concepts/concept-brake.md) §5
Composition I. Plan archive: i-phi
`docs/v0/proposal/plan/build/phi-core-revert-tool-27c894f6/plan.md`.

### Breaking

- **`LlmMessage` gains three new public fields** — `node_id: Option<NodeId>`,
  `parent_id: Option<NodeId>`, `tags: Vec<NodeTag>`. Construction via
  `LlmMessage::new(Message::…)` or the `with_turn_*` builders is unaffected
  (the constructors fill the new fields with their defaults). Direct
  `LlmMessage { … }` struct-literal construction breaks — callers must add
  `node_id: None, parent_id: None, tags: vec![]` or switch to the
  constructor. Custom serde is extended so old session JSON loads cleanly
  (the new fields are `#[serde(default)]` optionals — `nodeId` / `parentId` /
  `tags` keys are present only when non-default).
- **`AgentEvent` becomes `#[non_exhaustive]`** and gains the
  `RevertApplied { loop_id, category, target, abandoned_node_ids, summary,
  applied, reason, timestamp }` variant. Every exhaustive `match` against
  `AgentEvent` in a downstream crate now requires either an explicit
  `RevertApplied { … }` arm or a wildcard `_ => …`. The disruption is paid
  once now; subsequent additions to `AgentEvent` are non-breaking.
- **`AgentLoopConfig` gains `revert_pending: Option<Arc<Mutex<Vec<RevertRequest>>>>`**.
  Construction via `BasicAgent::build_config()` / `Agent::build_config()`
  default impl is unaffected (the field is filled automatically). Struct-
  literal construction breaks — callers must add `revert_pending: None`.

### Added

- New module `tools::revert` — `RevertTool`, `RevertRequest`, `RevertRecord`.
  The tool is model-callable with four kebab-case categories: `failure` /
  `tangent` / `completion` / `step-summary`, plus an optional `summary` the
  agent writes inline.
- New `BasicAgent::with_revert_tool()` builder — registers `RevertTool` and
  wires the shared pending queue into `AgentLoopConfig`. The opt-in
  guarantee is enforced end-to-end: the LLM never sees the tool unless this
  method was called.
- New `BasicAgent::tools()` accessor — read-only view of the registered
  tool set. Useful for tests that assert tool-registry shape (e.g. the
  Composition I opt-in regression).
- New `types::node_tag` module — `NodeId`, `NodeTag`, `TagKind`,
  `RevertCategory`, `RevertRenderPolicy`. `NodeId` renders inline as `n<N>`
  and parses leniently from `"n12"` / `"12"`.
- New `AgentContext::active_node_id` and `AgentContext::next_node_id` fields
  + `alloc_node_id()` and `seed_next_node_id_from_messages()` helpers.
- New `AgentContext::build_trunk_context()` — parent-chain walk that
  assembles the LLM-facing context from `active_node_id`. Cycle-guarded
  via a visited set; dangling parents stop gracefully; an unresolved
  active node falls back to `messages.clone()`.
- New `AgentContext::build_trunk_context_with_policy(policy, current_turn)`
  — applies kind-aware filtering: `Lesson` / `Finding` tags drop out of the
  prompt past the policy window (default 5 turns) AND the per-kind count
  cap (default 3); `Outcome` / `Checkpoint` tags stay pinned while
  on-trunk. Policy defaults are tunable per application via
  `RevertRenderPolicy`.
- New `apply_revert` between-turn drain in `agent_loop/run.rs`, mirroring
  `apply_prun`. Synchronous, emits exactly one `RevertApplied` event per
  drained request (success or rejection). Rejection rules: unknown target
  and user-message-in-span (per D6 — auto-rebase deferred to a future
  release).
- New concept doc `docs/concepts/composition-i.md`; companion
  `docs/concepts/concept-brake.md` is the design source-of-truth (promoted
  from i-phi to phi-core in this release).

### Documentation

- Promoted `concept-brake.md` from i-phi to `phi-core/docs/concepts/`.
- New `docs/concepts/composition-i.md` with `[EXISTS]` status tags.
- Bumped `phi-core = "0.8"` in `README.md` and added a Composition I
  feature bullet.

---

## [0.7.1] — 2026-05-16

Documentation-only patch release. No code changes. Bumped so the crates.io
README and rendered docs reflect the 0.7.0 surface accurately.

### Documentation

- Bumped `phi-core = "0.7"` in `README.md` and `docs/getting-started/installation.md`.
- Corrected `build_config()` return type in `docs/guides/configuration.md` to
  `Result<AgentLoopConfig, AgentBuildError>`.
- Extended `docs/specs/architecture.md` SessionStore section to describe the
  `SessionStore` trait + `FileSystemSessionStore` with atomic writes and
  advisory locks.
- Added a "0.7.0 additions" subsection to `docs/reference/api.md` listing
  module-path imports for `SessionStore`, `FileSystemSessionStore`,
  `CredentialProvider`, `StaticCredentialProvider`, `ResponseFormat`,
  `AgentBuildError`, `McpClientConfig`, `DEFAULT_REQUEST_TIMEOUT`.
- Added a "Pluggable store trait" subsection to `docs/concepts/sessions.md`
  documenting the trait API + `fs2` locking contract.
- Added this `CHANGELOG.md` (Keep-a-Changelog format).
- Refreshed verified-headers on all touched doc files.

---

## [0.7.0] — 2026-05-16

Hardening + ergonomics release. Brings phi-core to production-ready for
single-process agent workloads. One small breaking change to the `Agent` trait;
the rest is additive.

### Breaking

- **`Agent::build_config()`** now returns
  `Result<AgentLoopConfig, AgentBuildError>` instead of `AgentLoopConfig`.
  - The default implementation no longer panics when `model_config()` returns
    `None`; it returns `Err(AgentBuildError::MissingModelConfig)`.
  - `BasicAgent`'s override always returns `Ok(...)` because its constructor
    requires a `ModelConfig` — no behavioral change for the common path.
  - **Migration:** any custom `Agent` impl that overrides `build_config()` must
    wrap its return value in `Ok(...)`. Callers of `agent.build_config()` need
    to handle the `Result` (typically with `?` or `.expect()`).

### Added

- **Per-tool timeouts.** New `AgentLoopConfig.tool_timeout: Option<Duration>`
  and `AgentTool::timeout() -> Option<Duration>` default method. Resolution
  order: tool-level override → config-level → no timeout. On timeout, the tool
  call is cancelled and the LLM receives a structured error result (so it can
  self-correct) instead of starving sibling tools. Adds
  `ToolError::Timeout { duration }` variant.
- **Structured-output contract.** `ResponseFormat::{Text, JsonObject, JsonSchema}`
  enum on `StreamConfig`. Each provider maps it to its native JSON mode where
  available; Anthropic and Anthropic-on-Bedrock emulate via a synthetic
  tool-call; unsupported configurations surface `ProviderError::SchemaMismatch`
  instead of silently producing free-form text. New
  `Message::extract_json::<T: DeserializeOwned>()` for ergonomic deserialisation.
- **Credential refresh.** New `CredentialProvider` async trait
  (`current()` / `invalidate()`) attachable via
  `ModelConfig::with_credentials(provider)`. On `ProviderError::Auth`, the
  agent loop invalidates the cached credential and retries once before
  propagating — supports long-running agents on STS / OAuth tokens.
  `StaticCredentialProvider` is provided for testing.
- **`SessionStore` trait.** Async pluggable persistence trait
  (`save` / `load` / `list_ids` / `delete` / `list_for_agent`) alongside the
  existing free functions. In-tree `FileSystemSessionStore` adds advisory
  `fs2` exclusive locking on save: concurrent writers to the same
  `session_id` get `SessionError::Locked` instead of corrupted JSON.
- **MCP transport timeouts.** Both `StdioTransport` and `HttpTransport` now
  take a `request_timeout` (default 30 s). A hung MCP subprocess no longer
  blocks the entire agent loop indefinitely. Configure via the new
  `McpClientConfig` + `McpClient::connect_{stdio,http}_with_config()`. The
  `DEFAULT_REQUEST_TIMEOUT` constant is exported. Adds `McpError::Timeout`.

### Changed

- **Atomic session writes.** `save_session()` and `FileSystemSessionStore`
  now write to a tmp file and rename over the target. Readers no longer
  observe partially-written JSON during a save.
- **Internal:** consolidated `agent_id`/`session_id`/`loop_id` initialisation
  in `agent_loop::core` behind a single `ensure_loop_ids()` helper instead of
  scattered `.unwrap()` calls.

### Fixed

- **Poison-tolerant steering / follow-up queues.** A panic inside a hook or
  tool callback no longer crashes the agent session: the recovery helper logs
  a warning and returns the inner `Vec<AgentMessage>` via
  `PoisonError::into_inner()`.
- **Hot-path `.unwrap()`s removed** from `agent_loop::core`,
  `agent_loop::parallel`, and the Google provider's temperature parsing.
  Non-numeric temperatures now surface `ProviderError::Internal` instead of
  panicking mid-stream.

### Dependencies

- Added `fs2 = "0.4"` for advisory file locking in `FileSystemSessionStore`.

---

## [0.6.x] and earlier

See git history (`git log v0.6.0..v0.7.0` for the full diff).
