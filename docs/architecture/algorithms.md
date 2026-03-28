# phi-core — Design & Algorithms

## 1. Pseudocode Conventions

```
CONVENTIONS:
- FUNCTION name(param: Type) -> ReturnType
- IF / ELSE IF / ELSE / END IF
- FOR EACH item IN collection / END FOR
- WHILE condition / END WHILE
- RETURN value
- MATCH value / CASE pattern → action / END MATCH
- EMIT event(payload)            // send to async event channel
- AWAIT async_operation()        // async call
- LOCK(mutex) / UNLOCK(mutex)    // mutex operations
- SPAWN task                     // launch async task concurrently
- AWAIT_ALL(tasks)               // wait for all concurrent tasks
- SELECT { branch1, branch2 }    // race concurrent futures, take first
- ERROR "message"                // terminal failure path
- [invariant: condition]         // documents what must always be true
- [AMBIGUOUS: reason]            // underdetermined behavior
- → Type                         // return type annotation
- //                             // comment
```

---

## 2. Core Algorithm Catalogue

---

### `agent_loop` *(src/agent_loop/)*

**Purpose:** Start a fresh agent run from new prompt messages.
**Preconditions:** `prompts` is non-empty; `context.messages` may contain prior history.
**Postconditions:** All input filters have run; `AgentStart`/`AgentEnd` are emitted; returns all new messages produced.

```
FUNCTION agent_loop(
  prompts: Vec<AgentMessage>,
  context: AgentContext,         // mutable
  config: AgentLoopConfig,
  tx: EventChannel<AgentEvent>,
  cancel: CancellationToken
) -> Vec<AgentMessage>

  // ── before_loop hook ────────────────────────────────────────────────────
  // Fires before AgentStart. Return false to abort before the loop begins.
  IF config.before_loop defined AND NOT before_loop(context.messages, 0) THEN
    EMIT AgentEnd(messages=[])
    RETURN []
  END IF

  // ── Identity write-back ──────────────────────────────────────────────────
  // agent_id / session_id are set by Agent::prompt_*. Direct callers may leave
  // them None; agent_loop generates and writes them back so that a subsequent
  // agent_loop_continue on the same context can inherit them without extra setup.
  IF context.agent_id is None THEN context.agent_id ← new_uuid() END IF
  IF context.session_id is None THEN context.session_id ← new_uuid() END IF
  IF context.loop_id is None THEN context.loop_id ← new_uuid() END IF

  EMIT AgentStart {
    agent_id:          context.agent_id,
    session_id:        context.session_id,
    loop_id:           context.loop_id,
    parent_loop_id:    None,    // None = origin call
    continuation_kind: None,    // None = origin call
    timestamp:         now()
  }

  // ── Input filtering ─────────────────────────────────────────────────────
  IF config.input_filters is non-empty THEN
    user_text ← JOIN all text from User messages in prompts

    warnings ← []
    FOR EACH filter IN config.input_filters
      MATCH filter.filter(user_text)
        CASE Pass     → continue
        CASE Warn(w)  → warnings.append(w)
        CASE Reject(reason) →
          EMIT InputRejected(reason)
          EMIT AgentEnd(messages=[])
          RETURN []
      END MATCH
    END FOR

    IF warnings is non-empty THEN
      warning_text ← JOIN ["[Warning: " + w + "]" FOR w IN warnings]
      // Append to last User message's content
      append Content::Text(warning_text) to last User message in prompts
    END IF
  END IF

  // ── Append prompts to context ────────────────────────────────────────────
  FOR EACH prompt IN prompts
    context.messages.append(prompt)
  END FOR

  new_messages ← copy of prompts

  EMIT TurnStart

  // Emit events for each incoming prompt
  FOR EACH prompt IN prompts
    EMIT MessageStart(prompt)
    EMIT MessageEnd(prompt)
  END FOR

  // Run the main loop
  loop_usage ← run_loop(context, new_messages, config, tx, cancel)

  EMIT AgentEnd(new_messages)

  // ── after_loop hook ──────────────────────────────────────────────────────
  // Fires after AgentEnd with the messages produced and accumulated usage.
  IF config.after_loop defined THEN after_loop(new_messages, loop_usage) END IF

  RETURN new_messages

END FUNCTION
```

---

### `agent_loop_continue` *(src/agent_loop/)*

**Purpose:** Resume an agent run from existing context (no new prompts, continue from last user/tool-result message).
**Preconditions:** `context.messages` is non-empty; last message is NOT an assistant message; `context.agent_id` and `context.session_id` are `Some`.
**Postconditions:** Same as `agent_loop`.

```
FUNCTION agent_loop_continue(
  context: AgentContext,         // mutable
  config: AgentLoopConfig,
  tx: EventChannel<AgentEvent>,
  cancel: CancellationToken
) -> Vec<AgentMessage>

  [invariant: context.messages is non-empty]
  [invariant: context.messages.last().role != "assistant"]
  // Identity must carry over from the originating loop.
  // These are set by Agent::continue_loop_with_sender (or the direct caller who
  // bootstrapped the session). Silent UUID generation here would break traceability.
  [invariant: context.agent_id is Some]
  [invariant: context.session_id is Some]

  new_messages ← []

  // ── before_loop hook ────────────────────────────────────────────────────
  IF config.before_loop defined AND NOT before_loop(context.messages, 0) THEN
    EMIT AgentEnd(messages=[])
    RETURN []
  END IF

  EMIT AgentStart {
    agent_id:          context.agent_id.unwrap(),
    session_id:        context.session_id.unwrap(),
    loop_id:           context.loop_id OR new_uuid(),
    parent_loop_id:    context.parent_loop_id,    // None for Default, Some for Rerun/Branch
    continuation_kind: context.continuation_kind,  // Some(Default|Rerun|Branch)
    timestamp:         now()
  }

  loop_usage ← run_loop(context, new_messages, config, tx, cancel)

  EMIT AgentEnd(new_messages)

  // ── after_loop hook ──────────────────────────────────────────────────────
  IF config.after_loop defined THEN after_loop(new_messages, loop_usage) END IF

  RETURN new_messages

END FUNCTION
```

