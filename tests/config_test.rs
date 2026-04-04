//! Tests for the config module: parsing, env var substitution, and agent construction.

use phi_core::config::reference::{parse_config_ref, ConfigRef};
use phi_core::config::{
    agent_from_config, agent_from_config_with_registry, agents_from_config, parse_config,
    ConfigError, ConfigFormat,
};
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

// ===========================================================================
// Compact with Focus (tests 25–28)
// ===========================================================================

// ---------------------------------------------------------------------------
// 25. test_focus_message_none_default
// ---------------------------------------------------------------------------

#[test]
fn test_focus_message_none_default() {
    let cc = phi_core::CompactionConfig::default();
    assert!(
        cc.focus_message.is_none(),
        "CompactionConfig::default().focus_message should be None"
    );
}

// ---------------------------------------------------------------------------
// 26. test_compaction_instance_parse
// ---------------------------------------------------------------------------

#[test]
fn test_compaction_instance_parse() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[[compaction.instances]]
id = "{{spec_focused}}"
description = "Focus on specs"
focus_message = "Focus on API specs"
max_context_tokens = 80000

[[compaction.instances]]
id = "{{code_focused}}"
description = "Focus on code"
focus_message = "Focus on implementation details"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse compaction instances");
    assert_eq!(config.compaction.instances.len(), 2);
    assert_eq!(config.compaction.instances[0].id, "{{spec_focused}}");
    assert_eq!(
        config.compaction.instances[0].description.as_deref(),
        Some("Focus on specs")
    );
    assert_eq!(
        config.compaction.instances[0].focus_message.as_deref(),
        Some("Focus on API specs")
    );
    assert_eq!(
        config.compaction.instances[0].max_context_tokens,
        Some(80000)
    );
    assert_eq!(config.compaction.instances[1].id, "{{code_focused}}");
    assert_eq!(
        config.compaction.instances[1].focus_message.as_deref(),
        Some("Focus on implementation details")
    );
}

// ---------------------------------------------------------------------------
// 27. test_compaction_instance_ref_in_profile
// ---------------------------------------------------------------------------

#[test]
fn test_compaction_instance_ref_in_profile() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
name = "test"
compaction = "{{spec_focused}}"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(
        config.agent.profile.compaction.as_deref(),
        Some("{{spec_focused}}"),
        "profile compaction reference should be preserved"
    );
}

// ---------------------------------------------------------------------------
// 28. test_focus_message_from_config
// ---------------------------------------------------------------------------

#[test]
fn test_focus_message_from_config() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[compaction]
focus_message = "focus on APIs"
max_context_tokens = 100000
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    let ctx = agent
        .context_config()
        .expect("context_config should be Some");
    assert_eq!(
        ctx.compaction.focus_message.as_deref(),
        Some("focus on APIs"),
        "focus_message should be wired from config"
    );
}

// ===========================================================================
// G10: Tool Registry (tests 29–32)
// ===========================================================================

// ---------------------------------------------------------------------------
// 29. test_registry_with_defaults_has_6_tools
// ---------------------------------------------------------------------------

#[test]
fn test_registry_with_defaults_has_6_tools() {
    use phi_core::tools::ToolRegistry;

    let registry = ToolRegistry::new().with_defaults();
    let all_names = vec![
        "bash".to_string(),
        "read_file".to_string(),
        "write_file".to_string(),
        "edit_file".to_string(),
        "list_files".to_string(),
        "search".to_string(),
    ];
    let tools = registry.resolve(&all_names);
    assert_eq!(tools.len(), 6, "should resolve all 6 default tools");
}

// ---------------------------------------------------------------------------
// 30. test_registry_resolve_subset
// ---------------------------------------------------------------------------

#[test]
fn test_registry_resolve_subset() {
    use phi_core::tools::ToolRegistry;

    let registry = ToolRegistry::new().with_defaults();
    let tools = registry.resolve(&["bash".to_string(), "search".to_string()]);
    assert_eq!(tools.len(), 2, "should resolve 2 tools");
}

// ---------------------------------------------------------------------------
// 31. test_registry_resolve_unknown_skipped
// ---------------------------------------------------------------------------

