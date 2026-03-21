# Implementation Roadmap

> Generated from: `overview.md`, `architecture.md`, `pseudocode.md`
> Last updated: 2026-03-17
> Paradigm: Language-agnostic / Implementation-independent

This roadmap defines six progressive stages of implementation derived from the
reverse-engineered specification. Each level is a complete, testable stage.
Complete and stabilize each level fully before advancing to the next.

***

## Level 1 — Survive
> **Goal:** The system can start, load configuration, initialize its core
> structures, and confirm it is alive. Nothing works end-to-end yet,
> but nothing crashes either.

**Completion Criteria:** A smoke test confirms the Agent can be constructed
with a MockProvider, configured via builder methods, and all core data entities
can be instantiated without error. No LLM call is required to pass Level 1.

---

### Milestone 1.1 — Core Type System

- [ ] **REQ-001:** Define the `Content` enum with four variants: `Text { text }`, `Image { data: base64, mime_type }`, `Thinking { thinking, signature }`, and `ToolCall { id, name, arguments }`. Serialized with a `"type"` discriminant field. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: All four variants instantiate; round-trip JSON serialization produces the correct tagged shape.

- [ ] **REQ-002:** Define the `Message` enum with three variants: `User { content, timestamp }`, `Assistant { content, stop_reason, model, provider, usage, timestamp, error_message }`, and `ToolResult { tool_call_id, tool_name, content, is_error, timestamp }`. *(Source: [AR])*
  - Depends on: REQ-001, REQ-005, REQ-006
  - Definition of Done: All three variants instantiate; serialization preserves the `role` field with values `"user"`, `"assistant"`, `"toolResult"`.

- [ ] **REQ-003:** Define `AgentMessage` as an untagged enum wrapping `Llm(Message)` and `Extension(ExtensionMessage)`. *(Source: [AR])*
  - Depends on: REQ-002, REQ-004
  - Definition of Done: Both variants serialize/deserialize correctly; an `Extension` variant round-trips without loss.

- [ ] **REQ-004:** Define `ExtensionMessage` with fields `role: String` (always `"extension"`), `kind: String`, and `data: JSON`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Instantiates and serializes to `{role:"extension", kind:"...", data:{...}}`.

- [ ] **REQ-005:** Define `StopReason` enum with variants `Stop`, `Length`, `ToolUse`, `Error`, `Aborted`. Serialized in camelCase. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: All variants serialize to their documented camelCase strings.

- [ ] **REQ-006:** Define `Usage` struct with fields `input`, `output`, `cache_read`, `cache_write`, `total_tokens` (all `u64`). Include a `cache_hit_rate()` derived method. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: `cache_hit_rate()` returns `cache_read / (input + cache_read + cache_write)`.

- [ ] **REQ-007:** Define `AgentEvent` enum with all variants: `AgentStart`, `AgentEnd { messages }`, `TurnStart`, `TurnEnd { message, tool_results }`, `MessageStart { message }`, `MessageUpdate { message, delta }`, `MessageEnd { message }`, `ToolExecutionStart { tool_call_id, tool_name, args }`, `ToolExecutionUpdate { tool_call_id, tool_name, partial_result }`, `ToolExecutionEnd { tool_call_id, tool_name, result, is_error }`, `ProgressMessage { tool_call_id, tool_name, text }`, `InputRejected { reason }`. *(Source: [AR])*
  - Depends on: REQ-002, REQ-008
  - Definition of Done: All variants instantiate.

- [ ] **REQ-008:** Define `StreamDelta` enum with variants `Text { delta }`, `Thinking { delta }`, `ToolCallDelta { delta }`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: All variants instantiate and carry their string payload.

- [ ] **REQ-009:** Define `ToolContext` struct with fields `tool_call_id`, `tool_name`, `cancel: CancellationToken`, `on_update: Option<ToolUpdateFn>`, `on_progress: Option<ProgressFn>`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Struct instantiates; callback fields accept closures/function pointers.

- [ ] **REQ-010:** Define `ToolResult { content: Vec<Content>, details: JSON }` and `ToolError` enum with variants `Failed(String)`, `NotFound(String)`, `InvalidArgs(String)`, `Cancelled`. *(Source: [AR])*
  - Depends on: REQ-001
  - Definition of Done: All variants instantiate; `ToolError` converts to a display string.

- [ ] **REQ-011:** Define `ContextConfig` struct with fields and defaults: `max_context_tokens` (100,000), `system_prompt_tokens` (4,000), `keep_recent` (10), `keep_first` (2), `tool_output_max_lines` (50). *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Default construction produces the documented default values.

- [ ] **REQ-012:** Define `ExecutionLimits` struct with defaults `max_turns` (50), `max_total_tokens` (1,000,000), `max_duration` (600s); and `ExecutionTracker` runtime state with fields `limits`, `turns`, `tokens_used`, `started_at`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: `ExecutionTracker::new(limits)` initializes `turns=0`, `tokens_used=0`, `started_at=now`.

- [ ] **REQ-013:** Define `RetryConfig` with defaults: `max_retries` (3), `initial_delay_ms` (1,000), `backoff_multiplier` (2.0), `max_delay_ms` (30,000). *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Default construction produces documented defaults.

- [ ] **REQ-014:** Define `CacheConfig { enabled: bool, strategy: CacheStrategy }` and `CacheStrategy` enum with variants `Auto`, `Disabled`, `Manual { cache_system, cache_tools, cache_messages }`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: All variants instantiate; default `CacheConfig` has `enabled: true`, `strategy: Auto`.

- [ ] **REQ-015:** Define `StreamConfig` struct with fields `model`, `system_prompt`, `messages: Vec<Message>`, `tools: Vec<ToolDefinition>`, `thinking_level`, `api_key`, `max_tokens`, `temperature`, `model_config`, `cache_config`. *(Source: [AR])*
  - Depends on: REQ-014, REQ-016
  - Definition of Done: Struct instantiates with all optional fields as `None`.

- [ ] **REQ-016:** Define `ToolDefinition` struct with fields `name`, `description`, `parameters: JSON`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Struct instantiates and serializes to the expected JSON shape.

- [ ] **REQ-017:** Define `QueueMode` enum with variants `OneAtATime` and `All`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Both variants exist; default is `OneAtATime`.

- [ ] **REQ-018:** All types in the `AgentMessage` tree derive `Serialize` and `Deserialize`. *(Source: [OV])*
  - Depends on: REQ-001 through REQ-017
  - Definition of Done: Full round-trip JSON serialization of a `Vec<AgentMessage>` containing all message types is lossless.

- [ ] **REQ-019:** Define `ThinkingLevel` enum with variants `Off`, `Minimal`, `Low`, `Medium`, `High`. *(Source: [OV])*
  - Depends on: —
  - Definition of Done: All variants exist.

---

### Milestone 1.2 — Core Traits

- [ ] **REQ-020:** Define `StreamProvider` trait with a single method `stream(config: StreamConfig, tx: EventSender, cancel: CancellationToken) -> Result<Message, ProviderError>`. Define `ProviderError` enum with variants `Api(String)`, `Network(String)`, `Auth(String)`, `RateLimited { retry_after_ms: Option<u64> }`, `ContextOverflow { message: String }`, `Cancelled`, `Other(String)`. *(Source: [AR])*
  - Depends on: REQ-002, REQ-015
  - Definition of Done: Trait compiles; `ProviderError` variants all instantiate.

- [ ] **REQ-021:** Define `AgentTool` trait with methods `name() -> &str`, `label() -> &str`, `description() -> &str`, `parameters_schema() -> JSON`, `execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>`. *(Source: [AR])*
  - Depends on: REQ-009, REQ-010
  - Definition of Done: Trait compiles; a minimal struct can implement it.

- [ ] **REQ-022:** Define `InputFilter` trait with method `filter(text: &str) -> FilterResult` where `FilterResult` is `Pass`, `Warn(String)`, or `Reject(String)`. *(Source: [OV])*
  - Depends on: —
  - Definition of Done: Trait compiles; all three result variants exist.

- [ ] **REQ-023:** Define `CompactionStrategy` trait with method `compact(messages: Vec<AgentMessage>, config: ContextConfig) -> Vec<AgentMessage>`. *(Source: [AR])*
  - Depends on: REQ-003, REQ-011
  - Definition of Done: Trait compiles; a struct can implement it.

---

### Milestone 1.3 — Agent Struct Construction

- [ ] **REQ-024:** Implement `Agent::new(provider: impl StreamProvider) -> Agent`. Initialize all fields to documented defaults: `messages = []`, `tools = []`, `thinking_level = Off`, `tool_execution = Parallel`, `steering_mode = OneAtATime`, `follow_up_mode = OneAtATime`, `context_config = Some(default)`, `execution_limits = Some(default)`, `retry_config = default`, `is_streaming = false`, `cancel = None`. *(Source: [PS])*
  - Depends on: REQ-011 through REQ-017, REQ-019, REQ-020
  - Definition of Done: `Agent::new(mock_provider)` compiles and all fields have their documented defaults.

- [ ] **REQ-025:** Implement builder methods: `with_system_prompt(text)`, `with_model(id)`, `with_api_key(key)`, `with_max_tokens(n)`, `with_temperature(t)`, `with_model_config(cfg)`, `with_thinking_level(level)`. *(Source: [PS])*
  - Depends on: REQ-024
  - Definition of Done: Method chain `Agent::new(p).with_system_prompt("x").with_model("m").with_api_key("k")` compiles and all fields are set correctly.

- [ ] **REQ-026:** Implement `with_tools(vec)`, `with_context_config(cfg)`, `with_execution_limits(limits)`, `with_retry_config(cfg)`, `with_cache_config(cfg)`, `with_tool_execution(strategy)`, `with_steering_mode(mode)`, `with_follow_up_mode(mode)`. *(Source: [PS])*
  - Depends on: REQ-024
  - Definition of Done: All builders set their respective fields; `with_tools` replaces (or extends) the tools list.

- [ ] **REQ-027:** Initialize `steering_queue` and `follow_up_queue` as `Arc<Mutex<Vec<AgentMessage>>>` in `Agent::new`. *(Source: [AR])*
  - Depends on: REQ-003, REQ-024
  - Definition of Done: Both queues are non-null, independently lockable, and start empty.

---

### Milestone 1.4 — AgentContext and AgentLoopConfig

- [ ] **REQ-028:** Define `AgentContext` struct with fields `system_prompt: String`, `messages: Vec<AgentMessage>`, `tools: &[Box<dyn AgentTool>]`. *(Source: [AR])*
  - Depends on: REQ-003, REQ-021
  - Definition of Done: Struct compiles; `messages` is mutable in-place during the loop.

- [ ] **REQ-029:** Define `AgentLoopConfig` struct bundling all behavioral settings: `provider`, `model`, `api_key`, `thinking_level`, `max_tokens`, `temperature`, `model_config`, `get_steering_messages: Option<Fn()>`, `get_follow_up_messages: Option<Fn()>`, `context_config`, `compaction_strategy`, `execution_limits`, `cache_config`, `tool_execution`, `retry_config`, `before_turn`, `after_turn`, `on_error`, `input_filters`, `transform_context`, `convert_to_llm`. *(Source: [OV])*
  - Depends on: REQ-011 through REQ-017, REQ-023
  - Definition of Done: Struct compiles with all optional fields as `None`.

---

### Milestone 1.5 — MockProvider and Smoke Test

- [ ] **REQ-030:** Implement `MockProvider` that implements `StreamProvider`. Accepts a list of pre-configured responses to return in sequence. Returns a `Message::Assistant` with `stop_reason: Stop` and configurable text content. *(Source: [AR])*
  - Depends on: REQ-020
  - Definition of Done: `MockProvider::new(vec![response1, response2])` returns each response in order when `stream()` is called; after exhausting the list, returns a default stop response.

- [ ] **REQ-031:** Smoke test: construct `Agent::new(MockProvider::new([]))`, configure with builder methods, verify all fields are set correctly, and confirm no panic occurs. *(Source: [OV])*
  - Depends on: REQ-024 through REQ-030
  - Definition of Done: Test passes with zero panics; all configured fields read back correctly.

***

## Level 2 — Useful
> **Goal:** The primary use cases from the spec work end-to-end on valid,
> well-formed inputs. An agent can accept a prompt, call an LLM, execute
> tool calls, and return a final response.

