<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `compact_messages` *(src/context/)*

> **Note:** The algorithm below describes the legacy in-memory compaction (`compact_messages()`).
> The current system uses a non-destructive overlay model via `CompactionBlock` / `BlockCompactionStrategy`.
> See [compaction concept](../concepts/compaction.md) for the current design.

**Purpose:** Reduce context size using a 3-level strategy (Level 1 → 2 → 3) until messages fit the token budget.
**Preconditions:** `messages` is a complete conversation history.
**Postconditions:** Returns a subset/summary of messages with `total_tokens(result) <= budget`.

```
FUNCTION compact_messages(
  messages: Vec<AgentMessage>,
  config: ContextConfig
) -> Vec<AgentMessage>

  budget ← config.max_context_tokens - config.system_prompt_tokens

  // Already fits — return unchanged
  IF total_tokens(messages) <= budget THEN
    RETURN messages
  END IF

  // ── Level 1: Truncate verbose tool outputs ──────────────────────────────
  compacted ← level1_truncate_tool_outputs(messages, config.tool_output_max_lines)
  IF total_tokens(compacted) <= budget THEN
    RETURN compacted
  END IF

  // ── Level 2: Summarize old turns ────────────────────────────────────────
  compacted ← level2_summarize_old_turns(compacted, config.keep_recent)
  IF total_tokens(compacted) <= budget THEN
    RETURN compacted
  END IF

  // ── Level 3: Drop middle messages ───────────────────────────────────────
  RETURN level3_drop_middle(compacted, config, budget)

END FUNCTION
```

---

### `level1_truncate_tool_outputs` *(src/context/)*

> **Note:** The algorithm below describes the legacy in-memory compaction (`compact_messages()`).
> The current system uses a non-destructive overlay model via `CompactionBlock` / `BlockCompactionStrategy`.
> See [compaction concept](../concepts/compaction.md) for the current design.

**Purpose:** Truncate long tool output text to head + tail, preserving message structure.

```
FUNCTION level1_truncate_tool_outputs(
  messages: Vec<AgentMessage>,
  max_lines: usize
) -> Vec<AgentMessage>

  RETURN [
    FOR EACH msg IN messages
      IF msg is ToolResult THEN
        // Truncate each Text content block
        new_content ← [
          FOR EACH content IN msg.content
            IF content is Text THEN
              Text { text: truncate_head_tail(content.text, max_lines) }
            ELSE
              content unchanged
            END IF
        ]
        ToolResult { ...msg, content: new_content }
      ELSE
        msg unchanged
      END IF
  ]

END FUNCTION

FUNCTION truncate_head_tail(text: String, max_lines: usize) -> String
  lines ← text.split_lines()
  IF lines.count() <= max_lines THEN
    RETURN text
  END IF

  head_count ← max_lines / 2
  tail_count ← max_lines - head_count
  omitted ← lines.count() - head_count - tail_count

  RETURN (
    lines[0..head_count].join("\n") +
    "\n\n[... {omitted} lines truncated ...]\n\n" +
    lines[lines.count()-tail_count..].join("\n")
  )
END FUNCTION
```

---

### `level2_summarize_old_turns` *(src/context/)*

> **Note:** The algorithm below describes the legacy in-memory compaction (`compact_messages()`).
> The current system uses a non-destructive overlay model via `CompactionBlock` / `BlockCompactionStrategy`.
> See [compaction concept](../concepts/compaction.md) for the current design.

**Purpose:** Keep the most recent `keep_recent` messages in full; replace older assistant-plus-tool-result groups with one-line summaries.

