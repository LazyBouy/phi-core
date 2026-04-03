//! Tests for PrunTool, InRunEntry, and build_working_context.

use phi_core::tools::prun::{PrunTool, PrunVariant};
use phi_core::types::*;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create an AgentMessage::Llm(User) with a specific timestamp.
fn user_msg(text: &str, ts: u64) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::new(Message::User {
        content: vec![Content::Text {
            text: text.to_string(),
        }],
        timestamp: ts,
    }))
}

/// Create an AgentMessage::Llm(Assistant) with a specific timestamp.
fn assistant_msg(text: &str, ts: u64) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::new(Message::Assistant {
        content: vec![Content::Text {
            text: text.to_string(),
        }],
        stop_reason: StopReason::Stop,
        model: "test".into(),
        provider: "test".into(),
        usage: Usage::default(),
        timestamp: ts,
        error_message: None,
    }))
}

// ---------------------------------------------------------------------------
// 1. test_inrun_entry_live_in_working_context
// ---------------------------------------------------------------------------

#[test]
fn test_inrun_entry_live_in_working_context() {
    let u = user_msg("hello", 100);
    let a = assistant_msg("world", 200);

    let ctx = AgentContext {
        user_context: vec![u.clone()],
        inrun_context: vec![InRunEntry::Live(a.clone())],
        messages: vec![u.clone(), a.clone()],
        ..Default::default()
    };

    let wc = ctx.build_working_context();
    assert_eq!(wc.len(), 2);
    assert_eq!(wc[0].timestamp(), 100);
    assert_eq!(wc[1].timestamp(), 200);
}

// ---------------------------------------------------------------------------
// 2. test_pruned_silent_excluded_from_working_context
// ---------------------------------------------------------------------------

#[test]
fn test_pruned_silent_excluded_from_working_context() {
    let u = user_msg("hello", 100);

    let ctx = AgentContext {
        user_context: vec![u.clone()],
        inrun_context: vec![InRunEntry::PrunedSilent {
            tokens_removed: 50,
            timestamp: 200,
        }],
        messages: vec![u.clone()],
        ..Default::default()
    };

    let wc = ctx.build_working_context();
    // Only the user message should appear — pruned silent is excluded
    assert_eq!(wc.len(), 1);
    assert_eq!(wc[0].timestamp(), 100);
}

// ---------------------------------------------------------------------------
// 3. test_pruned_memo_appears_in_working_context
// ---------------------------------------------------------------------------

#[test]
fn test_pruned_memo_appears_in_working_context() {
    let u = user_msg("hello", 100);

    let ctx = AgentContext {
        user_context: vec![u.clone()],
        inrun_context: vec![InRunEntry::PrunedMemo {
            memo: "summary of pruned content".into(),
            tokens_removed: 50,
            timestamp: 200,
        }],
        messages: vec![u.clone()],
        ..Default::default()
    };

    let wc = ctx.build_working_context();
    // User msg + injected memo summary message
    assert_eq!(wc.len(), 2);
    assert_eq!(wc[0].timestamp(), 100);
    // The memo message should contain the memo text
    let last = &wc[1];
    assert_eq!(last.role(), "user");
    if let AgentMessage::Llm(lm) = last {
        if let Message::User { content, .. } = &lm.message {
            let text = match &content[0] {
                Content::Text { text } => text.as_str(),
                _ => panic!("expected Text content"),
            };
            assert!(
                text.contains("summary of pruned content"),
                "memo text not found in working context message"
            );
        } else {
            panic!("expected User message for memo");
        }
    } else {
        panic!("expected Llm message for memo");
    }
}

// ---------------------------------------------------------------------------
// 4. test_working_context_preserves_timestamp_order
// ---------------------------------------------------------------------------

