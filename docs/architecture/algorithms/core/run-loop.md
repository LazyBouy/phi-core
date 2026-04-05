<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
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
      // NOTE: context.continuation_kind is Option<ContinuationKind> on AgentContext.
      // None means Initial (first loop); Some(x) means a continuation.
      // The pseudocode below abstracts this as direct ContinuationKind values.
      //
      // Priority on the first turn:
      //   1. Branch continuation   → TurnTrigger::Branch   (explicit branch signal)
      //   2. Any other continuation (Default/Rerun/Compaction) → TurnTrigger::Continuation
      //      (the continuation itself is the follow-up, not a fresh user turn)
      //   3. Initial (origin agent_loop call) → config.first_turn_trigger
      //      (User for Agent::prompt, SubAgent for sub-agent callers)
      // Subsequent turns always use TurnTrigger::Continuation.
      IF first_turn THEN
        turn_trigger ←
          IF context.continuation_kind == Branch(..) THEN TurnTrigger::Branch
          ELSE IF context.continuation_kind != Initial   THEN TurnTrigger::Continuation
          ELSE config.first_turn_trigger
        first_turn ← false
      ELSE
        turn_trigger ← TurnTrigger::Continuation
      END IF

      EMIT TurnStart { turn_index: turn_number, triggered_by: turn_trigger }

      // Inject any pending (steering/follow-up) messages
      FOR EACH msg IN pending
        EMIT MessageStart(msg)
        EMIT MessageEnd(msg)
        context.messages.append(msg)
        new_messages.append(msg)
        context.user_context.append(msg)    // steering goes to user stream (never pruned)
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

      // Compact context if configured (strategies live in context_config.compaction)
      IF config.context_config is defined THEN
        ctx_config ← config.context_config
        IF tokens_exceed_threshold(context, ctx_config) THEN
          IF config.before_compaction_start defined THEN
            IF NOT before_compaction_start(estimated_tokens, message_count) THEN
              SKIP compaction this cycle
            END IF
          END IF
          EMIT CompactionStarted { ... }
          strategy ← ctx_config.compaction.in_memory_strategy OR DefaultCompaction
          context.messages ← strategy.compact(context.messages, ctx_config)
          EMIT CompactionEnded { ... }
          IF config.after_compaction_end defined THEN
            after_compaction_end(msgs_before, msgs_after, tokens_before, tokens_after)
          END IF
        END IF
      END IF


      // ── LLM call ────────────────────────────────────────────────────────
      message ← AWAIT stream_assistant_response(context, config, tx, cancel)

      agent_msg ← message as AgentMessage
      context.messages.append(agent_msg)
      new_messages.append(agent_msg)
      context.inrun_context.append(Live(agent_msg))   // track in inrun stream (model-generated)

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
          context.inrun_context.append(Live(am))   // track in inrun stream
        END FOR

        // Apply pending prun requests after tool execution (PrunTool stores requests during execute)
        IF config.prun_pending is defined THEN
          requests ← LOCK(config.prun_pending).drain()
          FOR EACH request IN requests
            apply_prun(context, request, tx)  // walks inrun_context backward, prunes Live entries
          END FOR
        END IF
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