**Completion Criteria:** Every primary use case from `overview.md` executes
successfully with valid inputs and a real (or mock) provider: single-turn text
response, multi-turn tool call cycle, message persistence round-trip, and agent
reset. The built-in coding tools all execute on valid inputs.

---

### Milestone 2.1 — Event Channel Infrastructure

- [ ] **REQ-032:** Implement an unbounded async event channel. The `agent_loop` holds the sender (`tx`); callers receive from the receiver (`rx`). The channel never blocks the sender. *(Source: [AR])*
  - Depends on: REQ-007
  - Definition of Done: Sender can emit 1,000 events without blocking; receiver drains them all in order.

- [ ] **REQ-033:** Implement `CancellationToken` with methods `new()`, `cancel()`, `is_cancelled() -> bool`, `child_token() -> CancellationToken`. Cancelling a parent automatically cancels all children. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Cancelling a root token causes `is_cancelled()` to return `true` on both the root and any child tokens created from it.

---

### Milestone 2.2 — Agent Prompt Entry Point

- [ ] **REQ-034:** Implement `Agent::prompt(text: String) -> EventReceiver` as a thin wrapper that constructs a `User` message and delegates to `prompt_messages`. *(Source: [PS])*
  - Depends on: REQ-002, REQ-035
  - Definition of Done: `agent.prompt("hello")` returns a receiver immediately (non-blocking).

- [ ] **REQ-035:** Implement `Agent::prompt_messages_with_sender(messages, tx)`: set `is_streaming = true`, create `CancellationToken`, build `AgentContext` snapshot, build `AgentLoopConfig` (wiring queue closures), spawn `agent_loop`, merge returned messages into `Agent.messages` on completion, set `is_streaming = false`. *(Source: [PS])*
  - Depends on: REQ-027, REQ-028, REQ-029, REQ-033, REQ-036
  - Definition of Done: After the spawned task completes, `agent.messages` contains the new messages and `is_streaming` is `false`.

---

### Milestone 2.3 — Agent Loop Core

- [ ] **REQ-036:** Implement `agent_loop`: emit `AgentStart`, append prompts to `context.messages`, emit `TurnStart`/`MessageStart`/`MessageEnd` for each prompt, call `run_loop`, emit `AgentEnd`, return new messages. *(Source: [PS])*
  - Depends on: REQ-032, REQ-037
  - Definition of Done: With `MockProvider`, a single call emits `AgentStart`, at least one `TurnStart`/`TurnEnd` pair, and `AgentEnd`; returned messages include the input prompt and the assistant response.

- [ ] **REQ-037:** Implement `agent_loop_continue`: emit `AgentStart`/`TurnStart`, call `run_loop`, emit `AgentEnd`. *(Source: [PS])*
  - Depends on: REQ-036
  - Definition of Done: Resumes from existing context without re-appending prompts.

- [ ] **REQ-038:** Implement `run_loop` inner loop (happy path only: no steering, no follow-ups, no limits): call `stream_assistant_response`, append assistant message, extract tool calls, call `execute_tool_calls`, append tool results, loop until no more tool calls, then break. *(Source: [PS])*
  - Depends on: REQ-039, REQ-045, REQ-060
  - Definition of Done: With a MockProvider that returns one tool call then one `Stop`, `run_loop` executes the tool and calls the LLM a second time before stopping.

---

### Milestone 2.4 — LLM Streaming (Happy Path)

- [ ] **REQ-039:** Implement `stream_assistant_response` (no retry): build `StreamConfig` from context and config, call `provider.stream()`, process stream events (`Start` → emit `MessageStart`; `TextDelta`/`ThinkingDelta`/`ToolCallDelta` → emit `MessageUpdate`; `Done` → emit `MessageEnd`; `Error` → emit `MessageStart`+`MessageEnd`), return final `Message`. *(Source: [PS])*
  - Depends on: REQ-007, REQ-008, REQ-015, REQ-020, REQ-032
  - Definition of Done: With MockProvider, caller receives `MessageStart`, one or more `MessageUpdate` with text deltas, and `MessageEnd` containing the complete assembled message.

- [ ] **REQ-040:** Implement `AnthropicProvider::stream`: POST to `https://api.anthropic.com/v1/messages` with `x-api-key` + `anthropic-version: 2023-06-01` headers, `stream: true` body; parse SSE events (`message_start`, `content_block_start`, `content_block_delta`, `message_delta`, `message_stop`); buffer `InputJsonDelta` tool-argument fragments; parse complete JSON on `content_block_stop`; emit `StreamEvent`s. *(Source: [AR])*
  - Depends on: REQ-020, REQ-039
  - Definition of Done: Integration test with a real or stubbed Anthropic endpoint produces a correctly parsed `Message::Assistant` with usage stats.

- [ ] **REQ-041:** Implement `OpenAiCompatProvider::stream`: POST to configured base URL + `/chat/completions` with `Authorization: Bearer` header, `stream: true`, `stream_options: {include_usage: true}`; parse SSE chunks `choices[0].delta`; accumulate tool-call argument strings; emit `StreamEvent`s. *(Source: [AR])*
  - Depends on: REQ-020, REQ-039
  - Definition of Done: Correctly parses a streamed chat-completion response from any OpenAI-compatible endpoint.

- [ ] **REQ-042:** Implement `ProviderRegistry` with `new()` (empty) and `default()` (pre-registers `AnthropicProvider` and `OpenAiCompatProvider`). `ProviderRegistry` itself implements `StreamProvider`, dispatching based on `ApiProtocol` or model prefix. *(Source: [AR])*
  - Depends on: REQ-040, REQ-041
  - Definition of Done: `ProviderRegistry::default()` can route a config to `AnthropicProvider` or `OpenAiCompatProvider` without manual dispatch.

- [ ] **REQ-043:** Implement `StopReason` determination in each provider: map provider-specific stop signals to the unified `StopReason` enum (`"end_turn"`/`"stop"` → `Stop`; `"max_tokens"`/`"length"` → `Length`; `"tool_use"`/`"tool_calls"` → `ToolUse`; cancellation → `Aborted`; errors → `Error`). *(Source: [PS])*
  - Depends on: REQ-005, REQ-040, REQ-041
  - Definition of Done: Each stop signal string maps to exactly one `StopReason` variant.

- [ ] **REQ-044:** Filter `Extension` messages out of `AgentMessage` history before building `StreamConfig.messages`. Only `Llm(Message)` variants are sent to the LLM. *(Source: [AR])*
  - Depends on: REQ-003, REQ-015
  - Definition of Done: An `AgentMessage::Extension` present in `context.messages` does not appear in the `StreamConfig` sent to the provider.

---

### Milestone 2.5 — Tool Execution (Happy Path)

- [ ] **REQ-045:** Implement `execute_tool_calls` dispatching to the configured `ToolExecutionStrategy`. For `Parallel` (default), use `execute_batch`. *(Source: [PS])*
  - Depends on: REQ-046
  - Definition of Done: Multiple tool calls from one LLM response are dispatched concurrently; results arrive in original call order.

- [ ] **REQ-046:** Implement `execute_single_tool`: find tool by name, emit `ToolExecutionStart`, build `ToolContext` with child cancel token and callbacks, call `tool.execute(args, ctx)`, emit `ToolExecutionEnd`, construct `Message::ToolResult`, emit `MessageStart`/`MessageEnd`, return `(ToolResult, is_error)`. *(Source: [PS])*
  - Depends on: REQ-007, REQ-009, REQ-010, REQ-021, REQ-033
  - Definition of Done: A registered tool is called; its result is wrapped in a `ToolResult` message; `ToolExecutionStart` and `ToolExecutionEnd` events are emitted.

- [ ] **REQ-047:** Implement `BashTool::execute` (basic): extract `command` param, run `bash -c {command}`, capture stdout+stderr, construct text output (`"Exit code: N\n{stdout}"` or `"Exit code: N\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"`), return `Ok(ToolResult)`. *(Source: [PS])*
  - Depends on: REQ-010, REQ-021
  - Definition of Done: `echo "hello"` returns `Ok(ToolResult)` with text containing `"Exit code: 0"` and `"hello"`.

- [ ] **REQ-048:** Implement `ReadFileTool::execute` (basic text path): extract `path` param, read file to string, split into lines, apply optional `offset`/`limit`, produce line-numbered output with header, return `Ok(ToolResult)`. *(Source: [PS])*
  - Depends on: REQ-010, REQ-021
  - Definition of Done: Reading a known text file returns numbered lines; partial reads with `offset`/`limit` return the correct slice with a range header.

- [ ] **REQ-049:** Implement `WriteFileTool::execute`: extract `path` and `content` params, create parent directories as needed, write file, return `Ok(ToolResult)`. *(Source: [AR])*
  - Depends on: REQ-010, REQ-021
  - Definition of Done: Writing to a path with non-existent parent directories succeeds; file is created on disk with correct content.

- [ ] **REQ-050:** Implement `EditFileTool::execute` (basic): extract `path`, `old_text`, `new_text`; read file; replace the first occurrence of `old_text` with `new_text`; write back; return confirmation text. *(Source: [PS])*
  - Depends on: REQ-010, REQ-021
  - Definition of Done: A known substitution in an existing file is applied correctly; confirmation message reports old/new line counts.

- [ ] **REQ-051:** Implement `ListFilesTool::execute` (basic): extract `path`, `pattern`, `max_depth`; build and run `find` command with exclusions for `target/`, `.git/`, `node_modules/`; return file paths as text. *(Source: [PS])*
  - Depends on: REQ-010, REQ-021
  - Definition of Done: Listing a known directory returns its files; excluded directories do not appear in results.

- [ ] **REQ-052:** Implement `SearchTool::execute` (basic): extract `pattern`, `path`, `include`, `case_sensitive`; prefer `rg`, fall back to `grep`; return matching lines. *(Source: [PS])*
  - Depends on: REQ-010, REQ-021
  - Definition of Done: Searching for a known string in a known directory returns matching file paths and line content.

- [ ] **REQ-053:** Implement `default_tools()` returning a `Vec<Box<dyn AgentTool>>` containing all six built-in tools: Bash, ReadFile, WriteFile, EditFile, ListFiles, Search. *(Source: [AR])*
  - Depends on: REQ-047 through REQ-052
  - Definition of Done: `default_tools()` returns exactly 6 tools with distinct names.

---

### Milestone 2.6 — Context Compaction (Happy Path)

- [ ] **REQ-054:** Implement `estimate_tokens(text) -> usize` using the heuristic `ceil(byte_length / 4)`. *(Source: [PS])*
  - Depends on: —
  - Definition of Done: `estimate_tokens("hello")` returns 2 (5 bytes / 4, rounded up).

- [ ] **REQ-055:** Implement `content_tokens(content: Vec<Content>) -> usize` and `message_tokens(msg: AgentMessage) -> usize` per the specified formulas (image tokens: `clamp(raw_bytes/750, 85, 16000)`; per-message overhead: +4 for user/assistant, +8 for tool result). *(Source: [PS])*
  - Depends on: REQ-001, REQ-003, REQ-054
  - Definition of Done: Token counts match the specified formulas for each content type.

- [ ] **REQ-056:** Implement `compact_messages(messages, config) -> Vec<AgentMessage>`: if under budget, return unchanged; else cascade through Level 1 → Level 2 → Level 3 until budget is satisfied. *(Source: [PS])*
  - Depends on: REQ-055, REQ-057, REQ-058, REQ-059
  - Definition of Done: `compact_messages` called on a history exceeding budget returns a smaller history with `total_tokens <= budget`.

- [ ] **REQ-057:** Implement `level1_truncate_tool_outputs`: for each `ToolResult` message, truncate each `Text` content block to at most `max_lines` using head+tail preservation with an omission marker. *(Source: [PS])*
  - Depends on: REQ-003, REQ-054
  - Definition of Done: A 200-line tool output truncated to `max_lines=50` produces a 50-line result with `"[... N lines truncated ...]"` marker.

- [ ] **REQ-058:** Implement `level2_summarize_old_turns`: keep the last `keep_recent` messages in full; replace older assistant+tool-result groups with a single one-line summary user message. *(Source: [PS])*
  - Depends on: REQ-003, REQ-054
  - Definition of Done: Old assistant messages and their tool results are replaced by `"[Summary] ..."` user messages; recent messages are untouched.

- [ ] **REQ-059:** Implement `level3_drop_middle`: keep `keep_first` head messages and `keep_recent` tail messages; replace the dropped middle with a marker message. Implement `keep_within_budget` fallback that greedily keeps the most-recent messages fitting the budget. *(Source: [PS])*
  - Depends on: REQ-003, REQ-054
  - Definition of Done: Result contains the first N and last M messages with a marker; total tokens fits the budget.

