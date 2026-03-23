# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**phi-core** is a Rust library (crate) for building AI coding agents. It provides a core agent loop, multi-provider LLM streaming, built-in tools, MCP integration, and context management. Published to crates.io as `phi-core`.

## Build & Development Commands

```bash
cargo build                          # Build the library
cargo test                           # Run all unit tests
cargo test <test_name>               # Run a single test by name
cargo test --test agent_test         # Run a specific test file
cargo fmt                            # Auto-format code
cargo fmt -- --check                 # Check formatting (CI uses this)
cargo clippy --all-targets           # Lint (CI runs with -Dwarnings)
cargo run --example cli              # Run the interactive CLI example
cargo run --example basic            # Run the minimal example
```

CI (`RUSTFLAGS="-Dwarnings"`) treats all clippy warnings as errors. Integration tests in `tests/integration_anthropic.rs` require a live API key and are skipped by default.

## Architecture

### Core Loop Pattern

The central abstraction is a **stateless agent loop** (`agent_loop.rs`) driven by two traits:

- **`StreamProvider`** (`provider/traits.rs`) — streams LLM responses via SSE into an mpsc channel, returning a complete `Message`
- **`AgentTool`** (`types.rs`) — defines tool name/schema/execution; the primary extension point for custom tools

The loop: stream assistant response → extract tool calls → execute tools (parallel by default) → append results → repeat until `StopReason::Stop` with no follow-ups.

`agent_loop` and `agent_loop_continue` are **free functions**, not methods. The `Agent` trait (`agents/agent.rs`) defines the runtime interface — prompting, state access, control, and steering queues. `BasicAgent` (`agents/basic_agent.rs`) is the default in-memory implementation: an optional stateful wrapper that manages message history, tool registry, steering/follow-up queues, and model configuration. The `_with_sender` methods (`prompt_with_sender`, `prompt_messages_with_sender`, `continue_loop_with_sender`) accept a caller-provided `mpsc::UnboundedSender<AgentEvent>` for real-time event consumption on a separate task.

### Provider System

7 provider implementations behind `StreamProvider`, dispatched by `ApiProtocol` enum via `ProviderRegistry`. The caller never names a provider struct — `BasicAgent::new(ModelConfig::anthropic(...))` is the full construction pattern:

| `ApiProtocol` | File | Covers |
|----------|------|--------|
| `AnthropicMessages` | `anthropic.rs` | Claude models |
| `OpenAiCompletions` | `openai_compat.rs` | OpenAI, Groq, Together, DeepSeek, Fireworks, Mistral, xAI, OpenRouter, etc. (15+) |
| `OpenAiResponses` | `openai_responses.rs` | OpenAI Responses API |
| `AzureOpenAiResponses` | `azure_openai.rs` | Azure OpenAI |
| `GoogleGenerativeAi` | `google.rs` | Gemini |
| `GoogleVertex` | `google_vertex.rs` | Vertex AI |
| `BedrockConverseStream` | `bedrock.rs` | Amazon Bedrock (ConverseStream) |

`ModelConfig` (`provider/model.rs`) is the single provider identity card: `id`, `name`, `api`, `provider`, `base_url`, `api_key`, `cost`, `headers`, `compat`. Factory methods: `anthropic()`, `openai()`, `local()`, `google()`, `openrouter()`. The `compat: Option<OpenAiCompat>` field holds per-provider quirk flags for the OpenAI-compat providers (auth style, reasoning format, max_tokens field name, etc.). `provider_override: Option<Arc<dyn StreamProvider>>` (skipped by serde) is an escape hatch for test injection or custom providers.

### Key Types

