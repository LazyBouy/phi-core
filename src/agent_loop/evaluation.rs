//! Pluggable evaluation strategies for [`agent_loop_parallel`].
//!
//! [`EvaluationStrategy`] selects the best result after parallel branches finish.
//! Five built-in implementations cover the most common use cases; implement the
//! trait for custom evaluation logic.
//!
//! # Built-in strategies
//!
//! | Strategy | Selection criterion | Use case |
//! |---|---|---|
//! | [`TransparentEvaluation`] | Pass-through (1 branch only) | Zero-overhead wrapper |
//! | [`PickFirstEvaluation`] | Always index 0 | Testing / deterministic default |
//! | [`TokenEfficientEvaluation`] | Lowest total token usage | Cost / latency priority |
//! | [`ElaborateEvaluation`] | Highest total token usage | Depth / thoroughness priority |
//! | [`LlmJudgeEvaluation`] | Separate LLM judge call | Best quality selection |
//!
//! [`agent_loop_parallel`]: crate::agent_loop::agent_loop_parallel

// `EvaluationDecision` and `EvaluationStrategy` are defined in `types.rs` so that
// `agent_loop_parallel` in `agent_loop.rs` can use them without a circular dependency.
// They are re-exported here for ergonomic imports from `crate::evaluation`.
pub use crate::types::{EvaluationDecision, EvaluationStrategy};

use super::config::AgentLoopConfig;
use super::core::agent_loop;
use crate::types::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ── TransparentEvaluation ─────────────────────────────────────────────────────

/// Single-branch pass-through — panics if more than one branch is present.
///
/// Use this when you want the evaluational-parallelism plumbing (tracing events,
/// `ParallelLoopResult`) without the overhead of running multiple branches.
/// The standard `agent_loop` case is just `agent_loop_parallel` with one config
/// and this strategy.
pub struct TransparentEvaluation;

#[async_trait::async_trait]
impl EvaluationStrategy for TransparentEvaluation {
    async fn evaluate(
        &self,
        _prompts: &[AgentMessage],
        outcomes: &[ParallelLoopOutcome],
        _tx: &mpsc::UnboundedSender<AgentEvent>,
        _cancel: CancellationToken,
    ) -> (EvaluationDecision, Usage) {
        assert_eq!(
            outcomes.len(),
            1,
            "TransparentEvaluation requires exactly one branch, got {}",
            outcomes.len()
        );
        (EvaluationDecision::Select(0), Usage::default())
    }
}

// ── PickFirstEvaluation ───────────────────────────────────────────────────────

/// Always selects the first branch (index 0), regardless of content.
///
/// Useful for testing, debugging, and as a safe deterministic default when
/// you only care about the output of the first config.
pub struct PickFirstEvaluation;

#[async_trait::async_trait]
impl EvaluationStrategy for PickFirstEvaluation {
    async fn evaluate(
        &self,
        _prompts: &[AgentMessage],
        _outcomes: &[ParallelLoopOutcome],
        _tx: &mpsc::UnboundedSender<AgentEvent>,
        _cancel: CancellationToken,
    ) -> (EvaluationDecision, Usage) {
        (EvaluationDecision::Select(0), Usage::default())
    }
}

// ── TokenEfficientEvaluation ─────────────────────────────────────────────────

/// Selects the branch with the **lowest total token usage**.
///
/// Prefer this strategy when cost or latency is the primary concern and you
/// want the most concise result that still answers the question.
pub struct TokenEfficientEvaluation;

#[async_trait::async_trait]
impl EvaluationStrategy for TokenEfficientEvaluation {
    async fn evaluate(
        &self,
        _prompts: &[AgentMessage],
        outcomes: &[ParallelLoopOutcome],
        _tx: &mpsc::UnboundedSender<AgentEvent>,
        _cancel: CancellationToken,
    ) -> (EvaluationDecision, Usage) {
        let idx = outcomes
            .iter()
            .enumerate()
            .min_by_key(|(_, o)| o.usage.total_tokens)
            .map(|(i, _)| i)
            .unwrap_or(0);
        (EvaluationDecision::Select(idx), Usage::default())
    }
}

// ── ElaborateEvaluation ───────────────────────────────────────────────────────

/// Selects the branch with the **highest total token usage**.
///
/// Prefer this strategy when depth and thoroughness are the priority and you
/// want the most detailed response among the branches.
pub struct ElaborateEvaluation;

