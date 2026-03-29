# Developer Conceptual Hierarchy

> A developer-facing map of every concept in phi-core, centered on the Agent entity.
> Designed to enable a future UI layer. Every concept is tagged:
> `[EXISTS]` = in code now | `[PLANNED]` = defined but not implemented | `[CONCEPTUAL]` = idea only

---

## The Agent: Three Attributes + Skills

```
                              ┌──────────────────┐
                              │      AGENT       │
                              │   agent_id [E]   │
                              └───────┬──────────┘
                                      │
           ┌──────────────┬───────────┼───────────┬──────────────┐
           │              │           │           │              │
    ┌──────▼──────┐ ┌─────▼─────┐ ┌──▼───┐ ┌────▼─────┐ ┌──────▼──────┐
    │   Profile   │ │ Sessions  │ │Skills│ │   MCP    │ │Introspection│
    │    [C]      │ │   [E]     │ │ [E]  │ │   [E]    │ │    [C]      │
    │ personality │ │  (Tasks)  │ │      │ │connectors│ │   memory    │
    └──────┬──────┘ └─────┬─────┘ └──────┘ └──────────┘ └──────┬──────┘
           │              │                                     │
           │         ┌────▼─────┐                         ┌────▼─────┐
           │         │  Session │                         │  Memory  │
           │         │   [E]    │                         │   [C]    │
           │         └────┬─────┘                         ├──────────┤
           │              │                               │Episodic  │
           │         ┌────▼─────┐                         │Semantic  │
           │         │   Loop   │                         │Procedural│
           │         │   [E]    │                         └──────────┘
           │         └────┬─────┘
           │              │
           │         ┌────▼─────┐
           │         │   Turn   │
           │         │  [E/P]   │
           │         └────┬─────┘
           │              │
           │    ┌─────────┼──────────┐
           │    │         │          │
           │  ┌─▼──┐  ┌──▼───┐  ┌──▼──┐
           │  │Msg │  │ Tool │  │Delta│
           │  │[E] │  │ [E]  │  │ [E] │
           │  └────┘  └──────┘  └─────┘
           │
    ┌──────▼──────────────────────────────────────┐
    │            INDEPENDENT ENTITIES              │
    ├─────────────────────────────────────────────┤
    │  Provider [E]     Event [E]                 │
    │  Message [E]      Compaction [E]            │
    │  Configuration [E]                          │
    │  SystemPromptStrategy [C]                   │
    │  ContextTranslationStrategy [C]             │
    └─────────────────────────────────────────────┘

[E] = EXISTS    [P] = PLANNED    [C] = CONCEPTUAL
```

---

## Model/Provider Fallback Hierarchy

```
Loop model  →  Session model  →  Agent default model
  [EXISTS]       [CONCEPTUAL]       [EXISTS]
```

If the Loop has no model specified, it falls back to the Session's model. If the Session has no model, both fall back to the Agent's default model. This enables mid-session provider switching.

---

## Entity Quick Reference

| Entity | Code Location | Status | Deep Dive |
|--------|--------------|--------|-----------|
| Agent | `agents/basic_agent.rs` | `[EXISTS]` | [agent.md](agent.md) |
| Agent Profile | scattered fields | `[CONCEPTUAL]` as struct | [agent.md](agent.md) |
| Session | `session/model.rs` | `[EXISTS]` | [session.md](session.md) |
| Loop (LoopRecord) | `session/model.rs` | `[EXISTS]` | [loop.md](loop.md) |
| Turn | event-pair TurnStart/End | `[EXISTS]` events; `[PLANNED]` struct | [turn.md](turn.md) |
| Message | `types/content.rs` | `[EXISTS]` | [message.md](message.md) |
| AgentMessage | `types/agent_message.rs` | `[EXISTS]` | [message.md](message.md) |
| Tool | `types/tool.rs` | `[EXISTS]` | [tool.md](tool.md) |
| Provider | `provider/model.rs` | `[EXISTS]` | [provider.md](provider.md) |
| Event | `types/event.rs` | `[EXISTS]` | [event.md](event.md) |
| Compaction | `context/compaction.rs` | `[EXISTS]` | [compaction.md](compaction.md) |
| Configuration | `context/config.rs` + `agent_loop/config.rs` | `[EXISTS]` | [config.md](config.md) |
| SystemPromptStrategy | not in code | `[CONCEPTUAL]` | [agent.md](agent.md) |
| ContextTranslationStrategy | not in code | `[CONCEPTUAL]` | [provider.md](provider.md) |
| Introspection / Memory | not in code | `[CONCEPTUAL]` | [agent.md](agent.md) |
| Permissions | not in code | `[CONCEPTUAL]` | [agent.md](agent.md) |

---

## Callback Ownership

Callbacks live on the entity they observe:

| Callback | Owner | Status |
|----------|-------|--------|
| before_task / after_task | Session | `[CONCEPTUAL]` |
| before_loop / after_loop | Loop | `[EXISTS]` |
| on_error | Loop | `[EXISTS]` |
| before_turn / after_turn | Turn | `[EXISTS]` |
| before_tool_execution / after_tool_execution | Tool | `[EXISTS]` |
| before_tool_execution_update / after_tool_execution_update | Tool | `[EXISTS]` |
| before_compaction_start / after_compaction_end | Compaction | `[CONCEPTUAL]` |

---

## Conceptual vs Code: Key Misalignments

These are places where the conceptual model differs from current code. They represent future refactoring opportunities:

| Concept | Current Code | Conceptual Target |
|---------|-------------|-------------------|
| Agent Profile | Scattered fields on BasicAgent | Dedicated `AgentProfile` struct with `profile_id` |
| thinking_level | On BasicAgent | Should be Session-level (task attribute) |
| temperature | On BasicAgent | Should be Session-level (task attribute) |
| Session model | No model field on Session | Session should carry model override |
| Session scope | Not in code | Ephemeral vs Persistent (Introspection mandatory for Persistent) |
| Turn struct | Implicit (TurnStart/TurnEnd events) | Explicit `Turn` struct in session model |
| SystemPromptStrategy | Static string | Dynamic trait with layered composition |
| Compaction config | Split across ContextConfig + AgentLoopConfig | Single CompactionConfig location |
| before_task / after_task | Not in code (before_loop exists) | Session-level callbacks |
| ContextTranslationStrategy | Not in code | Provider-pair mapping for mid-session switching |
| Introspection | Not in code | Memory extraction with 3 categories (episodic, semantic, procedural) |
| Permissions | Not in code | Include/exclude rules on Agent |

---

## Deep Dive Files

Each entity has its own deep dive document in this folder:

- [agent.md](agent.md) — Agent Profile, Capabilities, Skills, MCP, Permissions, Introspection
- [session.md](session.md) — Session (Task): identity, scope, formation, model, loops, input filters
- [loop.md](loop.md) — Loop (Iteration): model, turns, compaction, parallel groups, callbacks
- [turn.md](turn.md) — Turn (Step): trigger, messages, tool executions, streaming
- [message.md](message.md) — Content, Message, AgentMessage, LlmMessage, ExtensionMessage
- [tool.md](tool.md) — AgentTool trait, ToolContext, execution strategies, callbacks
- [provider.md](provider.md) — ModelConfig, ApiProtocol, registry, ContextTranslationStrategy
- [event.md](event.md) — AgentEvent lifecycle, StreamDelta, event flow
- [compaction.md](compaction.md) — CompactionBlock, strategies, scope, callbacks
- [config.md](config.md) — ContextConfig, ExecutionLimits, CacheConfig, AgentLoopConfig, hooks
