<!-- Last verified: 2026-04-05 by Claude Code -->
# Context Pruning

Context pruning is a model-directed mechanism for surgically removing irrelevant content from the working context during a run. Unlike compaction (which is threshold-triggered and bulk), pruning gives the model fine-grained control over what stays in the context window.

## Philosophy

Context pruning saves **context length** (tokens in the context window), not monetary cost. The token cost of a pruned message has already been paid -- pruning cannot reclaim it. What pruning reclaims is *space*: room in the context window for the model to continue working without hitting the context limit.

Think of it as a researcher working through a stack of papers. The researcher freely explores tangential references, reads through lengthy tool outputs, and investigates dead ends. When a line of inquiry turns out to be irrelevant, the researcher sets those papers aside rather than keeping them on the desk. The desk has limited space; the filing cabinet does not. Pruning moves content from desk to cabinet.

This freedom to explore without anxiety about context length is the core value proposition. The model can request verbose tool outputs, try multiple approaches, and investigate broadly -- knowing it can prune the dead ends and keep only what matters for the current task.

## Static vs In-Run Context

Every message in the context belongs to one of two streams:

- **user_context** -- All `User` messages: the initial prompt, follow-ups, steering messages, and any user-injected content. These represent user intent.
- **inrun_context** -- All model-generated content: `Assistant` messages, `ToolCall` content, and `ToolResult` messages. These are the model's working memory.

The **system_prompt** is separate from both streams. It is a dedicated field on `AgentContext`, always occupies the first position, and is never subject to pruning or compaction.

### Pruning Rules

- **user_context is NEVER pruned.** User intent is sacred. The model cannot discard the user's words, steering messages, or follow-up instructions.
- **inrun_context CAN be surgically pruned** by the model using the PrunTool. The model decides what is no longer relevant and removes it.
- **system_prompt is never pruned.** It is not part of either stream.

## PrunTool Variants

phi-core provides two pruning operations, both invoked by the model as tool calls:

### `prun(tokens)`

Silent removal. The model specifies a token budget to reclaim, and the oldest inrun_context entries (by timestamp) are removed from the working context until the budget is met.

- Removed content is preserved in the session log -- nothing is lost permanently
- Removed entries become invisible to the LLM on subsequent turns
- The model sees a `ToolResult` confirming how many tokens were reclaimed

### `prun_with_memo(tokens, memo)`

Removal with summary. Same as `prun`, but the model provides a concise memo string that replaces the pruned content in the working context.

- Each pruned message with a memo creates a separate `PrunedMemo` entry at its original timestamp, preserving chronological order
- Useful when the pruned content contained decisions or conclusions the model wants to remember
- The memo should be concise -- a few sentences, not a reproduction of the pruned content

### Model Autonomy

The model decides which variant to use and when. Typical patterns:

- **Silent prune** after exploring a dead end (e.g., reading a file that turned out to be irrelevant)
- **Memo prune** after a productive investigation (e.g., "Investigated auth module: uses JWT with RS256, tokens expire after 1h, refresh handled in middleware")
- **No prune** when all context remains relevant to the current task

## Working Context Rebuild

Each turn, the working context sent to the LLM is rebuilt from scratch by `build_working_context()`, merging the two streams:

1. Collect all `user_context` entries with their timestamps
2. Collect all *live* `inrun_context` entries with their timestamps
3. For each `PrunedMemo` entry, create a separate User message with the memo text at the entry's original timestamp
4. Sort all collected entries by timestamp to preserve chronological order
5. Prepend the system_prompt

The result is a coherent conversation history where:
- User messages are always present
- Pruned-silent entries are invisible (the conversation flows as if they never existed)
- Each pruned-with-memo entry appears as a separate brief summary message at its original timestamp position, preserving the chronological position of the message it replaced

## Session Log Integrity

The session log (`context.messages`) records **everything** that happened during the run. Pruning never modifies the session log -- it only affects what the LLM sees in the working context.

### PrunRecord

Each prune operation emits a `PrunApplied` event (recorded in `LoopRecord.events` by `SessionRecorder`) containing:

- **pruned_timestamps** -- `Vec<u64>` of timestamps identifying the pruned messages
- **tokens_removed** -- Total tokens reclaimed
- **messages_removed** -- Number of messages pruned
- **memo** -- Optional summary string (present only for `prun_with_memo`)

On session reload, the two context streams are reconstructed by walking `LoopRecord.events` to find `PrunApplied` events. The `pruned_timestamps` field identifies which messages were pruned. These messages are placed in the pruned state (`PrunedSilent` or `PrunedMemo` depending on whether `memo` is `Some`), and their memo (if any) is restored as a separate message at the correct chronological position.

## Compaction Interaction

Pruning and compaction are complementary mechanisms that operate at different levels:

| | Pruning | Compaction |
|---|---|---|
| **Trigger** | Model-directed (tool call) | Threshold-triggered (automatic) |
| **Granularity** | Surgical (specific messages) | Bulk (entire middle section) |
| **Scope** | inrun_context only | All messages in the compaction window |
| **Preserved in** | Session log + PrunRecord | CompactionBlock overlay |

### After Compaction

When compaction fires, it summarizes a range of messages into a CompactionBlock. After compaction:

- All surviving messages (the summary, kept-first, and kept-recent) become part of **user_context** -- they are treated as established context and are unprunable
- New model-generated content after compaction starts a fresh **inrun_context** stream
- The model can prune this new inrun_context as usual

This means compaction resets the pruning boundary. Content that was once prunable inrun_context, if it survives compaction, becomes permanent user_context.

## Configuration

### TOML

```toml
[tools]
enabled = ["bash", "read_file", "write_file", "edit_file", "search", "prun"]
```

Adding `"prun"` to the enabled tools list makes both `prun` and `prun_with_memo` available to the model. They are two operations exposed through a single tool registration.

### Rust (Programmatic)

```rust
use phi_core::agents::BasicAgent;
use phi_core::provider::ModelConfig;

let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Sonnet", &api_key))
    .with_default_tools()
    .with_prun_tool();  // enables context pruning
```

The `with_prun_tool()` builder method registers the PrunTool, making both pruning variants available. It can be combined with any other tool configuration.

### Recommended Setup

Context pruning works best when compaction is also configured, providing both surgical (model-directed) and bulk (automatic) context management:

```toml
[tools]
enabled = ["bash", "read_file", "write_file", "edit_file", "search", "prun"]

[compaction]
max_context_tokens = 200000
compact_at_pct = 0.85
keep_first_turns = 2
keep_recent_turns = 4
```

With this setup, the model can prune irrelevant exploration results as it works, and compaction provides a safety net if the context still grows too large.
