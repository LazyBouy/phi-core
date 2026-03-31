//! SystemPromptStrategy — structured system prompt composition.
//!
//! The system prompt is structured as **ordered blocks** with token budgets.
//! Three entities form a reference chain:
//!
//! 1. **SystemPromptStrategy** — defines the structure template (block names, order, max_length)
//! 2. **SystemPrompt** — fills content into a strategy's blocks (text or file paths)
//! 3. **AgentProfile.system_prompt** — references a SystemPrompt instance (or is raw text)
//!
//! # Example
//!
//! ```
//! use phi_core::agents::system_prompt::*;
//! use std::collections::HashMap;
//! use std::path::Path;
//!
//! // Define a strategy template
//! let strategy = CustomPromptStrategy {
//!     blocks: vec![
//!         PromptBlockDef { name: "identity".into(), order: 0, max_length: 500 },
//!         PromptBlockDef { name: "instructions".into(), order: 1, max_length: 2000 },
//!     ],
//! };
//!
//! // Fill content into the template
//! let mut blocks = HashMap::new();
//! blocks.insert("identity".into(), "You are Phi, an expert coder.".into());
//! blocks.insert("instructions".into(), "Write clean, well-tested code.".into());
//!
//! let prompt = SystemPrompt {
//!     id: "coder".into(),
//!     description: Some("Coding agent prompt".into()),
//!     strategy_ref: "agent_layout".into(),
//!     blocks,
//! };
//!
//! let result = prompt.compose(&strategy, Path::new(".")).unwrap();
//! assert!(result.contains("Phi"));
//! assert!(result.contains("well-tested"));
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ── Block definition ────────────────────────────────────────────────────────

/// A block definition within a SystemPromptStrategy (structure template).
///
/// Defines a named slot in the system prompt with an ordering and token budget.
/// The actual content is provided by a `SystemPrompt` instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptBlockDef {
    /// Block name (e.g., "identity", "instructions", "tools", "constraints").
    pub name: String,
    /// Assembly order — lower numbers appear first in the final prompt.
    pub order: u32,
    /// Maximum character budget for this block. Content exceeding this is truncated.
    pub max_length: usize,
}

// ── Strategy trait ──────────────────────────────────────────────────────────

/// Defines the structure template for a system prompt.
///
/// A strategy is reusable — multiple `SystemPrompt` instances can share one strategy,
/// each providing different content for the same block structure.
pub trait SystemPromptStrategy: Send + Sync {
    /// Return block definitions (structure only, no content).
    fn block_defs(&self) -> &[PromptBlockDef];
}

// ── Strategy implementations ────────────────────────────────────────────────

/// User-defined strategy with custom block definitions.
pub struct CustomPromptStrategy {
    pub blocks: Vec<PromptBlockDef>,
}

impl SystemPromptStrategy for CustomPromptStrategy {
    fn block_defs(&self) -> &[PromptBlockDef] {
        &self.blocks
    }
}

/// Predefined 4-block layout for general agents.
///
/// | Block | Order | Max Length |
/// |-------|-------|------------|
/// | identity | 0 | 500 |
/// | instructions | 1 | 2000 |
/// | tools | 2 | 1000 |
/// | constraints | 3 | 500 |
pub struct AgentPromptStrategy {
    blocks: Vec<PromptBlockDef>,
}

impl Default for AgentPromptStrategy {
    fn default() -> Self {
        Self {
            blocks: vec![
                PromptBlockDef {
                    name: "identity".into(),
                    order: 0,
                    max_length: 500,
                },
                PromptBlockDef {
                    name: "instructions".into(),
                    order: 1,
                    max_length: 2000,
                },
                PromptBlockDef {
                    name: "tools".into(),
                    order: 2,
                    max_length: 1000,
                },
                PromptBlockDef {
                    name: "constraints".into(),
                    order: 3,
                    max_length: 500,
                },
            ],
        }
    }
}

