<!-- Last verified: 2026-05-24 by Claude Code (phi-core 0.9.0 — BlockCompactionStrategy is now #[async_trait]; compact_session_loops is async fn) -->
# Context Compaction

Compaction manages context window pressure by creating non-destructive overlays on session history. Nothing is deleted or replaced — original messages remain authoritative in `LoopRecord.messages`.

## How it works

When the context approaches the token budget, a `CompactionBlock` is created on the current `LoopRecord`. This block controls what gets loaded into context for subsequent LLM calls, replacing the raw messages with a compacted view.

### CompactionBlock anatomy

A block has three sections:

```
┌─────────────────────────────────────────────┐
│  keep_first    │ Original turns, verbatim    │  Most recent loop only
│  (turns 0..1)  │ No modification              │
├────────────────┼────────────────────────────-│
│  keep_compacted│ Summarised one-liners       │  All loops
│  (turns 2..N-6)│ ≤ max_summary_tokens        │
├────────────────┼────────────────────────────-│
│  keep_recent   │ Tool outputs truncated      │  Most recent loop only
│  (turns N-5..N)│ Rest unchanged              │
└─────────────────────────────────────────────┘
```

- **`keep_first`** — verbatim turns from the start. Only for the most recent loop. Original messages in this range are used as-is.
- **`keep_compacted`** — fully summarised middle section. For the most recent loop this is the gap between keep_first and keep_recent. For older loops this covers the entire loop.
- **`keep_recent`** — recent turns with only tool outputs truncated. Only for the most recent loop.

### When compaction fires

Compaction uses a percentage-based threshold:

```
headroom = compact_at_pct − (system_tokens / max_tokens) − (current_tokens / max_tokens)
```

Compaction fires when `headroom < compact_budget_threshold_pct`.

With defaults (100k max, 4k system, 90% ceiling, 5% threshold): fires when current tokens exceed ~81k.

## Configuration

### ContextConfig

```rust
ContextConfig {
    max_context_tokens: 100_000,   // Model's context window
    system_prompt_tokens: 4_000,   // Reserved for system prompt
    compaction: CompactionConfig { // Always present when limits are set
        // WHEN
        compact_at_pct: 0.90,
        compact_budget_threshold_pct: 0.05,
        compaction_scope: CompactionScope::FixedCount(3),
        // HOW
        keep_first_turns: 2,
        keep_recent_turns: 10,
        max_summary_tokens: 2_000,
        tool_output_max_lines: 50,
    },
}
```

Compaction is disabled entirely by setting `context_config: None` on `AgentLoopConfig`.

## CompactionScope

Controls how many earlier loops are included in compaction and context loading:

- **`FixedCount(n)`** — Compact a fixed number of earlier loops. Simple and predictable.
- **`TokenBudget`** — Walk the chain backward, accumulating per-loop token estimates, and stop when `max_context_tokens` would be exceeded.

### TokenBudget and exceeding the window

The `TokenBudget` scope can include loops whose raw messages **exceed** `max_context_tokens`. This is intentional: the compacted summaries of those loops will fit in the window, even though the originals did not. This enables richer context for expensive summarisation strategies (e.g. LLM summarisers) that compress large loops into compact representations that then fit within the budget.

For example, if a loop has 50k tokens of raw messages and the window is 100k, `TokenBudget` includes it in scope. The strategy's `keep_compacted` method produces a ~500 token summary of that loop, which fits easily.

## Cross-loop compaction

When compaction fires, blocks are created for the current loop and earlier loops within the `compaction_scope` on the active chain.

The "active chain" is the linear path from root to current loop via `parent_loop_id` links:

- **Parallel branches** — only the selected branch is on the chain. Unselected siblings get their own compaction if/when they become active.
- **Reruns** — the rerun's parent points to the pre-rerun loop. Superseded runs are siblings, not ancestors.

### Loading rule

When building context from session history:
- Most recent loop: `keep_first` + `keep_compacted` + `keep_recent`
- Earlier loops (within `compaction_scope`): only `keep_compacted`
- Loops older than that: skipped entirely

## Custom strategies

Compaction strategies are fields on `CompactionConfig`, not on `AgentLoopConfig`. The dispatch logic in `run.rs` reads them from `ctx_config.compaction`:

- **`in_memory_strategy`** — custom in-memory compaction strategy (used when session is `None`)
- **`block_strategy`** — block-based compaction strategy (used when session is `Some`; falls back to `DefaultBlockCompaction`)

Implement `BlockCompactionStrategy` to customise any section.

As of phi-core 0.9.0, `BlockCompactionStrategy` is `#[async_trait]`-marked
and all four methods are `async fn` — implementations can issue LLM calls
inside `keep_compacted` / `keep_recent` without `block_in_place` workarounds:

```rust
use async_trait::async_trait;
use phi_core::{BlockCompactionStrategy, CompactionConfig, CompactedSection, TurnRange, TurnMap, DefaultBlockCompaction};
use phi_core::session::LoopRecord;

struct MyStrategy;

#[async_trait]
impl BlockCompactionStrategy for MyStrategy {
    async fn keep_first(&self, record: &LoopRecord, turn_map: &TurnMap, config: &CompactionConfig) -> Option<TurnRange> {
        DefaultBlockCompaction.keep_first(record, turn_map, config).await // delegate
    }

    async fn keep_recent(&self, record: &LoopRecord, turn_map: &TurnMap, config: &CompactionConfig) -> Option<CompactedSection> {
        DefaultBlockCompaction.keep_recent(record, turn_map, config).await // delegate
    }

    async fn keep_compacted(&self, record: &LoopRecord, turn_map: &TurnMap, config: &CompactionConfig, is_most_recent: bool) -> Option<CompactedSection> {
        // Custom LLM-based summarisation — issue LLM calls directly without bridging.
        my_llm_summarize(record, turn_map, config, is_most_recent).await
    }
}
```

Sync impls that don't `.await` anything migrate by adding
`#[async_trait::async_trait]` + the `async` keyword on each method signature;
the bodies remain unchanged. See the per-turn debug-capture surface in
[`debugging.md`](debugging.md) for the canonical pattern to inspect what
each compacted turn looked like to the model.

Set the custom strategy on `CompactionConfig`:

```rust
let compaction_config = CompactionConfig {
    block_strategy: Some(Arc::new(MyStrategy)),
    ..Default::default()
};
```

## Public APIs

### Orchestration functions

- `compact_session_loops(session, loop_id, strategy, config, max_tokens)` — Creates `CompactionBlock`s for the current loop and earlier loops within the configured scope. Mutates the session in place; caller persists to disk.
- `build_context_from_session(session, loop_id, config, max_tokens)` — Builds a compacted context by walking the loop chain, loading from blocks where available and raw messages otherwise.

### BasicAgent methods

- `compact_context_with_sender(&mut self, tx)` — Standalone compaction with full event lifecycle: `AgentStart(Compaction)` → `CompactionStarted` → compact → `CompactionEnded` → `AgentEnd`. No-op if session or config is missing.
- `compact_context(&mut self) -> usize` — Fire-and-forget compaction. Returns the number of loops that received new CompactionBlocks. Returns 0 if session or config is missing.

## Events

Two events bracket compaction:
- `CompactionStarted { loop_id, estimated_tokens, message_count, timestamp }`
- `CompactionEnded { loop_id, messages_before, messages_after, estimated_tokens_before, estimated_tokens_after, loops_compacted, timestamp }`

For standalone compaction (`compact_context_with_sender`), these appear inside a dedicated `LoopRecord` with `continuation_kind: Compaction`.

## TurnId tracking

Every message pushed during the agent loop carries a `TurnId { loop_id, turn_index }` identifying which turn produced it. This enables `TurnMap::from_messages()` to group messages by turn without replaying the event stream.

`TurnId` is stored on `LlmMessage.turn_id` and serialized as an optional `turnId` field alongside the existing message JSON. Old data without `turnId` deserializes with `turn_id: None`.

## Data model

### Struct definitions

```rust
pub struct CompactionBlock {
    pub keep_first: Option<TurnRange>,         // verbatim turns from start (most recent loop only)
    pub keep_recent: Option<CompactedSection>,  // truncated tool outputs (most recent loop only)
    pub keep_compacted: Option<CompactedSection>,// summarised section (all loops)
    pub created_at: DateTime<Utc>,
}

pub struct TurnRange {
    pub start_turn: u32,  // inclusive, matches TurnId.turn_index
    pub end_turn: u32,    // inclusive
}

pub struct CompactedSection {
    pub range: TurnRange,
    pub messages: Vec<AgentMessage>,  // replacement messages for this range
}

pub struct TurnId {
    pub loop_id: String,
    pub turn_index: u32,
}
```

### Serialization format

CompactionBlock on LoopRecord:

```json
{
  "loop_id": "session123.model.1",
  "messages": [ ... ],
  "compaction_block": {
    "keep_first": { "startTurn": 0, "endTurn": 1 },
    "keep_compacted": {
      "range": { "startTurn": 2, "endTurn": 7 },
      "messages": [
        { "role": "user", "content": [{"type": "text", "text": "[Summary] User asked about X"}], "timestamp": 123 }
      ]
    },
    "keep_recent": {
      "range": { "startTurn": 8, "endTurn": 12 },
      "messages": [ ... ]
    },
    "createdAt": "2026-03-28T10:00:00Z"
  }
}
```

TurnId on LlmMessage:

```json
{
  "role": "assistant",
  "content": [...],
  "stopReason": "stop",
  "model": "claude-sonnet-4-6",
  "provider": "anthropic",
  "usage": { ... },
  "timestamp": 123,
  "turnId": { "loopId": "session123.model.1", "turnIndex": 3 }
}
```

Old data without `turnId` deserializes as `turn_id: None`.

## Invariants

1. If `keep_first` is `Some`, `keep_compacted` must also be `Some` (there must be a middle to summarise).
2. If `keep_recent` is `Some`, `keep_compacted` must also be `Some`.
3. For older loops (not most recent), `keep_first` and `keep_recent` are always `None`.
4. `CompactedSection.range` bounds must be within the loop's turn count.
5. If a loop has a `compaction_block`, all older loops on the same chain must also have one.
6. If a ToolCall content block is within a section's turn range, its corresponding ToolResult message must also be within the same section. Turn-based grouping (via `TurnId`) enforces this.

## Summary budget semantics

`max_summary_tokens` is a token budget for the summarised output, not a per-turn limit. Strategies should aim to summarise ALL turns within this budget (e.g. shorter summaries or LLM-generated digests), not merely process turns until the budget runs out. `DefaultBlockCompaction` is a basic implementation that drops remaining turns when exhausted.

## Backward compatibility

- `LoopRecord.compaction_block` uses `#[serde(default, skip_serializing_if = "Option::is_none")]` — old records without the field deserialize as `None`.
- `LlmMessage.turn_id` uses `#[serde(default, skip_serializing_if = "Option::is_none")]` — old messages without `turnId` deserialize as `None`.
- The `CompactionConfig` field on `ContextConfig` uses `#[serde(default)]` — old configs get `CompactionConfig::default()`.
