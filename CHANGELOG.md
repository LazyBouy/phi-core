# Changelog

All notable changes to phi-core are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
