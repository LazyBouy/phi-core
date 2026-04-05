<!-- Last verified: 2026-04-05 by Claude Code -->
# Configuration

Configuration controls agent behavior at three levels: context management (`ContextConfig`), execution safety (`ExecutionLimits`), and the unified loop config (`AgentLoopConfig`) that bundles model, hooks, compaction, limits, caching, retry, and filters into a single borrowed struct for each `agent_loop` call.

## Concept Overview

```
Configuration [EXISTS]
├── ContextConfig [EXISTS] — max_context_tokens + compaction policy
├── CompactionConfig [EXISTS] — WHEN (thresholds, scope) + HOW (keep settings)
├── ExecutionLimits [EXISTS] — max_turns/tokens/duration/cost
├── CacheConfig [EXISTS] — Auto/Disabled/Manual
├── AgentLoopConfig [EXISTS] — 20+ fields (model, hooks, limits, filters)
├── Callback hooks [EXISTS] — 12 hook types across turn/loop/tool/error
├── ThinkingLevel [EXISTS] — Off/Minimal/Low/Medium/High
└── InputFilter [EXISTS] — Pass/Warn/Reject
```

---

## ContextConfig [EXISTS]

Model constraints plus compaction policy. When set on `AgentLoopConfig`, enables automatic context management.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `max_context_tokens` | `usize` | `100_000` | [EXISTS] | Maximum context tokens (the model's context window) |
| `system_prompt_tokens` | `usize` | `4_000` | [EXISTS] | Tokens reserved for the system prompt |
| `compaction` | `CompactionConfig` | (see below) | [EXISTS] | Compaction policy -- always present when context limits are set |
| `keep_recent` | `usize` | `10` | [EXISTS] | Legacy field (use `compaction.keep_recent_turns` instead) |
| `keep_first` | `usize` | `2` | [EXISTS] | Legacy field (use `compaction.keep_first_turns` instead) |
| `tool_output_max_lines` | `usize` | `50` | [EXISTS] | Legacy field (use `compaction.tool_output_max_lines` instead) |

---

## CompactionConfig [EXISTS]

Full compaction policy -- controls both WHEN to compact and HOW to compact. Embedded in `ContextConfig.compaction`.

### WHEN: Trigger Thresholds

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `compact_at_pct` | `f64` | `0.90` | [EXISTS] | Fraction of `max_context_tokens` below which headroom is measured |
| `compact_budget_threshold_pct` | `f64` | `0.05` | [EXISTS] | Minimum remaining headroom before compaction fires. With defaults (100k/4k): fires at ~81k tokens |
| `compaction_scope` | `CompactionScope` | `FixedCount(3)` | [EXISTS] | How many earlier loops to include: `FixedCount(n)` or `TokenBudget` |

### HOW: Compaction Parameters

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `keep_first_turns` | `usize` | `2` | [EXISTS] | Turns kept verbatim from start (most recent loop only) |
| `keep_recent_turns` | `usize` | `10` | [EXISTS] | Turns kept from end; extended to turn boundary so ToolCall/ToolResult pairs are never split |
| `max_summary_tokens` | `usize` | `2_000` | [EXISTS] | Token budget for the summarised middle section (total, not per-turn) |
| `tool_output_max_lines` | `usize` | `50` | [EXISTS] | Max lines per tool output in keep_recent section |

---

## ExecutionLimits [EXISTS]

Safety net against runaway agent loops. Checked before each turn by `ExecutionTracker`.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `max_turns` | `usize` | `50` | [EXISTS] | Maximum LLM turns (catches infinite tool-call loops) |
| `max_total_tokens` | `usize` | `1_000_000` | [EXISTS] | Maximum total tokens consumed across all turns |
| `max_duration` | `Duration` | `600s` | [EXISTS] | Maximum wall-clock duration |
| `max_cost` | `Option<f64>` | `None` | [EXISTS] | Maximum cumulative dollar cost; requires `model_config.cost` rates to be set |

### ExecutionTracker [EXISTS]

Runtime state tracker that checks limits before each turn.

| Field | Status | Description |
|-------|--------|-------------|
| `limits` | [EXISTS] | The `ExecutionLimits` being enforced |
| `turns` | [EXISTS] | Turn counter |
| `tokens_used` | [EXISTS] | Accumulated token count |
| `cost_accumulated` | [EXISTS] | Accumulated dollar cost |
| `started_at` | [EXISTS] | `Instant` when tracking began |

When a limit is hit, `check_limits()` returns a reason string. The agent loop injects a `"[Agent stopped: ...]"` user message so the LLM (and user) can see what happened.

---

## CacheConfig [EXISTS]

Controls prompt caching behavior for providers that support it.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `enabled` | `bool` | `true` | [EXISTS] | Master switch for caching hints |
| `strategy` | `CacheStrategy` | `Auto` | [EXISTS] | How cache breakpoints are placed |

### CacheStrategy [EXISTS]

| Variant | Status | Description |
|---------|--------|-------------|
| `Auto` | [EXISTS] | Automatic breakpoint placement (system prompt + tool defs + recent history) |
| `Disabled` | [EXISTS] | No caching |
| `Manual { cache_system, cache_tools, cache_messages }` | [EXISTS] | Fine-grained control over what gets cached |

---

## AgentLoopConfig [EXISTS]

All static settings for a single `agent_loop` / `agent_loop_continue` call. Borrowed (`&AgentLoopConfig`) throughout the loop -- never mutated. 20+ fields organized by concern.

### Model & Provider

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `model_config` | `ModelConfig` | [EXISTS] | Complete provider identity (model id, api_key, base_url, protocol, compat, cost) |
| `provider_override` | `Option<Arc<dyn StreamProvider>>` | [EXISTS] | Bypasses `ProviderRegistry` dispatch; for testing or custom providers |
| `thinking_level` | `ThinkingLevel` | [EXISTS] | Depth of model reasoning: Off, Minimal, Low, Medium, High |
| `max_tokens` | `Option<u32>` | [EXISTS] | Override `model_config.max_tokens` for this call |
| `temperature` | `Option<f32>` | [EXISTS] | Temperature override |

### Context Transformation

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `convert_to_llm` | `Option<ConvertToLlmFn>` | [EXISTS] | Converts `AgentMessage[]` to `Message[]` before each LLM call |
| `transform_context` | `Option<TransformContextFn>` | [EXISTS] | Transforms full context before `convert_to_llm` (pruning, reordering, injection) |

### Steering & Follow-up

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `get_steering_messages` | `Option<GetMessagesFn>` | [EXISTS] | Polled between tools for user interruptions |
| `get_follow_up_messages` | `Option<GetMessagesFn>` | [EXISTS] | Polled after agent finishes for queued work |

### Compaction

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `context_config` | `Option<ContextConfig>` | [EXISTS] | Context window configuration; `None` disables compaction |

> **Note:** Compaction strategies have been consolidated into `CompactionConfig` (G5). See `in_memory_strategy` and `block_strategy` fields on `CompactionConfig`. The former `compaction_strategy` and `block_compaction_strategy` fields no longer exist on `AgentLoopConfig`.

### Limits & Safety

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `execution_limits` | `Option<ExecutionLimits>` | [EXISTS] | Max turns, tokens, duration, cost |
| `cache_config` | `CacheConfig` | [EXISTS] | Prompt caching configuration |
| `tool_execution` | `ToolExecutionStrategy` | [EXISTS] | Sequential, Parallel, or Batched |
| `retry_config` | `RetryConfig` | [EXISTS] | Exponential backoff with jitter for transient errors |

### Callback Hooks -- Turn Level

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `before_turn` | `Option<BeforeTurnFn>` | [EXISTS] | `(messages, turn_index) -> bool`; return `false` to abort the turn |
| `after_turn` | `Option<AfterTurnFn>` | [EXISTS] | `(messages, turn_usage)` |

### Callback Hooks -- Loop Level

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `before_loop` | `Option<BeforeLoopFn>` | [EXISTS] | `(messages, loop_index) -> bool`; return `false` to abort |
| `after_loop` | `Option<AfterLoopFn>` | [EXISTS] | `(new_messages, accumulated_usage)` |
| `on_error` | `Option<OnErrorFn>` | [EXISTS] | Called when LLM returns `StopReason::Error` |

### Callback Hooks -- Tool Level

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `before_tool_execution` | `Option<BeforeToolExecutionFn>` | [EXISTS] | `(tool_name, tool_call_id, args) -> bool`; return `false` to skip |
| `after_tool_execution` | `Option<AfterToolExecutionFn>` | [EXISTS] | `(tool_name, tool_call_id, is_error)` |
| `before_tool_execution_update` | `Option<BeforeToolExecutionUpdateFn>` | [EXISTS] | `(tool_name, tool_call_id, text) -> bool`; return `false` to suppress |
| `after_tool_execution_update` | `Option<AfterToolExecutionUpdateFn>` | [EXISTS] | `(tool_name, tool_call_id, text)` |

### Input Filtering & Identity

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `input_filters` | `Vec<Arc<dyn InputFilter>>` | [EXISTS] | Filters run in order; first Reject wins |
| `first_turn_trigger` | `TurnTrigger` | [EXISTS] | Trigger type for first TurnStart; default `User`, set to `SubAgent` by sub-agent callers |
| `config_id` | `Option<String>` | [EXISTS] | Stable identity for loop_id construction: `"{session_id}.{config_id}.{N}"` |

---

## Callback Hook Type Aliases [EXISTS]

All hooks are `Option<Arc<dyn Fn(...)>>`. `None` means no hook (zero overhead).

| Type Alias | Signature | Status |
|------------|-----------|--------|
| `ConvertToLlmFn` | `Box<dyn Fn(&[AgentMessage]) -> Vec<Message>>` | [EXISTS] |
| `TransformContextFn` | `Box<dyn Fn(Vec<AgentMessage>) -> Vec<AgentMessage>>` | [EXISTS] |
| `GetMessagesFn` | `Box<dyn Fn() -> Vec<AgentMessage>>` | [EXISTS] |
| `BeforeLoopFn` | `Arc<dyn Fn(&[AgentMessage], usize) -> bool>` | [EXISTS] |
| `AfterLoopFn` | `Arc<dyn Fn(&[AgentMessage], &Usage)>` | [EXISTS] |
| `BeforeTurnFn` | `Arc<dyn Fn(&[AgentMessage], usize) -> bool>` | [EXISTS] |
| `AfterTurnFn` | `Arc<dyn Fn(&[AgentMessage], &Usage)>` | [EXISTS] |
| `BeforeToolExecutionFn` | `Arc<dyn Fn(&str, &str, &serde_json::Value) -> bool>` | [EXISTS] |
| `AfterToolExecutionFn` | `Arc<dyn Fn(&str, &str, bool)>` | [EXISTS] |
| `BeforeToolExecutionUpdateFn` | `Arc<dyn Fn(&str, &str, &str) -> bool>` | [EXISTS] |
| `AfterToolExecutionUpdateFn` | `Arc<dyn Fn(&str, &str, &str)>` | [EXISTS] |
| `OnErrorFn` | `Arc<dyn Fn(&str)>` | [EXISTS] |

---

## InputFilter Trait [EXISTS]

Synchronous filter applied to user input before the LLM call. Intentionally synchronous for hot-path performance; use `before_turn` for async moderation.

| Method | Status | Description |
|--------|--------|-------------|
| `filter(text) -> FilterResult` | [EXISTS] | Returns Pass, Warn(String), or Reject(String) |

### FilterResult [EXISTS]

| Variant | Status | Description |
|---------|--------|-------------|
| `Pass` | [EXISTS] | Message passes unchanged |
| `Warn(String)` | [EXISTS] | Message passes; warning appended to context for LLM to see |
| `Reject(String)` | [EXISTS] | Message rejected; agent loop returns immediately with `InputRejected` event |

Filters run in order. First `Reject` wins and discards accumulated warnings. `Warn` messages accumulate and are appended to the user message.

---

## ThinkingLevel [EXISTS]

Controls the depth of model reasoning before responding.

| Variant | Status | Description |
|---------|--------|-------------|
| `Off` (default) | [EXISTS] | No thinking tokens; fastest and cheapest |
| `Minimal` | [EXISTS] | Lightest reasoning pass |
| `Low` | [EXISTS] | Shallow chain-of-thought |
| `Medium` | [EXISTS] | Balanced reasoning; default for most agentic workflows |
| `High` | [EXISTS] | Maximum reasoning budget; most expensive |

---

## Usage [EXISTS]

Token metrics per turn or accumulated.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `input` | `u64` | [EXISTS] | Input tokens |
| `output` | `u64` | [EXISTS] | Output tokens |
| `reasoning` | `u64` | [EXISTS] | Reasoning tokens (subset of output; non-zero for OpenAI o-series) |
| `cache_read` | `u64` | [EXISTS] | Tokens served from cache |
| `cache_write` | `u64` | [EXISTS] | Tokens written to cache |
| `total_tokens` | `u64` | [EXISTS] | Total tokens |

| Method | Status | Description |
|--------|--------|-------------|
| `estimated_cost(cost_config)` | [EXISTS] | Dollar cost from per-million-token rates |
| `combine(other)` | [EXISTS] | Sum two Usage values |
| `cache_hit_rate()` | [EXISTS] | Fraction of input tokens from cache (0.0-1.0) |

---

## Code Reference

| Concept | File |
|---------|------|
| `ContextConfig`, `CompactionConfig`, `CompactionScope` | `src/context/config.rs` |
| `ExecutionLimits`, `ExecutionTracker` | `src/context/execution.rs` |
| `AgentLoopConfig` and all callback type aliases | `src/agent_loop/config.rs` |
| `Usage`, `CacheConfig`, `CacheStrategy`, `ThinkingLevel` | `src/types/usage.rs` |
| `InputFilter`, `FilterResult`, `EvaluationStrategy` | `src/types/parallel.rs` |
| `ToolExecutionStrategy` | `src/types/tool.rs` |
| `RetryConfig` | `src/provider/retry.rs` |

---

## Conceptual Notes

- **before_task / after_task** [EXISTS] -- Session-level callbacks on `SessionRecorderConfig`. `BeforeTaskFn: Arc<dyn Fn(&Session) -> bool>` fires on first `AgentStart` with a new session_id. `AfterTaskFn: Arc<dyn Fn(&Session)>` fires on `flush()`.
- **before_compaction_start / after_compaction_end** [EXISTS] -- Compaction lifecycle callbacks (G1) on `AgentLoopConfig`. `before_compaction_start(estimated_tokens, message_count) -> bool` fires before `CompactionStarted`. `after_compaction_end(msgs_before, msgs_after, tokens_before, tokens_after)` fires after `CompactionEnded`.
- **Per-loop config tracking** [EXISTS] -- Model, thinking_level, temperature, and other config values are captured per-loop in `LoopConfigSnapshot` on each `LoopRecord` (and in `AgentStart.config_snapshot`). Session no longer carries model_config, thinking_level, or temperature fields. Fallback hierarchy: Loop -> Agent default.
- **Config streamlining** [DONE] -- Compaction strategies (`in_memory_strategy`, `block_strategy`) have been consolidated into `CompactionConfig`, completing G5. The dispatch logic in `run.rs` reads them from `ctx_config.compaction`. `AgentLoopConfig` no longer carries strategy fields.
- **ParallelLoopOutcome / ParallelLoopResult** -- Defined in `src/types/parallel.rs`, these types support evaluational parallelism where multiple branches run concurrently and an `EvaluationStrategy` selects the winner. Related to config because parallel configs produce multiple `AgentLoopConfig` instances.
