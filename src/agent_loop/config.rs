use crate::context::{CompactionStrategy, ContextConfig, ExecutionLimits};
use crate::provider::{ModelConfig, StreamProvider};
use crate::types::*;
use std::sync::Arc;

// ── Context transformation callbacks ────────────────────────────────────────
/// All hook types use `Arc` (shared ownership) so they can be cloned into closures
/// and stored without lifetime complications. `Box<dyn Fn>` would suffice for single-owner
/// cases but `Arc` makes it trivially cheap to share across async tasks.
/// Converts `AgentMessage[]` → `Message[]` before each LLM call.
pub type ConvertToLlmFn = Box<dyn Fn(&[AgentMessage]) -> Vec<Message> + Send + Sync>;
/// Transforms the full context before `convert_to_llm` (for pruning, reordering, injection).
pub type TransformContextFn = Box<dyn Fn(Vec<AgentMessage>) -> Vec<AgentMessage> + Send + Sync>;
/// Returns pending messages (steering interrupts or follow-up work) when polled.
pub type GetMessagesFn = Box<dyn Fn() -> Vec<AgentMessage> + Send + Sync>;

// ── Loop hooks ───────────────────────────────────────────────────────────────
/// Called once before the entire agent loop begins (before `AgentStart` is emitted).
///
/// Arguments: `(messages, loop_index)` — `messages` is the full context at the time of the call;
/// `loop_index` is always `0` (reserved for future multi-loop scenarios).
/// Return `false` to abort: `AgentEnd` is emitted immediately with an empty message list.
pub type BeforeLoopFn = Arc<dyn Fn(&[AgentMessage], usize) -> bool + Send + Sync>;
/// Called once after the entire agent loop ends (after `AgentEnd` is emitted).
///
/// Arguments: `(new_messages, accumulated_usage)` — `new_messages` are the messages produced
/// by this loop call; `accumulated_usage` sums input/output tokens across all turns.
pub type AfterLoopFn = Arc<dyn Fn(&[AgentMessage], &Usage) + Send + Sync>;

// ── Turn hooks ───────────────────────────────────────────────────────────────
/// Called before each LLM turn (before `TurnStart` is emitted).
///
/// Arguments: `(messages, turn_index)` — `messages` is the full context (steering messages
/// queued for *this* turn are not yet visible); `turn_index` is 0-based.
/// Return `false` to abort the turn: no `TurnStart`/`TurnEnd` events are emitted,
/// but `AgentEnd` still fires normally.
pub type BeforeTurnFn = Arc<dyn Fn(&[AgentMessage], usize) -> bool + Send + Sync>;
/// Called after each LLM turn (after `TurnEnd` is emitted).
///
/// Arguments: `(messages, turn_usage)` — `turn_usage` covers only this turn's tokens.
/// Fires on both the normal path and the error/abort path.
pub type AfterTurnFn = Arc<dyn Fn(&[AgentMessage], &Usage) + Send + Sync>;

// ── Tool execution hooks ─────────────────────────────────────────────────────
/// Called before each tool call (before `ToolExecutionStart` is emitted).
///
/// Arguments: `(tool_name, tool_call_id, args)`.
/// Return `false` to skip the call: an error `ToolResult` is synthesised so the LLM still
/// receives a response, but `ToolExecutionStart`/`End` are **not** emitted.
pub type BeforeToolExecutionFn = Arc<dyn Fn(&str, &str, &serde_json::Value) -> bool + Send + Sync>;
/// Called after each tool call (after `ToolExecutionEnd` is emitted).
///
/// Arguments: `(tool_name, tool_call_id, is_error)`.
pub type AfterToolExecutionFn = Arc<dyn Fn(&str, &str, bool) + Send + Sync>;
/// Called before each incremental tool update (before `ToolExecutionUpdate` is emitted).
///
/// Fires every time a tool calls `ctx.on_update(partial)` — potentially many times per call
/// (e.g. each line of bash output). Arguments: `(tool_name, tool_call_id, text_content)`.
/// Return `false` to suppress the streaming event; the tool keeps running and its final
/// `ToolResult` (what the LLM sees) is **unaffected**.
pub type BeforeToolExecutionUpdateFn = Arc<dyn Fn(&str, &str, &str) -> bool + Send + Sync>;
/// Called after each incremental tool update (after `ToolExecutionUpdate` is emitted).
///
/// Only fires when the update was *not* suppressed by `BeforeToolExecutionUpdateFn`.
/// Arguments: `(tool_name, tool_call_id, text_content)`.
pub type AfterToolExecutionUpdateFn = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Called when the LLM returns `StopReason::Error`. Argument: the error message string.
pub type OnErrorFn = Arc<dyn Fn(&str) + Send + Sync>;

