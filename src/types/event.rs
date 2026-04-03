use super::agent_message::AgentMessage;
use super::content::Message;
use super::tool::ToolResult;
use super::usage::Usage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Agent events (for streaming UI updates)
// ---------------------------------------------------------------------------

/// How an `agent_loop_continue` call relates to the session's prior loops.
///
/// Set on [`AgentContext::continuation_kind`] before calling `agent_loop_continue`,
/// and surfaced in [`AgentEvent::AgentStart`] so that observability consumers
/// (logs, UIs, analysis tools) can understand the session execution tree without
/// inspecting message content.
///
/// The runtime does **not** enforce context constraints — e.g. it does not verify
/// that a `Rerun` uses an identical context to the original loop. That is the caller's
/// responsibility. The distinction is purely semantic / for traceability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContinuationKind {
    /// Unspecified continuation — preserves the original `agent_loop_continue` semantics.
    /// Use when the Rerun/Branch distinction is not relevant to the caller.
    Default,
    /// Retry of the same scenario from an equivalent context state.
    ///
    /// The `tag` is an RFC 3339 UTC timestamp auto-generated at call time.
    /// Use for error recovery, rate-limit retries, or explicit re-runs.
    Rerun { tag: String },
    /// Exploration of a different execution path from a specific branching point.
    ///
    /// The `tag` is an RFC 3339 UTC timestamp auto-generated at call time.
    /// Caller should modify `context.messages` to set up the alternative path
    /// before calling `agent_loop_continue`. The first turn emits
    /// [`TurnTrigger::Branch`] instead of `FollowUp`.
    Branch { tag: String },
    /// A standalone context-compaction pass. No LLM call — messages only.
    Compaction,
}