#[test]
fn test_registry_resolve_unknown_skipped() {
    use phi_core::tools::ToolRegistry;

    let registry = ToolRegistry::new().with_defaults();
    let tools = registry.resolve(&["bash".to_string(), "nonexistent".to_string()]);
    assert_eq!(
        tools.len(),
        1,
        "unknown tool names should be silently skipped"
    );
}

// ---------------------------------------------------------------------------
// 32. test_agent_from_config_with_registry
// ---------------------------------------------------------------------------

#[test]
fn test_agent_from_config_with_registry() {
    use phi_core::tools::ToolRegistry;

    let toml = r#"
[provider]
model = "test"
api_key = "test"

[tools]
enabled = ["bash", "search"]
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let registry = ToolRegistry::new().with_defaults();
    let agent =
        agent_from_config_with_registry(&config, &registry).expect("should build with registry");
    // Just verify it doesn't error — tool presence is internal to BasicAgent
    assert_eq!(
        agent.model_config().unwrap().id,
        "test",
        "agent should be constructed"
    );
}

// ===========================================================================
// agents_from_config() (tests 33–35)
// ===========================================================================

// ---------------------------------------------------------------------------
// 33. test_agents_from_config_empty_instances
// ---------------------------------------------------------------------------

#[test]
fn test_agents_from_config_empty_instances() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agents = agents_from_config(&config).expect("should build agents");
    assert_eq!(agents.len(), 1, "no instances → single default agent");
    assert_eq!(
        agents[0].0, "default",
        "default agent should be named 'default'"
    );
}

// ---------------------------------------------------------------------------
// 34. test_agents_from_config_with_instances
// ---------------------------------------------------------------------------

#[test]
fn test_agents_from_config_with_instances() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[[agent.instances]]
name = "writer"

[[agent.instances]]
name = "reviewer"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agents = agents_from_config(&config).expect("should build agents");
    assert_eq!(agents.len(), 2, "should build 2 agent instances");
    assert_eq!(agents[0].0, "writer");
    assert_eq!(agents[1].0, "reviewer");
}

// ---------------------------------------------------------------------------
// 35. test_agents_from_config_instance_system_prompt
// ---------------------------------------------------------------------------

#[test]
fn test_agents_from_config_instance_system_prompt() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[[agent.instances]]
name = "custom"
system_prompt = "You are a custom agent."
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agents = agents_from_config(&config).expect("should build agents");
    assert_eq!(agents.len(), 1);
    assert_eq!(
        agents[0].1.system_prompt(),
        "You are a custom agent.",
        "instance system_prompt override should be applied"
    );
}

// ===========================================================================
// before_turn/after_turn callbacks schema (tests 36–37)
// ===========================================================================

// ---------------------------------------------------------------------------
// 36. test_before_turn_in_callbacks_schema
// ---------------------------------------------------------------------------

#[test]
fn test_before_turn_in_callbacks_schema() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[callbacks]
before_turn = "scripts/hook.sh"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(
        config.callbacks.before_turn.as_deref(),
        Some("scripts/hook.sh"),
        "before_turn callback should be parsed"
    );
}

// ---------------------------------------------------------------------------
// 37. test_after_turn_in_callbacks_schema
// ---------------------------------------------------------------------------

#[test]
fn test_after_turn_in_callbacks_schema() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[callbacks]
after_turn = "scripts/hook.py"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    assert_eq!(
        config.callbacks.after_turn.as_deref(),
        Some("scripts/hook.py"),
        "after_turn callback should be parsed"
    );
}

// ===========================================================================
// Context translation, compaction instance, provider instance (tests 38–40b)
// ===========================================================================

// ---------------------------------------------------------------------------
// test_context_translation_stored_on_basic_agent
// ---------------------------------------------------------------------------

#[test]
fn test_context_translation_stored_on_basic_agent() {
    use phi_core::provider::context_translation::DefaultContextTranslation;
    use phi_core::provider::ModelConfig;
    use phi_core::{Agent, BasicAgent};
    use std::sync::Arc;

    let agent = BasicAgent::new(ModelConfig::anthropic("test", "test", "test"))
        .with_context_translation(Arc::new(DefaultContextTranslation));
    // Verify via Agent trait getter
    let ct = agent.context_translation();
    assert!(
        ct.is_some(),
        "context_translation should be Some after with_context_translation"
    );
}

