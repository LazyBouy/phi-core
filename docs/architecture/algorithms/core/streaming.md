<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `stream_assistant_response` *(src/agent_loop/)*

**Purpose:** Call the LLM with the current context, stream events to the channel, and return the final `Message`. Includes retry logic for transient errors.
**Preconditions:** `context.messages` has at least one user message.
**Postconditions:** Returns a complete `Message::Assistant`; events emitted include `MessageStart`, zero or more `MessageUpdate`, and `MessageEnd`.

```
FUNCTION stream_assistant_response(
  context: AgentContext,
  config: AgentLoopConfig,
  tx: EventChannel<AgentEvent>,
  cancel: CancellationToken
) -> Message

  // Build working context: merges user_context + live inrun_context + memos, sorted by timestamp.
  // Falls back to context.messages when prun streams are empty.
  base_messages ← context.build_working_context()

  // Apply optional context transform (e.g. for custom preprocessing)
  messages ← IF config.transform_context defined
              THEN config.transform_context(base_messages)
              ELSE base_messages

  // Filter to LLM-compatible messages (drop Extension messages)
  llm_messages ← IF config.convert_to_llm defined
                 THEN config.convert_to_llm(messages)
                 ELSE [m FOR m IN messages IF m is Llm variant]

  // Build tool schema list (schema only, no execute functions)
  tool_defs ← [
    ToolDefinition(name, description, parameters_schema)
    FOR EACH tool IN context.tools
  ]

  retry ← config.retry_config
  attempt ← 0

  // ── Retry loop ──────────────────────────────────────────────────────────
  WHILE true
    stream_config ← StreamConfig {
      model, system_prompt: context.system_prompt,
      messages: llm_messages, tools: tool_defs,
      thinking_level, api_key, max_tokens, temperature,
      model_config, cache_config
    }

    (stream_tx, stream_rx) ← new unbounded channel
    result ← AWAIT config.provider.stream(stream_config, stream_tx, cancel)

    MATCH result
      CASE Err(e) IF e.is_retryable()
                 AND attempt < retry.max_retries
                 AND NOT cancel.is_cancelled →
        attempt ← attempt + 1
        delay ← e.retry_after() OR retry.delay_for_attempt(attempt)
        log_retry(attempt, retry.max_retries, delay, e)
        AWAIT sleep(delay)
        CONTINUE  // retry

      CASE other →
        BREAK with (result, stream_rx)
    END MATCH
  END WHILE

  // ── Process streaming events ─────────────────────────────────────────────
  partial_message ← null

  FOR EACH stream_event IN stream_rx (drain available)
    MATCH stream_event
      CASE Start →
        placeholder ← empty Assistant message
        partial_message ← placeholder
        EMIT MessageStart(placeholder)

      CASE TextDelta(delta) →
        IF partial_message defined THEN
          EMIT MessageUpdate(partial_message, StreamDelta::Text(delta))
        END IF

      CASE ThinkingDelta(delta) →
        IF partial_message defined THEN
          EMIT MessageUpdate(partial_message, StreamDelta::Thinking(delta))
        END IF

      CASE ToolCallDelta(delta) →
        IF partial_message defined THEN
          EMIT MessageUpdate(partial_message, StreamDelta::ToolCallDelta(delta))
        END IF

      CASE Done(message) →
        am ← message as AgentMessage
        partial_message ← am
        // MessageStart was already emitted on Start
        EMIT MessageEnd(am)

      CASE Error(message) →
        am ← message as AgentMessage
        IF partial_message is null THEN
          EMIT MessageStart(am)
        END IF
        partial_message ← am
        EMIT MessageEnd(am)
    END MATCH
  END FOR

  // Return result
  MATCH result
    CASE Ok(msg) → RETURN msg
    CASE Err(e)  →
      RETURN Assistant {
        content: [Text("")],
        stop_reason: Error,
        model: config.model,
        provider: "unknown",
        usage: default,
        error_message: Some(e.to_string())
      }
  END MATCH

END FUNCTION
```

---
