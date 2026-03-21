# phi-core

A Rust library for building stateful, multi-turn AI coding agents. Provides a unified abstraction over 20+ LLM providers, a robust agent loop with tool execution, automatic context management, and real-time event streaming.

## Features

- **Multi-provider support** — Anthropic (Claude), OpenAI, Gemini, Azure OpenAI, AWS Bedrock, Vertex AI, and 15+ OpenAI-compatible backends (Groq, Together, DeepSeek, Mistral, Fireworks, xAI, etc.)
- **Stateful agent loop** — Multi-turn conversation with automatic tool call execution, steering injection, and follow-up queuing
- **Built-in tools** — Bash execution, file read/write/edit, directory listing, and code search
- **Real-time event streaming** — Token-level streaming via async channels
- **Context management** — Tiered compaction strategy to handle large conversations without hitting token limits
- **MCP integration** — Connect to any Model Context Protocol server via stdio or HTTP
- **OpenAPI integration** — Auto-generate tools from any OpenAPI 3.0 spec
- **Sub-agents** — Delegate tasks to isolated child agent instances
- **Skills system** — Load prompt skills from the [AgentSkills](https://agentskills.io) standard
- **Retry logic** — Exponential backoff with jitter for rate limits and network errors
- **Prompt caching** — Anthropic prompt cache support

---

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
phi-core = "0.6"
tokio = { version = "1", features = ["full"] }
```

To enable OpenAPI tool generation:

```toml
[dependencies]
phi-core = { version = "0.6", features = ["openapi"] }
```

**Minimum Supported Rust Version**: 1.75

---

## Quick Start

### Basic prompt

```rust
use phi_core::{Agent, providers::AnthropicProvider};

#[tokio::main]
async fn main() {
    let mut agent = Agent::new(AnthropicProvider)
        .with_model("claude-sonnet-4-20250514")
        .with_api_key(std::env::var("ANTHROPIC_API_KEY").unwrap());

    let mut rx = agent.prompt("What is 2 + 2?").await;

    while let Some(event) = rx.recv().await {
        if let AgentEvent::MessageUpdate { delta: StreamDelta::Text { delta }, .. } = event {
            print!("{}", delta);
        }
    }
}
```

### With built-in tools

```rust
use phi_core::{Agent, providers::AnthropicProvider, tools::default_tools};

let mut agent = Agent::new(AnthropicProvider)
    .with_model("claude-sonnet-4-20250514")
    .with_api_key(std::env::var("ANTHROPIC_API_KEY").unwrap())
    .with_system_prompt("You are a coding assistant with access to the local filesystem.")
    .with_tools(default_tools());

let mut rx = agent.prompt("List the files in the current directory.").await;
```

### Custom tool

```rust
use phi_core::{AgentTool, ToolContext, ToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{Value, json};

struct GreetTool;

#[async_trait]
impl AgentTool for GreetTool {
    fn name(&self) -> &str { "greet" }
    fn label(&self) -> &str { "Greeter" }
    fn description(&self) -> &str { "Greets a person by name." }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Name to greet" }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, params: Value, _ctx: ToolContext) -> Result<ToolResult, ToolError> {
        let name = params["name"].as_str().unwrap_or("world");
        Ok(ToolResult::text(format!("Hello, {}!", name)))
    }
}

let mut agent = Agent::new(AnthropicProvider)
    .with_tools(vec![Box::new(GreetTool)]);
```

---

## Architecture

phi-core is structured in two layers:

```
┌─────────────────────────────────────────────────────────┐
│  Agent (agent.rs)  — stateful wrapper                   │
│  Manages: history, tools, provider, steering queue       │
├─────────────────────────────────────────────────────────┤
│  agent_loop / agent_loop_continue  — stateless functions │
│  Core: stream → tool calls → execute → repeat           │
└─────────────────────────────────────────────────────────┘
```

The **agent loop** is the heartbeat:

1. Send user messages to LLM via `StreamProvider`
2. Stream response tokens in real time, emitting `AgentEvent`s
3. Extract tool calls from the completed response
4. Execute tools (parallel by default), collect results
5. Append tool results to conversation history
6. Repeat until `StopReason::Stop` with no pending follow-ups

### Provider System

All LLM providers implement a single `StreamProvider` trait, dispatched by `ApiProtocol`:

| Provider | Covers |
|---|---|
| `AnthropicProvider` | Claude models |
| `OpenAiCompatProvider` | OpenAI, Groq, Together, DeepSeek, Fireworks, Mistral, xAI, and 15+ more |
| `OpenAiResponsesProvider` | OpenAI Responses API |
| `AzureOpenAiProvider` | Azure OpenAI |
| `GoogleProvider` | Gemini |
| `GoogleVertexProvider` | Vertex AI |
| `BedrockProvider` | AWS Bedrock (ConverseStream) |
| `MockProvider` | Testing |

### Key Types

| Type | Description |
|---|---|
| `Content` | Atomic message unit: `Text`, `Image`, `Thinking`, `ToolCall` |
| `Message` | LLM conversation turn: `User`, `Assistant`, `ToolResult` |
| `AgentMessage` | Routing envelope: `Llm(Message)` or `Extension(...)` (app-only, never sent to LLM) |
| `AgentEvent` | Real-time event stream emitted to callers |
| `StreamDelta` | Token-level streaming updates: `Text`, `Thinking`, `ToolCallDelta` |
| `StopReason` | Why the LLM stopped: `Stop`, `ToolUse`, `Length`, `Error`, `Aborted`, `MaxTurns`, etc. |
| `AgentContext` | Loop execution state: history, tools, system prompt |

---

## Agent API

### Construction

```rust
Agent::new(provider)
    .with_model("claude-sonnet-4-20250514")
    .with_api_key("sk-...")
    .with_system_prompt("You are a helpful assistant.")
    .with_tools(default_tools())
    .with_thinking_level(ThinkingLevel::Medium)
    .with_max_tokens(8192)
    .with_temperature(0.7)
    .with_context_config(ContextConfig { max_context_tokens: 80_000, ..Default::default() })
    .with_execution_limits(ExecutionLimits { max_turns: 30, ..Default::default() })
    .with_retry_config(RetryConfig::default())
    .with_tool_execution(ToolExecutionStrategy::Parallel)
    .with_cache_config(CacheConfig { enabled: true, strategy: CacheStrategy::Auto });
```

### Conversation methods

```rust
// Start a new prompt
let mut rx = agent.prompt("Hello!").await;

// Provide a caller-owned sender (for concurrent use)
agent.prompt_with_sender("Hello!", tx).await;

// Build messages manually
agent.prompt_messages(vec![AgentMessage::Llm(Message::user("Hello!"))]).await;

// Inject a message mid-run (processed before next LLM turn)
agent.steer(message).await;

// Queue a message to send after the agent stops
agent.follow_up(message).await;

// Abort a running loop
agent.abort();

// Reset conversation history
agent.reset();

// Persist and restore conversation state
let json = agent.save_messages();
agent.restore_messages(&json)?;
```

### Integrations

```rust
// MCP servers
agent.with_mcp_server_stdio("npx", &["-y", "@modelcontextprotocol/server-filesystem", "."], None)
agent.with_mcp_server_http("http://localhost:3000")

// OpenAPI tools (requires `openapi` feature)
agent.with_openapi_file(Path::new("api.yaml"), config, &OperationFilter::All)
agent.with_openapi_url("https://api.example.com/openapi.json", config, &OperationFilter::ByTag("pets".into()))

// Skills
let skills = SkillSet::load(&[PathBuf::from("./skills")]);
agent.with_skills(skills)
```

---

## Event Streaming

Consume events from the returned receiver:

```rust
let mut rx = agent.prompt("Write a sorting algorithm.").await;

while let Some(event) = rx.recv().await {
    match event {
        AgentEvent::MessageUpdate { delta: StreamDelta::Text { delta }, .. } => {
            print!("{}", delta);
        }
        AgentEvent::ToolExecutionStart { tool_name, label, .. } => {
            println!("\n[Running: {}]", label);
        }
        AgentEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
            println!("[Done: {} (error: {})]", tool_name, is_error);
        }
        AgentEvent::TurnEnd { usage, .. } => {
            println!("\nTokens used: {}", usage.total_tokens);
        }
        AgentEvent::AgentEnd { messages, .. } => {
            println!("Agent finished with {} new messages", messages.len());
            break;
        }
        _ => {}
    }
}
```

### Full event lifecycle

```
AgentStart
  └─ TurnStart
      ├─ MessageStart
      │   └─ MessageUpdate (repeated per token)
      └─ MessageEnd
  └─ ToolExecutionStart (per tool)
  └─ ToolExecutionUpdate (progress, optional)
  └─ ToolExecutionEnd (per tool)
  └─ TurnEnd
