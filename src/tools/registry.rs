//! G10 — Tool registry for config-driven tool instantiation.
//!
//! `ToolRegistry` maps tool names to factory functions, enabling
//! `agent_from_config_with_registry()` to resolve `tools.enabled` names
//! into concrete `Arc<dyn AgentTool>` instances.

use crate::types::AgentTool;
use std::collections::HashMap;
use std::sync::Arc;

/// Maps tool names to factory functions for instantiation from config.
///
/// # Example
///
/// ```rust
/// use phi_core::tools::ToolRegistry;
///
/// let registry = ToolRegistry::new().with_defaults();
/// let tools = registry.resolve(&["bash".to_string(), "read_file".to_string()]);
/// assert_eq!(tools.len(), 2);
/// ```
pub struct ToolRegistry {
    factories: HashMap<String, Box<dyn Fn() -> Arc<dyn AgentTool> + Send + Sync>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register all 6 built-in tools.
    pub fn with_defaults(mut self) -> Self {
        self.register("bash", || Arc::new(super::BashTool::default()));
        self.register("read_file", || Arc::new(super::ReadFileTool::default()));
        self.register("write_file", || Arc::new(super::WriteFileTool::new()));
        self.register("edit_file", || Arc::new(super::EditFileTool::new()));
        self.register("list_files", || Arc::new(super::ListFilesTool::default()));
        self.register("search", || Arc::new(super::SearchTool::default()));
        self
    }

    /// Register a tool factory under the given name.
    ///
    /// If a factory with the same name already exists, it is replaced.
    pub fn register(
        &mut self,
        name: &str,
        factory: impl Fn() -> Arc<dyn AgentTool> + Send + Sync + 'static,
    ) {
        self.factories.insert(name.to_string(), Box::new(factory));
    }

    /// Resolve tool names to instances. Unknown names are silently skipped.
    pub fn resolve(&self, names: &[String]) -> Vec<Arc<dyn AgentTool>> {
        names
            .iter()
            .filter_map(|name| self.factories.get(name).map(|f| f()))
            .collect()
    }

    /// Check whether a tool name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }

    /// Return the number of registered tool factories.
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// Return whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_registry() {
        let registry = ToolRegistry::new();
        assert!(registry.is_empty());
        let tools = registry.resolve(&["bash".to_string()]);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_with_defaults() {
        let registry = ToolRegistry::new().with_defaults();
        assert_eq!(registry.len(), 6);
        assert!(registry.contains("bash"));
        assert!(registry.contains("read_file"));
        assert!(registry.contains("write_file"));
        assert!(registry.contains("edit_file"));
        assert!(registry.contains("list_files"));
        assert!(registry.contains("search"));
    }

    #[test]
    fn test_resolve_subset() {
        let registry = ToolRegistry::new().with_defaults();
        let tools = registry.resolve(&["bash".to_string(), "search".to_string()]);
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn test_resolve_skips_unknown() {
        let registry = ToolRegistry::new().with_defaults();
        let tools = registry.resolve(&["bash".to_string(), "nonexistent".to_string()]);
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn test_custom_registration() {
        let mut registry = ToolRegistry::new();
        registry.register("bash", || Arc::new(super::super::BashTool::default()));
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("bash"));
    }
}