/// All static settings for a single [`agent_loop`] / [`agent_loop_continue`] call.
///
/// Build with the public fields directly or via [`crate::agent::Agent`]'s builder methods.
/// The config is borrowed (`&AgentLoopConfig`) throughout the loop — it is never mutated.
///
/// ## Lifecycle hooks
///
/// All hook fields are `Option<Arc<dyn Fn(...)>>`. `None` means "no hook" (zero overhead).
/// See the module-level doc for the guaranteed ordering relative to [`AgentEvent`]s.
pub struct AgentLoopConfig {
    /// Complete provider identity: model id, api_key, base_url, protocol, compat flags, cost rates.
    /// The agent loop resolves the concrete `StreamProvider` from `model_config.api` via
    /// `ProviderRegistry`. Set `provider_override` to bypass the registry for custom providers.
    pub model_config: ModelConfig,

    /// Custom provider override. When `Some`, bypasses `ProviderRegistry` dispatch and uses
    /// this provider directly. Useful for testing (`MockProvider`) or custom implementations.
    /// When `None` (the default), the provider is resolved from `model_config.api`.
    pub provider_override: Option<Arc<dyn StreamProvider>>,

    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,

    /// Convert AgentMessage[] → Message[] before each LLM call.
    /// Default: keep only LLM-compatible messages.
    pub convert_to_llm: Option<ConvertToLlmFn>,

    /// Transform context before convert_to_llm (for pruning, compaction).
    pub transform_context: Option<TransformContextFn>,

    /// Get steering messages (user interruptions mid-run).
    pub get_steering_messages: Option<GetMessagesFn>,

    /// Get follow-up messages (queued work after agent finishes).
    pub get_follow_up_messages: Option<GetMessagesFn>,

    /// Context window configuration (auto-compaction).
    pub context_config: Option<ContextConfig>,

    /// Custom in-memory compaction strategy. When set, replaces the default
    /// `compact_messages()` call. Used when `AgentContext.session` is `None`.
    pub compaction_strategy: Option<Arc<dyn CompactionStrategy>>,

    /// Block-based compaction strategy for Session-aware compaction.
    /// Only used when `AgentContext.session` is `Some`. When `None`, falls back
    /// to `DefaultBlockCompaction`.
    pub block_compaction_strategy: Option<Arc<dyn crate::context::BlockCompactionStrategy>>,

    /// Execution limits (max turns, tokens, duration, cost).
    /// Cost is tracked automatically using `model_config.cost` rates after each turn.
    /// `ExecutionLimits.max_cost` enforcement is active whenever rates are non-zero.
    pub execution_limits: Option<ExecutionLimits>,

    /// Prompt caching configuration.
    pub cache_config: CacheConfig, //from types.rs

    /// Tool execution strategy (sequential, parallel, or batched).
    pub tool_execution: ToolExecutionStrategy, // from types.rs

    /// Retry configuration for transient provider errors.
    pub retry_config: crate::retry::RetryConfig,

    //******* Callbacks Turn *******
    /// Called before each LLM turn. Return `false` to abort the turn.
    pub before_turn: Option<BeforeTurnFn>,
    /// Called after each LLM turn with the current messages and the turn's usage.
    pub after_turn: Option<AfterTurnFn>,

    //******* Callbacks Loop *******
    /// Called before each Agent loop. Return `false` to abort the loop.
    pub before_loop: Option<BeforeLoopFn>,
    /// Called after each Agent loop with the current messages and the loop's usage.
    pub after_loop: Option<AfterLoopFn>,

    //******* Callbacks Tool Execution *******
    /// Called before each tool execution. Return `false` to skip the tool call.
    pub before_tool_execution: Option<BeforeToolExecutionFn>,
    /// Called after each tool execution.
    pub after_tool_execution: Option<AfterToolExecutionFn>,
    /// Called before each ToolExecutionUpdate event. Return `false` to suppress the event.
    pub before_tool_execution_update: Option<BeforeToolExecutionUpdateFn>,
    /// Called after each ToolExecutionUpdate event.
    pub after_tool_execution_update: Option<AfterToolExecutionUpdateFn>,

    /// Called when the LLM returns a `StopReason::Error`.
    pub on_error: Option<OnErrorFn>,

    /// Input filters applied to user messages before the LLM call.
    /// Filters run in order; first `Reject` wins and discards any accumulated
    /// warnings. `Warn` messages accumulate and are appended to the user message.
    pub input_filters: Vec<Arc<dyn InputFilter>>, // from types.rs

    /// The trigger type for the first TurnStart event in this run.
    /// Defaults to `TurnTrigger::User`; set to `SubAgent` by sub-agent callers.
    pub first_turn_trigger: TurnTrigger,

    /// Stable identity for this config, used as the middle segment of `loop_id`:
    ///   `loop_id = "{session_id}.{config_id}.{N}"`
    ///
    /// When `None` and the `Agent` wrapper is used, the identity is auto-derived by
    /// `Agent::next_loop_id()` from the provider, model, and thinking level:
    ///   `"{provider_id}.{model_slug}[.thinking]"`
    ///
    /// For direct callers of `agent_loop`, set `context.loop_id` explicitly — this field
    /// is only read by `Agent::next_loop_id()` and has no effect inside `agent_loop` itself.
    ///
    /// Set explicitly for human-readable or deterministic loop IDs, e.g.:
    ///   `config.config_id = Some("experiment-A".to_string());`
    ///   → loop IDs: `ses_xyz.experiment-A.1`, `ses_xyz.experiment-A.2`, …
    pub config_id: Option<String>,
}
