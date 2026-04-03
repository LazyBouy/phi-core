//! Config → Agent construction.
//!
//! [`agent_from_config`] is the entry point: it takes a parsed [`AgentConfig`] and
//! returns an `Arc<dyn Agent>` (a [`BasicAgent`] internally, wrapped in Arc).

use super::reference::{parse_config_ref, ConfigRef};
use super::schema::AgentConfig;
use crate::agent_loop::script_callback::{is_script_path, ScriptCallback};
use crate::agents::system_prompt::{CustomPromptStrategy, PromptBlockDef, SystemPrompt};
use crate::agents::{Agent, AgentProfile, BasicAgent};
use crate::context::{CompactionConfig, CompactionScope, ContextConfig, ExecutionLimits};
use crate::provider::ModelConfig;
use crate::tools::ToolRegistry;
use crate::types::{AgentTool, CacheConfig, CacheStrategy, ThinkingLevel, ToolExecutionStrategy};
use std::collections::HashMap;
use std::path::PathBuf;
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
///   Use [`agent_from_config_with_registry`] to resolve tools automatically (G10).
/// - **Callbacks/hooks are Phase 2.** Config stores callback/hook strings but the
///   builder ignores them in Phase 1. WASM plugin loading will activate them in Phase 2.
pub fn agent_from_config(config: &AgentConfig) -> Result<Arc<dyn Agent>, ConfigError> {
    let agent = build_basic_agent(config, None, None, None)?;
    Ok(Arc::new(agent))
}

/// Construct an agent from config, resolving tool names via a [`ToolRegistry`] (G10).
///
/// Tools listed in `config.tools.enabled` are resolved through the registry.
/// Unknown tool names are silently skipped. The rest of the construction pipeline
/// is identical to [`agent_from_config`].
pub fn agent_from_config_with_registry(
    config: &AgentConfig,
    registry: &ToolRegistry,
) -> Result<Arc<dyn Agent>, ConfigError> {
    let tools = registry.resolve(&config.tools.enabled);
    let agent = build_basic_agent(config, None, None, Some(tools))?;
    Ok(Arc::new(agent))
}

/// Construct multiple agents from a config with agent instances.
///
/// If `config.agent.instances` is empty, returns a single agent from
/// [`agent_from_config`] with the name `"default"`.
///
/// Otherwise, builds one agent per instance, resolving `agent_profile` refs
/// against `config.agent.profile.instances`. Each instance can override:
/// - `agent_profile` — reference to a named profile instance
/// - `system_prompt` — override the system prompt
/// - `profile` — inline profile overrides
#[allow(clippy::type_complexity)]
pub fn agents_from_config(
    config: &AgentConfig,
) -> Result<Vec<(String, Arc<dyn Agent>)>, ConfigError> {
    if config.agent.instances.is_empty() {
        let agent = agent_from_config(config)?;
        return Ok(vec![("default".to_string(), agent)]);
    }

    let mut agents = Vec::new();
    for instance in &config.agent.instances {
        let name = instance
            .name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());

        // Resolve profile: agent_profile ref -> find instance -> merge with base
        let profile_override = if let Some(ref profile_ref) = instance.agent_profile {
            let parsed = super::reference::parse_config_ref(profile_ref);
            let ref_name = parsed.effective_name();
            if let Some(inst) = find_profile_instance(config, ref_name) {
                Some(resolve_profile_instance(&config.agent.profile, inst)?)
            } else {
                None
            }
        } else {
            None
        };

        // Resolve provider instance from ref (if set)
        let provider_inst = if let Some(ref provider_ref) = instance.provider {
            let parsed = super::reference::parse_config_ref(provider_ref);
            let ref_name = parsed.effective_name();
            config.provider.instances.iter().find(|pi| {
                let id_name = pi
                    .id
                    .as_deref()
                    .map(|id| {
                        super::reference::parse_config_ref(id)
                            .effective_name()
                            .to_string()
                    })
                    .unwrap_or_default();
                let plain_name = pi.name.as_deref().unwrap_or("");
                id_name == ref_name || plain_name == ref_name
            })
        } else {
            None
        };

        // System prompt override from instance
        let system_prompt_override = instance.system_prompt.clone();

        let agent = build_basic_agent(config, profile_override.as_ref(), provider_inst, None)?;

        // Apply instance-level system prompt override after construction
        let agent: Arc<dyn Agent> = if let Some(ref prompt) = system_prompt_override {
            let mut a = build_basic_agent(config, profile_override.as_ref(), provider_inst, None)?;
            a = a.with_system_prompt(prompt.clone());
            Arc::new(a)
        } else {
            Arc::new(agent)
        };

        agents.push((name, agent));
    }
    Ok(agents)
}

