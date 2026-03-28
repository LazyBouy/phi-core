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

use crate::types::*;
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
}