impl SystemPromptStrategy for AgentPromptStrategy {
    fn block_defs(&self) -> &[PromptBlockDef] {
        &self.blocks
    }
}

/// Predefined 2-block layout for simple agents.
///
/// | Block | Order | Max Length |
/// |-------|-------|------------|
/// | identity | 0 | 1000 |
/// | task | 1 | 3000 |
pub struct MinimalPromptStrategy {
    blocks: Vec<PromptBlockDef>,
}

impl Default for MinimalPromptStrategy {
    fn default() -> Self {
        Self {
            blocks: vec![
                PromptBlockDef {
                    name: "identity".into(),
                    order: 0,
                    max_length: 1000,
                },
                PromptBlockDef {
                    name: "task".into(),
                    order: 1,
                    max_length: 3000,
                },
            ],
        }
    }
}

impl SystemPromptStrategy for MinimalPromptStrategy {
    fn block_defs(&self) -> &[PromptBlockDef] {
        &self.blocks
    }
}

// ── SystemPrompt instance ───────────────────────────────────────────────────

/// A concrete system prompt instance: content mapped to a strategy's blocks.
///
/// Each block value is either:
/// - **Inline text**: `"You are an expert coder."`
/// - **Relative file path**: `"file:prompts/identity.md"` — resolves from agent workspace
/// - **Absolute file path**: `"file:/etc/phi/identity.md"` — resolves as-is
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPrompt {
    /// Unique id (uses `{{...}}` reference protocol in config).
    pub id: String,
    /// Description for existence check queries (with `%` references).
    pub description: Option<String>,
    /// Reference to the strategy that defines this prompt's structure.
    pub strategy_ref: String,
    /// Block name → content mapping.
    pub blocks: HashMap<String, String>,
}