// ---------------------------------------------------------------------------
// test_compaction_instance_resolved_from_profile
// ---------------------------------------------------------------------------

#[test]
fn test_compaction_instance_resolved_from_profile() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[compaction]
max_context_tokens = 200000

[[compaction.instances]]
id = "{{coding}}"
focus_message = "Focus on code"

[agent.profile]
compaction = "{{coding}}"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("agent construction should succeed");
    let ctx = agent
        .context_config()
        .expect("context_config should be Some");
    assert_eq!(
        ctx.compaction.focus_message.as_deref(),
        Some("Focus on code"),
        "focus_message should be resolved from compaction instance"
    );
}

// ---------------------------------------------------------------------------
// test_agents_from_config_instance_provider_ref
// ---------------------------------------------------------------------------

#[test]
fn test_agents_from_config_instance_provider_ref() {
    let toml = r#"
[provider]
model = "default-model"
api_key = "test"

[[provider.instances]]
id = "{{%openai%}}"
model = "gpt-4o"
api = "openai_completions"

[[agent.instances]]
name = "openai-agent"
provider = "{{openai}}"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agents = agents_from_config(&config).expect("should build agents");
    let (name, agent) = agents
        .iter()
        .find(|(n, _)| n == "openai-agent")
        .expect("should find openai-agent");
    assert_eq!(name, "openai-agent");
    assert_eq!(
        agent.model_config().unwrap().id,
        "gpt-4o",
        "provider instance model should override default"
    );
}

// ===========================================================================
// OpenAI compat (tests 38–40)
// ===========================================================================

// ---------------------------------------------------------------------------
// 38. test_compat_config_parsed
// ---------------------------------------------------------------------------

#[test]
fn test_compat_config_parsed() {
    let toml = r#"
[provider]
model = "grok-3"
api_key = "test"
api = "openai_completions"

[provider.compat]
reasoning_format = "xai"
max_tokens_field = "max_completion_tokens"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("should build agent");
    let mc = agent.model_config().expect("model_config should exist");
    let compat = mc.compat.as_ref().expect("compat should be Some");
    assert_eq!(
        compat.thinking_format,
        phi_core::provider::model::ThinkingFormat::Xai,
        "reasoning_format should map to ThinkingFormat::Xai"
    );
    assert_eq!(
        compat.max_tokens_field,
        phi_core::provider::model::MaxTokensField::MaxCompletionTokens,
        "max_tokens_field should map to MaxCompletionTokens"
    );
}

// ---------------------------------------------------------------------------
// 39. test_compat_none_when_empty
// ---------------------------------------------------------------------------

#[test]
fn test_compat_none_when_empty() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("should build agent");
    let mc = agent.model_config().expect("model_config should exist");
    assert!(
        mc.compat.is_none(),
        "compat should be None when no compat fields are set"
    );
}

// ---------------------------------------------------------------------------
// 40. test_compat_reasoning_format_mapping
// ---------------------------------------------------------------------------

#[test]
fn test_compat_reasoning_format_mapping() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"
api = "openai_completions"

[provider.compat]
reasoning_format = "openrouter"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).unwrap();
    let agent = agent_from_config(&config).expect("should build agent");
    let mc = agent.model_config().unwrap();
    let compat = mc.compat.as_ref().expect("compat should be Some");
    assert_eq!(
        compat.thinking_format,
        phi_core::provider::model::ThinkingFormat::OpenRouter,
        "reasoning_format 'openrouter' should map to ThinkingFormat::OpenRouter"
    );
}

// ===========================================================================
// TokenCounter tests
// ===========================================================================

use phi_core::context::token::{estimate_tokens, HeuristicTokenCounter, TokenCounter};
use phi_core::ContextConfig;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// 9. test_heuristic_counter_matches_free_functions
// ---------------------------------------------------------------------------

