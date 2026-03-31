# Invocation Layer

The config-driven invocation layer is the single entry point for constructing and running agents. All agent features — profile, session config, tools, callbacks, compaction, execution limits — are expressed as config and resolved through one pipeline.

**Status:** `[PLANNED]`

**Why:** Features exist but are invoked through scattered Rust builder APIs. There is no unified config → CLI → UI pipeline. The invocation layer makes every feature discoverable and configurable without writing Rust code.

## Concept Overview

```
Invocation Layer [PLANNED]
├── Config Schema [PLANNED] — TOML format defining all agent parameters
│   ├── [agent.profile] → AgentProfile
│   ├── [[agent.instances]] → Named agent variations
│   ├── [provider] → Default ModelConfig
│   ├── [[provider.instances]] → Named provider configurations
│   ├── [session] → Session overrides (model, thinking, temperature, scope)
│   ├── [tools] → built-in names + plugin references
│   ├── [skills] → Skill directory paths
│   ├── [sub_agents] → Default sub-agent template
│   ├── [[sub_agents.instances]] → Named sub-agent configurations
│   ├── [callbacks] → lifecycle hooks (built-in + Phase 2 plugin refs)
│   ├── [compaction] → compaction policy + callbacks
│   ├── [execution] → limits, retry, cache, tool strategy
│   └── [hooks] → context transformation (Phase 2 plugin refs)
├── Config Parser [PLANNED] — TOML → typed Rust structs
├── Agent Constructor [PLANNED] — from_config(config) → Agent
└── Resolution Order [PLANNED] — config file → env vars → defaults
```

---

## Config Design Patterns

### Pattern 1: Override Order

Every field that can be set at multiple levels has an explicit override annotation. Higher levels override lower levels. `null` / absent means "inherit from lower level."

Override order key (highest wins, left to right):
- **S** = Session override
- **AI** = Agent Instance override
- **A** = Agent Profile default
- **PI** = Provider Instance override
- **P** = Provider default
- **C** = Config-level default (top-level section)

### Pattern 2: Default + Instances

Entities with multiple instances (providers, agents, sub-agents) use:
- A `[section]` block defining the **default** configuration
- An `[[section.instances]]` array defining **named variants** with overrides

The default is used when no instance is specified. Instances inherit from the default and override specific fields. This pattern applies recursively.

---

## Config Schema

The config schema is a TOML document that fully describes an agent's identity, capabilities, and runtime parameters. Every field has a sensible default — a minimal config specifying only `[provider]` is sufficient to create a working agent.

### `[agent.profile]` — Agent Identity (Default)

Maps to `AgentProfile` (G3). Override order: AI→A→C.

| Field | Type | Default | Override | Description |
|-------|------|---------|----------|-------------|
| `profile_id` | string | auto-generated UUID | — | Stable identity for profile sharing |
| `name` | string | `null` | AI→A→C | Human-readable agent name |
| `description` | string | `null` | AI→A→C | Purpose and capabilities |
| `system_prompt` | string | `""` | AI→A→C | Static system prompt. Future: replaced by SystemPromptStrategy |
| `thinking_level` | string | `"off"` | S→AI→A→C | Default reasoning depth: off, minimal, low, medium, high |
| `temperature` | float | `null` | S→AI→A→C | Default sampling temperature |
| `max_tokens` | integer | `null` | AI→A→C | Default max output tokens |
| `config_id` | string | `null` | AI→A→C | Stable config identity for loop_id derivation |
| `skills` | list of strings | `null` | AI→A→C | Skill directory paths (overrides `[skills].paths`) |

### `[[agent.instances]]` — Named Agent Variations

Each instance inherits all fields from `[agent.profile]` and overrides specific ones.

