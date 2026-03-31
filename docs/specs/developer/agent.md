# Agent

The central entity in the system. An Agent combines a given identity (Agent Profile), capabilities (tools, skills, MCP connections), permissions, and introspection into a single runtime unit that executes Sessions (tasks).

The `Agent` trait defines the runtime interface (prompting, state access, control, steering queues). `BasicAgent` is the default in-memory implementation that owns conversation state, tools, and provider configuration.

## Concept Overview

```
Agent
├── HEADER
│   ├── agent_id [EXISTS] — UUID, immutable
│   ├── Agent Profile [CONCEPTUAL as struct; EXISTS as scattered fields]
│   │   ├── profile_id [CONCEPTUAL] — distinct from agent_id; shareable
│   │   ├── SystemPromptStrategy [EXISTS] — how system prompt is composed
│   │   │   └── Currently: static system_prompt string [EXISTS]
│   │   ├── Agent Name [CONCEPTUAL]
│   │   └── Agent Description [CONCEPTUAL]
│   ├── Limits (Agent-level)
│   │   ├── context_config [EXISTS]
│   │   ├── execution_limits [EXISTS]
│   │   └── retry_config [EXISTS]
│   └── Default Model [EXISTS — BasicAgent.model_config]
│       └── Fallback when Session and Loop don't specify their own
│
├── TAB: Sessions (Tasks) [EXISTS]
│   └── (drill-down: Session → Loop → Turn)
├── TAB: Capabilities [EXISTS as Vec<Arc<dyn AgentTool>>]
│   ├── Tools [EXISTS]  ├── Sub-agents [EXISTS]
│   ├── OpenAPI tools [EXISTS]  └── Built-in tools [EXISTS]
├── TAB: Skills [EXISTS as SkillSet; CONCEPTUAL as browsable tab]
├── TAB: MCP Connections [EXISTS]
├── TAB: Permissions [CONCEPTUAL]
│   ├── Include rules [CONCEPTUAL]  └── Exclude rules [CONCEPTUAL]
├── TAB: Introspection [CONCEPTUAL] — mandatory when scope = Persistent
│   ├── Episodic Memory [CONCEPTUAL]  ├── Semantic Memory [CONCEPTUAL]
│   ├── Procedural Memory [CONCEPTUAL]
│   ├── Identity Shaping  └── Knowledge Base
│
└── STATE (runtime) [EXISTS]
    ├── session_id [EXISTS]  ├── messages [EXISTS]
    ├── queues [EXISTS]  └── counters [EXISTS]
```

---