AgentEnd
```

---

## Built-in Tools

All six built-in tools are returned by `default_tools()`:

| Tool | Description |
|---|---|
| `BashTool` | Execute shell commands with timeout and output capture |
| `ReadFileTool` | Read text or image files, with optional line range |
| `WriteFileTool` | Create or overwrite files, creating parent directories as needed |
| `EditFileTool` | Surgical search-and-replace edits |
| `ListFilesTool` | List directory contents with glob filtering |
| `SearchTool` | Grep/ripgrep-based code search |

---

## Context Management

phi-core automatically manages the context window to prevent token limit errors. Configuration:

```rust
ContextConfig {
    max_context_tokens: 100_000,   // Total context budget
    system_prompt_tokens: 4_000,   // Reserved for system prompt
    keep_recent: 10,               // Always keep this many recent messages
    keep_first: 2,                 // Always keep this many initial messages
    tool_output_max_lines: 50,     // Lines per tool output before truncation
}
```

When the budget is approached, compaction runs in tiers:

1. **Level 1** — Truncate long tool outputs
2. **Level 2** — Summarize old conversation turns
3. **Level 3** — Drop middle turns entirely

### Execution limits

```rust
ExecutionLimits {
    max_turns: 50,                  // Maximum LLM calls per run
    max_total_tokens: 1_000_000,    // Total token budget
    max_duration: Duration::from_secs(600),  // Wall-clock timeout
}
```

---

## Tool Execution Strategies

Control how concurrent tool calls are handled:

```rust
// All tools run concurrently (default)
.with_tool_execution(ToolExecutionStrategy::Parallel)

