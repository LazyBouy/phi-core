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
    let mut pending = match apply_input_filters(raw, &config.input_filters, tx, &loop_id) {
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
                if !before_turn(&context.messages, turn) {
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
                    let compaction_allowed = config
                        .before_compaction_start
                        .as_ref()
                        .map_or(true, |hook| hook(estimated, msgs_before));

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
                                    continuation_kind: context.continuation_kind.clone(),
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
                            );
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
                                hook(msgs_before, messages_after, estimated, tokens_after);
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
                                hook(msgs_before, messages_after, estimated, tokens_after);
                            }
                        }
                    } // if compaction_allowed
                }
            }

            // Stream assistant response
            let message = stream_assistant_response(context, config, tx, cancel, &loop_id).await;

            let agent_msg: AgentMessage =
                AgentMessage::from(message.clone()).with_turn_id(current_turn_id.clone());
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
                            on_error(err_str);
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
                    &loop_id,
                )
                .await;

                tool_results = execution.tool_results;
                steering_after_tools = execution.steering_messages;

                for result in &tool_results {
                    let am: AgentMessage =
                        AgentMessage::from(result.clone()).with_turn_id(current_turn_id.clone());
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
                after_turn(&context.messages, &turn_usage);
            }

            // Check steering after turn — filter before assigning to pending
            if let Some(steering) = steering_after_tools.take() {
                if !steering.is_empty() {
                    match apply_input_filters(steering, &config.input_filters, tx, &loop_id) {
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
            pending = match apply_input_filters(raw, &config.input_filters, tx, &loop_id) {
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
            match apply_input_filters(follow_ups, &config.input_filters, tx, &loop_id) {
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