```toml
[agent.profile]
name = "Assistant"
system_prompt = "You are helpful."
thinking_level = "off"

# Override order: session → agent instance → agent profile → config default

[[agent.instances]]
name = "Code Reviewer"
system_prompt = "You are a code reviewer. Focus on correctness."
thinking_level = "high"
provider = "Anthropic"  # references provider instance by name
model = "claude-opus-4-6"

[[agent.instances]]
name = "Quick Helper"
system_prompt = "Be brief and direct."
provider = "OpenAI"
model = "gpt-4o-mini"
```

### `[provider]` — Model & Provider (Default)

Maps to `ModelConfig`. Override order: PI→P.

| Field | Type | Default | Override | Description |
|-------|------|---------|----------|-------------|
| `name` | string | inferred | PI→P | Display name for logging |
| `model` | string | required | S→AI→A→PI→P | Model ID (e.g., `"claude-opus-4-6"`, `"gpt-4o"`) |
| `api` | string | inferred from model | PI→P | API protocol: `anthropic`, `openai`, `google`, `bedrock`, `azure`, `vertex` |
| `api_key` | string | `null` | PI→P | API key. Supports `${ENV_VAR}` substitution |
| `base_url` | string | provider default | PI→P | Custom API endpoint |
| `context_window` | integer | model default | PI→P | Model's context window size |
| `max_tokens` | integer | model default | PI→P | Model's max output tokens |
| `headers` | table | `{}` | PI→P | Custom HTTP headers: `{ "X-Custom" = "value" }` |

### `[provider.cost]` — Per-Token Cost Rates

Maps to `CostConfig`. Override order: PI→P.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `input_per_million` | float | `0.0` | Cost per million input tokens |
| `output_per_million` | float | `0.0` | Cost per million output tokens |
| `cache_read_per_million` | float | `0.0` | Cost per million cached-read tokens |
| `cache_write_per_million` | float | `0.0` | Cost per million cache-write tokens |

### `[provider.compat]` — OpenAI-Compatible Provider Quirks

Maps to `OpenAiCompat`. Only relevant for providers using `api = "openai"`. Override order: PI→P.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auth_style` | string | `"bearer"` | Authentication style: `bearer`, `x-api-key`, `query` |
| `reasoning_format` | string | `"standard"` | Reasoning token format: `standard`, `o1`, `deepseek` |
| `max_tokens_field` | string | `"max_tokens"` | Field name for max tokens in API request |
| `supports_streaming` | bool | `true` | Provider supports SSE streaming |
| `supports_tools` | bool | `true` | Provider supports tool/function calling |
| `supports_thinking` | bool | `false` | Provider supports extended thinking |
| `supports_system_prompt` | bool | `true` | Provider supports system prompt role |
| `supports_images` | bool | `false` | Provider supports image content |

### `[[provider.instances]]` — Named Provider Configurations

Each instance overrides the default `[provider]` fields. Referenced by name from agent instances and sub-agents.

```toml
[provider]
name = "Anthropic"
model = "claude-sonnet-4-6"
api_key = "${ANTHROPIC_API_KEY}"

[[provider.instances]]
name = "Anthropic"
models = ["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"]
api_key = "${ANTHROPIC_API_KEY}"

[[provider.instances]]
name = "OpenAI"
models = ["gpt-4o", "gpt-4o-mini", "o3"]
api_key = "${OPENAI_API_KEY}"
base_url = "https://api.openai.com/v1"

