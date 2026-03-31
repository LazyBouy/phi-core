# Configuration Guide

Define your entire agent in a config file — model, tools, compaction, limits — and construct it with two lines of Rust:

```rust
use phi_core::{parse_config_file, agent_from_config, Agent};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_config_file(Path::new("agent.toml"))?;
    let agent = agent_from_config(&config)?;

    // agent is Arc<dyn Agent> — ready to prompt
    println!("Agent model: {:?}", agent.model_config().unwrap().id);
    Ok(())
}
```

## Overview

The configuration system replaces scattered Rust builder calls with a declarative config file. Instead of this:

```rust
let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Sonnet", &key))
    .with_system_prompt("You are a coding assistant.")
    .with_thinking(ThinkingLevel::High)
    .with_temperature(0.2)
    .with_execution_limits(ExecutionLimits { max_turns: 50, .. })
    .with_context_config(ContextConfig { .. });
```

You write a TOML file:

```toml
[agent]
system_prompt = "You are a coding assistant."

[agent.profile]
thinking_level = "high"
temperature = 0.2

[provider]
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[execution]
max_turns = 50
```

**Three formats supported:** TOML (primary, Rust-idiomatic), JSON (programmatic generation), YAML (human-friendly alternative).

**Pipeline:** Config file &rarr; `parse_config_file()` &rarr; `AgentConfig` struct &rarr; `agent_from_config()` &rarr; `Arc<dyn Agent>`

---

## Quick Start

**1. Create `agent.toml`:**

```toml
[provider]
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"
```

**2. Load and use it:**

```rust
use phi_core::{parse_config_file, agent_from_config, Agent};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_config_file(Path::new("agent.toml"))?;
    let agent = agent_from_config(&config)?;

    // The agent is an Arc<dyn Agent> wrapping a BasicAgent internally.
    // Access configuration through trait methods:
    let model = agent.model_config().unwrap();
    println!("Using model: {} via {}", model.id, model.provider);

    Ok(())
}
```

Only the `[provider]` section is required. Everything else has sensible defaults.

---

## Config Formats

### TOML (Recommended)

The primary format. Clean, readable, Rust-idiomatic.

```rust
use phi_core::config::{parse_config, ConfigFormat};

let toml_str = r#"
[provider]
model = "claude-sonnet-4-20250514"
api_key = "sk-..."
"#;
let config = parse_config(toml_str, ConfigFormat::Toml)?;
```

### JSON

Useful when generating config programmatically.

```rust
let json_str = r#"{ "provider": { "model": "gpt-4o", "api_key": "sk-...", "api": "openai" } }"#;
let config = parse_config(json_str, ConfigFormat::Json)?;
```

### YAML

Human-friendly alternative.

```rust
let yaml_str = "provider:\n  model: claude-sonnet-4-20250514\n  api_key: sk-...";
let config = parse_config(yaml_str, ConfigFormat::Yaml)?;
```

### Auto-Detection

`parse_config_file` detects format from the file extension:

| Extension | Format |
|-----------|--------|
| `.toml` | TOML |
| `.json` | JSON |
| `.yaml`, `.yml` | YAML |

`parse_config_auto` tries all formats in order (TOML &rarr; JSON &rarr; YAML) and returns the first successful parse.

---

## Environment Variable Substitution

Any string field in the config can reference environment variables with `${VAR}`:

```toml
[provider]
api_key = "${ANTHROPIC_API_KEY}"
base_url = "${CUSTOM_API_URL}"

[agent]
system_prompt = "Running in ${ENVIRONMENT} mode."
```

**How it works:**
- Substitution happens before parsing (pre-parse text replacement)
- Works in all three formats (TOML, JSON, YAML)
- Missing variables produce `ConfigError::MissingEnvVar`
- Malformed patterns like `${UNCLOSED` are passed through literally
- Empty `${}` is passed through literally

---

## Agent Profile

An `AgentProfile` is a **reusable blueprint** that defines default configuration. Multiple agent instances can share the same profile while overriding specific fields.

