use super::config::*;
use super::core::{agent_loop, agent_loop_continue};
use super::helpers::derive_config_segment;
use crate::types::*;
use chrono::Utc;
use futures::future::join_all;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Internal: run N agent loop branches concurrently and collect their outcomes.
///
/// Uses `futures::future::join_all` (not `tokio::spawn`) for the branch futures so
/// the `'static` bound is not imposed on `AgentLoopConfig` hook fields. Each branch
/// gets its own forwarding task (`tokio::spawn`) that intercepts `AgentEnd.usage`
/// and forwards all events to the shared `tx`.
async fn run_parallel_branches(
    prompts: Vec<AgentMessage>,
    contexts: Vec<AgentContext>,
    configs: Vec<AgentLoopConfig>,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Vec<ParallelLoopOutcome> {
    let branch_futures: Vec<_> = contexts
        .into_iter()
        .zip(configs.into_iter())
        .enumerate()
        .map(|(i, (mut ctx, config))| {
            let loop_id = ctx.loop_id.clone().unwrap_or_default();
            let prompts = prompts.clone();
            let main_tx = tx.clone();
            let cancel = cancel.clone();

            async move {
                let (branch_tx, branch_rx) = mpsc::unbounded_channel::<AgentEvent>();
                let (usage_tx, usage_rx) = tokio::sync::oneshot::channel::<Usage>();

                // Record context size BEFORE the branch mutates it.
                let original_context_len = ctx.messages.len();

                // Forwarder: intercepts AgentEnd for usage, forwards all events to main_tx.
                // tokio::spawn is valid here: branch_rx, cloned main_tx, and usage_tx are all
                // owned Send + 'static values.
                tokio::spawn(async move {
                    let mut branch_rx = branch_rx;
                    let mut last_usage = Usage::default();
                    while let Some(event) = branch_rx.recv().await {
                        if let AgentEvent::AgentEnd { ref usage, .. } = event {
                            last_usage = usage.clone();
                        }
                        main_tx.send(event).ok();
                    }
                    // branch_tx is dropped when agent_loop returns -> recv() yields None ->
                    // send usage back, unblocking usage_rx.await below.
                    usage_tx.send(last_usage).ok();
                });

                // Route to agent_loop_continue when prompts is empty: the user query
                // is already in the context (agent_loop_continue mode). Preconditions
                // (non-empty context, not ending on assistant) are asserted by
                // agent_loop_parallel before dispatch.
                let new_messages = if prompts.is_empty() {
                    agent_loop_continue(&mut ctx, &config, branch_tx, cancel).await
                } else {
                    agent_loop(prompts, &mut ctx, &config, branch_tx, cancel).await
                };
                let usage = usage_rx.await.unwrap_or_default();

                ParallelLoopOutcome {
                    config_index: i,
                    loop_id,
                    context: ctx,
                    new_messages,
                    usage,
                    original_context_len,
                }
            }
        })
        .collect();

    join_all(branch_futures).await
}

/// Run multiple agent loop configurations concurrently from a shared base context,
/// evaluate the results with the supplied strategy, and return the selected outcome.
///
/// This is the foundation for evaluational parallelism. The standard single-loop case
/// is a transparent special case: one config + [`crate::evaluation::TransparentEvaluation`].
///
/// # Branch cloning
///
/// `base_context` is cloned once per config entry. Tools are `Arc`-shared (zero copy);
/// the message history is deep-cloned so branches start from identical state but diverge
/// independently. All branches inherit the same `session_id` for traceability.
///
/// # Loop IDs
///
/// Each branch receives a distinct `loop_id`:
/// ```text
/// "{session_id}.{config_segment}.{N}"
/// ```
/// where `config_segment` is derived from `config.config_id` or auto-derived from
/// provider + model + thinking level via [`derive_config_segment`].
///
/// # Events
///
/// Events from all branches are forwarded to `tx` interleaved. Each branch's
/// `AgentStart` carries a distinct `loop_id` for demultiplexing. A
/// [`AgentEvent::ParallelLoopStart`] / [`AgentEvent::ParallelLoopEnd`] pair
/// brackets the entire parallel execution.
///
/// # Session continuity
///
/// Feed `selected_context` into [`agent_loop_continue`] to resume the session
/// normally after the parallel evaluation --- this is a single-loop operation,
/// not a special session mode.
///
/// # `agent_loop_continue` mode
///
/// When `prompts` is empty, each branch is dispatched via [`agent_loop_continue`]
/// instead of [`agent_loop`]. This supports the "resume from existing context"
/// pattern where the user query is already the last message in `base_context`.
/// The same preconditions as `agent_loop_continue` apply: `base_context.messages`
/// must be non-empty and must not end on an assistant message.
pub async fn agent_loop_parallel(
    prompts: Vec<AgentMessage>,
    mut base_context: AgentContext,
    configs: Vec<AgentLoopConfig>,
    strategy: Arc<dyn EvaluationStrategy>,
    tx: mpsc::UnboundedSender<AgentEvent>,
    cancel: tokio_util::sync::CancellationToken,
) -> ParallelLoopResult {
    assert!(
        !configs.is_empty(),
        "agent_loop_parallel requires at least one config"
    );

    // agent_loop_continue mode precondition guards.
    if prompts.is_empty() {
        assert!(
            !base_context.messages.is_empty(),
            "agent_loop_parallel with empty prompts requires non-empty base_context.messages \
             (agent_loop_continue mode)"
        );
        assert!(
            base_context.messages.last().map(|m| m.role()) != Some("assistant"),
            "agent_loop_parallel with empty prompts requires context NOT ending on an \
             assistant message (agent_loop_continue mode)"
        );
    }

    // Ensure shared session / agent identity.
    if base_context.agent_id.is_none() {
        base_context.agent_id = Some(uuid::Uuid::new_v4().to_string());
    }
    if base_context.session_id.is_none() {
        base_context.session_id = Some(uuid::Uuid::new_v4().to_string());
    }
    let session_id = base_context.session_id.clone().unwrap();

    // Assign deterministic loop_ids: {session_id}.{config_segment}.{N}
    let loop_ids: Vec<String> = configs
        .iter()
        .enumerate()
        .map(|(i, cfg)| format!("{}.{}.{}", session_id, derive_config_segment(cfg), i + 1))
        .collect();

    tx.send(AgentEvent::ParallelLoopStart {
        session_id: session_id.clone(),
        loop_ids: loop_ids.clone(),
        timestamp: Utc::now(),
    })
    .ok();

    // Clone base context per branch; set individual loop_ids.
    let branch_contexts: Vec<AgentContext> = loop_ids
        .iter()
        .map(|lid| {
            let mut ctx = base_context.clone();
            ctx.loop_id = Some(lid.clone());
            ctx
        })
        .collect();

    let outcomes =
        run_parallel_branches(prompts.clone(), branch_contexts, configs, &tx, &cancel).await;

    let (decision, eval_usage) = strategy.evaluate(&prompts, &outcomes, &tx, cancel).await;
    let selected_index = match decision {
        EvaluationDecision::Select(i) => i.min(outcomes.len() - 1),
    };

    tx.send(AgentEvent::ParallelLoopEnd {
        session_id,
        selected_loop_id: outcomes[selected_index].loop_id.clone(),
        selected_config_index: selected_index,
        evaluation_usage: eval_usage.clone(),
        timestamp: Utc::now(),
    })
    .ok();

    let total_usage = outcomes
        .iter()
        .fold(Usage::default(), |acc, o| acc.combine(&o.usage))
        .combine(&eval_usage);

    // Destructure outcomes: pull out the selected one, keep the rest.
    let mut all_outcomes = outcomes;
    let selected = all_outcomes.remove(selected_index);

    ParallelLoopResult {
        selected_context: selected.context,
        selected_messages: selected.new_messages,
        selected_index,
        all_outcomes,
        total_usage,
    }
}
