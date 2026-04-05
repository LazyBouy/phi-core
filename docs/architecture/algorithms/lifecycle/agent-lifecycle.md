<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
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
      - Sets context_config.compaction.in_memory_strategy (custom compaction implementation)

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
    // compaction strategies are now inside context_config.compaction (G5)
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
    context_config: self.context_config,  // includes compaction strategies (G5)
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