---

### `run_loop` *(src/agent_loop/)*

**Purpose:** The shared inner logic for both `agent_loop` and `agent_loop_continue`. Handles the outer follow-up loop and the inner turn-by-tool loop.
**Preconditions:** Context contains at least one user message.
**Postconditions:** `new_messages` contains all messages produced; loop has exited cleanly or on limit/cancel/error.

```
FUNCTION run_loop(
  context: AgentContext,         // mutable
  new_messages: Vec<AgentMessage>,  // mutable accumulator
  config: AgentLoopConfig,
  tx: EventChannel<AgentEvent>,
  cancel: CancellationToken
) -> Usage  // accumulated usage across all turns

  first_turn ← true
  turn_number ← 0
  loop_usage ← Usage.default()
  tracker ← ExecutionTracker.new(config.execution_limits)  // optional

  // Drain any pending steering messages before starting
  pending ← config.get_steering_messages()  // or []

  // ── Outer loop: re-enters if follow-up messages arrive ──────────────────
  WHILE true
    IF cancel.is_cancelled THEN RETURN loop_usage END IF

    steering_after_tools ← null

    // ── Inner loop: runs once per turn (LLM call + tools) ─────────────────
    WHILE true
      IF cancel.is_cancelled THEN RETURN loop_usage END IF

      // Determine TurnTrigger for TurnStart event.
      // Priority on the first turn:
      //   1. Branch continuation   → TurnTrigger::Branch   (explicit branch signal)
      //   2. Any other continuation (Default/Rerun) → TurnTrigger::FollowUp
      //      (the continuation itself is the follow-up, not a fresh user turn)
      //   3. Origin call (continuation_kind == None) → config.first_turn_trigger
      //      (User for Agent::prompt, SubAgent for sub-agent callers)
      // Subsequent turns always use TurnTrigger::FollowUp.
      IF first_turn THEN
        turn_trigger ←
          IF context.continuation_kind == Branch(..) THEN TurnTrigger::Branch
          ELSE IF context.continuation_kind is Some  THEN TurnTrigger::FollowUp
          ELSE config.first_turn_trigger
        first_turn ← false
      ELSE
        turn_trigger ← TurnTrigger::FollowUp
      END IF

      EMIT TurnStart { turn_index: turn_number, triggered_by: turn_trigger }

      // Inject any pending (steering/follow-up) messages
      FOR EACH msg IN pending
        EMIT MessageStart(msg)
        EMIT MessageEnd(msg)
        context.messages.append(msg)
        new_messages.append(msg)
      END FOR
      pending ← []

      // Check execution limits
      IF tracker.check_limits() is Some(reason) THEN
        limit_msg ← User message "[Agent stopped: {reason}]"
        EMIT MessageStart(limit_msg)
        EMIT MessageEnd(limit_msg)
        context.messages.append(limit_msg)
        new_messages.append(limit_msg)
        RETURN loop_usage
      END IF

      // Before-turn callback — abort if returns false
      IF config.before_turn is defined THEN
        IF NOT config.before_turn(context.messages, turn_number) THEN
          RETURN loop_usage
        END IF
      END IF
      turn_number ← turn_number + 1

      // Compact context if configured
      IF config.context_config is defined THEN
        strategy ← config.compaction_strategy OR DefaultCompaction
        context.messages ← strategy.compact(context.messages, config.context_config)
      END IF

      // ── LLM call ────────────────────────────────────────────────────────
      message ← AWAIT stream_assistant_response(context, config, tx, cancel)

      agent_msg ← message as AgentMessage
      context.messages.append(agent_msg)
      new_messages.append(agent_msg)

      // Accumulate usage for after_loop hook
      loop_usage ← loop_usage + message.usage

      // Handle error/abort stop reasons
      IF message.stop_reason == Error OR message.stop_reason == Aborted THEN
        IF message.stop_reason == Error AND config.on_error is defined THEN
          config.on_error(message.error_message OR "Unknown error")
        END IF
        IF config.after_turn is defined THEN
          config.after_turn(context.messages, message.usage)
        END IF
        EMIT TurnEnd(agent_msg, tool_results=[])
        RETURN loop_usage
      END IF

      // Extract tool calls from assistant content
      tool_calls ← [
        (id, name, arguments)
        FOR EACH content IN message.content
        IF content is ToolCall
      ]

      tool_results ← []

      IF tool_calls is non-empty THEN
        execution ← AWAIT execute_tool_calls(
          context.tools, tool_calls, tx, cancel,
          config.get_steering_messages, config.tool_execution
        )
        tool_results ← execution.tool_results
        steering_after_tools ← execution.steering_messages

        FOR EACH result IN tool_results
          am ← result as AgentMessage
          context.messages.append(am)
          new_messages.append(am)
        END FOR
      END IF

      // Record turn for limit tracking
      tracker.record_turn(message.usage.input + message.usage.output)

      // After-turn callback
      IF config.after_turn is defined THEN
        config.after_turn(context.messages, message.usage)
      END IF

      EMIT TurnEnd(agent_msg, tool_results)

      // Check for steering that arrived during tool execution
      IF steering_after_tools is non-empty THEN
        pending ← steering_after_tools
        CONTINUE inner loop
      END IF

      pending ← config.get_steering_messages()

      // Exit inner loop if no tool calls and no pending messages
      IF tool_calls is empty AND pending is empty THEN
        BREAK inner loop
      END IF

    END WHILE  // inner loop

    // Check for follow-up work
    follow_ups ← config.get_follow_up_messages()
    IF follow_ups is non-empty THEN
      pending ← follow_ups
      CONTINUE outer loop
    END IF

    BREAK outer loop

  END WHILE  // outer loop

  RETURN loop_usage

END FUNCTION
```

---

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

  // Apply optional context transform (e.g. for custom preprocessing)
  messages ← IF config.transform_context defined
              THEN config.transform_context(context.messages)
              ELSE context.messages

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

