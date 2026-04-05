<!-- Last verified: 2026-04-05 by Claude Code -->
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

The system prompt is resolved through a priority chain. The first non-empty value wins:

1. `[agent].system_prompt` — explicit agent-level override (highest priority)
2. Profile instance `system_prompt` — when an agent instance references a profile via `{{agent_profile.name}}`
3. `[agent.profile].system_prompt` — base inline profile fallback
4. Empty string (no system prompt)

#### Inline Text

The simplest form — write the prompt directly:

```toml
[agent.profile]
system_prompt = "You are an expert software engineer."
```

Agent-level overrides the profile:

```toml
[agent.profile]
system_prompt = "You are a general assistant."   # default from blueprint

[agent]
system_prompt = "You are a Python specialist."   # overrides the profile
```

#### File Reference (`file:` prefix)

Load the prompt from a file. Relative paths resolve from the agent's `workspace` directory:

```toml
[agent]
workspace = "workspace"

[agent.profile]
system_prompt = "file:system_prompt.md"        # resolves to workspace/system_prompt.md
```

Absolute paths are used as-is:

```toml
[agent.profile]
system_prompt = "file:/etc/phi/prompts/coder.md"
```

The `file:` prefix works at all levels: `[agent].system_prompt`, `[agent.profile].system_prompt`, and `[[agent.profile.instances]].system_prompt`.

#### Strategy Reference (`{{...}}` protocol) {#system-prompt-strategy}

For advanced multi-block prompt composition, reference a system prompt instance. This uses a 3-entity chain: **strategy** (block template) → **prompt instance** (block content) → **agent reference**.

```toml
# 1. Define the strategy template — block structure with ordering and size limits
[[system_prompt_strategy.instances]]
id = "{{coding_strategy}}"

[[system_prompt_strategy.instances.blocks]]
name = "identity"
order = 0
max_length = 2000

[[system_prompt_strategy.instances.blocks]]
name = "instructions"
order = 1
max_length = 3000

[[system_prompt_strategy.instances.blocks]]
name = "constraints"
order = 2
max_length = 1000

# 2. Define the prompt instance — fills content into the strategy's blocks
#    Block values can be inline text or file: references (relative to workspace)
[[system_prompt.instances]]
id = "{{coding_prompt}}"
description = "System prompt for coding agents"
type = "{{system_prompt_strategy.coding_strategy}}"
identity = "You are an expert software engineer at a fintech company."
instructions = "file:prompts/coding_instructions.md"
constraints = "Never modify production databases. Always write tests."

# 3. Reference the prompt instance from the agent
[agent]
system_prompt = "{{system_prompt.coding_prompt}}"
workspace = "workspace"
```

The builder resolves the chain: finds the prompt instance → finds its strategy → sorts blocks by `order` → resolves `file:` paths → truncates each block to `max_length` → joins with double newlines.

