<!-- Last verified: 2026-04-05 by Claude Code -->
# Algorithms

Formal pseudocode descriptions of phi-core's key algorithms. Each file maps to a specific source module and documents the control flow, invariants, and decision logic.

## Pseudocode Conventions

```
CONVENTIONS:
- FUNCTION name(param: Type) -> ReturnType
- IF / ELSE IF / ELSE / END IF
- FOR EACH item IN collection / END FOR
- WHILE condition / END WHILE
- RETURN value
- MATCH value / CASE pattern → action / END MATCH
- EMIT event(payload)            // send to async event channel
- AWAIT async_operation()        // async call
- LOCK(mutex) / UNLOCK(mutex)    // mutex operations
- SPAWN task                     // launch async task concurrently
- AWAIT_ALL(tasks)               // wait for all concurrent tasks
- SELECT { branch1, branch2 }    // race concurrent futures, take first
- ERROR "message"                // terminal failure path
- [invariant: condition]         // documents what must always be true
- [AMBIGUOUS: reason]            // underdetermined behavior
- → Type                         // return type annotation
- //                             // comment
```

## Index

### Core Loop (`src/agent_loop/`)

| File | Algorithms | Source |
|------|-----------|--------|
| [agent-loop.md](core/agent-loop.md) | `agent_loop`, `agent_loop_continue` | `core.rs` |
| [run-loop.md](core/run-loop.md) | `run_loop` (inner turn engine) | `run.rs` |
| [streaming.md](core/streaming.md) | `stream_assistant_response` | `streaming.rs` |
| [tool-execution.md](core/tool-execution.md) | `execute_tool_calls`, `execute_sequential`, `execute_batch`, `execute_single_tool` | `tools.rs` |

### Context Management (`src/context/`)

| File | Algorithms | Source |
|------|-----------|--------|
| [compaction.md](context/compaction.md) | `compact_messages`, level 1/2/3 strategies, `estimate_tokens` | `compact_messages.rs`, `token.rs` |
| [decision-logic.md](context/decision-logic.md) | Tool execution strategy dispatch, compaction level selection, context overflow detection, input filter chain | `run.rs`, `config.rs`, `traits.rs` |

### Lifecycle & Patterns

| File | Algorithms | Source |
|------|-----------|--------|
| [agent-lifecycle.md](lifecycle/agent-lifecycle.md) | Agent construction, run lifecycle, abort, persistence, `BasicAgent::new`/`prompt` | `basic_agent.rs` |
| [concurrency.md](lifecycle/concurrency.md) | Parallel tool execution, cancellation token propagation, event channel architecture, steering queue thread safety | `tools.rs`, `basic_agent.rs` |

### Providers (`src/provider/`)

| File | Algorithms | Source |
|------|-----------|--------|
| [retry.md](providers/retry.md) | `delay_for_attempt` (exponential backoff) | `retry.rs` |
| [error-classification.md](providers/error-classification.md) | `ProviderError::classify`, StopReason determination, input filter chain | `traits.rs` |
| [sub-agent.md](providers/sub-agent.md) | `SubAgentTool::execute` | `sub_agent.rs` |

### Tools (`src/tools/`)

| File | Algorithms | Source |
|------|-----------|--------|
| [bash.md](tools/bash.md) | `BashTool::execute` | `bash.rs` |
| [file-tools.md](tools/file-tools.md) | `ReadFileTool`, `EditFileTool`, `ListFilesTool`, `SearchTool`, `SkillSet` | `file.rs`, `edit.rs`, `list.rs`, `search.rs`, `skills.rs` |
| [mcp.md](tools/mcp.md) | `McpClient::initialize` | `client.rs` |
| [openapi.md](tools/openapi.md) | `OpenApiToolAdapter::execute` | `adapter.rs` |