#[test]
fn test_heuristic_counter_matches_free_functions() {
    let counter = HeuristicTokenCounter;
    let texts = ["hello", "a longer piece of text for testing", "", "x"];
    for text in &texts {
        assert_eq!(
            counter.estimate_text(text),
            estimate_tokens(text),
            "HeuristicTokenCounter.estimate_text and estimate_tokens should agree for {:?}",
            text
        );
    }
}

// ---------------------------------------------------------------------------
// 10. test_custom_counter_override
// ---------------------------------------------------------------------------

struct ConstantCounter;

impl TokenCounter for ConstantCounter {
    fn estimate_text(&self, _text: &str) -> usize {
        1
    }
}

#[test]
fn test_custom_counter_override() {
    let counter = ConstantCounter;
    // estimate_text always returns 1
    assert_eq!(counter.estimate_text("hello world"), 1);

    // estimate_message should use estimate_text (which returns 1) + overhead,
    // NOT the heuristic chars/4 value
    let msg = phi_core::AgentMessage::Llm(phi_core::LlmMessage::new(phi_core::Message::User {
        content: vec![phi_core::Content::Text {
            text: "a very long string that would be many tokens with heuristic".to_string(),
        }],
        timestamp: 0,
    }));
    let custom_estimate = counter.estimate_message(&msg);
    let heuristic_estimate = HeuristicTokenCounter.estimate_message(&msg);

    // Custom should be much smaller: 1 (text) + 4 (overhead) = 5
    assert_eq!(custom_estimate, 1 + 4);
    // Heuristic should be much larger
    assert!(
        heuristic_estimate > custom_estimate,
        "heuristic ({}) should exceed constant counter ({})",
        heuristic_estimate,
        custom_estimate
    );
}

// ---------------------------------------------------------------------------
// 11. test_context_config_counter_fallback
// ---------------------------------------------------------------------------

#[test]
fn test_context_config_counter_fallback() {
    let config = ContextConfig {
        token_counter: None,
        ..Default::default()
    };
    let counter = config.counter();
    // Should behave like HeuristicTokenCounter
    assert_eq!(
        counter.estimate_text("hello"),
        HeuristicTokenCounter.estimate_text("hello")
    );
    assert_eq!(
        counter.estimate_text("test string"),
        HeuristicTokenCounter.estimate_text("test string")
    );
}

// ---------------------------------------------------------------------------
// 12. test_context_config_custom_counter
// ---------------------------------------------------------------------------

#[test]
fn test_context_config_custom_counter() {
    let config = ContextConfig {
        token_counter: Some(Arc::new(ConstantCounter)),
        ..Default::default()
    };
    let counter = config.counter();
    // Should use ConstantCounter (always returns 1), not heuristic
    assert_eq!(
        counter.estimate_text("hello world this is a long string"),
        1
    );
    assert_eq!(counter.estimate_text("x"), 1);
}

// ---------------------------------------------------------------------------
// file: prefix resolution in system_prompt
// ---------------------------------------------------------------------------

#[test]
fn test_file_prefix_system_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let prompt_path = dir.path().join("my_prompt.md");
    std::fs::write(&prompt_path, "You are a test agent from a file.").unwrap();

    // Use absolute path so workspace resolution isn't needed
    let toml = format!(
        r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
system_prompt = "file:{}"
"#,
        prompt_path.display()
    );
    let config = parse_config(&toml, ConfigFormat::Toml).expect("should parse");
    let agent = agent_from_config(&config).expect("should build");
    assert_eq!(agent.system_prompt(), "You are a test agent from a file.");
}

#[test]
fn test_file_prefix_relative_to_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("prompt.md"), "Workspace-relative prompt.").unwrap();

    let toml = format!(
        r#"
[provider]
model = "test"
api_key = "test"

[agent]
workspace = "{}"

[agent.profile]
system_prompt = "file:prompt.md"
"#,
        ws.display()
    );
    let config = parse_config(&toml, ConfigFormat::Toml).expect("should parse");
    let agent = agent_from_config(&config).expect("should build");
    assert_eq!(agent.system_prompt(), "Workspace-relative prompt.");
}

