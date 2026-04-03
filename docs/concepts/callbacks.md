# Lifecycle Callbacks

phi-core provides four tiers of lifecycle callbacks that let you observe and control the agent loop without modifying its internals. Loop-level, turn-level, and tool-level callbacks are set on `AgentLoopConfig` (or via `Agent` builder methods). Session-level callbacks (`before_task` / `after_task`) are set on `SessionRecorderConfig`.

## Tiers Overview

| Tier | Hooks | Scope |
|------|-------|-------|
| **Session-level** | `before_task`, `after_task` | Once per session (on `SessionRecorderConfig`) |
| **Loop-level** | `before_loop`, `after_loop` | Once per `agent_loop()` / `agent_loop_continue()` call |
| **Turn-level** | `before_turn`, `after_turn`, `on_error` | Once per LLM call (every turn) |
| **Tool-level** | `before_tool_execution`, `after_tool_execution`, `before_tool_execution_update`, `after_tool_execution_update` | Once per tool call |

---

## Loop-Level Hooks

### `before_loop`

Called once before `AgentStart` is emitted. Receives the current message history and an initial usage counter of 0. Return `false` to abort the entire run — `AgentEnd` is emitted with an empty message list and the loop exits immediately.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_before_loop(|messages, _usage| {
        println!("Starting run with {} existing messages", messages.len());
        true // return false to abort
    });
```

### `after_loop`

Called once after `AgentEnd` is emitted. Receives the new messages produced during the run and the accumulated `Usage` across all turns.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_after_loop(|new_messages, total_usage| {
        println!(
            "Run complete: {} new messages, {} total tokens",
            new_messages.len(),
            total_usage.total_tokens
        );
    });
```

---

## Turn-Level Hooks

### `before_turn`

Called before each LLM call. Receives the current message history and the turn number (0-indexed). Return `false` to abort the loop.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_before_turn(|messages, turn| {
        println!("Turn {} starting with {} messages", turn, messages.len());
        turn < 10 // Stop after 10 turns
    });
```

### `after_turn`

Called after each LLM response and tool execution. Receives the updated message history and the turn's token usage.

```rust
use std::sync::{Arc, Mutex};

let total_cost = Arc::new(Mutex::new(0u64));
let cost_tracker = total_cost.clone();

let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_after_turn(move |_messages, usage| {
        let mut cost = cost_tracker.lock().unwrap();
        *cost += usage.input + usage.output;
        println!("Cumulative tokens: {}", *cost);
    });
```

### `on_error`

Called when the LLM returns a `StopReason::Error`. Receives the error message string.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_error(|err| {
        eprintln!("LLM error: {}", err);
        // Log to monitoring, send alert, etc.
    });
```

---

## Tool-Level Hooks

### `before_tool_execution`

Called before each tool starts, after the `ToolExecutionStart` event would normally emit. Receives the call ID, tool name, and arguments. Return `false` to skip the tool — a `ToolExecutionEnd` with an error result is emitted and the tool's `execute()` is never called.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_before_tool_execution(|call_id, name, _args| {
        println!("About to run tool: {}", name);
        // Return false to block specific tools:
        name != "bash" // block bash, allow everything else
    });
```

### `after_tool_execution`

Called after each tool finishes (after `ToolExecutionEnd` is emitted). Receives the tool name, call ID, and whether the result was an error.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_after_tool_execution(|name, call_id, is_error| {
        if is_error {
            eprintln!("Tool {} ({}) failed", name, call_id);
        }
    });
```

### `before_tool_execution_update`

Called before each `ToolExecutionUpdate` event (streaming progress from a running tool). Return `false` to suppress the event — the tool keeps running and the final `ToolResult` is unaffected; only the intermediate streaming update is dropped.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_before_tool_execution_update(|name, call_id, text| {
        // Only forward updates for bash tool
        name == "bash"
    });
```

### `after_tool_execution_update`

Called after each `ToolExecutionUpdate` event, only if it was not suppressed by `before_tool_execution_update`.

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_after_tool_execution_update(|name, call_id, text| {
        // e.g., log streaming updates to a file
    });
```

---

## Script Callbacks

In addition to Rust closures, callbacks can be implemented as external shell or Python scripts. This allows non-Rust consumers to hook into the agent lifecycle without compiling Rust code.

Script callbacks are specified as command strings (e.g., `"./scripts/on_task_start.sh"` or `"python3 scripts/after_turn.py"`). The agent loop spawns the script as a subprocess, passing relevant context (such as session ID, turn number, or tool name) as environment variables or arguments. The script's exit code determines whether the action proceeds (0 = continue, non-zero = abort, for `Before*` hooks).

Script callbacks can be configured in the `[callbacks]` section of the config file or set programmatically via the `Agent` trait.

All callback tiers are wired in the script callback bridge. Loop-level (`before_loop`, `after_loop`), tool-level (`before_tool_execution`, `after_tool_execution`), compaction-level (`before_compaction_start`, `after_compaction_end`), and turn-level (`before_turn`, `after_turn`) hooks are all resolved from the `[callbacks]` config section and bridged to external scripts. The bridge passes hook context as JSON (message count, turn index, tool name, etc.) via stdin to the subprocess.

---

## Hook Ordering

The hooks fire in strict order relative to their paired events. This ordering is an invariant — it is enforced at runtime:

```
before_loop
  → AgentStart
    before_turn
      → TurnStart
        [MessageStart / MessageUpdate* / MessageEnd]
        [per tool call:]
          before_tool_execution
            → ToolExecutionStart
              (before_tool_execution_update → ToolExecutionUpdate → after_tool_execution_update)*
            ToolExecutionEnd →
          after_tool_execution
      TurnEnd →
    after_turn
  AgentEnd →
after_loop
```

### Short-Circuit Rules

| Hook returns `false` | Effect |
|---|---|
| `before_loop` | Aborts before `AgentStart`; emits `AgentEnd(messages=[])` |
| `before_turn` | Skips turn; neither `TurnStart` nor `TurnEnd` is emitted |
| `before_tool_execution` | Skips tool; emits error `ToolExecutionEnd` without calling `execute()` |
| `before_tool_execution_update` | Suppresses `ToolExecutionUpdate`; tool keeps running; `ToolResult` unaffected |

---

## Steering Checkpoints

Steering messages (injected via the agent's steering queue) are checked at six specific points in the turn cycle. These checkpoints give the caller opportunities to redirect the agent mid-run without waiting for the current loop iteration to complete.

### The Six Checkpoints

1. **Before turn** -- After `before_turn` fires, before the LLM call. The steering message is prepended to the message history as a User message before the model sees it.
2. **After turn** -- After the LLM response is received and `after_turn` fires. Steering is appended before the next turn begins.
3. **Between tool executions (Sequential)** -- When `tool_strategy = "sequential"`, the steering queue is checked between each individual tool call. This is the finest-grained checkpoint.
4. **Between batches (Batched)** -- When `tool_strategy = "batched"`, the steering queue is checked after each batch completes, before the next batch starts.
5. **After all tools (Parallel)** -- When `tool_strategy = "parallel"`, steering is checked once after all tool calls complete. No mid-batch interruption.
6. **On loop re-entry** -- At the top of each loop iteration, before `before_turn` fires.

### Per-Strategy Behavior

| Strategy | When steering is checked | Granularity |
|----------|------------------------|-------------|
| **Sequential** | Between each tool call | Per-tool |
| **Batched** | After each batch completes | Per-batch |
| **Parallel** | After all tools complete | Post-batch |

In all strategies, checkpoints 1, 2, and 6 always apply. The strategy only affects when steering is checked *during* tool execution (checkpoints 3-5).

### Why Mid-Stream and Mid-Tool Steering Is Not Supported

Steering is intentionally not checked:

- **During an LLM streaming response** -- The SSE stream is atomic from the agent loop's perspective. Interrupting a partial response would produce an inconsistent message (partial assistant text with no stop reason). The model's response must complete or fail before steering can take effect.
- **During a single tool's execution** -- A tool call is an atomic unit. Interrupting a bash command mid-execution or a file write mid-stream would leave the environment in an undefined state. The tool must return its `ToolResult` before steering is considered.

These boundaries are not limitations but invariants that keep the message history and environment consistent.

### Hard Abort with CancellationToken

For cases where waiting for the next steering checkpoint is unacceptable (e.g., runaway tool, user-initiated cancel), `CancellationToken` provides a hard abort:

```rust
use tokio_util::sync::CancellationToken;

let cancel = CancellationToken::new();
let cancel_clone = cancel.clone();

// In another task:
cancel_clone.cancel(); // triggers immediate abort
```

When the token is cancelled:
- The current LLM stream is dropped (partial response discarded)
- Running tools are cancelled via their async cancellation
- The loop emits `AgentEnd` with `StopReason::Aborted`
- No further turns or tool calls are attempted

`CancellationToken` is a last resort. Prefer steering for graceful redirection; use cancellation only when the agent must stop immediately.

---

## Combining Callbacks

All callbacks are optional and independent:

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key))
    .on_before_loop(|_msgs, _| true)
    .on_after_loop(|msgs, usage| {
        println!("Done: {} messages, {} tokens", msgs.len(), usage.total_tokens);
    })
    .on_before_turn(|_msgs, turn| turn < 20)
    .on_after_turn(|msgs, usage| {
        println!("Messages: {}, Tokens: {}/{}", msgs.len(), usage.input, usage.output);
    })
    .on_error(|err| eprintln!("Error: {}", err))
    .on_before_tool_execution(|_id, name, _args| {
        println!("Running: {}", name);
        true
    })
    .on_after_tool_execution(|name, _id, is_error| {
        println!("Tool {} finished (error={})", name, is_error);
    });
```

---

## Using with `AgentLoopConfig`

For direct loop usage without the `Agent` wrapper:

```rust
use std::sync::Arc;
use phi_core::agent_loop::AgentLoopConfig;
use phi_core::provider::ModelConfig;

let config = AgentLoopConfig {
    model_config: ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key),
    // Loop-level
    before_loop: Some(Arc::new(|_msgs, _| true)),
    after_loop: Some(Arc::new(|msgs, usage| { /* log */ })),
    // Turn-level
    before_turn: Some(Arc::new(|_msgs, turn| turn < 5)),
    after_turn: Some(Arc::new(|_msgs, _usage| { /* log */ })),
    on_error: Some(Arc::new(|err| eprintln!("{}", err))),
    // Tool-level
    before_tool_execution: Some(Arc::new(|id, name, args| true)),
    after_tool_execution: Some(Arc::new(|name, id, is_error| {})),
    before_tool_execution_update: Some(Arc::new(|name, id, text| true)),
    after_tool_execution_update: Some(Arc::new(|name, id, text| {})),
    ..Default::default()
};
```
