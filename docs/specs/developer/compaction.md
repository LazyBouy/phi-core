# Compaction System

The compaction system manages context window pressure by summarizing, truncating, or dropping older conversation turns when the token count approaches the model's limit. Two strategies coexist: a legacy in-memory approach that rewrites the message array, and a modern block-based approach that creates non-destructive overlays on `LoopRecord`s.

## Concept Overview

```
Compaction [EXISTS]
├── CompactionBlock [EXISTS] — non-destructive overlay
│   ├── keep_first, keep_compacted, keep_recent [EXISTS]
├── CompactionScope [EXISTS] — FixedCount(n) / TokenBudget
├── CompactionStrategy [EXISTS] — legacy in-memory
├── BlockCompactionStrategy [EXISTS] — modern overlay
├── TurnMap [EXISTS] — turn indices → message ranges
├── Callbacks: before/after compaction [CONCEPTUAL]
└── Config spread: ContextConfig + AgentLoopConfig [CONCEPTUAL: streamline]
```

---

## CompactionBlock [EXISTS]

Non-destructive compaction overlay stored on `LoopRecord` alongside the original messages. When present, the context loader uses this block instead of raw messages. Three sections control what gets loaded into context.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `keep_first` | `Option<TurnRange>` | [EXISTS] | Turns kept verbatim from the start; only populated for the MOST RECENT loop |
| `keep_compacted` | `Option<CompactedSection>` | [EXISTS] | Fully summarised section; populated for ALL loops |
| `keep_recent` | `Option<CompactedSection>` | [EXISTS] | Recent turns with truncated tool outputs; only populated for the MOST RECENT loop |
| `created_at` | `DateTime<Utc>` | [EXISTS] | When this block was created |

**Loading logic**:
- Most recent loop: loads `keep_first` (original messages) + `keep_compacted` (summaries) + `keep_recent` (truncated)
- Older loops: loads only `keep_compacted` (full-loop summary)
- No compaction block: loads raw messages

### Supporting Types

| Type | Status | Description |
|------|--------|-------------|
| `TurnRange { start_turn, end_turn }` | [EXISTS] | Inclusive range of turn indices within a loop |
| `CompactedSection { range, messages }` | [EXISTS] | A turn range plus the replacement messages for that range |

---

## CompactionScope [EXISTS]

Controls how many earlier loops are included in compaction and context loading.

| Variant | Status | Description |
|---------|--------|-------------|
| `FixedCount(usize)` | [EXISTS] | Compact a fixed number of earlier loops on the active chain (default: 3) |
| `TokenBudget` | [EXISTS] | Walk backward, accumulating per-loop token estimates, stop when `max_context_tokens` would be exceeded |

**TokenBudget note**: The scope can include loops whose raw messages EXCEED `max_context_tokens`. This is intentional -- the compacted summaries will fit even when originals don't, enabling richer context for LLM-based summarisation strategies.

---

## CompactionStrategy (Legacy) [EXISTS]

In-memory compaction that rewrites the message array. Used when `AgentContext.session` is `None`.

| Method | Status | Description |
|--------|--------|-------------|
| `compact(messages, config) -> Vec<AgentMessage>` | [EXISTS] | Takes ownership of messages and returns a compacted version |

### DefaultCompaction [EXISTS]

The built-in implementation. Delegates to `compact_messages()` which applies 3-level reduction:
1. Truncate tool outputs
2. Summarize turns
3. Drop middle

---

## BlockCompactionStrategy (Modern) [EXISTS]

Creates non-destructive `CompactionBlock` overlays. Used when `AgentContext.session` is `Some`.

| Method | Status | Description |
|--------|--------|-------------|
| `keep_first(record, turn_map, config) -> Option<TurnRange>` | [EXISTS] | Determine turns kept verbatim from start (most recent loop only) |
| `keep_recent(record, turn_map, config) -> Option<CompactedSection>` | [EXISTS] | Create recent section with truncated tool outputs (most recent loop only) |
| `keep_compacted(record, turn_map, config, is_most_recent) -> Option<CompactedSection>` | [EXISTS] | Create summarised section; for most recent: middle only; for older: entire loop |
| `compact(record, config, is_most_recent) -> CompactionBlock` | [EXISTS] | Default: assembles from the three methods above |

### DefaultBlockCompaction [EXISTS]

Stateless implementation. All parameters come from `CompactionConfig`.

| Section | Behavior |
|---------|----------|
| `keep_first` | Returns turn range `0..keep_first_turns` |
| `keep_recent` | Truncates tool outputs to `tool_output_max_lines` |
| `keep_compacted` | Per-turn one-liner summaries bounded by `max_summary_tokens`; drops remaining turns when budget exhausted |

**Limitation**: `DefaultBlockCompaction.keep_compacted` is basic -- it drops turns that exceed the token budget rather than producing a holistic summary. More sophisticated strategies (e.g. LLM-based) should summarise ALL turns within the budget.

