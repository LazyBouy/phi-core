//! Tests for the config module: parsing, env var substitution, and agent construction.

use phi_core::config::reference::{parse_config_ref, ConfigRef};
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

// ═══════════════════════════════════════════════════════════════════════════
// Config reference protocol tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_parse_config_ref_qualified() {
    assert_eq!(
        parse_config_ref("{{agent_profile.coder}}"),
        ConfigRef::Qualified {
            ref_type: "agent_profile".into(),
            name: "coder".into(),
            recreate: true,
        }
    );
}

#[test]
fn test_parse_config_ref_unqualified() {
    assert_eq!(
        parse_config_ref("{{coder}}"),
        ConfigRef::Unqualified {
            name: "coder".into(),
            recreate: true,
        }
    );
}

#[test]
fn test_parse_config_ref_no_recreate() {
    assert_eq!(
        parse_config_ref("{{%coder%}}"),
        ConfigRef::Unqualified {
            name: "coder".into(),
            recreate: false,
        }
    );
    assert_eq!(
        parse_config_ref("{{%provider.openai%}}"),
        ConfigRef::Qualified {
            ref_type: "provider".into(),
            name: "openai".into(),
            recreate: false,
        }
    );
}

#[test]
fn test_parse_config_ref_system_id() {
    assert_eq!(
        parse_config_ref("{{#fctsidd-abc-123#}}"),
        ConfigRef::SystemId {
            id: "fctsidd-abc-123".into(),
        }
    );
}

#[test]
fn test_parse_config_ref_literal() {
    assert_eq!(
        parse_config_ref("plain-string"),
        ConfigRef::Literal("plain-string".into())
    );
    assert_eq!(
        parse_config_ref("openai"),
        ConfigRef::Literal("openai".into())
    );
}

#[test]
fn test_profile_instances_parse() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
name = "base"
thinking_level = "medium"
temperature = 0.5

[[agent.profile.instances]]
id = "{{%coder%}}"
description = "A coding specialist"
thinking_level = "high"
temperature = 0.2
max_tokens = 16384

[[agent.profile.instances]]
id = "{{researcher}}"
description = "A research specialist"
thinking_level = "high"
temperature = 0.7
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(config.agent.profile.instances.len(), 2);
    assert_eq!(config.agent.profile.instances[0].id, "{{%coder%}}");
    assert_eq!(
        config.agent.profile.instances[0].thinking_level.as_deref(),
        Some("high")
    );
    assert_eq!(config.agent.profile.instances[1].id, "{{researcher}}");
}

#[test]
fn test_url_alias() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"
url = "http://localhost:8080/v1"

[[provider.instances]]
name = "local"
model = "llama3"
url = "http://localhost:11434/v1"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(
        config.provider.base_url.as_deref(),
        Some("http://localhost:8080/v1")
    );
    assert_eq!(
        config.provider.instances[0].base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
}

#[test]
fn test_agent_instance_with_profile_ref() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
name = "base"
thinking_level = "low"

[[agent.profile.instances]]
id = "{{%coder%}}"
thinking_level = "high"
temperature = 0.2

[[agent.instances]]
name = "code-writer"
agent_profile = "{{agent_profile.coder}}"
system_prompt = "You write code."
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(config.agent.instances.len(), 1);
    assert_eq!(
        config.agent.instances[0].agent_profile.as_deref(),
        Some("{{agent_profile.coder}}")
    );
    assert_eq!(
        config.agent.instances[0].system_prompt.as_deref(),
        Some("You write code.")
    );
}

#[test]
fn test_provider_instance_with_id() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[[provider.instances]]
id = "{{%openai%}}"
description = "OpenAI provider"
model = "gpt-4o"
api_key = "sk-test"
api = "openai_completions"
url = "https://api.openai.com/v1"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(config.provider.instances.len(), 1);
    assert_eq!(
        config.provider.instances[0].id.as_deref(),
        Some("{{%openai%}}")
    );
    assert_eq!(
        config.provider.instances[0].description.as_deref(),
        Some("OpenAI provider")
    );
    assert_eq!(
        config.provider.instances[0].base_url.as_deref(),
        Some("https://api.openai.com/v1")
    );
}

// ---------------------------------------------------------------------------
// 13. test_config_system_prompt_strategy_parse
// ---------------------------------------------------------------------------

#[test]
fn test_config_system_prompt_strategy_parse() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[[system_prompt_strategy.instances]]
id = "{{agent_layout}}"
description = "Standard 3-block layout"

[[system_prompt_strategy.instances.blocks]]
name = "identity"
order = 0
max_length = 500

[[system_prompt_strategy.instances.blocks]]
name = "instructions"
order = 1
max_length = 2000
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse strategy config");
    assert_eq!(config.system_prompt_strategy.instances.len(), 1);
    let inst = &config.system_prompt_strategy.instances[0];
    assert_eq!(inst.id, "{{agent_layout}}");
    assert_eq!(inst.blocks.len(), 2);
    assert_eq!(inst.blocks[0].name, "identity");
    assert_eq!(inst.blocks[0].order, Some(0));
    assert_eq!(inst.blocks[0].max_length, Some(500));
    assert_eq!(inst.blocks[1].name, "instructions");
    assert_eq!(inst.blocks[1].order, Some(1));
    assert_eq!(inst.blocks[1].max_length, Some(2000));
}

// ---------------------------------------------------------------------------
// 14. test_config_system_prompt_instance_parse
// ---------------------------------------------------------------------------

