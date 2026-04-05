<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
## 4. Decision Logic

### Tool Execution Strategy Dispatch

```
FUNCTION select_execution_strategy(strategy, tool_calls) -> ExecutionPath
  MATCH strategy
    CASE Sequential →
      // One at a time; check steering after each tool
      // Use when: tools have shared mutable state, need human-in-the-loop each step
      RETURN sequential_path

    CASE Parallel (default) →
      // All tools concurrently via join_all
      // Use when: tools are independent (most cases); lowest latency
      RETURN parallel_path

    CASE Batched { size } →
      // Groups of `size` concurrently; check steering between groups
      // Use when: tools are independent but human oversight between groups wanted
      RETURN batched_path(size)
  END MATCH
END FUNCTION
```

### Compaction Level Selection

```
FUNCTION select_compaction_level(messages, config) -> CompactionAction
  budget ← config.max_context_tokens - config.system_prompt_tokens
  current ← total_tokens(messages)

  IF current <= budget          → RETURN NoCompaction
  ELSE IF level1 fits in budget → RETURN Level1 (truncate tool outputs)
  ELSE IF level2 fits in budget → RETURN Level2 (summarize old turns)
  ELSE                          → RETURN Level3 (drop middle)
END FUNCTION
```

### StopReason Determination (in provider implementations)

```
FUNCTION determine_stop_reason(provider_stop_signal) -> StopReason
  MATCH provider_stop_signal
    CASE "end_turn" (Anthropic) | "stop" (OpenAI) | natural end → Stop
    CASE "max_tokens" (Anthropic) | "length" (OpenAI)           → Length
    CASE "tool_use" (Anthropic) | "tool_calls" (OpenAI)         → ToolUse
    CASE cancel token triggered                                  → Aborted
    CASE any provider error                                      → Error
  END MATCH
END FUNCTION
```

### Input Filter Chain

```
FUNCTION apply_input_filters(filters, user_text) -> FilterChainResult
  warnings ← []

  FOR EACH filter IN filters
    MATCH filter.filter(user_text)
      CASE Pass     → continue
      CASE Warn(w)  → warnings.append(w)
      CASE Reject(r) →
        // First Reject wins — discards all accumulated warnings
        RETURN Rejected(r)
    END MATCH
  END FOR

  IF warnings non-empty THEN
    RETURN PassWithWarnings(warnings)
  END IF

  RETURN Pass
END FUNCTION
```

### Context Overflow Detection

```
FUNCTION detect_context_overflow(provider_error_or_message) -> bool

  // Path 1: HTTP error response
  IF error is ProviderError::ContextOverflow THEN RETURN true END IF

  // Path 2: SSE streaming error (Anthropic, OpenAI report overflow in-stream)
  IF message.stop_reason == Error
     AND message.error_message defined
     AND is_context_overflow_message(message.error_message)
  THEN RETURN true END IF

  RETURN false

  // Caller response: next turn will trigger compact_messages() if context_config set
END FUNCTION
```

---
