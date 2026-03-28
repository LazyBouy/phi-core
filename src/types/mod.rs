//! Core type definitions for phi-core.
//!
//! This module is the dependency hub — imported by every other module via
//! `use crate::types::*`. All public items are re-exported at the module root
//! for backward compatibility.
//!
//! Sub-modules:
//! - [`content`] — Content enum, Message enum, StopReason, now_ms()
//! - [`extension`] — ExtensionMessage (non-LLM messages)
//! - [`agent_message`] — AgentMessage, LlmMessage, TurnId
//! - [`usage`] — Usage, CacheConfig, CacheStrategy, ThinkingLevel
//! - [`tool`] — AgentTool trait, ToolContext, ToolResult, ToolError, ToolExecutionStrategy
//! - [`event`] — AgentEvent, StreamDelta, ContinuationKind, TurnTrigger
//! - [`context`] — AgentContext
//! - [`parallel`] — ParallelLoopOutcome, ParallelLoopResult, InputFilter, EvaluationStrategy

pub mod agent_message;
pub mod content;
pub mod context;
pub mod event;
pub mod extension;
pub mod parallel;
pub mod tool;
pub mod usage;

// ── Glob re-exports ────────────────────────────────────────────────────────
// All public items re-exported so `use crate::types::*` continues to work.

pub use agent_message::*;
pub use content::*;
pub use context::*;
pub use event::*;
pub use extension::*;
pub use parallel::*;
pub use tool::*;
pub use usage::*;
