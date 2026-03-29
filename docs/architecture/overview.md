# Architecture Overview

> For detailed component specifications, trait signatures, sequence diagrams, and data models,
> see the full [Architecture Spec](../specs/architecture.md).
> For formal algorithm descriptions, see [Algorithms](algorithms.md).

## Layered Design

phi-core is organized as three conceptual layers within a single crate. Dependencies flow strictly downward — upper layers use lower layers, never the reverse.

```
┌─────────────────────────────────────────────┐
│  Layer 3: Orchestration          (planned)   │
│  Multi-agent, delegation, work modes         │
├─────────────────────────────────────────────┤
│  Layer 2: Agent + Providers                  │
│  Concrete providers, tools, retry, caching,  │
│  context management, MCP                     │
├─────────────────────────────────────────────┤
│  Layer 1: Core Loop                          │
│  agent_loop, types, traits                   │
│  Provider-agnostic. Tool-agnostic.           │
└─────────────────────────────────────────────┘
```

### Layer 1: Core Loop

The pure agent loop. No opinions about LLMs, no built-in tools. Just the control flow.

**Modules:** `types/`, `agent_loop/`, `provider/traits.rs`

**Owns:**
- `agent_loop()` / `agent_loop_continue()` — the loop itself
- `AgentTool` trait — interface tools must implement
- `StreamProvider` trait — interface providers must implement
- `AgentMessage`, `AgentEvent`, `StreamDelta` — message & event types
- `AgentContext` — system prompt + messages + tools
- Tool execution strategies (parallel/sequential/batched)
- Streaming tool output (`ToolUpdateFn`)
- Steering & follow-up message injection

**Does not own:** Any concrete provider or tool implementation.

### Layer 2: Agent + Providers

Batteries-included single-agent layer. Most users interact with this.

**Modules:** `agents/`, `context/`, `provider/*.rs`, `tools/*.rs`, `mcp/*.rs`

**Adds on top of Layer 1:**
- Concrete providers — Anthropic, OpenAI-compat, Google, Azure, Bedrock, Vertex
- Provider registry — dispatch by API protocol
- Prompt caching — automatic cache breakpoint placement
- Retry with backoff — exponential, jitter, respects retry-after
- Context management — token estimation, smart truncation, execution limits
- Built-in tools — bash, read_file, write_file, edit_file, list_files, search
- MCP client — stdio + HTTP transports, tool adapter
- `Agent` trait — the runtime interface (prompting, state, control)
- `BasicAgent` struct — default in-memory implementation of `Agent`; stateful builder wrapping it all together
- `SubAgentTool` — delegates tasks to a child `agent_loop()` as a tool

### Layer 3: Orchestration (planned)

Multi-agent coordination. Not yet implemented — the architecture is designed to support it when needed.

**Planned capabilities:**
- `Orchestrator` struct — spawn, delegate, and coordinate multiple agents
- Work modes:
  - **Interactive** — multi-turn, human in the loop (current default)
  - **Autonomous** — runs to completion without input (background tasks, CI)
  - **Pipeline** — input → output, chainable (scan → fix → verify)
  - **Supervisor** — delegates to other agents, synthesizes results
- Fan-out — same task to multiple agents (different providers for diversity)
- Pipeline chaining — output of agent A feeds input of agent B
- Agent communication through the orchestrator event bus

**Why not yet:** Multi-agent orchestration adds complexity. The single-agent loop handles 95% of use cases. Layer 3 will be built when a concrete use case drives it, not speculatively.

---

## Module Layout

```
phi-core/
├── src/
│   ├── lib.rs                  # Public re-exports
│   │
│   │── Layer 1: Core Loop ─────────────────────
│   ├── types/
│   │   ├── mod.rs              # Re-exports, Message, AgentMessage
│   │   ├── content.rs          # Content enum (Text, Image, Thinking, ToolCall)
│   │   ├── extension.rs        # ExtensionMessage
│   │   ├── agent_message.rs    # AgentMessage enum
│   │   ├── usage.rs            # Usage (token metrics)
│   │   ├── tool.rs             # AgentTool trait, ToolDefinition
│   │   ├── event.rs            # AgentEvent enum
│   │   ├── context.rs          # AgentContext
│   │   └── parallel.rs         # ToolExecutionStrategy
│   ├── agent_loop/             # Core loop: prompt → LLM → tools → repeat
│   │
│   │── Layer 2: Agent + Providers ─────────────
│   ├── agents/
│   │   ├── agent.rs            # Agent trait (runtime interface)
│   │   ├── basic_agent.rs      # BasicAgent struct (default in-memory impl)
│   │   └── sub_agent.rs        # SubAgentTool (child agent_loop as a tool)
│   ├── context/                # Token estimation, compaction, limits
│   ├── provider/
│   │   ├── retry.rs            # Retry with exponential backoff
│   │   ├── traits.rs           # StreamProvider trait, StreamEvent, ProviderError
│   │   ├── model.rs            # ModelConfig, ApiProtocol, OpenAiCompat
│   │   ├── registry.rs         # ProviderRegistry (protocol → provider)
│   │   ├── anthropic.rs        # Anthropic Messages API
│   │   ├── openai_compat.rs    # OpenAI Chat Completions (15+ providers)
│   │   ├── openai_responses.rs # OpenAI Responses API
│   │   ├── google.rs           # Google Generative AI
│   │   ├── google_vertex.rs    # Google Vertex AI
│   │   ├── bedrock.rs          # AWS Bedrock ConverseStream
│   │   ├── azure_openai.rs     # Azure OpenAI
│   │   ├── mock.rs             # Mock provider for testing
│   │   └── sse.rs              # SSE utilities
│   ├── tools/
│   │   ├── bash.rs             # BashTool
│   │   ├── file.rs             # ReadFileTool, WriteFileTool
│   │   ├── edit.rs             # EditFileTool
│   │   ├── list.rs             # ListFilesTool
│   │   └── search.rs           # SearchTool
│   └── mcp/
│       ├── client.rs           # MCP client (stdio + HTTP)
│       ├── tool_adapter.rs     # McpToolAdapter (MCP tool → AgentTool)
│       ├── transport.rs        # Transport implementations
│       └── types.rs            # MCP protocol types
```