/// Internal: build a `BasicAgent` from config with optional overrides.
///
/// - `profile_override`: when `Some`, replaces the profile built from `config.agent.profile`.
/// - `provider_instance`: when `Some`, overrides model config fields from a provider instance.
/// - `tools_override`: when `Some`, sets these tools on the agent.
fn build_basic_agent(
    config: &AgentConfig,
    profile_override: Option<&AgentProfile>,
    provider_instance: Option<&super::schema::ProviderInstance>,
    tools_override: Option<Vec<Arc<dyn AgentTool>>>,
) -> Result<BasicAgent, ConfigError> {
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

    let mut model_config = ModelConfig {
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
        compat: build_compat_config(&config.provider.compat),
    };

    // Apply provider instance overrides (from agent instance → provider ref)
    if let Some(pi) = provider_instance {
        if let Some(ref m) = pi.model {
            model_config.id = m.clone();
            model_config.name = m.clone();
        }
        if let Some(ref k) = pi.api_key {
            model_config.api_key = k.clone();
        }
        if let Some(ref a) = pi.api {
            model_config.api = parse_api_protocol(a)?;
            // Re-derive base_url if the instance doesn't set one
            if pi.base_url.is_none() {
                model_config.base_url = default_base_url(model_config.api);
            }
        }
        if let Some(ref u) = pi.base_url {
            model_config.base_url = u.clone();
        }
        if let Some(ref p) = pi.provider {
            model_config.provider = p.clone();
        }
    }

    // ── Build AgentProfile ───────────────────────────────────────────────
    let profile = match profile_override {
        Some(p) => p.clone(),
        None => build_profile(&config.agent.profile)?,
    };

    // ── Build the agent ──────────────────────────────────────────────────
    let mut agent = BasicAgent::new(model_config);

    // System prompt: agent-level overrides profile-level.
    // If the value is a {{...}} reference, resolve through the 3-entity chain:
    //   SystemPromptStrategy (template) → SystemPrompt (content) → compose()
    let raw_prompt = config
        .agent
        .system_prompt
        .as_deref()
        .or(config.agent.profile.system_prompt.as_deref())
        .unwrap_or("");
    let workspace_path = config
        .agent
        .workspace
        .as_deref()
        .or(config.default_workspace.as_deref())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let system_prompt = resolve_system_prompt(raw_prompt, config, &workspace_path)?;
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
    // Resolve compaction instance from profile ref (if set)
    let compaction_section = resolve_compaction_from_profile(config);
    if compaction_section.max_context_tokens.is_some() {
        let ctx_config = build_context_config(&compaction_section);
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

    // ── Tools (G10) ──────────────────────────────────────────────────────
    if let Some(tools) = tools_override {
        agent = agent.with_tools(tools);
    }

    // ── Workspace ────────────────────────────────────────────────────────
    // Resolution: agent-level workspace > default_workspace > None
    let workspace = config
        .agent
        .workspace
        .as_deref()
        .or(config.default_workspace.as_deref());
    if let Some(ws) = workspace {
        agent = agent.with_workspace(ws);
    }

    // ── Script-based callbacks ───────────────────────────────────────────
    // When config callback fields contain script paths (*.sh, *.py, or contain '/'),
    // wrap them as ScriptCallback closures. Non-script strings (e.g., "module::func")
    // are Phase 2 WASM references and are ignored.
    let cb_workspace = workspace.map(PathBuf::from);
    wire_script_callbacks(&mut agent, &config.callbacks, cb_workspace);

    Ok(agent)
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
        workspace: None,
    })
}

