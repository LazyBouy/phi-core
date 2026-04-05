<!-- Last verified: 2026-04-05 by Claude Code -->
# phi-core

**Simple, effective agent loop in Rust.**

phi-core is a library for building LLM-powered agents that can use tools. It provides the core loop — prompt the model, execute tool calls, feed results back — and gets out of your way.

## Philosophy

**The loop is the product.** An agent is just a loop: send messages to an LLM, get back text and tool calls, execute the tools, repeat until the model stops. phi-core implements this loop with streaming, cancellation, context management, and multi-provider support — so you don't have to.

## Features

- **Streaming events** — Real-time `AgentEvent` stream for UI updates (text deltas, thinking, tool execution)
- **Multi-provider** — Anthropic, OpenAI, Google Gemini, Amazon Bedrock, Azure OpenAI, and any OpenAI-compatible API
- **Tool system** — `AgentTool` trait with built-in coding tools (bash, file read/write/edit, search)
- **Context management** — Automatic token estimation, tiered compaction (truncate tool outputs → summarize → drop old messages)
- **Execution limits** — Max turns, tokens, and wall-clock time
- **Steering & follow-ups** — Interrupt the agent mid-run or queue work for after it finishes
- **Cancellation** — `CancellationToken`-based abort at any point
- **Builder pattern** — Ergonomic `BasicAgent` struct with chainable configuration; `Agent` trait for polymorphism
- **Config-driven construction** — TOML/JSON/YAML config → `agent_from_config()` → `Arc<dyn Agent>`
- **Session persistence** — `SessionRecorder` materializes structured session/loop/turn records from events
- **Sub-agents** — Delegate tasks to child agent loops via `SubAgentTool`
- **MCP integration** — Connect to external tool servers via Model Context Protocol (stdio + HTTP)
- **Evaluational parallelism** — Run N configs concurrently, select the best result via `EvaluationStrategy`

## Ecosystem

phi-core is part of the [LazyBouy](https://github.com/LazyBouy) ecosystem. It powers the agent backend for Phi applications.

- **Repository:** [github.com/LazyBouy/phi-core](https://github.com/LazyBouy/phi-core)
- **License:** MIT
