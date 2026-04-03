//! The `Agent` trait — the runtime interface for all agent implementations.
//!
//! This trait defines the core capabilities that any agent must provide:
//! prompting, state access, message management, and control. Builder methods
//! (configuration-time concerns) are intentionally excluded — each concrete
//! implementation provides its own builder API.
//!
//! # Implementations
//!
//! - [`BasicAgent`](super::BasicAgent) — the default in-memory implementation. Owns a single
//!   linear message history and runs the `agent_loop` directly.
//!
//! # Object Safety
//!
//! The trait is object-safe: methods use `String` (not `impl Into<String>`)
//! so `Box<dyn Agent>` and `&mut dyn Agent` work for runtime polymorphism.
//!
//! # Default Implementations
//!
//! - `prompt` / `prompt_messages` / `continue_loop` — delegate to the `_with_sender` variants.
//! - `prompt_with_sender` — wraps text in `AgentMessage::Llm(LlmMessage::new(Message::user(...)))`, calls
//!   `prompt_messages_with_sender`.
//! - Steering/follow-up queue methods — no-ops. Override to support mid-run interrupts.
//! - `last_loop_id` — returns `None`. Override if your impl tracks loop identity.

use crate::agent_loop::{
    AfterCompactionEndFn, AfterLoopFn, AfterToolExecutionFn, AfterToolExecutionUpdateFn,
    AfterTurnFn, AgentLoopConfig, BeforeCompactionStartFn, BeforeLoopFn, BeforeToolExecutionFn,
    BeforeToolExecutionUpdateFn, BeforeTurnFn, ConvertToLlmFn, TransformContextFn,
};
use crate::agents::AgentProfile;
use crate::context::{ContextConfig, ExecutionLimits};
use crate::provider::ModelConfig;
use crate::types::*;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Controls how messages are drained from the steering/follow-up queues per turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    /// Deliver one message per turn — allows the LLM to react to each steering message individually.
    OneAtATime,
    /// Deliver all queued messages at once — batches all pending steers into one turn.
    All,
}

/// The core runtime interface for an agent.
///
/// Programs against this trait to remain independent of the specific agent implementation.
/// Use [`BasicAgent`](super::BasicAgent) for the default in-memory implementation, or implement
/// this trait for richer implementations with persistence, branching, or distributed execution.
///
/// # Required Methods
///
/// The two primary required methods are `prompt_messages_with_sender` and
/// `continue_loop_with_sender` — all other prompting variants have default implementations
/// that delegate to these.
#[async_trait::async_trait]
pub trait Agent: Send {
    // ── Prompting (required) ─────────────────────────────────────────────────────

    /// Send messages as a prompt, streaming events to a caller-provided sender.
    ///
    /// This is the primary required prompting method — all other `prompt*` variants
    /// have default implementations that delegate here.
    async fn prompt_messages_with_sender(
        &mut self,
        messages: Vec<AgentMessage>,
        tx: mpsc::UnboundedSender<AgentEvent>,
    );

    /// Continue from current context, streaming events to a caller-provided sender.
    ///
    /// `kind` describes how this continuation relates to prior loops:
    /// - `Default` — unspecified continuation
    /// - `Rerun { tag }` — retry from the same context state
    /// - `Branch { tag }` — explore a different path from the same starting point
    async fn continue_loop_with_sender(
        &mut self,
        tx: mpsc::UnboundedSender<AgentEvent>,
        kind: ContinuationKind,
    );

    // ── Prompting (defaulted via _with_sender) ───────────────────────────────────

    /// Send a text prompt, streaming events to a caller-provided sender.
    ///
    /// Default: wraps `text` in `AgentMessage::Llm(LlmMessage::new(Message::user(text)))` and calls
    /// `prompt_messages_with_sender`.
    async fn prompt_with_sender(&mut self, text: String, tx: mpsc::UnboundedSender<AgentEvent>) {
        let msg = AgentMessage::Llm(LlmMessage::new(Message::user(text)));
        self.prompt_messages_with_sender(vec![msg], tx).await;
    }