### `delay_for_attempt` *(src/provider/retry.rs)*

**Purpose:** Compute the sleep duration before a retry attempt using exponential backoff with jitter.

```
FUNCTION delay_for_attempt(config: RetryConfig, attempt: usize) -> Duration
  // attempt is 1-indexed
  base_ms ← config.initial_delay_ms * (config.backoff_multiplier ^ (attempt - 1))
  capped_ms ← min(base_ms, config.max_delay_ms)

  // ±20% uniform jitter: multiply by random value in [0.8, 1.2]
  jitter ← 0.8 + random_float_0_to_1() * 0.4
  delay_ms ← floor(capped_ms * jitter)

  RETURN Duration::from_ms(delay_ms)

  // Examples with defaults (initial=1000ms, multiplier=2.0, max=30000ms):
  //   attempt 1 → base=1000ms  → ~800–1200ms
  //   attempt 2 → base=2000ms  → ~1600–2400ms
  //   attempt 3 → base=4000ms  → ~3200–4800ms
END FUNCTION
```

---

### `SubAgentTool::execute` *(src/agents/sub_agent.rs)*

**Purpose:** Delegate a task to an isolated child agent loop, return its final text as a `ToolResult`.
**Preconditions:** `params.task` is a non-empty string.
**Postconditions:** Returns final assistant text from the child run; child context is discarded.

```
FUNCTION SubAgentTool::execute(
  params: JSON,
  ctx: ToolContext
) -> Result<ToolResult, ToolError>

  task ← params["task"] as String  // ERROR "Missing required 'task' parameter" if absent
  cancel ← ctx.cancel
  on_update ← ctx.on_update
  on_progress ← ctx.on_progress

  // Build fresh child context (no history carried over)
  child_context ← AgentContext {
    system_prompt: self.system_prompt,
    messages: [],              // isolated — starts empty
    tools: self.tools          // child has its own toolset (no SubAgentTool instances)
  }

  child_config ← AgentLoopConfig {
    provider: self.provider,
    model: self.model,
    api_key: self.api_key,
    thinking_level: self.thinking_level,
    max_tokens: self.max_tokens,
    execution_limits: {
      max_turns: self.max_turns,       // primary guard (default: 10)
      max_total_tokens: 1_000_000,     // generous fallback
      max_duration: 300s               // generous fallback
    },
    // No steering, no follow-ups, no input filters in sub-agents
    get_steering_messages: null,
    get_follow_up_messages: null,
    input_filters: [],
    ...other config from self
  }

  (event_tx, event_rx) ← new unbounded channel

  // Forward events to parent if callbacks are present
  IF on_update defined OR on_progress defined THEN
    forwarder ← SPAWN async task:
      WHILE event ← event_rx.recv()
        IF event is ProgressMessage { text } THEN
          on_progress(text)  // if defined
        END IF
        IF event is MessageUpdate { delta: Text(delta) } THEN
          on_update(ToolResult{ content: [Text(delta)] })
        END IF
        IF event is ToolExecutionStart { tool_name } THEN
          on_update(ToolResult{ content: [Text("[sub-agent calling tool: {tool_name}]")] })
        END IF
      END WHILE
  END IF

  prompt_msg ← AgentMessage::Llm(Message::User(task))
  new_messages ← AWAIT agent_loop([prompt_msg], child_context, child_config, event_tx, cancel)

  IF forwarder defined THEN AWAIT forwarder END IF

  // Extract final assistant text
  result_text ← extract_final_text(new_messages)

  RETURN Ok(ToolResult {
    content: [Text(result_text)],
    details: { sub_agent: self.tool_name, turns: new_messages.count() }
  })

END FUNCTION

FUNCTION extract_final_text(messages: Vec<AgentMessage>) -> String
  FOR EACH msg IN REVERSE(messages)
    IF msg is Assistant THEN
      texts ← [t FOR t IN msg.content IF t is Text]
      IF texts non-empty THEN
        RETURN JOIN(texts)
      END IF
    END IF
  END FOR
  RETURN "(sub-agent produced no text output)"
END FUNCTION
```

---

### `BashTool::execute` *(src/tools/bash.rs)*

**Purpose:** Execute a shell command, capture output, enforce safety.
**Preconditions:** `params.command` is present.
**Postconditions:** Returns `Ok(ToolResult)` even for non-zero exit codes (LLM needs the error to self-correct).

```
FUNCTION BashTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  command ← params["command"] as String  // InvalidArgs if missing
  cancel ← ctx.cancel

  // Safety: check deny patterns (substring match)
  FOR EACH pattern IN self.deny_patterns
    IF command contains pattern THEN
      RETURN Err(Failed("Command blocked by safety policy: contains '{pattern}'"))
    END IF
  END FOR

  // Optional confirmation callback
  IF self.confirm_fn defined AND NOT self.confirm_fn(command) THEN
    RETURN Err(Failed("Command was not confirmed by the user."))
  END IF

  // Build subprocess: bash -c "{command}"
  cmd ← Command("bash", ["-c", command])
  IF self.cwd defined THEN cmd.current_dir(self.cwd) END IF
  cmd.stdout(piped), cmd.stderr(piped)

  // Race: cancellation vs timeout vs command completion
  result ← SELECT {
    cancel.cancelled()          → RETURN Err(Cancelled)
    sleep(self.timeout)         → RETURN Err(Failed("Command timed out after {N}s"))
    cmd.output()                → result  // may be Err if spawn failed
  }

  output ← result  // Err(io) → Err(Failed("Failed to execute: {e}"))

  stdout ← output.stdout as utf8 (lossy)
  stderr ← output.stderr as utf8 (lossy)

  // Truncate at limit
  IF stdout.len > self.max_output_bytes THEN
    stdout ← stdout[0..max_output_bytes] + "\n... (output truncated)"
  END IF
  IF stderr.len > self.max_output_bytes THEN
    stderr ← stderr[0..max_output_bytes] + "\n... (output truncated)"
  END IF

  exit_code ← output.exit_code OR -1

  text ←
    IF stderr is empty THEN
      "Exit code: {exit_code}\n{stdout}"
    ELSE
      "Exit code: {exit_code}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
    END IF

  // Always Ok — non-zero exit is NOT a ToolError
  RETURN Ok(ToolResult {
    content: [Text(text)],
    details: { exit_code, success: exit_code == 0 }
  })

END FUNCTION
```