#[async_trait::async_trait]
impl EvaluationStrategy for ElaborateEvaluation {
    async fn evaluate(
        &self,
        _prompts: &[AgentMessage],
        outcomes: &[ParallelLoopOutcome],
        _tx: &mpsc::UnboundedSender<AgentEvent>,
        _cancel: CancellationToken,
    ) -> (EvaluationDecision, Usage) {
        let idx = outcomes
            .iter()
            .enumerate()
            .max_by_key(|(_, o)| o.usage.total_tokens)
            .map(|(i, _)| i)
            .unwrap_or(0);
        (EvaluationDecision::Select(idx), Usage::default())
    }
}

// ── LlmJudgeEvaluation ────────────────────────────────────────────────────────

/// Uses a separate LLM call to judge which branch response is best.
///
/// # Judge prompt construction
///
/// The judge sees only clean, relevant content — never raw tool calls or intermediate
/// steps from inside a branch:
///
/// - **Prior conversation context** *(when present)*: the conversation history before
///   the user query, formatted as a human-readable transcript. Only `Content::Text`
///   survives — tool call arguments and images are stripped. Omitted when empty.
/// - **Original query**: text extracted from user messages in `prompts` (`agent_loop`
///   mode), or from the last `Message::User` in
///   `context.messages[..original_context_len]` (`agent_loop_continue` mode).
/// - **Per-branch response**: the final assistant text from the last
///   `Message::Assistant` in `outcome.new_messages`. Tool calls, tool results,
///   and all multi-turn exchanges within a branch are stripped. The judge evaluates
///   outcomes, not the reasoning trace.
///
/// # `agent_loop_continue` mode
///
/// When `prompts` is empty (continue mode), the judge locates the last
/// `Message::User` in `context.messages[..original_context_len]` as the query.
/// Everything before that message becomes the prior conversation context.
///
/// # Judge's comprehension criteria
///
/// All N branch final responses (plus prior context) must fit in the judge model's
/// context window *simultaneously* for a fair comparison. The token budget is
/// derived from `judge_config.context_config.max_context_tokens` (if set).
/// When no context limit is configured, all content is passed through as-is.
///
/// # 2-iteration compaction strategy
///
/// When combined content exceeds the budget, compaction is applied in two iterations:
///
/// **Iteration 1 — compact prior context only, outputs intact.**
/// The prior context is reduced through 3 progressive tiers while branch outputs
/// are preserved verbatim:
/// 1. **Tier 1**: keep only the last 80 lines.
/// 2. **Tier 2**: keep first paragraph + last paragraph only.
/// 3. **Tier 3**: hard char limit derived from remaining budget.
///
/// **Iteration 2 — compact both independently (if iteration 1 insufficient).**
/// Context stays at tier-3 form; branch outputs are now compacted independently
/// through the same 3-tier pipeline.
///
/// A [`AgentEvent::ProgressMessage`] warning is emitted to `tx` if the budget
/// cannot be satisfied after both iterations.
///
/// The judge's decision applies to the **original** (uncompacted) branch responses.
/// `ParallelLoopResult::selected_messages` always contains the uncompacted winner.
///
/// # Response parsing
///
/// The judge's reply is scanned for the first numeric token (e.g., "1", "2",
/// "Response 2"). Falls back to index 0 if no number is found or parsing fails.
///
/// # Session traceability
///
/// The judge loop inherits the `session_id` from the branches so all events
/// (including the judge's `AgentStart`) are visible in the same session trace.
pub struct LlmJudgeEvaluation {
    /// Config for the judge LLM call. Set `context_config.max_context_tokens`
    /// to enable the comprehension-criteria compaction check.
    pub judge_config: AgentLoopConfig,
    /// Optional system prompt override. When `None`, a built-in evaluation prompt is used.
    pub system_prompt: Option<String>,
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract only `Content::Text` items from a content slice (strips tool calls, images, thinking).
fn extract_text_only(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract plain text from user messages in `prompts`.
/// Strips tool calls, images, and thinking blocks — the judge only needs the question.
fn extract_query_text(prompts: &[AgentMessage]) -> String {
    prompts
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(LlmMessage {
                message: Message::User { content, .. },
                ..
            }) => Some(content),
            _ => None,
        })
        .flat_map(|content| {
            content.iter().filter_map(|c| match c {
                Content::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract the text of the **last** `Message::User` in `messages`.
/// Returns `None` if no user message with text content is found.
fn extract_last_user_text(messages: &[AgentMessage]) -> Option<String> {
    messages.iter().rev().find_map(|m| match m {
        AgentMessage::Llm(LlmMessage {
            message: Message::User { content, .. },
            ..
        }) => {
            let text = extract_text_only(content);
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    })
}

/// Format `messages` as a human-readable conversation transcript for the judge.
///
/// Only `Content::Text` is included — tool calls, images, and thinking blocks
/// are stripped. Returns an empty string if the slice is empty or has no text.
fn format_prior_context(messages: &[AgentMessage]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for m in messages {
        match m {
            AgentMessage::Llm(LlmMessage {
                message: Message::User { content, .. },
                ..
            }) => {
                let text = extract_text_only(content);
                if !text.is_empty() {
                    parts.push(format!("User: {}", text));
                }
            }
            AgentMessage::Llm(LlmMessage {
                message: Message::Assistant { content, .. },
                ..
            }) => {
                let text = extract_text_only(content);
                if !text.is_empty() {
                    parts.push(format!("Assistant: {}", text));
                }
            }
            AgentMessage::Llm(LlmMessage {
                message:
                    Message::ToolResult {
                        tool_name, content, ..
                    },
                ..
            }) => {
                let text = extract_text_only(content);
                if !text.is_empty() {
                    parts.push(format!("Tool [{}]: {}", tool_name, text));
                }
            }
            _ => {}
        }
    }
    parts.join("\n")
}

/// Extract the final assistant text from a branch's new messages.
/// Returns the text content of the last `Message::Assistant` found, or an empty
/// string when the branch produced no assistant text.
fn extract_final_assistant_text(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Llm(LlmMessage {
                message: Message::Assistant { content, .. },
                ..
            }) => {
                let text = extract_text_only(content);
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// Tier 1: keep only the last `max_lines` lines of `text`.
fn compact_tier1(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        text.to_string()
    } else {
        lines[lines.len() - max_lines..].join("\n")
    }
}

/// Tier 2: keep first paragraph + last paragraph (separated by `...`).
fn compact_tier2(text: &str) -> String {
    let paragraphs: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    match paragraphs.len() {
        0 => text.to_string(),
        1 => paragraphs[0].to_string(),
        _ => format!(
            "{}\n\n...\n\n{}",
            paragraphs[0],
            paragraphs[paragraphs.len() - 1]
        ),
    }
}

/// Tier 3: hard char-limit truncation with an ellipsis marker.
fn compact_tier3(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        // Truncate at a character boundary; ensure max_chars > 3 for the ellipsis.
        let cut = max_chars.saturating_sub(3);
        format!("{}...", &text[..cut])
    }
}

/// Rough chars-to-tokens estimate (1 token ≈ 4 chars for English text).
fn estimate_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

/// Apply iterative compaction until the combined response tokens fit within `token_budget`.
///
/// Returns the (possibly compacted) texts and a boolean indicating whether the
/// comprehension criteria was satisfied after all tiers.
fn compact_responses(responses: Vec<String>, token_budget: usize) -> (Vec<String>, bool) {
    // Fast path — already within budget.
    let mut current = responses;
    if current.iter().map(|r| estimate_tokens(r)).sum::<usize>() <= token_budget {
        return (current, true);
    }

    // Tier 1: tail truncation (keep last 80 lines per response).
    current = current.into_iter().map(|r| compact_tier1(&r, 80)).collect();
    if current.iter().map(|r| estimate_tokens(r)).sum::<usize>() <= token_budget {
        return (current, true);
    }

    // Tier 2: paragraph summary (first + last paragraph per response).
    current = current.into_iter().map(|r| compact_tier2(&r)).collect();
    if current.iter().map(|r| estimate_tokens(r)).sum::<usize>() <= token_budget {
        return (current, true);
    }

    // Tier 3: hard char limit derived from budget, minimum 200 chars per response.
    let n = current.len().max(1);
    let max_chars = std::cmp::max(200, (token_budget * 4) / n);
    current = current
        .into_iter()
        .map(|r| compact_tier3(&r, max_chars))
        .collect();

    let satisfied = current.iter().map(|r| estimate_tokens(r)).sum::<usize>() <= token_budget;
    (current, satisfied)
}

/// Two-iteration compaction for the combined judge input (prior context + branch outputs).
///
/// The `token_budget` covers `prior_context + outputs` only — the ~20% overhead for
/// system prompt, query, and framing has already been deducted by the caller.
///
/// **Iteration 1** — compact `prior_context` progressively (tier 1 → 2 → 3) while
/// keeping `outputs` intact. If the budget is satisfied after any tier, return early.
///
/// **Iteration 2** — if iteration 1 cannot satisfy the budget even at tier 3, the
/// context stays at its most-compacted form and `outputs` are now compacted
/// independently through the existing [`compact_responses`] pipeline.
///
/// Returns `(compacted_context, compacted_outputs, satisfied)`.
fn compact_for_judge(
    prior_context: String,
    outputs: Vec<String>,
    token_budget: usize,
) -> (String, Vec<String>, bool) {
    let out_tokens = || outputs.iter().map(|o| estimate_tokens(o)).sum::<usize>();

    // Fast path — already within budget.
    if estimate_tokens(&prior_context) + out_tokens() <= token_budget {
        return (prior_context, outputs, true);
    }

    // ── Iteration 1: compact context only, outputs intact ────────────────────
    let ctx1 = compact_tier1(&prior_context, 80);
    if estimate_tokens(&ctx1) + out_tokens() <= token_budget {
        return (ctx1, outputs, true);
    }

    let ctx2 = compact_tier2(&ctx1);
    if estimate_tokens(&ctx2) + out_tokens() <= token_budget {
        return (ctx2, outputs, true);
    }

    let n_out = outputs.len().max(1);
    let ctx_budget_chars = (token_budget.saturating_sub(out_tokens()) * 4).max(200);
    let ctx3 = compact_tier3(&ctx2, ctx_budget_chars);
    if estimate_tokens(&ctx3) + out_tokens() <= token_budget {
        return (ctx3, outputs, true);
    }

    // ── Iteration 2: compact outputs independently (context stays at tier-3) ─
    let out_budget = token_budget
        .saturating_sub(estimate_tokens(&ctx3))
        .max(200 * n_out);
    let (compacted_outputs, satisfied) = compact_responses(outputs, out_budget);
    (ctx3, compacted_outputs, satisfied)
}

/// Build the judge's user prompt listing the prior context, original query,
/// and all numbered branch responses.
fn build_judge_user_message(
    prior_context: Option<&str>,
    query: &str,
    responses: &[String],
) -> String {
    let mut msg = String::new();
    if let Some(ctx) = prior_context.filter(|s| !s.trim().is_empty()) {
        msg.push_str("Prior conversation context:\n");
        msg.push_str(ctx);
        msg.push_str("\n\n");
    }
    msg.push_str(&format!("Original query:\n{}\n\n", query));
    for (i, resp) in responses.iter().enumerate() {
        msg.push_str(&format!("Response {}:\n{}\n\n", i + 1, resp));
    }
    msg.push_str(
        "Which response is best? Reply with ONLY the response number (e.g., \"1\" or \"2\").",
    );
    msg
}

/// Scan the judge's reply for the first integer in range [1, max_index+1].
/// Returns a 0-based index. Falls back to 0 if no valid number is found.
fn parse_judge_selection(text: &str, max_index: usize) -> usize {
    for word in text.split_whitespace() {
        let digits: String = word.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<usize>() {
            if n >= 1 && n <= max_index + 1 {
                return n - 1;
            }
        }
    }
    0
}

// ── LlmJudgeEvaluation impl ───────────────────────────────────────────────────

#[async_trait::async_trait]
impl EvaluationStrategy for LlmJudgeEvaluation {
    async fn evaluate(
        &self,
        prompts: &[AgentMessage],
        outcomes: &[ParallelLoopOutcome],
        tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: CancellationToken,
    ) -> (EvaluationDecision, Usage) {
        // ── 1. Determine query and prior context ──────────────────────────────
        //
        // All branches share the same base context; outcomes[0] is representative.
        // `original_context_len` marks the boundary between the shared base context
        // and the new messages produced by each branch.
        let orig_len = outcomes
            .first()
            .map(|o| o.original_context_len)
            .unwrap_or(0);
        let orig_ctx_msgs: &[AgentMessage] = outcomes
            .first()
            .map(|o| &o.context.messages[..orig_len])
            .unwrap_or(&[]);

        let (query, prior_context_msgs): (String, &[AgentMessage]) = if !prompts.is_empty() {
            // agent_loop mode: query comes from prompts; the entire original context
            // (base_context.messages) is the prior conversation history.
            (extract_query_text(prompts), orig_ctx_msgs)
        } else {
            // agent_loop_continue mode: query is the last Message::User in the
            // original context. Prior context = everything BEFORE that user message.
            let last_user_pos = orig_ctx_msgs.iter().rposition(|m| {
                matches!(
                    m,
                    AgentMessage::Llm(LlmMessage {
                        message: Message::User { .. },
                        ..
                    })
                )
            });
            match last_user_pos {
                Some(pos) => (
                    extract_last_user_text(&orig_ctx_msgs[pos..pos + 1]).unwrap_or_default(),
                    &orig_ctx_msgs[..pos],
                ),
                None => (String::new(), orig_ctx_msgs),
            }
        };

        let prior_context_text = format_prior_context(prior_context_msgs);

        // ── 2. Extract per-branch final responses ─────────────────────────────
        let raw_responses: Vec<String> = outcomes
            .iter()
            .map(|o| extract_final_assistant_text(&o.new_messages))
            .collect();

        // ── 3. Apply 2-iteration compaction (prior context first, then outputs)
        //
        // The budget (in tokens) comes from judge_config.context_config if set.
        // Reserve ~20 % of the budget for system prompt, query, and framing overhead.
        let token_budget = self
            .judge_config
            .context_config
            .as_ref()
            .map(|c| c.max_context_tokens);

        let (prior_ctx_for_judge, responses) = if let Some(budget) = token_budget {
            // 80% of budget for prior context + outputs; 20% for system/query/framing.
            let content_budget = (budget * 4) / 5;
            let (pc, resp, satisfied) =
                compact_for_judge(prior_context_text, raw_responses, content_budget);
            if !satisfied {
                tx.send(AgentEvent::ProgressMessage {
                    loop_id: String::new(),
                    tool_call_id: "judge-compaction".into(),
                    tool_name: "LlmJudgeEvaluation".into(),
                    text: format!(
                        "LlmJudgeEvaluation: could not fit prior context + {} branch \
                         responses within the judge's context budget ({} tokens) after \
                         2-iteration compaction. Proceeding best-effort — judge comparison \
                         may be incomplete.",
                        outcomes.len(),
                        budget
                    ),
                })
                .ok();
            }
            (pc, resp)
        } else {
            (prior_context_text, raw_responses)
        };

        // ── 4. Build judge context ────────────────────────────────────────────
        let default_system = "You are an impartial judge evaluating AI assistant responses. \
            Select the response that best answers the user's query. \
            Reply with ONLY the response number (e.g., \"1\" or \"2\").";
        let system_prompt = self
            .system_prompt
            .as_deref()
            .unwrap_or(default_system)
            .to_string();

        let judge_user_text =
            build_judge_user_message(Some(&prior_ctx_for_judge), &query, &responses);

        // Inherit session_id from the branches for traceability.
        let session_id = outcomes.first().and_then(|o| o.context.session_id.clone());

        let mut judge_context = AgentContext {
            system_prompt,
            messages: vec![],
            tools: vec![],
            agent_id: None,
            session_id,
            loop_id: None,
            parent_loop_id: None,
            continuation_kind: None,
            session: None,
            user_context: Vec::new(),
            inrun_context: Vec::new(),
            active_node_id: None,
            next_node_id: 0,
        };

        let judge_prompts = vec![AgentMessage::Llm(LlmMessage::new(Message::user(
            judge_user_text,
        )))];

        // ── 5. Run judge loop — forward events and capture usage ──────────────
        //
        // The forwarder task is tokio::spawn-able because it only captures owned
        // Send + 'static values: cloned tx, judge_rx (Receiver), usage_tx (oneshot).
        let (judge_tx, judge_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (usage_tx, usage_rx) = tokio::sync::oneshot::channel::<Usage>();

        let main_tx = tx.clone();
        tokio::spawn(async move {
            let mut judge_rx = judge_rx;
            let mut last_usage = Usage::default();
            while let Some(event) = judge_rx.recv().await {
                if let AgentEvent::AgentEnd { ref usage, .. } = event {
                    last_usage = usage.clone();
                }
                main_tx.send(event).ok();
            }
            // judge_tx is dropped when agent_loop returns → recv() yields None →
            // usage is sent back, unblocking usage_rx.await below.
            usage_tx.send(last_usage).ok();
        });

        let judge_messages = agent_loop(
            judge_prompts,
            &mut judge_context,
            &self.judge_config,
            judge_tx,
            cancel,
        )
        .await;

        let judge_usage = usage_rx.await.unwrap_or_default();

        // ── 6. Parse judge selection ──────────────────────────────────────────
        let judge_text = extract_final_assistant_text(&judge_messages);
        let selected = parse_judge_selection(&judge_text, outcomes.len() - 1);

        (EvaluationDecision::Select(selected), judge_usage)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_outcome(loop_id: &str, total_tokens: u64, final_text: &str) -> ParallelLoopOutcome {
        let msg = AgentMessage::Llm(LlmMessage::new(Message::Assistant {
            content: vec![Content::Text {
                text: final_text.to_string(),
            }],
            stop_reason: StopReason::Stop,
            model: "test".into(),
            provider: "test".into(),
            usage: Usage {
                total_tokens,
                ..Default::default()
            },
            timestamp: 0,
            error_message: None,
        }));
        ParallelLoopOutcome {
            config_index: 0,
            loop_id: loop_id.to_string(),
            context: AgentContext {
                system_prompt: String::new(),
                messages: vec![],
                tools: vec![],
                agent_id: None,
                session_id: None,
                loop_id: None,
                parent_loop_id: None,
                continuation_kind: None,
                session: None,
                user_context: Vec::new(),
                inrun_context: Vec::new(),
                active_node_id: None,
                next_node_id: 0,
            },
            new_messages: vec![msg],
            usage: Usage {
                total_tokens,
                ..Default::default()
            },
            original_context_len: 0,
        }
    }

    fn dummy_tx() -> mpsc::UnboundedSender<AgentEvent> {
        let (tx, _rx) = mpsc::unbounded_channel();
        tx
    }

    #[tokio::test]
    async fn test_transparent_single_branch() {
        let outcomes = vec![make_outcome("loop1", 100, "hello")];
        let (decision, usage) = TransparentEvaluation
            .evaluate(&[], &outcomes, &dummy_tx(), CancellationToken::new())
            .await;
        assert!(matches!(decision, EvaluationDecision::Select(0)));
        assert_eq!(usage.total_tokens, 0);
    }

    #[tokio::test]
    #[should_panic(expected = "TransparentEvaluation requires exactly one branch")]
    async fn test_transparent_panics_on_multiple() {
        let outcomes = vec![
            make_outcome("loop1", 100, "a"),
            make_outcome("loop2", 200, "b"),
        ];
        TransparentEvaluation
            .evaluate(&[], &outcomes, &dummy_tx(), CancellationToken::new())
            .await;
    }

    #[tokio::test]
    async fn test_pick_first() {
        let outcomes = vec![
            make_outcome("loop1", 300, "verbose"),
            make_outcome("loop2", 50, "concise"),
        ];
        let (decision, _) = PickFirstEvaluation
            .evaluate(&[], &outcomes, &dummy_tx(), CancellationToken::new())
            .await;
        assert!(matches!(decision, EvaluationDecision::Select(0)));
    }

    #[tokio::test]
    async fn test_token_efficient() {
        let outcomes = vec![
            make_outcome("loop1", 500, "long verbose response"),
            make_outcome("loop2", 50, "short"),
            make_outcome("loop3", 200, "medium"),
        ];
        let (decision, _) = TokenEfficientEvaluation
            .evaluate(&[], &outcomes, &dummy_tx(), CancellationToken::new())
            .await;
        assert!(matches!(decision, EvaluationDecision::Select(1)));
    }

    #[tokio::test]
    async fn test_elaborate() {
        let outcomes = vec![
            make_outcome("loop1", 500, "long verbose response"),
            make_outcome("loop2", 50, "short"),
            make_outcome("loop3", 200, "medium"),
        ];
        let (decision, _) = ElaborateEvaluation
            .evaluate(&[], &outcomes, &dummy_tx(), CancellationToken::new())
            .await;
        assert!(matches!(decision, EvaluationDecision::Select(0)));
    }

    #[test]
    fn test_parse_judge_selection() {
        assert_eq!(parse_judge_selection("2", 2), 1);
        assert_eq!(parse_judge_selection("Response 1 is best.", 2), 0);
        assert_eq!(parse_judge_selection("I pick 3.", 3), 2);
        assert_eq!(parse_judge_selection("unclear", 2), 0); // fallback
        assert_eq!(parse_judge_selection("5", 2), 0); // out of range → fallback
    }

    #[test]
    fn test_compact_tier1() {
        let text = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let compacted = compact_tier1(&text, 80);
        assert_eq!(compacted.lines().count(), 80);
    }

    #[test]
    fn test_compact_tier2() {
        let text = "First paragraph.\n\nMiddle paragraph.\n\nLast paragraph.";
        let compacted = compact_tier2(text);
        assert!(compacted.contains("First paragraph."));
        assert!(compacted.contains("Last paragraph."));
        assert!(!compacted.contains("Middle paragraph."));
    }

    #[test]
    fn test_extract_query_text() {
        let prompts = vec![
            AgentMessage::Llm(LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: "Hello".into(),
                }],
                timestamp: 0,
            })),
            AgentMessage::Llm(LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: "World".into(),
                }],
                timestamp: 0,
            })),
        ];
        let query = extract_query_text(&prompts);
        assert_eq!(query, "Hello\nWorld");
    }