---

## TurnMap [EXISTS]

Maps turn indices to message index ranges within a message array. Built from messages by grouping on `TurnId.turn_index`.

| Method | Status | Description |
|--------|--------|-------------|
| `from_messages(messages) -> TurnMap` | [EXISTS] | Build from messages; messages without `turn_id` are their own group |
| `turn_count() -> u32` | [EXISTS] | Number of turn groups |
| `messages_for_range(range, all_msgs) -> &[AgentMessage]` | [EXISTS] | Slice of messages belonging to a `TurnRange` |
| `turn_msg_range(turn_index) -> Option<(usize, usize)>` | [EXISTS] | Message index range for a single turn |

---

## Orchestration [EXISTS]

Cross-loop compaction coordination. The orchestrator resolves scope, then creates `CompactionBlock`s for the current loop and earlier loops within scope.

| Function | Status | Description |
|----------|--------|-------------|
| `compact_session_loops(session, current_loop_id, strategy, config, max_context_tokens)` | [EXISTS] | Creates blocks: current loop gets all three sections; earlier loops get only `keep_compacted` |
| `build_context_from_session(session, current_loop_id, config, max_context_tokens)` | [EXISTS] | Walks the loop chain, loads from `CompactionBlock`s where available, raw messages otherwise |
| `resolve_scope(session, chain, scope, max_context_tokens)` | [EXISTS] | Resolves `CompactionScope` to a concrete count of earlier loops |

---

## CompactionConfig [EXISTS]

Full compaction policy -- controls both WHEN and HOW to compact.

### WHEN to compact

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `compact_at_pct` | `f64` | `0.90` | [EXISTS] | Fraction of `max_context_tokens` at which headroom is measured |
| `compact_budget_threshold_pct` | `f64` | `0.05` | [EXISTS] | Minimum headroom fraction before compaction fires |
| `compaction_scope` | `CompactionScope` | `FixedCount(3)` | [EXISTS] | How many earlier loops to include |

### HOW to compact

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `keep_first_turns` | `usize` | `2` | [EXISTS] | Turns kept verbatim from start (most recent loop) |
| `keep_recent_turns` | `usize` | `10` | [EXISTS] | Turns kept from end (extended to turn boundary) |
| `max_summary_tokens` | `usize` | `2_000` | [EXISTS] | Token budget for summarised middle section |
| `tool_output_max_lines` | `usize` | `50` | [EXISTS] | Max lines per tool output in keep_recent section |

---

## Code Reference

| Concept | File |
|---------|------|
| `CompactionBlock`, `TurnRange`, `CompactedSection`, `TurnMap` | `src/context/compaction.rs` |
| `CompactionStrategy`, `DefaultCompaction`, `BlockCompactionStrategy`, `DefaultBlockCompaction` | `src/context/strategy.rs` |
| `CompactionConfig`, `CompactionScope`, `ContextConfig` | `src/context/config.rs` |
| `compact_session_loops()`, `build_context_from_session()`, `resolve_scope()` | `src/context/orchestration.rs` |
| `compact_messages()` (legacy in-memory) | `src/context/compact_messages.rs` |
| `ContextTracker` (token tracking) | `src/context/tracker.rs` |
| `compaction_strategy` and `block_compaction_strategy` fields | `src/agent_loop/config.rs` |

---

## Conceptual Notes

- **before_compaction_start / after_compaction_end callbacks** [CONCEPTUAL] -- Currently no lifecycle hooks fire around compaction. The plan envisions `before_compaction_start` (for pre-compaction indexing/memory extraction) and `after_compaction_end` (for post-compaction verification) as blank-by-default callbacks.
- **Config spread** [CONCEPTUAL: streamline] -- Compaction configuration is currently split across two locations: `ContextConfig.compaction` (the `CompactionConfig` struct with WHEN/HOW settings) and `AgentLoopConfig` (which holds `compaction_strategy: Option<Arc<dyn CompactionStrategy>>` and `block_compaction_strategy: Option<Arc<dyn BlockCompactionStrategy>>`). Streamlining into a single `CompactionConfig` location that bundles both the policy and the strategy would reduce configuration surface area.
- **LLM-based Summarisation** -- `DefaultBlockCompaction.keep_compacted` is a basic per-turn one-liner generator. The `BlockCompactionStrategy` trait is designed for more sophisticated strategies that call an LLM to produce holistic digests of all turns within the `max_summary_tokens` budget.
- **Compaction Events** [EXISTS] -- `CompactionStarted` and `CompactionEnded` events bracket compaction execution, providing estimated token counts before/after. These are consumed by `SessionRecorder` for observability.
- **Legacy vs Modern** -- Two systems coexist: `CompactionStrategy` (legacy, in-memory, rewrites messages) is used when `AgentContext.session` is `None`; `BlockCompactionStrategy` (modern, non-destructive overlays) is used when session data is available. The legacy path is preserved for backward compatibility and simple stateless use cases.
