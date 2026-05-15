//! Model-invocable tool for surgical context pruning (2-stream architecture).
/*
ARCHITECTURE: PrunTool — model-directed context pruning via deferred execution

Unlike every other built-in tool (bash, read_file, write_file, edit_file, list_files,
search), PrunTool does NOT perform its work inside `execute()`. It cannot — pruning
mutates the agent's context window, which is owned by the agent loop, not the tool.

Instead, `execute()` only enqueues a `PrunRequest` onto a shared `Arc<Mutex<Vec<_>>>`
queue. Between turns, the agent loop drains this queue and applies the requested
pruning to the in-run context stream. See `agent_loop/run.rs` lines 424-426 (drain)
and the `apply_prun` function (around line 524) for the consumer side.

Two-stream context model (see `concepts/compaction` docs):
  `user_context`   — messages typed by the user; NEVER pruned (preserves intent)
  `inrun_context`  — assistant / tool-result chatter; the tail end is what `prun()` trims

Why deferred execution and not direct mutation?
  1. Ownership — the tool has `&self`; mutating the agent's context would require
     either threading `&mut AgentContext` through `ToolContext` (intrusive, breaks
     concurrency for parallel tool execution) or a second `Arc<Mutex<AgentContext>>`
     (deadlock risk because the loop already holds it).
  2. Timing — pruning mid-turn while the LLM stream is open would invalidate the
     content_index counters in `StreamEvent` deltas. Between-turn application is
     the only safe window.
  3. Auditing — the queued `PrunRequest` is part of the loop's event stream, so
     session recorders see the pruning as a discrete event and can reconstruct the
     full pre-prune context from the session log via `PrunRecord`.

Two variants share one tool implementation (toggled by `PrunVariant`):
  `prun(tokens)`              — silent removal; pruned content is gone from context
  `prun_with_memo(tokens, m)` — removal + replacement with a summary string the LLM
                                writes; useful when exploration had findings worth
                                keeping in compressed form.

Both variants are wired together in `BasicAgent::with_prun_tool()` so they share a
single `prun_pending` queue — order of submissions across the two tools is preserved.
*/

use crate::types::*;
use std::sync::{Arc, Mutex};

/// A pending prun request the LLM submitted via `prun` or `prun_with_memo`.
///
/// Lifecycle:
/// 1. `PrunTool::execute()` pushes one of these onto the shared `pending` queue.
/// 2. The agent loop drains the queue between turns (see `agent_loop/run.rs:424`).
/// 3. Each request is applied to `AgentContext.inrun_context` in submission order,
///    producing a `PrunRecord` event that the session recorder captures.
///
/// `tokens_to_remove` is an upper bound — the loop walks the tail of `inrun_context`
/// removing whole entries until at least this many tokens have been freed. User
/// messages are never affected (they live in the separate `user_context` stream).
#[derive(Debug, Clone)]
pub struct PrunRequest {
    /// Lower bound on tokens to remove from the tail of `inrun_context`. The loop
    /// rounds up to the nearest whole entry so a single message is never split.
    pub tokens_to_remove: usize,
    /// Optional summary inserted in place of pruned content. `Some` for the
    /// `prun_with_memo` variant; `None` for the silent `prun` variant.
    pub memo: Option<String>,
}

/// Structured metadata persisted in the `details` field of a prun `ToolResult`.
///
/// Captured by `SessionRecorder` so a session replay can reconstruct exactly what
/// was pruned and (if a memo was supplied) what replaced it. Crucially, the actual
/// pruned message contents live in the session log proper — `pruned_timestamps`
/// is the cross-reference key, not a copy of the content.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrunRecord {
    /// Unix-millis timestamps of every message removed in this prun cycle. Keyed
    /// against `Message::*::timestamp` so the session log can re-link pruned content.
    pub pruned_timestamps: Vec<u64>,
    /// Actual token count freed (may exceed `PrunRequest.tokens_to_remove` because
    /// pruning operates on whole-message boundaries).
    pub tokens_removed: usize,
    /// The summary string inserted in place of pruned content, if this was a
    /// `prun_with_memo` invocation.
    pub memo: Option<String>,
}

/// Which flavour of prun this `PrunTool` instance exposes to the model.
///
/// The same `PrunTool` struct backs both variants — only `name()`, `description()`,
/// `parameters_schema()`, and the memo-handling branch in `execute()` differ. Two
/// variants are exposed (rather than a single tool with an optional memo) so the
/// LLM sees them in `tools/list` as distinct affordances with separate descriptions
/// — easier for the model to pick the right one.
#[derive(Debug, Clone, Copy)]
pub enum PrunVariant {
    /// `prun(tokens)` — silently remove the last N tokens of in-run context.
    Prun,
    /// `prun_with_memo(tokens, memo)` — remove and replace with an LLM-written summary.
    PrunWithMemo,
}

