//! Core loop engine shared by `agent_loop` and `agent_loop_continue`.

use super::config::*;
use super::helpers::apply_input_filters;
use super::streaming::stream_assistant_response;
use super::tools::execute_tool_calls;
use crate::context::{
    build_context_from_session, compact_session_loops, BlockCompactionStrategy, CompactionStrategy,
    DefaultBlockCompaction, DefaultCompaction, ExecutionTracker,
};
use crate::types::*;
use chrono::Utc;
use tokio::sync::mpsc;
use tracing::warn;

/// Core loop shared by [`super::core::agent_loop`] and [`super::core::agent_loop_continue`].
/// Never called directly.
///
/// **Outer loop** — repeats when `get_follow_up_messages` returns work after the agent would stop.
/// **Inner loop** — repeats when the LLM requests tool calls or steering messages arrive mid-turn.
///
/// Per-turn event ordering (enforced every iteration):
/// `before_turn` → `TurnStart` → [prompt/steering messages] → [LLM stream] → [tools] → `TurnEnd` → `after_turn`
///
/// Returns accumulated [`Usage`] across all turns so the caller can pass it to `after_loop`.
pub(super) async fn run_loop(
    context: &mut AgentContext, // ACCUMULATOR — all messages (grows as turns complete)
    new_messages: &mut Vec<AgentMessage>, // RESULT COLLECTOR — only messages added in this call
    config: &AgentLoopConfig,   // STATIC SETTINGS — unchanged for lifetime of this call
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — borrowed; cloned into tool closures as needed
    cancel: &tokio_util::sync::CancellationToken, // ABORT — borrowed; child tokens derived per tool
    first_turn_prompts: Option<&[AgentMessage]>, // Initial prompts (agent_loop only); None for agent_loop_continue
) -> Usage {
    let loop_id = context.loop_id.clone().unwrap_or_default();

    // Composition I — when revert mode is active, seed `next_node_id` from any
    // pre-existing IDs on `messages` so a continuation does not collide with
    // node IDs already minted in a prior loop. Idempotent + cheap; gated on
    // `revert_pending.is_some()` so non-revert consumers see zero behavioural
    // change (the field is left at its default of 0 and is never read).
    if config.revert_pending.is_some() {
        context.seed_next_node_id_from_messages();
    }
    let mut first_turn = true;
    let mut turn: usize = 0; // single counter: passed to hooks (as usize) and TurnStart events (as u32)
    #[allow(unused_assignments)]
    let mut current_turn_id: Option<TurnId> = None; // set at TurnStart, applied to all messages in this turn
    let mut loop_usage = Usage::default(); // accumulated usage across all turns, returned for after_loop
    let mut tracker = config
        .execution_limits
        .as_ref()
        .map(|limits| ExecutionTracker::new(limits.clone()));

    // Check for steering messages at start
    // !!!SECURITY!!!: Filter initial steering messages before any turn starts.
    let raw = config
        .get_steering_messages
        .as_ref()
        .map(|f| f())
        .unwrap_or_default();
    let mut pending = match apply_input_filters(raw, &config.input_filters, tx, &loop_id).await {
        Ok(filtered) => filtered,
        Err(_) => return loop_usage,
    };

    // Outer loop: follow-ups after agent would stop
    loop {
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
            let turn_trigger = if first_turn {
                if matches!(
                    context.continuation_kind,
                    Some(ContinuationKind::Branch { .. })
                ) {
                    TurnTrigger::Branch
                } else if context.continuation_kind.is_some() {
                    TurnTrigger::Continuation
                } else {
                    config.first_turn_trigger.clone()
                }
            } else {
                TurnTrigger::Continuation
            };

            // Check execution limits BEFORE before_turn so we don't fire hooks for an impossible turn
            if let Some(ref tracker) = tracker {
                if let Some(reason) = tracker.check_limits() {
                    warn!("Execution limit reached: {}", reason);
                    let limit_msg = AgentMessage::Llm(LlmMessage::new(Message::User {
                        content: vec![Content::Text {
                            text: format!("[Agent stopped: {}]", reason),
                        }],
                        timestamp: now_ms(),
                    }));
                    tx.send(AgentEvent::MessageStart {
                        loop_id: loop_id.clone(),
                        message: limit_msg.clone(),
                    })
                    .ok();
                    tx.send(AgentEvent::MessageEnd {
                        loop_id: loop_id.clone(),
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
                if !before_turn(&context.messages, turn).await {
                    return loop_usage;
                }
            }

            // TurnStart — fires AFTER before_turn hook
            tx.send(AgentEvent::TurnStart {
                loop_id: loop_id.clone(),
                turn_index: turn as u32,
                timestamp: chrono::Utc::now(),
                triggered_by: turn_trigger,
            })
            .ok();

            // Capture the turn_id for this turn BEFORE incrementing.
            // All messages pushed during this turn will carry this id.
            current_turn_id = Some(TurnId {
                loop_id: loop_id.clone(),
                turn_index: turn as u32,
            });

            let was_first_turn = first_turn;
            if first_turn {
                first_turn = false;
            }
            // Capture the active turn-index BEFORE incrementing so downstream
            // emitters (`stream_assistant_response` → `AgentEvent::TurnRequest`)
            // can cite the same `turn_index` as the just-emitted `TurnStart`.
            let this_turn_index = turn as u32;
            turn += 1;

            // On the first turn of agent_loop(), emit events for the initial prompt messages
            if was_first_turn {
                if let Some(prompts) = first_turn_prompts {
                    for prompt in prompts {
                        tx.send(AgentEvent::MessageStart {
                            loop_id: loop_id.clone(),
                            message: prompt.clone(),
                        })
                        .ok();
                        tx.send(AgentEvent::MessageEnd {
                            loop_id: loop_id.clone(),
                            message: prompt.clone(),
                        })
                        .ok();
                    }
                }
            }

            // Inject pending steering/follow-up messages (after TurnStart — they are part of this turn)
            if !pending.is_empty() {
                for msg in pending.drain(..) {
                    let msg = msg.with_turn_id(current_turn_id.clone());
                    // Composition I — stamp node identity when revert mode is active.
                    let msg = stamp_node_identity(context, config, msg);
                    tx.send(AgentEvent::MessageStart {
                        loop_id: loop_id.clone(),
                        message: msg.clone(),
                    })
                    .ok();
                    tx.send(AgentEvent::MessageEnd {
                        loop_id: loop_id.clone(),
                        message: msg.clone(),
                    })
                    .ok();
                    context.messages.push(msg.clone());
                    new_messages.push(msg.clone());
                    // Also track in user_context stream
                    context.user_context.push(msg);
                }
            }

            // Compact context if configured — percentage-based threshold check
            if let Some(ref ctx_config) = config.context_config {
                let max_tokens = ctx_config.max_context_tokens;
                let comp = &ctx_config.compaction;
                let estimated = ctx_config.counter().estimate_messages(&context.messages);
                let system_frac = ctx_config.system_prompt_tokens as f64 / max_tokens as f64;
                let current_frac = estimated as f64 / max_tokens as f64;
                let headroom = comp.compact_at_pct - system_frac - current_frac;

                if headroom < comp.compact_budget_threshold_pct {
                    let msgs_before = context.messages.len();

                    // G1: before_compaction_start hook — skip compaction if returns false
                    let compaction_allowed = match config.before_compaction_start.as_ref() {
                        Some(hook) => hook(estimated, msgs_before).await,
                        None => true,
                    };

                    if compaction_allowed {
                        tx.send(AgentEvent::CompactionStarted {
                            loop_id: loop_id.clone(),
                            estimated_tokens: estimated,
                            message_count: msgs_before,
                            timestamp: Utc::now(),
                        })
                        .ok();

                        if let Some(ref mut session) = context.session {
                            // Block-based compaction path (Session available)
                            let lid = context.loop_id.as_deref().unwrap_or("");

                            // Ensure current loop exists in session with up-to-date messages
                            if session.get_loop(lid).is_none() {
                                session.loops.push(crate::session::LoopRecord {
                                    loop_id: lid.to_string(),
                                    session_id: context.session_id.clone().unwrap_or_default(),
                                    agent_id: context.agent_id.clone().unwrap_or_default(),
                                    parent_loop_id: context.parent_loop_id.clone(),
                                    continuation_kind: context
                                        .continuation_kind
                                        .clone()
                                        .unwrap_or_default(),
                                    started_at: Utc::now(),
                                    ended_at: None,
                                    status: crate::session::LoopStatus::Running,
                                    rejection: None,
                                    config: None,
                                    messages: context.messages.clone(),
                                    turns: Vec::new(),
                                    usage: Usage::default(),
                                    metadata: None,
                                    events: Vec::new(),
                                    children_loop_ids: Vec::new(),
                                    child_loop_refs: Vec::new(),
                                    parallel_group: None,
                                    compaction_block: None,
                                });
                            } else if let Some(record) = session.get_loop_mut(lid) {
                                record.messages = context.messages.clone();
                            }

                            let block_strategy: &dyn BlockCompactionStrategy = comp
                                .block_strategy
                                .as_deref()
                                .unwrap_or(&DefaultBlockCompaction);
                            compact_session_loops(
                                session,
                                lid,
                                block_strategy,
                                comp,
                                max_tokens,
                                ctx_config.token_counter.as_ref(),
                            )
                            .await;
                            context.messages = build_context_from_session(
                                session,
                                lid,
                                comp,
                                max_tokens,
                                ctx_config.token_counter.as_ref(),
                            );

                            let chain = session.loop_chain_to(lid);
                            let loops_compacted = chain
                                .iter()
                                .filter(|l| {
                                    session
                                        .get_loop(l)
                                        .map(|r| r.compaction_block.is_some())
                                        .unwrap_or(false)
                                })
                                .count();

                            let messages_after = context.messages.len();
                            let tokens_after =
                                ctx_config.counter().estimate_messages(&context.messages);

                            tx.send(AgentEvent::CompactionEnded {
                                loop_id: loop_id.clone(),
                                messages_before: msgs_before,
                                messages_after,
                                estimated_tokens_before: estimated,
                                estimated_tokens_after: tokens_after,
                                loops_compacted,
                                timestamp: Utc::now(),
                            })
                            .ok();

                            // G1: after_compaction_end hook
                            if let Some(ref hook) = config.after_compaction_end {
                                hook(msgs_before, messages_after, estimated, tokens_after).await;
                            }
                        } else {
                            // In-memory fallback (no Session — sub-agents, tests, etc.)
                            let strategy: &dyn CompactionStrategy = comp
                                .in_memory_strategy
                                .as_deref()
                                .unwrap_or(&DefaultCompaction);
                            context.messages =
                                strategy.compact(std::mem::take(&mut context.messages), ctx_config);

                            let messages_after = context.messages.len();
                            let tokens_after =
                                ctx_config.counter().estimate_messages(&context.messages);

                            tx.send(AgentEvent::CompactionEnded {
                                loop_id: loop_id.clone(),
                                messages_before: msgs_before,
                                messages_after,
                                estimated_tokens_before: estimated,
                                estimated_tokens_after: tokens_after,
                                loops_compacted: 0,
                                timestamp: Utc::now(),
                            })
                            .ok();

                            // G1: after_compaction_end hook
                            if let Some(ref hook) = config.after_compaction_end {
                                hook(msgs_before, messages_after, estimated, tokens_after).await;
                            }
                        }
                    } // if compaction_allowed
                }
            }

            // Stream assistant response
            let message =
                stream_assistant_response(context, config, tx, cancel, &loop_id, this_turn_index)
                    .await;

            let agent_msg: AgentMessage =
                AgentMessage::from(message.clone()).with_turn_id(current_turn_id.clone());
            // Composition I — stamp node identity when revert mode is active.
            let agent_msg = stamp_node_identity(context, config, agent_msg);
            context.messages.push(agent_msg.clone());
            new_messages.push(agent_msg.clone());
            // Track in inrun_context stream (model-generated)
            context
                .inrun_context
                .push(crate::types::InRunEntry::Live(agent_msg.clone()));

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
                            on_error(err_str).await;
                        }
                    }
                    // Accumulate usage into loop total
                    loop_usage.input += usage.input;
                    loop_usage.output += usage.output;
                    loop_usage.reasoning += usage.reasoning;
                    loop_usage.cache_read += usage.cache_read;
                    loop_usage.cache_write += usage.cache_write;
                    loop_usage.total_tokens += usage.total_tokens;
                    if let Some(ref mut t) = tracker {
                        t.record_cost(usage.estimated_cost(&config.model_config.cost));
                    }
                    tx.send(AgentEvent::TurnEnd {
                        loop_id: loop_id.clone(),
                        message: agent_msg,
                        usage: usage.clone(),
                        timestamp: chrono::Utc::now(),
                        tool_results: vec![],
                    })
                    .ok();
                    if let Some(ref after_turn) = config.after_turn {
                        after_turn(&context.messages, usage).await;
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
                    &loop_id,
                )
                .await;

                tool_results = execution.tool_results;
                steering_after_tools = execution.steering_messages;

                for result in &tool_results {
                    let am: AgentMessage =
                        AgentMessage::from(result.clone()).with_turn_id(current_turn_id.clone());
                    // Composition I — stamp node identity when revert mode is active.
                    let am = stamp_node_identity(context, config, am);
                    context.messages.push(am.clone());
                    new_messages.push(am.clone());
                    // Track in inrun_context stream (tool results are model-generated context)
                    context
                        .inrun_context
                        .push(crate::types::InRunEntry::Live(am));
                }

                // Apply pending prun requests
                if let Some(ref prun_pending) = config.prun_pending {
                    let requests: Vec<crate::tools::prun::PrunRequest> =
                        prun_pending.lock().unwrap().drain(..).collect();
                    for request in requests {
                        apply_prun(context, &request, tx, &loop_id);
                    }
                }

                // Apply pending revert_to_state requests (Composition I).
                // Gated on `config.revert_pending.is_some()` — the only path
                // that sets this is `BasicAgent::with_revert_tool()`. Without
                // the opt-in, the drain never executes; the LLM never sees the
                // tool; the active-node pointer stays unset; the linear
                // `build_working_context` path remains in force.
                if let Some(ref revert_pending) = config.revert_pending {
                    let requests: Vec<crate::tools::revert::RevertRequest> =
                        revert_pending.lock().unwrap().drain(..).collect();
                    for request in requests {
                        apply_revert(context, &request, turn, tx, &loop_id);
                    }
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
            loop_usage.reasoning += turn_usage.reasoning;
            loop_usage.cache_read += turn_usage.cache_read;
            loop_usage.cache_write += turn_usage.cache_write;
            loop_usage.total_tokens += turn_usage.total_tokens;
            if let Some(ref mut t) = tracker {
                t.record_cost(turn_usage.estimated_cost(&config.model_config.cost));
            }

            // TurnEnd fires BEFORE after_turn
            tx.send(AgentEvent::TurnEnd {
                loop_id: loop_id.clone(),
                message: agent_msg,
                usage: turn_usage.clone(),
                timestamp: chrono::Utc::now(),
                tool_results,
            })
            .ok();

            // after_turn hook fires AFTER TurnEnd
            if let Some(ref after_turn) = config.after_turn {
                after_turn(&context.messages, &turn_usage).await;
            }

            // Check steering after turn — filter before assigning to pending
            if let Some(steering) = steering_after_tools.take() {
                if !steering.is_empty() {
                    match apply_input_filters(steering, &config.input_filters, tx, &loop_id).await {
                        Ok(filtered) => {
                            pending = filtered;
                            continue;
                        }
                        Err(_) => return loop_usage,
                    }
                }
            }

            let raw = config
                .get_steering_messages
                .as_ref()
                .map(|f| f())
                .unwrap_or_default();
            pending = match apply_input_filters(raw, &config.input_filters, tx, &loop_id).await {
                Ok(filtered) => filtered,
                Err(_) => return loop_usage,
            };

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
            match apply_input_filters(follow_ups, &config.input_filters, tx, &loop_id).await {
                Ok(filtered) => {
                    pending = filtered;
                    continue;
                }
                Err(_) => return loop_usage,
            }
        }

        break;
    }
    loop_usage
}

/// Composition I — stamp a freshly-built [`AgentMessage`] with `node_id` +
/// `parent_id` when revert mode is active. No-op when `config.revert_pending`
/// is `None`, preserving the byte-identical non-revert-mode behaviour.
///
/// Parent linkage: when `context.active_node_id` is `Some`, use it as the
/// parent (revert just landed → next stamped node sits below the new active
/// pointer). Otherwise scan `context.messages` for the most recent stamped
/// `LlmMessage` and link to that. Falls back to `None` when nothing has been
/// stamped yet (the first stamped node in the session becomes a root).
///
/// After stamping, `context.active_node_id` advances to the new node so the
/// trunk assembly walk (Phase 4 `build_trunk_context`) finds the most recent
/// turn at the head of the chain.
fn stamp_node_identity(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    msg: AgentMessage,
) -> AgentMessage {
    if config.revert_pending.is_none() {
        return msg;
    }
    // Only LLM messages get IDs; extension messages stay untouched.
    match msg {
        AgentMessage::Llm(_) => {}
        AgentMessage::Extension(_) => return msg,
    }
    let new_id = context.alloc_node_id();
    let parent = context.active_node_id.or_else(|| {
        // No active pointer yet — link to the most recently stamped Llm node
        // in `messages`. If none exist, this is the first stamped node and
        // becomes a root.
        context.messages.iter().rev().find_map(|m| match m {
            AgentMessage::Llm(lm) => lm.node_id,
            _ => None,
        })
    });
    let stamped = msg.with_node_identity(new_id, parent);
    context.active_node_id = Some(new_id);
    stamped
}

/// Apply a single prun request to the in-run context, converting Live entries to
/// PrunedSilent or PrunedMemo from the tail until enough tokens are removed.
fn apply_prun(
    context: &mut AgentContext,
    request: &crate::tools::prun::PrunRequest,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    loop_id: &str,
) {
    use crate::context::token::message_tokens;
    use crate::types::InRunEntry;

    let mut tokens_remaining = request.tokens_to_remove;
    let mut total_tokens_removed: usize = 0;
    let mut messages_removed: usize = 0;
    let mut pruned_timestamps: Vec<u64> = Vec::new();

    // Walk inrun_context from the tail, pruning Live entries until budget is exhausted
    for entry in context.inrun_context.iter_mut().rev() {
        if tokens_remaining == 0 {
            break;
        }
        if let InRunEntry::Live(msg) = entry {
            let msg_tokens = message_tokens(msg);
            let tokens_to_remove = msg_tokens.min(tokens_remaining);
            tokens_remaining = tokens_remaining.saturating_sub(tokens_to_remove);
            total_tokens_removed += tokens_to_remove;
            messages_removed += 1;
            let ts = msg.timestamp();
            pruned_timestamps.push(ts);

            if let Some(ref memo) = request.memo {
                *entry = InRunEntry::PrunedMemo {
                    memo: memo.clone(),
                    tokens_removed: msg_tokens,
                    timestamp: ts,
                };
            } else {
                *entry = InRunEntry::PrunedSilent {
                    tokens_removed: msg_tokens,
                    timestamp: ts,
                };
            }
        }
    }

    if total_tokens_removed > 0 {
        tx.send(AgentEvent::PrunApplied {
            loop_id: loop_id.to_string(),
            tokens_removed: total_tokens_removed,
            messages_removed,
            memo: request.memo.clone(),
            pruned_timestamps,
            timestamp: chrono::Utc::now(),
        })
        .ok();
    }
}

/// Compose the one-line progress-thread breadcrumb attached to a reverted-to
/// node's summary tag.
///
/// Shapes (mirroring the `prun_with_memo` memo: drop content, keep a memo):
/// - summary + tool names → `"reverted past: <summary> (<names> abandoned)"`;
/// - summary only (no tool calls on the abandoned span) → `"reverted past: <summary>"`;
/// - tool names only (agent omitted summary) → `"reverted past: <names> (abandoned)"`;
/// - neither → `""` (empty; the render policy + a future fallback generator
///   handle the empty-text case).
///
/// Only the tool-call NAMES are carried, never the abandoned content itself —
/// re-introducing the abandoned branch is exactly the noise wall a revert is
/// meant to remove.
fn compose_revert_breadcrumb(summary: &str, tool_names: &[String]) -> String {
    let summary = summary.trim();
    let names = tool_names.join(", ");
    match (summary.is_empty(), tool_names.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("reverted past: {summary}"),
        (true, false) => format!("reverted past: {names} (abandoned)"),
        (false, false) => format!("reverted past: {summary} ({names} abandoned)"),
    }
}

/// Composition I — apply one `RevertRequest` between turns.
///
/// Mirrors [`apply_prun`] structurally: synchronous, no I/O, emits a single
/// `RevertApplied` event whether the revert succeeds or is rejected. The
/// forensic `messages` log is **never** mutated — the abandoned span stays in
/// `context.messages` and is only off-trunk from this point on (the
/// parent-chain walk in Phase 4 will skip it).
///
/// Effects on success:
/// 1. `context.active_node_id` becomes `Some(request.target)`.
/// 2. A `NodeTag` is attached to the target [`LlmMessage`] carrying the
///    composed breadcrumb (empty `text` if `summary` was `None` AND the
///    abandoned span had no tool calls) — EXCEPT for the empty-tail pinned
///    no-op (F-empty-tail-summary.a, CC-18): a `completion`/`step-summary`
///    revert with an empty abandoned tail attaches NO tag, because the pinned
///    tag would merely restate still-visible content. Decayable categories
///    (`failure`/`tangent`) always attach (the lesson replaces removed content).
/// 3. `RevertApplied { applied: true, .. }` is emitted with the list of
///    `abandoned_node_ids` (fires even on the empty-tail pinned no-op path).
///
/// Rejection rules (each emits `RevertApplied { applied: false, reason, .. }`
/// with no mutation):
/// - **Unknown target** — no [`LlmMessage`] in `context.messages` carries
///   `node_id == request.target`.
/// - **User message in abandoned span** (D6) — the conservative 0.8.0
///   behaviour: if any message strictly after the target node is a
///   `Message::User`, refuse so the agent cannot silently pretend the user
///   never spoke. Auto-rebase is deferred to a future release.
fn apply_revert(
    context: &mut AgentContext,
    request: &crate::tools::revert::RevertRequest,
    current_turn: usize,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    loop_id: &str,
) {
    use crate::types::{InRunEntry, NodeTag};

    // Helper: assemble the rejected-event payload once.
    let emit_rejected = |reason: &str, tx: &mpsc::UnboundedSender<AgentEvent>| {
        tx.send(AgentEvent::RevertApplied {
            loop_id: loop_id.to_string(),
            category: request.category,
            target: None,
            abandoned_node_ids: Vec::new(),
            summary: request.summary.clone(),
            applied: false,
            reason: Some(reason.to_string()),
            timestamp: chrono::Utc::now(),
        })
        .ok();
    };

    // (1) Resolve the target node. We index by `node_id` over `messages` (the
    // forensic log), not `inrun_context` — only `LlmMessage`s carry IDs.
    let target_idx_opt = context.messages.iter().position(|m| match m {
        AgentMessage::Llm(lm) => lm.node_id == Some(request.target),
        _ => false,
    });
    let Some(target_idx) = target_idx_opt else {
        emit_rejected(
            &format!(
                "revert target {} not found in message history",
                request.target
            ),
            tx,
        );
        return;
    };

    // (2) Conservative 0.8.0 rule: reject if the strictly-after span contains
    // a `Message::User`. Auto-rebase is out of scope for 0.8.0 (D6).
    for after in &context.messages[target_idx + 1..] {
        if let AgentMessage::Llm(lm) = after {
            if matches!(lm.message, Message::User { .. }) {
                emit_rejected(
                    "revert refused: abandoned span contains a user message; \
                     auto-rebase is not implemented in 0.8.0",
                    tx,
                );
                return;
            }
        }
    }

    // (3) Collect the abandoned `node_id`s — every Llm message strictly after
    // the target node that carries an id. Used for the event payload and the
    // tag's `abandoned_node_ids` cross-reference.
    let abandoned_node_ids: Vec<crate::types::NodeId> = context.messages[target_idx + 1..]
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(lm) => lm.node_id,
            _ => None,
        })
        .collect();

    // (4) Move the active pointer. The parent-chain walk in Phase 4 will use
    // this to assemble the trunk; until Phase 4 lands, the linear path still
    // wins because `build_working_context` does not yet branch on it — but
    // setting the pointer here is the load-bearing state change that Phase 4
    // depends on, and is observable in tests today.
    context.active_node_id = Some(request.target);

    // (5) Drop the abandoned `inrun_context` entries so the next turn's
    // linear-mode prompt (until Phase 4 lands) no longer carries the
    // abandoned chatter. We compare by message timestamp + node_id when the
    // entry is Live; PrunedMemo / PrunedSilent entries are timestamp-keyed.
    //
    // This is conservative: we keep entries whose `node_id` matches a message
    // at or before `target_idx`, and drop everything else. Phase 4's
    // parent-chain walk will subsume this, but the eager drop here keeps the
    // 0.8.0 step-1 behaviour coherent without requiring Phase 4 to ship in
    // lockstep.
    let surviving_ids: std::collections::HashSet<crate::types::NodeId> = context.messages
        [..=target_idx]
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(lm) => lm.node_id,
            _ => None,
        })
        .collect();
    context.inrun_context.retain(|entry| match entry {
        InRunEntry::Live(AgentMessage::Llm(lm)) => match lm.node_id {
            Some(id) => surviving_ids.contains(&id),
            // Entries without an id pre-date revert mode — keep them.
            None => true,
        },
        // Non-Llm Live entries and pruned markers are unaffected.
        _ => true,
    });

    // (5b) Compose the progress-thread breadcrumb. A revert drops the
    // post-target branch, erasing the model's record of what it just tried; a
    // clean rewind alone leaves a weak model looping and a strong model
    // stopping. Mirroring the `prun_with_memo` "drop content, leave a memo"
    // kernel pattern, we distill the abandoned span into a ONE-LINE breadcrumb —
    // the agent-supplied summary plus the NAMES of the tool calls that were on
    // the abandoned branch — so the rebuilt trunk keeps a progress thread the
    // model can pick up from. The abandoned content itself is NOT reintroduced;
    // only the tool-call names + the summary. General braking-machinery merit:
    // any consumer reverting repeatedly keeps a progress thread.
    //
    // Name SOURCE-SET (F-breadcrumb-tool-source REFINED, 0.11; KC-03 truthful
    // list): the abandoned work is sometimes ON the TARGET node itself, not
    // strictly after it. The #59 shape is a revert whose `step` names the heavy
    // `write_file` CALL node: the abandoned `write_file` is the target node's OWN
    // ToolCall, and the strictly-after span carries only the `revert_to_state`
    // call — so the old strictly-after-only walk mislabels the breadcrumb
    // `revert_to_state abandoned`. We therefore seed the name set with the TARGET
    // cluster's own tool-call name(s) FIRST (the work the model reverted ONTO +
    // abandoned), then add the strictly-after span. Pure no-op when the target is
    // a User / text-only Assistant / tool-result (no ToolCall on the target).
    //
    // KC-03 truthful-list carve-out (ADR-0003 §D3.2): a PINNED revert
    // (`completion`/`step-summary` → Outcome/Checkpoint) ONTO a RESULT node KEEPS
    // the target cluster's content (Rule 2 ADDS the summary; it does NOT replace).
    // Naming the kept target's own call would mislabel still-visible work as
    // "abandoned" (#81 obs #4). So for that case we EXCLUDE step (i) and seed ONLY
    // from the strictly-after span (the span the contract actually shrinks). The
    // abandon-class arm (`failure`/`tangent`) REPLACES the target content, so its
    // own calls ARE abandoned → keep step (i) for abandon-class.
    let target_is_result_node = matches!(
        &context.messages[target_idx],
        AgentMessage::Llm(lm) if matches!(lm.message, Message::ToolResult { .. })
    );
    let pinned_onto_result = !request.category.tag_kind().is_decayable() && target_is_result_node;
    let abandoned_tool_names: Vec<String> = {
        // The revert tool's own name is the TRIGGERING action, not abandoned
        // work — exclude it from the breadcrumb. In the #59 shape the revert
        // call sits in the strictly-after span (the model called
        // `revert_to_state(step="n1")` from the node after its target), so
        // without this guard the breadcrumb mislabels as
        // `(write_file, revert_to_state abandoned)`; the model abandoned the
        // `write_file`, not the revert. Matches `RevertTool::name()`.
        const REVERT_TOOL_NAME: &str = "revert_to_state";
        let mut names: Vec<String> = Vec::new();
        let push_call_names =
            |lm: &crate::types::LlmMessage, names: &mut Vec<String>| match &lm.message {
                Message::Assistant { content, .. } => {
                    for block in content {
                        if let crate::types::Content::ToolCall { name, .. } = block {
                            if name != REVERT_TOOL_NAME && !names.contains(name) {
                                names.push(name.clone());
                            }
                        }
                    }
                }
                Message::ToolResult { tool_name, .. } => {
                    if tool_name != REVERT_TOOL_NAME && !names.contains(tool_name) {
                        names.push(tool_name.clone());
                    }
                }
                Message::User { .. } => {}
            };
        // (i) the TARGET cluster's own tool-call(s) — the #59 reverted-onto work.
        // SKIPPED for a pinned-onto-result revert: the kept target's own work is
        // NOT abandoned (Rule 2 ADDS the summary; content stays) — KC-03 §D3.2.
        if !pinned_onto_result {
            if let AgentMessage::Llm(lm) = &context.messages[target_idx] {
                push_call_names(lm, &mut names);
            }
        }
        // (ii) the strictly-after abandoned span (the CC-16 walk).
        for m in &context.messages[target_idx + 1..] {
            let AgentMessage::Llm(lm) = m else { continue };
            push_call_names(lm, &mut names);
        }
        names
    };
    let summary_text = request.summary.clone().unwrap_or_default();
    let breadcrumb = compose_revert_breadcrumb(&summary_text, &abandoned_tool_names);

    // (6) Attach the breadcrumb as the summary tag on the target node.
    //
    // F-empty-tail-summary.a (CC-18, GitHub #72 v2): the single-axis rule gates
    // tag attachment by `is_decayable(category)` crossed with whether a tail was
    // actually dropped:
    //
    //  - **Decayable** (`failure`→`Lesson` / `tangent`→`Finding`): ALWAYS
    //    attach. The lesson *replaces* removed cluster content (the collapse
    //    drops the whole {n0,n1} body), so it is always meaningful — even with
    //    an empty tail (the #59 shape: the abandoned work is the target's own
    //    just-completed action).
    //  - **Pinned** (`completion`→`Outcome` / `step-summary`→`Checkpoint`):
    //    attach ONLY when a tail was dropped (`abandoned_node_ids` non-empty).
    //    Pinned categories KEEP the cluster verbatim, so their only reclamation
    //    is the dropped tail; with no tail the pinned tag would merely restate
    //    content still fully visible on the kept cluster — pure overhead. An
    //    empty-tail pinned revert is a clean no-op: the active-pointer move +
    //    `RevertApplied` event still fire (recordkeeping unchanged), but no tag
    //    is attached.
    //
    // Empty `breadcrumb` text only when the agent omitted `summary` AND the
    // abandoned span had no tool calls — Phase 5's render policy can still
    // classify (kind is well-defined) and a future fallback generator slots into
    // the empty-text branch.
    let category_decayable = request.category.tag_kind().is_decayable();
    if category_decayable || !abandoned_node_ids.is_empty() {
        let tag = NodeTag::new(
            request.category.tag_kind(),
            breadcrumb,
            current_turn as u32,
            abandoned_node_ids.clone(),
        );
        if let AgentMessage::Llm(lm) = &mut context.messages[target_idx] {
            lm.add_tag(tag);
        }
    }

    // (7) Emit success event.
    tx.send(AgentEvent::RevertApplied {
        loop_id: loop_id.to_string(),
        category: request.category,
        target: Some(request.target),
        abandoned_node_ids,
        summary: request.summary.clone(),
        applied: true,
        reason: None,
        timestamp: chrono::Utc::now(),
    })
    .ok();
}

