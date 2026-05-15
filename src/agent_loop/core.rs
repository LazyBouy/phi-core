//! Primary entry points for the agent loop: `agent_loop` and `agent_loop_continue`.

use super::config::*;
use super::helpers::apply_input_filters;
use super::run::run_loop;
use crate::types::*;
use tokio::sync::mpsc;

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
    // Populate identity IDs once at the top, generating UUIDs for any the caller didn't supply.
    // Writing them back to context lets a subsequent agent_loop_continue() call inherit them.
    // Hoisting here eliminates the prior `.clone().unwrap()` scatter — every AgentStart/AgentEnd
    // emission below reuses the owned local copies.
    let (agent_id, session_id, loop_id) = ensure_loop_ids(context);

    // before_loop hook — fires before AgentStart; false aborts the entire loop
    if let Some(ref before_loop) = config.before_loop {
        if !before_loop(&context.messages, 0) {
            tx.send(AgentEvent::AgentEnd {
                loop_id: loop_id.clone(),
                messages: vec![],
                usage: Usage::default(),
                timestamp: chrono::Utc::now(),
                rejection: None,
            })
            .ok();
            return vec![];
        }
    }

    tx.send(AgentEvent::AgentStart {
        agent_id: agent_id.clone(),
        session_id: session_id.clone(),
        loop_id: loop_id.clone(),
        parent_loop_id: context.parent_loop_id.clone(), // None for origin calls
        continuation_kind: context
            .continuation_kind
            .clone()
            .unwrap_or(ContinuationKind::Initial),
        timestamp: chrono::Utc::now(),
        metadata: None,
        config_snapshot: Some(build_config_snapshot(config, context)),
    })
    .ok();

    // !!!SECURITY!!!: Apply input filters before adding prompts to context.
    // Reject → emit InputRejected + AgentEnd and return immediately (no LLM call made).
    // Warn  → warning text appended to the last user message so the LLM sees it.
    // Pass  → prompts returned unchanged.
    let prompts = match apply_input_filters(prompts, &config.input_filters, &tx, &loop_id) {
        Ok(filtered) => filtered,
        Err(reason) => {
            // AgentEnd with rejection: pre-run rejection is the one case where
            // AgentEnd.rejection is Some — the agent never actually started.
            tx.send(AgentEvent::AgentEnd {
                loop_id: loop_id.clone(),
                messages: vec![],
                usage: Usage::default(),
                timestamp: chrono::Utc::now(),
                rejection: Some(reason),
            })
            .ok();
            return vec![];
        }
    };

    let mut new_messages: Vec<AgentMessage> = prompts.clone();

    // Add prompts to context
    for prompt in &prompts {
        context.messages.push(prompt.clone());
    }

    // Classify prompts into user_context (they're user messages)
    for prompt in &prompts {
        context.user_context.push(prompt.clone());
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
        loop_id,
        messages: new_messages.clone(),
        usage: loop_usage.clone(),
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
    if let Some(last) = context.messages.last() {
        assert!(
            last.role() != "assistant",
            "Cannot continue from assistant message"
        );
    }

    let mut new_messages: Vec<AgentMessage> = Vec::new();

    // Classify existing messages into streams (if not already populated)
    if context.user_context.is_empty() && context.inrun_context.is_empty() {
        for msg in &context.messages {
            match msg.as_llm() {
                Some(Message::User { .. }) => context.user_context.push(msg.clone()),
                Some(Message::Assistant { .. }) | Some(Message::ToolResult { .. }) => {
                    context
                        .inrun_context
                        .push(crate::types::InRunEntry::Live(msg.clone()));
                }
                _ => {} // Extension messages go to neither stream
            }
        }
    }

    // Hoist identity IDs into owned locals once. agent_id and session_id are asserted Some
    // above; loop_id is generated here if the Agent wrapper didn't supply one. Subsequent
    // event emissions reuse these clones rather than re-`.unwrap()`-ing the Option fields.
    let agent_id = context
        .agent_id
        .as_ref()
        .expect("asserted Some above")
        .clone();
    let session_id = context
        .session_id
        .as_ref()
        .expect("asserted Some above")
        .clone();
    let loop_id = context
        .loop_id
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone();

    // before_loop hook — fires before AgentStart; false aborts the entire loop
    if let Some(ref before_loop) = config.before_loop {
        if !before_loop(&context.messages, 0) {
            tx.send(AgentEvent::AgentEnd {
                loop_id: loop_id.clone(),
                messages: vec![],
                usage: Usage::default(),
                timestamp: chrono::Utc::now(),
                rejection: None,
            })
            .ok();
            return vec![];
        }
    }

    tx.send(AgentEvent::AgentStart {
        agent_id,
        session_id,
        loop_id: loop_id.clone(),
        parent_loop_id: context.parent_loop_id.clone(), // set by Agent wrapper
        continuation_kind: context
            .continuation_kind
            .clone()
            .unwrap_or(ContinuationKind::Initial),
        timestamp: chrono::Utc::now(),
        metadata: None,
        config_snapshot: Some(build_config_snapshot(config, context)),
    })
    .ok();

    let loop_usage = run_loop(context, &mut new_messages, config, &tx, &cancel, None).await;

    tx.send(AgentEvent::AgentEnd {
        loop_id,
        messages: new_messages.clone(),
        usage: loop_usage.clone(),
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

/// Ensure `context.agent_id`, `context.session_id`, and `context.loop_id` are populated.
///
/// Generates a fresh UUID for any that are `None`, writes back into `context`, and returns
/// owned clones of all three. Used by `agent_loop` (origin call) to hoist identity into
/// locals once so subsequent event emissions don't repeat `.clone().unwrap()` patterns
/// that would panic if the invariant ever drifted.
fn ensure_loop_ids(ctx: &mut AgentContext) -> (String, String, String) {
    let agent_id = ctx
        .agent_id
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone();
    let session_id = ctx
        .session_id
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone();
    let loop_id = ctx
        .loop_id
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone();
    (agent_id, session_id, loop_id)
}

/// Build a `LoopConfigSnapshot` from the current `AgentLoopConfig` and `AgentContext`.
fn build_config_snapshot(
    config: &AgentLoopConfig,
    context: &AgentContext,
) -> crate::session::LoopConfigSnapshot {
    // Extract config_id from the loop_id's config_segment if available.
    // loop_id format: "{session_id}.{config_segment}.{N}"
    let config_id = context
        .loop_id
        .as_deref()
        .and_then(|lid| {
            let session_id = context.session_id.as_deref().unwrap_or("");
            lid.strip_prefix(session_id)
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.rsplit_once('.'))
                .map(|(seg, _n)| seg.to_string())
        })
        .or_else(|| config.config_id.clone());

    crate::session::LoopConfigSnapshot {
        model: config.model_config.id.clone(),
        provider: config.model_config.provider.clone(),
        config_id,
        name: Some(config.model_config.name.clone()),
        api: Some(config.model_config.api),
        base_url: Some(config.model_config.base_url.clone()),
        reasoning: Some(config.model_config.reasoning),
        context_window: Some(config.model_config.context_window),
        max_tokens: Some(config.model_config.max_tokens),
        thinking_level: Some(config.thinking_level),
        temperature: config.temperature,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_loop_ids_populates_missing_fields() {
        let mut ctx = AgentContext::default();
        assert!(ctx.agent_id.is_none());
        assert!(ctx.session_id.is_none());
        assert!(ctx.loop_id.is_none());

        let (a, s, l) = ensure_loop_ids(&mut ctx);
        assert!(!a.is_empty() && !s.is_empty() && !l.is_empty());
        assert_eq!(ctx.agent_id.as_deref(), Some(a.as_str()));
        assert_eq!(ctx.session_id.as_deref(), Some(s.as_str()));
        assert_eq!(ctx.loop_id.as_deref(), Some(l.as_str()));
    }

    #[test]
    fn ensure_loop_ids_idempotent() {
        let mut ctx = AgentContext::default();
        let (a1, s1, l1) = ensure_loop_ids(&mut ctx);
        let (a2, s2, l2) = ensure_loop_ids(&mut ctx);
        assert_eq!(a1, a2);
        assert_eq!(s1, s2);
        assert_eq!(l1, l2);
    }

    #[test]
    fn ensure_loop_ids_preserves_existing() {
        let mut ctx = AgentContext {
            agent_id: Some("agent-x".into()),
            session_id: Some("session-y".into()),
            // loop_id intentionally None — should be generated.
            ..AgentContext::default()
        };

        let (a, s, l) = ensure_loop_ids(&mut ctx);
        assert_eq!(a, "agent-x");
        assert_eq!(s, "session-y");
        assert!(!l.is_empty());
        assert_eq!(ctx.loop_id.as_deref(), Some(l.as_str()));
    }
}