```
FUNCTION level2_summarize_old_turns(
  messages: Vec<AgentMessage>,
  keep_recent: usize
) -> Vec<AgentMessage>

  len ← messages.count()
  IF len <= keep_recent THEN RETURN messages END IF

  boundary ← len - keep_recent  // messages before this index are candidates

  result ← []
  i ← 0

  WHILE i < boundary
    msg ← messages[i]

    MATCH msg
      CASE Assistant(content) →
        // Build one-line summary
        short_texts ← [t FOR t IN text content IF t.len <= 200]
        tool_count  ← count of ToolCall blocks in content

        summary ←
          IF short_texts non-empty  → JOIN(short_texts)
          ELSE IF tool_count > 0    → "[Assistant used {tool_count} tool(s)]"
          ELSE                      → "[Assistant response]"

        result.append(User{ content: [Text("[Summary] {summary}")] })

        // Skip following ToolResult messages that belong to this turn
        i ← i + 1
        WHILE i < boundary AND messages[i] is ToolResult
          i ← i + 1
        END WHILE
        CONTINUE  // skip i++ below

      CASE ToolResult →
        // Skip orphaned tool results
        i ← i + 1
        CONTINUE

      CASE other →
        // Keep user messages and extension messages
        result.append(other)
    END MATCH

    i ← i + 1
  END WHILE

  // Append recent messages in full
  result.extend(messages[boundary..])
  RETURN result

END FUNCTION
```

---

### `level3_drop_middle` *(src/context/)*

> **Note:** The algorithm below describes the legacy in-memory compaction (`compact_messages()`).
> The current system uses a non-destructive overlay model via `CompactionBlock` / `BlockCompactionStrategy`.
> See [compaction concept](../concepts/compaction.md) for the current design.

**Purpose:** Keep the first `keep_first` and last `keep_recent` messages; drop everything in between, inserting a marker.

```
FUNCTION level3_drop_middle(
  messages: Vec<AgentMessage>,
  config: ContextConfig,
  budget: usize
) -> Vec<AgentMessage>

  len ← messages.count()
  first_end   ← min(config.keep_first, len)
  recent_start ← max(0, len - config.keep_recent)

  IF first_end >= recent_start THEN
    // Not enough room to split — keep as many recent as fit
    RETURN keep_within_budget(messages, budget)
  END IF

  removed ← recent_start - first_end
  marker ← User { content: [Text("[Context compacted: {removed} messages removed to fit context window]")] }

  result ← messages[0..first_end] + [marker] + messages[recent_start..]

  IF total_tokens(result) > budget THEN
    RETURN keep_within_budget(result, budget)
  END IF

  RETURN result

END FUNCTION

FUNCTION keep_within_budget(messages, budget) -> Vec<AgentMessage>
  // Greedily keep most-recent messages that fit
  result ← []
  remaining ← budget

  FOR EACH msg IN REVERSE(messages)
    tokens ← message_tokens(msg)
    IF tokens > remaining THEN BREAK END IF
    remaining ← remaining - tokens
    result.prepend(msg)
  END FOR

  IF result.count() < messages.count() THEN
    removed ← messages.count() - result.count()
    result.prepend(User { content: [Text("[Context compacted: {removed} messages removed]")] })
  END IF

  RETURN result
END FUNCTION
```

---

### `estimate_tokens` *(src/context/)*

**Purpose:** Fast heuristic token count for a text string.

```
FUNCTION estimate_tokens(text: String) -> usize
  RETURN ceil(text.byte_length() / 4)
  // Heuristic: ~4 UTF-8 bytes per token for English text.
  // Not precise — use tiktoken for exact counts.
END FUNCTION

FUNCTION content_tokens(content: Vec<Content>) -> usize
  total ← 0
  FOR EACH block IN content
    MATCH block
      CASE Text { text }          → total += estimate_tokens(text)
      CASE Image { data }         →
        raw_bytes ← data.base64_decoded_byte_length()
        // ~750 bytes per image token; floor 85, cap 16,000
        total += clamp(raw_bytes / 750, 85, 16_000)
      CASE Thinking { thinking }  → total += estimate_tokens(thinking)
      CASE ToolCall { name, args }→
        total += estimate_tokens(name) + estimate_tokens(args.to_string()) + 8
    END MATCH
  END FOR
  RETURN total
END FUNCTION

FUNCTION message_tokens(msg: AgentMessage) -> usize
  MATCH msg
    CASE Llm(User { content })            → RETURN content_tokens(content) + 4
    CASE Llm(Assistant { content })       → RETURN content_tokens(content) + 4
    CASE Llm(ToolResult { tool_name, content }) →
      RETURN content_tokens(content) + estimate_tokens(tool_name) + 8
    CASE Extension { data }               → RETURN estimate_tokens(data.to_string()) + 4
  END MATCH
END FUNCTION
```

---