#[cfg(test)]
mod apply_revert_tests {
    //! Composition I Phase 3 unit tests for [`apply_revert`].
    //!
    //! These exercise the synchronous between-turn application directly,
    //! without spinning up `MockProvider` — the goal is to lock in the four
    //! contractual outcomes: (a) successful revert, (b) unknown target
    //! rejection, (c) user-message-in-span rejection, (d) tag attachment +
    //! event payload.
    use super::*;
    use crate::tools::revert::RevertRequest;
    use crate::types::{
        AgentMessage, Content, LlmMessage, Message, NodeId, RevertCategory, StopReason, TagKind,
        Usage,
    };

    fn user_msg_node(text: &str, ts: u64, node: NodeId, parent: Option<NodeId>) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: text.to_string(),
                }],
                timestamp: ts,
            })
            .with_node_identity(node, parent),
        )
    }

    fn assistant_msg_node(
        text: &str,
        ts: u64,
        node: NodeId,
        parent: Option<NodeId>,
    ) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: text.to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: ts,
                error_message: None,
            })
            .with_node_identity(node, parent),
        )
    }

    fn drain_events(rx: &mut mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    #[test]
    fn apply_revert_success_moves_pointer_attaches_tag_emits_event() {
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a sort", 1, NodeId(10), None),
                assistant_msg_node("I'll write bubble sort", 2, NodeId(11), Some(NodeId(10))),
                assistant_msg_node("timed out", 3, NodeId(12), Some(NodeId(11))),
            ],
            next_node_id: 13,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(10),
            summary: Some("bubble sort timed out — try a faster algorithm".into()),
        };

        apply_revert(&mut ctx, &req, 7, &tx, "loop-1");

        // (a) active pointer moved
        assert_eq!(ctx.active_node_id, Some(NodeId(10)));

        // (b) summary tag attached on the target with the correct kind + turn
        let target_msg = match &ctx.messages[0] {
            AgentMessage::Llm(lm) => lm,
            _ => unreachable!(),
        };
        assert_eq!(target_msg.tags.len(), 1);
        assert_eq!(target_msg.tags[0].kind, TagKind::Lesson);
        // The tag text is now the composed breadcrumb. The abandoned span here
        // is text-only assistant messages (no tool calls), so the breadcrumb is
        // the summary-only shape `reverted past: <summary>`.
        assert_eq!(
            target_msg.tags[0].text,
            "reverted past: bubble sort timed out — try a faster algorithm"
        );
        assert_eq!(target_msg.tags[0].created_at_turn, 7);
        assert_eq!(
            target_msg.tags[0].abandoned_node_ids,
            vec![NodeId(11), NodeId(12)]
        );

        // (c) forensic log is intact — abandoned messages still present
        assert_eq!(ctx.messages.len(), 3);

        // (d) success event emitted
        let events = drain_events(&mut rx);
        let revert_event = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::RevertApplied {
                    applied,
                    target,
                    abandoned_node_ids,
                    category,
                    summary,
                    reason,
                    ..
                } if *applied => Some((target, abandoned_node_ids, category, summary, reason)),
                _ => None,
            })
            .expect("a successful RevertApplied event must be emitted");
        assert_eq!(*revert_event.0, Some(NodeId(10)));
        assert_eq!(*revert_event.1, vec![NodeId(11), NodeId(12)]);
        assert_eq!(*revert_event.2, RevertCategory::Failure);
        assert_eq!(
            revert_event.3.as_deref(),
            Some("bubble sort timed out — try a faster algorithm")
        );
        assert!(revert_event.4.is_none());
    }

    #[test]
    fn apply_revert_pinned_with_dropped_tail_attaches_tag() {
        // CC-18 F-empty-tail-summary.a (c): a `completion` (pinned/Outcome)
        // revert that DROPS a tail (target is an earlier node) attaches the
        // pinned tag — the tag summarizes the reclaimed tail.
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("seal the sub-task", 1, NodeId(10), None),
                assistant_msg_node("sub-task done", 2, NodeId(11), Some(NodeId(10))),
                assistant_msg_node("a follow-on detour", 3, NodeId(12), Some(NodeId(11))),
            ],
            next_node_id: 13,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Completion,
            target: NodeId(11), // tail = n12 is dropped
            summary: Some("sub-task sealed".into()),
        };

        apply_revert(&mut ctx, &req, 5, &tx, "loop-1");

        // The pinned tag IS attached (a tail was dropped).
        let target_msg = match &ctx.messages[1] {
            AgentMessage::Llm(lm) => lm,
            _ => unreachable!(),
        };
        assert_eq!(
            target_msg.tags.len(),
            1,
            "pinned revert WITH a dropped tail must attach the outcome tag"
        );
        assert_eq!(target_msg.tags[0].kind, TagKind::Outcome);
        // Active pointer moved + success event fired with the abandoned tail.
        assert_eq!(ctx.active_node_id, Some(NodeId(11)));
        let events = drain_events(&mut rx);
        let applied = events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::RevertApplied { applied: true, abandoned_node_ids, .. }
                    if abandoned_node_ids == &vec![NodeId(12)]
            )
        });
        assert!(
            applied,
            "RevertApplied{{applied:true}} with the dropped tail"
        );
    }

    #[test]
    fn apply_revert_pinned_empty_tail_is_noop_no_tag() {
        // CC-18 F-empty-tail-summary.a (d): a `step-summary` (pinned/Checkpoint)
        // revert that targets the LAST node (no tail to drop) is a clean no-op:
        // NO tag is attached (the pinned tag would merely restate still-visible
        // content), but the active-pointer move + `RevertApplied` event STILL
        // fire (recordkeeping unchanged).
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("checkpoint here", 1, NodeId(10), None),
                assistant_msg_node("the step I just finished", 2, NodeId(11), Some(NodeId(10))),
            ],
            next_node_id: 12,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::StepSummary,
            target: NodeId(11), // the LAST node → empty tail
            summary: Some("sealing the step I just finished".into()),
        };

        apply_revert(&mut ctx, &req, 5, &tx, "loop-1");

        // NO tag attached (empty-tail pinned no-op).
        let target_msg = match &ctx.messages[1] {
            AgentMessage::Llm(lm) => lm,
            _ => unreachable!(),
        };
        assert!(
            target_msg.tags.is_empty(),
            "empty-tail pinned revert must attach NO tag (no-op)"
        );
        // But the pointer STILL moves + the event STILL fires.
        assert_eq!(ctx.active_node_id, Some(NodeId(11)));
        let events = drain_events(&mut rx);
        let applied = events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::RevertApplied { applied: true, abandoned_node_ids, .. }
                    if abandoned_node_ids.is_empty()
            )
        });
        assert!(
            applied,
            "the empty-tail no-op still emits RevertApplied{{applied:true}}"
        );
    }

    #[test]
    fn apply_revert_decayable_empty_tail_still_attaches_lesson() {
        // CC-18 F-empty-tail-summary.a — the DECAYABLE side: a `failure` revert
        // onto the LAST node (empty tail, the #59 shape) STILL attaches the
        // lesson, because the lesson REPLACES removed cluster content (always
        // meaningful), unlike a pinned tag which would restate visible content.
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a plan", 1, NodeId(10), None),
                assistant_msg_node("plan-v1 was wrong", 2, NodeId(11), Some(NodeId(10))),
            ],
            next_node_id: 12,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(11), // empty tail
            summary: Some("plan-v1 was wrong".into()),
        };

        apply_revert(&mut ctx, &req, 5, &tx, "loop-1");

        let target_msg = match &ctx.messages[1] {
            AgentMessage::Llm(lm) => lm,
            _ => unreachable!(),
        };
        assert_eq!(
            target_msg.tags.len(),
            1,
            "decayable (failure) revert always attaches the lesson, even empty-tail"
        );
        assert_eq!(target_msg.tags[0].kind, TagKind::Lesson);
    }

    #[test]
    fn apply_revert_unknown_target_is_rejected() {
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("hello", 1, NodeId(10), None),
                assistant_msg_node("hi", 2, NodeId(11), Some(NodeId(10))),
            ],
            next_node_id: 12,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Tangent,
            target: NodeId(99),
            summary: None,
        };

        apply_revert(&mut ctx, &req, 0, &tx, "loop-1");

        // Pointer unchanged, no tags attached, no mutation.
        assert!(ctx.active_node_id.is_none());
        for m in &ctx.messages {
            if let AgentMessage::Llm(lm) = m {
                assert!(lm.tags.is_empty());
            }
        }

        let events = drain_events(&mut rx);
        let rejected = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::RevertApplied {
                    applied: false,
                    reason,
                    target,
                    abandoned_node_ids,
                    ..
                } => Some((reason, target, abandoned_node_ids)),
                _ => None,
            })
            .expect("rejection event must be emitted");
        assert!(rejected.0.as_deref().unwrap_or("").contains("not found"));
        assert_eq!(*rejected.1, None);
        assert!(rejected.2.is_empty());
    }

    #[test]
    fn apply_revert_refuses_when_span_contains_user_message() {
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a sort", 1, NodeId(10), None),
                assistant_msg_node("trying bubble", 2, NodeId(11), Some(NodeId(10))),
                user_msg_node("actually use quicksort", 3, NodeId(12), Some(NodeId(11))),
                assistant_msg_node("ok", 4, NodeId(13), Some(NodeId(12))),
            ],
            next_node_id: 14,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(10),
            summary: Some("nope".into()),
        };

        apply_revert(&mut ctx, &req, 0, &tx, "loop-1");

        // No state mutation on rejection.
        assert!(ctx.active_node_id.is_none());
        let target_tags = match &ctx.messages[0] {
            AgentMessage::Llm(lm) => &lm.tags,
            _ => unreachable!(),
        };
        assert!(target_tags.is_empty());

        let events = drain_events(&mut rx);
        let reason = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::RevertApplied {
                    applied: false,
                    reason,
                    ..
                } => reason.clone(),
                _ => None,
            })
            .expect("user-message rejection event must be emitted");
        assert!(reason.contains("user message"));
    }

    #[test]
    fn apply_revert_drops_inrun_context_entries_off_trunk() {
        // Two assistant messages stamped with node IDs; inrun_context mirrors them.
        // Reverting to n10 must drop the n11 inrun entry but keep older entries
        // without node_ids (legacy / pre-revert-mode messages).
        let a10 = assistant_msg_node("on-trunk", 1, NodeId(10), None);
        let a11 = assistant_msg_node("off-trunk", 2, NodeId(11), Some(NodeId(10)));
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("seed", 0, NodeId(9), None),
                a10.clone(),
                a11.clone(),
            ],
            inrun_context: vec![InRunEntry::Live(a10.clone()), InRunEntry::Live(a11.clone())],
            next_node_id: 12,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Completion,
            target: NodeId(10),
            summary: None,
        };

        apply_revert(&mut ctx, &req, 0, &tx, "loop-1");

        assert_eq!(ctx.active_node_id, Some(NodeId(10)));
        // Only the n10 live entry survives in inrun_context.
        let live_ids: Vec<Option<NodeId>> = ctx
            .inrun_context
            .iter()
            .filter_map(|e| match e {
                InRunEntry::Live(AgentMessage::Llm(lm)) => Some(lm.node_id),
                _ => None,
            })
            .collect();
        assert_eq!(live_ids, vec![Some(NodeId(10))]);
    }

    #[test]
    fn apply_revert_summary_none_attaches_empty_text_tag() {
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("seed", 1, NodeId(10), None),
                assistant_msg_node("trail", 2, NodeId(11), Some(NodeId(10))),
            ],
            next_node_id: 12,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::StepSummary,
            target: NodeId(10),
            summary: None,
        };

        apply_revert(&mut ctx, &req, 0, &tx, "loop-1");
        let tag = match &ctx.messages[0] {
            AgentMessage::Llm(lm) => &lm.tags[0],
            _ => unreachable!(),
        };
        assert_eq!(tag.kind, TagKind::Checkpoint);
        assert_eq!(tag.text, "");
    }

    // ── progress-thread breadcrumb ──

    fn assistant_toolcall_node(
        tool: &str,
        ts: u64,
        node: NodeId,
        parent: Option<NodeId>,
    ) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::ToolCall {
                    id: format!("tc-{ts}"),
                    name: tool.to_string(),
                    arguments: serde_json::json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: ts,
                error_message: None,
            })
            .with_node_identity(node, parent),
        )
    }

    #[test]
    fn breadcrumb_composed_from_summary_and_tool_names() {
        // The abandoned span carries two tool calls; the breadcrumb names them
        // alongside the agent summary so the model knows concretely what it
        // tried-and-abandoned.
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a plan", 1, NodeId(10), None),
                assistant_toolcall_node("write_file", 2, NodeId(11), Some(NodeId(10))),
                assistant_toolcall_node("run_tests", 3, NodeId(12), Some(NodeId(11))),
            ],
            next_node_id: 13,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(10),
            summary: Some("wrote plan-v1.md".into()),
        };

        apply_revert(&mut ctx, &req, 4, &tx, "loop-1");

        let tag = match &ctx.messages[0] {
            AgentMessage::Llm(lm) => &lm.tags[0],
            _ => unreachable!(),
        };
        assert_eq!(
            tag.text,
            "reverted past: wrote plan-v1.md (write_file, run_tests abandoned)"
        );
    }

    #[test]
    fn breadcrumb_summary_only_when_span_has_no_tool_calls() {
        // A pure-text abandoned branch (no tool calls) falls back to the
        // summary alone — a graceful narrowing, not a deferred feature.
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a plan", 1, NodeId(10), None),
                assistant_msg_node(
                    "thinking out loud, no tools",
                    2,
                    NodeId(11),
                    Some(NodeId(10)),
                ),
            ],
            next_node_id: 12,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Tangent,
            target: NodeId(10),
            summary: Some("approach v1 was tangential".into()),
        };

        apply_revert(&mut ctx, &req, 4, &tx, "loop-1");

        let tag = match &ctx.messages[0] {
            AgentMessage::Llm(lm) => &lm.tags[0],
            _ => unreachable!(),
        };
        assert_eq!(tag.text, "reverted past: approach v1 was tangential");
    }

    #[test]
    fn abandoned_branch_content_not_reintroduced() {
        // The rebuilt active trunk must NOT carry the abandoned branch's body
        // text — only the one-line breadcrumb. We assert the abandoned message
        // bodies are absent from the woven trunk after the revert.
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a plan", 1, NodeId(10), None),
                assistant_toolcall_node("write_file", 2, NodeId(11), Some(NodeId(10))),
                assistant_msg_node(
                    "ABANDONED-BODY-secret detail that must not survive",
                    3,
                    NodeId(12),
                    Some(NodeId(11)),
                ),
            ],
            next_node_id: 13,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(10),
            summary: Some("wrote plan-v1.md".into()),
        };

        apply_revert(&mut ctx, &req, 4, &tx, "loop-1");

        // Build the rendered trunk the way the loop does (revert mode).
        let policy = crate::types::RevertRenderPolicy::default();
        let trunk = ctx.build_trunk_context_with_policy(&policy, 4);
        let woven = AgentContext::weave_braking_annotations(trunk);

        let all_text: String = woven
            .iter()
            .map(|m| match m {
                AgentMessage::Llm(lm) => match &lm.message {
                    Message::User { content, .. }
                    | Message::Assistant { content, .. }
                    | Message::ToolResult { content, .. } => content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                },
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" ");

        // The abandoned body text is gone; the one-line breadcrumb survives.
        assert!(
            !all_text.contains("ABANDONED-BODY-secret"),
            "abandoned branch content must not be reintroduced: {all_text}"
        );
        // KC-03 single-label: the woven render carries the breadcrumb as a SINGLE
        // `[<kind>: …]` label — the tag's own `reverted past: ` prefix is stripped
        // at weave time to avoid the `[revert-lesson: reverted past: …]` double-label
        // (ADR-0003 §D3.2). The breadcrumb summary itself still survives.
        assert!(
            all_text.contains("[revert-lesson: wrote plan-v1.md"),
            "the one-line breadcrumb must survive into the trunk (single-label): {all_text}"
        );
    }

    #[test]
    fn apply_revert_onto_call_node_names_target_cluster_tool() {
        // F-breadcrumb-tool-source REFINED (0.11): the #59 shape — the model
        // reverts ONTO the heavy `write_file` CALL node itself (step names that
        // node). The abandoned `write_file` is the TARGET node's own ToolCall,
        // NOT in the strictly-after span (which holds only the revert call). The
        // breadcrumb must name `write_file`, not `revert_to_state`.
        let write_call = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::ToolCall {
                    id: "call_abc".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({ "path": "plan-v1.md", "content": "HEAVY" }),
                }],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 2,
                error_message: None,
            })
            .with_node_identity(NodeId(0), None),
        );
        let mut ctx = AgentContext {
            messages: vec![write_call],
            next_node_id: 1,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(0),
            summary: Some("approach v1 was tangential".into()),
        };

        apply_revert(&mut ctx, &req, 3, &tx, "loop-1");

        let tag_text = match &ctx.messages[0] {
            AgentMessage::Llm(lm) => lm.tags[0].text.clone(),
            _ => unreachable!(),
        };
        assert!(
            tag_text.contains("write_file"),
            "breadcrumb must name the abandoned write_file work: {tag_text}"
        );
        assert!(
            !tag_text.contains("revert_to_state"),
            "breadcrumb must NOT mislabel as revert_to_state: {tag_text}"
        );
    }

    #[test]
    fn breadcrumb_onto_tool_result_tip_excludes_revert_tool_name() {
        // The #59-canonical (minimax) shape — the model reverts ONTO the
        // tool-RESULT node (`step="n1"`), and its own `revert_to_state` call
        // sits in the strictly-after span. Without the revert-tool exclusion
        // the breadcrumb mislabels as `(write_file, revert_to_state abandoned)`.
        // After Fix B the breadcrumb must name `write_file` only.
        //
        // Tree: n0 user → n1 write_file CALL → n2 write_file RESULT (target) →
        //       n3 assistant carrying the `revert_to_state` ToolCall (the
        //       triggering action, strictly-after the target).
        let write_call = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::ToolCall {
                    id: "call_abc".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({ "path": "plan-v1.md", "content": "HEAVY" }),
                }],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 2,
                error_message: None,
            })
            .with_node_identity(NodeId(1), Some(NodeId(0))),
        );
        let write_result = AgentMessage::Llm(
            LlmMessage::new(Message::ToolResult {
                tool_call_id: "call_abc".into(),
                tool_name: "write_file".into(),
                content: vec![Content::Text {
                    text: "Wrote 333 bytes".into(),
                }],
                is_error: false,
                timestamp: 3,
            })
            .with_node_identity(NodeId(2), Some(NodeId(1))),
        );
        let revert_call = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::ToolCall {
                    id: "call_rev".into(),
                    name: "revert_to_state".into(),
                    arguments: serde_json::json!({ "category": "failure", "step": "n2" }),
                }],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 4,
                error_message: None,
            })
            .with_node_identity(NodeId(3), Some(NodeId(2))),
        );
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a plan", 1, NodeId(0), None),
                write_call,
                write_result,
                revert_call,
            ],
            next_node_id: 4,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(2), // revert onto the tool-RESULT node
            summary: Some("approach v1 was tangential".into()),
        };

        apply_revert(&mut ctx, &req, 5, &tx, "loop-1");

        // The breadcrumb is attached to the target node (the tool-result, n2).
        let tag_text = match &ctx.messages[2] {
            AgentMessage::Llm(lm) => lm.tags[0].text.clone(),
            _ => unreachable!(),
        };
        assert!(
            tag_text.contains("write_file"),
            "breadcrumb must name the abandoned write_file work: {tag_text}"
        );
        assert!(
            !tag_text.contains("revert_to_state"),
            "breadcrumb must NOT name the revert tool's own call: {tag_text}"
        );
    }

    /// KC-03 (#81 obs #4) — TRUTHFUL abandoned-list on a PINNED-onto-RESULT
    /// revert: the breadcrumb names ONLY the tools whose results were SHRUNK
    /// (strictly after X), NOT the kept target cluster's own tool (whose content
    /// stays — Rule 2 ADD). A pinned (`completion`) revert onto the FIRST result
    /// of a parallel cluster (mem_help KEPT, on-trunk) must name only the shrunk
    /// `skill_help`, never the kept `memory_help` (ADR-0003 §D3.2).
    #[test]
    fn pinned_revert_onto_result_breadcrumb_names_only_shrunk_tools() {
        // n0 user → n1 assistant [mem_call(memory_help), skill_call(skill_help)] →
        // n2 memory_help RESULT (parent n1) → n3 skill_help RESULT (parent n2).
        let parallel_call = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![
                    Content::ToolCall {
                        id: "mem_call".into(),
                        name: "memory_help".into(),
                        arguments: serde_json::json!({ "q": "recent" }),
                    },
                    Content::ToolCall {
                        id: "skill_call".into(),
                        name: "skill_help".into(),
                        arguments: serde_json::json!({ "q": "list" }),
                    },
                ],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 2,
                error_message: None,
            })
            .with_node_identity(NodeId(1), Some(NodeId(0))),
        );
        let mem_result = AgentMessage::Llm(
            LlmMessage::new(Message::ToolResult {
                tool_call_id: "mem_call".into(),
                tool_name: "memory_help".into(),
                content: vec![Content::Text {
                    text: "memory: ShortTerm…".into(),
                }],
                is_error: false,
                timestamp: 3,
            })
            .with_node_identity(NodeId(2), Some(NodeId(1))),
        );
        let skill_result = AgentMessage::Llm(
            LlmMessage::new(Message::ToolResult {
                tool_call_id: "skill_call".into(),
                tool_name: "skill_help".into(),
                content: vec![Content::Text {
                    text: "skills: …".into(),
                }],
                is_error: false,
                timestamp: 4,
            })
            .with_node_identity(NodeId(3), Some(NodeId(2))),
        );
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("do parallel work", 1, NodeId(0), None),
                parallel_call,
                mem_result,
                skill_result,
            ],
            next_node_id: 4,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        // PINNED (completion → Outcome) revert ONTO the first result n2 (mem_help
        // KEPT on-trunk); skill_help (n3) is strictly-after → shrunk.
        let req = RevertRequest {
            category: RevertCategory::Completion,
            target: NodeId(2),
            summary: Some("sealed the lookup".into()),
        };

        apply_revert(&mut ctx, &req, 5, &tx, "loop-1");

        // The breadcrumb is attached to the kept target node (n2).
        let tag_text = match &ctx.messages[2] {
            AgentMessage::Llm(lm) => lm.tags[0].text.clone(),
            _ => unreachable!(),
        };
        assert!(
            tag_text.contains("skill_help"),
            "breadcrumb must name the SHRUNK skill_help: {tag_text}"
        );
        assert!(
            !tag_text.contains("memory_help"),
            "breadcrumb must NOT name the KEPT memory_help (Rule 2 ADD, not abandoned): {tag_text}"
        );
    }

    /// CC-18 iter-2 (#73 / D-TEST-0068) — END-TO-END persistence via REAL
    /// `apply_revert` (not a synthetic lone-breadcrumb fixture). A real heavy
    /// write_file cluster is reverted (`failure`) via `apply_revert`, then the
    /// model CONTINUES FORWARD (new nodes appended, active pointer advanced).
    /// The production render pipeline (`build_trunk_context` →
    /// `collapse_abandon_class_cluster` → `decay_tags_by_policy`) must:
    ///  (a) mask the abandoned heavy body on EVERY subsequent render (persistent,
    ///      mid-trunk) — the iter-1 tip-only collapse could not do this;
    ///  (b) DROP the now-content-less node once its lesson is out-of-window (the
    ///      reachable decay-drop F-collapsed-node-decay.a);
    ///  (c) never mutate the forensic `messages` log.
    #[test]
    fn persistent_collapse_end_to_end_via_real_apply_revert() {
        use crate::types::node_tag::RevertRenderPolicy;
        // n0 user → n1 write_file CALL (heavy plan-v1) → n2 write_file RESULT.
        let heavy_call = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::ToolCall {
                    id: "call_v1".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": "plan-v1.md",
                        "content": "PLAN-V1-HEAVY-BODY"
                    }),
                }],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 2,
                error_message: None,
            })
            .with_node_identity(NodeId(1), Some(NodeId(0))),
        );
        let heavy_result = AgentMessage::Llm(
            LlmMessage::new(Message::ToolResult {
                tool_call_id: "call_v1".into(),
                tool_name: "write_file".into(),
                content: vec![Content::Text {
                    text: "Wrote 304 bytes".into(),
                }],
                is_error: false,
                timestamp: 3,
            })
            .with_node_identity(NodeId(2), Some(NodeId(1))),
        );
        let mut ctx = AgentContext {
            messages: vec![
                user_msg_node("write a plan", 1, NodeId(0), None),
                heavy_call,
                heavy_result,
            ],
            next_node_id: 3,
            ..Default::default()
        };

        // REAL revert: `failure` onto the tool-RESULT node n2 (the #59 shape) at
        // turn 1. apply_revert moves the active pointer to n2 + attaches the
        // abandon-class lesson breadcrumb naming the abandoned write_file work.
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let req = RevertRequest {
            category: RevertCategory::Failure,
            target: NodeId(2),
            summary: Some("plan-v1 abandoned".into()),
        };
        apply_revert(&mut ctx, &req, 1, &tx, "loop-1");
        assert_eq!(ctx.active_node_id, Some(NodeId(2)));

        // CONTINUE FORWARD: the model writes plan-v2 (n3 call + n4 result),
        // parented off the reverted-to n2. The active pointer advances to n4, so
        // the reverted n1/n2 cluster is now MID-trunk on later renders.
        let v2_call = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::ToolCall {
                    id: "call_v2".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": "plan-v2.md",
                        "content": "PLAN-V2-BODY"
                    }),
                }],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 4,
                error_message: None,
            })
            .with_node_identity(NodeId(3), Some(NodeId(2))),
        );
        let v2_result = AgentMessage::Llm(
            LlmMessage::new(Message::ToolResult {
                tool_call_id: "call_v2".into(),
                tool_name: "write_file".into(),
                content: vec![Content::Text {
                    text: "Wrote 721 bytes".into(),
                }],
                is_error: false,
                timestamp: 5,
            })
            .with_node_identity(NodeId(4), Some(NodeId(3))),
        );
        ctx.messages.push(v2_call);
        ctx.messages.push(v2_result);
        ctx.active_node_id = Some(NodeId(4));
        ctx.next_node_id = 5;

        let before = serde_json::to_string(&ctx.messages).unwrap();
        let policy = RevertRenderPolicy {
            lesson_window_turns: 3,
            lesson_window_count: 0,
        };

        // The full production render pipeline (mirrors streaming.rs).
        let render = |turn: u32| -> Vec<AgentMessage> {
            let raw = ctx.build_trunk_context();
            let collapsed = ctx.collapse_abandon_class_cluster(raw, &policy, turn);
            AgentContext::decay_tags_by_policy(collapsed, &policy, turn)
        };

        fn has_heavy(trunk: &[AgentMessage], needle: &str) -> bool {
            trunk.iter().any(|m| match m {
                AgentMessage::Llm(lm) => match &lm.message {
                    Message::Assistant { content, .. } => content.iter().any(|b| match b {
                        Content::ToolCall { arguments, .. } => {
                            arguments.to_string().contains(needle)
                        }
                        _ => false,
                    }),
                    _ => false,
                },
                _ => false,
            })
        }

        // (a) PERSISTENT masking: on turns 1, 2, 3 (the model kept going), the
        // abandoned plan-v1 heavy body is GONE on EVERY render — the #73 gap.
        for turn in [1u32, 2, 3] {
            let r = render(turn);
            assert!(
                !has_heavy(&r, "PLAN-V1-HEAVY-BODY"),
                "turn {turn}: abandoned plan-v1 stays masked mid-trunk (persistent)"
            );
            assert!(
                has_heavy(&r, "PLAN-V2-BODY"),
                "turn {turn}: the live plan-v2 is untouched"
            );
        }

        // (b) REACHABLE decay-drop: past the window (turn 10) the collapsed n1
        // node (whose content was reclaimed) is dropped; plan-v1 never re-appears.
        let decayed = render(10);
        let has_n1 = decayed
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(lm) if lm.node_id == Some(NodeId(1))));
        assert!(
            !has_n1,
            "out-of-window: the collapsed plan-v1 node is dropped (reachable decay-drop)"
        );
        assert!(
            !has_heavy(&decayed, "PLAN-V1-HEAVY-BODY"),
            "out-of-window: plan-v1 never re-appears"
        );

        // (c) the forensic log is byte-identical — render-only, never mutated.
        let after = serde_json::to_string(&ctx.messages).unwrap();
        assert_eq!(
            before, after,
            "the persistent collapse must NEVER mutate context.messages"
        );
    }
}
