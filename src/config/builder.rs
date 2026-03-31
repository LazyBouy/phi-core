//! Config → Agent construction.
//!
//! [`agent_from_config`] is the entry point: it takes a parsed [`AgentConfig`] and
//! returns an `Arc<dyn Agent>` (a [`BasicAgent`] internally, wrapped in Arc).

use super::schema::AgentConfig;
use crate::agents::{Agent, AgentProfile, BasicAgent};
use crate::context::{CompactionConfig, CompactionScope, ContextConfig, ExecutionLimits};
use crate::provider::ModelConfig;
use crate::types::{CacheConfig, CacheStrategy, ThinkingLevel, ToolExecutionStrategy};
use std::sync::Arc;

/// Errors from config parsing and agent construction.
#[derive(Debug)]
pub enum ConfigError {
    /// Config string could not be parsed.
    Parse(String),
    /// An environment variable referenced via `${VAR}` is not set.
    MissingEnvVar { var: String },
    /// A config field has an invalid value.
    InvalidField {
        field: String,
        value: String,
        expected: String,
    },
    /// I/O error reading a config file.
    Io(std::io::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(msg) => write!(f, "Config parse error: {msg}"),
            Self::MissingEnvVar { var } => write!(f, "Missing environment variable: ${{{var}}}"),
            Self::InvalidField {
                field,
                value,
                expected,
            } => write!(
                f,
                "Invalid value for {field}: \"{value}\" (expected {expected})"
            ),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Construct an agent from a parsed config.
///
/// Returns `Arc<dyn Agent>` — internally builds a [`BasicAgent`] (the reference
/// implementation), wraps it in `Arc` for shared ownership across async tasks.
///
/// # Notes
///
/// - **Tools are not instantiated.** Config specifies tool names via `tools.enabled`;
///   the caller must register tool instances via `.set_tools()` on the returned agent.
///   Full tool-from-config requires a tool registry (tracked as G10).
/// - **Callbacks/hooks are Phase 2.** Config stores callback/hook strings but the
///   builder ignores them in Phase 1. WASM plugin loading will activate them in Phase 2.
pub fn agent_from_config(config: &AgentConfig) -> Result<Arc<dyn Agent>, ConfigError> {
    // ── Build ModelConfig ────────────────────────────────────────────────
    let model = config
        .provider
        .model
        .as_deref()
        .unwrap_or("unknown")
        .to_string();
    let api_key = config.provider.api_key.as_deref().unwrap_or("").to_string();
    let provider_name = config
        .provider
        .provider
        .as_deref()
        .unwrap_or("anthropic")
        .to_string();
    let base_url = config
        .provider
        .base_url
        .as_deref()
        .unwrap_or("")
        .to_string();
    let display_name = config
        .provider
        .name
        .as_deref()
        .unwrap_or(&model)
        .to_string();

    let api_protocol = parse_api_protocol(
        config
            .provider
            .api
            .as_deref()
            .unwrap_or("anthropic_messages"),
    )?;

    let model_config = ModelConfig {
        id: model,
        name: display_name,
        api: api_protocol,
        provider: provider_name,
        base_url: if base_url.is_empty() {
            default_base_url(api_protocol)
        } else {
            base_url
        },
        api_key,
        reasoning: config.provider.reasoning.unwrap_or(false),
        context_window: config.provider.context_window.unwrap_or(200_000),
        max_tokens: config.provider.max_tokens.unwrap_or(8_192),
        cost: build_cost_config(&config.provider.cost),
        headers: config.provider.headers.clone(),
        compat: None, // TODO: build from config.provider.compat when needed
    };

    // ── Build AgentProfile ───────────────────────────────────────────────
    let profile = build_profile(&config.agent.profile)?;

    // ── Build the agent ──────────────────────────────────────────────────
    let mut agent = BasicAgent::new(model_config);

    // System prompt: agent-level overrides profile-level
    let system_prompt = config
        .agent
        .system_prompt
        .as_deref()
        .or(config.agent.profile.system_prompt.as_deref())
        .unwrap_or("");
    if !system_prompt.is_empty() {
        agent = agent.with_system_prompt(system_prompt);
    }

    // Apply profile
    agent = agent.with_profile(profile);

    // Thinking level — use profile value (already set via with_profile), but
    // agent-level config can further override
    if let Some(ref level_str) = config.agent.profile.thinking_level {
        let level = parse_thinking_level(level_str)?;
        agent = agent.with_thinking(level);
    }

    // Temperature
    if let Some(temp) = config.agent.profile.temperature {
        agent = agent.with_temperature(temp);
    }

    // Max tokens
    if let Some(max) = config.agent.profile.max_tokens {
        agent = agent.with_max_tokens(max);
    }

    // Config ID
    if let Some(ref id) = config.agent.profile.config_id {
        agent = agent.with_config_id(id.clone());
    }

    // ── Context / Compaction (G5) ────────────────────────────────────────
    if config.compaction.max_context_tokens.is_some() {
        let ctx_config = build_context_config(&config.compaction);
        agent = agent.with_context_config(ctx_config);
    }

    // ── Execution limits ─────────────────────────────────────────────────
    if has_execution_config(&config.execution) {
        let limits = build_execution_limits(&config.execution);
        agent = agent.with_execution_limits(limits);
    }

    // ── Retry config ─────────────────────────────────────────────────────
    if has_retry_config(&config.execution.retry) {
        let retry = build_retry_config(&config.execution.retry);
        agent = agent.with_retry_config(retry);
    }

    // ── Cache config ─────────────────────────────────────────────────────
    if config.execution.cache.enabled.is_some() || config.execution.cache.strategy.is_some() {
        let cache = build_cache_config(&config.execution.cache);
        agent = agent.with_cache_config(cache);
    }

    // ── Tool execution strategy ──────────────────────────────────────────
    if let Some(ref strategy_str) = config.tools.tool_strategy {
        let strategy = parse_tool_execution_strategy(strategy_str, config.tools.batch_size)?;
        agent = agent.with_tool_execution(strategy);
    }

    Ok(Arc::new(agent))
}

// ── Helper functions ────────────────────────────────────────────────────────

fn build_profile(section: &super::schema::ProfileSection) -> Result<AgentProfile, ConfigError> {
    let thinking_level = section
        .thinking_level
        .as_deref()
        .map(parse_thinking_level)
        .transpose()?;

    Ok(AgentProfile {
        profile_id: section
            .profile_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name: section.name.clone(),
        description: section.description.clone(),
        system_prompt: section.system_prompt.clone(),
        thinking_level,
        temperature: section.temperature,
        max_tokens: section.max_tokens,
        config_id: section.config_id.clone(),
        skills: section.skills.clone(),
    })
}

fn parse_thinking_level(s: &str) -> Result<ThinkingLevel, ConfigError> {
    match s.to_lowercase().as_str() {
        "off" => Ok(ThinkingLevel::Off),
        "minimal" => Ok(ThinkingLevel::Minimal),
        "low" => Ok(ThinkingLevel::Low),
        "medium" => Ok(ThinkingLevel::Medium),
        "high" => Ok(ThinkingLevel::High),
        _ => Err(ConfigError::InvalidField {
            field: "thinking_level".to_string(),
            value: s.to_string(),
            expected: "off, minimal, low, medium, high".to_string(),
        }),
    }
}

fn parse_api_protocol(s: &str) -> Result<crate::provider::model::ApiProtocol, ConfigError> {
    use crate::provider::model::ApiProtocol;
    match s.to_lowercase().replace('-', "_").as_str() {
        "anthropic_messages" | "anthropic" => Ok(ApiProtocol::AnthropicMessages),
        "openai_completions" | "openai" => Ok(ApiProtocol::OpenAiCompletions),
        "openai_responses" => Ok(ApiProtocol::OpenAiResponses),
        "azure_openai_responses" | "azure" => Ok(ApiProtocol::AzureOpenAiResponses),
        "google_generative_ai" | "google" | "gemini" => Ok(ApiProtocol::GoogleGenerativeAi),
        "google_vertex" | "vertex" => Ok(ApiProtocol::GoogleVertex),
        "bedrock_converse_stream" | "bedrock" => Ok(ApiProtocol::BedrockConverseStream),
        _ => Err(ConfigError::InvalidField {
            field: "provider.api".to_string(),
            value: s.to_string(),
            expected: "anthropic_messages, openai_completions, openai_responses, \
                       azure_openai_responses, google_generative_ai, google_vertex, \
                       bedrock_converse_stream"
                .to_string(),
        }),
    }
}

fn default_base_url(api: crate::provider::model::ApiProtocol) -> String {
    use crate::provider::model::ApiProtocol;
    match api {
        ApiProtocol::AnthropicMessages => "https://api.anthropic.com".to_string(),
        ApiProtocol::OpenAiCompletions | ApiProtocol::OpenAiResponses => {
            "https://api.openai.com".to_string()
        }
        ApiProtocol::GoogleGenerativeAi => "https://generativelanguage.googleapis.com".to_string(),
        _ => String::new(),
    }
}

fn build_cost_config(section: &super::schema::CostSection) -> crate::provider::model::CostConfig {
    crate::provider::model::CostConfig {
        input_per_million: section.input_per_million.unwrap_or(0.0),
        output_per_million: section.output_per_million.unwrap_or(0.0),
        cache_read_per_million: section.cache_read_per_million.unwrap_or(0.0),
        cache_write_per_million: section.cache_write_per_million.unwrap_or(0.0),
    }
}

fn build_context_config(section: &super::schema::CompactionSection) -> ContextConfig {
    let defaults = ContextConfig::default();
    let comp_defaults = CompactionConfig::default();

    ContextConfig {
        max_context_tokens: section
            .max_context_tokens
            .unwrap_or(defaults.max_context_tokens),
        system_prompt_tokens: section
            .system_prompt_tokens
            .unwrap_or(defaults.system_prompt_tokens),
        compaction: CompactionConfig {
            compact_at_pct: section
                .compact_at_pct
                .unwrap_or(comp_defaults.compact_at_pct),
            compact_budget_threshold_pct: section
                .compact_budget_threshold_pct
                .unwrap_or(comp_defaults.compact_budget_threshold_pct),
            compaction_scope: CompactionScope::default(),
            keep_first_turns: section
                .keep_first_turns
                .unwrap_or(comp_defaults.keep_first_turns),
            keep_recent_turns: section
                .keep_recent_turns
                .unwrap_or(comp_defaults.keep_recent_turns),
            max_summary_tokens: section
                .max_summary_tokens
                .unwrap_or(comp_defaults.max_summary_tokens),
            tool_output_max_lines: section
                .tool_output_max_lines
                .unwrap_or(comp_defaults.tool_output_max_lines),
        },
        keep_recent: defaults.keep_recent,
        keep_first: defaults.keep_first,
        tool_output_max_lines: defaults.tool_output_max_lines,
    }
}

fn has_execution_config(section: &super::schema::ExecutionSection) -> bool {
    section.max_turns.is_some()
        || section.max_total_tokens.is_some()
        || section.max_duration_secs.is_some()
        || section.max_cost.is_some()
}

fn build_execution_limits(section: &super::schema::ExecutionSection) -> ExecutionLimits {
    let defaults = ExecutionLimits::default();
    ExecutionLimits {
        max_turns: section.max_turns.unwrap_or(defaults.max_turns),
        max_total_tokens: section
            .max_total_tokens
            .unwrap_or(defaults.max_total_tokens),
        max_duration: std::time::Duration::from_secs(
            section
                .max_duration_secs
                .unwrap_or(defaults.max_duration.as_secs()),
        ),
        max_cost: section.max_cost.or(defaults.max_cost),
    }
}

fn has_retry_config(section: &super::schema::RetrySection) -> bool {
    section.max_retries.is_some()
        || section.initial_delay_ms.is_some()
        || section.backoff_multiplier.is_some()
        || section.max_delay_ms.is_some()
}

fn build_retry_config(
    section: &super::schema::RetrySection,
) -> crate::provider::retry::RetryConfig {
    let defaults = crate::provider::retry::RetryConfig::default();
    crate::provider::retry::RetryConfig {
        max_retries: section.max_retries.unwrap_or(defaults.max_retries),
        initial_delay_ms: section
            .initial_delay_ms
            .unwrap_or(defaults.initial_delay_ms),
        backoff_multiplier: section
            .backoff_multiplier
            .unwrap_or(defaults.backoff_multiplier),
        max_delay_ms: section.max_delay_ms.unwrap_or(defaults.max_delay_ms),
    }
}

fn build_cache_config(section: &super::schema::CacheSection) -> CacheConfig {
    let enabled = section.enabled.unwrap_or(true);
    let strategy = match section.strategy.as_deref() {
        Some("disabled") => CacheStrategy::Disabled,
        Some("auto") | None => CacheStrategy::Auto,
        _ => CacheStrategy::Auto, // unknown strategies default to auto
    };
    CacheConfig { enabled, strategy }
}

fn parse_tool_execution_strategy(
    s: &str,
    batch_size: Option<usize>,
) -> Result<ToolExecutionStrategy, ConfigError> {
    match s.to_lowercase().as_str() {
        "sequential" => Ok(ToolExecutionStrategy::Sequential),
        "parallel" => Ok(ToolExecutionStrategy::Parallel),
        "batched" => Ok(ToolExecutionStrategy::Batched {
            size: batch_size.unwrap_or(3),
        }),
        _ => Err(ConfigError::InvalidField {
            field: "tools.tool_strategy".to_string(),
            value: s.to_string(),
            expected: "sequential, parallel, batched".to_string(),
        }),
    }
}