// ---------------------------------------------------------------------------
// Profile instance system_prompt participates in resolution
// ---------------------------------------------------------------------------

#[test]
fn test_profile_instance_system_prompt_resolved() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
name = "base"
system_prompt = "base fallback prompt"

[[agent.profile.instances]]
id = "{{specialist}}"
system_prompt = "specialist prompt from instance"

[[agent.instances]]
name = "my-specialist"
agent_profile = "{{agent_profile.specialist}}"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    let agents = agents_from_config(&config).expect("should build");
    assert_eq!(agents.len(), 1);
    let (name, agent) = &agents[0];
    assert_eq!(name, "my-specialist");
    assert_eq!(agent.system_prompt(), "specialist prompt from instance");
}

#[test]
fn test_profile_instance_file_system_prompt() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("specialist.md"), "File-based specialist.").unwrap();

    let toml = format!(
        r#"
[provider]
model = "test"
api_key = "test"

[agent]
workspace = "{}"

[agent.profile]
name = "base"

[[agent.profile.instances]]
id = "{{{{specialist}}}}"
system_prompt = "file:specialist.md"

[[agent.instances]]
name = "my-specialist"
agent_profile = "{{{{agent_profile.specialist}}}}"
"#,
        dir.path().display()
    );
    let config = parse_config(&toml, ConfigFormat::Toml).expect("should parse");
    let agents = agents_from_config(&config).expect("should build");
    let (_, agent) = &agents[0];
    assert_eq!(agent.system_prompt(), "File-based specialist.");
}

#[test]
fn test_agent_level_overrides_profile_instance() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
name = "base"

[[agent.profile.instances]]
id = "{{specialist}}"
system_prompt = "profile instance prompt"

[[agent.instances]]
name = "overridden"
agent_profile = "{{agent_profile.specialist}}"
system_prompt = "agent instance override"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    let agents = agents_from_config(&config).expect("should build");
    let (_, agent) = &agents[0];
    // Agent instance system_prompt overrides profile instance
    assert_eq!(agent.system_prompt(), "agent instance override");
}

#[test]
fn test_base_profile_used_without_instance() {
    let toml = r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
name = "base"
system_prompt = "base prompt"

[[agent.instances]]
name = "generalist"
"#;
    let config = parse_config(toml, ConfigFormat::Toml).expect("should parse");
    let agents = agents_from_config(&config).expect("should build");
    let (_, agent) = &agents[0];
    assert_eq!(agent.system_prompt(), "base prompt");
}

// ---------------------------------------------------------------------------
// Per-instance workspace
// ---------------------------------------------------------------------------

#[test]
fn test_per_instance_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ws_alpha = dir.path().join("alpha");
    let ws_beta = dir.path().join("beta");
    std::fs::create_dir_all(&ws_alpha).unwrap();
    std::fs::create_dir_all(&ws_beta).unwrap();
    std::fs::write(ws_alpha.join("prompt.md"), "Alpha prompt.").unwrap();
    std::fs::write(ws_beta.join("prompt.md"), "Beta prompt.").unwrap();

    let toml = format!(
        r#"
[provider]
model = "test"
api_key = "test"

[agent.profile]
name = "base"

[[agent.profile.instances]]
id = "{{{{writer}}}}"
system_prompt = "file:prompt.md"

[[agent.instances]]
name = "alpha"
agent_profile = "{{{{agent_profile.writer}}}}"
workspace = "{}"

[[agent.instances]]
name = "beta"
agent_profile = "{{{{agent_profile.writer}}}}"
workspace = "{}"
"#,
        ws_alpha.display(),
        ws_beta.display()
    );
    let config = parse_config(&toml, ConfigFormat::Toml).expect("should parse");
    let agents = agents_from_config(&config).expect("should build");
    assert_eq!(agents.len(), 2);

    let (name_a, agent_a) = &agents[0];
    let (name_b, agent_b) = &agents[1];
    assert_eq!(name_a, "alpha");
    assert_eq!(name_b, "beta");
    assert_eq!(agent_a.system_prompt(), "Alpha prompt.");
    assert_eq!(agent_b.system_prompt(), "Beta prompt.");
}
