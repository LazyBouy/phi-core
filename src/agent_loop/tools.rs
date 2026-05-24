//! Tool execution — dispatch, concurrency strategies, and single-tool lifecycle.
//!
//! Extracted from `agent_loop.rs`. Contains the tool execution pipeline:
//! [`execute_tool_calls`] dispatches to [`execute_sequential`], [`execute_batch`],
//! or batched execution depending on [`ToolExecutionStrategy`]. Each individual
//! tool invocation goes through [`execute_single_tool`] with full lifecycle events.
//! [`skip_tool_call`] emits a synthetic result for tools skipped by steering interrupts.

use super::config::*;
use crate::types::*;
use std::sync::Arc;
use tokio::sync::mpsc;

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
pub(super) struct ToolExecutionResult {
    /// The Message::ToolResult messages to append to the conversation.
    pub(super) tool_results: Vec<Message>,
    /// Steering messages received mid-execution (user interrupt). If Some, remaining tools were skipped.
    pub(super) steering_messages: Option<Vec<AgentMessage>>,
}

/*
execute_tool_calls — dispatches to the right execution strategy.

RUST QUIRK: `&[Arc<dyn AgentTool>]` — a slice of shared trait objects

  Arc<dyn AgentTool>  — a reference-counted tool of unknown concrete type
  Vec<Arc<dyn AgentTool>> — owned collection of Arc-wrapped tools
  &[Arc<dyn AgentTool>]  — borrowed slice view into that collection

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
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_calls(
    tools: &[Arc<dyn AgentTool>], // REGISTRY — available implementations (unchanged per-turn)
    tool_calls: &[(String, String, serde_json::Value)], // INVOCATIONS — (id, name, args) tuples from the LLM
    tx: &mpsc::UnboundedSender<AgentEvent>,             // OBSERVER — events forwarded to callers
    cancel: &tokio_util::sync::CancellationToken, // ABORT — checked; child tokens for each tool
    get_steering: Option<&GetMessagesFn>, // INTERRUPT CHECK — polled between tools; None = no steering
    strategy: &ToolExecutionStrategy,     // DISPATCH — Sequential | Parallel | Batched{size}
    config: &AgentLoopConfig,
    loop_id: &str,
) -> ToolExecutionResult {
    match strategy {
        ToolExecutionStrategy::Sequential => {
            execute_sequential(tools, tool_calls, tx, cancel, get_steering, config, loop_id).await
        }
        ToolExecutionStrategy::Parallel => {
            execute_batch(tools, tool_calls, tx, cancel, get_steering, config, loop_id).await
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
                let batch_result =
                    execute_batch(tools, batch, tx, cancel, None, config, loop_id).await;
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
                                results.push(skip_tool_call(skip_id, skip_name, tx, loop_id));
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
pub(super) async fn execute_sequential(
    tools: &[Arc<dyn AgentTool>], // REGISTRY — look up implementations by name
    tool_calls: &[(String, String, serde_json::Value)], // INVOCATIONS — (id, name, args); processed in order
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — forwarded to execute_single_tool
    cancel: &tokio_util::sync::CancellationToken, // ABORT — forwarded to execute_single_tool
    get_steering: Option<&GetMessagesFn>, // INTERRUPT CHECK — polled after each tool; non-empty → skip remaining
    config: &AgentLoopConfig, // HOOKS — before/after_tool_execution* forwarded to execute_single_tool
    loop_id: &str,
) -> ToolExecutionResult {
    let mut results: Vec<Message> = Vec::new();
    let mut steering_messages: Option<Vec<AgentMessage>> = None;

    for (index, (id, name, args)) in tool_calls.iter().enumerate() {
        let (result_msg, _is_error) =
            execute_single_tool(tools, id, name, args, tx, cancel, config, loop_id).await;
        results.push(result_msg);

        // Check for steering — skip remaining tools if user interrupted
        if let Some(get_steering_fn) = get_steering {
            let steering = get_steering_fn();
            if !steering.is_empty() {
                steering_messages = Some(steering);
                for (skip_id, skip_name, _) in &tool_calls[index + 1..] {
                    results.push(skip_tool_call(skip_id, skip_name, tx, loop_id));
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
pub(super) async fn execute_batch(
    tools: &[Arc<dyn AgentTool>], // REGISTRY — shared across all concurrent executions
    tool_calls: &[(String, String, serde_json::Value)], // INVOCATIONS — all run concurrently (or as a sub-batch)
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — shared (UnboundedSender is Clone + cheap)
    cancel: &tokio_util::sync::CancellationToken, // ABORT — each task gets a child token
    get_steering: Option<&GetMessagesFn>, // INTERRUPT CHECK — polled once after the full batch finishes
    config: &AgentLoopConfig, // HOOKS — before/after_tool_execution* forwarded to execute_single_tool
    loop_id: &str,
) -> ToolExecutionResult {
    use futures::future::join_all;

    let futures: Vec<_> = tool_calls
        .iter()
        .map(|(id, name, args)| {
            execute_single_tool(tools, id, name, args, tx, cancel, config, loop_id)
        })
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
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_single_tool(
    tools: &[Arc<dyn AgentTool>], // REGISTRY — searched by `name` to find the implementation
    id: &str,   // INSTANCE ID — unique per call; correlates Start/Update/End events
    name: &str, // SELECTOR — which registry entry to invoke (unknown name → is_error)
    args: &serde_json::Value, // INPUT — LLM-chosen arguments (dynamic JSON per invocation)
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — pushes ToolExecution* events; independent of return
    cancel: &tokio_util::sync::CancellationToken, // ABORT — child_token() derived inside for per-tool cancellation
    config: &AgentLoopConfig, // HOOKS — before/after_tool_execution and update variants
    loop_id: &str,
) -> (Message, bool) {
    // (Message::ToolResult for LLM context, is_error for ToolExecutionEnd event)
    // Find the tool by name. `find` returns Option<&&Arc<dyn AgentTool>>.
    // We use it directly — if None, we return a "tool not found" error result below.
    let tool = tools.iter().find(|t| t.name() == name);

    // before_tool_execution hook — false skips this tool call entirely
    if let Some(ref hook) = config.before_tool_execution {
        if !hook(name, id, args).await {
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
                loop_id: loop_id.to_string(),
                message: tool_result_msg.clone().into(),
            })
            .ok();
            tx.send(AgentEvent::MessageEnd {
                loop_id: loop_id.to_string(),
                message: tool_result_msg.clone().into(),
            })
            .ok();
            return (tool_result_msg, true);
        }
    }

    tx.send(AgentEvent::ToolExecutionStart {
        loop_id: loop_id.to_string(),
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
        let loop_id_owned = loop_id.to_string();
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
            // (kept sync in 0.9.0; see config.rs `BeforeToolExecutionUpdateFn` docstring)
            let emit = before_update
                .as_ref()
                .map_or(true, |h| h(&name, &id, &content_str));

            if emit {
                tx.send(AgentEvent::ToolExecutionUpdate {
                    loop_id: loop_id_owned.clone(),
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
        let loop_id_owned = loop_id.to_string();
        Some(Arc::new(move |text: String| {
            tx.send(AgentEvent::ProgressMessage {
                loop_id: loop_id_owned.clone(),
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
        Some(tool) => {
            // Resolve the effective per-tool timeout: per-tool override > config-level > None.
            // When `None`, fall through to the original unbounded execute (preserving prior behaviour).
            let effective_timeout = tool.timeout().or(config.tool_timeout);
            // Clone ctx.cancel BEFORE moving ctx into execute, so we can signal cooperative
            // cleanup on a timeout fire.
            let tool_cancel = ctx.cancel.clone();

            let exec_result = match effective_timeout {
                None => tool.execute(args.clone(), ctx).await,
                Some(d) => match tokio::time::timeout(d, tool.execute(args.clone(), ctx)).await {
                    Ok(r) => r,
                    Err(_) => {
                        // Best-effort cooperative cleanup — tools that check `is_cancelled()`
                        // can free resources before the next turn starts.
                        tool_cancel.cancel();
                        Err(ToolError::Timeout { duration: d })
                    }
                },
            };

            match exec_result {
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
            }
        }
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
        loop_id: loop_id.to_string(),
        tool_call_id: id.to_string(),
        tool_name: name.to_string(),
        result: result.clone(),
        is_error,
        child_loop_id: result.child_loop_id.clone(), // Some only for sub-agent tools
    })
    .ok();
    // after_tool_execution hook — fires after ToolExecutionEnd
    if let Some(ref hook) = config.after_tool_execution {
        hook(name, id, is_error).await;
    }

    let tool_result_msg = Message::ToolResult {
        tool_call_id: id.to_string(),
        tool_name: name.to_string(),
        content: result.content,
        is_error,
        timestamp: now_ms(),
    };

    tx.send(AgentEvent::MessageStart {
        loop_id: loop_id.to_string(),
        message: tool_result_msg.clone().into(),
    })
    .ok();
    tx.send(AgentEvent::MessageEnd {
        loop_id: loop_id.to_string(),
        message: tool_result_msg.clone().into(),
    })
    .ok();

    (tool_result_msg, is_error)
}

/// Emit a "skipped" tool result when a user steering message interrupted execution.
/// The LLM protocol requires EVERY ToolCall to have a corresponding ToolResult —
/// even if we never actually ran the tool. This satisfies that contract.
pub(super) fn skip_tool_call(
    tool_call_id: &str, // INSTANCE ID — matches the ToolCall.id that was skipped
    tool_name: &str,    // NAME — included in events for caller visibility
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — emits Start+End so caller sees the skip in the event stream
    loop_id: &str,
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
        loop_id: loop_id.to_string(),
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        args: serde_json::Value::Null,
    })
    .ok();

    tx.send(AgentEvent::ToolExecutionEnd {
        loop_id: loop_id.to_string(),
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
        loop_id: loop_id.to_string(),
        message: msg.clone().into(),
    })
    .ok();
    tx.send(AgentEvent::MessageEnd {
        loop_id: loop_id.to_string(),
        message: msg.clone().into(),
    })
    .ok();

    msg
}