---

### `ReadFileTool::execute` *(src/tools/file.rs)*

**Purpose:** Read a file's contents. Routes to binary (image) or text path based on extension.

```
FUNCTION ReadFileTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  path ← params["path"] as String  // InvalidArgs if missing

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  metadata ← AWAIT fs.metadata(path)  // Err → Failed("Cannot access {path}: {e}")

  IF is_image_extension(path) THEN
    // ── Image path ────────────────────────────────────────────────────────
    IF metadata.size > 20MB THEN
      RETURN Err(Failed("Image too large"))
    END IF
    bytes ← AWAIT fs.read(path)
    data ← base64_encode(bytes)
    mime_type ← get_mime_type(path)
    RETURN Ok(ToolResult {
      content: [Image { data, mime_type }],
      details: { path, bytes: bytes.len() }
    })
  END IF

  // ── Text path ─────────────────────────────────────────────────────────
  IF metadata.size > self.max_bytes THEN
    RETURN Err(Failed("File too large. Use offset/limit for partial reads."))
  END IF

  content ← AWAIT fs.read_to_string(path)
  lines ← content.split_lines()
  total ← lines.count()

  offset ← params["offset"] as usize (1-indexed)  // optional, default: 1
  limit  ← params["limit"]  as usize               // optional, default: all

  (start, end) ← compute_range(offset, limit, total)

  // Line-numbered output: "   1 | first line"
  numbered ← ["{start+i+1:>4} | {line}" FOR (i, line) IN enumerate(lines[start..end])]

  header ←
    IF start > 0 OR end < total THEN "[Lines {start+1}-{end} of {total}]"
    ELSE "[{total} lines]"

  RETURN Ok(ToolResult {
    content: [Text("{header}\n{numbered.join('\n')}")],
    details: { path }
  })

END FUNCTION
```

---

### `EditFileTool::execute` *(src/tools/edit.rs)*

**Purpose:** Make a surgical search-and-replace edit in an existing file.
**Preconditions:** File exists; `old_text` occurs exactly once in the file.
**Postconditions:** File on disk has exactly the one occurrence of `old_text` replaced by `new_text`.

```
FUNCTION EditFileTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  path     ← params["path"]     as String  // InvalidArgs if missing
  old_text ← params["old_text"] as String  // InvalidArgs if missing
  new_text ← params["new_text"] as String  // InvalidArgs if missing

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  content ← AWAIT fs.read_to_string(path)
  // Err → Failed("Cannot read {path}. Use write_file to create new files.")

  match_count ← count of occurrences of old_text in content

  IF match_count == 0 THEN
    // Provide helpful fuzzy hint
    hint ← find_similar_text(content, old_text)
    IF hint defined THEN
      message ← "old_text not found in {path}.\n\nDid you mean:\n```\n{hint}\n```\n..."
    ELSE
      message ← "old_text not found in {path}.\n\nTip: Use read_file to see contents..."
    END IF
    RETURN Err(Failed(message))
  END IF

  IF match_count > 1 THEN
    RETURN Err(Failed(
      "old_text matches {match_count} locations. Include more context to make match unique."
    ))
  END IF

  // Replace exactly the first (and only) occurrence
  new_content ← content.replace_once(old_text, new_text)
  AWAIT fs.write(path, new_content)

  old_lines ← old_text.line_count()
  new_lines ← new_text.line_count()

  RETURN Ok(ToolResult {
    content: [Text("Replaced {old_lines} line(s) with {new_lines} line(s) in {path}")],
    details: { path, old_lines, new_lines }
  })

END FUNCTION

FUNCTION find_similar_text(content: String, target: String) -> Option<String>
  // Fuzzy hint: find the first line of target in the file
  target_trimmed ← target.trim()
  first_line ← target_trimmed.first_line().trim()
  IF first_line is empty THEN RETURN None END IF

  lines ← content.split_lines()
  FOR EACH (i, line) IN enumerate(lines)
    IF line contains first_line THEN
      end ← min(i + target_trimmed.line_count() + 1, lines.count())
      RETURN Some(lines[i..end].join("\n"))
    END IF
  END FOR

  RETURN None
END FUNCTION
```

---

### `SkillSet::format_for_prompt` *(src/context/skills.rs)*

**Purpose:** Format all loaded skills as an XML index for injection into the system prompt.
**Standard:** Conforms to the AgentSkills open standard (agentskills.io/integrate-skills).

```
FUNCTION SkillSet::format_for_prompt() -> String

  IF self.skills is empty THEN RETURN "" END IF

  // Skills are sorted by name ascending
  sorted_skills ← sort(self.skills, by: skill.name)

  out ← "<available_skills>\n"

  FOR EACH skill IN sorted_skills
    out += "  <skill>\n"
    out += "    <name>"        + xml_escape(skill.name)                      + "</name>\n"
    out += "    <description>" + xml_escape(skill.description)               + "</description>\n"
    out += "    <location>"    + xml_escape(skill.file_path.to_string())     + "</location>\n"
    out += "  </skill>\n"
  END FOR

  out += "</available_skills>"
  RETURN out

  // xml_escape replaces: & → &amp;  < → &lt;  > → &gt;  " → &quot;  ' → &apos;

END FUNCTION

// Example output:
// <available_skills>
//   <skill>
//     <name>weather</name>
//     <description>Get current weather and forecasts.</description>
//     <location>/home/user/.skills/weather/SKILL.md</location>
//   </skill>
// </available_skills>
```

### `SkillSet::load` *(src/context/skills.rs)*

**Purpose:** Load skills from one or more directories. Later directories override earlier ones on name collision.

