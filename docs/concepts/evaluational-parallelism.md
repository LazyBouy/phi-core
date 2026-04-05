<!-- Last verified: 2026-04-05 by Claude Code -->

# Evaluational Parallelism

Evaluational parallelism runs the **same prompt** through multiple `AgentLoopConfig`s
concurrently, evaluates the results with a pluggable strategy, and delivers the single
best outcome. This lets you compare models, prompt variants, or reasoning settings in
one call — then continue the session normally with the winner.

## Overview

```
             ┌─ Config A ─► Branch A ─► response A ─┐
prompt ──────┤                                        ├─► Evaluate ─► selected response
             └─ Config B ─► Branch B ─► response B ─┘
```

Every branch receives an identical copy of the base context (message history, tools) and
the same prompt. Branches run concurrently. After all branches finish, the
`EvaluationStrategy` picks the winner and returns its context and messages.

### When to use evaluational parallelism vs. parallel sub-agents

| | Evaluational parallelism | Parallel sub-agents |
|---|---|---|
| Task structure | Same task, different configs | Different subtasks |
| Context shared | Yes (cloned base context) | No (isolated child contexts) |
| Result | One selected outcome | All results merged |
| Typical use | Multi-model comparison, A/B prompts | Divide-and-conquer work |

## Entry point

```rust
pub async fn agent_loop_parallel(
    prompts: Vec<AgentMessage>,
    base_context: AgentContext,           // cloned once per config
    configs: Vec<AgentLoopConfig>,        // one per branch
    strategy: Arc<dyn EvaluationStrategy>,
    tx: mpsc::UnboundedSender<AgentEvent>,
    cancel: CancellationToken,
) -> ParallelLoopResult
```

`base_context` is cloned once per config entry — tools are `Arc`-shared (zero copy);
the message history is deep-cloned so branches start from identical state but diverge
independently.

### Minimal example

```rust
use phi_core::{agent_loop_parallel, PickFirstEvaluation, AgentContext, AgentLoopConfig};
use phi_core::provider::ModelConfig;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

let config_a = AgentLoopConfig {
    model_config: ModelConfig::anthropic("claude-opus-4-6", "my-key", "claude-opus-4-6"),
    ..AgentLoopConfig::default()
};
let config_b = AgentLoopConfig {
    model_config: ModelConfig::anthropic("claude-haiku-4-5", "my-key", "claude-haiku-4-5"),
    ..AgentLoopConfig::default()
};

let (tx, mut rx) = mpsc::unbounded_channel();
let result = agent_loop_parallel(
    vec![AgentMessage::Llm(Message::user("Explain quantum entanglement."))],
    AgentContext { system_prompt: "Be concise.".into(), ..Default::default() },
    vec![config_a, config_b],
    Arc::new(PickFirstEvaluation),  // or any EvaluationStrategy
    tx,
    CancellationToken::new(),
)
.await;

println!("Selected branch: {}", result.selected_index);
// Continue the session with the winning context
// agent_loop_continue(&mut result.selected_context, &next_config, tx, cancel).await;
```

## ParallelLoopResult

```rust
pub struct ParallelLoopResult {
    pub selected_context: AgentContext,        // winning branch's full context
    pub selected_messages: Vec<AgentMessage>,  // messages produced by the winner
    pub selected_index: usize,                 // 0-based index into original configs
    pub all_outcomes: Vec<ParallelLoopOutcome>,// remaining (non-selected) outcomes
    pub total_usage: Usage,                    // all branch usages + evaluation usage
}
```

Feed `selected_context` directly into `agent_loop_continue()` to resume the session
normally — parallel execution is a single-loop operation, not a special session mode.

## Built-in strategies

### TransparentEvaluation

Single-branch pass-through. Panics if more than one config is provided.

Use this when you want the parallel plumbing (events, `ParallelLoopResult`) for a
single config — zero evaluation overhead.

```rust
Arc::new(TransparentEvaluation)
```

### PickFirstEvaluation

Always selects index 0 regardless of content.

Deterministic, zero-cost. Useful for testing and debugging multi-branch setups where
you only care about the first config's output.