[[provider.instances]]
name = "Local"
models = ["llama-3.1-70b"]
base_url = "http://localhost:8080/v1"
api_key = ""
```

### `[session]` — Session Overrides

Maps to Session fields (G4, G7, G9). These override agent profile defaults for the current session/task. Override order: S→AI→A→C.

| Field | Type | Default | Override | Description |
|-------|------|---------|----------|-------------|
| `scope` | string | `"ephemeral"` | S | `ephemeral` (logs stored, no introspection) or `persistent` (introspection fires during save) |
| `model` | string | `null` | S→AI→A→PI→P | Model override for this session |
| `thinking_level` | string | `null` | S→AI→A→C | Thinking override for this session |
| `temperature` | float | `null` | S→AI→A→C | Temperature override for this session |

**Resolution:** Session values override Agent Instance values, which override Agent Profile values. `null` means "inherit from next level."

### `[tools]` — Tool Registry

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `builtin` | list of strings | `["bash", "read_file", "write_file", "edit_file", "list_files", "search"]` | Built-in tools to enable. `"all"` enables all. `"none"` disables all. |
| `mcp` | list of tables | `[]` | MCP server connections. Each: `{ command, args, transport }` |
| `plugins` | list of tables | `[]` | WASM plugin tools. Each: `{ name, path }` (Phase 2) |

### `[skills]` — Skill Directories

Maps to `SkillSet`. Override order: AI→A→C.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `paths` | list of strings | `[]` | Directories to load SKILL.md files from. Agent instances can override. |

### `[sub_agents]` — Sub-Agent Default Template

Default configuration for sub-agents. Acts as a template — the spawning agent can dynamically alter or override any field at runtime. If no overrides are provided, the sub-agent runs with these defaults.

| Field | Type | Default | Override | Description |
|-------|------|---------|----------|-------------|
| `max_turns` | integer | `10` | instances→default | Maximum turns for sub-agent |
| `thinking_level` | string | `"off"` | instances→default | Default thinking depth |
| `tools` | list of strings | inherits parent | instances→default | Tools available to sub-agent |

### `[[sub_agents.instances]]` — Named Sub-Agent Configurations

Each instance is a tool the parent agent can invoke. Inherits from `[sub_agents]` default.

```toml
[sub_agents]
max_turns = 10
thinking_level = "medium"

# Sub-agent instances — each becomes a tool in the parent's registry.
# The spawning agent can dynamically override any field at runtime.
# The config serves as a template/baseline.

[[sub_agents.instances]]
name = "researcher"
description = "Deep research on topics"
system_prompt = "Research thoroughly."
provider = "Anthropic"
model = "claude-opus-4-6"
tools = ["search", "read_file"]

[[sub_agents.instances]]
name = "code_writer"
description = "Writes and tests code"
system_prompt = "Write clean, tested code."
tools = ["bash", "read_file", "write_file", "edit_file"]
# inherits max_turns=10 and thinking_level="medium" from default
```

**Two patterns — both supported:**
- **Static sub-agents (config-defined):** Predefined in TOML as default templates. Registered at agent construction. The LLM decides when to invoke them. The spawning agent can override any field at runtime.
- **Dynamic sub-agents (runtime-created):** Built from the static config as a starting point, then altered programmatically. The config serves as a template/baseline; runtime code modifies fields as needed before spawning.

### `[callbacks]` — Lifecycle Hooks

Initially: references to built-in callback identifiers. Phase 2: WASM plugin references. All callbacks are Code-only (closures today, WASM plugins in Phase 2).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `before_turn` | string | `null` | `[Code-only: Phase 2]` Before each LLM turn. Return false to abort. |
| `after_turn` | string | `null` | `[Code-only: Phase 2]` After each LLM turn. |
| `before_loop` | string | `null` | `[Code-only: Phase 2]` Before agent loop starts. Return false to abort. |
| `after_loop` | string | `null` | `[Code-only: Phase 2]` After agent loop ends. |
| `before_tool_execution` | string | `null` | `[Code-only: Phase 2]` Before each tool call. Return false to skip. |
| `after_tool_execution` | string | `null` | `[Code-only: Phase 2]` After each tool call. |
| `before_tool_execution_update` | string | `null` | `[Code-only: Phase 2]` Before each streaming tool update. |
| `after_tool_execution_update` | string | `null` | `[Code-only: Phase 2]` After each streaming tool update. |
| `on_error` | string | `null` | `[Code-only: Phase 2]` When LLM returns StopReason::Error. |
| `before_compaction_start` | string | `null` | `[Code-only: Phase 2]` (G1) Before compaction fires. |
| `after_compaction_end` | string | `null` | `[Code-only: Phase 2]` (G1) After compaction completes. |
| `before_task` | string | `null` | `[Code-only: Phase 2]` (G2) Session-level lifecycle hook. |
| `after_task` | string | `null` | `[Code-only: Phase 2]` (G2) Session-level lifecycle hook. |

### `[hooks]` — Context Transformation

Code-only hooks for message pipeline manipulation. Phase 2: WASM plugin references.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `convert_to_llm` | string | `null` | `[Code-only: Phase 2]` AgentMessage[] → Message[] before each LLM call |
| `transform_context` | string | `null` | `[Code-only: Phase 2]` Prune/reorder/inject before convert_to_llm |

### `[[filters]]` — Input Filters

Code-only filters for message validation. Phase 2: WASM plugin references.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `plugin` | string | — | `[Code-only: Phase 2]` Plugin path or identifier |

### `[compaction]` — Context Management

Maps to `CompactionConfig` (G5 consolidation target). Override order: C only.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_context_tokens` | integer | `100000` | Model's context window |
| `system_prompt_tokens` | integer | `4000` | Tokens reserved for system prompt |
| `compact_at_pct` | float | `0.90` | Fraction of context that triggers compaction |
| `budget_threshold_pct` | float | `0.05` | Minimum headroom before compaction fires |
| `scope` | string | `"fixed_count(3)"` | Compaction scope: `fixed_count(N)` or `token_budget` |
| `keep_first_turns` | integer | `2` | Turns kept verbatim from start |
| `keep_recent_turns` | integer | `10` | Recent turns kept with truncated tool output |
| `tool_output_max_lines` | integer | `50` | Max lines per tool output in recent section |
| `max_summary_tokens` | integer | `4000` | Token budget for summarized sections |
| `strategy` | string | `null` | `[Code-only: Phase 2]` Custom compaction strategy plugin |