/// Model-invocable tool for surgical context pruning.
///
/// The `pending` queue is shared with the agent loop via `AgentLoopConfig.prun_pending`
/// (an `Arc<Mutex<Vec<PrunRequest>>>`). One `PrunTool` per variant; both variants
/// share the same `pending` queue so cross-variant ordering is preserved.
pub struct PrunTool {
    /*
    RUST QUIRK: `Arc<Mutex<Vec<PrunRequest>>>` — the canonical "shared mutable queue"

    Three layers, each with a purpose:
      `Vec<PrunRequest>`      — the queue itself; FIFO of pending requests
      `Mutex<Vec<...>>`       — serialised access; only one thread mutates at a time
      `Arc<Mutex<...>>`       — shared ownership across the tool, the agent loop,
                                and (when parallel tool execution is on) sibling tools

    `Arc::clone()` increments a reference count; cheap. `mutex.lock().unwrap()` blocks
    until exclusive access is acquired. Drained between turns by the agent loop.

    Python analogy: a `threading.Lock`-guarded `collections.deque` shared via a class
    attribute — except Rust forces the locking discipline at compile time.
    */
    pending: Arc<Mutex<Vec<PrunRequest>>>,
    /// Which of the two surface APIs this instance exposes; `name()`/`description()`
    /// switch on it.
    variant: PrunVariant,
}

impl PrunTool {
    /// Create a new `PrunTool` bound to a shared `pending` queue.
    ///
    /// Call once per variant (Prun + PrunWithMemo) passing the same `Arc<Mutex<_>>`
    /// so both tools enqueue into the same drain. `BasicAgent::with_prun_tool()`
    /// does this wiring automatically.
    pub fn new(pending: Arc<Mutex<Vec<PrunRequest>>>, variant: PrunVariant) -> Self {
        Self { pending, variant }
    }
}

#[async_trait::async_trait]
impl AgentTool for PrunTool {
    fn name(&self) -> &str {
        match self.variant {
            PrunVariant::Prun => "prun",
            PrunVariant::PrunWithMemo => "prun_with_memo",
        }
    }

    fn label(&self) -> &str {
        match self.variant {
            PrunVariant::Prun => "Prun",
            PrunVariant::PrunWithMemo => "Prun with Memo",
        }
    }

    fn description(&self) -> &str {
        match self.variant {
            PrunVariant::Prun => "Surgically remove the last N tokens of model-generated (in-run) context. Use when exploration or tool results waste context length. Pruned content is preserved in session log.",
            PrunVariant::PrunWithMemo => "Surgically remove the last N tokens of in-run context and replace with a summary memo. Use when exploration had findings worth remembering but full content is too verbose.",
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        match self.variant {
            PrunVariant::Prun => serde_json::json!({
                "type": "object",
                "properties": {
                    "tokens": {"type": "integer", "description": "Tokens to remove from tail of in-run context"}
                },
                "required": ["tokens"]
            }),
            PrunVariant::PrunWithMemo => serde_json::json!({
                "type": "object",
                "properties": {
                    "tokens": {"type": "integer", "description": "Tokens to remove from tail of in-run context"},
                    "memo": {"type": "string", "description": "Summary to insert in place of pruned content"}
                },
                "required": ["tokens", "memo"]
            }),
        }
    }

    /*
    DESIGN: execute() enqueues; it does not prune.

    The function looks oddly small for a tool — that's intentional. Real pruning is
    performed by the agent loop between turns (see file-level ARCHITECTURE block).
    All `execute()` does is:
      1. Validate input (`tokens > 0`, plus `memo` for the with-memo variant).
      2. Push a `PrunRequest` onto the shared queue.
      3. Return a placeholder `ToolResult` so the LLM sees the call was accepted.

    `_ctx` is intentionally unused — there's no I/O, no cancellation budget to honour,
    no streaming output. The synthetic ToolResult will be observed by the LLM as
    "your prun request was recorded"; the actual pruning takes effect before the
    next prompt is built, replacing those messages in the context the LLM sees next.
    */
    async fn execute(
        &self,
        params: serde_json::Value, // LLM INPUT — `{"tokens": N}` or `{"tokens": N, "memo": "..."}`
        _ctx: ToolContext,         // SYSTEM ENV — unused; pruning is deferred to the agent loop
    ) -> Result<ToolResult, ToolError> {
        // Validate `tokens` — must be a positive integer. A missing or non-integer
        // value would otherwise silently default to 0 and produce a no-op enqueue.
        let tokens = params.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        if tokens == 0 {
            return Err(ToolError::InvalidArgs("tokens must be > 0".to_string()));
        }

        // Extract the memo only for the with-memo variant. The bare `prun` variant
        // ignores any memo field even if the LLM accidentally supplies one — this
        // keeps the two tools' on-the-wire semantics strictly separate.
        let memo = match self.variant {
            PrunVariant::PrunWithMemo => params
                .get("memo")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            PrunVariant::Prun => None,
        };

        // Enqueue. `.lock().unwrap()` panics on mutex poisoning, which would indicate
        // a panic in a previous holder of the lock — a bug worth surfacing loudly.
        // (Contrast with the steering-queue poison-tolerant lock in BasicAgent, where
        // recoverable behaviour is preferred because hooks run user code; here the
        // only writer is this tool plus the agent-loop drain, both internal.)
        self.pending.lock().unwrap().push(PrunRequest {
            tokens_to_remove: tokens,
            memo,
        });

        // Synthetic acknowledgement message — the LLM sees this in the next turn's
        // ToolResult. The actual pruning is invisible to the model except by the
        // shorter context window it observes next turn.
        Ok(ToolResult {
            content: vec![Content::Text {
                text: format!(
                    "Prun request recorded: {} tokens will be removed before next turn.",
                    tokens
                ),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}