- [ ] **REQ-060:** Integrate `compact_messages` call in `run_loop` before each LLM call when `context_config` is `Some`. *(Source: [PS])*
  - Depends on: REQ-038, REQ-056
  - Definition of Done: When configured, each LLM call is preceded by a compaction pass; when `context_config` is `None`, no compaction occurs.

---

### Milestone 2.7 — Execution Limits

- [ ] **REQ-061:** Implement `ExecutionTracker::record_turn(tokens: usize)` (increments `turns` and adds to `tokens_used`) and `check_limits() -> Option<String>` (returns a reason string if any limit is exceeded: turns, total tokens, or wall-clock duration). *(Source: [AR])*
  - Depends on: REQ-012
  - Definition of Done: `check_limits()` returns `None` when under all limits and `Some("max turns exceeded")` when over.

- [ ] **REQ-062:** Integrate execution limit checking in `run_loop`: call `tracker.check_limits()` at the start of each inner loop iteration; if exceeded, append a synthetic `User` message `"[Agent stopped: {reason}]"`, emit `MessageStart`/`MessageEnd`, and return. *(Source: [PS])*
  - Depends on: REQ-038, REQ-061
  - Definition of Done: An agent with `max_turns=2` stops after exactly 2 LLM calls; the last message contains the stop reason.

---

### Milestone 2.8 — Message Persistence and Agent Control

- [ ] **REQ-063:** Implement `Agent::save_messages() -> String`: serialize `agent.messages` to a JSON string. *(Source: [OV])*
  - Depends on: REQ-018
  - Definition of Done: `save_messages()` returns a valid JSON array; the string can be parsed back without error.

- [ ] **REQ-064:** Implement `Agent::restore_messages(json: &str)`: deserialize the JSON string into `Vec<AgentMessage>` and replace `agent.messages`. *(Source: [OV])*
  - Depends on: REQ-018, REQ-063
  - Definition of Done: After `save_messages()` → `restore_messages()`, the agent's message history is identical to the original.

- [ ] **REQ-065:** Implement `Agent::reset()`: clear `messages`, drain both queues, cancel any active run, reset `is_streaming` to `false`, drop the cancel token. *(Source: [AR])*
  - Depends on: REQ-033
  - Definition of Done: After `reset()`, `messages` is empty, both queues are empty, and `is_streaming` is false.

- [ ] **REQ-066:** Implement `Agent::steer(msg: AgentMessage)` (push to `steering_queue`) and `Agent::follow_up(msg: AgentMessage)` (push to `follow_up_queue`). *(Source: [AR])*
  - Depends on: REQ-027
  - Definition of Done: After `steer(msg)`, the steering queue contains exactly that message and is safe to read from another thread.

- [ ] **REQ-067:** Implement `Agent::abort()`: if a cancel token exists, call `cancel()` on it. *(Source: [AR])*
  - Depends on: REQ-033, REQ-035
  - Definition of Done: Calling `abort()` during an active run causes `cancel.is_cancelled()` to return `true` inside the running agent loop.

***

## Level 3 — Smart
> **Goal:** The system handles reality. Invalid inputs, missing data,
> external failures, and edge cases are all handled gracefully.
> Every `[invariant]` and `ERROR` branch from the pseudocode is implemented.

**Completion Criteria:** No unhandled exception can be triggered by a known
class of bad input. All error paths from `pseudocode.md` are covered:
provider failures, tool errors, context overflow, execution limits,
filter rejections, and cancellation.

---

### Milestone 3.1 — Input Filter Chain

- [ ] **REQ-068:** Implement the input filter chain at the start of `agent_loop`: join all `Text` content from `User` messages in prompts, run each registered `InputFilter` in order. *(Source: [PS])*
  - Depends on: REQ-022, REQ-036
  - Definition of Done: A filter registered via `with_input_filter` is called with the user's text before any LLM call.

- [ ] **REQ-069:** On first `Reject` result, emit `InputRejected { reason }` then `AgentEnd { messages: [] }` and return an empty message list immediately. *(Source: [PS])*
  - Depends on: REQ-068
  - Definition of Done: A rejecting filter stops the run before the first LLM call; the caller's event stream contains `InputRejected` followed by `AgentEnd`.

- [ ] **REQ-070:** Accumulate `Warn` results; after all filters pass, append all warning text as `Content::Text` to the last `User` message before it is appended to context. *(Source: [PS])*
  - Depends on: REQ-068
  - Definition of Done: A warning filter adds `"[Warning: ...]"` text to the user message; the run continues normally.

---

### Milestone 3.2 — Retry Engine

- [ ] **REQ-071:** Implement `delay_for_attempt(config, attempt) -> Duration`: exponential backoff formula `initial_delay_ms * (multiplier ^ (attempt - 1))`, capped at `max_delay_ms`, multiplied by a uniform random jitter in `[0.8, 1.2]`. *(Source: [PS])*
  - Depends on: REQ-013
  - Definition of Done: With defaults, attempt 1 produces a duration in `[800ms, 1200ms]`; attempt 3 produces a duration in `[3200ms, 4800ms]`.

- [ ] **REQ-072:** Implement `is_retryable()` on `ProviderError`: returns `true` only for `RateLimited` and `Network` variants. *(Source: [AR])*
  - Depends on: REQ-020
  - Definition of Done: `Auth`, `Api`, `ContextOverflow`, `Cancelled`, `Other` all return `false`; `RateLimited` and `Network` return `true`.

- [ ] **REQ-073:** Implement `retry_after()` on `ProviderError`: extracts `retry_after_ms` from `RateLimited { retry_after_ms: Some(n) }` if present; returns `None` otherwise. *(Source: [AR])*
  - Depends on: REQ-020
  - Definition of Done: `ProviderError::RateLimited { retry_after_ms: Some(5000) }.retry_after()` returns `Some(Duration::from_ms(5000))`.

- [ ] **REQ-074:** Integrate retry loop into `stream_assistant_response`: on a retryable error, sleep for `retry_after() OR delay_for_attempt(attempt)` and retry up to `max_retries` times; stop retrying if `cancel.is_cancelled()`. *(Source: [PS])*
  - Depends on: REQ-039, REQ-071, REQ-072, REQ-073
  - Definition of Done: A `RateLimited` error causes the loop to wait and retry; after exhausting retries, the error is propagated as an `Error` stop reason.

---

### Milestone 3.3 — Provider Error Classification

- [ ] **REQ-075:** Implement `ProviderError::classify(status: u16, message: String) -> ProviderError`: route to `ContextOverflow` first (status 400/413 or matching overflow phrase), then `RateLimited` (429), then `Auth` (401/403), then `Api`. *(Source: [PS])*
  - Depends on: REQ-020
  - Definition of Done: HTTP 429 maps to `RateLimited`; HTTP 401 maps to `Auth`; "prompt is too long" in the body maps to `ContextOverflow`.

- [ ] **REQ-076:** Implement `is_context_overflow(status, message) -> bool`: check for empty body with status 400/413 (Cerebras/Mistral pattern); check for any of 15+ documented overflow phrases (case-insensitive substring match). *(Source: [PS])*
  - Depends on: —
  - Definition of Done: All 15 documented overflow phrases are recognized; unrelated 400 errors with non-empty body are not misclassified.

- [ ] **REQ-077:** Implement context overflow recovery: when the streaming error event contains a message matching overflow detection (`Message::is_context_overflow()`), treat it as an overflow on the next turn by triggering `compact_messages` (if `context_config` is set). *(Source: [AR])*
  - Depends on: REQ-056, REQ-075, REQ-076
  - Definition of Done: A mock that returns an overflow error on turn 1 causes compaction before turn 2.

---

### Milestone 3.4 — Tool Error Handling

- [ ] **REQ-078:** On `ToolError::Failed(msg)` or `ToolError::InvalidArgs(msg)`: convert to a `ToolResult` with `content: [Text(msg)]` and `is_error: true`; always return this to the LLM so it can self-correct. *(Source: [AR])*
  - Depends on: REQ-010, REQ-046
  - Definition of Done: A tool that returns `Err(Failed("oops"))` produces a `ToolResult` message with `is_error: true` and the text `"oops"`.

- [ ] **REQ-079:** On `ToolError::NotFound(name)`: produce `ToolResult { content: [Text("Tool {name} not found")], is_error: true }`. *(Source: [PS])*
  - Depends on: REQ-046
  - Definition of Done: Requesting a non-existent tool name in a tool call produces a `NotFound` error result.

- [ ] **REQ-080:** On `ToolError::Cancelled`: produce `ToolResult { content: [Text("Skipped due to queued user message.")], is_error: true }`. *(Source: [AR])*
  - Depends on: REQ-010, REQ-046
  - Definition of Done: A tool skipped due to steering produces the documented skipped message.

---

### Milestone 3.5 — Error and Abort Stop Reason Handling

- [ ] **REQ-081:** In `run_loop`, when the assistant message has `stop_reason == Error`: call `on_error(error_message)` if defined, call `after_turn` if defined, emit `TurnEnd`, return immediately. *(Source: [PS])*
  - Depends on: REQ-038, REQ-082
  - Definition of Done: A mock provider that returns an error stop reason causes the loop to exit; `on_error` is called with the message text.

- [ ] **REQ-082:** In `run_loop`, when `stop_reason == Aborted`: call `after_turn` if defined, emit `TurnEnd`, return immediately. *(Source: [PS])*
  - Depends on: REQ-038
  - Definition of Done: Calling `agent.abort()` mid-run causes the loop to exit cleanly; `TurnEnd` is emitted.

- [ ] **REQ-083:** Construct a synthetic error `Message::Assistant` on irrecoverable provider failure (after retry exhaustion): empty content, `stop_reason: Error`, `error_message: Some(e.to_string())`. *(Source: [PS])*
  - Depends on: REQ-002, REQ-039
  - Definition of Done: A provider that always fails produces an `Assistant` message with `stop_reason: Error` containing the provider's error text.

---

### Milestone 3.6 — Sequential and Batched Tool Execution

- [ ] **REQ-084:** Implement `execute_sequential`: execute tool calls one at a time; after each, check the steering queue; on non-empty steering, skip remaining tools with `ToolError::Cancelled` results and return steering messages. *(Source: [PS])*
  - Depends on: REQ-046, REQ-080
  - Definition of Done: With steering arriving after tool 1 of 3, tools 2 and 3 receive skipped error results; the steering message is returned for injection.

- [ ] **REQ-085:** Implement `execute_batch` (Parallel): launch all tools concurrently via `join_all`; after all complete, check steering once; return steering if present. *(Source: [PS])*
  - Depends on: REQ-046
  - Definition of Done: Three parallel tools all complete; steering arriving before their completion is returned after all finish.

- [ ] **REQ-086:** Implement `Batched { size }` dispatch: split tool calls into groups of `size`; run each group via `execute_batch`; check steering between groups; on steering, skip remaining groups with cancelled results. *(Source: [PS])*
  - Depends on: REQ-085
  - Definition of Done: With 5 tool calls, `Batched { size: 2 }` executes groups [1,2], [3,4], [5]; steering after group 1 skips groups 2 and 3.

---

### Milestone 3.7 — Steering and Follow-up Queue Integration

- [ ] **REQ-087:** In `run_loop`, drain the steering queue at the start of the outer loop before the first inner-loop iteration. *(Source: [PS])*
  - Depends on: REQ-038
  - Definition of Done: Messages enqueued via `steer()` before `prompt()` is called are injected as the first pending messages.

- [ ] **REQ-088:** After tool execution, if steering messages were captured, set them as `pending` and continue the inner loop (injecting them before the next LLM call). *(Source: [PS])*
  - Depends on: REQ-038, REQ-084, REQ-085
  - Definition of Done: A steering message injected during tool execution appears in context before the subsequent LLM call.

- [ ] **REQ-089:** After the inner loop exits (no tool calls, no pending steering), check the follow-up queue; if non-empty, add follow-up messages to `pending` and continue the outer loop. *(Source: [PS])*
  - Depends on: REQ-038
  - Definition of Done: A follow-up message enqueued via `follow_up()` causes the agent to re-enter the loop rather than stopping.