## Data Flow

```
                    ┌─────────────┐
                    │   Caller    │
                    └──────┬──────┘
                           │ prompt / prompt_messages
                    ┌──────▼──────┐
                    │ BasicAgent  │  Layer 2: stateful wrapper
                    │ (agents/)   │  Manages queues, tools, state
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │ agent_loop  │  Layer 1: core loop
                    │             │  Prompt → LLM → Tools → Repeat
                    └──┬───────┬──┘
                       │       │
              ┌────────▼──┐ ┌──▼────────┐
              │ Provider  │ │   Tools   │  Layer 2: implementations
              │ .stream() │ │ .execute()│
              └────────┬──┘ └──┬────────┘
                       │       │
              ┌────────▼──┐ ┌──▼────────┐
              │ LLM API   │ │ OS / FS   │
              │ (HTTP)    │ │ (shell)   │
              └───────────┘ └───────────┘

Events flow back via mpsc::UnboundedSender<AgentEvent>
```

## How Providers Plug In

1. Implement `StreamProvider` trait (Layer 1 interface)
2. Register with `ProviderRegistry` under an `ApiProtocol` (Layer 2)
3. Set `ModelConfig.api` to match that protocol
4. The registry dispatches `stream()` calls to the right provider

Each provider translates between phi-core's `Message`/`Content` types and the provider's native API format. All providers emit `StreamEvent`s through the channel for real-time updates.

## How Tools Plug In

1. Implement `AgentTool` trait (Layer 1 interface)
2. Add to the tools vec (via `default_tools()` or custom)
3. The agent loop converts tools to `ToolDefinition` (name, description, schema) for the LLM
4. When the LLM returns `Content::ToolCall`, the loop finds the matching tool and calls `execute()`
5. Results are wrapped in `Message::ToolResult` and added to context

Tools receive a `CancellationToken` child token — they should check it for cooperative cancellation during long operations.

## Design Principles

- **Layers are conceptual, not physical.** One crate, clean module boundaries, no feature flags needed.
- **Dependencies flow down.** Layer 1 never imports from Layer 2. Layer 2 never imports from Layer 3.
- **Layer 1 is stable.** The core loop and traits change rarely. New features are added in Layer 2 or 3.
- **Build what's needed.** Layer 3 is designed but not implemented. It will be built when a use case demands it, not speculatively.
- **Simple over clever.** A straightforward loop with good defaults beats an elegant abstraction nobody can debug.

## First Principles: Core vs External

phi-core is a library, not a framework. These principles determine what belongs inside the crate and what should be built on top of it by consumers.

### A feature belongs in phi-core if:

1. **All agents need it** — every consumer would re-implement it independently. The agent loop, message types, event stream, and tool trait are universal primitives.
2. **Requires deep loop integration** — it needs hooks inside the turn cycle that callbacks alone can't provide cleanly. Compaction, execution limits, and streaming are examples.
3. **Defines the contract** — traits and interfaces that standardize how consumers extend the system. `StreamProvider`, `AgentTool`, `CompactionStrategy`, and `InputFilter` are extension contracts.
4. **Fragmentation risk** — if consumers implement it differently, interoperability breaks. Session format, event vocabulary, and message types must be shared.
5. **Cross-cutting** — it touches multiple modules and can't be layered on top without forking the crate.

### A feature should be external if:

1. **Application-specific** — workflows, domain tools, business logic, UI patterns.
2. **Infrastructure** — databases, web servers, authentication, deployment, CI/CD.
3. **Opinionated** — reasonable projects would choose differently. Vector databases, tracing backends, embedding models, and memory strategies are consumer choices.
4. **Implementable via existing extension points** — it can be built cleanly using the traits and callbacks already in core. Permissions (via `InputFilter` + `BeforeToolExecutionFn`), model fallback chains (via custom `StreamProvider`), and observability backends (via `AgentEvent` stream) are examples.