## HEADER

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `agent_id` | `String` (UUID v4) | `[EXISTS]` | Stable identifier assigned at construction. Included in every `AgentStart` event. Immutable for the lifetime of the agent instance. |
| **Agent Profile** | — | `[CONCEPTUAL]` as struct; `[EXISTS]` as scattered fields | Personality container. Conceptually separate from Agent (multiple agents could share one profile). Currently fields live directly on `BasicAgent`. |
| `profile_id` | `String` | `[CONCEPTUAL]` | Distinct from `agent_id`. Would allow profile sharing across agents. |
| `SystemPromptStrategy` | trait | `[EXISTS]` | Defines how the system prompt is composed per turn. A trait with `compose(context) -> String` supporting layered prompt building. Uses a 3-entity model: **strategy template** (the trait implementation defining composition logic), **prompt instance** (a concrete prompt produced by the strategy for a given context), and **profile ref** (reference to the agent profile providing personality/identity). Currently `BasicAgent` also retains a static `system_prompt` string as a fallback. |
| `system_prompt` | `String` | `[EXISTS]` | Static system prompt string. Conceptually belongs to the Agent Profile. |
| Agent Name | `String` | `[CONCEPTUAL]` | Human-readable name for the agent. |
| Agent Description | `String` | `[CONCEPTUAL]` | Description of the agent's purpose and capabilities. |
| `workspace` | `Option<PathBuf>` | `[EXISTS]` | Working directory for this agent. Overrides the global `default_workspace` from config. Tools that interact with the filesystem use this as their base path. |
| `model_config` | `ModelConfig` | `[EXISTS]` | Default model for this agent. Falls back here when Session and Loop don't specify their own. Contains: model id, API key, base URL, API protocol, cost rates, context window size. |
| `context_config` | `Option<ContextConfig>` | `[EXISTS]` | Token budget and compaction policy. Agent-level limit. |
| `execution_limits` | `Option<ExecutionLimits>` | `[EXISTS]` | Max turns (50), max tokens (1M), max duration (10 min), cost tracking. Agent-level limit. |
| `retry_config` | `RetryConfig` | `[EXISTS]` | Retry policy for provider errors. Exponential backoff with jitter. Agent-level. |
| `cache_config` | `CacheConfig` | `[EXISTS]` | Prompt caching behavior (enabled/disabled, strategy: Auto/Disabled/Manual). |
| `tool_execution` | `ToolExecutionStrategy` | `[EXISTS]` | How tool calls are executed: Parallel (default), Sequential, Batched. |
| `thinking_level` | `ThinkingLevel` | `[EXISTS]` on Agent | Controls depth of model reasoning (Off/Minimal/Low/Medium/High). Conceptually a Session-level attribute, currently set on Agent. |
| `temperature` | `Option<f32>` | `[EXISTS]` on Agent | Sampling temperature. Conceptually a Session-level attribute, currently set on Agent. |
| `max_tokens` | `Option<u32>` | `[EXISTS]` | Max output tokens per response. None = use model default. |
| `provider_override` | `Option<Arc<dyn StreamProvider>>` | `[EXISTS]` | Escape hatch for test injection or custom providers. Bypasses `ProviderRegistry` dispatch. |

---

## TAB: Sessions (Tasks) `[EXISTS]`

Sessions are the actions an agent performs. Each Session contains Loops (iterations) which contain Turns (steps). See [session.md](session.md).

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `session_id` | `String` (UUID v4) | `[EXISTS]` | Current session identifier. Rotatable via `check_and_rotate`. |

---

## TAB: Capabilities `[EXISTS]`

Registered tools available to the agent. Stored as `Vec<Arc<dyn AgentTool>>`.

| Capability | Status | Description |
|------------|--------|-------------|
| Tools | `[EXISTS]` | Registered `AgentTool` implementations. Added via `with_tools()`. |
| Sub-agents | `[EXISTS]` | Via `SubAgentTool`. Spawns child agent loops in separate sessions. |
| OpenAPI tools | `[EXISTS]` | Auto-generated from OpenAPI 3.0 spec via `OpenApiToolAdapter`. Feature-gated (`openapi`). |
| Built-in tools | `[EXISTS]` | Bash, File, Edit, Grep, ListDir, ReadFile. |

---

## TAB: Skills `[EXISTS]` as SkillSet; `[CONCEPTUAL]` as browsable tab

Declarative capabilities loaded from `SKILL.md` files with YAML frontmatter.

| Field | Status | Description |
|-------|--------|-------------|
| `SkillSet` | `[EXISTS]` | Loaded via `with_skills()`. Discovery and loading from filesystem. |
| Skill discovery | `[EXISTS]` | Finds `<name>/SKILL.md` files. |
| Skill browsing / editing | `[CONCEPTUAL]` | Interactive skill management in a UI. |

---

## TAB: MCP Connections `[EXISTS]`

Model Context Protocol integration for external tool servers.

| Field | Status | Description |
|-------|--------|-------------|
| MCP server connections | `[EXISTS]` | Stdio and HTTP transports via `McpClient` / `McpTransport`. |
| Discovered tools | `[EXISTS]` | Auto-registered from MCP server via `McpToolAdapter`. Transparent to agent loop. |
| MCP connection management | `[CONCEPTUAL]` | Browsable tab for managing connections in a UI. |

---

## TAB: Permissions `[CONCEPTUAL]`