```rust
Arc::new(PickFirstEvaluation)
```

### TokenEfficientEvaluation

Selects the branch with the **lowest total token usage**.

Prefer when cost or latency matters more than response depth. The model that solved
the task most concisely wins.

```rust
Arc::new(TokenEfficientEvaluation)
```

### ElaborateEvaluation

Selects the branch with the **highest total token usage**.

Prefer when depth and thoroughness are the priority. The most verbose response wins —
useful when you want the most comprehensive analysis.

```rust
Arc::new(ElaborateEvaluation)
```

### LlmJudgeEvaluation

Uses a separate LLM call to evaluate which branch produced the best response.

```rust
use phi_core::LlmJudgeEvaluation;

Arc::new(LlmJudgeEvaluation {
    judge_config: AgentLoopConfig {
        model_config: ModelConfig::anthropic("claude-opus-4-6", "my-key", "claude-opus-4-6"),
        context_config: Some(ContextConfig {
            max_context_tokens: 100_000,
            ..Default::default()
        }),
        ..AgentLoopConfig::default()
    },
    system_prompt: None, // use built-in judge prompt
})
```

## `agent_loop_continue` mode

When `prompts` is empty, `agent_loop_parallel` routes each branch to
`agent_loop_continue` instead of `agent_loop`. This lets you run parallel evaluation
from an existing conversation context — the user query is already the last message in
`base_context`.

```rust
// The user query is the last message in context (no new prompts to add).
let result = agent_loop_parallel(
    vec![],          // empty → agent_loop_continue mode
    base_context,    // must be non-empty and not end on an assistant message
    configs,
    strategy,
    tx,
    cancel,
)
.await;
```

Same preconditions as `agent_loop_continue` apply: `base_context.messages` must be
non-empty and must not end on an assistant message.

### `original_context_len` on `ParallelLoopOutcome`

Each outcome carries `original_context_len: usize` — the number of messages in the
cloned context at the moment the branch was dispatched:

```rust
pub struct ParallelLoopOutcome {
    // ...
    pub original_context_len: usize,
}
```

`context.messages[..original_context_len]` is the shared base context all branches
started from. Messages at `[original_context_len..]` are new messages produced by
that branch.

Evaluation strategies use this field to extract the original user query and prior
conversation history without separate bookkeeping, regardless of whether
`agent_loop` or `agent_loop_continue` mode was used.

## LLM Judge — prompt construction and comprehension criteria

### What the judge sees

The judge receives only clean, relevant content:

- **Prior conversation context** *(new)*: the conversation history before the user
  query, formatted as a human-readable transcript. Tool call arguments and images
  are stripped — only `Content::Text` survives. Omitted from the prompt when empty.
- **Original query**: text extracted from user messages in `prompts` (agent_loop mode),
  or from the last `Message::User` in `context.messages[..original_context_len]`
  (agent_loop_continue mode). Tool calls, images, and thinking are stripped.
- **Per-branch response**: the text of the **last `Message::Assistant`** in each
  branch's `new_messages`. Tool calls, tool results, and intermediate multi-turn
  exchanges are stripped entirely — the judge evaluates outcomes, not reasoning traces.

Example judge prompt (with prior context):
```
Prior conversation context:
User: What is quantum mechanics?
Assistant: Quantum mechanics is the branch of physics that...

Original query:
Can you explain quantum entanglement in simple terms?

Response 1:
Quantum entanglement is when two particles share a quantum state...

Response 2:
Think of two magic dice...

Which response is best? Reply with ONLY the response number (e.g., "1" or "2").
```

### Query extraction in `agent_loop_continue` mode

When `prompts` is empty, the judge cannot read the query directly from the `prompts`
slice. It instead locates the **last `Message::User`** in
`outcome.context.messages[..original_context_len]` and extracts its text content.
Everything before that message becomes the prior conversation context.

### Judge's comprehension criteria

The judge can only make a fair comparison when it sees **all N branch final responses
simultaneously** alongside the prior context and query. For this to work, the combined
content must fit within the judge model's context window.

This condition — all content fitting in the judge's context at once — is called the
**judge's comprehension criteria**.