```toml
[agent.profile]
name = "coding-agent"
description = "An agent specialized for code generation and review"
system_prompt = "You are an expert software engineer."
thinking_level = "high"
temperature = 0.2
max_tokens = 16384
config_id = "coder"
skills = ["code-review", "debugging"]
```

### System Prompt Resolution

Agent-level `system_prompt` overrides the profile's:

```toml
[agent.profile]
system_prompt = "You are a general assistant."   # default from blueprint

[agent]
system_prompt = "You are a Python specialist."   # overrides the profile
```

Resolution order: `[agent].system_prompt` &gt; `[agent.profile].system_prompt` &gt; empty string

### Thinking Level

Controls depth of model reasoning. Specified as a string in config:

| Config Value | Rust Enum | Description |
|-------------|-----------|-------------|
| `"off"` | `ThinkingLevel::Off` | No chain-of-thought (default) |
| `"minimal"` | `ThinkingLevel::Minimal` | Lightweight reasoning |
| `"low"` | `ThinkingLevel::Low` | Some reasoning |
| `"medium"` | `ThinkingLevel::Medium` | Moderate reasoning |
| `"high"` | `ThinkingLevel::High` | Deep reasoning before responding |

Parsing is case-insensitive: `"High"`, `"HIGH"`, `"high"` all work.

### Skills vs Tools