```
FUNCTION SkillSet::load(dirs: Vec<Path>) -> Result<SkillSet, SkillError>

  skill_map ← HashMap<String, Skill>  // key = skill name

  FOR EACH (index, dir) IN enumerate(dirs)
    IF dir does not exist THEN
      CONTINUE  // silently skip missing directories
    END IF

    source_label ← "dir:{index}"

    FOR EACH entry IN list_subdirectories(dir)
      skill_md_path ← entry.path / "SKILL.md"
      IF skill_md_path does not exist THEN
        CONTINUE
      END IF

      content ← read_to_string(skill_md_path)
      (name, description) ← parse_frontmatter(content)
      // Returns SkillError::InvalidFrontmatter or SkillError::MissingField on failure

      base_dir ← canonicalize(entry.path)
      file_path ← base_dir / "SKILL.md"

      skill ← Skill { name, description, file_path, base_dir, source: source_label }
      skill_map[name] ← skill  // later dirs OVERRIDE earlier on name collision
    END FOR
  END FOR

  skills ← sort(skill_map.values(), by: skill.name)
  RETURN Ok(SkillSet { skills })

END FUNCTION

FUNCTION parse_frontmatter(content: String) -> Result<(name, description), SkillError>
  // Content must start with "---"
  IF NOT content.trim_start().starts_with("---") THEN
    RETURN Err(InvalidFrontmatter)
  END IF

  // Find closing "---"
  yaml_block ← content between first "---" and next "\n---"
  IF no closing delimiter THEN
    RETURN Err(InvalidFrontmatter)
  END IF

  name ← ""
  description ← ""

  FOR EACH line IN yaml_block.lines()
    IF line.starts_with("name:") THEN
      name ← unquote(line.after("name:").trim())
    ELSE IF line.starts_with("description:") THEN
      description ← unquote(line.after("description:").trim())
    END IF
    // All other YAML fields silently ignored
  END FOR

  IF name is empty THEN RETURN Err(MissingField("name")) END IF
  IF description is empty THEN RETURN Err(MissingField("description")) END IF

  RETURN Ok((name, description))

  // unquote(): strips surrounding single or double quotes if present

END FUNCTION
```

---

### `ProviderError::classify` *(src/provider/traits.rs)*

**Purpose:** Map an HTTP error response to the correct `ProviderError` variant.

```
FUNCTION ProviderError::classify(status: u16, message: String) -> ProviderError

  IF is_context_overflow(status, message) THEN
    RETURN ContextOverflow { message }
  END IF

  IF status == 429 THEN
    RETURN RateLimited { retry_after_ms: None }
  END IF

  IF status == 401 OR status == 403 THEN
    RETURN Auth(message)
  END IF

  RETURN Api(message)

END FUNCTION

FUNCTION is_context_overflow(status: u16, message: String) -> bool
  // Some providers (Cerebras, Mistral) return 400/413 with empty body
  IF (status == 400 OR status == 413) AND message.trim() is empty THEN
    RETURN true
  END IF
  lower ← message.to_lowercase()
  RETURN any of OVERFLOW_PHRASES is a substring of lower

  // OVERFLOW_PHRASES includes:
  //   "prompt is too long"          (Anthropic)
  //   "input is too long"           (Bedrock)
  //   "exceeds the context window"  (OpenAI)
  //   "exceeds the maximum"         (Google)
  //   "maximum prompt length"       (xAI)
  //   "reduce the length of the messages" (Groq)
  //   "maximum context length"      (OpenRouter)
  //   "context length exceeded"     (generic)
  //   "too many tokens"             (generic)
  //   ... 15 phrases total

END FUNCTION
```

---

## 3. Initialization & Lifecycle Sequences

### Agent Construction (Builder Pattern)

```
SEQUENCE AgentConstruction
  1. BasicAgent::new(model_config: ModelConfig)
     - Stores model_config (provider identity: id, api_key, base_url, api protocol, cost rates)
     - Initializes messages = []
     - Initializes tools = []
     - Sets defaults: thinking = Off, tool_execution = Parallel, retry = default

  2. .with_system_prompt(text)
     - Stores system_prompt string

  3. .with_tools(vec)
     - Replaces or extends the tools list

  5. .with_context_config(config)
     - Enables automatic compaction before each turn

  6. .with_execution_limits(limits)
     - Enables turn/token/duration caps

  7. .with_skills(skill_set)
     - Appends skill XML index to system_prompt

  8. .with_mcp_server_stdio(cmd, args, env)     [async]
     - Spawns MCP subprocess
     - Calls initialize + tools/list over JSON-RPC
     - Wraps each discovered tool as McpToolAdapter (implements AgentTool)
     - Appends adapters to tools list

  9. .with_openapi_file/url/spec(...)           [async, feature-gated]
     - Parses OpenAPI spec
     - Generates one OpenApiToolAdapter per matching operation
     - Appends adapters to tools list

  10. Callbacks: .on_before_turn(f), .on_after_turn(f), .on_error(f)
      - Stores function pointers; called at appropriate points in run_loop

  11. .with_input_filter(filter)
      - Appends to input_filters list

  12. .with_compaction_strategy(strategy)
      - Overrides default DefaultCompaction with custom implementation

END SEQUENCE
```

### Agent Run Lifecycle

```
SEQUENCE AgentRun (invoked by agent.prompt("..."))
  1. Acquire run lock (ensure not already streaming)
     - is_streaming ← true
     - Create new CancellationToken

  2. Build AgentContext from current Agent state
     - Snapshot: system_prompt, messages (copy), tools

  3. Build AgentLoopConfig from current Agent config
     - Wire get_steering_messages → drain steering_queue
     - Wire get_follow_up_messages → drain follow_up_queue

  4. Create event channel (tx, rx)

  5. SPAWN async task: agent_loop(prompts, context, config, tx, cancel)

  6. Return rx to caller immediately (non-blocking)
     - Caller consumes events: AgentStart, TurnStart/End, MessageStart/Update/End,
       ToolExecutionStart/Update/End, ProgressMessage, AgentEnd

  7. When AgentEnd received or channel closes:
     - Merge new_messages into Agent.messages
     - is_streaming ← false
     - CancellationToken dropped

END SEQUENCE
```

