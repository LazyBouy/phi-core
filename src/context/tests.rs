use super::compact_messages::truncate_text_head_tail;
use super::*;
use crate::types::*;

#[test]
fn test_estimate_tokens() {
    assert!(estimate_tokens("hello world") > 0);
    assert!(estimate_tokens("hello world") < 10);
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_truncate_head_tail() {
    let text = (1..=100)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    let result = truncate_text_head_tail(&text, 10);
    assert!(result.contains("line 1"));
    assert!(result.contains("line 5")); // head
    assert!(result.contains("line 100")); // tail
    assert!(result.contains("truncated"));
    assert!(!result.contains("line 50")); // middle removed
}

#[test]
fn test_level1_truncation() {
    let big_output = (1..=200)
        .map(|i| format!("output line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    let messages = vec![
        AgentMessage::Llm(LlmMessage::new(Message::user("do something"))),
        AgentMessage::Llm(LlmMessage::new(Message::ToolResult {
            tool_call_id: "tc-1".into(),
            tool_name: "bash".into(),
            content: vec![Content::Text { text: big_output }],
            is_error: false,
            timestamp: 0,
        })),
    ];

    let compacted = compact_messages::level1_truncate_tool_outputs(&messages, 20);
    let tool_msg = &compacted[1];
    if let AgentMessage::Llm(LlmMessage {
        message: Message::ToolResult { content, .. },
        ..
    }) = tool_msg
    {
        if let Content::Text { text } = &content[0] {
            assert!(text.contains("truncated"));
            assert!(text.contains("output line 1")); // head
            assert!(text.contains("output line 200")); // tail
            assert!(text.lines().count() < 50);
        } else {
            panic!("expected text content");
        }
    } else {
        panic!("expected tool result");
    }
}

#[test]
fn test_compact_within_budget() {
    let messages = vec![
        AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))),
        AgentMessage::Llm(LlmMessage::new(Message::user("World"))),
    ];
    let config = ContextConfig::default();
    let result = compact_messages(messages.clone(), &config);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_compact_drops_middle_when_needed() {
    let mut messages = Vec::new();
    for i in 0..100 {
        messages.push(AgentMessage::Llm(LlmMessage::new(Message::user(format!(
            "Message {} {}",
            i,
            "x".repeat(200)
        )))));
    }

    let config = ContextConfig {
        max_context_tokens: 500,
        system_prompt_tokens: 100,
        compaction: CompactionConfig::default(),
        token_counter: None,
        keep_recent: 5,
        keep_first: 2,
        tool_output_max_lines: 20,
    };

    let result = compact_messages(messages, &config);
    assert!(result.len() < 100);
    assert!(result.len() >= 2);
}

#[test]
fn test_context_tracker_no_usage() {
    let tracker = ContextTracker::new();
    let messages = vec![
        AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))),
        AgentMessage::Llm(LlmMessage::new(Message::user("World"))),
    ];
    let tokens = tracker.estimate_context_tokens(&messages);
    assert!(tokens > 0);
    assert_eq!(tokens, total_tokens(&messages));
}

#[test]
fn test_context_tracker_with_usage() {
    let mut tracker = ContextTracker::new();
    let messages = vec![
        AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))),
        AgentMessage::Llm(LlmMessage::new(Message::Assistant {
            content: vec![Content::Text {
                text: "Hi there!".into(),
            }],
            stop_reason: StopReason::Stop,
            model: "test".into(),
            provider: "test".into(),
            usage: Usage {
                input: 100,
                output: 50,
                ..Default::default()
            },
            timestamp: 0,
            error_message: None,
        })),
        AgentMessage::Llm(LlmMessage::new(Message::user("Follow up question here"))),
    ];
    tracker.record_usage(
        &Usage {
            input: 100,
            output: 50,
            ..Default::default()
        },
        1,
    );
    let tokens = tracker.estimate_context_tokens(&messages);
    let trailing_estimate = token::message_tokens(&messages[2]);
    assert_eq!(tokens, 150 + trailing_estimate);
}

#[test]
fn test_context_tracker_reset() {
    let mut tracker = ContextTracker::new();
    tracker.record_usage(
        &Usage {
            input: 1000,
            output: 500,
            ..Default::default()
        },
        5,
    );
    tracker.reset();
    let messages = vec![AgentMessage::Llm(LlmMessage::new(Message::user("test")))];
    assert_eq!(
        tracker.estimate_context_tokens(&messages),
        total_tokens(&messages)
    );
}

#[test]
fn test_execution_limits() {
    let limits = ExecutionLimits {
        max_turns: 3,
        max_total_tokens: 1000,
        max_duration: std::time::Duration::from_secs(60),
        max_cost: None,
    };

    let mut tracker = ExecutionTracker::new(limits);
    assert!(tracker.check_limits().is_none());

    tracker.record_turn(100);
    tracker.record_turn(100);
    assert!(tracker.check_limits().is_none());

    tracker.record_turn(100);
    assert!(tracker.check_limits().is_some());
}
