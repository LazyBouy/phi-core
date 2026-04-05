<!-- Last verified: 2026-04-05 by Claude Code -->
# Sub-Agents

Sub-agents let a parent agent delegate tasks to child agent loops, each with their own system prompt, tools, and `ModelConfig`. The parent LLM invokes them like any other tool.

## Overview

```
Parent Agent
├── prompt("Research X and implement Y")
│   ├── calls SubAgentTool("researcher", task="Research X")
│   │   └── child agent_loop() with read/search tools → returns findings
│   ├── calls SubAgentTool("coder", task="Implement Y based on findings")
│   │   └── child agent_loop() with edit/write tools → returns result
│   └── summarizes both results
```

Each sub-agent invocation starts a fresh conversation — no state leaks between calls.

## Creating Sub-Agents

```rust
use phi_core::agents::SubAgentTool;
use phi_core::provider::ModelConfig;
use phi_core::tools;
use std::sync::Arc;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();

let researcher = SubAgentTool::new(
    "researcher",
    ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key),
)
.with_description("Searches and reads files to gather information.")
.with_system_prompt("You are a research assistant. Be thorough and concise.")
.with_tools(vec![
    Arc::new(tools::ReadFileTool::new()),
    Arc::new(tools::SearchTool::new()),
])
.with_max_turns(10);
```

## Registering on a Parent Agent

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let mut agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
.with_system_prompt("You coordinate between sub-agents.")
.with_sub_agent(researcher)
.with_sub_agent(coder);
```

The parent sees sub-agents as regular tools. It decides when to delegate based on its system prompt.

## Parallel Execution

When the parent LLM calls multiple sub-agents in a single response, they run concurrently (default `Parallel` strategy). Two sub-agents each taking 50ms complete in ~50ms total, not 100ms.

## Configuration

| Method | Purpose |
|--------|---------|
| `with_description()` | What the parent LLM sees (helps it decide when to delegate) |
| `with_system_prompt()` | The sub-agent's own instructions |
| `with_provider_override(provider)` | Bypass `ProviderRegistry` (primarily for tests) |
| `with_tools()` | Tools available to the sub-agent (accepts `Vec<Arc<dyn AgentTool>>`) |
| `with_max_turns(N)` | Turn limit (default: 10). Primary guard against runaway execution. |
| `with_max_tokens(N)` | Max tokens for LLM responses |
| `with_thinking()` | Enable extended thinking for the sub-agent |
| `with_cache_config()` | Prompt caching settings |
| `with_tool_execution(strategy)` | Tool execution strategy (Parallel, Sequential, Batched) |
| `with_retry_config(config)` | Retry configuration for transient errors |
| `with_parent_loop_id(id: String)` | Sets `parent_loop_id` on the child's `AgentContext`. The child's `AgentStart` event will carry this value, enabling parent→child ancestry tracing across the event stream. |

## Event Forwarding

When the parent provides an `on_update` callback (standard for all tools), sub-agent events are forwarded as `ToolExecutionUpdate` events. The parent's UI sees real-time progress from the child:

- Text deltas from the sub-agent's LLM responses
- Tool call notifications from the sub-agent's tool usage

When the child loop completes, the parent emits `ToolExecutionEnd` with `child_loop_id: Some(loop_id)` set to the child's `loop_id`. This lets you correlate `ToolExecutionEnd` on the parent side with `AgentStart`/`AgentEnd` on the child side when both event streams are consumed.

## Design Decisions

- **Context isolation**: Each invocation starts fresh. Sub-agents don't accumulate history across calls.
- **No nesting**: Sub-agents are not given other `SubAgentTool`s. This prevents infinite delegation chains.
- **Cancellation propagation**: The parent's cancellation token is forwarded. Aborting the parent aborts all sub-agents.
- **Turn limiting**: The default 10-turn limit prevents runaway execution. The parent's execution limits also apply to total wall-clock time.

## Example

See [`examples/sub_agent.rs`](../../examples/sub_agent.rs) for a complete coordinator with researcher and coder sub-agents.
