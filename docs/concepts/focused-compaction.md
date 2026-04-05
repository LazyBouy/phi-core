<!-- Last verified: 2026-04-05 by Claude Code -->

# Focused Compaction

Focused compaction extends the [context compaction](compaction.md) system with two features: **focus messages** that steer what the compaction summary emphasizes, and **compaction instances** that let you define named compaction configurations reusable across agent profiles.

## Focus Message

The `focus_message` field on `CompactionConfig` is an optional string prepended to the compacted section before the LLM summarizes it. It tells the summarizer what to prioritize when condensing conversation history.

Without a focus message, compaction produces a generic summary. With one, the summary retains details relevant to your domain:

```rust
use phi_core::context::{ContextConfig, CompactionConfig};

let config = ContextConfig {
    max_context_tokens: 200_000,
    compaction: CompactionConfig {
        focus_message: Some(
            "Focus on specification details, API contracts, and architectural decisions.".to_string()
        ),
        ..Default::default()
    },
    ..Default::default()
};
```

The focus message does not change the compaction trigger logic (thresholds, turn counts). It only affects the content of the summarized middle section.

### When to use a focus message

- **Domain-specific agents**: An agent reviewing legal contracts should retain clause references, not general pleasantries.
- **Long coding sessions**: Focus on file paths, function signatures, and design rationale so the agent can continue working after compaction.
- **Research agents**: Preserve citations, data points, and methodology notes.

---

## Compaction Instances

Compaction instances are named variations of the compaction defaults, declared with `[[context.compaction.instances]]` in the config file. Each instance uses the `{{...}}` ID reference protocol to declare its name, and overrides specific fields from the parent `[compaction]` section. Fields not set on the instance fall through to the parent defaults.

### Config example

```toml
# ── Context config (max_context_tokens lives on ContextConfig, not CompactionConfig) ──
[context]
max_context_tokens = 200000

# ── Compaction defaults ─────────────────────────────────────────
[context.compaction]
compact_at_pct = 0.85
compact_budget_threshold_pct = 0.05
keep_first_turns = 2
keep_recent_turns = 4
max_summary_tokens = 2000
tool_output_max_lines = 50
focus_message = "Retain key decisions and code changes."

# ── Named compaction instances ──────────────────────────────────
[[context.compaction.instances]]
id = "{{%coding%}}"
description = "Compaction tuned for coding tasks"
focus_message = "Focus on file paths, function signatures, and design rationale."
keep_recent_turns = 6
max_summary_tokens = 3000

[[context.compaction.instances]]
id = "{{%research%}}"
description = "Compaction tuned for research tasks"
focus_message = "Preserve citations, data sources, and methodology."
keep_first_turns = 3
max_summary_tokens = 4000
```

### Referencing from an agent profile

Agent profiles reference a compaction instance via the `compaction` field, using the `{{...}}` ID protocol:

```toml
[agent.profile]
name = "coding-agent"
system_prompt = "You are an expert software engineer."
compaction = "{{compaction.coding}}"

[[agent.profile.instances]]
id = "{{%researcher%}}"
description = "A research-focused profile variant"
compaction = "{{compaction.research}}"
```

When the agent is constructed from config, the referenced compaction instance is resolved and its fields are merged with the compaction defaults to produce the final `CompactionConfig`.

---

## Programmatic Usage

When building agents in Rust without a config file, focused compaction is set directly on `CompactionConfig`:

```rust
use phi_core::context::CompactionConfig;
use phi_core::agent_loop::AgentLoopConfig;
use phi_core::provider::ModelConfig;

let context = phi_core::context::ContextConfig {
    max_context_tokens: 200_000,
    compaction: CompactionConfig {
        compact_at_pct: 0.85,
        compact_budget_threshold_pct: 0.05,
        keep_first_turns: 2,
        keep_recent_turns: 6,
        max_summary_tokens: 3_000,
        tool_output_max_lines: 50,
        focus_message: Some(
            "Focus on file paths, function signatures, and design rationale.".to_string()
        ),
        ..Default::default()
    },
    ..Default::default()
};

let config = AgentLoopConfig {
    model_config: ModelConfig::anthropic("claude-sonnet-4-20250514", "Sonnet", &api_key),
    context_config: Some(context),
    ..Default::default()
};
```

Or via `BasicAgent` builder methods:

```rust
use phi_core::{BasicAgent, context::CompactionConfig};
use phi_core::provider::ModelConfig;

let agent = BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Sonnet", &api_key))
    .with_context_config(phi_core::context::ContextConfig {
        max_context_tokens: 200_000,
        compaction: CompactionConfig {
            focus_message: Some("Retain specification details and API contracts.".to_string()),
            ..Default::default()
        },
        ..Default::default()
    });
```

---

## Summary

| Feature | Purpose |
|---------|---------|
| `focus_message` | Steers compaction summarization toward domain-relevant content |
| `[[compaction.instances]]` | Named compaction configurations with `{{...}}` ID protocol |
| Profile `compaction` field | Links an agent profile to a specific compaction instance |