    #[test]
    fn test_extract_final_assistant_text() {
        let messages = vec![
            AgentMessage::Llm(LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: "first".into(),
                }],
                stop_reason: StopReason::Stop,
                model: "m".into(),
                provider: "p".into(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            })),
            AgentMessage::Llm(LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: "final".into(),
                }],
                stop_reason: StopReason::Stop,
                model: "m".into(),
                provider: "p".into(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            })),
        ];
        assert_eq!(extract_final_assistant_text(&messages), "final");
    }

    #[test]
    fn test_extract_last_user_text() {
        let messages = vec![
            AgentMessage::Llm(LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: "first query".into(),
                }],
                timestamp: 0,
            })),
            AgentMessage::Llm(LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: "answer".into(),
                }],
                stop_reason: StopReason::Stop,
                model: "m".into(),
                provider: "p".into(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            })),
            AgentMessage::Llm(LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: "follow-up".into(),
                }],
                timestamp: 0,
            })),
        ];
        // Should return the LAST user message text.
        assert_eq!(
            extract_last_user_text(&messages),
            Some("follow-up".to_string())
        );
    }

    #[test]
    fn test_extract_last_user_text_none() {
        let messages: Vec<AgentMessage> = vec![];
        assert_eq!(extract_last_user_text(&messages), None);
    }

    #[test]
    fn test_format_prior_context() {
        let messages = vec![
            AgentMessage::Llm(LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: "Hello".into(),
                }],
                timestamp: 0,
            })),
            AgentMessage::Llm(LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: "Hi there!".into(),
                }],
                stop_reason: StopReason::Stop,
                model: "m".into(),
                provider: "p".into(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            })),
        ];
        let transcript = format_prior_context(&messages);
        assert!(transcript.contains("User: Hello"));
        assert!(transcript.contains("Assistant: Hi there!"));
    }

    #[test]
    fn test_compact_for_judge_no_compaction_needed() {
        let ctx = "short context".to_string();
        let outputs = vec!["short response".to_string()];
        let (c, o, satisfied) = compact_for_judge(ctx.clone(), outputs.clone(), 10_000);
        assert!(satisfied);
        assert_eq!(c, ctx);
        assert_eq!(o, outputs);
    }

    #[test]
    fn test_compact_for_judge_iter1_compacts_context_only() {
        // Make a large context and tiny outputs; budget forces context compaction.
        let many_lines: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        let outputs = vec!["tiny".to_string()];
        // Budget that fits outputs but not full context (outputs ≈ 1 token, context ≈ 1000).
        let budget = 100;
        let (c, o, satisfied) = compact_for_judge(many_lines, outputs.clone(), budget);
        // Outputs should be unchanged (iteration 1 only compacts context).
        assert_eq!(o, outputs);
        // Context should be shorter.
        assert!(estimate_tokens(&c) < 1000);
        // satisfied depends on whether budget was met; either way outputs are intact.
        let _ = satisfied;
    }
}
