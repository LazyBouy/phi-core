//! Config schema — the deserialization target for TOML/JSON/YAML config files.
//!
//! All structs use `#[serde(default)]` so omitted sections/fields get sensible defaults.
//! String fields are used for enum-like values (e.g. `thinking_level = "high"`) —
//! parsing to Rust enums happens in the builder.

use serde::Deserialize;
use std::collections::HashMap;

// ── Top-level config ────────────────────────────────────────────────────────

/// Top-level agent configuration. All sections are optional with defaults.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct AgentConfig {
    pub agent: AgentSection,
    pub provider: ProviderSection,
    pub session: SessionSection,
    pub tools: ToolsSection,
    pub skills: SkillsSection,
    pub sub_agents: SubAgentsSection,
    /// Phase 2 WASM — callback references stored as strings.
    pub callbacks: CallbacksSection,
    /// Phase 2 WASM — hook references stored as strings.
    pub hooks: HooksSection,
    pub compaction: CompactionSection,
    pub execution: ExecutionSection,
}

// ── Agent section ───────────────────────────────────────────────────────────

/// Agent-level configuration. `system_prompt` here overrides the profile's.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct AgentSection {
    /// Agent-level system prompt override. When set, takes precedence over
    /// `profile.system_prompt`.
    pub system_prompt: Option<String>,
    /// The agent's profile blueprint.
    pub profile: ProfileSection,
    /// Named agent instances (for multi-agent configs).
    pub instances: Vec<AgentInstanceSection>,
}

/// Profile section — the reusable agent blueprint.
/// Maps to `AgentProfile` in the builder. Multiple agent instances can share
/// the same profile.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ProfileSection {
    /// Unique profile identifier. Auto-generated if omitted.
    pub profile_id: Option<String>,
    /// Human-readable name.
    pub name: Option<String>,
    /// Description of the profile's purpose.
    pub description: Option<String>,
    /// Default system prompt for agents using this profile.
    pub system_prompt: Option<String>,
    /// Thinking level: "off", "minimal", "low", "medium", "high".
    pub thinking_level: Option<String>,
    /// Temperature for LLM calls.
    pub temperature: Option<f32>,
    /// Max output tokens.
    pub max_tokens: Option<u32>,
    /// Stable config identity for loop_id generation.
    pub config_id: Option<String>,
    /// Skill names loaded via SkillSet from SKILL.md files (NOT tools).
    pub skills: Vec<String>,
}

/// A named agent instance that can reference or override a profile.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct AgentInstanceSection {
    /// Instance name (for identification).
    pub name: Option<String>,
    /// Override profile for this instance.
    pub profile: Option<ProfileSection>,
    /// Override system prompt for this instance.
    pub system_prompt: Option<String>,
    /// Override provider reference for this instance.
    pub provider: Option<String>,
}

// ── Provider section ────────────────────────────────────────────────────────

/// Provider configuration — model identity, API credentials, and protocol.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ProviderSection {
    /// Model identifier sent to the API (e.g. "claude-sonnet-4-20250514", "gpt-4o").
    pub model: Option<String>,
    /// API key or credential. Supports `${ENV_VAR}` substitution.
    pub api_key: Option<String>,
    /// API protocol: "anthropic_messages", "openai_completions", "openai_responses",
    /// "azure_openai_responses", "google_generative_ai", "google_vertex",
    /// "bedrock_converse_stream".
    pub api: Option<String>,
    /// Base URL for API requests (without trailing slash).
    pub base_url: Option<String>,
    /// Provider name (e.g. "anthropic", "openai", "xai").
    pub provider: Option<String>,
    /// Human-friendly model name.
    pub name: Option<String>,
    /// Whether this model supports reasoning/thinking.
    pub reasoning: Option<bool>,
    /// Context window size in tokens.
    pub context_window: Option<u32>,
    /// Default max output tokens.
    pub max_tokens: Option<u32>,
    /// Cost configuration.
    pub cost: CostSection,
    /// Additional headers to send with requests.
    pub headers: HashMap<String, String>,
    /// OpenAI-compat quirk flags.
    pub compat: CompatSection,
    /// Named provider instances (for multi-provider configs).
    pub instances: Vec<ProviderInstance>,
}

/// Cost rates for token usage tracking.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct CostSection {
    pub input_per_million: Option<f64>,
    pub output_per_million: Option<f64>,
    pub cache_read_per_million: Option<f64>,
    pub cache_write_per_million: Option<f64>,
}

/// OpenAI-compat quirk flags.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct CompatSection {
    pub auth_style: Option<String>,
    pub reasoning_format: Option<String>,
    pub max_tokens_field: Option<String>,
    pub supports_streaming: Option<bool>,
    pub supports_system_message: Option<bool>,
}

