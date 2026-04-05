<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `execute_tool_calls` *(src/agent_loop/)*

**Purpose:** Dispatch a list of tool calls using the configured execution strategy.
**Preconditions:** `tool_calls` is non-empty.
**Postconditions:** Returns one `ToolResult` message per input tool call (in order); skipped tools produce error results.

```
FUNCTION execute_tool_calls(
  tools: Vec<AgentTool>,
  tool_calls: [(id, name, args)],
  tx: EventChannel<AgentEvent>,
  cancel: CancellationToken,
  get_steering: optional function,
  strategy: ToolExecutionStrategy
) -> ToolExecutionResult { tool_results, steering_messages }

  MATCH strategy

    CASE Sequential →
      RETURN execute_sequential(tools, tool_calls, tx, cancel, get_steering)

    CASE Parallel →
      RETURN execute_batch(tools, tool_calls, tx, cancel, get_steering)

    CASE Batched { size } →
      results ← []
      steering_messages ← null

      FOR EACH batch IN chunks(tool_calls, size)
        batch_result ← AWAIT execute_batch(tools, batch, tx, cancel, steering=null)
        results.extend(batch_result.tool_results)

        // Check steering between batches
        IF get_steering defined THEN
          steering ← get_steering()
          IF steering is non-empty THEN
            steering_messages ← steering
            // Skip remaining tool calls
            remaining_idx ← (batch_index + 1) * size
            FOR EACH (skip_id, skip_name) IN tool_calls[remaining_idx..]
              results.append(skip_tool_call(skip_id, skip_name, tx))
            END FOR
            BREAK
          END IF
        END IF
      END FOR

      RETURN { tool_results: results, steering_messages }

  END MATCH

END FUNCTION
```

---

### `execute_sequential` *(src/agent_loop/)*

**Purpose:** Execute tool calls one at a time, checking for steering between each.

```
FUNCTION execute_sequential(
  tools, tool_calls, tx, cancel, get_steering
) -> ToolExecutionResult

  results ← []
  steering_messages ← null

  FOR EACH (index, (id, name, args)) IN enumerate(tool_calls)
    (result_msg, _) ← AWAIT execute_single_tool(tools, id, name, args, tx, cancel)
    results.append(result_msg)

    IF get_steering defined THEN
      steering ← get_steering()
      IF steering is non-empty THEN
        steering_messages ← steering
        // Skip remaining tool calls
        FOR EACH (skip_id, skip_name) IN tool_calls[index+1..]
          results.append(skip_tool_call(skip_id, skip_name, tx))
        END FOR
        BREAK
      END IF
    END IF
  END FOR

  RETURN { tool_results: results, steering_messages }

END FUNCTION
```

---

### `execute_batch` *(src/agent_loop/)*

**Purpose:** Execute all tool calls in a batch concurrently, then check for steering.

```
FUNCTION execute_batch(
  tools, tool_calls, tx, cancel, get_steering
) -> ToolExecutionResult

  // Launch all tools concurrently
  futures ← [execute_single_tool(tools, id, name, args, tx, cancel)
             FOR EACH (id, name, args) IN tool_calls]

  batch_results ← AWAIT_ALL(futures)   // wait for all to complete
  results ← [msg FOR (msg, _) IN batch_results]

  // Check steering after all complete
  steering_messages ← null
  IF get_steering defined THEN
    steering ← get_steering()
    IF steering is non-empty THEN
      steering_messages ← steering
    END IF
  END IF

  RETURN { tool_results: results, steering_messages }

END FUNCTION
```

---

### `execute_single_tool` *(src/agent_loop/)*

**Purpose:** Execute one tool call, emitting progress events and returning the result as a `ToolResult` message.

```
FUNCTION execute_single_tool(
  tools: Vec<AgentTool>,
  id: String, name: String, args: JSON,
  tx: EventChannel<AgentEvent>,
  cancel: CancellationToken,
  config: AgentLoopConfig   // for before/after_tool_execution* hooks
) -> (Message::ToolResult, is_error: bool)

  tool ← find tool WHERE tool.name() == name  // may be None

  // ── before_tool_execution hook ───────────────────────────────────────────
  // Return false to skip this tool call entirely.
  IF config.before_tool_execution defined THEN
    IF NOT before_tool_execution(name, id, args) THEN
      // Emit a skipped error result so the LLM knows the call did not run
      skip_result ← ToolResult{ content: [Text("Tool call skipped by before_tool_execution hook")], is_error: true }
      EMIT ToolExecutionEnd(id, name, skip_result, is_error=true, child_loop_id=None)
      msg ← Message::ToolResult{ ..., is_error: true }
      EMIT MessageStart(msg); EMIT MessageEnd(msg)
      RETURN (msg, true)
    END IF
  END IF

  EMIT ToolExecutionStart(tool_call_id=id, tool_name=name, args)

  // Build callbacks for streaming partial results.
  // Each on_update call runs through the before/after_tool_execution_update hooks.
  on_update ← callback(partial: ToolResult):
    // Extract text content for hooks
    text_content ← JOIN text blocks from partial.content
    // before_tool_execution_update — false suppresses the event
    emit ← IF config.before_tool_execution_update defined
               THEN before_tool_execution_update(name, id, text_content)
               ELSE true
    IF emit THEN
      EMIT ToolExecutionUpdate(id, name, partial_result=partial)
      // after_tool_execution_update — fires only when event was not suppressed
      IF config.after_tool_execution_update defined THEN
        after_tool_execution_update(name, id, text_content)
      END IF
    END IF
  on_progress ← callback that EMITS ProgressMessage(id, name, text)

  ctx ← ToolContext {
    tool_call_id: id,
    tool_name: name,
    cancel: cancel.child_token(),  // new child token, same lineage
    on_update: on_update,
    on_progress: on_progress
  }

  (result, is_error) ←
    IF tool found THEN
      MATCH AWAIT tool.execute(args, ctx)
        CASE Ok(r)  → (r, false)
        CASE Err(e) → (ToolResult{ content: [Text(e.to_string())] }, true)
      END MATCH
    ELSE
      (ToolResult{ content: [Text("Tool {name} not found")] }, true)
    END IF

  // child_loop_id is set by SubAgentTool; None for all other tools
  EMIT ToolExecutionEnd(id, name, result, is_error, child_loop_id: result.child_loop_id)

  // ── after_tool_execution hook ────────────────────────────────────────────
  IF config.after_tool_execution defined THEN
    after_tool_execution(name, id, is_error)
  END IF

  msg ← Message::ToolResult {
    tool_call_id: id, tool_name: name,
    content: result.content, is_error, timestamp: now_ms()
  }
  EMIT MessageStart(msg)
  EMIT MessageEnd(msg)

  RETURN (msg, is_error)

END FUNCTION
```

---