`skills` in the profile are **skill names** loaded via `SkillSet` from `SKILL.md` files (per the [AgentSkills standard](https://agentskills.io)). They are NOT tools. See [Skills](../concepts/skills.md) for details.

---

## Provider Configuration

The `[provider]` section defines the LLM model, API credentials, and protocol.

```toml
[provider]
model = "claude-sonnet-4-20250514"    # Model ID sent to the API
api_key = "${ANTHROPIC_API_KEY}"      # API credential
api = "anthropic_messages"            # API protocol
provider = "anthropic"                # Provider name
name = "Claude Sonnet 4"             # Human-friendly display name
reasoning = true                      # Model supports thinking
context_window = 200000               # Context window in tokens
max_tokens = 8192                     # Default max output tokens
```

### API Protocols

| Config Value | Aliases | Protocol |
|-------------|---------|----------|
| `"anthropic_messages"` | `"anthropic"` | Anthropic Messages API |
| `"openai_completions"` | `"openai"` | OpenAI Chat Completions |
| `"openai_responses"` | | OpenAI Responses API |
| `"azure_openai_responses"` | `"azure"` | Azure OpenAI |
| `"google_generative_ai"` | `"google"`, `"gemini"` | Google Gemini |
| `"google_vertex"` | `"vertex"` | Google Vertex AI |
| `"bedrock_converse_stream"` | `"bedrock"` | Amazon Bedrock |

**Default base URLs** are set automatically per protocol when `base_url` is omitted:
- Anthropic: `https://api.anthropic.com`
- OpenAI: `https://api.openai.com`
- Google: `https://generativelanguage.googleapis.com`
- Others: empty (uses provider defaults)

**Important:** The API protocol is NOT auto-detected from the model name. If you set `model = "gpt-4o"`, you must also set `api = "openai"` explicitly.

### Cost Rates

Enable cost tracking by setting per-token rates:

```toml
[provider.cost]
input_per_million = 3.0       # $ per million input tokens
output_per_million = 15.0     # $ per million output tokens
cache_read_per_million = 0.3  # $ per million cache-read tokens
cache_write_per_million = 3.75
```

Cost is tracked automatically after each turn. Combine with `[execution].max_cost` to enforce a budget.

### Custom Headers

```toml
[provider]
model = "my-model"

[provider.headers]
"X-Custom-Header" = "value"
"Authorization" = "Bearer ${CUSTOM_TOKEN}"
```

### Multiple Providers

Use `[[provider.instances]]` to define named providers alongside the default:

```toml
# Default provider (used unless overridden)
[provider]
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

# Named OpenAI instance
[[provider.instances]]
name = "openai"
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"
api = "openai_completions"

# Named local instance
[[provider.instances]]
name = "local"
model = "llama-3"
api = "openai_completions"
base_url = "http://localhost:8080/v1"
api_key = "not-needed"
```

Agent instances and sub-agents can reference these by name (e.g., `provider = "openai"`).

---

## Session Configuration

The `[session]` section controls session scope and provides session-level overrides.

```toml
[session]
scope = "persistent"       # "ephemeral" (default) or "persistent"
thinking_level = "high"    # Overrides the profile's thinking_level
temperature = 0.1          # Overrides the profile's temperature
```

### Session Scope

| Value | Behavior |
|-------|----------|
| `"ephemeral"` | Session exists only in memory for the process lifetime (default) |
| `"persistent"` | Session data is written to a store and survives restarts |

**Note:** Setting `scope = "persistent"` declares intent but does not automatically configure a storage backend. The caller must set up session persistence using the [session recorder](../concepts/sessions.md).

### Override Resolution Order

For thinking level and temperature, session overrides take precedence:

```
Session override > Profile value > Crate default
```

This is implemented via `AgentProfile::resolve_thinking_level()` and `AgentProfile::resolve_temperature()`.

---

## Tools

The `[tools]` section declares which tools the agent can use and how they execute.

```toml
[tools]
enabled = ["bash", "file_read", "file_write", "search"]
tool_strategy = "parallel"   # "sequential", "parallel", or "batched"
batch_size = 3               # Only used when strategy is "batched"
```

### Tool Execution Strategies

| Strategy | Behavior |
|----------|----------|
| `"sequential"` | One tool at a time; checks steering queue between each |
| `"parallel"` | All tool calls concurrent; check steering after all complete (default) |
| `"batched"` | Run N concurrent, wait, check steering, next batch |

### Registering Tools at Runtime

**Tools are NOT instantiated from the config file.** The config specifies tool *names* only. You must register tool instances after constructing the agent:

```rust
use phi_core::{parse_config_file, agent_from_config, Agent};
use phi_core::tools::{BashTool, ReadFileTool, WriteFileTool, SearchTool};
use std::sync::Arc;

let config = parse_config_file(Path::new("agent.toml"))?;
let agent = agent_from_config(&config)?;

// Cast to mutable and register tools
let agent_mut = Arc::get_mut(&mut agent).unwrap();
agent_mut.set_tools(vec![
    Arc::new(BashTool::default()),
    Arc::new(ReadFileTool::new()),
    Arc::new(WriteFileTool::new()),
    Arc::new(SearchTool::new()),
]);
```

Tool instantiation from config names is tracked as a future feature (G10 — Tool Registry).

---

## Context & Compaction

The `[compaction]` section controls automatic context management. When the conversation grows too long, compaction summarizes older messages to stay within the model's context window.

```toml
[compaction]
max_context_tokens = 200000     # Model's context window
system_prompt_tokens = 4000     # Tokens reserved for system prompt
compact_at_pct = 0.85           # Start measuring at 85% capacity
compact_budget_threshold_pct = 0.05  # Compact when < 5% headroom remains
keep_first_turns = 2            # Keep first 2 turns verbatim
keep_recent_turns = 4           # Keep last 4 turns verbatim
max_summary_tokens = 2000       # Token budget for the summarized middle
tool_output_max_lines = 50      # Truncate tool outputs to 50 lines
```

**Compaction must be explicitly enabled** by setting `max_context_tokens`. If omitted, compaction is disabled entirely.

### How Compaction Works

1. Before each LLM turn, the loop estimates current token usage
2. If usage exceeds the trigger threshold, compaction fires
3. First N turns are kept verbatim (preserves initial context)
4. Middle turns are summarized (aggressive token reduction)
5. Last M turns are kept verbatim (preserves recent history)
6. Tool outputs in kept turns are truncated to `max_lines`

See [Context Compaction](../concepts/compaction.md) for the full algorithm.

---

## Execution Limits

The `[execution]` section sets safety guards that prevent runaway loops and budget overruns.

```toml
[execution]
max_turns = 50              # Maximum LLM turns (default: 50)
max_total_tokens = 1000000  # Total token budget (default: 1,000,000)
max_duration_secs = 600     # Wall-clock timeout in seconds (default: 600)
max_cost = 5.0              # Dollar cost cap (requires [provider.cost] rates)
```

### Cost Tracking

Cost enforcement requires both cost rates and a budget:

```toml
[provider.cost]
input_per_million = 3.0
output_per_million = 15.0

[execution]
max_cost = 5.0   # Stop when accumulated cost reaches $5
```

Without cost rates (all zeros), `max_cost` has no effect. Token usage is always tracked regardless.

### Retry Configuration

Automatic retry for transient provider errors (rate limits, network issues):

```toml
[execution.retry]
max_retries = 3           # Retry attempts (default: 3, 0 = disabled)
initial_delay_ms = 1000   # First retry delay in ms
backoff_multiplier = 2.0  # Exponential backoff multiplier
max_delay_ms = 30000      # Maximum delay cap
```

Only `RateLimited` and `Network` errors are retried. Invalid requests and context overflows fail immediately.

### Cache Configuration

Control prompt caching behavior:

```toml
[execution.cache]
enabled = true        # Master switch (default: true)
strategy = "auto"     # "auto" or "disabled"
```

---

## Sub-Agents

Define sub-agents that run their own agent loops when invoked as tools:

```toml
[[sub_agents.instances]]
name = "researcher"
description = "Searches the web for information"
system_prompt = "You are a research assistant. Search thoroughly."
model = "claude-haiku-4-5-20251001"
max_turns = 10
tools = ["web_search"]

[[sub_agents.instances]]
name = "code_writer"
description = "Writes and edits code files"
system_prompt = "You are a code generation expert."
provider = "openai"    # References a [[provider.instances]] by name
max_turns = 20
tools = ["bash", "file_write"]
```

**Sub-agents do NOT inherit** the parent agent's configuration. Each sub-agent is fully independent — set all needed fields explicitly.

---

## Multi-Agent Configurations

For complex setups, combine named providers with named agent instances:

```toml
# Providers
[provider]
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[[provider.instances]]
name = "fast"
model = "claude-haiku-4-5-20251001"
api_key = "${ANTHROPIC_API_KEY}"

# Agent instances
[[agent.instances]]
name = "planner"
system_prompt = "You are an architect. Plan the approach."
provider = "fast"

[[agent.instances]]
name = "executor"
system_prompt = "You are an implementer. Write the code."
```

---

## Callbacks & Hooks (Phase 2)

The config schema accepts `[callbacks]` and `[hooks]` sections for lifecycle hooks:

```toml
[callbacks]
before_loop = "my_plugin::before_loop"
after_turn = "my_plugin::after_turn"

[hooks]
transform_context = "my_plugin::transform"
```

**These are not yet active.** In Phase 1, the config parser accepts these strings but `agent_from_config()` ignores them. WASM plugin loading will activate them in Phase 2.

To set hooks programmatically today, use the `Agent` trait setter methods after construction:

```rust
let agent = agent_from_config(&config)?;
let agent_mut = Arc::get_mut(&mut agent).unwrap();
agent_mut.set_before_loop(Some(Arc::new(|msgs, n| {
    println!("Loop starting with {} messages", msgs.len());
    true // return false to abort
})));
```

---

## Complete Example

A full coding agent configuration using every section:

```toml
# ── Agent identity ────────────────────────────────────────────
[agent]
system_prompt = "You are an expert software engineer."

[agent.profile]
name = "coding-agent"
description = "Full-featured coding assistant"
thinking_level = "high"
temperature = 0.2
max_tokens = 16384
config_id = "coder-v1"
skills = ["code-review"]

# ── Provider ──────────────────────────────────────────────────
[provider]
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"
reasoning = true
context_window = 200000

[provider.cost]
input_per_million = 3.0
output_per_million = 15.0
cache_read_per_million = 0.3
cache_write_per_million = 3.75

# ── Session ───────────────────────────────────────────────────
[session]
scope = "persistent"

# ── Tools ─────────────────────────────────────────────────────
[tools]
enabled = ["bash", "file_read", "file_write", "search", "edit_file"]
tool_strategy = "parallel"

# ── Context management ────────────────────────────────────────
[compaction]
max_context_tokens = 200000
system_prompt_tokens = 4000
compact_at_pct = 0.85
keep_first_turns = 2
keep_recent_turns = 4
max_summary_tokens = 2000
tool_output_max_lines = 50

# ── Execution limits ──────────────────────────────────────────
[execution]
max_turns = 100
max_total_tokens = 2000000
max_duration_secs = 1800
max_cost = 10.0

[execution.retry]
max_retries = 3
initial_delay_ms = 1000
backoff_multiplier = 2.0

[execution.cache]
enabled = true
strategy = "auto"

# ── Sub-agents ────────────────────────────────────────────────
[[sub_agents.instances]]
name = "researcher"
description = "Searches for information and documentation"
system_prompt = "Find relevant information. Be thorough."
model = "claude-haiku-4-5-20251001"
max_turns = 10
tools = ["web_search"]
```

---

## Field Reference

### `[agent]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `system_prompt` | string | None | Agent-level system prompt (overrides profile) |
| `profile` | table | (empty) | Profile blueprint (see below) |
| `instances` | array | `[]` | Named agent instances |

### `[agent.profile]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `profile_id` | string | UUID | Unique profile identifier |
| `name` | string | None | Human-readable name |
| `description` | string | None | Profile description |
| `system_prompt` | string | None | Default system prompt |
| `thinking_level` | string | None | `"off"`, `"minimal"`, `"low"`, `"medium"`, `"high"` |
| `temperature` | float | None | LLM temperature (0.0-2.0) |
| `max_tokens` | integer | None | Max output tokens |
| `config_id` | string | None | Stable identity for loop_id generation |
| `skills` | array | `[]` | Skill names (SKILL.md, not tools) |

### `[provider]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `"unknown"` | Model ID sent to API |
| `api_key` | string | `""` | API credential (supports `${VAR}`) |
| `api` | string | `"anthropic_messages"` | API protocol |
| `base_url` | string | (per protocol) | API base URL |
| `provider` | string | `"anthropic"` | Provider name |
| `name` | string | model value | Display name |
| `reasoning` | bool | `false` | Supports thinking/reasoning |
| `context_window` | integer | `200000` | Context window tokens |
| `max_tokens` | integer | `8192` | Default max output tokens |

### `[provider.cost]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `input_per_million` | float | `0.0` | Input token rate |
| `output_per_million` | float | `0.0` | Output token rate |
| `cache_read_per_million` | float | `0.0` | Cache read rate |
| `cache_write_per_million` | float | `0.0` | Cache write rate |

### `[session]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `scope` | string | `"ephemeral"` | `"ephemeral"` or `"persistent"` |
| `thinking_level` | string | None | Session-level override |
| `temperature` | float | None | Session-level override |

### `[tools]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | array | `[]` | Tool names (resolved by caller) |
| `tool_strategy` | string | `"parallel"` | `"sequential"`, `"parallel"`, `"batched"` |
| `batch_size` | integer | `3` | Batch size for `"batched"` strategy |

### `[compaction]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_context_tokens` | integer | None | Context window (must set to enable compaction) |
| `system_prompt_tokens` | integer | `4000` | Reserved system prompt tokens |
| `compact_at_pct` | float | `0.90` | Measurement threshold |
| `compact_budget_threshold_pct` | float | `0.05` | Compaction trigger |
| `keep_first_turns` | integer | `2` | Verbatim turns from start |
| `keep_recent_turns` | integer | `10` | Verbatim turns from end |
| `max_summary_tokens` | integer | `2000` | Summary token budget |
| `tool_output_max_lines` | integer | `50` | Tool output line cap |

### `[execution]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_turns` | integer | `50` | Maximum LLM turns |
| `max_total_tokens` | integer | `1000000` | Total token budget |
| `max_duration_secs` | integer | `600` | Wall-clock timeout (seconds) |
| `max_cost` | float | None | Dollar cost cap |

### `[execution.retry]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_retries` | integer | `3` | Retry attempts (0 = disabled) |
| `initial_delay_ms` | integer | `1000` | First retry delay (ms) |
| `backoff_multiplier` | float | `2.0` | Exponential backoff factor |
| `max_delay_ms` | integer | `30000` | Maximum delay cap (ms) |

### `[execution.cache]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Master switch |
| `strategy` | string | `"auto"` | `"auto"` or `"disabled"` |

---

## Error Handling

`agent_from_config()` and the parse functions return `ConfigError`:

| Variant | Cause | Fix |
|---------|-------|-----|
| `Parse(msg)` | Invalid TOML/JSON/YAML syntax | Check syntax; the message includes the parser error |
| `MissingEnvVar { var }` | `${VAR}` references an unset env var | Set the variable or remove the reference |
| `InvalidField { field, value, expected }` | Invalid enum value (e.g., `thinking_level = "extreme"`) | Use one of the expected values |
| `Io(err)` | File not found or not readable | Check file path and permissions |

### Common Mistakes

**Forgetting to set the API protocol for non-Anthropic models:**
```toml
# Wrong — defaults to anthropic_messages, fails at runtime
[provider]
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"

# Correct
[provider]
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"
api = "openai"
```

**Setting max_cost without cost rates:**
```toml
# max_cost is ignored — no rates to compute cost from
[execution]
max_cost = 5.0

# Correct — set rates AND budget
[provider.cost]
input_per_million = 3.0
output_per_million = 15.0

[execution]
max_cost = 5.0
```

**Expecting tools to be instantiated from config:**
```toml
[tools]
enabled = ["bash", "file_read"]
# These are names only — you must call agent.set_tools() in Rust
```

---

## Programmatic Usage

### Using AgentConfig Directly

You can construct `AgentConfig` in Rust without a file:

```rust
use phi_core::config::schema::{AgentConfig, ProviderSection, ProfileSection, AgentSection};

let config = AgentConfig {
    provider: ProviderSection {
        model: Some("claude-sonnet-4-20250514".into()),
        api_key: Some(std::env::var("ANTHROPIC_API_KEY")?),
        ..Default::default()
    },
    agent: AgentSection {
        system_prompt: Some("You are helpful.".into()),
        profile: ProfileSection {
            thinking_level: Some("high".into()),
            ..Default::default()
        },
        ..Default::default()
    },
    ..Default::default()
};

let agent = agent_from_config(&config)?;
```

### Mixing Config with Programmatic Overrides

After `agent_from_config()`, use `Agent` trait methods to add hooks, tools, or modify settings:

```rust
use phi_core::{parse_config_file, agent_from_config, Agent};
use std::sync::Arc;

let config = parse_config_file(Path::new("agent.toml"))?;
let mut agent = agent_from_config(&config)?;

// Get mutable access to add tools and hooks
let a = Arc::get_mut(&mut agent).unwrap();
a.set_tools(vec![Arc::new(phi_core::tools::BashTool::default())]);
a.set_before_loop(Some(Arc::new(|msgs, _| {
    println!("Starting with {} messages", msgs.len());
    true
})));
```

### Reading Config Through the Agent Trait

All configuration is accessible through `Agent` trait methods:

```rust
let agent = agent_from_config(&config)?;

// Config accessors (all have defaults)
agent.model_config();       // Option<&ModelConfig>
agent.profile();            // Option<&AgentProfile>
agent.system_prompt();      // &str
agent.thinking_level();     // ThinkingLevel
agent.temperature();        // Option<f32>
agent.max_tokens();         // Option<u32>
agent.context_config();     // Option<&ContextConfig>
agent.execution_limits();   // Option<&ExecutionLimits>
agent.cache_config();       // CacheConfig
agent.tool_execution();     // ToolExecutionStrategy
agent.retry_config();       // RetryConfig
agent.session();            // Option<&Session>
agent.build_config();       // AgentLoopConfig (full loop config)
```
