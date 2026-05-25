//! Context window management — smart truncation and token counting.
//!
//! This module provides:
//! - Token estimation (fast, no external deps)
//! - Tiered compaction (tool output truncation → turn summarization → full summary)
//! - Non-destructive compaction overlays (CompactionBlock on LoopRecord)
//! - Execution limits (max turns, tokens, duration)
//!
//! Sub-modules:
//! - [`token`] — Token estimation functions
//! - [`config`] — ContextConfig, CompactionConfig, CompactionScope
//! - [`tracker`] — ContextTracker (hybrid real+estimated tracking)
//! - [`compaction`] — CompactionBlock, CompactedSection, TurnRange, TurnMap
//! - [`strategy`] — CompactionStrategy, BlockCompactionStrategy, DefaultBlockCompaction
//! - [`compact_messages`] — Legacy tiered compaction (level 1/2/3)
//! - [`execution`] — ExecutionLimits, ExecutionTracker
//! - [`orchestration`] — compact_session_loops, build_context_from_session

pub mod compact_messages;
pub mod compaction;
pub mod config;
pub mod execution;
pub mod orchestration;
pub mod skills;
pub mod strategy;
pub mod token;
pub mod tracker;

// ── Re-exports ─────────────────────────────────────────────────────────────
// All public items re-exported so `use phi_core::context::Foo` continues to work.

pub use compact_messages::{compact_messages, compact_messages_with_counter};
pub use compaction::{CompactedSection, CompactionBlock, TurnMap, TurnRange};
pub use config::{CompactionConfig, CompactionScope, ContextConfig};
pub use execution::{CurrentToolExecution, ExecutionLimits, ExecutionTracker};
pub use orchestration::{build_context_from_session, compact_session_loops};
pub use skills::SkillSet;
pub use strategy::{
    BlockCompactionStrategy, CompactionStrategy, DefaultBlockCompaction, DefaultCompaction,
};
pub use token::{estimate_tokens, total_tokens, HeuristicTokenCounter, TokenCounter};
pub use tracker::ContextTracker;

// truncate_text_head_tail is pub(super) in compact_messages — available
// within context/ submodules (strategy.rs uses it). Not re-exported at crate root.

#[cfg(test)]
mod tests;
