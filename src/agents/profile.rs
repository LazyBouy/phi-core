use crate::types::ThinkingLevel;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A reusable agent blueprint that defines default configuration for agents.
///
/// Multiple agents can share the same profile, inheriting its settings as defaults.
/// Agent-level settings (e.g. `system_prompt` on `BasicAgent`) override the profile's
/// values when both are set.
///
/// ## Resolution order
///
/// For fields like `thinking_level` and `temperature`, the resolution order is:
///   1. Session override (if set)
///   2. Profile value (if set)
///   3. Crate default (`ThinkingLevel::Off`, `None`)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Unique identifier for this profile. Auto-generated UUID when using `Default`.
    #[serde(default = "default_profile_id")]
    pub profile_id: String,

    /// Human-readable name for agents using this profile.
    #[serde(default)]
    pub name: Option<String>,

    /// Description of the profile's purpose or capabilities.
    #[serde(default)]
    pub description: Option<String>,

    /// Default system prompt for agents using this profile.
    /// Can be overridden at the agent level.
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// Default thinking level for agents using this profile.
    #[serde(default)]
    pub thinking_level: Option<ThinkingLevel>,

    /// Default temperature for agents using this profile.
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Default max tokens for agents using this profile.
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// Stable config identity. When set, used as the middle segment of `loop_id`:
    ///   `loop_id = "{session_id}.{config_id}.{N}"`
    #[serde(default)]
    pub config_id: Option<String>,

    /// Skill names loaded via `SkillSet` from SKILL.md files.
    /// These are NOT tools — they are skill definitions per the AgentSkills standard.
    #[serde(default)]
    pub skills: Vec<String>,

    /// Agent workspace directory. File paths in system prompt blocks resolve
    /// relative to this directory.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
}

fn default_profile_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Default for AgentProfile {
    fn default() -> Self {
        Self {
            profile_id: default_profile_id(),
            name: None,
            description: None,
            system_prompt: None,
            thinking_level: None,
            temperature: None,
            max_tokens: None,
            config_id: None,
            skills: Vec::new(),
            workspace: None,
        }
    }
}

impl AgentProfile {
    /// Resolve thinking level with optional session override.
    ///
    /// Resolution: session_override > profile value > ThinkingLevel::Off
    pub fn resolve_thinking_level(&self, session_override: Option<ThinkingLevel>) -> ThinkingLevel {
        session_override
            .or(self.thinking_level)
            .unwrap_or(ThinkingLevel::Off)
    }

    /// Resolve temperature with optional session override.
    ///
    /// Resolution: session_override > profile value > None
    pub fn resolve_temperature(&self, session_override: Option<f32>) -> Option<f32> {
        session_override.or(self.temperature)
    }
}
