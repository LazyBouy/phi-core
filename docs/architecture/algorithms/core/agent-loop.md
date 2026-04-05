<!-- Last verified: 2026-04-05 by Claude Code -->
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

  // ── loop_id generation (must happen before before_loop so AgentEnd can carry it) ──
  IF context.loop_id is None THEN context.loop_id ← new_uuid() END IF

  // ── before_loop hook ────────────────────────────────────────────────────
  // Fires before AgentStart. Return false to abort before the loop begins.
  IF config.before_loop defined AND NOT before_loop(context.messages, 0) THEN
    EMIT AgentEnd(loop_id=context.loop_id, messages=[])
    RETURN []
  END IF

  // ── Identity write-back ──────────────────────────────────────────────────
  // agent_id / session_id are set by Agent::prompt_*. Direct callers may leave
  // them None; agent_loop generates and writes them back so that a subsequent
  // agent_loop_continue on the same context can inherit them without extra setup.
  IF context.agent_id is None THEN context.agent_id ← new_uuid() END IF
  IF context.session_id is None THEN context.session_id ← new_uuid() END IF

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

  // ── Classify existing messages into 2-stream model (if not already populated) ──
  IF context.user_context is empty AND context.inrun_context is empty THEN
    FOR EACH msg IN context.messages
      IF msg is User         → context.user_context.append(msg)
      IF msg is Assistant or ToolResult → context.inrun_context.append(Live(msg))
      // Extension messages go to neither stream
    END FOR
  END IF

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