### `[execution]` — Limits & Safety

Maps to `ExecutionLimits` + `ToolExecutionStrategy`. Override order: C only.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_turns` | integer | `50` | Maximum turns per loop |
| `max_tokens` | integer | `1000000` | Maximum total tokens per loop |
| `max_duration_secs` | integer | `600` | Maximum wall-clock time per loop |
| `max_cost` | float | `null` | Maximum dollar cost per loop |
| `tool_strategy` | string | `"parallel"` | Tool execution: `parallel`, `sequential`, `batched(N)` |

### `[execution.retry]` — Retry Configuration

Maps to `RetryConfig`. Override order: C only.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_attempts` | integer | `3` | Maximum retry attempts for transient errors |
| `initial_delay_ms` | integer | `1000` | Delay before first retry (milliseconds) |
| `backoff_multiplier` | float | `2.0` | Multiplier applied each attempt |
| `max_delay_ms` | integer | `30000` | Maximum delay cap (milliseconds) |

### `[execution.cache]` — Prompt Caching

Maps to `CacheConfig`. Override order: C only.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Master switch for caching |
| `strategy` | string | `"auto"` | Cache strategy: `auto`, `disabled`, `manual` |
| `system` | bool | `true` | (manual only) Cache the system prompt |
| `tools` | bool | `true` | (manual only) Cache tool definitions |
| `messages` | bool | `true` | (manual only) Cache conversation history |

---

## Complete Field Audit

### A. Config-Representable Fields — Full Override Mapping

