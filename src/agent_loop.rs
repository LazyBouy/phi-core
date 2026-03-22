//! The core agent loop: prompt → LLM stream → tool execution → repeat.
//!
//! This is the heart of phi-core. Two public entry points share the same inner logic:
//!
//! - [`agent_loop`] — starts a fresh run with new prompt messages
//! - [`agent_loop_continue`] — resumes from existing context (retries, branching)
//!
//! Both return the *new* [`AgentMessage`]s produced by this call. Real-time progress is
//! pushed to the caller-supplied `tx: mpsc::UnboundedSender<AgentEvent>` channel.
//!
//! # Hook / Event Ordering Invariant - Guaranteed by design, critical for predictable integrations
//!
//! Every run enforces this strict ordering, regardless of how many turns or tool calls occur:
//!
//! ```text
//! before_loop → AgentStart
//!   before_turn → TurnStart
//!     [MessageStart/End for initial prompts — first turn of agent_loop() only]
//!     [MessageStart/End for injected steering messages]
//!     [LLM: MessageStart → MessageUpdate* → MessageEnd]
//!     [per tool call:]
//!       before_tool_execution → ToolExecutionStart
//!         (before_tool_execution_update → ToolExecutionUpdate → after_tool_execution_update)*
//!       ToolExecutionEnd → after_tool_execution
//!   TurnEnd → after_turn
//!   (repeat inner block for each follow-up / steering-triggered turn)
//! AgentEnd → after_loop
//! ```
//!
//! Hooks returning `false` short-circuit: `before_loop` aborts before `AgentStart` is emitted;
//! `before_turn` skips the turn without emitting `TurnStart`/`TurnEnd`;
//! `before_tool_execution` skips the tool call without emitting `ToolExecutionStart`/`End`.

use crate::context::{
    CompactionStrategy, ContextConfig, DefaultCompaction, ExecutionLimits, ExecutionTracker,
};

use crate::provider::{ModelConfig, StreamConfig, StreamEvent, StreamProvider, ToolDefinition};
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
use tokio::sync::mpsc;
use tracing::warn;

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
    pub provider: Arc<dyn StreamProvider>,
    pub model: String,
    pub api_key: String,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,

    /// Optional model configuration for multi-provider support.
    /// When set, passed through to StreamConfig so providers can use
    /// base_url, headers, compat flags, etc.
    pub model_config: Option<ModelConfig>,

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

    /// Custom compaction strategy. When set, replaces the default
    /// `compact_messages()` call. Only invoked when `context_config` is `Some`.
    pub compaction_strategy: Option<Arc<dyn CompactionStrategy>>,

    /// Execution limits (max turns, tokens, duration).
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

/// Default convert_to_llm: keep only user/assistant/toolResult messages.
fn default_convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|m| m.as_llm().cloned())
        .collect()
}

