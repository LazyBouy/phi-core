/*
lib.rs — The crate root of yo-core (phi-core).

RUST QUIRK: `mod` vs `use` — Declaration vs Import

  `mod foo;`   — "this crate has a module called foo, load it from foo.rs (or foo/mod.rs)"
                 Python analogy: nothing direct, but closest is the implicit __init__.py that
                 makes a directory a package. In Rust, you must explicitly declare every module.

  `pub mod foo;` — same, but the module is publicly accessible to crate users.
                   Private modules (without `pub`) can still be used inside this crate.

  `use foo::Bar;` — "bring Bar into scope by name" (no declaration, just aliasing)
                    Python analogy: `from foo import Bar`

  `pub use foo::Bar;` — "bring Bar into scope AND re-export it as part of THIS module's public API"
                        Python analogy: `from foo import Bar` in __init__.py (making it top-level)

RUST QUIRK: `pub use types::*;`

The `*` glob re-export makes every public item in `types` available directly from `yo_core::`.
So users can write `use yo_core::AgentTool` instead of `use yo_core::types::AgentTool`.
This is a deliberate ergonomic choice — types are the most-used exports, so flattening them
to the crate root reduces import noise.

RUST QUIRK: `#[cfg(feature = "openapi")]`

Feature flags gate optional compilation. The `openapi` feature is listed in Cargo.toml under
`[features]`. Without it, the openapi module is completely absent from the compiled binary —
zero size, zero compile time. This is Rust's equivalent of optional dependencies / extras.

  Cargo.toml:  [features]  openapi = ["dep:utoipa"]
  Enable:      cargo build --features openapi
  In code:     #[cfg(feature = "openapi")] pub mod openapi;

Architecture summary: the `pub use` lines below define the "public API surface" of phi-core.
Everything a library user needs should be reachable without knowing internal module paths.
*/

pub mod agent_loop;
pub mod agents;
pub mod context;
pub mod mcp;
pub mod provider;
pub mod session;
pub mod tools;
pub mod types;
// retry.rs moved to provider/retry.rs
// skills.rs moved to context/skills.rs

// Feature-gated OpenAPI integration. Enabled with: cargo build --features openapi
#[cfg(feature = "openapi")]
pub mod openapi;

// Re-export the most-used types at the crate root for ergonomic imports.
// Users write `use phi_core::Agent` / `use phi_core::BasicAgent` instead of
// navigating internal module paths.
pub use agent_loop::evaluation::{
    ElaborateEvaluation, LlmJudgeEvaluation, PickFirstEvaluation, TokenEfficientEvaluation,
    TransparentEvaluation,
};
pub use agent_loop::{agent_loop, agent_loop_continue, agent_loop_parallel};
pub use agents::SubAgentTool;
pub use agents::{Agent, BasicAgent, QueueMode};
pub use context::skills::SkillSet;
pub use context::{
    build_context_from_session, compact_session_loops, BlockCompactionStrategy, CompactedSection,
    CompactionBlock, CompactionConfig, CompactionScope, CompactionStrategy, ContextConfig,
    ContextTracker, DefaultBlockCompaction, DefaultCompaction, TurnMap, TurnRange,
};
pub use provider::retry::RetryConfig;
pub use session::{
    delete_session, list_session_ids, load_session, load_sessions_for_agent, save_session,
    ChildLoopRef, LoopConfigSnapshot, LoopEvent, LoopRecord, LoopStatus, ParallelGroupRecord,
    Session, SessionError, SessionFormation, SessionRecorder, SessionRecorderConfig, SpawnRef,
};
pub use types::*; // glob re-export: ALL public items from types become top-level exports