| Field | Struct | Override Order | Config Section |
|-------|--------|---------------|----------------|
| `model` | `ModelConfig` | S→AI→A→PI→P | `[provider].model` / `[session].model` |
| `thinking_level` | `ThinkingLevel` | S→AI→A→C | `[agent.profile]` / `[session]` |
| `temperature` | `Option<f32>` | S→AI→A→C | `[agent.profile]` / `[session]` |
| `system_prompt` | `String` | AI→A→C | `[agent.profile]` |
| `max_tokens` | `Option<u32>` | AI→A→C | `[agent.profile]` |
| `name` (display) | `ModelConfig` | PI→P | `[provider].name` |
| `context_window` | `ModelConfig` | PI→P | `[provider].context_window` |
| `api_key` | `ModelConfig` | PI→P | `[provider].api_key` |
| `base_url` | `ModelConfig` | PI→P | `[provider].base_url` |
| `cost.*` | `CostConfig` | PI→P | `[provider.cost]` |
| `headers` | `ModelConfig` | PI→P | `[provider].headers` |
| `compat.*` | `OpenAiCompat` | PI→P | `[provider.compat]` |
| `tool_execution` | `ToolExecutionStrategy` | C | `[execution].tool_strategy` |
| `max_turns` | `ExecutionLimits` | C | `[execution].max_turns` |
| `max_total_tokens` | `ExecutionLimits` | C | `[execution].max_tokens` |
| `max_duration` | `ExecutionLimits` | C | `[execution].max_duration_secs` |
| `max_cost` | `ExecutionLimits` | C | `[execution].max_cost` |
| `retry.*` | `RetryConfig` | C | `[execution.retry]` |
| `cache.*` | `CacheConfig` | C | `[execution.cache]` |
| `compaction.*` | `CompactionConfig` | C | `[compaction]` |
| Skills | `SkillSet` | AI→A→C | `[skills].paths` |
| Sub-agents | `SubAgentTool` | instances→default | `[[sub_agents.instances]]` |
| `config_id` | `AgentLoopConfig` | AI→A→C | `[agent.profile].config_id` |
| `scope` | `SessionScope` | S | `[session].scope` |
| `builtin` tools | `Vec<AgentTool>` | C | `[tools].builtin` |
| MCP servers | `Vec<McpToolAdapter>` | C | `[tools].mcp` |

### B. Code-Only Fields — Phase 2 WASM Plugin References

These require executable code (closures/trait impls today). Phase 2 maps them to WASM plugin references in config.

| Field | Struct | Config Representation (Phase 2) |
|-------|--------|--------------------------------|
| `before_turn` | `AgentLoopConfig` | `[callbacks].before_turn = "plugin:path.wasm"` |
| `after_turn` | `AgentLoopConfig` | `[callbacks].after_turn = "plugin:..."` |
| `before_loop` | `AgentLoopConfig` | `[callbacks].before_loop = "..."` |
| `after_loop` | `AgentLoopConfig` | `[callbacks].after_loop = "..."` |
| `before_tool_execution` | `AgentLoopConfig` | `[callbacks].before_tool_execution = "..."` |
| `after_tool_execution` | `AgentLoopConfig` | `[callbacks].after_tool_execution = "..."` |
| `before_tool_execution_update` | `AgentLoopConfig` | `[callbacks].before_tool_execution_update = "..."` |
| `after_tool_execution_update` | `AgentLoopConfig` | `[callbacks].after_tool_execution_update = "..."` |
| `on_error` | `AgentLoopConfig` | `[callbacks].on_error = "..."` |
| `before_compaction_start` | Planned (G1) | `[callbacks].before_compaction_start = "..."` |
| `after_compaction_end` | Planned (G1) | `[callbacks].after_compaction_end = "..."` |
| `convert_to_llm` | `AgentLoopConfig` | `[hooks].convert_to_llm = "..."` |
| `transform_context` | `AgentLoopConfig` | `[hooks].transform_context = "..."` |
| `input_filters` | `AgentLoopConfig` | `[[filters]]` with plugin refs |
| `compaction_strategy` | `AgentLoopConfig` | `[compaction].strategy = "plugin:..."` |
| `block_compaction_strategy` | `AgentLoopConfig` | Same as above |
| `provider_override` | `BasicAgent` | Not in config — test/injection escape hatch only |

### C. Runtime Fields — NOT in Config

Populated during execution, not at construction. Not configurable.