/// A named provider instance with overrides.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ProviderInstance {
    pub name: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub provider: Option<String>,
}

// ── Session section ─────────────────────────────────────────────────────────

/// Session-level overrides.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct SessionSection {
    /// Session scope: "ephemeral" or "persistent".
    pub scope: Option<String>,
    /// Session-level thinking level override.
    pub thinking_level: Option<String>,
    /// Session-level temperature override.
    pub temperature: Option<f32>,
}

// ── Tools section ───────────────────────────────────────────────────────────

/// Tool configuration.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ToolsSection {
    /// List of enabled tool names (resolved by the caller's tool registry).
    pub enabled: Vec<String>,
    /// Tool execution strategy: "sequential", "parallel", "batched".
    pub tool_strategy: Option<String>,
    /// Batch size for "batched" strategy.
    pub batch_size: Option<usize>,
    /// Named tool instances with overrides.
    pub instances: Vec<ToolInstance>,
}

/// A named tool instance with configuration overrides.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ToolInstance {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub config: HashMap<String, serde_json::Value>,
}

// ── Skills section ──────────────────────────────────────────────────────────

/// Skills configuration.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct SkillsSection {
    /// Skill directory paths to load SKILL.md files from.
    pub paths: Vec<String>,
}

// ── Sub-agents section ──────────────────────────────────────────────────────

/// Sub-agent configuration.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct SubAgentsSection {
    /// Named sub-agent definitions.
    pub instances: Vec<SubAgentInstance>,
}

/// A sub-agent instance.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct SubAgentInstance {
    pub name: Option<String>,
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<usize>,
    pub tools: Vec<String>,
}

// ── Callbacks & Hooks (Phase 2 WASM) ────────────────────────────────────────

/// Callback references — Phase 2 WASM plugin loading.
/// In Phase 1, these are stored as strings but not acted upon.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct CallbacksSection {
    pub before_loop: Option<String>,
    pub after_loop: Option<String>,
    pub before_turn: Option<String>,
    pub after_turn: Option<String>,
    pub before_tool_execution: Option<String>,
    pub after_tool_execution: Option<String>,
    pub before_compaction_start: Option<String>,
    pub after_compaction_end: Option<String>,
}

/// Hook references — Phase 2 WASM plugin loading.
/// In Phase 1, these are stored as strings but not acted upon.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct HooksSection {
    pub convert_to_llm: Option<String>,
    pub transform_context: Option<String>,
}

// ── Compaction section (G5 — unified config) ────────────────────────────────

/// Compaction configuration — unifies context management settings (G5).
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct CompactionSection {
    /// Maximum context tokens (the model's context window).
    pub max_context_tokens: Option<usize>,
    /// Tokens reserved for the system prompt.
    pub system_prompt_tokens: Option<usize>,
    /// Fraction of max_context_tokens below which headroom is measured.
    pub compact_at_pct: Option<f64>,
    /// Minimum remaining headroom fraction before compaction fires.
    pub compact_budget_threshold_pct: Option<f64>,
    /// Turns to keep verbatim from the start.
    pub keep_first_turns: Option<usize>,
    /// Minimum turns to keep from the end.
    pub keep_recent_turns: Option<usize>,
    /// Token budget for the summarised middle section.
    pub max_summary_tokens: Option<usize>,
    /// Max lines per tool output in the keep_recent section.
    pub tool_output_max_lines: Option<usize>,
}

// ── Execution section ───────────────────────────────────────────────────────

/// Execution limits and related configuration.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ExecutionSection {
    /// Maximum number of LLM turns.
    pub max_turns: Option<usize>,
    /// Maximum total tokens consumed across all turns.
    pub max_total_tokens: Option<usize>,
    /// Maximum wall-clock duration in seconds.
    pub max_duration_secs: Option<u64>,
    /// Maximum cumulative dollar cost.
    pub max_cost: Option<f64>,
    /// Retry configuration.
    pub retry: RetrySection,
    /// Cache configuration.
    pub cache: CacheSection,
}

/// Retry configuration for transient provider errors.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct RetrySection {
    /// Maximum retry attempts (0 = no retries).
    pub max_retries: Option<usize>,
    /// Initial delay before first retry in milliseconds.
    pub initial_delay_ms: Option<u64>,
    /// Backoff multiplier applied each attempt.
    pub backoff_multiplier: Option<f64>,
    /// Maximum delay cap in milliseconds.
    pub max_delay_ms: Option<u64>,
}

/// Cache configuration.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct CacheSection {
    /// Master switch — false disables all caching hints.
    pub enabled: Option<bool>,
    /// Cache strategy: "auto", "disabled", or a manual config.
    pub strategy: Option<String>,
}