The budget is derived automatically from `judge_config.context_config.max_context_tokens`
(if set). About 20% of the budget is reserved for the system prompt, query framing, and
overhead; the remaining 80% is allocated for prior context + branch responses combined.

When no `context_config` is set on `judge_config`, no compaction is applied (all content
is passed through as-is).

### 2-iteration compaction strategy

When the combined content exceeds the budget, compaction is applied in two iterations:

**Iteration 1 — compact prior context only, outputs intact**

The prior conversation context is compacted through 3 progressive tiers while branch
outputs are preserved verbatim:

1. **Tier 1 — tail truncation**: keep only the last 80 lines of the context transcript.
2. **Tier 2 — paragraph summary**: keep only the first paragraph and last paragraph
   (separated by `...`).
3. **Tier 3 — hard char limit**: truncate to a per-response char limit derived from
   the remaining budget, minimum 200 chars. The formula is
   `max(200, (token_budget * 4) / n)` where `n` is the number of texts being compacted
   and the `* 4` factor converts from tokens to chars (1 token ~ 4 chars estimate).

After each tier, the combined token estimate is re-checked. If the budget is satisfied,
the judge proceeds with the compacted context and intact outputs.

**Iteration 2 — compact both context and outputs independently**

If iteration 1 cannot satisfy the budget even at tier 3, the context stays at its most-
compacted (tier-3) form and branch outputs are now compacted independently through the
same tiered compaction pipeline (legacy `compact_messages()`; see [compaction](compaction.md) for the modern CompactionBlock system).

```
prior context (tier-3)  +  outputs (tier-1 → 2 → 3)  →  check budget after each tier
```

If the criteria still cannot be satisfied after iteration 2, a `ProgressMessage` warning
is emitted to `tx` and the judge proceeds best-effort.

### Why context is compacted first

Iteration 1 biases the judge towards seeing the **complete, uncompacted branch outputs**
— the actual decision material. Prior conversation history is ancillary; trimming it first
preserves the most important information for fair comparison.

### Original responses are always preserved

Compaction only affects what the **judge reads**. The `selected_messages` field in
`ParallelLoopResult` always contains the original, uncompacted winning branch response.

### Setting the judge's context limit

Set `judge_config.context_config.max_context_tokens` to the judge model's context window
size (in tokens). This enables the comprehension-criteria check:

```rust
context_config: Some(ContextConfig {
    max_context_tokens: 200_000, // Claude Opus 4.6 context window
    ..Default::default()
}),
```

Different judge models have different context windows — the limit is co-located with the
model config that actually has the constraint.

### Design decisions

**`original_context_len` on outcome (not a separate parameter)**
The `EvaluationStrategy` trait receives only `outcomes` and `prompts`. Embedding
`original_context_len` in each outcome avoids changing the trait signature and keeps all
outcome data co-located. Since all branches share the same base context, the value is
identical across outcomes — using `outcomes[0]` is idiomatic.

**Same tier functions for context and output compaction**
`compact_tier1/2/3` were designed for document text but work equally well on a formatted
conversation transcript. Reusing the same primitives minimises code surface and keeps
compaction behaviour consistent.

**Budget allocation — context gets priority (iteration 1)**
Iteration 1 compacts only the prior context, keeping outputs intact. This preserves the
complete branch responses — the actual decision material — while trimming ancillary
history first. Outputs are only compacted in iteration 2 when the context alone cannot
satisfy the budget.

## Session identity and loop IDs

All branches share the same `session_id` for traceability. Each branch gets a distinct
`loop_id` following the format:

```
{session_id}.{config_segment}.{N}
```

where `config_segment` is derived from `config.config_id` (if set) or auto-derived as
`{provider}.{model-slug}[.thinking]`.

Example with two configs:
```
ses_abc123.anthropic.claude-opus-4-6.1
ses_abc123.anthropic.claude-haiku-4-5.2
```

The judge loop (if used) also runs in the same session:
```
ses_abc123.anthropic.claude-opus-4-6.3   ← judge's loop
```

## Observability

Two events bracket the entire parallel execution:

