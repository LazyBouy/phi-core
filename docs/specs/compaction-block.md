# CompactionBlock Specification

## Struct definitions

### CompactionBlock

```rust
pub struct CompactionBlock {
    pub keep_first: Option<TurnRange>,         // verbatim turns from start (most recent loop only)
    pub keep_recent: Option<CompactedSection>,  // truncated tool outputs (most recent loop only)
    pub keep_compacted: Option<CompactedSection>,// summarised section (all loops)
    pub created_at: DateTime<Utc>,
}
```

### TurnRange

```rust
pub struct TurnRange {
    pub start_turn: u32,  // inclusive, matches TurnId.turn_index
    pub end_turn: u32,    // inclusive
}
```

### CompactedSection

```rust
pub struct CompactedSection {
    pub range: TurnRange,
    pub messages: Vec<AgentMessage>,  // replacement messages for this range
}
```

### TurnId

```rust
pub struct TurnId {
    pub loop_id: String,
    pub turn_index: u32,
}
```

## JSON serialization

### CompactionBlock on LoopRecord

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

### TurnId on LlmMessage

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
6. If a ToolCall content block is within a section's turn range, its corresponding ToolResult message must also be within the same section. Turn-based grouping (via `TurnId`) enforces this — all messages in a turn carry the same `TurnId`, so they are never split across sections.

## CompactionConfig fields

- `max_summary_tokens` — Token budget for the summarised output. Strategies should aim to summarise ALL turns within this budget (e.g. shorter summaries or LLM-generated digests), not merely process turns until the budget runs out. `DefaultBlockCompaction` is a basic implementation that drops remaining turns when exhausted.

## Backward compatibility

- `LoopRecord.compaction_block` uses `#[serde(default, skip_serializing_if = "Option::is_none")]` — old records without the field deserialize as `None`.
- `LlmMessage.turn_id` uses `#[serde(default, skip_serializing_if = "Option::is_none")]` — old messages without `turnId` deserialize as `None`.
- The `CompactionConfig` field on `ContextConfig` uses `#[serde(default)]` — old configs get `CompactionConfig::default()`.
