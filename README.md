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
phi-core = "0.7"
tokio = { version = "1", features = ["full"] }
```

To enable OpenAPI tool generation:

```toml
[dependencies]
phi-core = { version = "0.7", features = ["openapi"] }
```

**Minimum Supported Rust Version**: 1.75

---

## Quick Start

### Basic prompt

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;
use phi_core::{AgentEvent, StreamDelta};

#[tokio::main]
async fn main() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
    let mut agent = BasicAgent::new(ModelConfig::anthropic(
        "claude-sonnet-4-20250514",
        "Claude Sonnet 4",
        &api_key,
    ));

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
use phi_core::{BasicAgent, tools::default_tools};
use phi_core::provider::ModelConfig;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let mut agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
.with_system_prompt("You are a coding assistant with access to the local filesystem.")
.with_tools(default_tools());

let mut rx = agent.prompt("List the files in the current directory.").await;
```

### Custom tool

```rust
use phi_core::{BasicAgent, AgentTool, ToolContext, ToolResult, ToolError};
use phi_core::provider::ModelConfig;
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

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let mut agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
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

All providers are selected by `ModelConfig.api: ApiProtocol` and resolved automatically via
`ProviderRegistry`. You never name a provider struct directly — just pass a `ModelConfig`:

| `ApiProtocol` variant | Wire format | Factory method |
|---|---|---|
| `AnthropicMessages` | Anthropic Messages API | `ModelConfig::anthropic(id, name, key)` |
| `OpenAiCompletions` | OpenAI Chat Completions (15+ backends) | `ModelConfig::openai(id, name, key)` / `ModelConfig::local(url, id, key)` / `ModelConfig::openrouter(id, key)` |
| `OpenAiResponses` | OpenAI Responses API | Direct struct construction |
| `AzureOpenAiResponses` | Azure OpenAI | Direct struct construction |
| `GoogleGenerativeAi` | Gemini | `ModelConfig::google(id, name, key)` |
| `GoogleVertex` | Vertex AI | Direct struct construction |
| `BedrockConverseStream` | AWS Bedrock | Direct struct construction |

`OpenAiCompat` flags handle the 15+ OpenAI-compatible provider quirks (auth style, reasoning
format, `max_tokens` field name, etc.) without needing a separate provider per service.

### Key Types

| Type | Description |
|---|---|
| `Content` | Atomic message unit: `Text`, `Image`, `Thinking`, `ToolCall` |
| `Message` | LLM conversation turn: `User`, `Assistant`, `ToolResult` |
| `AgentMessage` | Routing envelope: `Llm(LlmMessage)` or `Extension(...)` (app-only, never sent to LLM). LlmMessage wraps Message + optional TurnId for turn tracking |
| `AgentEvent` | Real-time event stream emitted to callers |
| `StreamDelta` | Token-level streaming updates: `Text`, `Thinking`, `ToolCallDelta` |
| `StopReason` | Why the LLM stopped: `Stop`, `ToolUse`, `Length`, `Error`, `Aborted`, `MaxTurns`, etc. |
| `AgentContext` | Loop execution state: history, tools, system prompt |

---

## Agent API

### Construction

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let mut agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
.with_system_prompt("You are a helpful assistant.")
.with_tools(default_tools())
.with_thinking(ThinkingLevel::Medium)
.with_max_tokens(8192)
.with_context_config(ContextConfig { max_context_tokens: 80_000, ..Default::default() })
.with_execution_limits(ExecutionLimits { max_turns: 30, ..Default::default() })
.with_retry_config(RetryConfig::default())
.with_tool_execution(ToolExecutionStrategy::Parallel)
.with_cache_config(CacheConfig { enabled: true, strategy: CacheStrategy::Auto });

// Temperature is a public field (no builder method):
agent.temperature = Some(0.7);
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

The modern system uses non-destructive CompactionBlock overlays — see docs/concepts/compaction.md for the current design.

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
use phi_core::agent_loop::{agent_loop, AgentLoopConfig};
use phi_core::provider::ModelConfig;
use phi_core::{AgentContext, AgentMessage, Message, tools::default_tools};

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let config = AgentLoopConfig {
    model_config: ModelConfig::anthropic(
        "claude-sonnet-4-20250514",
        "Claude Sonnet 4",
        &api_key,
    ),
    ..Default::default()
};

let mut context = AgentContext {
    system_prompt: "You are a helpful assistant.".into(),
    messages: vec![],
    tools: default_tools(),
    ..Default::default()
};

let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
let cancel = tokio_util::sync::CancellationToken::new();

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

`ModelConfig` is the single descriptor for every provider connection — it bundles the model ID,
API key, base URL, and any per-provider quirk flags. Pass it to `BasicAgent::new()` or
`SubAgentTool::new()`.

### Anthropic

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
// Enable extended thinking
.with_thinking(ThinkingLevel::High)
// Enable prompt caching
.with_cache_config(CacheConfig { enabled: true, strategy: CacheStrategy::Auto })
```

### OpenAI