### Abort Lifecycle

```
SEQUENCE AgentAbort (invoked by agent.abort())
  1. IF cancel token exists THEN
       cancel.cancel()  // signals all child tokens
  2. Agent loop checks cancel.is_cancelled() at:
     - Start of each outer/inner loop iteration
     - In BashTool's tokio::select! race
     - In ReadFileTool/WriteFileTool/EditFileTool before each I/O op
  3. Loop exits cleanly at next check point; AgentEnd NOT emitted on abort
     [AMBIGUOUS: AgentEnd may or may not be emitted depending on where
      in the loop cancellation is detected — Start/Done events from provider
      may still arrive before cancellation is noticed]
END SEQUENCE
```

### Message Persistence

```
SEQUENCE MessagePersistence
  Save:
    1. agent.save_messages() → serde_json::to_string(agent.messages)
    2. Caller writes JSON string to disk/storage

  Restore:
    1. Caller reads JSON string from disk/storage
    2. agent.restore_messages(json_str) → serde_json::from_str(json_str) → Vec<AgentMessage>
    3. Agent.messages ← deserialized messages
    4. Next agent.prompt() continues from restored history

  All types in AgentMessage tree derive Serialize + Deserialize.
  JSON format: array of untagged AgentMessage items;
    Llm variant: has "role" field ("user", "assistant", "toolResult")
    Extension variant: has "role" field "extension" + "kind" + "data"
END SEQUENCE
```

---

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

## 5. Concurrency & Async Patterns

### Parallel Tool Execution

```
PATTERN ParallelToolExecution
  // When Parallel strategy is used, all tool calls race concurrently.
  // This is safe because:
  //   1. Tools share no mutable state (each has its own ToolContext)
  //   2. Each ToolContext gets a child cancellation token (same lineage, independent trigger)
  //   3. The event channel (tx) is cloned into each ToolContext — Unbounded sends never block
  //   4. Results are collected in original order via join_all (preserves tool_call ordering)

  futures ← [execute_single_tool(id, name, args) FOR EACH (id, name, args) IN tool_calls]
  results ← AWAIT_ALL(futures)   // futures::join_all — waits for ALL, order preserved
  // Steering is checked AFTER all complete (cannot interrupt mid-batch in Parallel mode)

PATTERN SequentialToolExecution
  // Tools run one at a time; steering is checked after each.
  // Use when tools access shared resources (e.g., same file, same database row).

PATTERN BatchedToolExecution
  // Groups of N run in parallel; steering checked between groups.
  // Balances latency (N concurrent) with control (interrupt between groups).
```

### Cancellation Token Propagation

```
PATTERN CancellationPropagation
  // CancellationToken forms a tree. Cancelling a parent cancels all children.

  Agent.cancel (root token)
    └── AgentLoop cancel (same token passed in)
          └── ToolContext.cancel (child_token() — inherits from parent)
                └── SubAgentTool: forwards parent cancel to child agent_loop()

  // Checks occur at:
  //   - Top of each loop iteration in run_loop (fast path)
  //   - tokio::select! in BashTool (races against timeout)
  //   - Explicit is_cancelled() checks in ReadFileTool, WriteFileTool, EditFileTool

  // Important: abort() on Agent cancels ALL in-progress tool calls simultaneously,
  // regardless of execution strategy.
```

### Event Channel Architecture

```
PATTERN EventChannelArchitecture
  // Single producer (AgentLoop), single consumer (caller).
  // Channel: tokio::mpsc::unbounded_channel — never blocks sender.

  AgentLoop ──tx──→ UnboundedChannel ──rx──→ Application

  // Sub-agent events are NOT directly forwarded to parent channel.
  // SubAgentTool spawns a separate task to translate sub-agent events:
  //   AgentEvent::MessageUpdate(Text(delta)) → on_update(ToolResult{text:delta})
  //   AgentEvent::ProgressMessage{text}      → on_progress(text)
  // These are then emitted to the parent channel as ToolExecutionUpdate/ProgressMessage.

  // This means: parent sees sub-agent activity but via ToolExecutionUpdate wrappers,
  // NOT as nested AgentStart/AgentEnd/TurnStart/TurnEnd events.
```

### Steering Queue Thread Safety

```
PATTERN SteeringQueueSafety
  // steering_queue and follow_up_queue are Arc<Mutex<Vec<AgentMessage>>>.

  // Write path (application thread):
  //   agent.steer(msg)     → LOCK(queue), queue.push(msg), UNLOCK
  //   agent.follow_up(msg) → LOCK(follow_up_queue), queue.push(msg), UNLOCK

  // Read path (agent loop task) — behavior depends on QueueMode:
  //   QueueMode::OneAtATime (default):
  //     LOCK(queue), msg = queue.remove(0), UNLOCK, return [msg]
  //     → delivers exactly one message per check; rest remain for next check
  //   QueueMode::All:
  //     LOCK(queue), msgs = queue.drain_all(), UNLOCK, return msgs
  //     → delivers everything at once

  // Read is called only between tool executions — never concurrently with another read.
  // No deadlock risk: lock is held for microseconds (no I/O inside lock).
  // No data race: Mutex guarantees exclusive access.

  // Queues are passed to AgentLoopConfig as closures capturing the Arc pointer,
  // so the external caller can enqueue messages from any thread at any time.
```

---

## 6. Additional Algorithms

---

### `BasicAgent::new` and `BasicAgent::prompt` *(src/agents/basic_agent.rs)*

**Purpose:** Construct a BasicAgent and start a run. These are the primary application-facing entry points.

