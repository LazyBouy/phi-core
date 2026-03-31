//! Configuration module — TOML/JSON/YAML config → Agent construction pipeline.
//!
//! # Overview
//!
//! This module provides a declarative configuration system for building agents:
//!
//! 1. **Schema** ([`AgentConfig`]) — the deserialization target for config files
//! 2. **Parser** ([`parse_config`], [`parse_config_file`]) — multi-format parsing with env var substitution
//! 3. **Builder** ([`agent_from_config`]) — constructs `Arc<dyn Agent>` from parsed config
//!
//! # Example
//!
//! ```ignore
//! let config = parse_config_file(Path::new("agent.toml"))?;
//! let agent = agent_from_config(&config)?;
//! ```

mod builder;
mod parser;
mod schema;

pub use builder::{agent_from_config, ConfigError};
pub use parser::{parse_config, parse_config_auto, parse_config_file, ConfigFormat};
pub use schema::{
    AgentConfig, AgentInstanceSection, AgentSection, CacheSection, CallbacksSection,
    CompactionSection, CompatSection, CostSection, ExecutionSection, HooksSection, ProfileSection,
    ProviderInstance, ProviderSection, RetrySection, SessionSection, SkillsSection,
    SubAgentsSection, ToolInstance, ToolsSection,
};