/// Build a profile by resolving a `{{...}}` reference against the config's
/// profile instances, then merging: profile defaults ← instance overrides.
fn resolve_profile_instance(
    base: &super::schema::ProfileSection,
    instance: &super::schema::ProfileInstanceSection,
) -> Result<AgentProfile, ConfigError> {
    // Instance fields override base profile defaults (Option::or pattern)
    let thinking_str = instance
        .thinking_level
        .as_deref()
        .or(base.thinking_level.as_deref());
    let thinking_level = thinking_str.map(parse_thinking_level).transpose()?;

    Ok(AgentProfile {
        profile_id: base
            .profile_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name: instance.name.clone().or_else(|| base.name.clone()),
        description: instance
            .description
            .clone()
            .or_else(|| base.description.clone()),
        system_prompt: instance
            .system_prompt
            .clone()
            .or_else(|| base.system_prompt.clone()),
        thinking_level,
        temperature: instance.temperature.or(base.temperature),
        max_tokens: instance.max_tokens.or(base.max_tokens),
        config_id: instance
            .config_id
            .clone()
            .or_else(|| base.config_id.clone()),
        skills: if instance.skills.is_empty() {
            base.skills.clone()
        } else {
            instance.skills.clone()
        },
        workspace: None,
    })
}

/// Look up a profile instance by reference name within the config.
///
/// Searches `[[agent.profile.instances]]` for an instance whose `id` field
/// matches the given name (after stripping `{{...}}` syntax from both sides).
fn find_profile_instance<'a>(
    config: &'a AgentConfig,
    ref_name: &str,
) -> Option<&'a super::schema::ProfileInstanceSection> {
    config.agent.profile.instances.iter().find(|inst| {
        let inst_ref = super::reference::parse_config_ref(&inst.id);
        inst_ref.effective_name() == ref_name
    })
}

fn resolve_compaction_from_profile(config: &AgentConfig) -> super::schema::CompactionSection {
    if let Some(ref comp_ref) = config.agent.profile.compaction {
        let parsed = super::reference::parse_config_ref(comp_ref);
        let ref_name = parsed.effective_name();
        if let Some(inst) = config
            .compaction
            .instances
            .iter()
            .find(|i| super::reference::parse_config_ref(&i.id).effective_name() == ref_name)
        {
            return merge_compaction_instance(&config.compaction, inst);
        }
    }
    config.compaction.clone()
}

fn merge_compaction_instance(
    base: &super::schema::CompactionSection,
    inst: &super::schema::CompactionInstanceSection,
) -> super::schema::CompactionSection {
    super::schema::CompactionSection {
        max_context_tokens: inst.max_context_tokens.or(base.max_context_tokens),
        system_prompt_tokens: inst.system_prompt_tokens.or(base.system_prompt_tokens),
        compact_at_pct: inst.compact_at_pct.or(base.compact_at_pct),
        compact_budget_threshold_pct: inst
            .compact_budget_threshold_pct
            .or(base.compact_budget_threshold_pct),
        keep_first_turns: inst.keep_first_turns.or(base.keep_first_turns),
        keep_recent_turns: inst.keep_recent_turns.or(base.keep_recent_turns),
        max_summary_tokens: inst.max_summary_tokens.or(base.max_summary_tokens),
        tool_output_max_lines: inst.tool_output_max_lines.or(base.tool_output_max_lines),
        focus_message: inst
            .focus_message
            .clone()
            .or_else(|| base.focus_message.clone()),
        instances: base.instances.clone(),
    }
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
            focus_message: section.focus_message.clone(),
            in_memory_strategy: None,
            block_strategy: None,
        },
        token_counter: None,
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