```rust
use phi_core::provider::ModelConfig;

let api_key = std::env::var("OPENAI_API_KEY").unwrap();
BasicAgent::new(ModelConfig::openai("gpt-4o", "GPT-4o", &api_key))
```

### OpenAI-compatible (Groq, Together, DeepSeek, etc.)

```rust
use phi_core::provider::{ModelConfig, OpenAiCompat};

// Groq — pass the base URL via ModelConfig::local()
let api_key = std::env::var("GROQ_API_KEY").unwrap();
BasicAgent::new(ModelConfig::local(
    "https://api.groq.com/openai/v1",
    "llama-3.3-70b-versatile",
    &api_key,
))

// OpenRouter — dedicated factory with correct compat flags
let or_key = std::env::var("OPENROUTER_API_KEY").unwrap();
BasicAgent::new(ModelConfig::openrouter("anthropic/claude-sonnet-4", &or_key))
```

### Google Gemini

```rust
use phi_core::provider::ModelConfig;

let api_key = std::env::var("GEMINI_API_KEY").unwrap();
BasicAgent::new(ModelConfig::google("gemini-2.5-pro", "Gemini 2.5 Pro", &api_key))
```

### AWS Bedrock

```rust
use phi_core::provider::{ModelConfig, ApiProtocol};

// Bedrock uses "access_key:secret[:session_token]" as api_key, or "" for IAM roles
let creds = std::env::var("AWS_BEDROCK_CREDENTIALS").unwrap_or_default();
BasicAgent::new(ModelConfig {
    id: "anthropic.claude-sonnet-4-20250514-v1:0".into(),
    name: "Claude Sonnet 4 (Bedrock)".into(),
    api: ApiProtocol::BedrockConverseStream,
    provider: "bedrock".into(),
    base_url: "us-east-1".into(), // AWS region
    api_key: creds,
    ..Default::default()
})
```

### Azure OpenAI

```rust
use phi_core::provider::{ModelConfig, ApiProtocol};

let api_key = std::env::var("AZURE_OPENAI_API_KEY").unwrap();
BasicAgent::new(ModelConfig {
    id: "gpt-4o".into(),
    name: "GPT-4o (Azure)".into(),
    api: ApiProtocol::AzureOpenAiResponses,
    provider: "azure_openai".into(),
    base_url: "https://my-resource.openai.azure.com/openai/deployments/my-deployment".into(),
    api_key,
    ..Default::default()
})
```

---

## MCP Integration

Connect to any [Model Context Protocol](https://modelcontextprotocol.io) server:

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let model_config = ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key);

// stdio (local process)
let mut agent = BasicAgent::new(model_config.clone())
    .with_mcp_server_stdio(
        "npx",
        &["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"],
        None,
    )
    .await?;

// HTTP (remote server)
let mut agent = BasicAgent::new(model_config)
    .with_mcp_server_http("http://localhost:3000")
    .await?;
```

MCP tools are exposed transparently as `AgentTool` instances — the agent loop treats them identically to built-in tools.

---

## OpenAPI Integration

Auto-generate tools from any OpenAPI 3.0 spec (requires `openapi` feature):

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;
use phi_core::openapi::{OpenApiConfig, OperationFilter};

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let mut agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
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
use phi_core::BasicAgent;
use phi_core::agents::SubAgentTool;
use phi_core::provider::ModelConfig;
use phi_core::tools::default_tools;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();

let researcher = SubAgentTool::new(
    "researcher",
    ModelConfig::anthropic("claude-haiku-4-5-20251001", "Claude Haiku", &api_key),
)
.with_description("Research a topic and return a summary")
.with_tools(
    default_tools()
        .into_iter()
        .map(|t| std::sync::Arc::from(t))
        .collect(),
);

let mut agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
.with_sub_agent(researcher);
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
use phi_core::{BasicAgent, SkillSet};
use phi_core::provider::ModelConfig;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let skills = SkillSet::load(&[PathBuf::from("./skills")]);
let mut agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
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

Hook into the agent loop with before/after turn callbacks via the builder API:

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
.on_before_turn(|messages, turn_index| {
    println!("Turn {} starting, {} messages in history", turn_index, messages.len());
    true // return false to abort the turn
})
.on_after_turn(|messages, usage| {
    println!("Turn ended. Tokens used: {}", usage.total_tokens);
});
```

For the low-level API, callbacks live on `AgentLoopConfig`:

```rust
use phi_core::agent_loop::AgentLoopConfig;
use std::sync::Arc;

let config = AgentLoopConfig {
    before_turn: Some(Arc::new(|messages, turn_index| {
        println!("Turn {} starting", turn_index);
        true
    })),
    after_turn: Some(Arc::new(|messages, usage| {
        println!("Turn ended: {} tokens", usage.total_tokens);
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

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release notes. Latest: **0.7.0** —
hardening + ergonomics (per-tool timeouts, structured-output contract,
credential refresh, pluggable `SessionStore`, MCP transport timeouts,
poison-tolerant queues). One breaking change to `Agent::build_config()`.

---

## License

MIT — see [LICENSE](LICENSE) for details.
