# phi-core — Project Overview

## 1. Purpose Statement

`phi-core` is a Rust async library for building stateful, multi-turn LLM agents that can autonomously execute tools to accomplish tasks. The library solves the core engineering problems of agent construction: routing between many LLM provider APIs through a unified interface, running a prompt-then-tool-call loop until the model signals completion, streaming real-time events to UI consumers, and automatically managing context windows so conversations do not exceed model token limits. It is designed to be embedded as a dependency in application code — it provides no standalone binary, no HTTP server, and no user interface of its own.

## 2. Key Capabilities

| Capability | Source Location |
|---|---|
| Multi-turn conversation loop (prompt → LLM → tool call → repeat) | `src/agent_loop/` |
| Support for 20+ LLM providers via 7 distinct API protocols | `src/provider/` |
| Real-time event streaming over an async channel | `src/types/` (`AgentEvent`), `src/agent_loop/` |
| Parallel, sequential, or batched tool execution | `src/agent_loop/:execute_tool_calls()` |
| Context compaction via CompactionBlock overlays (legacy: tiered compact_messages()) | `src/context/` — compaction is now modeled via `CompactionBlock` |
| Built-in coding tools: bash execution, file read/write/edit, directory listing, grep search | `src/tools/` |
| Sub-agent delegation: run an isolated child agent as a tool | `src/agents/sub_agent.rs` |
| Model Context Protocol (MCP) client for stdio and HTTP tool servers | `src/mcp/` |
| AgentSkills system: load instruction sets from directory-based skill files | `src/context/skills.rs` |
| OpenAPI tool auto-generation from spec files or URLs (optional feature) | `src/openapi/` |
| JSON serialization of entire conversation history for persistence | `src/types/` (all types derive `Serialize`/`Deserialize`) |
| Exponential-backoff retry for rate-limit and network errors | `src/provider/retry.rs` |
| Prompt caching hints for compatible providers (Anthropic) | `src/types/` (`CacheConfig`) |
| Extended thinking / reasoning mode | `src/types/` (`ThinkingLevel`) |
| Lifecycle callbacks: before/after each turn, on error | `src/agent_loop/` (`BeforeTurnFn`, `AfterTurnFn`, `OnErrorFn`) |
| Loop-level hooks: setup/teardown around each complete agent run | `src/agent_loop/` (`BeforeLoopFn`, `AfterLoopFn`) |
| Tool-level hooks: intercept each tool execution and streaming update | `src/agent_loop/` (`BeforeToolExecutionFn`, `AfterToolExecutionFn`, `BeforeToolExecutionUpdateFn`, `AfterToolExecutionUpdateFn`) |
| Agent identity: stable `agent_id` / `session_id` / `loop_id` for cross-loop traceability | `src/agents/basic_agent.rs`, `src/types/` |
| Evaluational parallelism: `agent_loop_parallel()` runs N `AgentLoopConfig`s concurrently on the same prompt, evaluates results via the pluggable `EvaluationStrategy` trait, and delivers the best outcome. Built-in strategies: `TransparentEvaluation`, `PickFirstEvaluation`, `TokenEfficientEvaluation`, `ElaborateEvaluation`, `LlmJudgeEvaluation` (with iterative compaction to satisfy judge's comprehension criteria). `ParallelLoopStart`/`ParallelLoopEnd` events bracket execution. Session continuity: `selected_context` feeds directly into `agent_loop_continue()`. | `src/agent_loop/` (`agent_loop_parallel`), `src/agent_loop/evaluation.rs`, `src/types/` |
| Continuation kinds: `Rerun` and `Branch` variants for retry vs. explore semantics | `src/types/` (`ContinuationKind`), `src/agent_loop/` |
| Input filtering: moderation, PII redaction, injection detection | `src/types/` (`InputFilter`) |
| User steering mid-run: inject messages between tool calls | `src/agents/basic_agent.rs` (steering queue), `src/agent_loop/` |
| Follow-up work queuing: append more tasks after agent would stop | `src/agents/basic_agent.rs` (follow-up queue), `src/agent_loop/` |
| Execution limits: max turns, max total tokens, max duration | `src/context/` (`ExecutionLimits`, `ExecutionTracker`) |

## 3. Inputs & Outputs

### Inputs

| Input | Format | Description |
|---|---|---|
| User prompt | `Vec<AgentMessage>` or `String` | Text (or multi-content) messages to start or continue a conversation |
| System prompt | `String` | Instruction set defining agent behavior, injected at each LLM call |
| Tool definitions | `Vec<Box<dyn AgentTool>>` | Executable tools exposed to the LLM via JSON Schema |
| LLM provider config | `ModelConfig` | Single provider identity card: `id`, `api_key`, `base_url`, `api: ApiProtocol`, `cost`, `compat`. Factory methods: `ModelConfig::anthropic()`, `::openai()`, `::local()`, `::google()`, `::openrouter()`. Pass to `BasicAgent::new()` or `AgentLoopConfig.model_config`. |
| Steering messages | `Vec<AgentMessage>` via queue | User-injected messages that interrupt mid-run tool execution |
| Follow-up messages | `Vec<AgentMessage>` via queue | Queued tasks appended when the agent would otherwise stop |
| Context config | `ContextConfig` | Token budget, compaction parameters |
| Execution limits | `ExecutionLimits` | Max turns, tokens, duration |
| Skill directories | `Vec<Path>` | Directories containing `SKILL.md` files |
| MCP server commands | Command string, args, env | Stdio or HTTP MCP server specifications |
| OpenAPI spec | File path, URL, or YAML/JSON string | API specs to auto-generate tools from |
| Cancellation token | `CancellationToken` | External abort signal |

### Outputs

| Output | Format | Description |
|---|---|---|
| Agent event stream | `UnboundedReceiver<AgentEvent>` | Real-time stream of all events (text deltas, tool calls, results, errors) |
| Final messages | `Vec<AgentMessage>` | All new messages produced in the run (returned from `agent_loop()`) |
| Serialized conversation | JSON | Complete message history, serializable for persistence |
| Tool results | Embedded in `AgentEvent::ToolExecutionEnd` | Structured result of each tool call |
| Usage statistics | `Usage` struct per turn | Input/output/cache token counts per LLM call |

## 4. Actors & Use Cases

### Application Developer
The primary consumer. Embeds `phi-core` as a library dependency.

| Use Case | How Triggered |
|---|---|
| Build a coding assistant | Create `Agent`, attach built-in tools, call `agent.prompt("...")` |
| Build a CLI REPL | Loop reading stdin, call `agent.prompt()`, render events (see `examples/cli.rs`) |
| Persist conversation across sessions | Call `agent.save_messages()` → JSON → `agent.restore_messages()` |
| Run a task autonomously with limits | Set `ExecutionLimits`, observe `AgentEvent::AgentEnd` |
| Interrupt a running agent | Call `agent.steer(message)` while event loop is running |
| Chain specialized agents | Attach `SubAgentTool` instances to a parent agent |
| Use third-party tools | Connect to an MCP server via `agent.with_mcp_server_stdio()` |
| Expose a REST API as tools | Load OpenAPI spec via `agent.with_openapi_file()` |

### End User (via application)
Interacts through the application wrapping this library. Uses cases match what the application exposes (e.g., CLI prompts in `examples/cli.rs`: `/quit`, `/clear`, `/model`).

### LLM Provider
External service receiving structured HTTP requests. The library sends conversation history and tool schemas; the provider returns streaming token deltas and final messages. Providers never call back into the library.

### MCP Server
External process exposing tools over the Model Context Protocol. The library connects as a client via stdio pipe or HTTP. The server exposes tool definitions that are adapted into `AgentTool` instances.

### Sub-Agent
A child instance of the agent loop spawned internally when a `SubAgentTool` is called. Operates with its own fresh context and toolset. Results are returned to the parent as a `ToolResult`.

## 5. Constraints & Non-Goals

- **No built-in HTTP server.** The library is embeddable only; serving the agent over HTTP requires external frameworks.
- **No user interface.** UI rendering (text display, color, input handling) is the application's responsibility (see `examples/cli.rs` for a reference implementation).
- **No authentication management.** API keys must be supplied by the caller. The library does not fetch, rotate, or cache credentials.
- **Single event consumer per run.** `agent_loop()` returns a single `UnboundedReceiver<AgentEvent>`. Fan-out to multiple consumers requires application-level bridging.
- **No agent-to-agent networking.** Sub-agents run in-process only. No remote agent delegation.
- **No persistent storage.** Conversation state is held in memory. Serialization to disk is the caller's responsibility (the library provides `serialize`/`deserialize` helpers).
- **No token counting via external libraries.** Token estimation uses the fast heuristic of 4 characters per token (`src/context/:estimate_tokens()`). Precision counting (via tiktoken etc.) is a non-goal.
- **No multi-modal generation.** Images can be sent *to* the model (as `Content::Image`), but image *generation* is not supported.
- **No structured output / JSON mode.** The library passes raw messages; enforcing structured output is the caller's responsibility via system prompt.
- **Skipped tools on steering.** When steering messages arrive mid-batch, remaining tool calls in that batch are skipped with an error result — their outputs are never computed. This is a documented behavior, not a bug.

## 6. Key Terminology Glossary

| Term | Definition |
|---|---|
| **Agent** | The runtime interface trait (`src/agents/agent.rs`). Programs against this trait to remain independent of the specific implementation. `BasicAgent` (`src/agents/basic_agent.rs`) is the default in-memory implementation: owns conversation history, tools, `ModelConfig` (provider identity + auth + cost), and configuration. Construction: `BasicAgent::new(ModelConfig::anthropic(...))`. The application-facing entry point. |
| **Agent Loop** | The recursive execution cycle (`src/agent_loop/`) that calls the LLM, processes tool calls, checks steering, and repeats until the LLM stops or limits are hit. |
| **Turn** | One complete LLM call plus the resulting tool executions. Bounded by `TurnStart`/`TurnEnd` events. |
| **Steering** | A `Vec<AgentMessage>` injected into the running loop between tool executions. Used to redirect the agent mid-task without restarting it. |
| **Follow-up** | A `Vec<AgentMessage>` queued to be injected after the agent would naturally stop. Extends the run without creating a new `agent_loop()` call. |
| **ModelConfig** | The single, complete description of a provider connection (`src/provider/model.rs`). Fields: `id` (model name sent to API), `name` (display label), `api: ApiProtocol` (wire-protocol dispatch key), `provider` (logging label), `base_url`, `api_key`, `cost: CostConfig`, `headers`, `compat: Option<OpenAiCompat>`. Factory methods: `anthropic()`, `openai()`, `local()`, `google()`, `openrouter()`. Passed to `BasicAgent::new()`, `SubAgentTool::new()`, and `AgentLoopConfig.model_config`. |
| **ApiProtocol** | Enum that selects which HTTP wire format to use: `AnthropicMessages`, `OpenAiCompletions`, `OpenAiResponses`, `AzureOpenAiResponses`, `GoogleGenerativeAi`, `GoogleVertex`, `BedrockConverseStream`. Used by `ProviderRegistry` as a dispatch key. |
| **StreamProvider** | The trait (`src/provider/traits.rs`) that any LLM backend must implement. Has a single method `stream()` that takes a `StreamConfig` and sends `StreamEvent`s. |
| **AgentTool** | The trait (`src/types/`) that any executable tool must implement. Methods: `name()`, `label()`, `description()`, `parameters_schema()`, `execute()`. |
| **ToolContext** | A struct passed to `AgentTool::execute()` containing the call ID, name, cancellation token, and optional progress callbacks. |
| **AgentEvent** | The streaming event enum emitted to the consumer during a run. Covers agent lifecycle, turn lifecycle, message streaming, and tool execution. |
| **StreamDelta** | A partial content update emitted during LLM streaming: `Text`, `Thinking`, or `ToolCallDelta`. |
| **StopReason** | Why the LLM ended its response: `Stop` (natural end), `Length` (token limit), `ToolUse` (returned tool calls), `Error` (failure), `Aborted` (cancellation). |
| **AgentMessage** | The top-level message enum stored in the conversation history. Either `Llm(LlmMessage)` (sent to the LLM; LlmMessage wraps Message + optional TurnId for turn tracking) or `Extension(ExtensionMessage)` (app-only metadata). |
| **Message** | The LLM-protocol message enum: `User`, `Assistant`, or `ToolResult`. |
| **Content** | A single content block within a message: `Text`, `Image` (base64), `Thinking`, or `ToolCall`. |
| **Usage** | Token count metadata returned with each `Assistant` message: `input`, `output`, `cache_read`, `cache_write`, `total_tokens`. |
| **ContextConfig** | Configuration for the automatic context compaction: token budget, lines-to-keep per tool output, number of recent/first messages to preserve. |
| **CompactionStrategy** | A trait for customizing how messages are compacted when the token budget is exceeded. The default implementation uses 3 tiers. |
| **CompactionBlock** | The model used by the compaction system to represent compacted message regions. Replaces the previous inline approach in `compact_messages()` with a structured block-based representation. |
| **ExecutionLimits** | Hard caps on agent execution: `max_turns`, `max_total_tokens`, `max_duration`. When exceeded, the loop appends a system message and stops. |
| **ToolExecutionStrategy** | How multiple tool calls from one LLM response are dispatched: `Sequential`, `Parallel` (default), or `Batched { size }`. |
| **CacheConfig** / **CacheStrategy** | Controls prompt caching breakpoint placement for providers that support it (Anthropic). Strategies: `Auto`, `Disabled`, `Manual`. |
| **ThinkingLevel** | Controls extended reasoning depth: `Off`, `Minimal`, `Low`, `Medium`, `High`. Translated to provider-specific parameters. |
| **AgentSkills** | A directory-based system for loading instruction files (`SKILL.md`) that extend agent capabilities. Compatible with the AgentSkills open standard. |
| **MCP** | Model Context Protocol. A standard for tool servers that communicate over stdio or HTTP. The library acts as an MCP client. |
| **SubAgentTool** | An `AgentTool` implementation that, when called by the parent LLM, spawns a complete child `agent_loop()` with isolated context. |
| **InputFilter** | A synchronous trait applied to user text before the LLM call. Returns `Pass`, `Warn(text)` (appended to message), or `Reject(reason)` (aborts run). |
| **ExtensionMessage** | An `AgentMessage` variant that is not sent to the LLM. Used for application-specific metadata (UI state, notifications) stored in conversation history. |
| **ContextTracker** | Tracks context token usage using a hybrid of real provider-reported counts and local heuristic estimates for messages since the last report. |
| **ProviderError** | The error enum returned by `StreamProvider::stream()`. Variants: `Api`, `Network`, `Auth`, `RateLimited`, `ContextOverflow`, `Cancelled`, `Other`. |
| **ToolDefinition** | A schema-only description of a tool sent to the LLM (name, description, JSON Schema parameters). Does not include the `execute` function. |
| **RetryConfig** | Exponential-backoff configuration for retrying `RateLimited` and `Network` provider errors. |
| **AgentLoopConfig** | A flat configuration struct passed to `agent_loop()` / `agent_loop_continue()` bundling all behavioral settings. Required field: `model_config: ModelConfig` (provider identity, auth, cost rates). Optional `provider_override: Option<Arc<dyn StreamProvider>>` bypasses registry dispatch (used in tests). |
| **QueueMode** | Controls how queued messages (steering/follow-ups) are consumed per read. `OneAtATime` (default): pops only the first queued message. `All`: drains the entire queue at once. |
| **McpContent** | A content item returned by an MCP tool call. Variants: `Text { text }` and `Image { data: base64, mimeType }`. |
| **OpenApiAuth** | Authentication method for OpenAPI requests. Variants: `None`, `Bearer(token)`, `ApiKey { header, value }`. Token/value is redacted in debug output. |
| **OperationFilter** | Controls which OpenAPI operations become tools. Variants: `All`, `ByOperationId`, `ByTag`, `ByPathPrefix`. Operations without an `operationId` are always skipped. |
| **agent_id** | A UUID v4 string generated once when `Agent::new()` is called. Stable for the lifetime of the `Agent` instance. Included in every `AgentStart` event to identify which agent produced the run. |
| **session_id** | A UUID v4 string generated once when `Agent::new()` is called. Groups all loops (origin + continuations) that belong to one logical session. Stable for the lifetime of the `Agent` instance. |
| **loop_id** | A string of the form `"{session_id}.{config_id}.{N}"` that uniquely identifies one `agent_loop` / `agent_loop_continue` call. The `config_id` segment is either caller-supplied or auto-derived from provider + model + thinking level. `N` is a per-`config_id` monotonic counter. Included in every `AgentStart` event. |
| **ContinuationKind** | Labels how an `agent_loop_continue` call relates to prior loops. Set on `AgentContext.continuation_kind` before calling. Variants: `Default` (unspecified continuation), `Rerun { tag }` (retry the same scenario from an equivalent context), `Branch { tag }` (explore a different execution path). Tags are RFC 3339 UTC timestamps. Surfaced in `AgentStart.continuation_kind`. |
| **TurnTrigger** | Identifies what caused a turn to begin. Emitted in `TurnStart.triggered_by`. Variants: `User` (first turn of an origin `agent_loop` call), `SubAgent` (running as a sub-agent via `SubAgentTool`), `FollowUp` (subsequent turns, tool round-trips, Default/Rerun continuations, and steering-injected turns), `Branch` (first turn of a `ContinuationKind::Branch` continuation). |
| **BeforeLoopFn** / **AfterLoopFn** | Loop-level lifecycle hooks on `AgentLoopConfig`. `BeforeLoopFn` fires before `AgentStart` — return `false` to abort the run before it begins. `AfterLoopFn` fires after `AgentEnd` with the new messages and accumulated usage. |
| **BeforeToolExecutionFn** / **AfterToolExecutionFn** | Tool-level lifecycle hooks on `AgentLoopConfig`. `BeforeToolExecutionFn` fires before `ToolExecutionStart` — return `false` to skip the tool call. `AfterToolExecutionFn` fires after `ToolExecutionEnd` with the tool name, call ID, and error flag. |
| **BeforeToolExecutionUpdateFn** / **AfterToolExecutionUpdateFn** | Streaming tool update hooks on `AgentLoopConfig`. Fire around each `ToolExecutionUpdate` event emitted when a tool calls `ctx.on_update(partial)`. `BeforeToolExecutionUpdateFn` returns `false` to suppress the event (tool keeps running; final `ToolResult` is unaffected). `AfterToolExecutionUpdateFn` fires after the event if not suppressed. |