#[test]
fn test_working_context_preserves_timestamp_order() {
    let u1 = user_msg("first", 100);
    let a1 = assistant_msg("reply", 200);
    let u2 = user_msg("steering", 300);
    let a2 = assistant_msg("second reply", 400);

    let ctx = AgentContext {
        user_context: vec![u1.clone(), u2.clone()],
        inrun_context: vec![InRunEntry::Live(a1.clone()), InRunEntry::Live(a2.clone())],
        messages: vec![u1, a1, u2, a2],
        ..Default::default()
    };

    let wc = ctx.build_working_context();
    assert_eq!(wc.len(), 4);
    let timestamps: Vec<u64> = wc.iter().map(|m| m.timestamp()).collect();
    assert_eq!(timestamps, vec![100, 200, 300, 400]);
}

// ---------------------------------------------------------------------------
// 5. test_working_context_fallback_to_messages
// ---------------------------------------------------------------------------

#[test]
fn test_working_context_fallback_to_messages() {
    let u = user_msg("hello", 100);
    let a = assistant_msg("world", 200);

    let ctx = AgentContext {
        user_context: vec![],
        inrun_context: vec![],
        messages: vec![u.clone(), a.clone()],
        ..Default::default()
    };

    let wc = ctx.build_working_context();
    // Falls back to messages when both streams are empty
    assert_eq!(wc.len(), 2);
    assert_eq!(wc[0].timestamp(), 100);
    assert_eq!(wc[1].timestamp(), 200);
}

// ---------------------------------------------------------------------------
// 6. test_user_context_never_prunable
// ---------------------------------------------------------------------------

#[test]
fn test_user_context_never_prunable() {
    let u1 = user_msg("important prompt", 100);
    let a1 = assistant_msg("reply", 200);
    let u2 = user_msg("follow-up", 300);

    // Simulate a prun that converted the assistant to PrunedSilent
    let ctx = AgentContext {
        user_context: vec![u1.clone(), u2.clone()],
        inrun_context: vec![InRunEntry::PrunedSilent {
            tokens_removed: 50,
            timestamp: 200,
        }],
        messages: vec![u1, a1, u2],
        ..Default::default()
    };

    let wc = ctx.build_working_context();
    // Both user messages should survive; only the assistant was pruned
    assert!(wc.len() >= 2);
    let timestamps: Vec<u64> = wc.iter().map(|m| m.timestamp()).collect();
    assert!(timestamps.contains(&100), "user_context msg ts=100 missing");
    assert!(timestamps.contains(&300), "user_context msg ts=300 missing");
    // The pruned assistant at ts=200 should NOT appear
    assert!(
        !timestamps.contains(&200),
        "pruned assistant should not appear"
    );
}

// ---------------------------------------------------------------------------
// 7. test_prun_tool_schema
// ---------------------------------------------------------------------------

#[test]
fn test_prun_tool_schema() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = PrunTool::new(pending, PrunVariant::Prun);

    assert_eq!(tool.name(), "prun");

    let schema = tool.parameters_schema();
    let required = schema["required"]
        .as_array()
        .expect("required should be array");
    let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        required_strs.contains(&"tokens"),
        "prun schema should require 'tokens'"
    );
    // Prun variant should NOT require "memo"
    assert!(
        !required_strs.contains(&"memo"),
        "prun schema should not require 'memo'"
    );
}

// ---------------------------------------------------------------------------
// 8. test_prun_with_memo_tool_schema
// ---------------------------------------------------------------------------

#[test]
fn test_prun_with_memo_tool_schema() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = PrunTool::new(pending, PrunVariant::PrunWithMemo);

    assert_eq!(tool.name(), "prun_with_memo");

    let schema = tool.parameters_schema();
    let required = schema["required"]
        .as_array()
        .expect("required should be array");
    let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        required_strs.contains(&"tokens"),
        "prun_with_memo schema should require 'tokens'"
    );
    assert!(
        required_strs.contains(&"memo"),
        "prun_with_memo schema should require 'memo'"
    );
}