```rust
AgentEvent::ParallelLoopStart {
    session_id: String,
    loop_ids: Vec<String>,   // one per branch, in config order
    timestamp: DateTime<Utc>,
}

AgentEvent::ParallelLoopEnd {
    session_id: String,
    selected_loop_id: String,
    selected_config_index: usize,
    evaluation_usage: Usage,  // judge LLM usage (zero if no judge)
    timestamp: DateTime<Utc>,
}
```

Events from all branches are interleaved in `tx`. Demultiplex by `loop_id` from each
branch's `AgentStart` event.

## Session continuity

`agent_loop_parallel` is a single-loop operation. After it returns, call
`agent_loop_continue` on `result.selected_context` to continue the session:

```rust
let result = agent_loop_parallel(prompts, base_ctx, configs, strategy, tx, cancel).await;

// The session continues normally with the winning branch's context
let follow_up = agent_loop_continue(
    &mut result.selected_context,
    &next_config,
    tx2,
    cancel2,
)
.await;
```

## Complete example — multi-model comparison with LLM judge

```rust
use phi_core::{
    agent_loop_parallel, agent_loop_continue,
    AgentContext, AgentLoopConfig, AgentMessage, AgentEvent, Message,
};
use phi_core::context::ContextConfig;
use phi_core::LlmJudgeEvaluation;
use phi_core::provider::ModelConfig;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    // Branch A: fast, cost-efficient model
    let config_a = AgentLoopConfig {
        model_config: ModelConfig::anthropic("claude-haiku-4-5", API_KEY, "claude-haiku-4-5"),
        ..AgentLoopConfig::default()
    };

    // Branch B: powerful model
    let config_b = AgentLoopConfig {
        model_config: ModelConfig::anthropic("claude-opus-4-6", API_KEY, "claude-opus-4-6"),
        ..AgentLoopConfig::default()
    };

    // Judge: evaluates which response is better
    let judge_config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("claude-opus-4-6", API_KEY, "claude-opus-4-6"),
        context_config: Some(ContextConfig {
            max_context_tokens: 200_000,
            ..Default::default()
        }),
        ..AgentLoopConfig::default()
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let cancel = CancellationToken::new();

    let result = agent_loop_parallel(
        vec![AgentMessage::Llm(Message::user("What is the most important physics discovery of the 20th century?"))],
        AgentContext {
            system_prompt: "You are a knowledgeable assistant.".into(),
            ..Default::default()
        },
        vec![config_a, config_b],
        Arc::new(LlmJudgeEvaluation { judge_config, system_prompt: None }),
        tx,
        cancel,
    )
    .await;

    println!("Selected branch: {}", result.selected_index);
    println!("Total tokens used: {}", result.total_usage.total_tokens);

    // Collect and display the winning response
    for msg in &result.selected_messages {
        if let phi_core::AgentMessage::Llm(phi_core::Message::Assistant { content, .. }) = msg {
            for block in content {
                if let phi_core::Content::Text { text } = block {
                    println!("Response: {}", text);
                }
            }
        }
    }

    // Continue the session with the winner
    // let (tx2, _rx2) = mpsc::unbounded_channel();
    // agent_loop_continue(&mut result.selected_context, &next_config, tx2, cancel2).await;
}
```

## Custom evaluation strategies

Implement `EvaluationStrategy` for custom evaluation logic:

```rust
use phi_core::{AgentEvent, AgentMessage, ParallelLoopOutcome, Usage};
use phi_core::{EvaluationDecision, EvaluationStrategy};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct LongestResponseEvaluation;

#[async_trait::async_trait]
impl EvaluationStrategy for LongestResponseEvaluation {
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
            .max_by_key(|(_, o)| {
                // Sum all text content lengths across new messages
                o.new_messages.iter().filter_map(|m| m.as_llm()).flat_map(|msg| {
                    if let phi_core::Message::Assistant { content, .. } = msg {
                        content.iter().filter_map(|c| {
                            if let phi_core::Content::Text { text } = c { Some(text.len()) } else { None }
                        }).collect::<Vec<_>>()
                    } else {
                        vec![]
                    }
                }).sum::<usize>()
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        (EvaluationDecision::Select(idx), Usage::default())
    }
}
```