// One tool at a time, checks steering queue between each
.with_tool_execution(ToolExecutionStrategy::Sequential)

// Concurrent within batches, steering check between batches
.with_tool_execution(ToolExecutionStrategy::Batched { size: 4 })
```

---

## Low-level API

For advanced use cases, use the stateless free functions directly:

```rust
use phi_core::agent_loop::{agent_loop, agent_loop_continue, AgentLoopConfig};

let config = AgentLoopConfig::default();
let mut context = AgentContext {
    system_prompt: "You are a helpful assistant.".into(),
    messages: vec![],
    tools: default_tools(),
    ..Default::default()
};

let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
let cancel = CancellationToken::new();

let new_messages = agent_loop(
    vec![AgentMessage::Llm(Message::user("Hello"))],
    &mut context,
    &config,
    tx,
    cancel,
).await;
```

---

## Providers

### Anthropic

```rust
Agent::new(AnthropicProvider)
    .with_model("claude-sonnet-4-20250514")
    .with_api_key(std::env::var("ANTHROPIC_API_KEY").unwrap())
    // Enable extended thinking
    .with_thinking_level(ThinkingLevel::High)
    // Enable prompt caching
    .with_cache_config(CacheConfig { enabled: true, strategy: CacheStrategy::Auto })
```

### OpenAI

```rust
Agent::new(OpenAiCompatProvider)
    .with_model("gpt-4o")
    .with_api_key(std::env::var("OPENAI_API_KEY").unwrap())
```

### OpenAI-compatible (Groq, Together, DeepSeek, etc.)

```rust
Agent::new(OpenAiCompatProvider)
    .with_model("llama-3.3-70b-versatile")
    .with_api_key(std::env::var("GROQ_API_KEY").unwrap())
    .with_model_config(ModelConfig {
        base_url: Some("https://api.groq.com/openai/v1".into()),
        ..Default::default()
    })
```

### Google Gemini

```rust
Agent::new(GoogleProvider)
    .with_model("gemini-2.5-pro")
    .with_api_key(std::env::var("GEMINI_API_KEY").unwrap())
```

### AWS Bedrock

```rust
Agent::new(BedrockProvider)
    .with_model("anthropic.claude-sonnet-4-20250514-v1:0")
    // Uses AWS SDK default credential chain (env vars, ~/.aws/credentials, IAM role, etc.)
```

### Azure OpenAI

```rust
Agent::new(AzureOpenAiProvider)
    .with_model("gpt-4o")
    .with_api_key(std::env::var("AZURE_OPENAI_API_KEY").unwrap())
    .with_model_config(ModelConfig {
        base_url: Some("https://my-resource.openai.azure.com/openai/deployments/my-deployment".into()),
        ..Default::default()
    })