fn build_compat_config(
    section: &super::schema::CompatSection,
) -> Option<crate::provider::model::OpenAiCompat> {
    use crate::provider::model::{MaxTokensField, OpenAiCompat, ThinkingFormat};

    // Return None if all fields are empty (non-OpenAI provider)
    if section.auth_style.is_none()
        && section.reasoning_format.is_none()
        && section.max_tokens_field.is_none()
        && section.supports_streaming.is_none()
        && section.supports_system_message.is_none()
    {
        return None;
    }

    let mut compat = OpenAiCompat::default();

    if let Some(ref fmt) = section.reasoning_format {
        compat.thinking_format = match fmt.to_lowercase().as_str() {
            "xai" => ThinkingFormat::Xai,
            "qwen" => ThinkingFormat::Qwen,
            "openrouter" => ThinkingFormat::OpenRouter,
            _ => ThinkingFormat::OpenAi,
        };
    }

    if let Some(ref field) = section.max_tokens_field {
        compat.max_tokens_field = match field.to_lowercase().as_str() {
            "max_completion_tokens" => MaxTokensField::MaxCompletionTokens,
            _ => MaxTokensField::MaxTokens,
        };
    }

    if let Some(streaming) = section.supports_streaming {
        compat.supports_usage_in_streaming = streaming;
    }

    if let Some(developer) = section.supports_system_message {
        compat.supports_developer_role = developer;
    }

    Some(compat)
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

// ── System prompt resolution ────────────────────────────────────────────

/// Resolve system prompt: if raw text is a `{{...}}` reference, resolve through
/// the 3-entity chain (SystemPromptStrategy → SystemPrompt → compose()).
/// If it's a literal string, return as-is.
fn resolve_system_prompt(
    raw: &str,
    config: &AgentConfig,
    workspace: &std::path::Path,
) -> Result<String, ConfigError> {
    if raw.is_empty() {
        return Ok(String::new());
    }

    let config_ref = parse_config_ref(raw);
    match config_ref {
        ConfigRef::Literal(_) => Ok(raw.to_string()),
        ref r if r.is_reference() => {
            let prompt_name = r.effective_name();

            // Find the SystemPrompt instance
            let prompt_inst = config
                .system_prompt
                .instances
                .iter()
                .find(|p| parse_config_ref(&p.id).effective_name() == prompt_name)
                .ok_or_else(|| ConfigError::InvalidField {
                    field: "agent.system_prompt".into(),
                    value: raw.into(),
                    expected: format!(
                        "a system_prompt instance named '{prompt_name}' in [[system_prompt.instances]]"
                    ),
                })?;

            // Find the referenced strategy
            let strategy_ref_raw = prompt_inst.strategy_type.as_deref().unwrap_or("");
            let strategy_name = parse_config_ref(strategy_ref_raw)
                .effective_name()
                .to_string();

            let strategy_inst = config
                .system_prompt_strategy
                .instances
                .iter()
                .find(|s| parse_config_ref(&s.id).effective_name() == strategy_name)
                .ok_or_else(|| ConfigError::InvalidField {
                    field: "system_prompt.type".into(),
                    value: strategy_ref_raw.into(),
                    expected: format!(
                        "a strategy named '{strategy_name}' in [[system_prompt_strategy.instances]]"
                    ),
                })?;

            // Build the strategy
            let block_defs: Vec<PromptBlockDef> = strategy_inst
                .blocks
                .iter()
                .map(|b| PromptBlockDef {
                    name: b.name.clone(),
                    order: b.order.unwrap_or(0),
                    max_length: b.max_length.unwrap_or(usize::MAX),
                })
                .collect();
            let strategy = CustomPromptStrategy { blocks: block_defs };

            // Build the SystemPrompt with block content
            let mut blocks = HashMap::new();
            for (key, value) in &prompt_inst.blocks {
                // Skip known metadata fields captured by serde(flatten)
                if key == "id" || key == "description" || key == "type" {
                    continue;
                }
                if let Some(text) = value.as_str() {
                    blocks.insert(key.clone(), text.to_string());
                }
            }

            let prompt = SystemPrompt {
                id: prompt_inst.id.clone(),
                description: prompt_inst.description.clone(),
                strategy_ref: strategy_ref_raw.to_string(),
                blocks,
            };

            prompt
                .compose(&strategy, workspace)
                .map_err(ConfigError::Io)
        }
        _ => Ok(raw.to_string()),
    }
}

// ── Script callback wiring ──────────────────────────────────────────────

/// Wire script-based callbacks from config into the agent via trait setters.
/// Script paths (*.sh, *.py, or containing '/') are wrapped as ScriptCallback closures.
/// Non-script strings are Phase 2 WASM references and are ignored.
fn wire_script_callbacks(
    agent: &mut dyn Agent,
    callbacks: &super::schema::CallbacksSection,
    workspace: Option<PathBuf>,
) {
    if let Some(ref path) = callbacks.before_loop {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace.clone());
            agent.set_before_loop(Some(Arc::new(move |msgs, n| {
                let input = serde_json::json!({
                    "hook": "before_loop",
                    "message_count": msgs.len(),
                    "loop_index": n,
                });
                script
                    .execute_sync(&input)
                    .ok()
                    .and_then(|v| v.get("allow").and_then(|a| a.as_bool()))
                    .unwrap_or(true)
            })));
        }
    }

    if let Some(ref path) = callbacks.after_loop {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace.clone());
            agent.set_after_loop(Some(Arc::new(move |_msgs, _usage| {
                let input = serde_json::json!({"hook": "after_loop"});
                let _ = script.execute_sync(&input);
            })));
        }
    }

    if let Some(ref path) = callbacks.before_turn {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace.clone());
            agent.set_before_turn(Some(Arc::new(move |msgs, turn| {
                let input = serde_json::json!({
                    "hook": "before_turn",
                    "message_count": msgs.len(),
                    "turn_index": turn,
                });
                script
                    .execute_sync(&input)
                    .ok()
                    .and_then(|v| v.get("allow").and_then(|a| a.as_bool()))
                    .unwrap_or(true)
            })));
        }
    }

    if let Some(ref path) = callbacks.after_turn {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace.clone());
            agent.set_after_turn(Some(Arc::new(move |_msgs, _usage| {
                let input = serde_json::json!({"hook": "after_turn"});
                let _ = script.execute_sync(&input);
            })));
        }
    }

    if let Some(ref path) = callbacks.before_tool_execution {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace.clone());
            agent.set_before_tool_execution(Some(Arc::new(move |name, id, _args| {
                let input = serde_json::json!({
                    "hook": "before_tool_execution",
                    "tool_name": name,
                    "tool_call_id": id,
                });
                script
                    .execute_sync(&input)
                    .ok()
                    .and_then(|v| v.get("allow").and_then(|a| a.as_bool()))
                    .unwrap_or(true)
            })));
        }
    }

    if let Some(ref path) = callbacks.after_tool_execution {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace.clone());
            agent.set_after_tool_execution(Some(Arc::new(move |name, id, is_error| {
                let input = serde_json::json!({
                    "hook": "after_tool_execution",
                    "tool_name": name,
                    "tool_call_id": id,
                    "is_error": is_error,
                });
                let _ = script.execute_sync(&input);
            })));
        }
    }

    if let Some(ref path) = callbacks.before_compaction_start {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace.clone());
            agent.set_before_compaction_start(Some(Arc::new(move |tokens, count| {
                let input = serde_json::json!({
                    "hook": "before_compaction_start",
                    "estimated_tokens": tokens,
                    "message_count": count,
                });
                script
                    .execute_sync(&input)
                    .ok()
                    .and_then(|v| v.get("allow").and_then(|a| a.as_bool()))
                    .unwrap_or(true)
            })));
        }
    }

    if let Some(ref path) = callbacks.after_compaction_end {
        if is_script_path(path) {
            let script = ScriptCallback::new(path, workspace);
            agent.set_after_compaction_end(Some(Arc::new(
                move |before, after, tok_before, tok_after| {
                    let input = serde_json::json!({
                        "hook": "after_compaction_end",
                        "messages_before": before,
                        "messages_after": after,
                        "tokens_before": tok_before,
                        "tokens_after": tok_after,
                    });
                    let _ = script.execute_sync(&input);
                },
            )));
        }
    }
}
