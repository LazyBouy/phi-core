<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
## 5. Concurrency & Async Patterns

### Parallel Tool Execution

```
PATTERN ParallelToolExecution
  // When Parallel strategy is used, all tool calls race concurrently.
  // This is safe because:
  //   1. Tools share no mutable state (each has its own ToolContext)
  //   2. Each ToolContext gets a child cancellation token (same lineage, independent trigger)
  //   3. The event channel (tx) is cloned into each ToolContext — Unbounded sends never block
  //   4. Results are collected in original order via join_all (preserves tool_call ordering)

  futures ← [execute_single_tool(id, name, args) FOR EACH (id, name, args) IN tool_calls]
  results ← AWAIT_ALL(futures)   // futures::join_all — waits for ALL, order preserved
  // Steering is checked AFTER all complete (cannot interrupt mid-batch in Parallel mode)

PATTERN SequentialToolExecution
  // Tools run one at a time; steering is checked after each.
  // Use when tools access shared resources (e.g., same file, same database row).

PATTERN BatchedToolExecution
  // Groups of N run in parallel; steering checked between groups.
  // Balances latency (N concurrent) with control (interrupt between groups).
```

### Cancellation Token Propagation

```
PATTERN CancellationPropagation
  // CancellationToken forms a tree. Cancelling a parent cancels all children.

  Agent.cancel (root token)
    └── AgentLoop cancel (same token passed in)
          └── ToolContext.cancel (child_token() — inherits from parent)
                └── SubAgentTool: forwards parent cancel to child agent_loop()

  // Checks occur at:
  //   - Top of each loop iteration in run_loop (fast path)
  //   - tokio::select! in BashTool (races against timeout)
  //   - Explicit is_cancelled() checks in ReadFileTool, WriteFileTool, EditFileTool

  // Important: abort() on Agent cancels ALL in-progress tool calls simultaneously,
  // regardless of execution strategy.
```

### Event Channel Architecture

```
PATTERN EventChannelArchitecture
  // Single producer (AgentLoop), single consumer (caller).
  // Channel: tokio::mpsc::unbounded_channel — never blocks sender.

  AgentLoop ──tx──→ UnboundedChannel ──rx──→ Application

  // Sub-agent events are NOT directly forwarded to parent channel.
  // SubAgentTool spawns a separate task to translate sub-agent events:
  //   AgentEvent::MessageUpdate(Text(delta)) → on_update(ToolResult{text:delta})
  //   AgentEvent::ProgressMessage{text}      → on_progress(text)
  // These are then emitted to the parent channel as ToolExecutionUpdate/ProgressMessage.

  // This means: parent sees sub-agent activity but via ToolExecutionUpdate wrappers,
  // NOT as nested AgentStart/AgentEnd/TurnStart/TurnEnd events.
```

### Steering Queue Thread Safety

```
PATTERN SteeringQueueSafety
  // steering_queue and follow_up_queue are Arc<Mutex<Vec<AgentMessage>>>.

  // Write path (application thread):
  //   agent.steer(msg)     → LOCK(queue), queue.push(msg), UNLOCK
  //   agent.follow_up(msg) → LOCK(follow_up_queue), queue.push(msg), UNLOCK

  // Read path (agent loop task) — behavior depends on QueueMode:
  //   QueueMode::OneAtATime (default):
  //     LOCK(queue), msg = queue.remove(0), UNLOCK, return [msg]
  //     → delivers exactly one message per check; rest remain for next check
  //   QueueMode::All:
  //     LOCK(queue), msgs = queue.drain_all(), UNLOCK, return msgs
  //     → delivers everything at once

  // Read is called only between tool executions — never concurrently with another read.
  // No deadlock risk: lock is held for microseconds (no I/O inside lock).
  // No data race: Mutex guarantees exclusive access.

  // Queues are passed to AgentLoopConfig as closures capturing the Arc pointer,
  // so the external caller can enqueue messages from any thread at any time.
```

---