| Field | Struct | Why Runtime |
|-------|--------|------------|
| `messages` | `AgentContext` | Accumulated conversation history |
| `tools` (instances) | `AgentContext` | Instantiated tool objects (config specifies *which* tools by name) |
| `session` | `AgentContext` | Managed by SessionRecorder from events |
| `steering_queue` | `BasicAgent` | Populated via `Agent::steer()` at runtime |
| `follow_up_queue` | `BasicAgent` | Populated via `Agent::follow_up()` at runtime |
| `steering_mode` | `BasicAgent` | Runtime control via `Agent::set_steering_mode()` |
| `follow_up_mode` | `BasicAgent` | Runtime control |
| `is_streaming` | `BasicAgent` | Guard flag during execution |
| `cancel` | `BasicAgent` | Created per-loop for cancellation |
| `get_steering_messages` | `AgentLoopConfig` | Closure wrapping queue access |
| `get_follow_up_messages` | `AgentLoopConfig` | Closure wrapping queue access |

### D. Internal/Derived Fields — NOT in Config

Auto-generated or derived. Not user-configurable.

| Field | Struct | Why Internal |
|-------|--------|-------------|
| `agent_id` | `BasicAgent` | Auto-generated UUID at construction |
| `session_id` | `BasicAgent` | Auto-generated UUID, rotated via `new_session()` |
| `loop_id` | `AgentContext` | Derived from `session_id.config_id.counter` |
| `parent_loop_id` | `AgentContext` | Set automatically for continuations |
| `continuation_kind` | `AgentContext` | Set by `continue_loop()` call semantics |
| `first_turn_trigger` | `AgentLoopConfig` | Auto-set: `User` for origin, `SubAgent` for sub-agents |
| `loop_counters` | `BasicAgent` | Per-config counter for loop_id generation |
| `last_loop_id` | `BasicAgent` | Tracked automatically per-prompt |
| `last_active_at` | `BasicAgent` | Updated automatically per-prompt |
| All `LoopRecord` fields | `LoopRecord` | Populated by SessionRecorder from events |
| All `Turn` fields | `Turn` | Populated by SessionRecorder from events |
| All `Session` identity fields | `Session` | Managed by SessionRecorder |

### `[system_prompt_strategy]` -- System Prompt Strategy Template

Defines a reusable strategy for assembling system prompts from ordered blocks. Each block has a name, order, and optional max length. Strategies are referenced by prompt instances.

```toml
[[system_prompt_strategy.instances]]
id = "coding-assistant"
description = "Strategy for coding assistant prompts"

[[system_prompt_strategy.instances.blocks]]
name = "identity"
order = 1
max_length = 500

[[system_prompt_strategy.instances.blocks]]
name = "capabilities"
order = 2
max_length = 1000

[[system_prompt_strategy.instances.blocks]]
name = "instructions"
order = 3
max_length = 2000
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique strategy identifier |
| `description` | string | Human-readable description |
| `blocks[].name` | string | Block name (referenced by prompt instances) |
| `blocks[].order` | integer | Assembly order (lower = earlier in prompt) |
| `blocks[].max_length` | integer | Maximum token length for this block |

### `[system_prompt]` -- System Prompt Instances

Defines concrete prompt instances that reference a strategy and provide content for each block. Block content fields are flattened at the instance level using the block name.

```toml
[[system_prompt.instances]]
id = "code-reviewer-prompt"
description = "Prompt for code review tasks"
type = "coding-assistant"  # references strategy id
identity = "You are a senior code reviewer."
capabilities = "You can read files, search code, and run tests."
instructions = "Focus on correctness, security, and maintainability."
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique prompt instance identifier |
| `description` | string | Human-readable description |
| `type` | string | References a `system_prompt_strategy` instance by id |
| *(block names)* | string | Flattened block content fields matching the strategy's block names |

### `default_workspace` and `[agent].workspace` -- Workspace Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_workspace` | `Option<String>` | `null` | Top-level default workspace directory for all agents |
| `[agent.profile].workspace` | `Option<String>` | `null` | Agent-level workspace override; takes precedence over `default_workspace` |

