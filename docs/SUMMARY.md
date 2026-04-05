<!-- Last verified: 2026-04-05 by Claude Code -->
# Summary

[Introduction](introduction.md)

# Getting Started

- [Installation](getting-started/installation.md)
- [Quick Start](getting-started/quick-start.md)

# Core Concepts

- [The Agent Loop](concepts/agent-loop.md)
- [Evaluational Parallelism](concepts/evaluational-parallelism.md)
- [Messages & Events](concepts/messages-events.md)
- [Tools](concepts/tools.md)
- [Context Management](concepts/context-management.md)
- [Prompt Caching](concepts/prompt-caching.md)
- [Retry with Backoff](concepts/retry.md)
- [Skills](concepts/skills.md)
- [Sub-Agents](concepts/sub-agents.md)
- [State Persistence](concepts/persistence.md)
- [Lifecycle Callbacks](concepts/callbacks.md)
- [Sessions](concepts/sessions.md)
- [Context Compaction](concepts/compaction.md)
- [Focused Compaction](concepts/focused-compaction.md)
- [Context Translation](concepts/context-translation.md)
- [Context Pruning](concepts/context-pruning.md)

# Guides

- [Configuration](guides/configuration.md)
- [MCP Integration](guides/mcp.md)
- [OpenAPI Tools](guides/openapi.md)

# Providers

- [Overview](providers/overview.md)
- [Anthropic](providers/anthropic.md)
- [OpenAI Compatible](providers/openai-compat.md)
- [Google Gemini](providers/google.md)
- [Amazon Bedrock](providers/bedrock.md)
- [Azure OpenAI](providers/azure-openai.md)

# Reference

- [Built-in Tools](reference/tools.md)
- [Configuration](reference/configuration.md)
- [API Reference](reference/api.md)
- [Glossary & Capabilities](reference/glossary.md)

# Architecture

- [Overview](architecture/overview.md)
- [Algorithms](architecture/algorithms.md)
  - [Agent Loop](architecture/algorithms/core/agent-loop.md)
  - [Run Loop](architecture/algorithms/core/run-loop.md)
  - [Streaming](architecture/algorithms/core/streaming.md)
  - [Tool Execution](architecture/algorithms/core/tool-execution.md)
  - [Compaction](architecture/algorithms/context/compaction.md)
  - [Decision Logic](architecture/algorithms/context/decision-logic.md)
  - [Agent Lifecycle](architecture/algorithms/lifecycle/agent-lifecycle.md)
  - [Concurrency](architecture/algorithms/lifecycle/concurrency.md)
  - [Retry](architecture/algorithms/providers/retry.md)
  - [Error Classification](architecture/algorithms/providers/error-classification.md)
  - [Sub-Agent](architecture/algorithms/providers/sub-agent.md)
  - [Bash Tool](architecture/algorithms/tools/bash.md)
  - [File Tools](architecture/algorithms/tools/file-tools.md)
  - [MCP](architecture/algorithms/tools/mcp.md)
  - [OpenAPI](architecture/algorithms/tools/openapi.md)

# Developer

- [Conceptual Hierarchy](specs/overview.md)
- [Agent](specs/developer/agent.md)
- [Session](specs/developer/session.md)
- [Loop](specs/developer/loop.md)
- [Turn](specs/developer/turn.md)
- [Message](specs/developer/message.md)
- [Tool](specs/developer/tool.md)
- [Provider](specs/developer/provider.md)
- [Event](specs/developer/event.md)
- [Compaction](specs/developer/compaction.md)
- [Configuration](specs/developer/config.md)

# Specs

- [Architecture Spec](specs/architecture.md)
- [Roadmap](specs/roadmap.md)
