//! Tests for the SystemPromptStrategy module: block ordering, truncation, file resolution,
//! and built-in strategy defaults.

use phi_core::agents::system_prompt::*;
use std::collections::HashMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// 1. test_prompt_block_def_ordering
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_block_def_ordering() {
    let strategy = CustomPromptStrategy {
        blocks: vec![
            PromptBlockDef {
                name: "c".into(),
                order: 2,
                max_length: 1000,
            },
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
        ],
    };

    let mut blocks = HashMap::new();
    blocks.insert("a".into(), "ALPHA".into());
    blocks.insert("b".into(), "BRAVO".into());
    blocks.insert("c".into(), "CHARLIE".into());

    let prompt = SystemPrompt {
        id: "test".into(),
        description: None,
        strategy_ref: "test".into(),
        blocks,
    };

    let result = prompt.compose(&strategy, Path::new(".")).unwrap();
    // Blocks should appear in order 0, 1, 2 regardless of definition order.
    assert_eq!(result, "ALPHA\n\nBRAVO\n\nCHARLIE");
}

// ---------------------------------------------------------------------------
// 2. test_prompt_block_max_length_truncation
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_block_max_length_truncation() {
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

// ---------------------------------------------------------------------------
// 3. test_system_prompt_compose_from_strategy
// ---------------------------------------------------------------------------

#[test]
fn test_system_prompt_compose_from_strategy() {
    let strategy = AgentPromptStrategy::default();

    let mut blocks = HashMap::new();
    blocks.insert("identity".into(), "You are Phi.".into());
    blocks.insert("instructions".into(), "Write clean code.".into());

    let prompt = SystemPrompt {
        id: "agent-prompt".into(),
        description: Some("Agent prompt".into()),
        strategy_ref: "agent_layout".into(),
        blocks,
    };

    let result = prompt.compose(&strategy, Path::new(".")).unwrap();
    assert!(result.contains("You are Phi."), "should contain identity");
    assert!(
        result.contains("Write clean code."),
        "should contain instructions"
    );
    // identity (order 0) appears before instructions (order 1)
    let identity_pos = result.find("You are Phi.").unwrap();
    let instructions_pos = result.find("Write clean code.").unwrap();
    assert!(
        identity_pos < instructions_pos,
        "identity should appear before instructions"
    );
}

// ---------------------------------------------------------------------------
// 4. test_system_prompt_missing_block_skipped
// ---------------------------------------------------------------------------

#[test]
fn test_system_prompt_missing_block_skipped() {
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

    // Only provide content for a and c, skip b.
    let mut blocks = HashMap::new();
    blocks.insert("a".into(), "first".into());
    blocks.insert("c".into(), "third".into());

    let prompt = SystemPrompt {
        id: "test".into(),
        description: None,
        strategy_ref: "test".into(),
        blocks,
    };

    let result = prompt.compose(&strategy, Path::new(".")).unwrap();
    assert_eq!(result, "first\n\nthird");
}

// ---------------------------------------------------------------------------
// 5. test_system_prompt_file_resolution_relative
// ---------------------------------------------------------------------------

#[test]
fn test_system_prompt_file_resolution_relative() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("filename.txt");
    std::fs::write(&file_path, "file content").unwrap();

    let strategy = CustomPromptStrategy {
        blocks: vec![PromptBlockDef {
            name: "a".into(),
            order: 0,
            max_length: 1000,
        }],
    };

    let mut blocks = HashMap::new();
    blocks.insert("a".into(), "file:filename.txt".into());

    let prompt = SystemPrompt {
        id: "test".into(),
        description: None,
        strategy_ref: "test".into(),
        blocks,
    };

    let result = prompt.compose(&strategy, dir.path()).unwrap();
    assert_eq!(result, "file content");
}

// ---------------------------------------------------------------------------
// 6. test_system_prompt_file_resolution_absolute
// ---------------------------------------------------------------------------

#[test]
fn test_system_prompt_file_resolution_absolute() {
    let dir = tempfile::tempdir().unwrap();
    let abs_path = dir.path().join("phi_test_prompt_abs.txt");
    std::fs::write(&abs_path, "absolute file content").unwrap();

    let strategy = CustomPromptStrategy {
        blocks: vec![PromptBlockDef {
            name: "a".into(),
            order: 0,
            max_length: 1000,
        }],
    };

    let file_ref = format!("file:{}", abs_path.display());
    let mut blocks = HashMap::new();
    blocks.insert("a".into(), file_ref);

    let prompt = SystemPrompt {
        id: "test".into(),
        description: None,
        strategy_ref: "test".into(),
        blocks,
    };

    let result = prompt.compose(&strategy, Path::new(".")).unwrap();
    assert_eq!(result, "absolute file content");
    // Cleanup handled by tempdir drop
}

// ---------------------------------------------------------------------------
// 7. test_system_prompt_file_not_found_error
// ---------------------------------------------------------------------------

#[test]
fn test_system_prompt_file_not_found_error() {
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
    assert!(result.is_err(), "should error on missing file");
}

// ---------------------------------------------------------------------------
// 8. test_agent_prompt_strategy_builtin
// ---------------------------------------------------------------------------

#[test]
fn test_agent_prompt_strategy_builtin() {
    let s = AgentPromptStrategy::default();
    let defs = s.block_defs();
    assert_eq!(defs.len(), 4);
    assert_eq!(defs[0].name, "identity");
    assert_eq!(defs[0].order, 0);
    assert_eq!(defs[1].name, "instructions");
    assert_eq!(defs[1].order, 1);
    assert_eq!(defs[2].name, "tools");
    assert_eq!(defs[2].order, 2);
    assert_eq!(defs[3].name, "constraints");
    assert_eq!(defs[3].order, 3);
}

// ---------------------------------------------------------------------------
// 9. test_minimal_prompt_strategy_builtin
// ---------------------------------------------------------------------------

#[test]
fn test_minimal_prompt_strategy_builtin() {
    let s = MinimalPromptStrategy::default();
    let defs = s.block_defs();
    assert_eq!(defs.len(), 2);
    assert_eq!(defs[0].name, "identity");
    assert_eq!(defs[0].order, 0);
    assert_eq!(defs[1].name, "task");
    assert_eq!(defs[1].order, 1);
}
