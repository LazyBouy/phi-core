//! Tests for the config module: parsing, env var substitution, and agent construction.

use phi_core::config::{agent_from_config, parse_config, ConfigError, ConfigFormat};
#[allow(unused_imports)]
use phi_core::Agent; // for trait methods

// ---------------------------------------------------------------------------
// 1. test_minimal_config_parses
// ---------------------------------------------------------------------------

#[test]
fn test_minimal_config_parses() {
    let toml = r#"
[provider]
model = "test-model"
api_key = "test-key"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("minimal config should parse");
    assert_eq!(config.provider.model.as_deref(), Some("test-model"));
    assert_eq!(config.provider.api_key.as_deref(), Some("test-key"));
}

// ---------------------------------------------------------------------------
// 2. test_full_config_parses
// ---------------------------------------------------------------------------

#[test]
fn test_full_config_parses() {
    let toml = r#"
[agent]
system_prompt = "You are a helpful agent."

[agent.profile]
name = "TestProfile"
thinking_level = "high"
temperature = 0.7
max_tokens = 4096

[provider]
model = "claude-sonnet-4-20250514"
api_key = "sk-test-full"
api = "anthropic_messages"
base_url = "https://api.anthropic.com"
provider = "anthropic"
reasoning = true
context_window = 200000
max_tokens = 8192

[session]
scope = "persistent"
thinking_level = "medium"
temperature = 0.5

[tools]
enabled = ["bash", "read_file"]
tool_strategy = "parallel"

[compaction]
max_context_tokens = 100000
keep_first_turns = 2
keep_recent_turns = 5

[execution]
max_turns = 20
max_total_tokens = 500000
max_duration_secs = 300
max_cost = 1.5
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("full config should parse");
    assert_eq!(
        config.agent.system_prompt.as_deref(),
        Some("You are a helpful agent.")
    );
    assert_eq!(config.agent.profile.name.as_deref(), Some("TestProfile"));
    assert_eq!(
        config.provider.model.as_deref(),
        Some("claude-sonnet-4-20250514")
    );
    assert_eq!(config.session.scope.as_deref(), Some("persistent"));
    assert_eq!(config.tools.enabled, vec!["bash", "read_file"]);
    assert_eq!(config.compaction.max_context_tokens, Some(100000));
    assert_eq!(config.execution.max_turns, Some(20));
    assert_eq!(config.execution.max_cost, Some(1.5));
}

// ---------------------------------------------------------------------------
// 3. test_env_var_substitution
// ---------------------------------------------------------------------------

#[test]
fn test_env_var_substitution() {
    // Safety: test-only env var manipulation.
    unsafe {
        std::env::set_var("PHI_TEST_API_KEY", "my-secret-key");
    }

    let toml = r#"
[provider]
model = "test-model"
api_key = "${PHI_TEST_API_KEY}"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("env var substitution should work");
    assert_eq!(config.provider.api_key.as_deref(), Some("my-secret-key"));

    unsafe {
        std::env::remove_var("PHI_TEST_API_KEY");
    }
}

// ---------------------------------------------------------------------------
// 4. test_missing_env_var_error
// ---------------------------------------------------------------------------

#[test]
fn test_missing_env_var_error() {
    // Ensure the variable is definitely not set.
    unsafe {
        std::env::remove_var("DEFINITELY_NOT_SET_PHI_CONFIG_TEST");
    }

    let toml = r#"
[provider]
model = "test-model"
api_key = "${DEFINITELY_NOT_SET_PHI_CONFIG_TEST}"
"#;
    let result = parse_config(toml, ConfigFormat::Toml);
    assert!(result.is_err(), "should error on missing env var");
    match result.unwrap_err() {
        ConfigError::MissingEnvVar { var } => {
            assert_eq!(var, "DEFINITELY_NOT_SET_PHI_CONFIG_TEST");
        }
        other => panic!("expected MissingEnvVar, got: {other}"),
    }
}

// ---------------------------------------------------------------------------
// 5. test_agent_from_config_minimal
// ---------------------------------------------------------------------------

#[test]
fn test_agent_from_config_minimal() {
    let toml = r#"
[provider]
model = "test-model"
api_key = "test-key"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    let mc = agent.model_config().expect("model_config should be Some");
    assert_eq!(mc.id, "test-model");
}

// ---------------------------------------------------------------------------
// 6. test_agent_from_config_with_profile
// ---------------------------------------------------------------------------

#[test]
fn test_agent_from_config_with_profile() {
    let toml = r#"
[provider]
model = "test-model"
api_key = "test-key"

[agent.profile]
name = "Tester"
thinking_level = "high"
temperature = 0.9
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    let profile = agent.profile().expect("profile should be Some");
    assert_eq!(profile.name.as_deref(), Some("Tester"));
    assert_eq!(profile.thinking_level, Some(phi_core::ThinkingLevel::High));
    assert_eq!(profile.temperature, Some(0.9));
}

// ---------------------------------------------------------------------------
// 7. test_agent_from_config_with_execution
// ---------------------------------------------------------------------------

#[test]
fn test_agent_from_config_with_execution() {
    let toml = r#"
[provider]
model = "test-model"
api_key = "test-key"

[execution]
max_turns = 10
max_total_tokens = 100000
max_duration_secs = 60
max_cost = 0.5
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    let limits = agent
        .execution_limits()
        .expect("execution_limits should be Some");
    assert_eq!(limits.max_turns, 10);
    assert_eq!(limits.max_total_tokens, 100000);
    assert_eq!(limits.max_duration, std::time::Duration::from_secs(60));
    assert_eq!(limits.max_cost, Some(0.5));
}

// ---------------------------------------------------------------------------
// 8. test_thinking_level_parsing
// ---------------------------------------------------------------------------

#[test]
fn test_thinking_level_parsing() {
    let levels = ["off", "minimal", "low", "medium", "high"];
    let expected = [
        phi_core::ThinkingLevel::Off,
        phi_core::ThinkingLevel::Minimal,
        phi_core::ThinkingLevel::Low,
        phi_core::ThinkingLevel::Medium,
        phi_core::ThinkingLevel::High,
    ];

    for (level_str, expected_level) in levels.iter().zip(expected.iter()) {
        let toml = format!(
            r#"
[provider]
model = "test-model"
api_key = "test-key"

[agent.profile]
thinking_level = "{level_str}"
"#
        );
        let config = parse_config(&toml, ConfigFormat::Toml).unwrap();
        let agent = agent_from_config(&config).expect("agent construction should succeed");
        assert_eq!(
            agent.thinking_level(),
            *expected_level,
            "thinking_level mismatch for \"{level_str}\""
        );
    }

    // Invalid value should produce ConfigError::InvalidField
    let toml = r#"
[provider]
model = "test-model"
api_key = "test-key"

[agent.profile]
thinking_level = "invalid"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let result = agent_from_config(&config);
    match result {
        Err(ConfigError::InvalidField { field, value, .. }) => {
            assert_eq!(field, "thinking_level");
            assert_eq!(value, "invalid");
        }
        Err(other) => panic!("expected InvalidField, got: {other}"),
        Ok(_) => panic!("expected error for invalid thinking_level, got Ok"),
    }
}