- [ ] **REQ-090:** Implement `QueueMode::OneAtATime` (pop exactly one message per read) and `QueueMode::All` (drain the entire queue per read). Both modes are thread-safe (mutex-protected). *(Source: [AR])*
  - Depends on: REQ-017, REQ-027
  - Definition of Done: `OneAtATime` leaves remaining messages in the queue; `All` empties it; both are safe to call from the agent loop while another thread pushes.

---

### Milestone 3.8 — Lifecycle Callbacks

- [ ] **REQ-091:** Call `before_turn(messages, turn_number) -> bool` at the start of each turn (before the LLM call). If it returns `false`, return from `run_loop` immediately without emitting `AgentEnd`. *(Source: [PS])*
  - Depends on: REQ-038
  - Definition of Done: A `before_turn` that returns `false` on turn 2 stops the loop after turn 1; `AgentEnd` is not emitted.

- [ ] **REQ-092:** Call `after_turn(messages, usage)` after each LLM call and its tool executions, including on error/abort paths. *(Source: [PS])*
  - Depends on: REQ-038
  - Definition of Done: `after_turn` is called exactly once per turn, including when the turn ends in an error.

- [ ] **REQ-093:** Call `on_error(message: &str)` when `stop_reason == Error`. *(Source: [PS])*
  - Depends on: REQ-081
  - Definition of Done: An error-returning provider invokes the `on_error` callback with the error message string.

---

### Milestone 3.9 — Tool Safety and Edge Cases

- [ ] **REQ-094:** `BashTool`: check each `deny_pattern` against the command (substring match) before execution; return `Err(Failed("Command blocked..."))` on match. *(Source: [PS])*
  - Depends on: REQ-047
  - Definition of Done: A command containing a deny pattern is rejected before any subprocess is spawned.

- [ ] **REQ-095:** `BashTool`: race subprocess completion against a configurable timeout and the cancellation token; on timeout return `Err(Failed("Command timed out after Ns"))`; on cancellation return `Err(Cancelled)`. *(Source: [PS])*
  - Depends on: REQ-047
  - Definition of Done: `sleep 300` with a 2s timeout produces a timeout error; cancellation produces `Cancelled`.

- [ ] **REQ-096:** `BashTool`: truncate `stdout` and `stderr` independently at `max_output_bytes` (default 256KB) and append `"\n... (output truncated)"`. *(Source: [PS])*
  - Depends on: REQ-047
  - Definition of Done: Output exceeding 256KB is truncated with the documented suffix.

- [ ] **REQ-097:** `BashTool`: optional `confirm_fn` callback; if defined and returns `false`, return `Err(Failed("Command was not confirmed by the user."))`. *(Source: [PS])*
  - Depends on: REQ-047
  - Definition of Done: A rejecting `confirm_fn` prevents subprocess execution.

- [ ] **REQ-098:** `ReadFileTool`: check file size before reading. Text files exceeding `max_bytes` (1MB): return `Err(Failed("File too large. Use offset/limit..."))`. Image files exceeding 20MB: return `Err(Failed("Image too large"))`. *(Source: [PS])*
  - Depends on: REQ-048
  - Definition of Done: Reading a file above the size limit returns the documented error without reading the file contents.

- [ ] **REQ-099:** `ReadFileTool`: for image extensions, read file as bytes, base64-encode, detect MIME type from extension, return `Content::Image`. *(Source: [PS])*
  - Depends on: REQ-001, REQ-048
  - Definition of Done: Reading a `.png` file returns a `ToolResult` with `Content::Image { data: base64, mime_type: "image/png" }`.

- [ ] **REQ-100:** `ReadFileTool`: check `ctx.cancel.is_cancelled()` before each I/O operation; return `Err(Cancelled)` if set. *(Source: [PS])*
  - Depends on: REQ-048
  - Definition of Done: Cancelling before a read returns `Cancelled` without touching the file.

- [ ] **REQ-101:** `EditFileTool`: if `old_text` matches zero occurrences, attempt `find_similar_text` for a fuzzy hint; return `Err(Failed("old_text not found... Did you mean: ..."))`. *(Source: [PS])*
  - Depends on: REQ-050
  - Definition of Done: An edit with wrong `old_text` returns a `Failed` error; if a similar line exists, the hint is included.

- [ ] **REQ-102:** `EditFileTool`: if `old_text` matches more than one occurrence, return `Err(Failed("old_text matches N locations. Include more context..."))`. *(Source: [PS])*
  - Depends on: REQ-050
  - Definition of Done: Attempting to replace ambiguous text returns a descriptive error with the match count.

- [ ] **REQ-103:** `EditFileTool`: check `ctx.cancel.is_cancelled()` before each I/O operation. *(Source: [PS])*
  - Depends on: REQ-050
  - Definition of Done: Cancellation before read or write returns `Err(Cancelled)`.

- [ ] **REQ-104:** `WriteFileTool`: check `ctx.cancel.is_cancelled()` before writing. *(Source: [AR])*
  - Depends on: REQ-049
  - Definition of Done: Cancellation prevents the write from occurring.

- [ ] **REQ-105:** `ListFilesTool`: race `find` execution against a timeout (default 10s) and the cancellation token; truncate results at `max_results` (default 200) with a truncation suffix. *(Source: [PS])*
  - Depends on: REQ-051
  - Definition of Done: Listing a directory with 500 files returns 200 with the truncation message.

- [ ] **REQ-106:** `SearchTool`: fall back from `rg` to `grep` if ripgrep is not available on the system. Check `ctx.cancel.is_cancelled()` before execution. *(Source: [PS])*
  - Depends on: REQ-052
  - Definition of Done: Search succeeds on a system without `rg` installed; cancellation is respected.

---

### Milestone 3.10 — Agent Invariants

- [ ] **REQ-107:** In `prompt_messages_with_sender`, assert `!self.is_streaming` with a clear panic message before proceeding. *(Source: [PS])*
  - Depends on: REQ-035
  - Definition of Done: Calling `prompt()` while a run is active panics with a message directing the caller to use `steer()` or `follow_up()`.

- [ ] **REQ-108:** In `agent_loop_continue`, validate preconditions: `context.messages` is non-empty and the last message is not an `Assistant` variant. *(Source: [PS])*
  - Depends on: REQ-037
  - Definition of Done: Calling `agent_loop_continue` with an empty context or with a trailing assistant message returns an error or panics with a clear message.

---

### Milestone 3.11 — Skill System

- [ ] **REQ-109:** Implement `SkillSet::load(dirs: Vec<Path>)`: iterate directories, skip missing ones silently, scan each for subdirectories containing `SKILL.md`, parse frontmatter, build a name-keyed map (later dirs override earlier on collision), return sorted `SkillSet`. *(Source: [PS])*
  - Depends on: REQ-110
  - Definition of Done: Loading two dirs where both contain a skill named `"foo"` results in the second dir's version being used.

- [ ] **REQ-110:** Implement `parse_frontmatter(content) -> (name, description)`: require content to begin with `---`, extract YAML block up to next `\n---`, parse `name:` and `description:` lines, strip surrounding quotes, return `Err(InvalidFrontmatter)` or `Err(MissingField)` on failure. *(Source: [PS])*
  - Depends on: —
  - Definition of Done: Valid frontmatter parses correctly; missing `name` field returns a `MissingField` error; missing delimiters return `InvalidFrontmatter`.

- [ ] **REQ-111:** Implement `SkillSet::format_for_prompt()`: emit `<available_skills>` XML block with one `<skill>` element per skill (sorted by name ascending), XML-escaping all string values; return empty string if no skills loaded. *(Source: [PS])*
  - Depends on: REQ-109
  - Definition of Done: Output is well-formed XML; special characters in skill names/descriptions are correctly escaped.

- [ ] **REQ-112:** Implement `SkillSet::load_dir(dir, source)` and `SkillSet::merge(other)`. *(Source: [AR])*
  - Depends on: REQ-109
  - Definition of Done: `merge` causes the other's skills to override on name conflict.

- [ ] **REQ-113:** Implement `Agent::with_skills(skill_set)`: call `format_for_prompt()` and append the XML block to `self.system_prompt`. *(Source: [PS])*
  - Depends on: REQ-111
  - Definition of Done: After `with_skills(set)`, the agent's system prompt contains the `<available_skills>` XML block.

---

### Milestone 3.12 — MCP Client

- [ ] **REQ-114:** Implement `McpClient::connect_stdio(cmd, args, env)`: spawn subprocess with piped stdin/stdout; complete the 3-step initialize handshake; return `Ok(McpClient)`. *(Source: [PS])*
  - Depends on: REQ-115, REQ-116
  - Definition of Done: Spawning a compliant MCP server subprocess results in a connected client; `server_info` is populated from the handshake.

- [ ] **REQ-115:** Implement `McpClient::send_request(method, params)`: construct a JSON-RPC 2.0 request with auto-incremented atomic ID, send over transport, receive response, return `Err(JsonRpc{...})` on error field or `Err(Protocol("Empty result"))` on missing result. *(Source: [PS])*
  - Depends on: —
  - Definition of Done: A JSON-RPC response with an error field maps to `McpError::JsonRpc`; a valid result field is returned as `Ok(value)`.

- [ ] **REQ-116:** Implement `McpClient::list_tools()` and `McpClient::call_tool(name, args)`. *(Source: [PS])*
  - Depends on: REQ-115
  - Definition of Done: `list_tools()` returns a parsed `Vec<McpToolInfo>`; `call_tool()` returns a parsed `McpToolCallResult`.

- [ ] **REQ-117:** Implement `McpToolAdapter` implementing `AgentTool`: wraps `McpToolInfo` metadata and an `Arc<Mutex<McpClient>>`; `execute()` calls `client.call_tool()` and converts `McpContent` to `Content` variants. *(Source: [AR])*
  - Depends on: REQ-001, REQ-021, REQ-116
  - Definition of Done: An `McpToolAdapter` can be registered on an agent and called successfully in a tool-use turn.

- [ ] **REQ-118:** Handle all `McpError` variants gracefully: `Transport`, `Protocol`, `JsonRpc`, `Serialization`, `Io`, `ConnectionClosed` all surface as `ToolError::Failed` with descriptive messages. *(Source: [AR])*
  - Depends on: REQ-117
  - Definition of Done: Each `McpError` variant produces a non-panicking `ToolError::Failed` with a message identifying the error type and context.

- [ ] **REQ-119:** Implement `Agent::with_mcp_server_stdio(cmd, args, env)`: call `McpClient::connect_stdio`, then `McpToolAdapter::from_client`, append resulting tool adapters to `self.tools`. *(Source: [AR])*
  - Depends on: REQ-114, REQ-117
  - Definition of Done: After `with_mcp_server_stdio`, the agent's tool list includes all tools reported by the MCP server.

***

## Level 4 — Professional
> **Goal:** The system is safe, observable, and maintainable.
> It can be operated with multiple provider backends, supports prompt caching
> and extended thinking, exposes useful observability hooks, and shuts down
> gracefully.

**Completion Criteria:** All 7 provider protocols are implemented. Prompt
caching, thinking levels, structured logging, and security-sensitive fields
are all handled. The cancellation tree propagates correctly to all I/O
boundaries. The system is configurable for production use.

---

### Milestone 4.1 — Full Provider Suite

- [ ] **REQ-120:** Implement `GoogleProvider::stream` (Gemini API): POST to `{base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse&key={API_KEY}`; use custom SSE parser (split on `\n\n`, extract `data:` line); map tool calls from `functionDeclarations`; auto-generate tool IDs as `"google-fc-{index}"`; tool results as `functionResponse` parts. *(Source: [AR])*
  - Depends on: REQ-020
  - Definition of Done: A Gemini streaming response is parsed into the correct `StreamEvent`s; tool IDs are auto-generated in the documented format.

- [ ] **REQ-121:** Implement `GoogleVertexProvider::stream` (Vertex AI): identical wire format to Gemini; endpoint pattern `https://{region}-aiplatform.googleapis.com/...`; auth via `Authorization: Bearer {OAUTH_TOKEN}`; tool IDs as `"vertex-fc-{index}"`. *(Source: [AR])*
  - Depends on: REQ-120
  - Definition of Done: Vertex request differs from Gemini only in endpoint and auth header.