/*
DESIGN: Why agent_loop takes these separate parameters — each plays a different role:
  `prompts`  = NEW INPUT    — the messages being added THIS call (taken by value; appended to
                              context; also emitted as MessageStart/End inside the first TurnStart)
  `context`  = ACCUMULATOR  — the full conversation history (system prompt + all past turns);
                              mutated in-place as the loop runs each turn
  `config`   = STATIC       — model, callbacks, limits; never changes within a single call
  `tx`       = OBSERVER     — channel to push real-time AgentEvents to external callers (UI, logger)
  `cancel`   = ABORT SIGNAL — cooperative cancellation; any code holding this token can stop the loop

Why return Vec<AgentMessage> (not the whole context)?
The caller already holds `context` via the `&mut` reference. Returning only the NEW messages
from this call avoids duplicating the entire history — the caller can append to their own copy.
*/
/// Start an agent loop with new prompt messages.
///
/// Appends `prompts` to `context`, runs the full hook/event lifecycle (see module doc),
/// and returns only the messages produced by this call. Events are pushed to `tx` in real time.
pub async fn agent_loop(
    prompts: Vec<AgentMessage>, // NEW INPUT — added to context and emitted inside first TurnStart
    context: &mut AgentContext, // ACCUMULATOR — full history; grows in-place each turn
    config: &AgentLoopConfig, // STATIC SETTINGS — model, tools, callbacks; unchanged during the loop
    tx: mpsc::UnboundedSender<AgentEvent>, // OBSERVER — taken by value; all AgentEvents pushed here
    cancel: tokio_util::sync::CancellationToken, // ABORT — checked between every major step; child tokens for tools
) -> Vec<AgentMessage> {
    // before_loop hook — fires before AgentStart; false aborts the entire loop
    if let Some(ref before_loop) = config.before_loop {
        if !before_loop(&context.messages, 0) {
            tx.send(AgentEvent::AgentEnd {
                messages: vec![],
                timestamp: chrono::Utc::now(),
                rejection: None,
            })
            .ok();
            return vec![];
        }
    }

    // Generate agent_id / session_id if not set by the caller, then write them back to context
    // so any subsequent agent_loop_continue() call on the same context inherits them automatically.
    if context.agent_id.is_none() {
        context.agent_id = Some(uuid::Uuid::new_v4().to_string());
    }
    if context.session_id.is_none() {
        context.session_id = Some(uuid::Uuid::new_v4().to_string());
    }
    // loop_id: use caller-supplied value or fall back to a UUID (Agent sets this via next_loop_id).
    if context.loop_id.is_none() {
        context.loop_id = Some(uuid::Uuid::new_v4().to_string());
    }

    tx.send(AgentEvent::AgentStart {
        agent_id: context.agent_id.clone().unwrap(), // safe: just set above
        session_id: context.session_id.clone().unwrap(), // safe: just set above
        loop_id: context.loop_id.clone().unwrap(),   // safe: just set above
        parent_loop_id: context.parent_loop_id.clone(), // None for origin calls
        continuation_kind: context.continuation_kind.clone(), // None for origin calls
        timestamp: chrono::Utc::now(),
        metadata: None,
    })
    .ok();

    // !!!SECURITY!!!: Apply input filters before adding prompts to context
    let prompts = if !config.input_filters.is_empty() {
        let user_text: String = prompts
            .iter()
            .filter_map(|m| {
                if let AgentMessage::Llm(Message::User { content, .. }) = m {
                    Some(
                        content
                            .iter()
                            .filter_map(|c| {
                                /*
                                The meaning of the below if let is:
                                "If this content c is a Text variant, extract the text string; otherwise skip it".
                                Python equivalent would be:
                                if isinstance(c, Content) and c.variant == "Text":
                                    text = c.text

                                This is "if let Pattern = value → pattern match + destructure + bind" in one shot
                                For image we could have done:

                                if let Content::Image { data, .. } = c {
                                    Some(data.as_str())  // data is already bound, no reassignment needed
                                }

                                or

                                if let Content::Image { data, mime_type } = c {
                                    Some(format!("data:{};base64,{}", mime_type, data))
                                }

                                it produces: data:image/png;base64,ABC123...

                                Other return options:
                                // Tuple — simplest
                                Some((data.as_str(), mime_type.as_str()))

                                // Struct — most expressive
                                Some(ImageContent { data, mime_type })

                                // String — if you just need serialized form
                                Some(format!("{}:{}", mime_type, data))
                                */
                                if let Content::Text { text } = c {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let mut warnings: Vec<String> = Vec::new();
        for filter in &config.input_filters {
            match filter.filter(&user_text) {
                FilterResult::Pass => {}
                FilterResult::Warn(w) => warnings.push(w),
                FilterResult::Reject(reason) => {
                    tx.send(AgentEvent::InputRejected {
                        reason: reason.clone(),
                    })
                    .ok();

                    // Rejection: emit InputRejected (already sent above) then AgentEnd with
                    // rejection reason so callers can distinguish a filter abort from a normal end.
                    tx.send(AgentEvent::AgentEnd {
                        messages: vec![],
                        timestamp: chrono::Utc::now(),
                        rejection: Some(reason.clone()),
                    })
                    .ok();
                    return vec![];
                }
            }
        }

        // Append warnings to the last user message's content (avoids consecutive user messages)
        if !warnings.is_empty() {
            let warning_text = warnings
                .iter()
                .map(|w| format!("[Warning: {}]", w))
                .collect::<Vec<_>>()
                .join("\n");

            let mut modified = prompts;
            // Find and extend the last user message
            for msg in modified.iter_mut().rev() {
                // .rev() : iterate in reverse to find the last user message
                if let AgentMessage::Llm(Message::User { content, .. }) = msg {
                    content.push(Content::Text { text: warning_text });
                    break;
                }
            }
            modified
        } else {
            prompts
        }
    } else {
        prompts
    };

    let mut new_messages: Vec<AgentMessage> = prompts.clone();

    // Add prompts to context
    for prompt in &prompts {
        context.messages.push(prompt.clone());
    }

    // Run the main loop (streaming + tools + steering + limits + callbacks)
    let loop_usage = run_loop(
        context,
        &mut new_messages,
        config,
        &tx,
        &cancel,
        Some(&prompts),
    )
    .await;

    tx.send(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
        timestamp: chrono::Utc::now(),
        rejection: None,
    })
    .ok();
    // after_loop hook — fires after AgentEnd
    if let Some(ref after_loop) = config.after_loop {
        after_loop(&new_messages, &loop_usage);
    }
    new_messages
}

/*
DESIGN: agent_loop_continue vs agent_loop
Unlike agent_loop, this takes NO `prompts` — the conversation already exists in `context`.
Used for retries and session-branching scenarios where the caller has already appended messages
(or queued them via steering/follow-up callbacks) and simply wants to resume execution.
No TurnStart/MessageStart events for prior context are re-emitted — the loop starts at turn 0
from whatever state context.messages is in.
*/
/// Resume an agent loop from existing context without new prompts.
///
/// Use for retries, session branching, or re-runs from a specific point. The context must be
/// non-empty and must not end with an assistant message. New follow-up/steering messages can
/// be injected via `config.get_follow_up_messages` / `config.get_steering_messages`.
///
/// Returns only the messages produced by this continuation call.
pub async fn agent_loop_continue(
    context: &mut AgentContext, // ACCUMULATOR — existing history (must be non-empty, not end on assistant)
    config: &AgentLoopConfig,   // STATIC SETTINGS — same config as the original call
    tx: mpsc::UnboundedSender<AgentEvent>, // OBSERVER — all AgentEvents pushed here
    cancel: tokio_util::sync::CancellationToken, // ABORT — fresh or shared token for this continuation
) -> Vec<AgentMessage> {
    // Identity must carry over from the originating loop. These are set by Agent::prompt_*
    // (or by the direct caller who bootstrapped the session). Silent UUID generation here
    // would mean every continuation gets a different identity — breaking ancestry tracking.
    assert!(
        context.agent_id.is_some(),
        "agent_loop_continue requires context.agent_id to be set — \
         identity must carry over from the originating loop"
    );
    assert!(
        context.session_id.is_some(),
        "agent_loop_continue requires context.session_id to be set — \
         the session must be established before a continuation"
    );

    assert!(
        !context.messages.is_empty(),
        "Cannot continue: no messages in context"
    );

    // LLM APIs require strict alternation: user → assistant → user → assistant → …
    // The conversation must end on a "user" or "tool_result" message so the model
    // has something to respond to.
    //
    // An assistant message as the final entry means the model already had its turn
    // and the loop is in a "finished" state. Resuming from here would send the API
    // a second consecutive assistant message with no user prompt — either a protocol
    // error or a semantically broken exchange.
    //
    // Valid resume states:  user message (awaiting a reply)
    //                       tool_result  (tools finished; model needs to process results)
    // Invalid resume state: assistant    (already responded; nothing new to react to)
    if let Some(last) = context.messages.last() {
        assert!(
            last.role() != "assistant",
            "Cannot continue from assistant message"
        );
    }

    let mut new_messages: Vec<AgentMessage> = Vec::new();

    // before_loop hook — fires before AgentStart; false aborts the entire loop
    if let Some(ref before_loop) = config.before_loop {
        if !before_loop(&context.messages, 0) {
            tx.send(AgentEvent::AgentEnd {
                messages: vec![],
                timestamp: chrono::Utc::now(),
                rejection: None,
            })
            .ok();
            return vec![];
        }
    }

    // loop_id: use caller-supplied value (Agent sets this via next_loop_id) or fall back to UUID.
    if context.loop_id.is_none() {
        context.loop_id = Some(uuid::Uuid::new_v4().to_string());
    }

    tx.send(AgentEvent::AgentStart {
        agent_id: context.agent_id.clone().unwrap(), // safe: asserted above
        session_id: context.session_id.clone().unwrap(), // safe: asserted above
        loop_id: context.loop_id.clone().unwrap(),   // safe: just set above
        parent_loop_id: context.parent_loop_id.clone(), // set by Agent wrapper
        continuation_kind: context.continuation_kind.clone(), // set by Agent wrapper
        timestamp: chrono::Utc::now(),
        metadata: None,
    })
    .ok();

    let loop_usage = run_loop(context, &mut new_messages, config, &tx, &cancel, None).await;

    tx.send(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
        timestamp: chrono::Utc::now(),
        rejection: None,
    })
    .ok();
    // after_loop hook — fires after AgentEnd
    if let Some(ref after_loop) = config.after_loop {
        after_loop(&new_messages, &loop_usage);
    }
    new_messages
}

/// Core loop shared by [`agent_loop`] and [`agent_loop_continue`]. Never called directly.
///
/// **Outer loop** — repeats when `get_follow_up_messages` returns work after the agent would stop.
/// **Inner loop** — repeats when the LLM requests tool calls or steering messages arrive mid-turn.
///
/// Per-turn event ordering (enforced every iteration):
/// `before_turn` → `TurnStart` → [prompt/steering messages] → [LLM stream] → [tools] → `TurnEnd` → `after_turn`
///
/// Returns accumulated [`Usage`] across all turns so the caller can pass it to `after_loop`.
async fn run_loop(
    context: &mut AgentContext, // ACCUMULATOR — all messages (grows as turns complete)
    new_messages: &mut Vec<AgentMessage>, // RESULT COLLECTOR — only messages added in this call
    config: &AgentLoopConfig,   // STATIC SETTINGS — unchanged for lifetime of this call
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — borrowed; cloned into tool closures as needed
    cancel: &tokio_util::sync::CancellationToken, // ABORT — borrowed; child tokens derived per tool
    first_turn_prompts: Option<&[AgentMessage]>, // Initial prompts (agent_loop only); None for agent_loop_continue
) -> Usage {
    let mut first_turn = true;
    let mut turn: usize = 0; // single counter: passed to hooks (as usize) and TurnStart events (as u32)
    let mut loop_usage = Usage::default(); // accumulated usage across all turns, returned for after_loop
    let mut tracker = config
        .execution_limits
        .as_ref()
        .map(|limits| ExecutionTracker::new(limits.clone()));

    // Check for steering messages at start
    /*
    The Rust chain reads as: "if get_steering_messages exists on config,
    call it and get its messages; otherwise give me an empty Vec" .
    The .as_ref() avoids consuming the function, .map(|f| f()) calls it only
    if present, and .unwrap_or_default() substitutes an empty Vec
    if it was None.
     */
    let mut pending: Vec<AgentMessage> = config
        .get_steering_messages
        .as_ref()
        .map(|f| f())
        .unwrap_or_default();

    // Outer loop: follow-ups after agent would stop
    loop {
        //loop {} is Rust's infinite loop construct, equivalent to Python's while True:
        if cancel.is_cancelled() {
            return loop_usage;
        }

        let mut steering_after_tools: Option<Vec<AgentMessage>> = None;

        // Inner loop: runs at least once, then continues if tool calls or pending messages
        loop {
            if cancel.is_cancelled() {
                return loop_usage;
            }

            // Determine the trigger type for this turn's TurnStart event.
            //
            // Priority on the first turn:
            //   1. Branch continuation   → TurnTrigger::Branch   (explicit branch signal)
            //   2. Any other continuation (Default/Rerun) → TurnTrigger::FollowUp
            //      (the continuation itself is the follow-up, not a fresh user turn)
            //   3. Origin call (continuation_kind == None) → config.first_turn_trigger
            //      (User for Agent::prompt, SubAgent for sub-agent callers)
            //
            // Subsequent turns always use FollowUp (tool round-trips, steering injections).
            let turn_trigger = if first_turn {
                if matches!(
                    context.continuation_kind,
                    Some(ContinuationKind::Branch { .. })
                ) {
                    TurnTrigger::Branch
                } else if context.continuation_kind.is_some() {
                    // Default or Rerun continuation — treat as a follow-up, not a new user turn
                    TurnTrigger::FollowUp
                } else {
                    config.first_turn_trigger.clone()
                }
            } else {
                TurnTrigger::FollowUp
            };

            // Check execution limits BEFORE before_turn so we don't fire hooks for an impossible turn
            if let Some(ref tracker) = tracker {
                if let Some(reason) = tracker.check_limits() {
                    warn!("Execution limit reached: {}", reason);
                    let limit_msg = AgentMessage::Llm(Message::User {
                        content: vec![Content::Text {
                            text: format!("[Agent stopped: {}]", reason),
                        }],
                        timestamp: now_ms(),
                    });
                    tx.send(AgentEvent::MessageStart {
                        message: limit_msg.clone(),
                    })
                    .ok();
                    tx.send(AgentEvent::MessageEnd {
                        message: limit_msg.clone(),
                    })
                    .ok();
                    context.messages.push(limit_msg.clone());
                    new_messages.push(limit_msg);
                    return loop_usage;
                }
            }

            // before_turn hook — fires BEFORE TurnStart; false aborts this turn
            if let Some(ref before_turn) = config.before_turn {
                if !before_turn(&context.messages, turn) {
                    return loop_usage;
                }
            }

            // TurnStart — fires AFTER before_turn hook
            tx.send(AgentEvent::TurnStart {
                turn_index: turn as u32,
                timestamp: chrono::Utc::now(),
                triggered_by: turn_trigger,
            })
            .ok();

            let was_first_turn = first_turn;
            if first_turn {
                first_turn = false;
            }
            turn += 1;

            // On the first turn of agent_loop(), emit events for the initial prompt messages
            // (these are after TurnStart so they appear inside the turn in the event stream)
            if was_first_turn {
                if let Some(prompts) = first_turn_prompts {
                    for prompt in prompts {
                        tx.send(AgentEvent::MessageStart {
                            message: prompt.clone(),
                        })
                        .ok();
                        tx.send(AgentEvent::MessageEnd {
                            message: prompt.clone(),
                        })
                        .ok();
                    }
                }
            }

            // Inject pending steering/follow-up messages (after TurnStart — they are part of this turn)
            if !pending.is_empty() {
                for msg in pending.drain(..) {
                    tx.send(AgentEvent::MessageStart {
                        message: msg.clone(),
                    })
                    .ok();
                    tx.send(AgentEvent::MessageEnd {
                        message: msg.clone(),
                    })
                    .ok();
                    context.messages.push(msg.clone());
                    new_messages.push(msg);
                }
            }

            // Compact context if configured (tiered: tool outputs → summarize → drop)
            if let Some(ref ctx_config) = config.context_config {
                let strategy: &dyn CompactionStrategy = config
                    .compaction_strategy
                    .as_deref()
                    .unwrap_or(&DefaultCompaction);
                context.messages =
                    strategy.compact(std::mem::take(&mut context.messages), ctx_config);
            }

            // Stream assistant response
            let message = stream_assistant_response(context, config, tx, cancel).await;

            let agent_msg: AgentMessage = message.clone().into();
            context.messages.push(agent_msg.clone());
            new_messages.push(agent_msg.clone());

            // Check for error/abort
            if let Message::Assistant {
                ref stop_reason,
                ref error_message,
                ref usage,
                ..
            } = message
            {
                if *stop_reason == StopReason::Error || *stop_reason == StopReason::Aborted {
                    if *stop_reason == StopReason::Error {
                        if let Some(ref on_error) = config.on_error {
                            let err_str = error_message.as_deref().unwrap_or("Unknown error");
                            on_error(err_str);
                        }
                    }
                    // Accumulate usage into loop total
                    loop_usage.input += usage.input;
                    loop_usage.output += usage.output;
                    loop_usage.cache_read += usage.cache_read;
                    loop_usage.cache_write += usage.cache_write;
                    loop_usage.total_tokens += usage.total_tokens;
                    // TurnEnd fires BEFORE after_turn
                    tx.send(AgentEvent::TurnEnd {
                        message: agent_msg,
                        timestamp: chrono::Utc::now(),
                        tool_results: vec![],
                    })
                    .ok();
                    // after_turn hook fires AFTER TurnEnd
                    if let Some(ref after_turn) = config.after_turn {
                        after_turn(&context.messages, usage);
                    }
                    return loop_usage;
                }
            }

            // Extract tool calls
            let tool_calls: Vec<_> = match &message {
                Message::Assistant { content, .. } => content
                    .iter()
                    .filter_map(|c| match c {
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                        } => Some((id.clone(), name.clone(), arguments.clone())),
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            };

            let has_tool_calls = !tool_calls.is_empty();
            let mut tool_results: Vec<Message> = Vec::new();

            if has_tool_calls {
                let execution = execute_tool_calls(
                    &context.tools,
                    &tool_calls,
                    tx,
                    cancel,
                    config.get_steering_messages.as_ref(),
                    &config.tool_execution,
                    config,
                )
                .await;

                tool_results = execution.tool_results;
                steering_after_tools = execution.steering_messages;

                for result in &tool_results {
                    let am: AgentMessage = result.clone().into();
                    context.messages.push(am.clone());
                    new_messages.push(am);
                }
            }

            // Extract turn usage for accumulation and hooks
            let turn_usage = match &message {
                Message::Assistant { usage, .. } => usage.clone(),
                _ => Usage::default(),
            };

            // Track turn for execution limits
            if let Some(ref mut tracker) = tracker {
                let turn_tokens = (turn_usage.input + turn_usage.output) as usize;
                tracker.record_turn(turn_tokens);
            }

            // Accumulate usage into loop total
            loop_usage.input += turn_usage.input;
            loop_usage.output += turn_usage.output;
            loop_usage.cache_read += turn_usage.cache_read;
            loop_usage.cache_write += turn_usage.cache_write;
            loop_usage.total_tokens += turn_usage.total_tokens;

            // TurnEnd fires BEFORE after_turn
            tx.send(AgentEvent::TurnEnd {
                message: agent_msg,
                timestamp: chrono::Utc::now(),
                tool_results,
            })
            .ok();

            // after_turn hook fires AFTER TurnEnd
            if let Some(ref after_turn) = config.after_turn {
                after_turn(&context.messages, &turn_usage);
            }

            // Check steering after turn
            if let Some(steering) = steering_after_tools.take() {
                if !steering.is_empty() {
                    pending = steering;
                    continue;
                }
            }

            pending = config
                .get_steering_messages
                .as_ref()
                .map(|f| f())
                .unwrap_or_default();

            // Exit inner loop if no more tool calls and no pending messages
            if !has_tool_calls && pending.is_empty() {
                break;
            }
        }

        // Agent would stop. Check for follow-ups.
        let follow_ups = config
            .get_follow_up_messages
            .as_ref()
            .map(|f| f())
            .unwrap_or_default();

        if !follow_ups.is_empty() {
            pending = follow_ups;
            continue;
        }

        break;
    }
    loop_usage
}

/*
stream_assistant_response — the core LLM call.

This function does three things:
  1. Prepares the payload (context transform → LLM message conversion → tool definitions)
  2. Calls provider.stream() in a retry loop for transient failures
  3. Drains the event channel and re-emits events as AgentEvents for the UI

ARCHITECTURE NOTE: Dual-output design of provider.stream()

provider.stream() has an unusual dual-output pattern:
  - It takes a `stream_tx: mpsc::UnboundedSender<StreamEvent>` (push-based, fires during streaming)
  - It returns `Result<Message, ProviderError>` (pull-based, available after await completes)

Why both? Because SSE streaming and HTTP completion are sequential:
  a) SSE events arrive token-by-token (we push them into stream_tx for the UI)
  b) The final complete Message is only available when the stream ends (returned as Result)

The UI reads from stream_rx (the receiving end of the channel) while the provider
pushes into stream_tx. This decouples the UI rendering from the HTTP layer.

*/
/// Stream an assistant response from the LLM.
async fn stream_assistant_response(
    context: &AgentContext, // READ-ONLY — converts messages for LLM but never mutates context
    config: &AgentLoopConfig, // SETTINGS — model, system prompt, cache; used to build StreamConfig
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — re-emits StreamEvents as AgentEvents for the caller
    cancel: &tokio_util::sync::CancellationToken, // ABORT — forwarded to provider.stream(); cloned as provider_cancel
) -> Message {
    // complete LLM response (all content blocks assembled); synthetic error Message on failure
    // Apply context transform (optional hook to prune/reshape messages before LLM sees them)
    let messages = if let Some(transform) = &config.transform_context {
        transform(context.messages.clone())
    } else {
        context.messages.clone()
    };

    // Convert AgentMessage[] → Message[]: strip Extension messages, keep only LLM-visible ones.
    // This is the "context filter" — Extension messages are UI-only and must never enter the prompt.
    let convert = config.convert_to_llm.as_ref();
    let llm_messages = match convert {
        Some(f) => f(&messages),
        None => default_convert_to_llm(&messages), // default: keep only Llm(Message) variants
    };

    // Build tool definitions — the JSON Schema descriptions the LLM uses to decide which tool to call.
    // `.iter().map(...).collect()` is the idiomatic Rust "transform a collection" pattern.
    // Python analogy: [ToolDefinition(name=t.name(), ...) for t in context.tools]
    let tool_defs: Vec<ToolDefinition> = context
        .tools
        .iter()
        .map(|t| ToolDefinition {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters_schema(),
        })
        .collect();

    /*
    RETRY LOOP — loop { ... break value } returning a value

    RUST QUIRK: `loop` can return a value via `break expr`.
    This is unique to Rust — loops are expressions, not just statements.

      let result = loop {
          if condition { break some_value; }  // ← breaks out AND returns some_value
          // otherwise keep looping
      };

    Here we break with a tuple `(result, stream_rx)` — Rust allows breaking with
    any expression, including tuples and structs. The destructuring on the left
    `let (result, mut stream_rx) = loop { ... };` unpacks it immediately.

    MATCH GUARD: `Err(e) if e.is_retryable() && ...`
    The `if` after a match pattern is a "match guard" — an extra condition that must
    be true for that arm to fire. Without it, all Err variants would match the arm.
    Python analogy:
      if isinstance(result, Err) and result.is_retryable() and attempt < max:
          ...
    */
    let retry = &config.retry_config;
    let mut attempt = 0;
    let (result, mut stream_rx) = loop {
        let stream_config = StreamConfig {
            model: config.model.clone(),
            system_prompt: context.system_prompt.clone(),
            messages: llm_messages.clone(),
            tools: tool_defs.clone(),
            thinking_level: config.thinking_level,
            api_key: config.api_key.clone(),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            model_config: config.model_config.clone(),
            cache_config: config.cache_config.clone(),
        };

        // Create a fresh channel per attempt — previous stream_rx is dropped when loop continues.
        // stream_tx is given to the provider; stream_rx stays here for event draining below.
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let provider_cancel = cancel.clone();

        let result = config
            .provider
            .stream(stream_config, stream_tx, provider_cancel)
            .await; // .await suspends here until the SSE stream completes

        match &result {
            // Match guard: only retry if retryable, under the limit, and not cancelled
            Err(e) if e.is_retryable() && attempt < retry.max_retries && !cancel.is_cancelled() => {
                attempt += 1;
                // Use the provider's Retry-After header if present, else use exponential backoff
                let delay = e
                    .retry_after()
                    .unwrap_or_else(|| retry.delay_for_attempt(attempt));
                // unwrap_or_else takes a CLOSURE (lazy evaluation) — the delay is only computed
                // if retry_after() returns None. Saves computing an unused value.
                crate::retry::log_retry(attempt, retry.max_retries, &delay, e);
                tokio::time::sleep(delay).await;
                continue; // jump back to top of loop
            }
            _ => break (result, stream_rx), // success or non-retryable error — exit loop with tuple
        }
    };

    /*
    Drain the event channel and re-emit as AgentEvents.

    stream_rx is a tokio mpsc receiver. The provider sent StreamEvents into stream_tx
    during the `.await` above. Now we drain them all with `try_recv()`:

    RUST QUIRK: `while let Ok(event) = stream_rx.try_recv()`
    `try_recv()` returns:
      Ok(event)  — got an event
      Err(_)     — channel empty OR closed
    `while let Ok(event) = ...` loops as long as we get Ok values. When empty → stops.
    This is non-blocking: it drains all buffered events synchronously.

    `.ok()` on `tx.send(...)`:
    `tx.send()` returns Result<(), SendError> — it fails only if the receiver is dropped.
    `.ok()` converts the Result to Option and silently discards the error.
    Pattern: "fire-and-forget" — we don't care if the subscriber dropped.
    */
    let mut partial_message: Option<AgentMessage> = None;
    while let Ok(event) = stream_rx.try_recv() {
        match &event {
            StreamEvent::Start => {
                // Create a placeholder so deltas have a message to attach to.
                // It will be replaced by the real message on Done.
                let placeholder = AgentMessage::Llm(Message::Assistant {
                    content: Vec::new(),
                    stop_reason: StopReason::Stop,
                    model: config.model.clone(),
                    provider: String::new(),
                    usage: Usage::default(),
                    timestamp: now_ms(),
                    error_message: None,
                });
                partial_message = Some(placeholder.clone());
                tx.send(AgentEvent::MessageStart {
                    message: placeholder,
                })
                .ok(); // .ok() = discard Result — receiver being dropped is non-fatal
            }
            StreamEvent::TextDelta { delta, .. } => {
                // `if let Some(ref msg) = partial_message` — borrow the inner value without moving.
                // `ref msg` means: bind msg as &AgentMessage (a reference), not as AgentMessage (moved).
                // Without `ref`, the match would try to MOVE partial_message out, leaving it unusable.
                if let Some(ref msg) = partial_message {
                    tx.send(AgentEvent::MessageUpdate {
                        message: msg.clone(),
                        delta: StreamDelta::Text {
                            delta: delta.clone(),
                        },
                    })
                    .ok();
                }
            }
            StreamEvent::ThinkingDelta { delta, .. } => {
                if let Some(ref msg) = partial_message {
                    tx.send(AgentEvent::MessageUpdate {
                        message: msg.clone(),
                        delta: StreamDelta::Thinking {
                            delta: delta.clone(),
                        },
                    })
                    .ok();
                }
            }
            StreamEvent::ToolCallDelta { delta, .. } => {
                if let Some(ref msg) = partial_message {
                    tx.send(AgentEvent::MessageUpdate {
                        message: msg.clone(),
                        delta: StreamDelta::ToolCallDelta {
                            delta: delta.clone(),
                        },
                    })
                    .ok();
                }
            }
            StreamEvent::Done { message } => {
                // message.clone().into() — uses the `From<Message> for AgentMessage` impl
                // defined in types.rs to wrap the Message in AgentMessage::Llm automatically.
                let am: AgentMessage = message.clone().into();
                partial_message = Some(am.clone());
                // MessageStart was already emitted on StreamEvent::Start
                tx.send(AgentEvent::MessageEnd { message: am }).ok();
            }
            StreamEvent::Error { message } => {
                let am: AgentMessage = message.clone().into();
                // Only emit MessageStart if Start wasn't received
                // (error before stream opened → no Start event was sent)
                if partial_message.is_none() {
                    tx.send(AgentEvent::MessageStart {
                        message: am.clone(),
                    })
                    .ok();
                }
                partial_message = Some(am.clone());
                tx.send(AgentEvent::MessageEnd { message: am }).ok();
            }
            _ => {} // catch-all: ignore any future StreamEvent variants we don't handle here
        }
    }

    // Return the final result: the complete Message from the provider (or a synthetic error Message)
    match result {
        Ok(msg) => msg,
        Err(e) => {
            // Non-retryable error or retries exhausted. Build a synthetic error Message so the
            // agent loop can record it and fire on_error callbacks. We never panic — errors are
            // part of the protocol, not exceptional conditions.
            warn!("Provider error: {}", e);
            Message::Assistant {
                content: vec![Content::Text {
                    text: String::new(), // empty — the error lives in error_message
                }],
                stop_reason: StopReason::Error,
                model: config.model.clone(),
                provider: "unknown".into(), // .into() converts &str → String
                usage: Usage::default(),
                timestamp: now_ms(),
                error_message: Some(e.to_string()), // Display trait → String via to_string()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

/*
ToolExecutionResult — internal return type from execute_tool_calls().

Why a struct instead of a tuple?
With two fields (tool_results, steering_messages), a tuple would work:
  (Vec<Message>, Option<Vec<AgentMessage>>)
But a struct is self-documenting — the field names make the intent clear
at the call site without needing to look at the function signature.

This is a private struct (no `pub`) — it's only used within this module.
Rust visibility is module-scoped: private = visible only within this file.
*/
struct ToolExecutionResult {
    /// The Message::ToolResult messages to append to the conversation.
    tool_results: Vec<Message>,
    /// Steering messages received mid-execution (user interrupt). If Some, remaining tools were skipped.
    steering_messages: Option<Vec<AgentMessage>>,
}

/*
execute_tool_calls — dispatches to the right execution strategy.

RUST QUIRK: `&[Box<dyn AgentTool>]` — a slice of trait objects

  Box<dyn AgentTool>  — a heap-allocated tool of unknown concrete type
  Vec<Box<dyn AgentTool>> — owned collection of tools
  &[Box<dyn AgentTool>]  — borrowed slice view into that collection

We take `&[...]` (a slice) not `&Vec<...>` because slices are more general:
any contiguous collection (Vec, array, etc.) can be viewed as a slice.
It's idiomatic Rust to accept slices in functions that only need to read.

`tool_calls: &[(String, String, serde_json::Value)]`
A slice of 3-tuples: (tool_call_id, tool_name, arguments).
The tuple packs related data together without needing a named struct.
The LLM returns these as Content::ToolCall items — extracted and passed here.

RUST QUIRK: Pattern matching as dispatch (no if/else chain needed)
`match strategy { Sequential => ..., Parallel => ..., Batched { size } => ... }`
This is exhaustive — if a new ToolExecutionStrategy variant is added later,
the compiler will force you to handle it here. No silent "forgot to update" bugs.
*/
/*
DESIGN: Why `tools` AND `tool_calls` are separate parameters — registry vs invocations
  `tools`      = REGISTRY     — all available implementations (the "phone book"); set at Agent
                                configuration time; unchanged per-turn
  `tool_calls` = INVOCATIONS  — what the LLM asked to do THIS turn (the "calls to make");
                                arrives fresh each turn as Content::ToolCall items from the LLM
The same BashTool entry may appear 5× in `tool_calls` with different arguments.
One registry entry → many call-site invocations. They can never be the same structure.
The LLM can also hallucinate tool names; `tools` lookup can fail, producing is_error=true.
*/
async fn execute_tool_calls(
    tools: &[Box<dyn AgentTool>], // REGISTRY — available implementations (unchanged per-turn)
    tool_calls: &[(String, String, serde_json::Value)], // INVOCATIONS — (id, name, args) tuples from the LLM
    tx: &mpsc::UnboundedSender<AgentEvent>,             // OBSERVER — events forwarded to callers
    cancel: &tokio_util::sync::CancellationToken, // ABORT — checked; child tokens for each tool
    get_steering: Option<&GetMessagesFn>, // INTERRUPT CHECK — polled between tools; None = no steering
    strategy: &ToolExecutionStrategy,     // DISPATCH — Sequential | Parallel | Batched{size}
    config: &AgentLoopConfig,
) -> ToolExecutionResult {
    match strategy {
        ToolExecutionStrategy::Sequential => {
            execute_sequential(tools, tool_calls, tx, cancel, get_steering, config).await
        }
        ToolExecutionStrategy::Parallel => {
            execute_batch(tools, tool_calls, tx, cancel, get_steering, config).await
        }
        ToolExecutionStrategy::Batched { size } => {
            /*
            RUST QUIRK: `.chunks(*size)` — split a slice into sub-slices

            `tool_calls.chunks(n)` returns an iterator of slices, each up to n elements.
            Example: [A, B, C, D, E].chunks(2) → [A,B], [C,D], [E]

            `.enumerate()` wraps each item with its index: (0, [A,B]), (1, [C,D]), ...
            We need the index to calculate how many tools were already executed when
            steering fires (to skip the rest).

            `*size` dereferences size — it's `&usize` (a reference) here because it's
            pattern-matched from `Batched { size }` where size is a field of the enum,
            and we're matching by reference (`&ToolExecutionStrategy`).
            */
            let mut results: Vec<Message> = Vec::new();
            let mut steering_messages: Option<Vec<AgentMessage>> = None;

            for (batch_idx, batch) in tool_calls.chunks(*size).enumerate() {
                let batch_result = execute_batch(tools, batch, tx, cancel, None, config).await;
                // .extend() appends all items from an iterator into the Vec
                // Python analogy: results.extend(batch_result.tool_results)
                results.extend(batch_result.tool_results);

                // Check steering between batches
                if let Some(get_steering_fn) = get_steering {
                    let steering = get_steering_fn();
                    if !steering.is_empty() {
                        steering_messages = Some(steering);
                        // Skip remaining batches — emit skip events so the LLM gets tool results
                        // for all called tools (even skipped ones need a ToolResult in the protocol)
                        let executed = (batch_idx + 1) * *size;
                        if executed < tool_calls.len() {
                            for (skip_id, skip_name, _) in &tool_calls[executed..] {
                                results.push(skip_tool_call(skip_id, skip_name, tx));
                            }
                        }
                        break;
                    }
                }
            }

            ToolExecutionResult {
                tool_results: results,
                steering_messages,
            }
        }
    }
}

/// Execute tool calls one at a time, checking for steering interrupts between each call.
async fn execute_sequential(
    tools: &[Box<dyn AgentTool>], // REGISTRY — look up implementations by name
    tool_calls: &[(String, String, serde_json::Value)], // INVOCATIONS — (id, name, args); processed in order
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — forwarded to execute_single_tool
    cancel: &tokio_util::sync::CancellationToken, // ABORT — forwarded to execute_single_tool
    get_steering: Option<&GetMessagesFn>, // INTERRUPT CHECK — polled after each tool; non-empty → skip remaining
    config: &AgentLoopConfig, // HOOKS — before/after_tool_execution* forwarded to execute_single_tool
) -> ToolExecutionResult {
    let mut results: Vec<Message> = Vec::new();
    let mut steering_messages: Option<Vec<AgentMessage>> = None;

    for (index, (id, name, args)) in tool_calls.iter().enumerate() {
        let (result_msg, _is_error) =
            execute_single_tool(tools, id, name, args, tx, cancel, config).await;
        results.push(result_msg);

        // Check for steering — skip remaining tools if user interrupted
        if let Some(get_steering_fn) = get_steering {
            let steering = get_steering_fn();
            if !steering.is_empty() {
                steering_messages = Some(steering);
                for (skip_id, skip_name, _) in &tool_calls[index + 1..] {
                    results.push(skip_tool_call(skip_id, skip_name, tx));
                }
                break;
            }
        }
    }

    ToolExecutionResult {
        tool_results: results,
        steering_messages,
    }
}

/// Execute a batch of tool calls concurrently via `futures::join_all`, then check for steering.
///
/// Steering is only checked *after the whole batch completes*, not between individual calls.
/// Use [`execute_sequential`] if you need per-call interrupt checking.
async fn execute_batch(
    tools: &[Box<dyn AgentTool>], // REGISTRY — shared across all concurrent executions
    tool_calls: &[(String, String, serde_json::Value)], // INVOCATIONS — all run concurrently (or as a sub-batch)
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — shared (UnboundedSender is Clone + cheap)
    cancel: &tokio_util::sync::CancellationToken, // ABORT — each task gets a child token
    get_steering: Option<&GetMessagesFn>, // INTERRUPT CHECK — polled once after the full batch finishes
    config: &AgentLoopConfig, // HOOKS — before/after_tool_execution* forwarded to execute_single_tool
) -> ToolExecutionResult {
    use futures::future::join_all;

    let futures: Vec<_> = tool_calls
        .iter()
        .map(|(id, name, args)| execute_single_tool(tools, id, name, args, tx, cancel, config))
        .collect();

    let batch_results = join_all(futures).await;

    let results: Vec<Message> = batch_results.into_iter().map(|(msg, _)| msg).collect();

    // Check steering after batch completes
    let steering_messages = if let Some(get_steering_fn) = get_steering {
        let steering = get_steering_fn();
        if steering.is_empty() {
            None
        } else {
            Some(steering)
        }
    } else {
        None
    };

    ToolExecutionResult {
        tool_results: results,
        steering_messages,
    }
}

/*
DESIGN: Why execute_single_tool both returns AND uses `tx`
The two outputs serve completely different audiences:
  RETURN `(Message, bool)` = for the AGENT LOOP — accumulates into tool_results Vec, then sent
                             back to the LLM as the next turn's context
  `tx` events              = for the EXTERNAL CALLER — real-time progress (start/update/end)
                             streamed to the UI or logger as the tool runs
The loop cannot get its structured data from the channel — reading your own output would be
circular. The return value is the protocol; the channel is the live feed.

Why `id` AND `name` as separate params?
  `id`   = INSTANCE identifier — unique per call (e.g. "call_abc123"); used to correlate
           events with the ToolCall that triggered them (same tool called twice → different id)
  `name` = SELECTOR — which tool to look up in the registry (e.g. "bash")
*/
/// Execute a single tool call, emit lifecycle events, and return the `ToolResult` message.
///
/// Returns `(Message::ToolResult, is_error)`. The message is appended to the LLM context by
/// the caller; `is_error` is forwarded to the `ToolExecutionEnd` event and `after_tool_execution` hook.
async fn execute_single_tool(
    tools: &[Box<dyn AgentTool>], // REGISTRY — searched by `name` to find the implementation
    id: &str,   // INSTANCE ID — unique per call; correlates Start/Update/End events
    name: &str, // SELECTOR — which registry entry to invoke (unknown name → is_error)
    args: &serde_json::Value, // INPUT — LLM-chosen arguments (dynamic JSON per invocation)
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — pushes ToolExecution* events; independent of return
    cancel: &tokio_util::sync::CancellationToken, // ABORT — child_token() derived inside for per-tool cancellation
    config: &AgentLoopConfig, // HOOKS — before/after_tool_execution and update variants
) -> (Message, bool) {
    // (Message::ToolResult for LLM context, is_error for ToolExecutionEnd event)
    // Find the tool by name. `find` returns Option<&&Box<dyn AgentTool>>.
    // We use it directly — if None, we return a "tool not found" error result below.
    let tool = tools.iter().find(|t| t.name() == name);

    // before_tool_execution hook — false skips this tool call entirely
    if let Some(ref hook) = config.before_tool_execution {
        if !hook(name, id, args) {
            let skipped_result = ToolResult {
                content: vec![Content::Text {
                    text: "Tool execution skipped by before_tool_execution hook.".to_string(),
                }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            };
            let tool_result_msg = Message::ToolResult {
                tool_call_id: id.to_string(),
                tool_name: name.to_string(),
                content: skipped_result.content,
                is_error: true,
                timestamp: now_ms(),
            };
            tx.send(AgentEvent::MessageStart {
                message: tool_result_msg.clone().into(),
            })
            .ok();
            tx.send(AgentEvent::MessageEnd {
                message: tool_result_msg.clone().into(),
            })
            .ok();
            return (tool_result_msg, true);
        }
    }

    tx.send(AgentEvent::ToolExecutionStart {
        tool_call_id: id.to_string(),
        tool_name: name.to_string(),
        args: args.clone(),
    })
    .ok();

    /*
    RUST QUIRK: Closures capturing the environment with `move`

    `Arc::new(move |partial: ToolResult| { ... })` — a closure that OWNS the captured values.

    Without `move`: closures borrow their environment (references). This would fail here
    because `tx`, `id`, `name` live on the stack of execute_single_tool — they'd be
    dropped before the callback is ever called (it outlives this stack frame).

    With `move`: the closure TAKES OWNERSHIP of the captured variables.
    It's saying: "give me my own copy of tx, id, and name — I'll outlive the frame that created me."

    Why clone before the move?
      let tx = tx.clone();   // clone the Arc<channel> — cheap, increments the Arc count
      let id = id.to_string(); // clone the &str into an owned String

    After these clones, the closure captures the *clones*, not the originals.
    The originals stay available for the function to keep using after the closure is built.

    Python analogy:
      callback = lambda partial: channel.send(ToolExecutionUpdate(tool_call_id=id, ...))
      # Python closures capture by reference (late binding), but here we need early binding
      # to avoid the variable being reused/dropped. Python doesn't have this issue because
      # it uses reference counting and garbage collection automatically.

    The `Arc::new(...)` wraps the closure in a shared reference-counted pointer so it can
    be stored in the ToolUpdateFn type alias and cloned cheaply across threads.
    */
    let on_update: Option<ToolUpdateFn> = {
        let tx = tx.clone();
        let id = id.to_string();
        let name = name.to_string();
        let before_update = config.before_tool_execution_update.clone();
        let after_update = config.after_tool_execution_update.clone();
        Some(Arc::new(move |partial: ToolResult| {
            // Extract text content for hooks
            let content_str: String = partial
                .content
                .iter()
                .filter_map(|c| {
                    if let Content::Text { text } = c {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            // before_tool_execution_update — false suppresses the event (tool keeps running)
            let emit = before_update
                .as_ref()
                .map_or(true, |h| h(&name, &id, &content_str));

            if emit {
                tx.send(AgentEvent::ToolExecutionUpdate {
                    tool_call_id: id.clone(),
                    tool_name: name.clone(),
                    partial_result: partial,
                })
                .ok();
                // after_tool_execution_update — fires after ToolExecutionUpdate
                if let Some(ref hook) = after_update {
                    hook(&name, &id, &content_str);
                }
            }
        }))
    };

    let on_progress: Option<ProgressFn> = {
        let tx = tx.clone();
        let id = id.to_string();
        let name = name.to_string();
        Some(Arc::new(move |text: String| {
            tx.send(AgentEvent::ProgressMessage {
                tool_call_id: id.clone(),
                tool_name: name.clone(),
                text,
            })
            .ok();
        }))
    };

    let ctx = ToolContext {
        tool_call_id: id.to_string(),
        tool_name: name.to_string(),
        // child_token() creates a new CancellationToken that is cancelled when the parent is cancelled.
        // This allows per-tool cancellation: cancel the parent → all child tokens are cancelled.
        // You can also cancel an individual child without affecting other tools or the parent.
        cancel: cancel.child_token(),
        on_update,
        on_progress,
    };

    /*
    RUST QUIRK: Nested match for error handling — no exceptions, just values

    In Rust, errors are values returned in Result<T, E>.
    There are no try/except blocks. Instead, you match on the result.

    This nested match reads as:
      1. Did we find the tool? (outer match on `tool`)
         - Some(tool) → try to execute it
           - Ok(r)  → success: (ToolResult, is_error=false)
           - Err(e) → failure: build an error ToolResult from the error message
         - None → tool not registered: build a "not found" error ToolResult

    WHY NOT PANIC? Tools returning errors is expected — the LLM can make up tool
    names or pass invalid arguments. We convert the error to a ToolResult with
    is_error=true so the LLM sees the failure and can self-correct.
    This is "errors as data" — the failure is part of the conversation, not an exception.
    */
    let (result, is_error) = match tool {
        Some(tool) => match tool.execute(args.clone(), ctx).await {
            Ok(r) => (r, false),
            Err(e) => (
                ToolResult {
                    content: vec![Content::Text {
                        text: e.to_string(), // Display trait → "Tool not found: bash", etc.
                    }],
                    details: serde_json::Value::Null,
                    child_loop_id: None,
                },
                true,
            ),
        },
        None => (
            ToolResult {
                content: vec![Content::Text {
                    text: format!("Tool {} not found", name),
                }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            },
            true,
        ),
    };

    tx.send(AgentEvent::ToolExecutionEnd {
        tool_call_id: id.to_string(),
        tool_name: name.to_string(),
        result: result.clone(),
        is_error,
        child_loop_id: result.child_loop_id.clone(), // Some only for sub-agent tools
    })
    .ok();
    // after_tool_execution hook — fires after ToolExecutionEnd
    if let Some(ref hook) = config.after_tool_execution {
        hook(name, id, is_error);
    }

    let tool_result_msg = Message::ToolResult {
        tool_call_id: id.to_string(),
        tool_name: name.to_string(),
        content: result.content,
        is_error,
        timestamp: now_ms(),
    };

    tx.send(AgentEvent::MessageStart {
        message: tool_result_msg.clone().into(),
    })
    .ok();
    tx.send(AgentEvent::MessageEnd {
        message: tool_result_msg.clone().into(),
    })
    .ok();

    (tool_result_msg, is_error)
}

/// Emit a "skipped" tool result when a user steering message interrupted execution.
/// The LLM protocol requires EVERY ToolCall to have a corresponding ToolResult —
/// even if we never actually ran the tool. This satisfies that contract.
fn skip_tool_call(
    tool_call_id: &str, // INSTANCE ID — matches the ToolCall.id that was skipped
    tool_name: &str,    // NAME — included in events for caller visibility
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — emits Start+End so caller sees the skip in the event stream
) -> Message {
    // Message::ToolResult with is_error=true, content = "Skipped due to queued user message."
    let result = ToolResult {
        content: vec![Content::Text {
            text: "Skipped due to queued user message.".into(),
        }],
        details: serde_json::Value::Null,
        child_loop_id: None,
    };

    tx.send(AgentEvent::ToolExecutionStart {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        args: serde_json::Value::Null,
    })
    .ok();

    tx.send(AgentEvent::ToolExecutionEnd {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        result: result.clone(),
        is_error: true,
        child_loop_id: None,
    })
    .ok();

    let msg = Message::ToolResult {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        content: result.content,
        is_error: true,
        timestamp: now_ms(),
    };

    tx.send(AgentEvent::MessageStart {
        message: msg.clone().into(),
    })
    .ok();
    tx.send(AgentEvent::MessageEnd {
        message: msg.clone().into(),
    })
    .ok();

    msg
}
