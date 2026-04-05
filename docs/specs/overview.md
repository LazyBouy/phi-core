<!-- Last verified: 2026-04-05 by Claude Code -->
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
    │    [E]      │ │   [E]     │ │ [E]  │ │   [E]    │ │    [C]      │
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
           │         │   [E]    │
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
    │  SystemPromptStrategy [E]                   │
    │  ContextTranslationStrategy [E]             │
    └─────────────────────────────────────────────┘

[E] = EXISTS    [P] = PLANNED    [C] = CONCEPTUAL
```

---

## Model/Provider Fallback Hierarchy

```
Loop model (LoopConfigSnapshot)  →  Agent default model
         [EXISTS]                       [EXISTS]
```

Each loop captures its model config in `LoopConfigSnapshot` at `AgentStart` time. Session-level model override has been removed; the fallback is directly to the Agent's default model.

---

## Entity Quick Reference

| Entity | Code Location | Status | Deep Dive |
|--------|--------------|--------|-----------|
| Agent | `agents/basic_agent.rs` | `[EXISTS]` | [agent.md](agent.md) |
| Agent Profile | `agents/profile.rs` | `[EXISTS]` | [agent.md](agent.md) |
| Session | `session/model.rs` | `[EXISTS]` | [session.md](session.md) |
| Loop (LoopRecord) | `session/model.rs` | `[EXISTS]` | [loop.md](loop.md) |
| Turn | `session/model.rs` + event-pair | `[EXISTS]` events; `[EXISTS]` struct | [turn.md](turn.md) |
| Message | `types/content.rs` | `[EXISTS]` | [message.md](message.md) |
| AgentMessage | `types/agent_message.rs` | `[EXISTS]` | [message.md](message.md) |
| Tool | `types/tool.rs` | `[EXISTS]` | [tool.md](tool.md) |
| Provider | `provider/model.rs` | `[EXISTS]` | [provider.md](provider.md) |
| Event | `types/event.rs` | `[EXISTS]` | [event.md](event.md) |
| Compaction | `context/compaction.rs` | `[EXISTS]` | [compaction.md](compaction.md) |
| Configuration | `context/config.rs` + `agent_loop/config.rs` | `[EXISTS]` | [config.md](config.md) |
| SystemPromptStrategy | trait + implementations | `[EXISTS]` | [agent.md](agent.md) |
| ContextTranslationStrategy | `provider/context_translation.rs` | `[EXISTS]` | [provider.md](provider.md) |
| Introspection / Memory | not in code | `[CONCEPTUAL]` | [agent.md](agent.md) |
| Permissions | not in code | `[CONCEPTUAL]` | [agent.md](agent.md) |

---

## Callback Ownership

Callbacks live on the entity they observe:

| Callback | Owner | Status |
|----------|-------|--------|
| before_task / after_task | Session (SessionRecorderConfig) | `[EXISTS]` |
| before_loop / after_loop | Loop | `[EXISTS]` |
| on_error | Loop | `[EXISTS]` |
| before_turn / after_turn | Turn | `[EXISTS]` |
| before_tool_execution / after_tool_execution | Tool | `[EXISTS]` |
| before_tool_execution_update / after_tool_execution_update | Tool | `[EXISTS]` |
| before_compaction_start / after_compaction_end | Compaction | `[EXISTS]` |

---

## Conceptual vs Code: Key Misalignments

These are places where the conceptual model differs from current code. They represent future refactoring opportunities:

| Concept | Status | Notes |
|---------|--------|-------|
| ~~Agent Profile~~ | `[EXISTS]` ✓ | `AgentProfile` struct in `agents/profile.rs` with profile_id, name, description, system_prompt, etc. |
| ~~thinking_level on Session~~ | Removed | Session-level `thinking_level` removed. Now captured per-loop in `LoopConfigSnapshot`. `AgentProfile::resolve_thinking_level()` removed. |
| ~~temperature on Session~~ | Removed | Session-level `temperature` removed. Now captured per-loop in `LoopConfigSnapshot`. `AgentProfile::resolve_temperature()` removed. |
| ~~Session model~~ | Removed | Session-level `model_config` removed. Model config is now captured per-loop in `LoopConfigSnapshot`. |
| ~~Session scope~~ | `[EXISTS]` ✓ | `SessionScope::Ephemeral \| Persistent` (G7). |
| ~~SystemPromptStrategy~~ | `[EXISTS]` ✓ | Trait + 3-entity model (strategy template → prompt instance → agent ref). `file:` and `{{...}}` resolution. |
| ~~Compaction config~~ | `[EXISTS]` ✓ | Strategies consolidated into `CompactionConfig` (G5). |
| ~~before_task / after_task~~ | `[EXISTS]` ✓ | On `SessionRecorderConfig` (G2). |
| ~~ContextTranslationStrategy~~ | `[EXISTS]` ✓ | Trait + `DefaultContextTranslation` in `provider/context_translation.rs` (G8). |
| Introspection | `[CONCEPTUAL]` | Memory extraction with 3 categories (episodic, semantic, procedural). Not in code. |
| Permissions | `[CONCEPTUAL]` | Include/exclude rules on Agent. Not in code. |

---

## Core Gaps

Prioritized list of features that belong in phi-core (per [First Principles](../../architecture/overview.md#first-principles-core-vs-external)) but are not yet implemented. Each gap is derived from `[CONCEPTUAL]` items in the entity specs.

### Priority 1 — Small, High-Value — ALL IMPLEMENTED ✓

| ID | Feature | Status |
|----|---------|--------|
| **G1** | Compaction callbacks (`before_compaction_start` / `after_compaction_end`) | `[EXISTS]` — On `AgentLoopConfig`. |
| **G3** | Agent Profile struct | `[EXISTS]` — `AgentProfile` in `agents/profile.rs`. |
| **G4** | Session model override | Removed — `Session.model_config` removed. Model config now captured per-loop in `LoopConfigSnapshot`. |
| **G7** | Session scope | `[EXISTS]` — `SessionScope::Ephemeral \| Persistent`. |
| **G9** | Session task attributes | Removed — `Session.thinking_level`, `Session.temperature`, and `Session.model_config` moved to per-loop `LoopConfigSnapshot`. `AgentProfile::resolve_thinking_level()` and `resolve_temperature()` removed. |

### Priority 2 — Medium Refactors

| ID | Feature | Why Core | Effort | Spec Ref |
|----|---------|----------|--------|----------|
| **G5** | Compaction config consolidation `[EXISTS]` | Compaction strategies (`in_memory_strategy`, `block_strategy`) are now fields on `CompactionConfig`, consolidating what was previously split across `ContextConfig` + `AgentLoopConfig`. | ~100 LOC | `config.md`, misalignment table above |
| **G2** | Session-level callbacks (`before_task` / `after_task`) `[EXISTS]` | `before_task` and `after_task` callbacks now exist on `SessionRecorderConfig`. `before_task` fires on the first `AgentStart` with a new `session_id`; `after_task` fires on `flush()`. | ~80 LOC | callback ownership table above |
| **G6** | SystemPromptStrategy trait `[EXISTS]` | The `SystemPromptStrategy` trait now exists with a `compose(context) -> String` method. Supports a 3-entity model: strategy template, prompt instance, profile ref. Full 5-layer composition is a future enhancement. | ~100 LOC | `agent.md` |

### Priority 3 — Needs Design

| ID | Feature | Why Core | Effort | Spec Ref |
|----|---------|----------|--------|----------|
| **G8** | ContextTranslationStrategy `[EXISTS]` | ContextTranslationStrategy trait with DefaultContextTranslation. Read-only translation for cross-provider compatibility. | ~150 LOC | `provider.md`, misalignment table above |
| **G10** | Tool Registry `[EXISTS]` | ToolRegistry maps config tool names to instances. 6 built-in tools registered. | ~200 LOC | `config.md` |

### External — Not Core

These are explicitly **not** core gaps. They can be built on top of phi-core using existing extension points:

| Item | Extension Point |
|------|----------------|
| Introspection / Memory | External crate using G1 compaction callbacks + session data |
| Permissions | `InputFilter` + `BeforeToolExecutionFn` |
| Multi-agent orchestration | `agent_loop` / `agent_loop_continue` / `agent_loop_parallel` |
| Model fallback chains | Custom `StreamProvider` wrapping multiple providers |
| Observability backends | `AgentEvent` stream |
| Domain tools | `AgentTool` trait |

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