- **`Content`** — enum: `Text`, `Image`, `Thinking`, `ToolCall`
- **`Message`** — enum: `User`, `Assistant`, `ToolResult` — each variant carries its own fields
- **`AgentMessage`** — `Llm(Message)` | `Extension(ExtensionMessage)` — extension messages (`role`, `kind`, `data`) don't enter LLM context
- **`AgentEvent`** — full event stream emitted to callers: `AgentStart`, `TurnStart`, `MessageStart/Update/End`, `ToolExecutionStart/Update/End`, `ProgressMessage`, `InputRejected`, `TurnEnd`, `AgentEnd`
- **`StopReason`** — `Stop`, `Length`, `ToolUse`, `Error`, `Aborted`
- **`Usage`** — token metrics per turn or accumulated: `input`, `output`, `reasoning` (subset of `output`; non-zero for OpenAI o-series only), `cache_read`, `cache_write`, `total_tokens`. `estimated_cost(&CostConfig)` computes dollar cost. Carried directly on `TurnEnd.usage` and `AgentEnd.usage` (no message destructuring needed).

### Context Management (`context.rs`)

- **`ContextTracker`** — hybrid real-usage + estimation for token tracking
- **`compact_messages()`** — tiered compaction: Level 1 (truncate tool outputs) → Level 2 (summarize old turns) → Level 3 (drop middle turns)
- **`ExecutionLimits`/`ExecutionTracker`** — max turns (50), max tokens (1M), max duration (10 min). Cost tracking is automatic: `Usage::estimated_cost(&model_config.cost)` fires after each turn when rates are non-zero (set `model_config.cost` fields)

### Tool Execution (`agent_loop.rs`)

`ToolExecutionStrategy` controls concurrency:
- `Parallel` (default) — `futures::join_all` for all tool calls
- `Sequential` — one at a time, checks steering queue between each
- `Batched { size }` — concurrent within batch, steering check between batches

### OpenAPI Integration (`openapi/`, feature-gated)

Behind the `openapi` Cargo feature. `OpenApiToolAdapter` parses an OpenAPI 3.0 spec and creates one `AgentTool` per operation. Factory methods: `from_str`, `from_file`, `from_url`, `from_spec`. `OperationFilter` controls which operations become tools. Added to `Agent` via `with_openapi_file()` / `with_openapi_url()` / `with_openapi_spec()`.

### MCP Integration (`mcp/`)

`McpClient` communicates via `McpTransport` trait (stdio or HTTP). `McpToolAdapter` wraps MCP tools to implement `AgentTool`, making them transparent to the agent loop. Added via `Agent::with_mcp_server_stdio()` / `with_mcp_server_http()`.

### Testing

All unit tests use `MockProvider` (`provider/mock.rs`) to simulate LLM responses without network. Test files are in `tests/` — `agent_test.rs`, `agent_loop_test.rs`, `tools_test.rs`. Construct with:
```rust
let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
    .with_provider_override(Arc::new(MockProvider::texts(vec!["response"])));
```
`with_provider_override()` bypasses `ProviderRegistry` dispatch and uses the supplied provider directly.

## Key Design Conventions

- Context overflow detection is centralized in `OVERFLOW_PHRASES` (`provider/traits.rs`) covering 15+ provider-specific error strings; both HTTP errors and SSE-embedded errors are classified
- Tools return stdout/stderr even on failure so the LLM can self-correct
- Retry logic (`retry.rs`) uses exponential backoff with ±20% jitter; only retries `RateLimited` and `Network` errors
- The `skills.rs` module loads `<name>/SKILL.md` files with YAML frontmatter per the AgentSkills standard
- Lifecycle callbacks have three tiers: turn-level (`BeforeTurnFn`/`AfterTurnFn`/`OnErrorFn`), loop-level (`BeforeLoopFn`/`AfterLoopFn` — fire before `AgentStart` / after `AgentEnd`), and tool-level (`BeforeToolExecutionFn`/`AfterToolExecutionFn` — fire around each `ToolExecutionStart`/`ToolExecutionEnd`); `BeforeToolExecutionUpdateFn`/`AfterToolExecutionUpdateFn` additionally wrap each `ToolExecutionUpdate` event. Returning `false` from any `Before*` hook short-circuits the corresponding action. Hook ordering is strictly enforced — hooks fire before their paired event is emitted.