- [ ] **REQ-122:** Implement `BedrockProvider::stream` (ConverseStream API): endpoint `{base_url}/model/{model}/converse-stream`; newline-delimited JSON (not standard SSE); parse events `contentBlockDelta`, `contentBlockStart`, `contentBlockStop`, `messageStop`, `metadata`; tool spec format: `toolSpec { inputSchema: { json: schema } }`; tool result format: `{ toolResult: { toolUseId, content, status } }`. *(Source: [AR])*
  - Depends on: REQ-020
  - Definition of Done: A Bedrock ndjson streaming response is correctly parsed; tool definitions and results are in the Bedrock-specific format.

- [ ] **REQ-123:** Implement `OpenAiResponsesProvider::stream` (OpenAI Responses API): endpoint `{base_url}/responses`; system prompt in `"instructions"` field; SSE events `response.output_text.delta`, `response.reasoning.delta`, `response.function_call_arguments.*`, `response.completed`. *(Source: [AR])*
  - Depends on: REQ-020
  - Definition of Done: The Responses API wire format differs correctly from Chat Completions in system prompt field and event names.

- [ ] **REQ-124:** Implement `AzureOpenAiProvider::stream`: endpoint `{base_url}/responses?api-version=2025-01-01-preview`; auth via `api-key: {AZURE_OPENAI_API_KEY}` header (not `Authorization: Bearer`); same request/response format as OpenAI Responses API. *(Source: [AR])*
  - Depends on: REQ-123
  - Definition of Done: Azure auth uses `api-key` header; base URL pattern `https://{resource}.openai.azure.com/openai/deployments/{deployment}` is supported.

- [ ] **REQ-125:** Register all 7 providers (Anthropic, OpenAiCompat, OpenAiResponses, Azure, Google, Vertex, Bedrock) in `ProviderRegistry::default()`. *(Source: [AR])*
  - Depends on: REQ-042, REQ-120 through REQ-124
  - Definition of Done: `ProviderRegistry::default()` can dispatch to any of the 7 implementations based on protocol selection.

---

### Milestone 4.2 — Prompt Caching

- [ ] **REQ-126:** Implement `CacheStrategy::Auto`: provider automatically places `cache_control: { type: "ephemeral" }` breakpoints at the system prompt, the last tool definition, and the second-to-last message. *(Source: [AR])*
  - Depends on: REQ-014, REQ-040
  - Definition of Done: In Anthropic requests, the three cache breakpoints appear in the correct positions when `strategy: Auto`.

- [ ] **REQ-127:** Implement `CacheStrategy::Manual { cache_system, cache_tools, cache_messages }`: conditionally apply breakpoints per flag. Implement `CacheStrategy::Disabled`: no breakpoints emitted. *(Source: [AR])*
  - Depends on: REQ-126
  - Definition of Done: Each flag independently controls placement of its respective cache breakpoint.

- [ ] **REQ-128:** Propagate `Usage.cache_read` and `Usage.cache_write` from Anthropic response metadata into `Message::Assistant.usage`. *(Source: [AR])*
  - Depends on: REQ-006, REQ-040
  - Definition of Done: Cache token counts from Anthropic are populated in the usage struct after a cached-hit response.

---

### Milestone 4.3 — Extended Thinking

- [ ] **REQ-129:** Map `ThinkingLevel` to Anthropic `thinking` parameter: `Off` → omit; `Minimal` → `budget_tokens: 128`; `Low` → 512; `Medium` → 2048; `High` → 8192. *(Source: [AR])*
  - Depends on: REQ-019, REQ-040
  - Definition of Done: Setting `ThinkingLevel::Medium` causes `{type:"enabled", budget_tokens:2048}` to appear in the Anthropic request.

- [ ] **REQ-130:** Map `ThinkingLevel` to OpenAI-compat `reasoning_effort` parameter when `supports_reasoning_effort` flag is set: `Minimal`/`Low` → `"low"`; `Medium` → `"medium"`; `High` → `"high"`. *(Source: [AR])*
  - Depends on: REQ-019, REQ-041
  - Definition of Done: `ThinkingLevel::High` with a reasoning-capable provider produces `reasoning_effort: "high"` in the request body.

- [ ] **REQ-131:** Parse `Thinking` content blocks from streaming responses (Anthropic `thinking` type blocks; OpenAI `delta.reasoning_content` / xAI `delta.reasoning`); emit as `StreamDelta::Thinking` and store as `Content::Thinking` in the final message. *(Source: [AR])*
  - Depends on: REQ-001, REQ-008, REQ-040
  - Definition of Done: A streaming response containing thinking/reasoning content produces `MessageUpdate` events with `StreamDelta::Thinking` and the final `Content::Thinking` block in the assembled message.

---

### Milestone 4.4 — MCP HTTP Transport

- [ ] **REQ-132:** Implement `McpClient::connect_http(url)`: POST JSON-RPC bodies to the configured URL (stateless, no persistent connection); complete the initialize handshake. *(Source: [AR])*
  - Depends on: REQ-115
  - Definition of Done: An HTTP-based MCP server can be connected to and queried for tools.

- [ ] **REQ-133:** Implement `Agent::with_mcp_server_http(url)` builder. Support optional tool name prefix (`{prefix}__{name}`) for namespace disambiguation. *(Source: [AR])*
  - Depends on: REQ-117, REQ-132
  - Definition of Done: HTTP MCP tools appear in the agent's tool list; with a prefix configured, tool names are formatted as `"{prefix}__{name}"`.

- [ ] **REQ-134:** On MCP stdio transport shutdown, send EOF on stdin then kill the child process. *(Source: [AR])*
  - Depends on: REQ-114
  - Definition of Done: Dropping or closing the stdio MCP client terminates the child process cleanly.

---

### Milestone 4.5 — Observability and Logging

- [ ] **REQ-135:** Implement structured retry logging: when a retry occurs, log attempt number, max retries, delay, and the triggering error at an appropriate log level. *(Source: [PS])*
  - Depends on: REQ-074
  - Definition of Done: A retried request produces a structured log entry containing all four fields.

- [ ] **REQ-136:** Implement `ContextTracker`: combine provider-reported token counts (from `Usage`) with local `estimate_tokens` for messages appended since the last provider report. Expose `current_tokens() -> usize`. *(Source: [AR])*
  - Depends on: REQ-054, REQ-055
  - Definition of Done: After a turn with known provider-reported usage, `current_tokens()` reflects the reported value; after additional messages are appended, it adds heuristic estimates.

- [ ] **REQ-137:** Populate `ToolResult.details` with structured metadata per tool: `BashTool` → `{ exit_code, success }`; `ReadFileTool` → `{ path }`; `WriteFileTool` → `{ path }`; `EditFileTool` → `{ path, old_lines, new_lines }`; `ListFilesTool` → `{ total, truncated }`; `SubAgentTool` → `{ sub_agent, turns }`. *(Source: [AR])*
  - Depends on: REQ-047 through REQ-052
  - Definition of Done: `ToolResult.details` for a bash execution contains `exit_code` and `success` keys.

---

### Milestone 4.6 — Security

- [ ] **REQ-138:** Redact sensitive `OpenApiAuth` credentials in debug output: `Bearer(token)` displays as `Bearer("****")`; `ApiKey { value }` displays as `ApiKey { header: "...", value: "****" }`. *(Source: [AR])*
  - Depends on: —
  - Definition of Done: Printing/logging an `OpenApiAuth::Bearer("secret")` value produces `"****"` instead of the actual token.

- [ ] **REQ-139:** Implement the complete `BashTool` deny-pattern list (configurable; default list to be specified at implementation time based on the safety policy described in the spec). *(Source: [PS])*
  - Depends on: REQ-094
  - Definition of Done: A configurable list of deny patterns is applied; at least the patterns documented in the spec are included in the default list.

---

### Milestone 4.7 — Graceful Cancellation

- [ ] **REQ-140:** Implement `CancellationToken::child_token()`: creates a new token that is cancelled when the parent is cancelled. Each `ToolContext` receives a child token. *(Source: [PS])*
  - Depends on: REQ-033, REQ-046
  - Definition of Done: Calling `agent.abort()` (which cancels the root token) causes all active tool contexts' `cancel.is_cancelled()` to return `true` simultaneously.

- [ ] **REQ-141:** `SubAgentTool` forwards the parent's cancel token to the child `agent_loop()`, so `agent.abort()` terminates sub-agents as well. *(Source: [PS])*
  - Depends on: REQ-033, REQ-140
  - Definition of Done: Aborting the parent agent cancels the sub-agent's run.

---

### Milestone 4.8 — Callbacks and Advanced Configuration

- [ ] **REQ-142:** Implement `on_update` callback in `ToolContext`: when called, emits `AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, partial_result }` to the event channel. *(Source: [AR])*
  - Depends on: REQ-007, REQ-046
  - Definition of Done: A tool that calls `ctx.on_update(partial)` causes `ToolExecutionUpdate` events to appear in the stream before `ToolExecutionEnd`.

- [ ] **REQ-143:** Implement `on_progress` callback in `ToolContext`: when called, emits `AgentEvent::ProgressMessage { tool_call_id, tool_name, text }`. *(Source: [AR])*
  - Depends on: REQ-007, REQ-046
  - Definition of Done: A tool that calls `ctx.on_progress("working...")` causes a `ProgressMessage` event in the stream.

- [ ] **REQ-144:** Implement `Agent::prompt_with_sender(text, tx)`: like `prompt`, but streams events to a caller-provided sender rather than creating a new channel. *(Source: [AR])*
  - Depends on: REQ-034
  - Definition of Done: Events are sent to the provided `tx`; the caller can multiplex one sender across multiple prompts.

- [ ] **REQ-145:** Implement `transform_context` and `convert_to_llm` optional hooks on `AgentLoopConfig`. When set, `stream_assistant_response` calls them to preprocess messages before building `StreamConfig`. *(Source: [PS])*
  - Depends on: REQ-039
  - Definition of Done: A `transform_context` hook that adds a prefix message causes that message to appear in every LLM call.

- [ ] **REQ-146:** Implement `Agent::with_compaction_strategy(strategy)` builder; when set, use the custom `CompactionStrategy` instead of the default 3-tier cascade. *(Source: [AR])*
  - Depends on: REQ-023, REQ-060
  - Definition of Done: A custom strategy that always returns an empty list causes the LLM to be called with no history.

- [ ] **REQ-147:** Define `ModelConfig` struct with fields: `base_url: Option<String>`, `headers: Map<String,String>`, `max_tokens_field: String` (default `"max_tokens"`), `supports_developer_role: bool`, `supports_reasoning_effort: bool`. Apply in `OpenAiCompatProvider`. *(Source: [AR])*
  - Depends on: REQ-041
  - Definition of Done: Setting `max_tokens_field: "max_completion_tokens"` causes the OpenAI provider to use that key in the request body.

***

## Level 5 — Creative
> **Goal:** The system surpasses the original. Sub-agent delegation,
> OpenAPI tool generation, advanced Anthropic protocol features, and all
> documented ambiguities are resolved with principled design decisions.

**Completion Criteria:** `SubAgentTool` works end-to-end; the OpenAPI adapter
generates callable tools from a spec file; all `[AMBIGUOUS]` items have a
documented resolution; performance benchmarks for parallel tool execution
meet or exceed documented expectations.

---

### Milestone 5.1 — Sub-Agent Delegation

- [ ] **REQ-148:** Implement `SubAgentTool::execute`: validate `params["task"]` is non-empty; build a fresh `AgentContext` (empty messages, own toolset); build `AgentLoopConfig` with `max_turns` guard (default 10), no steering/follow-ups, no input filters; spawn child `agent_loop`; await result; call `extract_final_text`. *(Source: [PS])*
  - Depends on: REQ-036, REQ-157
  - Definition of Done: A sub-agent tool registered on a parent agent completes a delegated task and returns the child agent's final text as a `ToolResult`.

- [ ] **REQ-149:** Implement `extract_final_text(messages) -> String`: scan messages in reverse for the last `Assistant` message with `Text` content blocks; join and return them; fall back to `"(sub-agent produced no text output)"`. *(Source: [PS])*
  - Depends on: REQ-002
  - Definition of Done: `extract_final_text` returns the text of the last assistant message; an all-tool-call assistant message returns the fallback string.

- [ ] **REQ-150:** Sub-agent event forwarding: spawn a task to consume child `AgentEvent`s and forward them to parent channel as `ToolExecutionUpdate` (for `MessageUpdate::Text`) and `ProgressMessage` (for child `ProgressMessage`) events. *(Source: [PS])*
  - Depends on: REQ-007, REQ-148
  - Definition of Done: Parent event stream includes `ToolExecutionUpdate` events showing the sub-agent's text generation in real time.