Agent instances can also override workspace:

```toml
default_workspace = "./workspace"

[agent.profile]
workspace = "./agent-workspace"  # overrides default_workspace

[[agent.instances]]
name = "researcher"
workspace = "./research-workspace"  # overrides agent profile workspace
```

---

## Resolution Order

```
Config file values
  → override with env vars (${VAR} substitution in string fields)
  → override with CLI flags (when CLI adapter exists — external)
  → fill remaining with defaults
```

For multi-level fields (model, thinking_level, temperature):
```
Session override → Agent Instance → Agent Profile default → Provider Instance → Provider default → Config default
```

All secrets (`api_key`, credentials) support `${ENV_VAR}` substitution to avoid plaintext in config files.

---

## Agent Construction

### Entry Point

```rust
/// Construct an Agent from a parsed config.
/// This is the single entry point for config-driven agent creation.
pub fn agent_from_config(config: AgentConfig) -> Result<BasicAgent, ConfigError>
```

`AgentConfig` is the typed Rust struct deserialized from the TOML config. It maps directly to the config sections above.

### Construction Flow

```
TOML file → parse → AgentConfig struct
  → resolve env vars
  → build AgentProfile from [agent.profile] + [provider]
  → build Session overrides from [session]
  → register tools from [tools] (built-in + MCP + plugins)
  → register skills from [skills]
  → register sub-agents from [[sub_agents.instances]]
  → register callbacks from [callbacks] (Phase 2: load WASM plugins)
  → build CompactionConfig from [compaction]
  → build ExecutionLimits from [execution]
  → build RetryConfig from [execution.retry]
  → build CacheConfig from [execution.cache]
  → assemble BasicAgent (or custom Agent impl)
```

### Config Hierarchy (Agent-Centric)

```
┌──────────────────────────────────────────────────┐
│                    CONFIG FILE                     │
│  (TOML — all static values flow from here)        │
├──────────────────────────────────────────────────┤
│  [agent.profile]     → AgentProfile              │
│  [[agent.instances]] → Named agent variations     │
│  [provider]          → Default ModelConfig        │
│  [[provider.instances]] → Named providers         │
│  [provider.cost]     → CostConfig                │
│  [provider.compat]   → OpenAiCompat              │
│  [session]           → Session overrides          │
│  [tools]             → Tool registry (by name)    │
│  [skills]            → SkillSet paths             │
│  [sub_agents]        → Sub-agent default template │
│  [[sub_agents.instances]] → Named sub-agents      │
│  [callbacks]         → Phase 2 WASM plugin refs   │
│  [hooks]             → Phase 2 WASM plugin refs   │
│  [[filters]]         → Phase 2 WASM plugin refs   │
│  [compaction]        → CompactionConfig           │
│  [execution]         → ExecutionLimits            │
│  [execution.retry]   → RetryConfig               │
│  [execution.cache]   → CacheConfig               │
└───────────────────────┬──────────────────────────┘
                        │
                        ▼
              ┌─────────────────┐
              │  AgentProfile   │ ← agent identity + defaults
              └────────┬────────┘
                       │
              ┌────────▼────────┐
              │  Session        │ ← per-task overrides
              │  (model, thinking, temp, scope)
              └────────┬────────┘
                       │ resolve_*()
              ┌────────▼────────┐
              │  Agent          │ ← runtime: tools, queues, state
              │  (BasicAgent)   │
              └────────┬────────┘
                       │ build_config()
              ┌────────▼────────┐
              │ AgentLoopConfig │ ← resolved, static, borrowed
              └────────┬────────┘
                       │
              ┌────────▼────────┐
              │  agent_loop()   │ ← execution engine
              └─────────────────┘
```

---

## Config Examples

### Minimal Config

```toml
# Smallest possible config — everything else uses defaults
[provider]
model = "claude-sonnet-4-6"
api_key = "${ANTHROPIC_API_KEY}"
```

