<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
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