- [ ] **REQ-151:** Implement `SubAgentTool` builder: `SubAgentTool::new(name, provider).with_system_prompt(...).with_model(...).with_api_key(...).with_tools(...).with_max_turns(...).with_thinking_level(...)`. *(Source: [AR])*
  - Depends on: REQ-021, REQ-148
  - Definition of Done: A fully configured `SubAgentTool` can be added to a parent agent's tool list via `with_tools`.

---

### Milestone 5.2 — OpenAPI Adapter (Feature-Gated)

- [ ] **REQ-152:** Implement `OpenApiAdapter::from_str(spec, config, filter)`: auto-detect JSON vs YAML (first non-whitespace char `{` or `[` → JSON, else YAML); parse OpenAPI 3.x spec; resolve base URL; generate one `OpenApiToolAdapter` per matching operation. *(Source: [AR])*
  - Depends on: REQ-153, REQ-154, REQ-155, REQ-156
  - Definition of Done: A valid OpenAPI 3.x spec string (JSON and YAML both) produces one tool adapter per operation with an `operationId`.

- [ ] **REQ-153:** Classify parameters: `path` → URL substitution with RFC 3986 percent-encoding; `query` → query string; `header` → request headers; `cookie` → skip with no error; `requestBody` (application/json only) → keyed as `"body"` (or `"_request_body"` on name collision). *(Source: [AR])*
  - Depends on: REQ-021
  - Definition of Done: Path parameters appear in the URL; query parameters appear in the query string; cookie parameters are silently ignored.

- [ ] **REQ-154:** Implement the HTTP execution pipeline per tool call: validate params, substitute path params, build URL, chain query/header params, apply `OpenApiAuth`, apply `custom_headers`, optionally attach JSON body, send request, read body, truncate at `max_response_bytes` on a UTF-8 boundary, return `"{METHOD} {URL} → {STATUS}\n\n{BODY}"`. *(Source: [AR])*
  - Depends on: REQ-021
  - Definition of Done: A POST to a test endpoint with path, query, and body params produces the documented return format.

- [ ] **REQ-155:** Implement `OperationFilter`: `All` (include everything with an `operationId`); `ByOperationId(ids)` (include only listed IDs); `ByTag(tags)` (include operations tagged with any listed tag); `ByPathPrefix(prefix)` (include operations whose path starts with prefix). Operations without `operationId` always emit a warning and are skipped. *(Source: [AR])*
  - Depends on: REQ-152
  - Definition of Done: Each filter variant correctly includes/excludes operations; an operation without `operationId` logs a warning and is excluded regardless of filter.

- [ ] **REQ-156:** Apply optional `name_prefix` from `OpenApiConfig`: tool name becomes `"{prefix}__{operationId}"` when set. *(Source: [AR])*
  - Depends on: REQ-152
  - Definition of Done: With `name_prefix: Some("myapi")`, the tool for `operationId: "getUser"` is named `"myapi__getUser"`.

- [ ] **REQ-157:** Implement `from_file(path, config, filter)` (async file read) and `from_url(url, config, filter)` (HTTP GET via HTTP client). *(Source: [AR])*
  - Depends on: REQ-152
  - Definition of Done: Both sources produce identical tool lists as `from_str` on the same spec content.

- [ ] **REQ-158:** Implement `Agent::with_openapi_file`, `with_openapi_url`, `with_openapi_spec` builders on `Agent`. Gate the entire `openapi` module behind an `openapi` feature flag. *(Source: [AR])*
  - Depends on: REQ-026, REQ-157
  - Definition of Done: Without the `openapi` feature, the code compiles successfully without the adapter; with it, all three builders are available.

---

### Milestone 5.3 — Advanced Anthropic Protocol

- [ ] **REQ-159:** Implement Anthropic OAuth auth path: when `model_config` indicates OAuth, use `Authorization: Bearer {TOKEN}` header plus beta headers `claude-code-20250219,oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14`, `x-app: cli`, `anthropic-dangerous-direct-browser-access: true`, `user-agent: claude-cli/2.1.2`. *(Source: [AR])*
  - Depends on: REQ-040
  - Definition of Done: An OAuth-configured provider sends all documented headers; standard API key auth sends the standard `x-api-key` header.

- [ ] **REQ-160:** Implement Anthropic `InputJsonDelta` tool-argument streaming: buffer incremental `InputJsonDelta` text fragments in `arguments["__partial_json"]`; parse the complete accumulated string as JSON on `content_block_stop`. *(Source: [AR])*
  - Depends on: REQ-040
  - Definition of Done: A tool call streamed in 5 `InputJsonDelta` fragments produces a single, complete, parseable JSON `arguments` object.

---

### Milestone 5.4 — Ambiguity Resolutions

- [ ] **REQ-161:** [AMBIGUOUS] Standardize `AgentEnd` emission on abort: define and document whether `AgentEnd` is emitted when cancellation is detected at various checkpoints (start of loop, mid-stream, mid-tool). Implement a consistent policy. *(Source: [PS])*
  - Depends on: REQ-067, REQ-082
  - Definition of Done: The chosen policy is documented; behavior is consistent regardless of where in the loop cancellation is detected.

- [ ] **REQ-162:** [AMBIGUOUS] Document heuristic token counting limitations: note that `estimate_tokens` is a 4-char heuristic; define a `TokenCounter` abstraction point that allows a caller to inject a precise counter (e.g., tiktoken integration) without changing the compaction logic. *(Source: [OV])*
  - Depends on: REQ-054
  - Definition of Done: A `TokenCounter` trait or injection point exists; the default implementation uses the 4-char heuristic; a precise implementation can be substituted via configuration.

- [ ] **REQ-163:** [AMBIGUOUS] Define sub-agent error propagation: document what `execute()` returns when the child `agent_loop` produces only error/empty messages. Implement the `extract_final_text` fallback consistently. *(Source: [PS])*
  - Depends on: REQ-149
  - Definition of Done: The policy is documented; child agent error messages are reflected in the fallback text or surfaced as `ToolError::Failed`.

***

## Level 6 — Boss
> **Goal:** The system is exceptional. It is fully tested, scalable,
> developer-friendly, and operates as a platform with a clear public
> API contract and operational runbooks.

**Completion Criteria:** The system passes load tests at 10x expected
tool concurrency. Full test coverage includes unit, integration, property-based,
and end-to-end tests. Public API documentation is complete. Operational
runbooks cover all known failure modes.

---

### Milestone 6.1 — Full Test Suite

- [ ] **REQ-164:** Unit tests for all three compaction levels (`level1`, `level2`, `level3`) including: no-op when under budget; exact budget boundary; message count edge cases (fewer messages than `keep_recent`/`keep_first`); correct ordering of head+marker+tail in level 3. *(Source: [AR])*
  - Depends on: REQ-056 through REQ-059
  - Definition of Done: All edge cases identified above have dedicated test cases that pass.

- [ ] **REQ-165:** Property-based tests for `compact_messages`: for any valid `(messages, config)` input, `total_tokens(compact_messages(messages, config)) <= budget`. *(Source: [AR])*
  - Depends on: REQ-056
  - Definition of Done: 10,000 random test cases all satisfy the budget invariant without panic.

- [ ] **REQ-166:** Unit tests for `delay_for_attempt`: verify exponential growth; verify jitter stays in `[0.8, 1.2]` range over 10,000 samples; verify `max_delay_ms` cap is respected. *(Source: [AR])*
  - Depends on: REQ-071
  - Definition of Done: All three assertions pass across the full retry range.

- [ ] **REQ-167:** Integration tests for each of the 7 provider protocols using a mock HTTP server: correct request format, correct response parsing, correct `StopReason` mapping, correct tool-call extraction. *(Source: [AR])*
  - Depends on: REQ-040 through REQ-042, REQ-120 through REQ-124
  - Definition of Done: Each provider has at least one happy-path integration test and one error-path test using a local mock server.

- [ ] **REQ-168:** Integration test for MCP stdio transport: spawn a minimal mock MCP server subprocess; verify initialize handshake, tool listing, and tool execution. *(Source: [AR])*
  - Depends on: REQ-114 through REQ-119
  - Definition of Done: The mock MCP server can be connected to, queried, and called; all three phases produce correct results.

- [ ] **REQ-169:** End-to-end agent loop tests using `MockProvider`: test single-turn text response; multi-turn tool call cycle; steering injection mid-run; follow-up queue; execution limit enforcement; context compaction trigger; input filter rejection. *(Source: [AR])*
  - Depends on: REQ-036 through REQ-090
  - Definition of Done: All seven scenarios have a passing automated test.

---

### Milestone 6.2 — Load and Scale Testing

- [ ] **REQ-170:** Load test: run 100 parallel agents each with 10 concurrent tool calls using `MockProvider`. Verify no data races, no deadlocks, correct result ordering, no memory leaks. *(Source: [AR])*
  - Depends on: REQ-045, REQ-085
  - Definition of Done: 1,000 total tool calls complete correctly with no panics and tool results are in original call order.

- [ ] **REQ-171:** Load test: run a single agent for 1,000 turns with compaction enabled. Verify token estimates stay bounded; no unbounded memory growth; compaction fires when expected. *(Source: [AR])*
  - Depends on: REQ-056, REQ-060
  - Definition of Done: Memory usage stabilizes after compaction; no messages are dropped that violate `keep_first`/`keep_recent` invariants.

- [ ] **REQ-172:** Memory profile: verify `Agent.messages` does not grow unboundedly in a long conversation with compaction enabled. *(Source: [AR])*
  - Depends on: REQ-056, REQ-060
  - Definition of Done: Message count stays within `keep_first + keep_recent + small_constant` after steady state is reached.

---

### Milestone 6.3 — Public API Contract and Documentation

- [ ] **REQ-173:** Publish complete API reference documentation for all public types, traits, and functions with usage examples for each primary use case from `overview.md`. *(Source: [OV])*
  - Depends on: REQ-001 through REQ-163
  - Definition of Done: A developer with no prior context can build a working coding assistant and CLI REPL from the docs alone.

- [ ] **REQ-174:** Document all 7 provider integration contracts: authentication method, endpoint pattern, request format, response parsing notes, any quirks (e.g., Bedrock ndjson, Google tool ID generation, Azure `api-key` header). *(Source: [AR])*
  - Depends on: REQ-040 through REQ-042, REQ-120 through REQ-124
  - Definition of Done: Each provider has a documentation page listing all fields from the integration contract table.

- [ ] **REQ-175:** Write and publish working example implementations: (1) CLI REPL with `/quit`, `/clear`, `/model` commands; (2) coding assistant with all built-in tools; (3) multi-agent pipeline with `SubAgentTool`. *(Source: [OV])*
  - Depends on: REQ-053, REQ-148
  - Definition of Done: All three examples compile and run end-to-end; the CLI REPL handles all three slash commands.

- [ ] **REQ-176:** Publish AgentSkills standard compliance documentation and MCP integration guide. *(Source: [OV])*
  - Depends on: REQ-109 through REQ-113, REQ-114 through REQ-119
  - Definition of Done: Both guides include a "getting started" section that results in a working integration.

---

### Milestone 6.4 — Developer Tooling and Operational Readiness

- [ ] **REQ-177:** Package and publish the library with proper semantic versioning. The `openapi` feature is opt-in. Document all feature flags. *(Source: [AR])*
  - Depends on: REQ-158
  - Definition of Done: Library installs as a dependency; `openapi` feature is absent from the default build; enabling it adds the adapter without breaking existing code.

- [ ] **REQ-178:** CI pipeline: run unit tests, integration tests (with mock servers), and `openapi`-feature tests on every commit. Gate provider live tests behind API key secrets. *(Source: [AR])*
  - Depends on: REQ-164 through REQ-169
  - Definition of Done: CI passes on every commit; provider live tests run in a separate gated workflow.

- [ ] **REQ-179:** Operational runbook covering: retry tuning (when to adjust `RetryConfig`); context overflow handling (choosing `ContextConfig` values); provider failover (switching providers on persistent failures); MCP server crash recovery; performance profiling guide. *(Source: [AR])*
  - Depends on: REQ-071 through REQ-077
  - Definition of Done: The runbook covers all five topics with actionable decision trees.

***

## Requirement Index