    /// Send a text prompt. Returns a stream of `AgentEvent`s.
    ///
    /// Default: creates an internal channel and calls `prompt_with_sender`.
    async fn prompt(&mut self, text: String) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.prompt_with_sender(text, tx).await;
        rx
    }

    /// Send messages as a prompt. Returns a stream of `AgentEvent`s.
    ///
    /// Default: creates an internal channel and calls `prompt_messages_with_sender`.
    async fn prompt_messages(
        &mut self,
        messages: Vec<AgentMessage>,
    ) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.prompt_messages_with_sender(messages, tx).await;
        rx
    }

    /// Continue from current context. Returns a stream of `AgentEvent`s.
    ///
    /// Default: creates an internal channel and calls `continue_loop_with_sender(Default)`.
    async fn continue_loop(&mut self) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.continue_loop_with_sender(tx, ContinuationKind::Default)
            .await;
        rx
    }

    // ── State (required) ─────────────────────────────────────────────────────────

    /// Full message history.
    fn messages(&self) -> &[AgentMessage];

    /// Whether the agent is currently running a loop.
    fn is_streaming(&self) -> bool;

    /// Stable UUID assigned at construction; included in every `AgentStart` event.
    fn agent_id(&self) -> &str;

    /// Stable UUID assigned at construction; groups all loops from this instance.
    fn session_id(&self) -> &str;

    // ── State (defaulted) ────────────────────────────────────────────────────────

    /// The `loop_id` of the most recently started loop; `None` before first run.
    ///
    /// Default: returns `None`. Override to track loop identity.
    fn last_loop_id(&self) -> Option<&str> {
        None
    }

    // ── Message mutation (required) ──────────────────────────────────────────────

    /// Clear all messages from history.
    fn clear_messages(&mut self);

    /// Append a single message to history.
    fn append_message(&mut self, msg: AgentMessage);

    /// Replace the entire message history.
    fn replace_messages(&mut self, msgs: Vec<AgentMessage>);

    /// Serialize message history to JSON.
    fn save_messages(&self) -> Result<String, serde_json::Error>;

    /// Restore message history from JSON.
    fn restore_messages(&mut self, json: &str) -> Result<(), serde_json::Error>;

    /// Replace the tool set.
    fn set_tools(&mut self, tools: Vec<Arc<dyn AgentTool>>);

    // ── Control (required) ───────────────────────────────────────────────────────

    /// Cancel the current run via `CancellationToken`.
    fn abort(&self);

    /// Clear all state (messages, queues, streaming flag).
    fn reset(&mut self);

    // ── Steering/follow-up queues (defaulted — no-ops) ───────────────────────────

    /// Queue a steering message — interrupts the agent mid-tool-execution.
    ///
    /// Default: no-op. Override to support mid-run interrupts.
    fn steer(&self, _msg: AgentMessage) {}

    /// Queue a follow-up message — processed after the current agent turn completes.
    ///
    /// Default: no-op.
    fn follow_up(&self, _msg: AgentMessage) {}

    /// Clear all pending steering messages. Default: no-op.
    fn clear_steering_queue(&self) {}

    /// Clear all pending follow-up messages. Default: no-op.
    fn clear_follow_up_queue(&self) {}

    /// Clear both steering and follow-up queues.
    fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    /// Set how steering messages are delivered. Default: no-op.
    fn set_steering_mode(&mut self, _mode: QueueMode) {}

    /// Set how follow-up messages are delivered. Default: no-op.
    fn set_follow_up_mode(&mut self, _mode: QueueMode) {}

    // ── Configuration access (defaulted) ──────────────────────────────────

    /// The agent's profile blueprint. Default: `None`.
    fn profile(&self) -> Option<&AgentProfile> {
        None
    }

    /// The agent's system prompt. Default: empty string.
    fn system_prompt(&self) -> &str {
        ""
    }

    /// The agent's model configuration. Default: `None`.
    fn model_config(&self) -> Option<&ModelConfig> {
        None
    }

    /// The agent's thinking level. Default: `ThinkingLevel::Off`.
    fn thinking_level(&self) -> ThinkingLevel {
        ThinkingLevel::Off
    }

    /// The agent's temperature setting. Default: `None`.
    fn temperature(&self) -> Option<f32> {
        None
    }

    /// The agent's max tokens setting. Default: `None`.
    fn max_tokens(&self) -> Option<u32> {
        None
    }

    /// The agent's context config. Default: `None`.
    fn context_config(&self) -> Option<&ContextConfig> {
        None
    }

    /// The agent's execution limits. Default: `None`.
    fn execution_limits(&self) -> Option<&ExecutionLimits> {
        None
    }

    /// The agent's cache config. Default: `CacheConfig::default()`.
    fn cache_config(&self) -> CacheConfig {
        CacheConfig::default()
    }

    /// The agent's tool execution strategy. Default: `ToolExecutionStrategy::default()`.
    fn tool_execution(&self) -> ToolExecutionStrategy {
        ToolExecutionStrategy::default()
    }

    /// The agent's retry config. Default: `RetryConfig::default()`.
    fn retry_config(&self) -> crate::provider::retry::RetryConfig {
        crate::provider::retry::RetryConfig::default()
    }

    // ── Session (defaulted) ───────────────────────────────────────────────

    /// The agent's current session. Default: `None`.
    fn session(&self) -> Option<&crate::session::Session> {
        None
    }

    /// The agent's workspace directory. File paths in system prompt blocks
    /// resolve relative to this. Default: `None` (uses current directory).
    fn workspace(&self) -> Option<&Path> {
        None
    }

    // ── Hook setters (defaulted — no-ops) ─────────────────────────────────

    /// Set the before-turn hook. Default: no-op.
    fn set_before_turn(&mut self, _f: Option<BeforeTurnFn>) {}

    /// Set the after-turn hook. Default: no-op.
    fn set_after_turn(&mut self, _f: Option<AfterTurnFn>) {}

    /// Set the before-loop hook. Default: no-op.
    fn set_before_loop(&mut self, _f: Option<BeforeLoopFn>) {}

    /// Set the after-loop hook. Default: no-op.
    fn set_after_loop(&mut self, _f: Option<AfterLoopFn>) {}

    /// Set the before-tool-execution hook. Default: no-op.
    fn set_before_tool_execution(&mut self, _f: Option<BeforeToolExecutionFn>) {}

    /// Set the after-tool-execution hook. Default: no-op.
    fn set_after_tool_execution(&mut self, _f: Option<AfterToolExecutionFn>) {}

    /// Set the before-tool-execution-update hook. Default: no-op.
    fn set_before_tool_execution_update(&mut self, _f: Option<BeforeToolExecutionUpdateFn>) {}

    /// Set the after-tool-execution-update hook. Default: no-op.
    fn set_after_tool_execution_update(&mut self, _f: Option<AfterToolExecutionUpdateFn>) {}

    /// Set the convert-to-LLM function. Default: no-op.
    fn set_convert_to_llm(&mut self, _f: Option<ConvertToLlmFn>) {}

    /// Set the transform-context function. Default: no-op.
    fn set_transform_context(&mut self, _f: Option<TransformContextFn>) {}

    /// Set the block compaction strategy. Default: no-op.
    fn set_block_compaction_strategy(
        &mut self,
        _s: Option<Arc<dyn crate::context::BlockCompactionStrategy>>,
    ) {
    }

    /// Set the before-compaction-start hook (G1). Default: no-op.
    fn set_before_compaction_start(&mut self, _f: Option<BeforeCompactionStartFn>) {}

    /// Set the after-compaction-end hook (G1). Default: no-op.
    fn set_after_compaction_end(&mut self, _f: Option<AfterCompactionEndFn>) {}

    /// Enable or disable the prun tool. Default: no-op.
    fn set_prun_enabled(&mut self, _enabled: bool) {}

    /// Set the context translation strategy (G8). Default: no-op.
    fn set_context_translation(
        &mut self,
        _s: Option<Arc<dyn crate::provider::context_translation::ContextTranslationStrategy>>,
    ) {
    }

    /// Get the context translation strategy (G8). Default: None.
    fn context_translation(
        &self,
    ) -> Option<Arc<dyn crate::provider::context_translation::ContextTranslationStrategy>> {
        None
    }

    // ── Config assembly (defaulted) ───────────────────────────────────────

    /// Assemble an [`AgentLoopConfig`] from this agent's current settings.
    ///
    /// The default implementation builds a config from the trait's accessor methods.
    /// `BasicAgent` overrides this to additionally wire steering queues, hooks, and
    /// other implementation-specific state.
    ///
    /// # Panics
    ///
    /// Panics if `model_config()` returns `None`. Override `build_config()` or
    /// implement `model_config()` to avoid this.
    fn build_config(&self) -> AgentLoopConfig {
        let model_config = self
            .model_config()
            .expect(
                "build_config() requires model_config(); \
                 override build_config() or implement model_config()",
            )
            .clone();
        AgentLoopConfig {
            model_config,
            provider_override: None,
            thinking_level: self.thinking_level(),
            max_tokens: self.max_tokens(),
            temperature: self.temperature(),
            convert_to_llm: None,
            transform_context: None,
            get_steering_messages: None,
            get_follow_up_messages: None,
            context_config: self.context_config().cloned(),
            execution_limits: self.execution_limits().cloned(),
            cache_config: self.cache_config(),
            tool_execution: self.tool_execution(),
            retry_config: self.retry_config(),
            before_turn: None,
            after_turn: None,
            before_loop: None,
            after_loop: None,
            before_tool_execution: None,
            after_tool_execution: None,
            before_tool_execution_update: None,
            after_tool_execution_update: None,
            before_compaction_start: None,
            after_compaction_end: None,
            on_error: None,
            input_filters: vec![],
            first_turn_trigger: TurnTrigger::User,
            config_id: None,
            context_translation: self.context_translation(),
            prun_pending: None,
        }
    }
}