```
FUNCTION BasicAgent::new(model_config: ModelConfig) -> BasicAgent
  RETURN BasicAgent {
    model_config: model_config,       // complete provider identity: id, api_key, base_url, api, cost
    system_prompt: "",
    thinking_level: Off,
    max_tokens: None,
    temperature: None,
    messages: [],
    tools: [],
    steering_queue: Arc(Mutex([])),
    follow_up_queue: Arc(Mutex([])),
    steering_mode: QueueMode::OneAtATime,
    follow_up_mode: QueueMode::OneAtATime,
    context_config: Some(ContextConfig::default()),
    execution_limits: Some(ExecutionLimits::default()),
    cache_config: CacheConfig::default(),
    tool_execution: Parallel,
    retry_config: RetryConfig::default(),
    before_turn: None,
    after_turn: None,
    on_error: None,
    input_filters: [],
    compaction_strategy: None,
    cancel: None,
    is_streaming: false
  }
END FUNCTION

FUNCTION Agent::prompt(text: String) -> UnboundedReceiver<AgentEvent>
  RETURN Agent::prompt_messages([AgentMessage::Llm(Message::user(text))])
END FUNCTION

FUNCTION Agent::prompt_messages(messages: Vec<AgentMessage>) -> UnboundedReceiver<AgentEvent>
  (tx, rx) ← new unbounded channel
  SPAWN Agent::prompt_messages_with_sender(messages, tx)
  RETURN rx
END FUNCTION

FUNCTION Agent::prompt_messages_with_sender(
  messages: Vec<AgentMessage>,
  tx: EventSender<AgentEvent>
) [async]

  // Guard: panics if already streaming
  ASSERT NOT self.is_streaming,
    "Agent is already streaming. Use steer() or follow_up()."

  self.is_streaming ← true
  self.cancel ← Some(CancellationToken::new())

  // Build context snapshot for this run
  context ← AgentContext {
    system_prompt: self.system_prompt.clone(),
    messages: self.messages.clone(),
    tools: self.tools  // borrowed
  }

  // Wire queue closures — capture Arc pointers
  steering_arc ← Arc::clone(self.steering_queue)
  followup_arc ← Arc::clone(self.follow_up_queue)

  config ← AgentLoopConfig {
    provider: self.provider,
    model: self.model,
    api_key: self.api_key,
    thinking_level: self.thinking_level,
    max_tokens: self.max_tokens,
    temperature: self.temperature,
    model_config: self.model_config,
    get_steering_messages: closure {
      LOCK(steering_arc)
      MATCH self.steering_mode
        CASE OneAtATime → IF queue non-empty THEN [queue.remove(0)] ELSE []
        CASE All        → queue.drain_all()
      UNLOCK
    },
    get_follow_up_messages: closure {
      LOCK(followup_arc)
      MATCH self.follow_up_mode
        CASE OneAtATime → IF queue non-empty THEN [queue.remove(0)] ELSE []
        CASE All        → queue.drain_all()
      UNLOCK
    },
    context_config: self.context_config,
    compaction_strategy: self.compaction_strategy,
    execution_limits: self.execution_limits,
    cache_config: self.cache_config,
    tool_execution: self.tool_execution,
    retry_config: self.retry_config,
    before_turn: self.before_turn,
    after_turn: self.after_turn,
    on_error: self.on_error,
    input_filters: self.input_filters
  }

  new_messages ← AWAIT agent_loop(messages, context, config, tx, self.cancel.unwrap())

  // Merge new messages back into Agent.messages
  self.messages.extend(new_messages)

  self.is_streaming ← false
  self.cancel ← None

END FUNCTION
```

---

### `McpClient::initialize` *(src/mcp/)*

**Purpose:** Perform the 3-step MCP handshake to establish a session with a tool server.

```
FUNCTION McpClient::connect_stdio(
  command: String,
  args: Vec<String>,
  env: Option<Map<String,String>>
) -> Result<McpClient, McpError>

  // Spawn child process
  process ← spawn_process(command, args, env,
    stdin=piped, stdout=piped, stderr=inherit)
  // McpError::Transport on spawn failure

  transport ← StdioTransport { process }
  client ← McpClient { transport: Arc(Mutex(transport)), server_info: None }

  AWAIT client.initialize()
  RETURN Ok(client)

END FUNCTION

FUNCTION McpClient::initialize() -> Result<ServerInfo, McpError>

  // Step 1: send initialize
  result ← AWAIT self.send_request("initialize", {
    protocolVersion: "2024-11-05",
    capabilities: {},
    clientInfo: { name: "phi-core", version: CARGO_PKG_VERSION }
  })
  // Deserialize result as InitializeResult { protocolVersion, capabilities, serverInfo }

  self.server_info ← Some(result.serverInfo)

  // Step 2: send notifications/initialized (no params)
  AWAIT self.send_request("notifications/initialized", None)
  // Server may ignore the response id for this notification

  RETURN Ok(result.serverInfo)

END FUNCTION

FUNCTION McpClient::send_request(method: String, params: Option<Value>) -> Result<Value, McpError>

  request ← JsonRpcRequest {
    jsonrpc: "2.0",
    id: ATOMIC_COUNTER.fetch_add(1),  // monotonically increasing from 1
    method,
    params
  }

  response ← AWAIT self.transport.send(request)

  IF response.error is Some THEN
    RETURN Err(JsonRpc { code: error.code, message: error.message })
  END IF

  IF response.result is None THEN
    RETURN Err(Protocol("Empty result"))
  END IF

  RETURN Ok(response.result)

END FUNCTION

FUNCTION McpClient::list_tools() -> Result<Vec<McpToolInfo>, McpError>
  result ← AWAIT self.send_request("tools/list", {})
  RETURN deserialize result.tools as Vec<McpToolInfo>
END FUNCTION

FUNCTION McpClient::call_tool(name: String, arguments: Value) -> Result<McpToolCallResult, McpError>
  result ← AWAIT self.send_request("tools/call", { name, arguments })
  RETURN deserialize result as McpToolCallResult
END FUNCTION
```

---

### `ListFilesTool::execute` *(src/tools/list.rs)*

**Purpose:** List files in a directory, with optional glob filtering and depth limit.

