# Changelog

All notable changes to phi-core are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