| REQ | Description | Level | Milestone | Source | Depends On |
|-----|-------------|-------|-----------|--------|------------|
| REQ-001 | `Content` enum (Text, Image, Thinking, ToolCall) | 1 | 1.1 | [AR] | — |
| REQ-002 | `Message` enum (User, Assistant, ToolResult) | 1 | 1.1 | [AR] | REQ-001, REQ-005, REQ-006 |
| REQ-003 | `AgentMessage` enum (Llm, Extension) | 1 | 1.1 | [AR] | REQ-002, REQ-004 |
| REQ-004 | `ExtensionMessage` struct | 1 | 1.1 | [AR] | — |
| REQ-005 | `StopReason` enum | 1 | 1.1 | [AR] | — |
| REQ-006 | `Usage` struct with `cache_hit_rate()` | 1 | 1.1 | [AR] | — |
| REQ-007 | `AgentEvent` enum (all variants) | 1 | 1.1 | [AR] | REQ-002, REQ-008 |
| REQ-008 | `StreamDelta` enum | 1 | 1.1 | [AR] | — |
| REQ-009 | `ToolContext` struct | 1 | 1.1 | [AR] | — |
| REQ-010 | `ToolResult` and `ToolError` types | 1 | 1.1 | [AR] | REQ-001 |
| REQ-011 | `ContextConfig` struct with defaults | 1 | 1.1 | [AR] | — |
| REQ-012 | `ExecutionLimits` and `ExecutionTracker` | 1 | 1.1 | [AR] | — |
| REQ-013 | `RetryConfig` with defaults | 1 | 1.1 | [AR] | — |
| REQ-014 | `CacheConfig` and `CacheStrategy` | 1 | 1.1 | [AR] | — |
| REQ-015 | `StreamConfig` struct | 1 | 1.1 | [AR] | REQ-014, REQ-016 |
| REQ-016 | `ToolDefinition` struct | 1 | 1.1 | [AR] | — |
| REQ-017 | `QueueMode` enum | 1 | 1.1 | [AR] | — |
| REQ-018 | Full Serialize/Deserialize on AgentMessage tree | 1 | 1.1 | [OV] | REQ-001–017 |
| REQ-019 | `ThinkingLevel` enum | 1 | 1.1 | [OV] | — |
| REQ-020 | `StreamProvider` trait and `ProviderError` enum | 1 | 1.2 | [AR] | REQ-002, REQ-015 |
| REQ-021 | `AgentTool` trait | 1 | 1.2 | [AR] | REQ-009, REQ-010 |
| REQ-022 | `InputFilter` trait | 1 | 1.2 | [OV] | — |
| REQ-023 | `CompactionStrategy` trait | 1 | 1.2 | [AR] | REQ-003, REQ-011 |
| REQ-024 | `Agent::new()` with all field defaults | 1 | 1.3 | [PS] | REQ-011–017, REQ-019–020 |
| REQ-025 | Builder methods: system_prompt, model, api_key, etc. | 1 | 1.3 | [PS] | REQ-024 |
| REQ-026 | Builder methods: tools, context_config, limits, etc. | 1 | 1.3 | [PS] | REQ-024 |
| REQ-027 | Steering/follow-up queues as Arc<Mutex<Vec>> | 1 | 1.3 | [AR] | REQ-003, REQ-024 |
| REQ-028 | `AgentContext` struct | 1 | 1.4 | [AR] | REQ-003, REQ-021 |
| REQ-029 | `AgentLoopConfig` struct | 1 | 1.4 | [OV] | REQ-011–017, REQ-023 |
| REQ-030 | `MockProvider` implementation | 1 | 1.5 | [AR] | REQ-020 |
| REQ-031 | Smoke test: Agent constructs without error | 1 | 1.5 | [OV] | REQ-024–030 |
| REQ-032 | Unbounded async event channel | 2 | 2.1 | [AR] | REQ-007 |
| REQ-033 | `CancellationToken` with child_token propagation | 2 | 2.1 | [AR] | — |
| REQ-034 | `Agent::prompt()` entry point | 2 | 2.2 | [PS] | REQ-002, REQ-035 |
| REQ-035 | `Agent::prompt_messages_with_sender()` | 2 | 2.2 | [PS] | REQ-027–029, REQ-033, REQ-036 |
| REQ-036 | `agent_loop()` implementation | 2 | 2.3 | [PS] | REQ-032, REQ-037 |
| REQ-037 | `agent_loop_continue()` implementation | 2 | 2.3 | [PS] | REQ-036 |
| REQ-038 | `run_loop()` inner loop (happy path) | 2 | 2.3 | [PS] | REQ-039, REQ-045, REQ-060 |
| REQ-039 | `stream_assistant_response()` (no retry) | 2 | 2.4 | [PS] | REQ-007–008, REQ-015, REQ-020, REQ-032 |
| REQ-040 | `AnthropicProvider::stream()` | 2 | 2.4 | [AR] | REQ-020, REQ-039 |
| REQ-041 | `OpenAiCompatProvider::stream()` | 2 | 2.4 | [AR] | REQ-020, REQ-039 |
| REQ-042 | `ProviderRegistry` with default() | 2 | 2.4 | [AR] | REQ-040, REQ-041 |
| REQ-043 | `StopReason` determination in providers | 2 | 2.4 | [PS] | REQ-005, REQ-040–041 |
| REQ-044 | Filter Extension messages before LLM call | 2 | 2.4 | [AR] | REQ-003, REQ-015 |
| REQ-045 | `execute_tool_calls()` (Parallel dispatch) | 2 | 2.5 | [PS] | REQ-046 |
| REQ-046 | `execute_single_tool()` | 2 | 2.5 | [PS] | REQ-007, REQ-009–010, REQ-021, REQ-033 |
| REQ-047 | `BashTool::execute()` (basic) | 2 | 2.5 | [PS] | REQ-010, REQ-021 |
| REQ-048 | `ReadFileTool::execute()` (basic) | 2 | 2.5 | [PS] | REQ-010, REQ-021 |
| REQ-049 | `WriteFileTool::execute()` | 2 | 2.5 | [AR] | REQ-010, REQ-021 |
| REQ-050 | `EditFileTool::execute()` (basic) | 2 | 2.5 | [PS] | REQ-010, REQ-021 |
| REQ-051 | `ListFilesTool::execute()` (basic) | 2 | 2.5 | [PS] | REQ-010, REQ-021 |
| REQ-052 | `SearchTool::execute()` (basic) | 2 | 2.5 | [PS] | REQ-010, REQ-021 |
| REQ-053 | `default_tools()` returning all 6 tools | 2 | 2.5 | [AR] | REQ-047–052 |
| REQ-054 | `estimate_tokens()` heuristic | 2 | 2.6 | [PS] | — |
| REQ-055 | `content_tokens()` and `message_tokens()` | 2 | 2.6 | [PS] | REQ-001, REQ-003, REQ-054 |
| REQ-056 | `compact_messages()` 3-tier cascade | 2 | 2.6 | [PS] | REQ-055, REQ-057–059 |
| REQ-057 | `level1_truncate_tool_outputs()` | 2 | 2.6 | [PS] | REQ-003, REQ-054 |
| REQ-058 | `level2_summarize_old_turns()` | 2 | 2.6 | [PS] | REQ-003, REQ-054 |
| REQ-059 | `level3_drop_middle()` and `keep_within_budget()` | 2 | 2.6 | [PS] | REQ-003, REQ-054 |
| REQ-060 | Integrate compaction in `run_loop` | 2 | 2.6 | [PS] | REQ-038, REQ-056 |
| REQ-061 | `ExecutionTracker::record_turn()` and `check_limits()` | 2 | 2.7 | [AR] | REQ-012 |
| REQ-062 | Execution limit enforcement in `run_loop` | 2 | 2.7 | [PS] | REQ-038, REQ-061 |
| REQ-063 | `Agent::save_messages()` | 2 | 2.8 | [OV] | REQ-018 |
| REQ-064 | `Agent::restore_messages()` | 2 | 2.8 | [OV] | REQ-018, REQ-063 |
| REQ-065 | `Agent::reset()` | 2 | 2.8 | [AR] | REQ-033 |
| REQ-066 | `Agent::steer()` and `Agent::follow_up()` | 2 | 2.8 | [AR] | REQ-027 |
| REQ-067 | `Agent::abort()` | 2 | 2.8 | [AR] | REQ-033, REQ-035 |
| REQ-068 | Input filter chain execution | 3 | 3.1 | [PS] | REQ-022, REQ-036 |
| REQ-069 | `Reject` → emit `InputRejected` + `AgentEnd([])` | 3 | 3.1 | [PS] | REQ-068 |
| REQ-070 | `Warn` → append warning text to last user message | 3 | 3.1 | [PS] | REQ-068 |
| REQ-071 | `delay_for_attempt()` exponential backoff with jitter | 3 | 3.2 | [PS] | REQ-013 |
| REQ-072 | `is_retryable()` on `ProviderError` | 3 | 3.2 | [AR] | REQ-020 |
| REQ-073 | `retry_after()` on `ProviderError` | 3 | 3.2 | [AR] | REQ-020 |
| REQ-074 | Retry loop in `stream_assistant_response` | 3 | 3.2 | [PS] | REQ-039, REQ-071–073 |
| REQ-075 | `ProviderError::classify()` HTTP status routing | 3 | 3.3 | [PS] | REQ-020 |
| REQ-076 | `is_context_overflow()` phrase matching | 3 | 3.3 | [PS] | — |
| REQ-077 | Context overflow recovery trigger | 3 | 3.3 | [AR] | REQ-056, REQ-075–076 |
| REQ-078 | `ToolError::Failed`/`InvalidArgs` → error ToolResult | 3 | 3.4 | [AR] | REQ-010, REQ-046 |
| REQ-079 | `ToolError::NotFound` → "Tool X not found" | 3 | 3.4 | [PS] | REQ-046 |
| REQ-080 | `ToolError::Cancelled` → "Skipped" ToolResult | 3 | 3.4 | [AR] | REQ-010, REQ-046 |
| REQ-081 | Error stop reason handling in `run_loop` | 3 | 3.5 | [PS] | REQ-038, REQ-082 |
| REQ-082 | Aborted stop reason handling in `run_loop` | 3 | 3.5 | [PS] | REQ-038 |
| REQ-083 | Synthetic error `Message::Assistant` on provider failure | 3 | 3.5 | [PS] | REQ-002, REQ-039 |
| REQ-084 | `execute_sequential()` with steering check | 3 | 3.6 | [PS] | REQ-046, REQ-080 |
| REQ-085 | `execute_batch()` (Parallel) with post-batch steering | 3 | 3.6 | [PS] | REQ-046 |
| REQ-086 | `Batched { size }` dispatch with inter-batch steering | 3 | 3.6 | [PS] | REQ-085 |
| REQ-087 | Drain steering queue at start of outer loop | 3 | 3.7 | [PS] | REQ-038 |
| REQ-088 | Inject steering messages into `pending` after tools | 3 | 3.7 | [PS] | REQ-038, REQ-084–085 |
| REQ-089 | Follow-up queue check re-enters outer loop | 3 | 3.7 | [PS] | REQ-038 |
| REQ-090 | `QueueMode::OneAtATime` and `QueueMode::All` | 3 | 3.7 | [AR] | REQ-017, REQ-027 |
| REQ-091 | `before_turn` callback with abort-if-false | 3 | 3.8 | [PS] | REQ-038 |
| REQ-092 | `after_turn` callback on every turn | 3 | 3.8 | [PS] | REQ-038 |
| REQ-093 | `on_error` callback on Error stop reason | 3 | 3.8 | [PS] | REQ-081 |
| REQ-094 | `BashTool` deny patterns | 3 | 3.9 | [PS] | REQ-047 |
| REQ-095 | `BashTool` timeout + cancellation race | 3 | 3.9 | [PS] | REQ-047 |
| REQ-096 | `BashTool` output truncation | 3 | 3.9 | [PS] | REQ-047 |
| REQ-097 | `BashTool` `confirm_fn` callback | 3 | 3.9 | [PS] | REQ-047 |
| REQ-098 | `ReadFileTool` size limits (1MB text, 20MB image) | 3 | 3.9 | [PS] | REQ-048 |
| REQ-099 | `ReadFileTool` image path (base64, MIME detection) | 3 | 3.9 | [PS] | REQ-001, REQ-048 |
| REQ-100 | `ReadFileTool` cancellation check | 3 | 3.9 | [PS] | REQ-048 |
| REQ-101 | `EditFileTool` zero-match error with fuzzy hint | 3 | 3.9 | [PS] | REQ-050 |
| REQ-102 | `EditFileTool` multiple-match error | 3 | 3.9 | [PS] | REQ-050 |
| REQ-103 | `EditFileTool` cancellation check | 3 | 3.9 | [PS] | REQ-050 |
| REQ-104 | `WriteFileTool` cancellation check | 3 | 3.9 | [AR] | REQ-049 |
| REQ-105 | `ListFilesTool` timeout + max_results truncation | 3 | 3.9 | [PS] | REQ-051 |
| REQ-106 | `SearchTool` rg→grep fallback + cancellation | 3 | 3.9 | [PS] | REQ-052 |
| REQ-107 | `is_streaming` guard in `prompt_messages_with_sender` | 3 | 3.10 | [PS] | REQ-035 |
| REQ-108 | `agent_loop_continue` precondition validation | 3 | 3.10 | [PS] | REQ-037 |
| REQ-109 | `SkillSet::load()` with collision handling | 3 | 3.11 | [PS] | REQ-110 |
| REQ-110 | `parse_frontmatter()` with error variants | 3 | 3.11 | [PS] | — |
| REQ-111 | `SkillSet::format_for_prompt()` XML output | 3 | 3.11 | [PS] | REQ-109 |
| REQ-112 | `SkillSet::load_dir()` and `SkillSet::merge()` | 3 | 3.11 | [AR] | REQ-109 |
| REQ-113 | `Agent::with_skills()` builder | 3 | 3.11 | [PS] | REQ-111 |
| REQ-114 | `McpClient::connect_stdio()` with handshake | 3 | 3.12 | [PS] | REQ-115, REQ-116 |
| REQ-115 | `McpClient::send_request()` JSON-RPC 2.0 | 3 | 3.12 | [PS] | — |
| REQ-116 | `McpClient::list_tools()` and `call_tool()` | 3 | 3.12 | [PS] | REQ-115 |
| REQ-117 | `McpToolAdapter` implementing `AgentTool` | 3 | 3.12 | [AR] | REQ-001, REQ-021, REQ-116 |
| REQ-118 | All `McpError` variants → `ToolError::Failed` | 3 | 3.12 | [AR] | REQ-117 |
| REQ-119 | `Agent::with_mcp_server_stdio()` builder | 3 | 3.12 | [AR] | REQ-114, REQ-117 |
| REQ-120 | `GoogleProvider::stream()` (Gemini API) | 4 | 4.1 | [AR] | REQ-020 |
| REQ-121 | `GoogleVertexProvider::stream()` (Vertex AI) | 4 | 4.1 | [AR] | REQ-120 |
| REQ-122 | `BedrockProvider::stream()` (ConverseStream) | 4 | 4.1 | [AR] | REQ-020 |
| REQ-123 | `OpenAiResponsesProvider::stream()` | 4 | 4.1 | [AR] | REQ-020 |
| REQ-124 | `AzureOpenAiProvider::stream()` | 4 | 4.1 | [AR] | REQ-123 |
| REQ-125 | All 7 providers in `ProviderRegistry::default()` | 4 | 4.1 | [AR] | REQ-042, REQ-120–124 |
| REQ-126 | `CacheStrategy::Auto` breakpoint placement | 4 | 4.2 | [AR] | REQ-014, REQ-040 |
| REQ-127 | `CacheStrategy::Manual` and `Disabled` | 4 | 4.2 | [AR] | REQ-126 |
| REQ-128 | Cache token counts in `Usage` | 4 | 4.2 | [AR] | REQ-006, REQ-040 |
| REQ-129 | `ThinkingLevel` → Anthropic `thinking` parameter | 4 | 4.3 | [AR] | REQ-019, REQ-040 |
| REQ-130 | `ThinkingLevel` → OpenAI `reasoning_effort` | 4 | 4.3 | [AR] | REQ-019, REQ-041 |
| REQ-131 | Parse `Thinking` content from streaming responses | 4 | 4.3 | [AR] | REQ-001, REQ-008, REQ-040 |
| REQ-132 | `McpClient::connect_http()` | 4 | 4.4 | [AR] | REQ-115 |
| REQ-133 | `Agent::with_mcp_server_http()` with prefix support | 4 | 4.4 | [AR] | REQ-117, REQ-132 |
| REQ-134 | MCP stdio shutdown (EOF + kill) | 4 | 4.4 | [AR] | REQ-114 |
| REQ-135 | Structured retry logging | 4 | 4.5 | [PS] | REQ-074 |
| REQ-136 | `ContextTracker` hybrid token tracking | 4 | 4.5 | [AR] | REQ-054–055 |
| REQ-137 | `ToolResult.details` per-tool metadata | 4 | 4.5 | [AR] | REQ-047–052 |
| REQ-138 | `OpenApiAuth` credential redaction in debug | 4 | 4.6 | [AR] | — |
| REQ-139 | `BashTool` default deny-pattern list | 4 | 4.6 | [PS] | REQ-094 |
| REQ-140 | `CancellationToken::child_token()` propagation | 4 | 4.7 | [PS] | REQ-033, REQ-046 |
| REQ-141 | Sub-agent inherits parent cancel token | 4 | 4.7 | [PS] | REQ-033, REQ-140 |
| REQ-142 | `on_update` callback → `ToolExecutionUpdate` event | 4 | 4.8 | [AR] | REQ-007, REQ-046 |
| REQ-143 | `on_progress` callback → `ProgressMessage` event | 4 | 4.8 | [AR] | REQ-007, REQ-046 |
| REQ-144 | `Agent::prompt_with_sender()` | 4 | 4.8 | [AR] | REQ-034 |
| REQ-145 | `transform_context`/`convert_to_llm` hooks | 4 | 4.8 | [PS] | REQ-039 |
| REQ-146 | `Agent::with_compaction_strategy()` builder | 4 | 4.8 | [AR] | REQ-023, REQ-060 |
| REQ-147 | `ModelConfig` struct and application in OpenAiCompat | 4 | 4.8 | [AR] | REQ-041 |
| REQ-148 | `SubAgentTool::execute()` | 5 | 5.1 | [PS] | REQ-036, REQ-157 |
| REQ-149 | `extract_final_text()` | 5 | 5.1 | [PS] | REQ-002 |
| REQ-150 | Sub-agent event forwarding to parent channel | 5 | 5.1 | [PS] | REQ-007, REQ-148 |
| REQ-151 | `SubAgentTool` builder API | 5 | 5.1 | [AR] | REQ-021, REQ-148 |
| REQ-152 | `OpenApiAdapter::from_str()` JSON/YAML parsing | 5 | 5.2 | [AR] | REQ-153–156 |
| REQ-153 | OpenAPI parameter classification | 5 | 5.2 | [AR] | REQ-021 |
| REQ-154 | OpenAPI HTTP execution pipeline | 5 | 5.2 | [AR] | REQ-021 |
| REQ-155 | `OperationFilter` variants | 5 | 5.2 | [AR] | REQ-152 |
| REQ-156 | `name_prefix` tool naming | 5 | 5.2 | [AR] | REQ-152 |
| REQ-157 | `from_file()` and `from_url()` spec sources | 5 | 5.2 | [AR] | REQ-152 |
| REQ-158 | OpenAPI builders on Agent + feature flag | 5 | 5.2 | [AR] | REQ-026, REQ-157 |
| REQ-159 | Anthropic OAuth auth path | 5 | 5.3 | [AR] | REQ-040 |
| REQ-160 | Anthropic `InputJsonDelta` tool-arg streaming | 5 | 5.3 | [AR] | REQ-040 |
| REQ-161 | [AMBIGUOUS] `AgentEnd` on abort policy | 5 | 5.4 | [PS] | REQ-067, REQ-082 |
| REQ-162 | [AMBIGUOUS] `TokenCounter` abstraction point | 5 | 5.4 | [OV] | REQ-054 |
| REQ-163 | [AMBIGUOUS] Sub-agent error propagation policy | 5 | 5.4 | [PS] | REQ-149 |
| REQ-164 | Compaction algorithm unit tests | 6 | 6.1 | [AR] | REQ-056–059 |
| REQ-165 | Property-based tests: budget invariant | 6 | 6.1 | [AR] | REQ-056 |
| REQ-166 | Retry backoff unit tests | 6 | 6.1 | [AR] | REQ-071 |
| REQ-167 | Provider integration tests (mock HTTP server) | 6 | 6.1 | [AR] | REQ-040–042, REQ-120–124 |
| REQ-168 | MCP stdio integration test | 6 | 6.1 | [AR] | REQ-114–119 |
| REQ-169 | End-to-end agent loop tests (MockProvider) | 6 | 6.1 | [AR] | REQ-036–090 |
| REQ-170 | Load test: 100 parallel agents, 10 concurrent tools | 6 | 6.2 | [AR] | REQ-045, REQ-085 |
| REQ-171 | Load test: 1,000-turn single agent with compaction | 6 | 6.2 | [AR] | REQ-056, REQ-060 |
| REQ-172 | Memory profile: message growth is bounded | 6 | 6.2 | [AR] | REQ-056, REQ-060 |
| REQ-173 | Public API reference documentation | 6 | 6.3 | [OV] | REQ-001–163 |
| REQ-174 | Provider integration contract documentation | 6 | 6.3 | [AR] | REQ-040–042, REQ-120–124 |
| REQ-175 | Working example implementations | 6 | 6.3 | [OV] | REQ-053, REQ-148 |
| REQ-176 | AgentSkills + MCP integration guides | 6 | 6.3 | [OV] | REQ-109–119 |
| REQ-177 | Library packaging with feature flags | 6 | 6.4 | [AR] | REQ-158 |
| REQ-178 | CI pipeline with gated live tests | 6 | 6.4 | [AR] | REQ-164–169 |
| REQ-179 | Operational runbooks | 6 | 6.4 | [AR] | REQ-071–077 |