Access control for agent actions. Not yet implemented.

| Field | Status | Description |
|-------|--------|-------------|
| Include rules | `[CONCEPTUAL]` | Whitelist of allowed actions. |
| Exclude rules | `[CONCEPTUAL]` | Blacklist of denied actions. |

---

## TAB: Introspection `[CONCEPTUAL]`

Memory extraction from session logs and identity. Mandatory when Session scope is Persistent.

### Memory Categories

| Category | Status | Description |
|----------|--------|-------------|
| Episodic Memory | `[CONCEPTUAL]` | What happened in past sessions (events, conversations). |
| Semantic Memory | `[CONCEPTUAL]` | Distilled knowledge (facts, concepts, relationships). |
| Procedural Memory | `[CONCEPTUAL]` | Successful strategies learned over time (patterns, playbooks). |

### Memory Destinations

| Destination | Status | Description |
|-------------|--------|-------------|
| Identity Shaping | `[CONCEPTUAL]` | Memory feeds back to evolve the Agent Profile. |
| Knowledge Base | `[CONCEPTUAL]` | Searchable database for future use. |

---

## Agent State (Runtime) `[EXISTS]`

Mutable state that changes during execution.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `session_id` | `String` | `[EXISTS]` | Current session. Rotatable via `check_and_rotate` on inactivity timeout. |
| `messages` | `Vec<AgentMessage>` | `[EXISTS]` | Full conversation history (LLM + Extension messages). |
| `steering_queue` | `Arc<Mutex<Vec<AgentMessage>>>` | `[EXISTS]` | Mid-run interrupt messages. Drained per `steering_mode` (OneAtATime / All). |
| `follow_up_queue` | `Arc<Mutex<Vec<AgentMessage>>>` | `[EXISTS]` | Post-turn follow-up messages. Drained per `follow_up_mode`. |
| `loop_counters` | `HashMap<String, usize>` | `[EXISTS]` | Per-(session, config) monotonic counters for loop ID generation. |
| `last_loop_id` | `Option<String>` | `[EXISTS]` | Most recently started loop. Used for `parent_loop_id` in continuations. |
| `last_active_at` | `Option<DateTime<Utc>>` | `[EXISTS]` | Timestamp of last prompt call. Used by `check_and_rotate` for inactivity detection. |
| `cancel` | `Option<CancellationToken>` | `[EXISTS]` | Abort handle. `Some` during streaming, `None` otherwise. |
| `is_streaming` | `bool` | `[EXISTS]` | Guard against concurrent `prompt()` calls. |
| `session` | `Option<Session>` | `[EXISTS]` | Optional session for block-based compaction. |

---

## Code Reference

| File | What it contains |
|------|-----------------|
| `src/agents/agent.rs` | `Agent` trait — runtime interface (prompting, state, control, steering queues). `QueueMode` enum. |
| `src/agents/basic_agent.rs` | `BasicAgent` struct — default in-memory implementation. Builder pattern. All fields listed above. |

---

## Conceptual Notes

- **Agent Profile as a separate struct** does not exist in code. The `system_prompt` field lives directly on `BasicAgent`. A future `AgentProfile` struct would hold `profile_id`, `SystemPromptStrategy`, name, and description, enabling profile sharing across agents.
- **SystemPromptStrategy** now exists as a trait with a `compose(context) -> String` method. It follows a 3-entity model: **strategy template** (the trait implementation), **prompt instance** (concrete prompt for a given context), **profile ref** (agent profile reference). Full 5-layer composition (base personality, task context, tool/skill index, memory context, turn-specific instructions) is future work. `BasicAgent` retains a static `system_prompt` string as a fallback.
- **thinking_level and temperature** are currently on Agent but conceptually belong at Session level (task-specific attributes). Moving them to Session would allow different tasks to use different reasoning depths without reconfiguring the agent.
- **Introspection** is the largest conceptual gap. It requires session log analysis, memory categorization (episodic/semantic/procedural), and feedback loops to Agent Profile evolution.
