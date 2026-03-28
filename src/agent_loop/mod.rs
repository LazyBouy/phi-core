//! Agent loop — the core execution engine for phi-core agents.
//!
//! This module is split into focused sub-modules:
//! - [`config`] — Hook type aliases and `AgentLoopConfig`
//! - [`core`] — Entry points: `agent_loop`, `agent_loop_continue`
//! - [`run`] — Core turn engine (`run_loop`)
//! - [`streaming`] — LLM response streaming
//! - [`tools`] — Tool execution pipeline
//! - [`parallel`] — Evaluational parallelism (`agent_loop_parallel`)
//! - [`helpers`] — Utilities (input filters, config derivation, etc.)

mod config;
mod core;
mod helpers;
mod parallel;
mod run;
mod streaming;
mod tools;

// ── Public re-exports ────────────────────────────────────────────────────────

// Hook types + config struct
pub use config::*;

// Primary entry points
pub use core::{agent_loop, agent_loop_continue};

// Parallel evaluation
pub use parallel::agent_loop_parallel;

// Internal utility — used by parallel.rs via super::, kept pub(crate) for future BasicAgent use
#[allow(unused_imports)]
pub(crate) use helpers::derive_config_segment;