***

## Known Ambiguities

Items marked `[AMBIGUOUS]` in the spec that require a design decision
before implementation:

| ID | Description | Suggested Resolution | Level Introduced |
|----|-------------|----------------------|------------------|
| AMB-001 | `AgentEnd` emission on abort — pseudocode says `AgentEnd` is NOT emitted on abort, but notes this may vary depending on where in the loop cancellation is detected (provider `Start`/`Done` events may still arrive). | Define a clear policy: `AgentEnd` is ALWAYS emitted when the loop exits, including on abort, so callers can rely on the channel always closing cleanly. Gate this by ensuring cancellation detection before the loop attempts to emit `AgentEnd`. | 5 |
| AMB-002 | Token counting precision — `estimate_tokens` uses a 4-chars-per-token heuristic explicitly noted as imprecise. No integration with tiktoken or similar is specified. | Introduce a `TokenCounter` trait (or function pointer) on `ContextConfig` that defaults to the 4-char heuristic but can be overridden by the caller. This keeps the default zero-dependency while enabling precision via injection. | 5 |
| AMB-003 | Sub-agent error propagation — when a child `agent_loop` produces only error or tool-only messages (no `Text` in the final assistant message), `extract_final_text` returns a fixed fallback string. It is unclear whether the calling tool should return `Ok(ToolResult { fallback })` or `Err(ToolError::Failed(...))`. | Return `Ok(ToolResult)` with the fallback text always. If the sub-agent produced an error assistant message, include the `error_message` field in the fallback text so the parent LLM can see and react to it. | 5 |

***

## Level Completion Checklist

- [ ] **Level 1 — Survive:** All core types, traits, and the Agent struct initialize without error; smoke test passes.
- [ ] **Level 2 — Useful:** Text prompt → LLM call → tool execution → final response works end-to-end; all 6 built-in tools execute on valid input; message persistence round-trips correctly.
- [ ] **Level 3 — Smart:** Input filters, retry, provider error classification, tool errors, execution limits, steering/follow-up queues, lifecycle callbacks, tool safety guards, skill loading, and MCP client all handle their error paths without panicking.
- [ ] **Level 4 — Professional:** All 7 provider protocols implemented; prompt caching and extended thinking integrated; cancellation propagates to all I/O; structured logging in place; `ContextTracker` accurate.
- [ ] **Level 5 — Creative:** Sub-agent delegation works end-to-end; OpenAPI adapter generates callable tools; Anthropic OAuth and `InputJsonDelta` streaming are correct; all three ambiguities have documented resolutions and implementations.
- [ ] **Level 6 — Boss:** All test suites pass (unit, property-based, integration, end-to-end, load); public API docs and examples are complete; CI runs automatically; operational runbooks are written.