/// Identifies what caused a new turn to begin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TurnTrigger {
    /// First turn triggered by a user message (`agent_loop`).
    User,
    /// This agent was invoked as a sub-agent by a parent agent.
    SubAgent,
    /// Continuation turn: tool round-trip, steering message, or `Default` / `Rerun` continuation.
    FollowUp,
    /// First turn of a `Branch` continuation (`agent_loop_continue` with `ContinuationKind::Branch`).
    /// Subsequent turns within the same branched loop use `FollowUp`.
    Branch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentEvent {
    /*********************** NOTEs ON AGENT EVENTS *************************
    AgentEvent is the runtime's event vocabulary — it captures all the significant happenings
    in the agent loop that the UI or other consumers might want to react to.
    This includes LLM-related events (message start/update/end, tool execution start/update/end)
    as well as more general events (agent start/end, turn start/end, input rejected).
    The key design principle is that AgentEvent should be comprehensive enough
    to capture all relevant state changes and milestones in the agent's operation,
    without being too tightly coupled to the specifics of any one LLM provider.

    ********************* SOME IMPROVEMENTS TO CONSIDER FOR AGENT EVENTS *************************
    1. More granular turn events: e.g., separate events for "LLM call start", "LLM call end",
    "tool calls start", "tool call end" within a turn, to give the UI more hooks for showing loading
    states, progress bars, etc.

    2. Error events: a dedicated AgentEvent variant for errors that occur during the agent loop
    (e.g., LLM call failures, tool execution errors) to allow the UI to react specifically
    to errors (e.g., show error messages, retry buttons).

    3. Input events: events for when user input is received, validated, rejected, etc.,
    to give the UI more hooks for showing input-related feedback.

    */
    /// Fires once when agent_loop() is entered — before any LLM call.
    /// UI use: show "thinking..." spinner, clear previous output, record start time.
    AgentStart {
        /// Stable identifier for this agent instance. Caller-supplied or auto-generated UUID v4.
        agent_id: String,
        /// Identifier for the current session. Groups related loops under one logical umbrella.
        /// Persists across multiple `agent_loop()` / `agent_loop_continue()` calls.
        session_id: String,
        /// Unique identifier for this specific loop call within the session.
        /// Format: `"{session_id}.{config_id}.{N}"` — encodes which config produced this loop.
        /// Auto-derived from provider + model + thinking_level when not explicitly set.
        loop_id: String,
        /// The `loop_id` of the loop this continues from. `None` for origin calls
        /// (`agent_loop()` with no prior session). Set for all `agent_loop_continue()` calls.
        parent_loop_id: Option<String>,
        /// How this continuation relates to prior loops. `None` for origin calls.
        /// `Some(Default|Rerun|Branch)` for `agent_loop_continue()` calls.
        continuation_kind: Option<ContinuationKind>,
        /// Wall-clock time when the agent loop was entered.
        timestamp: chrono::DateTime<chrono::Utc>,
        /// Extensible bag for caller-supplied context (e.g., user info, request tags, trace IDs).
        /// Passed through as-is — the agent loop does not read or modify this field.
        metadata: Option<serde_json::Value>,
    },

    /// Fires once when agent_loop() exits — after all turns and follow-ups complete.
    /// UI use: hide spinner, render final output, log token cost from the last assistant message.
    AgentEnd {
        /// Identifies which loop this event belongs to — matches `AgentStart.loop_id`.
        loop_id: String,
        /// All new messages added during this run (not the full history).
        /// Empty when `rejection` is `Some` — input was blocked before the LLM was called.
        messages: Vec<AgentMessage>,
        /// Total token usage accumulated across all turns in this run.
        /// All fields are 0 when `rejection` is `Some` (no LLM calls were made).
        usage: Usage,
        /// Wall-clock time when the agent loop exited.
        timestamp: chrono::DateTime<chrono::Utc>,
        /// `Some(reason)` when an InputFilter rejected the input before any LLM call.
        /// `None` on normal exit. When `Some`, `messages` will be empty.
        rejection: Option<String>,
    },

    /// Fires at the start of each LLM turn (one LLM call = one turn).
    /// A single agent run may have many turns: initial response + one per tool-call round-trip.
    /// UI use: show "waiting for LLM..." indicator between tool results and the next response.
    TurnStart {
        /// Identifies which loop this event belongs to — matches `AgentStart.loop_id`.
        loop_id: String,
        /// Zero-based index of this turn within the current agent run (0 = first turn after AgentStart).
        turn_index: u32,
        /// Wall-clock time when this turn began.
        timestamp: chrono::DateTime<chrono::Utc>,
        /// What caused this turn to begin. Distinguishes user-initiated turns from
        /// system continuations (tool round-trips, follow-ups, sub-agent invocations).
        triggered_by: TurnTrigger,
    },

    /// Fires at the end of each LLM turn, carrying the assistant message and all tool results.
    /// UI use: close the turn's loading indicator; `tool_results` can be shown as a collapsible block.
    TurnEnd {
        /// Identifies which loop this event belongs to — matches `AgentStart.loop_id`.
        loop_id: String,
        /// The assistant message produced this turn (may include text, thinking, and tool calls).
        message: AgentMessage,
        /// Token usage for this turn — direct access without destructuring `message`.
        /// Useful for per-turn cost tracking and rate-limit management.
        usage: Usage,
        /// Wall-clock time when this turn completed (after all tool calls finished executing).
        timestamp: chrono::DateTime<chrono::Utc>,
        /// Executed tool results from this turn. Empty when no tool calls were made (`StopReason::Stop`).
        /// These are the results fed back to the LLM — distinct from `message.content` which holds
        /// the LLM's tool call *requests*.
        tool_results: Vec<Message>,
    },

    /// Fires when a new message object is first created (before streaming content).
    /// For LLM assistant messages: fires when the SSE stream opens (StreamEvent::Start).
    /// For user/tool messages:      fires immediately (they're complete on creation).
    /// UI use: create a placeholder message bubble in the chat UI.
    MessageStart {
        loop_id: String,
        message: AgentMessage,
    },

    /// Fires for each streaming token/chunk as the LLM generates content.
    /// `delta` carries the incremental update — text token, thinking chunk, or tool-call JSON piece.
    /// `message` is the current accumulated state (useful if you join late and missed earlier deltas).
    /// UI use: append delta to the message bubble — this is what makes "typing" animations work.
    MessageUpdate {
        loop_id: String,
        message: AgentMessage,
        delta: StreamDelta,
    },

    /// Fires when a message is fully complete (all tokens received).
    /// `message` is the final, complete message — safe to persist or hand off.
    /// UI use: finalize the message bubble, enable copy/share actions.
    MessageEnd {
        loop_id: String,
        message: AgentMessage,
    },

    /// Fires when a tool call begins execution (before execute() is called).
    /// `args` is the raw JSON the LLM sent — useful for showing "what the agent is doing".
    /// UI use: show "[tool_name] running with args: ..." status line.
    ToolExecutionStart {
        loop_id: String,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },

    /// Fires when a tool streams a partial result mid-execution (via ctx.on_update).
    /// Not all tools emit these — only long-running tools that support streaming output.
    /// UI use: update a live output preview (e.g., bash command output as it streams in).
    ToolExecutionUpdate {
        loop_id: String,
        tool_call_id: String,
        tool_name: String,
        partial_result: ToolResult,
    },

    /// Fires when a tool finishes execution (success or error).
    /// `is_error: true` means the tool failed but the LLM will see the error and can retry/recover.
    /// `result.content` is what gets appended as Message::ToolResult into the conversation.
    /// UI use: mark the tool status as done/failed; show final output in the tool block.
    ToolExecutionEnd {
        loop_id: String,
        tool_call_id: String,
        tool_name: String,
        result: ToolResult,
        is_error: bool,
        /// Set to the child agent's `loop_id` when this tool spawned a sub-agent loop.
        /// `None` for all regular (non-sub-agent) tools.
        /// Enables parent→child traceability in the event stream without parsing tool result text.
        child_loop_id: Option<String>,
    },

    /// Fires when a tool emits a user-facing status string (via ctx.on_progress).
    /// Unlike ToolExecutionUpdate (structured partial results), this is plain text for display.
    /// Example: a bash tool might emit "Installing dependencies..." before the final output.
    /// UI use: show as a transient status line / toast notification under the tool block.
    ProgressMessage {
        loop_id: String,
        tool_call_id: String,
        tool_name: String,
        text: String,
    },

    /// Fires when an InputFilter rejects the user's message before the LLM is called.
    /// `reason` is the human-readable explanation from the filter (e.g., "PII detected").
    /// The agent loop returns immediately after emitting this — no LLM call is made.
    /// UI use: show an error banner; do NOT bubble this up as a normal message.
    InputRejected { loop_id: String, reason: String },

    /// Emitted by `agent_loop_parallel` before any branch is dispatched.
    /// `loop_ids` lists the assigned loop_id for each branch, in config order.
    /// UI use: show a "running N parallel branches" status indicator.
    ParallelLoopStart {
        session_id: String,
        loop_ids: Vec<String>,
        timestamp: DateTime<Utc>,
    },

    /// Emitted by `agent_loop_parallel` after evaluation selects a winning branch.
    /// `evaluation_usage` is the judge LLM's usage (zero if no judge was used).
    /// UI use: show which branch was selected; hide or collapse the non-selected branches.
    ParallelLoopEnd {
        session_id: String,
        selected_loop_id: String,
        selected_config_index: usize,
        evaluation_usage: Usage,
        timestamp: DateTime<Utc>,
    },

    /// Emitted immediately before a compaction strategy runs.
    /// Paired with `CompactionEnded`.
    CompactionStarted {
        loop_id: String,
        /// Estimated token count of the context before compaction.
        estimated_tokens: usize,
        /// Number of messages in context before compaction.
        message_count: usize,
        timestamp: DateTime<Utc>,
    },

    /// A prun tool removed in-run context.
    PrunApplied {
        loop_id: String,
        tokens_removed: usize,
        messages_removed: usize,
        memo: Option<String>,
        /// Timestamps of the pruned messages — enables session reconstruction on reload.
        pruned_timestamps: Vec<u64>,
        timestamp: DateTime<Utc>,
    },

    /// Emitted after compaction completes. Only emitted when compaction triggered
    /// (threshold was exceeded). Paired with `CompactionStarted`.
    CompactionEnded {
        loop_id: String,
        messages_before: usize,
        messages_after: usize,
        estimated_tokens_before: usize,
        estimated_tokens_after: usize,
        /// How many loops got new CompactionBlocks (including current).
        loops_compacted: usize,
        timestamp: DateTime<Utc>,
    },
}

/*
StreamDelta — incremental token-level updates from the LLM stream.

Why is this a separate enum from AgentEvent?
Because streaming is a different concern from messaging. A MessageUpdate carries:
  1. the partial message accumulator (so late subscribers can catch up), AND
  2. just the new delta (so efficient UIs only re-render the new bit)

Having a dedicated StreamDelta enum lets you switch in the UI:
  match delta {
      StreamDelta::Text { delta }        => append_text(delta),
      StreamDelta::Thinking { delta }    => append_thinking(delta),
      StreamDelta::ToolCallDelta { delta } => accumulate_tool_json(delta),
  }

StreamDelta::ToolCallDelta carries raw JSON fragments as the LLM streams tool
argument JSON (e.g., `{"path": "src/ma` ... `in.rs"}`). The receiver must
accumulate and parse these into a complete JSON value only after MessageEnd.
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamDelta {
    /// A text token fragment from the LLM's response.
    Text { delta: String },
    /// A thinking/reasoning chunk (extended thinking mode only).
    Thinking { delta: String },
    /// A fragment of the JSON arguments for a tool call (accumulate until MessageEnd).
    ToolCallDelta { delta: String },
}