```

---

## MCP Integration

Connect to any [Model Context Protocol](https://modelcontextprotocol.io) server:

```rust
// stdio (local process)
let mut agent = Agent::new(AnthropicProvider)
    .with_mcp_server_stdio(
        "npx",
        &["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"],
        None,
    )
    .await?;

// HTTP (remote server)
let mut agent = Agent::new(AnthropicProvider)
    .with_mcp_server_http("http://localhost:3000")
    .await?;
```

MCP tools are exposed transparently as `AgentTool` instances — the agent loop treats them identically to built-in tools.

---

## OpenAPI Integration

Auto-generate tools from any OpenAPI 3.0 spec (requires `openapi` feature):

```rust
use phi_core::openapi::{OpenApiConfig, OperationFilter};

let mut agent = Agent::new(AnthropicProvider)
    .with_openapi_file(
        Path::new("petstore.yaml"),
        OpenApiConfig { base_url: "https://api.example.com".into(), ..Default::default() },
        &OperationFilter::All,
    )
    .await?;

// Filter to specific operations
.with_openapi_url(
    "https://api.example.com/openapi.json",
    config,
    &OperationFilter::ByTag("pets".into()),
)
```

---

## Sub-agents

Delegate tasks to isolated child agent instances:

```rust
use phi_core::sub_agent::SubAgentTool;

let sub_agent_tool = SubAgentTool::new(
    "researcher",
    "Research a topic and return a summary",
    AnthropicProvider,
    "claude-haiku-4-5",
    api_key,
    default_tools(),
);

let mut agent = Agent::new(AnthropicProvider)
    .with_tools(vec![Box::new(sub_agent_tool)]);
```

Sub-agents get their own isolated conversation context and cannot themselves spawn further sub-agents (depth limiting is enforced automatically).

---

## Skills

Load skills from the [AgentSkills](https://agentskills.io) standard — `SKILL.md` files with YAML frontmatter:

```markdown
---
name: code-review
description: Perform a thorough code review
---

Review the provided code for correctness, performance, security, and style...
```

```rust
let skills = SkillSet::load(&[PathBuf::from("./skills")]);
let mut agent = Agent::new(AnthropicProvider)
    .with_skills(skills);
// Skills are injected as an <available_skills> block in the system prompt
```

---

## Conversation Persistence

Save and restore conversation state across sessions:

```rust
// Save
let json = agent.save_messages();
std::fs::write("conversation.json", &json)?;

// Restore
let json = std::fs::read_to_string("conversation.json")?;
agent.restore_messages(&json)?;
```

---

## Callbacks

Hook into the agent loop with before/after turn callbacks:

```rust
use phi_core::agent_loop::AgentLoopConfig;

let config = AgentLoopConfig {
    before_turn: Some(Arc::new(|ctx| {
        println!("Turn starting, {} messages in history", ctx.messages.len());
        Ok(())
    })),
    after_turn: Some(Arc::new(|ctx, stop_reason| {
        println!("Turn ended: {:?}", stop_reason);
        Ok(())
    })),
    ..Default::default()
};
```

---

## Development

### Build and test

```bash
cargo build
cargo test
cargo test --test agent_loop_test     # Run a specific test file
cargo run --example cli               # Interactive CLI example
cargo run --example basic             # Minimal example
cargo build --features openapi        # Build with OpenAPI support
```

### Linting and formatting

```bash
cargo fmt
cargo clippy --all-targets
```

CI runs with `RUSTFLAGS="-Dwarnings"` — all clippy warnings are treated as errors.

### Integration tests

Integration tests in `tests/integration_anthropic.rs` require a live `ANTHROPIC_API_KEY` and are skipped by default. To run them:

```bash
ANTHROPIC_API_KEY=sk-ant-... cargo test --test integration_anthropic
```

### Examples

| Example | Description |
|---|---|
| `basic.rs` | Minimal text prompt with Anthropic |
| `cli.rs` | Full interactive multi-turn REPL with tools and streaming |
| `callbacks.rs` | Demonstrates `before_turn` / `after_turn` hooks |
| `persistence.rs` | Save and restore conversation history |
| `sub_agent.rs` | Task delegation with `SubAgentTool` |

---

## License

MIT — see [LICENSE](LICENSE) for details.