impl SystemPrompt {
    /// Compose the final system prompt text by resolving blocks against the strategy.
    ///
    /// - Sorts blocks by strategy order
    /// - Resolves `"file:path"` references (relative paths use `working_dir`)
    /// - Truncates each block to its `max_length`
    /// - Concatenates with double newlines
    pub fn compose(
        &self,
        strategy: &dyn SystemPromptStrategy,
        working_dir: &Path,
    ) -> Result<String, std::io::Error> {
        let mut defs: Vec<&PromptBlockDef> = strategy.block_defs().iter().collect();
        defs.sort_by_key(|d| d.order);

        let mut parts = Vec::new();
        for def in &defs {
            if let Some(raw) = self.blocks.get(&def.name) {
                let content = resolve_content(raw, working_dir)?;
                parts.push(truncate_to_chars(&content, def.max_length));
            }
        }
        Ok(parts.join("\n\n"))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Resolve block content: if prefixed with "file:", read from disk.
/// Relative paths resolve from `working_dir`.
fn resolve_content(raw: &str, working_dir: &Path) -> Result<String, std::io::Error> {
    if let Some(path_str) = raw.strip_prefix("file:") {
        let path = Path::new(path_str);
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            working_dir.join(path)
        };
        std::fs::read_to_string(full_path)
    } else {
        Ok(raw.to_string())
    }
}

/// Truncate a string to at most `max_chars` characters.
fn truncate_to_chars(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_strategy_block_defs() {
        let strategy = CustomPromptStrategy {
            blocks: vec![
                PromptBlockDef {
                    name: "a".into(),
                    order: 1,
                    max_length: 100,
                },
                PromptBlockDef {
                    name: "b".into(),
                    order: 0,
                    max_length: 200,
                },
            ],
        };
        assert_eq!(strategy.block_defs().len(), 2);
    }

    #[test]
    fn test_agent_prompt_strategy_has_4_blocks() {
        let s = AgentPromptStrategy::default();
        assert_eq!(s.block_defs().len(), 4);
        assert_eq!(s.block_defs()[0].name, "identity");
        assert_eq!(s.block_defs()[3].name, "constraints");
    }

    #[test]
    fn test_minimal_prompt_strategy_has_2_blocks() {
        let s = MinimalPromptStrategy::default();
        assert_eq!(s.block_defs().len(), 2);
        assert_eq!(s.block_defs()[0].name, "identity");
        assert_eq!(s.block_defs()[1].name, "task");
    }

    #[test]
    fn test_compose_orders_by_block_order() {
        let strategy = CustomPromptStrategy {
            blocks: vec![
                PromptBlockDef {
                    name: "second".into(),
                    order: 2,
                    max_length: 1000,
                },
                PromptBlockDef {
                    name: "first".into(),
                    order: 0,
                    max_length: 1000,
                },
                PromptBlockDef {
                    name: "middle".into(),
                    order: 1,
                    max_length: 1000,
                },
            ],
        };
        let mut blocks = HashMap::new();
        blocks.insert("first".into(), "AAA".into());
        blocks.insert("middle".into(), "BBB".into());
        blocks.insert("second".into(), "CCC".into());
        let prompt = SystemPrompt {
            id: "test".into(),
            description: None,
            strategy_ref: "test".into(),
            blocks,
        };
        let result = prompt.compose(&strategy, Path::new(".")).unwrap();
        assert_eq!(result, "AAA\n\nBBB\n\nCCC");
    }

    #[test]
    fn test_compose_truncates() {
        let strategy = CustomPromptStrategy {
            blocks: vec![PromptBlockDef {
                name: "a".into(),
                order: 0,
                max_length: 5,
            }],
        };
        let mut blocks = HashMap::new();
        blocks.insert("a".into(), "hello world".into());
        let prompt = SystemPrompt {
            id: "test".into(),
            description: None,
            strategy_ref: "test".into(),
            blocks,
        };
        let result = prompt.compose(&strategy, Path::new(".")).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_compose_skips_missing_blocks() {
        let strategy = CustomPromptStrategy {
            blocks: vec![
                PromptBlockDef {
                    name: "a".into(),
                    order: 0,
                    max_length: 1000,
                },
                PromptBlockDef {
                    name: "b".into(),
                    order: 1,
                    max_length: 1000,
                },
                PromptBlockDef {
                    name: "c".into(),
                    order: 2,
                    max_length: 1000,
                },
            ],
        };
        let mut blocks = HashMap::new();
        blocks.insert("a".into(), "first".into());
        blocks.insert("c".into(), "third".into());
        // b is missing
        let prompt = SystemPrompt {
            id: "test".into(),
            description: None,
            strategy_ref: "test".into(),
            blocks,
        };
        let result = prompt.compose(&strategy, Path::new(".")).unwrap();
        assert_eq!(result, "first\n\nthird");
    }

    #[test]
    fn test_compose_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("identity.txt");
        std::fs::write(&file_path, "I am a test agent").unwrap();

        let strategy = CustomPromptStrategy {
            blocks: vec![PromptBlockDef {
                name: "identity".into(),
                order: 0,
                max_length: 1000,
            }],
        };
        let mut blocks = HashMap::new();
        blocks.insert("identity".into(), "file:identity.txt".into());
        let prompt = SystemPrompt {
            id: "test".into(),
            description: None,
            strategy_ref: "test".into(),
            blocks,
        };
        let result = prompt.compose(&strategy, dir.path()).unwrap();
        assert_eq!(result, "I am a test agent");
    }

    #[test]
    fn test_compose_file_not_found() {
        let strategy = CustomPromptStrategy {
            blocks: vec![PromptBlockDef {
                name: "a".into(),
                order: 0,
                max_length: 1000,
            }],
        };
        let mut blocks = HashMap::new();
        blocks.insert("a".into(), "file:nonexistent.txt".into());
        let prompt = SystemPrompt {
            id: "test".into(),
            description: None,
            strategy_ref: "test".into(),
            blocks,
        };
        let result = prompt.compose(&strategy, Path::new("."));
        assert!(result.is_err());
    }
}