### Full Config

```toml
[agent.profile]
name = "Code Reviewer"
description = "Reviews pull requests for correctness and style"
system_prompt = """
You are a senior code reviewer. Focus on correctness, security, and maintainability.
"""
thinking_level = "high"

[provider]
name = "Anthropic"
model = "claude-opus-4-6"
api_key = "${ANTHROPIC_API_KEY}"

[provider.cost]
input_per_million = 15.0
output_per_million = 75.0

[[provider.instances]]
name = "OpenAI"
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"

[session]
scope = "persistent"

[tools]
builtin = ["bash", "read_file", "search", "edit_file"]

[tools.mcp.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
transport = "stdio"

[skills]
paths = ["./skills", "~/.phi/skills"]

[sub_agents]
max_turns = 10
thinking_level = "medium"

[[sub_agents.instances]]
name = "researcher"
description = "Deep research on topics"
system_prompt = "Research thoroughly and cite sources."
tools = ["search", "read_file"]

[compaction]
max_context_tokens = 200000
keep_recent_turns = 15
max_summary_tokens = 6000

[execution]
max_turns = 30
max_duration_secs = 300
tool_strategy = "parallel"

[execution.retry]
max_attempts = 3
initial_delay_ms = 1000

[execution.cache]
strategy = "auto"
```

---

## Relationship to Core Gaps (G1-G9)

The config schema subsumes the P1 gaps — they become config sections rather than standalone refactors:

| Gap | Config Section | Implementation |
|-----|---------------|---------------|
| G1: Compaction callbacks | `[callbacks].before_compaction_start` | Callback types + config mapping |
| G3: Agent Profile | `[agent.profile]` + `[[agent.instances]]` | `AgentProfile` struct |
| G4: Session model override | `[session].model` | Session field + resolution |
| G5: Compaction config consolidation | `[compaction]` | Unified `CompactionConfig` |
| G7: Session scope | `[session].scope` | `SessionScope` enum |
| G9: Session task attributes | `[session].thinking_level`, `[session].temperature` | Session fields + resolution |

---

## Core vs External Boundary

| Component | Boundary | Rationale |
|-----------|----------|-----------|
| Config schema (TOML structure) | **Core** | Contract — all consumers share one schema |
| Config parser | **Core** | Cross-cutting, all agents need it |
| `agent_from_config()` | **Core** | Single entry point |
| Env var substitution | **Core** | Universal deployment pattern |
| CLI adapter | **External** | App-specific CLI needs |
| UI adapter | **External** | App-specific UI choices |
| Config file discovery | **External** | App-specific paths |

---

## Code Reference

| File | What it will contain |
|------|---------------------|
| `src/config/schema.rs` | `AgentConfig` struct (deserialized from TOML) |
| `src/config/parser.rs` | TOML parsing + env var resolution |
| `src/config/builder.rs` | `agent_from_config()` constructor |
| `src/config/mod.rs` | Module root + re-exports |
| `src/agents/profile.rs` | `AgentProfile` struct with `resolve_*` methods |

---

## Design Decisions

- **TOML over YAML/JSON:** TOML is the Rust ecosystem standard (Cargo.toml). It's readable, has good error messages, and maps naturally to nested structs. JSON lacks comments. YAML has whitespace footguns.
- **Config is additive, not exclusive:** The programmatic Rust API (`BasicAgent::new().with_*()`) remains fully supported. Config is an additional entry point for users who don't want to write Rust.
- **No runtime config reloading:** Config is read once at agent construction. Hot-reload is a future enhancement, not in scope.
- **Default + Instances pattern:** Enables multi-model, multi-agent setups. Instances inherit from defaults and override specific fields. Recursive application (provider instances, agent instances, sub-agent instances).
- **Explicit override annotations:** Every multi-level field documents its full override chain. No implicit precedence rules — the config spec is the source of truth.