#[test]
fn test_config_system_prompt_instance_parse() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[[system_prompt.instances]]
id = "{{coder_prompt}}"
description = "Prompt for coding agent"
type = "{{agent_layout}}"
identity = "You are Phi."
instructions = "Write clean code."
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse prompt instance");
    assert_eq!(config.system_prompt.instances.len(), 1);
    let inst = &config.system_prompt.instances[0];
    assert_eq!(inst.id, "{{coder_prompt}}");
    assert_eq!(inst.strategy_type.as_deref(), Some("{{agent_layout}}"));
    // Block content captured via flatten
    assert!(
        inst.blocks.contains_key("identity"),
        "should have identity block"
    );
    assert!(
        inst.blocks.contains_key("instructions"),
        "should have instructions block"
    );
}

// ---------------------------------------------------------------------------
// 15. test_workspace_from_config
// ---------------------------------------------------------------------------

#[test]
fn test_workspace_from_config() {
    let toml = r#"
default_workspace = "/tmp/test"

[provider]
model = "test"
api_key = "test"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse workspace config");
    assert_eq!(config.default_workspace.as_deref(), Some("/tmp/test"));
}

// ---------------------------------------------------------------------------
// 16. test_basic_agent_with_workspace
// ---------------------------------------------------------------------------

#[test]
fn test_basic_agent_with_workspace() {
    use phi_core::provider::ModelConfig;
    use phi_core::{Agent, BasicAgent};
    use std::path::Path;

    let agent =
        BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test")).with_workspace("/tmp");

    assert_eq!(
        agent.workspace(),
        Some(Path::new("/tmp")),
        "workspace should be set"
    );
}

// ---------------------------------------------------------------------------
// 17. test_agent_workspace_overrides_default
// ---------------------------------------------------------------------------

#[test]
fn test_agent_workspace_overrides_default() {
    let toml = r#"
default_workspace = "/default"

[provider]
model = "test"
api_key = "test"

[agent]
workspace = "/override"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(
        config.agent.workspace.as_deref(),
        Some("/override"),
        "agent workspace should be /override"
    );
    assert_eq!(
        config.default_workspace.as_deref(),
        Some("/default"),
        "default_workspace should be /default"
    );
}

// ---------------------------------------------------------------------------
// 18. test_workspace_none_when_omitted
// ---------------------------------------------------------------------------

#[test]
fn test_workspace_none_when_omitted() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert!(
        config.default_workspace.is_none(),
        "default_workspace should be None when omitted"
    );
    assert!(
        config.agent.workspace.is_none(),
        "agent.workspace should be None when omitted"
    );
}

// ---------------------------------------------------------------------------
// 19. test_basic_agent_workspace_none_by_default
// ---------------------------------------------------------------------------

#[test]
fn test_basic_agent_workspace_none_by_default() {
    use phi_core::provider::ModelConfig;
    use phi_core::{Agent, BasicAgent};

    let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"));
    assert!(
        agent.workspace().is_none(),
        "workspace should be None by default"
    );
}

// ---------------------------------------------------------------------------
// 20. test_config_agent_workspace_field
// ---------------------------------------------------------------------------

#[test]
fn test_config_agent_workspace_field() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent]
workspace = "/home/agent"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(
        config.agent.workspace.as_deref(),
        Some("/home/agent"),
        "agent workspace should parse correctly"
    );
}

// ---------------------------------------------------------------------------
// 21. test_config_callbacks_section_parse
// ---------------------------------------------------------------------------

#[test]
fn test_config_callbacks_section_parse() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[callbacks]
before_loop = "scripts/hook.sh"
after_loop = "scripts/after.py"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse callbacks config");
    assert_eq!(
        config.callbacks.before_loop.as_deref(),
        Some("scripts/hook.sh")
    );
    assert_eq!(
        config.callbacks.after_loop.as_deref(),
        Some("scripts/after.py")
    );
}

// ---------------------------------------------------------------------------
// 22. test_system_prompt_ref_resolved_from_config
// ---------------------------------------------------------------------------

#[test]
fn test_system_prompt_ref_resolved_from_config() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[[system_prompt_strategy.instances]]
id = "{{layout}}"

[[system_prompt_strategy.instances.blocks]]
name = "identity"
order = 0
max_length = 1000

[[system_prompt_strategy.instances.blocks]]
name = "task"
order = 1
max_length = 2000

[[system_prompt.instances]]
id = "{{coder_prompt}}"
type = "{{layout}}"
identity = "I am Phi."
task = "Write code."

[agent.profile]
system_prompt = "{{coder_prompt}}"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    let prompt = agent.system_prompt();
    assert!(
        prompt.contains("I am Phi."),
        "system_prompt should contain identity block, got: {prompt}"
    );
    assert!(
        prompt.contains("Write code."),
        "system_prompt should contain task block, got: {prompt}"
    );
    // Identity (order 0) should come before task (order 1)
    let identity_pos = prompt.find("I am Phi.").unwrap();
    let task_pos = prompt.find("Write code.").unwrap();
    assert!(
        identity_pos < task_pos,
        "identity block should appear before task block"
    );
}

// ---------------------------------------------------------------------------
// 23. test_raw_system_prompt_still_works
// ---------------------------------------------------------------------------

#[test]
fn test_raw_system_prompt_still_works() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent]
system_prompt = "Hello world"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    assert_eq!(
        agent.system_prompt(),
        "Hello world",
        "raw system_prompt should be returned as-is"
    );
}

// ---------------------------------------------------------------------------
// 24. test_workspace_wired_from_config
// ---------------------------------------------------------------------------

#[test]
fn test_workspace_wired_from_config() {
    use std::path::Path;

    let toml = r#"
default_workspace = "/tmp/ws"

[provider]
model = "test"
api_key = "test"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    assert_eq!(
        agent.workspace(),
        Some(Path::new("/tmp/ws")),
        "workspace should be wired from default_workspace"
    );
}