See the Field Reference for [`[system_prompt_strategy]`](#system_prompt_strategy) and [`[system_prompt]`](#system_prompt) sections.

#### Profile Instance Override

When using named profile instances, the instance's `system_prompt` participates in resolution. The `system_prompt` field on a profile instance supports all three modes — inline text, `file:` path, or `{{...}}` reference to a system_prompt instance:

```toml
# ── System prompt strategy + instance (reusable prompt definition) ───
[[system_prompt_strategy.instances]]
id = "{{simple}}"

[[system_prompt_strategy.instances.blocks]]
name = "identity"
order = 0
max_length = 5000

[[system_prompt.instances]]
id = "{{coder_prompt}}"
type = "{{system_prompt_strategy.simple}}"
identity = "file:prompts/coder.md"

[[system_prompt.instances]]
id = "{{reviewer_prompt}}"
type = "{{system_prompt_strategy.simple}}"
identity = "file:prompts/reviewer.md"

# ── Profile instances reference system_prompt instances ──────────────
[agent.profile]
name = "base"
system_prompt = "You are a general assistant."   # base fallback

[[agent.profile.instances]]
id = "{{coder}}"
system_prompt = "{{system_prompt.coder_prompt}}"   # profile → system_prompt instance
temperature = 0.2
max_tokens = 16384

[[agent.profile.instances]]
id = "{{reviewer}}"
system_prompt = "{{system_prompt.reviewer_prompt}}" # profile → system_prompt instance
temperature = 0.1
max_tokens = 8192

# ── Agent instances reference profile instances ──────────────────────
[[agent.instances]]
name = "code-writer"
agent_profile = "{{agent_profile.coder}}"          # agent → profile → system_prompt

[[agent.instances]]
name = "code-reviewer"
agent_profile = "{{agent_profile.reviewer}}"       # agent → profile → system_prompt

[[agent.instances]]
name = "generalist"
# no agent_profile → uses base [agent.profile].system_prompt
```

**Full reference chain:** `agent.instances` → `agent.profile.instances` (via `agent_profile`) → `system_prompt.instances` (via `system_prompt`) → `system_prompt_strategy.instances` (via `type`). Each layer can override or inherit from the one above.

When an agent instance omits `agent_profile`, it is built using the base `[agent.profile]` directly (no instance override). The base profile's `system_prompt`, `temperature`, and other fields apply as defaults.

#### Workspace-relative Resolution

The `file:` prefix resolves relative to the agent's `workspace` directory. Each agent instance can set its own workspace, so the same `file:` reference resolves to different files per agent:

```toml
[agent.profile]
name = "base"

[[agent.profile.instances]]
id = "{{copywriter}}"
system_prompt = "file:system_prompt.md"   # same file ref, different workspace resolution
temperature = 0.7

[[agent.instances]]
name = "alpha-writer"
agent_profile = "{{agent_profile.copywriter}}"
workspace = "projects/alpha"              # reads projects/alpha/system_prompt.md

[[agent.instances]]
name = "beta-writer"
agent_profile = "{{agent_profile.copywriter}}"
workspace = "projects/beta"               # reads projects/beta/system_prompt.md
```

**Workspace resolution order:**
1. `[[agent.instances]].workspace` — per-agent instance (highest priority)
2. `[agent].workspace` — shared agent-level
3. `default_workspace` — global default
4. `"."` — current directory

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

## Profile Instances

Profile instances are **named variations** of the profile blueprint. Each instance inherits the profile defaults and overrides specific fields. This lets you define a single profile and then create specialized variants without duplicating the entire configuration.

Use `[[agent.profile.instances]]` to define instances. Each instance requires an `id` field using the `{{...}}` ID reference protocol (see below). Instance fields override the corresponding profile defaults; any field not specified falls through to the profile value.

Agent instances reference a profile instance via the `agent_profile` field, using either a qualified reference (`{{agent_profile.name}}`) or an unqualified reference (`{{name}}`) if the name is unique across all namespaces.

### Example

```toml
# ── Profile defaults ──────────────────────────────────────────
[agent.profile]
name = "coding-agent"
description = "An agent specialized for code tasks"
system_prompt = "You are an expert software engineer."
thinking_level = "high"
temperature = 0.2
max_tokens = 16384

# ── Profile instances (override specific fields) ─────────────
[[agent.profile.instances]]
id = "{{%coder%}}"
description = "A code generation specialist"
thinking_level = "high"
temperature = 0.2
max_tokens = 16384
config_id = "coder"

[[agent.profile.instances]]
id = "{{%reviewer%}}"
description = "A code review specialist"
thinking_level = "high"
temperature = 0.1
max_tokens = 8192
config_id = "reviewer"

# ── Agent instances referencing profile instances ─────────────
[[agent.instances]]
name = "code-writer"
agent_profile = "{{agent_profile.coder}}"
system_prompt = "You write clean, well-tested code. Follow existing patterns."

[[agent.instances]]
name = "code-reviewer"
agent_profile = "{{agent_profile.reviewer}}"
system_prompt = "You review code for bugs, security issues, and style violations."
```

The `code-writer` agent inherits all profile defaults and applies the `coder` instance overrides. The `code-reviewer` agent uses the `reviewer` instance, which sets a lower temperature and smaller token budget for more focused review output.

---

## ID Reference Protocol

The `{{...}}` syntax is a lightweight reference protocol for linking configuration entities (providers, profile instances, sub-agents) by name. It appears in `id` fields (to declare an entity) and in reference fields like `provider` and `agent_profile` (to point to an entity).

### Syntax

| Pattern | Meaning |
|---------|---------|
| `{{type.name}}` | Qualified reference, recreate if invoked |
| `{{%type.name%}}` | Qualified reference, no recreation if already exists |
| `{{name}}` | Unqualified reference (unique resolve), recreate if invoked |
| `{{%name%}}` | Unqualified reference, no recreation if already exists |
| `{{#system_id#}}` | Literal system ID, no recreation |

### Namespaces

References are resolved within namespaces. The three namespaces are:

- **`agent_profile`** -- Profile instances declared in `[[agent.profile.instances]]`
- **`provider`** -- Provider instances declared in `[[provider.instances]]`
- **`sub_agent`** -- Sub-agent instances declared in `[[sub_agents.instances]]`

### Resolution

**Qualified references** (`{{type.name}}`) include the namespace prefix and always resolve unambiguously. Use these when multiple namespaces could contain the same name.

**Unqualified references** (`{{name}}`) omit the namespace. The system searches all namespaces and resolves the reference only if the name is unique. If multiple entities share the same name across namespaces, an unqualified reference is ambiguous and will produce an error.

### Recreation Semantics

The `%` sigil controls whether an entity is recreated when referenced:

- **Without `%`** (`{{name}}` or `{{type.name}}`): The entity is recreated each time it is resolved. Use this when you want fresh instances.
- **With `%`** (`{{%name%}}` or `{{%type.name%}}`): The entity is reused if it already exists (matched by latest creation date). Use this for shared singletons like provider connections.

The `{{#system_id#}}` form references a literal system-generated ID and never triggers recreation.

### Usage in ID Fields

When declaring an entity, the `id` field establishes the entity's name within its namespace:

```toml
[[provider.instances]]
id = "{{%openai%}}"          # declares "openai" in the provider namespace
model = "gpt-4o"
```

### Usage in Reference Fields

When referencing an entity from another section, use the reference syntax:

```toml
[[agent.instances]]
name = "my-agent"
provider = "{{provider.openai}}"       # qualified reference
agent_profile = "{{reviewer}}"         # unqualified (must be unique)
```

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

Use `[[provider.instances]]` to define named providers alongside the default. Each instance uses the `{{...}}` ID reference protocol to declare its name in the `provider` namespace. The `url` field is an alias for `base_url`.

```toml
# Default provider — Anthropic (used unless overridden)
[provider]
model = "claude-sonnet-4-20250514"
name = "Claude Sonnet 4"
api_key = "${ANTHROPIC_API_KEY}"
api = "anthropic_messages"
provider = "anthropic"

[provider.cost]
input_per_million = 3.0
output_per_million = 15.0
cache_read_per_million = 0.3
cache_write_per_million = 3.75

# OpenAI
[[provider.instances]]
id = "{{%openai%}}"
description = "OpenAI GPT-4o provider"
name = "GPT-4o"
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"
api = "openai_completions"
url = "https://api.openai.com/v1"

# OpenRouter
[[provider.instances]]
id = "{{%openrouter%}}"
description = "OpenRouter multi-model gateway"
name = "OpenRouter"
model = "anthropic/claude-sonnet-4"
api_key = "${OPENROUTER_API_KEY}"
api = "openai_completions"
url = "https://openrouter.ai/api/v1"
provider = "openrouter"

# Google Gemini
[[provider.instances]]
id = "{{%gemini%}}"
description = "Google Gemini 2.5 Flash provider"
name = "Gemini 2.5 Flash"
model = "gemini-2.5-flash"
api_key = "${GOOGLE_API_KEY}"
api = "google_generative_ai"

# Ollama (local)
[[provider.instances]]
id = "{{%ollama%}}"
description = "Local Ollama instance for development"
name = "Ollama Llama 3.2"
model = "llama3.2"
api = "openai_completions"
url = "http://localhost:11434/v1"
api_key = "not-needed"
provider = "ollama"
```

Agent instances and sub-agents reference these via the ID protocol (e.g., `provider = "{{provider.openai}}"` or `provider = "{{ollama}}"` if unique).

---

## Session Configuration

The `[session]` section controls session scope.

```toml
[session]
scope = "persistent"       # "ephemeral" (default) or "persistent"
```

### Session Scope

| Value | Behavior |
|-------|----------|
| `"ephemeral"` | Session exists only in memory for the process lifetime (default) |
| `"persistent"` | Session data is written to a store and survives restarts |

**Note:** Setting `scope = "persistent"` declares intent but does not automatically configure a storage backend. The caller must set up session persistence using the [session recorder](../concepts/sessions.md).

Thinking level and temperature are configured per-loop via `LoopConfigSnapshot` (captured on each `AgentStart` event) rather than at the session level. Set them on the agent profile or `AgentLoopConfig`.

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

### Context Pruning

Enable model-directed context pruning with `with_prun_tool()`. This lets the model surgically remove irrelevant inrun content (its own messages, tool calls, tool results) from the working context to reclaim space in the context window. User messages are never pruned. See [Context Pruning](../concepts/context-pruning.md) for details.

```rust
let agent = BasicAgent::new(model_config)
    .with_default_tools()
    .with_prun_tool();
```

Or via config:

```toml
[tools]
enabled = ["bash", "read_file", "write_file", "prun"]
```

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

### Tool Registry

Instead of manually registering tools after construction, use `agent_from_config_with_registry()` to resolve tool names from the config automatically:

```rust
use phi_core::{parse_config_file, agent_from_config_with_registry, Agent};
use phi_core::tools::ToolRegistry;
use std::path::Path;

let config = parse_config_file(Path::new("agent.toml"))?;

// Create a registry with the 6 built-in tools
let registry = ToolRegistry::new().with_defaults();

// Tools listed in config.tools.enabled are resolved through the registry
let agent = agent_from_config_with_registry(&config, &registry)?;
```

The default registry includes all 6 built-in tools: `bash`, `read_file`, `write_file`, `edit_file`, `list_files`, `search`. You can also register custom tools:

```rust
let mut registry = ToolRegistry::new().with_defaults();
registry.register("my_tool", || Arc::new(MyCustomTool::new()));

let agent = agent_from_config_with_registry(&config, &registry)?;
```

Unknown tool names in `tools.enabled` are silently skipped. Use `registry.contains(name)` to check availability before construction if needed.

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

### Focused Compaction

The `focus_message` field steers what the compaction summary emphasizes. Compaction instances let you define named variations that agent profiles can reference.

```toml
[compaction]
max_context_tokens = 200000
focus_message = "Retain key decisions and code changes."

# Named compaction instances
[[compaction.instances]]
id = "{{%coding%}}"
focus_message = "Focus on file paths, function signatures, and design rationale."
keep_recent_turns = 6
max_summary_tokens = 3000

[[compaction.instances]]
id = "{{%research%}}"
focus_message = "Preserve citations, data sources, and methodology."
keep_first_turns = 3
max_summary_tokens = 4000
```

Profiles reference a compaction instance via `compaction = "{{compaction.coding}}"`:

```toml
[agent.profile]
name = "coding-agent"
compaction = "{{compaction.coding}}"
```

See [Focused Compaction](../concepts/focused-compaction.md) for full details.

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

## Agent Workspace

The `workspace` field sets the working directory for an agent. Tools that interact with the filesystem (bash, file read/write, etc.) use this as their base path.

There are two levels of workspace configuration:

- **`default_workspace`** (top-level config field): Sets the default workspace for all agents. If omitted, the current working directory is used.
- **`workspace`** (per-agent field on `[agent.profile]` or `[[agent.instances]]`): Overrides `default_workspace` for a specific agent.

```toml
default_workspace = "/home/user/projects"

[agent.profile]
workspace = "/home/user/projects/my-app"   # overrides default_workspace for this agent
```

---

## Callbacks & Hooks

The config schema accepts `[callbacks]` and `[hooks]` sections for lifecycle hooks:

```toml
[callbacks]
before_loop = "my_plugin::before_loop"
after_turn = "my_plugin::after_turn"
before_task = "./scripts/on_task_start.sh"
after_task = "python3 scripts/after_task.py"

[hooks]
transform_context = "my_plugin::transform"
```

Script-based callbacks (shell scripts, Python scripts) are supported. The agent spawns the script as a subprocess, passing context via environment variables. Exit code 0 means continue; non-zero aborts the action (for `Before*` hooks). WASM plugin loading for Rust-native callbacks is planned for Phase 2.

### Session-Level Callbacks

`before_task` and `after_task` are session-level callbacks configured on `SessionRecorderConfig`:

- **`before_task`**: Fires on the first `AgentStart` event with a new `session_id`. Use for task-level setup, metrics initialization, or audit logging.
- **`after_task`**: Fires on `flush()`. Use for task-level teardown, billing, or summary generation.

### Programmatic Hooks

To set hooks programmatically, use the `Agent` trait setter methods after construction:

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
| `system_prompt` | string | None | Agent-level system prompt (overrides profile). Supports: inline text, `file:path` (relative to workspace), or `{{...}}` reference to a `[[system_prompt.instances]]` entry. |
| `profile` | table | (empty) | Profile blueprint (see below) |
| `workspace` | string | None | Workspace directory for `file:` resolution and tool paths |
| `instances` | array | `[]` | Named agent instances |

### `[agent.profile]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `profile_id` | string | UUID | Unique profile identifier |
| `name` | string | None | Human-readable name |
| `description` | string | None | Profile description |
| `system_prompt` | string | None | Default system prompt. Supports: inline text, `file:path`, or `{{...}}` reference. |
| `thinking_level` | string | None | `"off"`, `"minimal"`, `"low"`, `"medium"`, `"high"` |
| `temperature` | float | None | LLM temperature (0.0-2.0) |
| `max_tokens` | integer | None | Max output tokens |
| `config_id` | string | None | Stable identity for loop_id generation |
| `skills` | array | `[]` | Skill names (SKILL.md, not tools) |
| `instances` | array | `[]` | Named profile instances (see `ProfileInstanceSection`) |

### `ProfileInstanceSection`

Each entry in `[[agent.profile.instances]]`:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | **required** | `{{...}}` ID in the `agent_profile` namespace |
| `description` | string | None | Human-readable description of this variant |
| `name` | string | (from profile) | Override name |
| `system_prompt` | string | (from profile) | Override system prompt (supports inline, `file:`, or `{{...}}`) |
| `thinking_level` | string | (from profile) | Override thinking level |
| `temperature` | float | (from profile) | Override temperature |
| `max_tokens` | integer | (from profile) | Override max output tokens |
| `config_id` | string | None | Stable identity for loop_id generation |
| `skills` | array | (from profile) | Override skill names |

### `AgentInstanceSection`

Each entry in `[[agent.instances]]`:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"unnamed"` | Instance name |
| `agent_profile` | string | None | Profile instance reference (`{{...}}` syntax) |
| `profile` | table | None | Inline profile override (not a reference) |
| `system_prompt` | string | None | Instance-specific system prompt |
| `provider` | string | (default provider) | Provider reference (`{{...}}` syntax) |
| `workspace` | string | None | Per-instance workspace directory (overrides `[agent].workspace`) |

### `[provider]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `"unknown"` | Model ID sent to API |
| `api_key` | string | `""` | API credential (supports `${VAR}`) |
| `api` | string | `"anthropic_messages"` | API protocol |
| `base_url` | string | (per protocol) | API base URL (`url` is an accepted alias) |
| `provider` | string | `"anthropic"` | Provider name |
| `name` | string | model value | Display name |
| `reasoning` | bool | `false` | Supports thinking/reasoning |
| `context_window` | integer | `200000` | Context window tokens |
| `max_tokens` | integer | `8192` | Default max output tokens |

### `ProviderInstanceSection`

Each entry in `[[provider.instances]]` accepts all fields from `[provider]` above, plus:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | None | `{{...}}` ID in the `provider` namespace |
| `description` | string | None | Human-readable description of this provider |
| `url` | string | None | Alias for `base_url` |

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

### `[system_prompt_strategy]` {#system_prompt_strategy}

Strategy templates define block structure for multi-block system prompts.

#### `[[system_prompt_strategy.instances]]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | **required** | `{{...}}` ID for this strategy template |
| `description` | string | None | Human-readable description |
| `blocks` | array | `[]` | Block definitions (see below) |

#### `[[system_prompt_strategy.instances.blocks]]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | **required** | Block name (e.g., `"identity"`, `"instructions"`, `"constraints"`) |
| `order` | integer | `0` | Assembly order — lower appears first in the composed prompt |
| `max_length` | integer | unlimited | Maximum character budget for this block |

### `[system_prompt]` {#system_prompt}

Prompt instances fill content into a strategy's blocks.

#### `[[system_prompt.instances]]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | **required** | `{{...}}` ID for this prompt instance |
| `description` | string | None | Human-readable description |
| `type` | string | None | `{{...}}` reference to a strategy instance (e.g., `"{{system_prompt_strategy.coding}}"`) |
| *(block names)* | string | — | Each block defined in the strategy gets a field here. Value is inline text or `"file:path"` (relative to workspace). |

**Note:** Block content fields use `#[serde(flatten)]` — they appear as top-level keys on the instance, not nested under a `blocks` table.

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

---

<!-- TODO: Add more configuration examples:
  - Multi-agent setup with different providers per agent
  - System prompt strategy with file-based blocks (full working example)
  - OpenRouter / Ollama local config with compat flags
  - Sub-agent with tool delegation — show how [[sub_agents.instances]] link to [[agent.instances]] and how parent agent delegates tasks to sub-agents via provider/tool references
  - Session-level callback scripts (before_task / after_task)
  - Per-instance workspace with file: system prompts
  - Compaction instances referenced by profile
  - Cost tracking and max_cost enforcement
-->