```
FUNCTION ListFilesTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  path      ← params["path"]      as String  // optional; default: current directory
  pattern   ← params["pattern"]   as String  // optional glob filter, e.g. "*.rs"
  max_depth ← params["max_depth"] as usize   // optional; default: 3

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  // Build `find` command
  cmd ← "find {path} -maxdepth {max_depth} -type f"
  IF pattern defined THEN cmd += " -name '{pattern}'" END IF
  // Excluded paths (prepended to command):
  //   -not -path "*/target/*"
  //   -not -path "*/.git/*"
  //   -not -path "*/node_modules/*"

  SELECT {
    ctx.cancel.cancelled() → RETURN Err(Cancelled)
    sleep(self.timeout)    → RETURN Err(Failed("List timed out"))
    run(cmd)               → output
  }

  lines ← output.stdout.split_lines()

  truncated ← false
  IF lines.count() > self.max_results THEN
    lines ← lines[0..self.max_results]
    truncated ← true
  END IF

  text ← lines.join("\n")
  IF truncated THEN
    text += "\n... (truncated at {self.max_results} results)"
  END IF

  RETURN Ok(ToolResult {
    content: [Text(text)],
    details: { total: lines.count(), truncated }
  })

END FUNCTION
```

**Defaults:** `max_results = 200`, `timeout = 10s`

---

### `SearchTool::execute` *(src/tools/search.rs)*

**Purpose:** Search file contents using regex via ripgrep (preferred) or grep (fallback).

```
FUNCTION SearchTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  pattern        ← params["pattern"]        as String  // required; regex
  path           ← params["path"]           as String  // optional; default: self.root or cwd
  include        ← params["include"]        as String  // optional file glob, e.g. "*.rs"
  case_sensitive ← params["case_sensitive"] as bool    // optional; default: false

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  // Prefer ripgrep (rg) if available, fall back to grep
  IF rg_available() THEN
    cmd ← ["rg", "--line-number", "--no-heading",
            "--max-count={self.max_results}"]
    IF NOT case_sensitive THEN cmd += ["--ignore-case"] END IF
    IF include defined THEN cmd += ["--glob={include}"] END IF
    cmd += [pattern, path]
  ELSE
    cmd ← ["grep", "-r", "-n", "-m{self.max_results}"]
    IF NOT case_sensitive THEN cmd += ["-i"] END IF
    IF include defined THEN cmd += ["--include={include}"] END IF
    cmd += [pattern, path]
  END IF

  SELECT {
    ctx.cancel.cancelled() → RETURN Err(Cancelled)
    sleep(self.timeout)    → RETURN Err(Failed("Search timed out"))
    run(cmd)               → (exit_code, stdout, stderr)
  }

  // Exit code 1 = no matches found (not an error)
  IF exit_code == 1 AND stderr is empty THEN
    stdout ← ""
  END IF
  // Exit code 2+ or non-empty stderr = actual failure
  IF exit_code >= 2 OR (exit_code != 0 AND stderr non-empty) THEN
    RETURN Err(Failed(stderr))
  END IF

  lines ← stdout.split_lines()
  match_count ← lines.count()

  text ← stdout
  IF match_count >= self.max_results THEN
    text += "\n... (truncated at {self.max_results} matches)"
  END IF

  RETURN Ok(ToolResult {
    content: [Text(text)],
    details: { matches: match_count }
  })

END FUNCTION
```

**Defaults:** `max_results = 50`, `timeout = 30s`
**Output format:** `{file}:{line_number}:{matched_line}`

---

### `OpenApiToolAdapter::execute` *(src/openapi/)*

**Purpose:** Execute a single OpenAPI operation as an HTTP request.

```
FUNCTION OpenApiToolAdapter::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  // Normalize params: null → {}; non-object → error
  IF params is null THEN params ← {} END IF
  IF params is NOT object THEN
    RETURN Ok(ToolResult { content: [Text("Error: params must be an object")] })
  END IF

  // ── Step 1: Substitute path parameters ────────────────────────────────────
  url_path ← self.info.path  // e.g. "/users/{userId}/posts/{postId}"
  FOR EACH param_name IN self.info.path_params
    value ← params[param_name]
    IF value is missing THEN
      RETURN Ok(ToolResult { content: [Text("Error: missing required path param '{param_name}'")] })
    END IF
    encoded ← percent_encode_rfc3986(value.to_string())
    url_path ← replace(url_path, "{" + param_name + "}", encoded)
  END FOR

  // ── Step 2: Build base URL ─────────────────────────────────────────────────
  url ← self.base_url + url_path

  // ── Step 3: Build HTTP request ────────────────────────────────────────────
  method ← parse_http_method(self.info.method)  // GET, POST, PUT, etc.
  request ← self.client.request(method, url)

  // Query parameters
  FOR EACH param_name IN self.info.query_params
    IF params[param_name] defined THEN
      request ← request.query(param_name, params[param_name].to_string())
    END IF
  END FOR

  // Header parameters
  FOR EACH param_name IN self.info.header_params
    IF params[param_name] defined THEN
      request ← request.header(param_name, params[param_name].to_string())
    END IF
  END FOR

  // Authentication
  MATCH self.config.auth
    CASE None           → (no-op)
    CASE Bearer(token)  → request ← request.bearer_auth(token)
    CASE ApiKey{header,value} → request ← request.header(header, value)
  END MATCH

  // Custom headers
  FOR EACH (key, value) IN self.config.custom_headers
    request ← request.header(key, value)
  END FOR

  // Request body (application/json only)
  IF self.info.has_body THEN
    body ← params["body"] OR params["_request_body"]
    IF body defined THEN
      request ← request.json(body)
    END IF
  END IF

  // ── Step 4: Send and read response ────────────────────────────────────────
  response ← AWAIT request.send()
  // McpError on network failure → return as content text

  status ← response.status_code()
  body_text ← AWAIT response.text()

  // Truncate at limit (respecting UTF-8 boundaries)
  IF body_text.byte_length() > self.config.max_response_bytes THEN
    body_text ← truncate_utf8(body_text, self.config.max_response_bytes)
  END IF

  result_text ← "{self.info.method} {url} → {status}\n\n{body_text}"

  RETURN Ok(ToolResult {
    content: [Text(result_text)],
    details: { status_code: status, url }
  })

END FUNCTION
```
