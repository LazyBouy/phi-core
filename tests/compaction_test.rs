//! Tests for the non-destructive compaction overlay system.

use chrono::Utc;
use phi_core::context::*;
use phi_core::session::{LoopRecord, LoopStatus, Session, SessionFormation};
use phi_core::*;

// ---------------------------------------------------------------------------
// Helper: create a simple LoopRecord with N turns of user+assistant pairs
// ---------------------------------------------------------------------------
fn make_loop_record(loop_id: &str, num_turns: u32, parent: Option<&str>) -> LoopRecord {
    let mut messages = Vec::new();
    for t in 0..num_turns {
        let tid = Some(TurnId {
            loop_id: loop_id.to_string(),
            turn_index: t,
        });
        messages.push(
            AgentMessage::from(Message::user(format!("Turn {} question", t)))
                .with_turn_id(tid.clone()),
        );
        messages.push(
            AgentMessage::from(Message::Assistant {
                content: vec![Content::Text {
                    text: format!("Turn {} answer with some content that is meaningful", t),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            })
            .with_turn_id(tid.clone()),
        );
    }
    LoopRecord {
        loop_id: loop_id.to_string(),
        session_id: "test-session".to_string(),
        agent_id: "test-agent".to_string(),
        parent_loop_id: parent.map(|s| s.to_string()),
        continuation_kind: None,
        started_at: Utc::now(),
        ended_at: Some(Utc::now()),
        status: LoopStatus::Completed,
        rejection: None,
        config: None,
        messages,
        usage: Usage::default(),
        metadata: None,
        events: Vec::new(),
        children_loop_ids: Vec::new(),
        child_loop_refs: Vec::new(),
        parallel_group: None,
        compaction_block: None,
    }
}

fn make_session(loops: Vec<LoopRecord>) -> Session {
    let now = Utc::now();
    Session {
        session_id: "test-session".to_string(),
        agent_id: "test-agent".to_string(),
        created_at: now,
        last_active_at: now,
        formation: SessionFormation::Explicit { timestamp: now },
        parent_spawn_ref: None,
        loops,
    }
}

// ---------------------------------------------------------------------------
// TurnMap tests
// ---------------------------------------------------------------------------

#[test]
fn test_turn_map_from_messages_groups_by_turn_id() {
    let loop_id = "test.model.1";
    let mut messages = Vec::new();
    // Turn 0: user + assistant + tool_result (3 messages)
    for _ in 0..3 {
        messages.push(
            AgentMessage::from(Message::user("hi")).with_turn_id(Some(TurnId {
                loop_id: loop_id.into(),
                turn_index: 0,
            })),
        );
    }
    // Turn 1: user + assistant (2 messages)
    for _ in 0..2 {
        messages.push(
            AgentMessage::from(Message::user("hello")).with_turn_id(Some(TurnId {
                loop_id: loop_id.into(),
                turn_index: 1,
            })),
        );
    }

    let tm = TurnMap::from_messages(&messages);
    assert_eq!(tm.turn_count(), 2);

    let range0 = TurnRange {
        start_turn: 0,
        end_turn: 0,
    };
    assert_eq!(tm.messages_for_range(&range0, &messages).len(), 3);

    let range1 = TurnRange {
        start_turn: 1,
        end_turn: 1,
    };
    assert_eq!(tm.messages_for_range(&range1, &messages).len(), 2);

    let full_range = TurnRange {
        start_turn: 0,
        end_turn: 1,
    };
    assert_eq!(tm.messages_for_range(&full_range, &messages).len(), 5);
}

#[test]
fn test_turn_map_legacy_messages_without_turn_id() {
    let messages = vec![
        AgentMessage::from(Message::user("no turn id 1")),
        AgentMessage::from(Message::user("no turn id 2")),
    ];
    let tm = TurnMap::from_messages(&messages);
    // Each legacy message is its own group
    assert_eq!(tm.turn_count(), 2);
}

// ---------------------------------------------------------------------------
// CompactionBlock creation tests
// ---------------------------------------------------------------------------

#[test]
fn test_compaction_block_creation_most_recent() {
    let record = make_loop_record("test.model.1", 10, None);
    let config = CompactionConfig::default(); // keep_first=2, keep_recent=10, max_summary=2000

    let block = DefaultBlockCompaction.compact(&record, &config, true);

    // With 10 turns and keep_first=2, keep_recent=10, there's no middle
    // (2 + 10 > 10), so keep_compacted should be None
    assert!(block.keep_first.is_some());
    assert!(block.keep_compacted.is_none()); // No room for middle
}

#[test]
fn test_compaction_block_creation_with_middle() {
    let record = make_loop_record("test.model.1", 20, None);
    let config = CompactionConfig {
        keep_first_turns: 2,
        keep_recent_turns: 5,
        max_summary_tokens: 2_000,
        ..CompactionConfig::default()
    };

    let block = DefaultBlockCompaction.compact(&record, &config, true);

    assert!(block.keep_first.is_some());
    let kf = block.keep_first.unwrap();
    assert_eq!(kf.start_turn, 0);
    assert_eq!(kf.end_turn, 1); // turns 0-1

    assert!(block.keep_compacted.is_some());
    let kc = block.keep_compacted.unwrap();
    assert_eq!(kc.range.start_turn, 2);
    assert_eq!(kc.range.end_turn, 14); // turns 2-14

    assert!(block.keep_recent.is_some());
    let kr = block.keep_recent.unwrap();
    assert_eq!(kr.range.start_turn, 15);
    assert_eq!(kr.range.end_turn, 19); // turns 15-19
}

#[test]
fn test_compaction_block_creation_earlier_loop() {
    let record = make_loop_record("test.model.1", 10, None);
    let config = CompactionConfig::default();

    let block = DefaultBlockCompaction.compact(&record, &config, false);

    // Earlier loops: only keep_compacted, no keep_first or keep_recent
    assert!(block.keep_first.is_none());
    assert!(block.keep_recent.is_none());
    assert!(block.keep_compacted.is_some());
}

// ---------------------------------------------------------------------------
// build_context_from_session tests
// ---------------------------------------------------------------------------

#[test]
fn test_build_context_falls_back_to_raw() {
    let record = make_loop_record("test.model.1", 3, None);
    let session = make_session(vec![record]);
    let config = CompactionConfig::default();

    let context = build_context_from_session(&session, "test.model.1", &config, 100_000);

    // No compaction block → raw messages loaded (3 turns × 2 msgs = 6)
    assert_eq!(context.len(), 6);
}

#[test]
fn test_build_context_from_session_with_blocks() {
    let mut record = make_loop_record("test.model.1", 20, None);
    let config = CompactionConfig {
        keep_first_turns: 2,
        keep_recent_turns: 5,
        max_summary_tokens: 2_000,
        ..CompactionConfig::default()
    };

    // Create a compaction block
    record.compaction_block = Some(DefaultBlockCompaction.compact(&record, &config, true));

    let session = make_session(vec![record]);
    let context = build_context_from_session(&session, "test.model.1", &config, 100_000);

    // Should have: keep_first messages + keep_compacted summaries + keep_recent messages
    // Much fewer than the original 40 messages (20 turns × 2)
    assert!(context.len() < 40);
    assert!(!context.is_empty());
}

// ---------------------------------------------------------------------------
// compact_session_loops tests
// ---------------------------------------------------------------------------

#[test]
fn test_compact_session_loops_writes_earlier() {
    let loop1 = make_loop_record("test.model.1", 5, None);
    let loop2 = make_loop_record("test.model.2", 5, Some("test.model.1"));
    let loop3 = make_loop_record("test.model.3", 10, Some("test.model.2"));
    let mut session = make_session(vec![loop1, loop2, loop3]);

    let config = CompactionConfig {
        compaction_scope: CompactionScope::FixedCount(2),
        keep_first_turns: 1,
        keep_recent_turns: 3,
        max_summary_tokens: 2_000,
        ..CompactionConfig::default()
    };

    compact_session_loops(
        &mut session,
        "test.model.3",
        &DefaultBlockCompaction,
        &config,
        100_000,
    );

    // Current loop should have a block
    assert!(session
        .get_loop("test.model.3")
        .unwrap()
        .compaction_block
        .is_some());
    // Earlier loop should also have a block (compact_earlier_loops = 2)
    assert!(session
        .get_loop("test.model.2")
        .unwrap()
        .compaction_block
        .is_some());
    assert!(session
        .get_loop("test.model.1")
        .unwrap()
        .compaction_block
        .is_some());
}

// ---------------------------------------------------------------------------
// Percentage threshold test
// ---------------------------------------------------------------------------

#[test]
fn test_pct_threshold_calculation() {
    let config = CompactionConfig::default(); // 90%, 5%
    let max_tokens = 100_000usize;
    let system_tokens = 4_000usize;

    // At 80k tokens: headroom = 0.90 - 0.04 - 0.80 = 0.06 > 0.05 → no compaction
    let system_frac = system_tokens as f64 / max_tokens as f64;
    let current_frac = 80_000f64 / max_tokens as f64;
    let headroom = config.compact_at_pct - system_frac - current_frac;
    assert!(headroom >= config.compact_budget_threshold_pct);

    // At 82k tokens: headroom = 0.90 - 0.04 - 0.82 = 0.04 < 0.05 → compaction fires
    let current_frac = 82_000f64 / max_tokens as f64;
    let headroom = config.compact_at_pct - system_frac - current_frac;
    assert!(headroom < config.compact_budget_threshold_pct);
}

// ---------------------------------------------------------------------------
// Serialization round-trip test
// ---------------------------------------------------------------------------

#[test]
fn test_compaction_block_serialization_roundtrip() {
    let block = CompactionBlock {
        keep_first: Some(TurnRange {
            start_turn: 0,
            end_turn: 1,
        }),
        keep_compacted: Some(CompactedSection {
            range: TurnRange {
                start_turn: 2,
                end_turn: 5,
            },
            messages: vec![AgentMessage::from(Message::user("[Summary] test"))],
        }),
        keep_recent: None,
        created_at: Utc::now(),
    };

    let json = serde_json::to_string(&block).unwrap();
    let deserialized: CompactionBlock = serde_json::from_str(&json).unwrap();

    assert_eq!(block.keep_first, deserialized.keep_first);
    assert!(deserialized.keep_compacted.is_some());
    assert!(deserialized.keep_recent.is_none());
}

#[test]
fn test_turn_id_serialization_roundtrip() {
    let msg = AgentMessage::from(Message::user("hello")).with_turn_id(Some(TurnId {
        loop_id: "test.model.1".into(),
        turn_index: 3,
    }));

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("turnId"));
    assert!(json.contains("turnIndex"));

    let deserialized: AgentMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.turn_id().unwrap().turn_index, 3);
}

#[test]
fn test_turn_id_backward_compat_deserialization() {
    // Old format: no turnId field
    let json = r#"{"role":"user","content":[{"type":"text","text":"hello"}],"timestamp":0}"#;
    let msg: AgentMessage = serde_json::from_str(json).unwrap();
    assert!(msg.turn_id().is_none());
    assert_eq!(msg.role(), "user");
}

// ---------------------------------------------------------------------------
// CompactionScope::TokenBudget tests
// ---------------------------------------------------------------------------

#[test]
fn test_compaction_scope_token_budget_partial() {
    // Create 3 loops: loop1 has ~50 tokens, loop2 has ~50, loop3 (current) has ~50
    // With max_context_tokens = 80, only loop2 should be in scope (loop1 exceeds budget)
    let loop1 = make_loop_record("test.model.1", 5, None); // ~5 turns × 2 msgs
    let loop2 = make_loop_record("test.model.2", 3, Some("test.model.1"));
    let loop3 = make_loop_record("test.model.3", 3, Some("test.model.2"));
    let mut session = make_session(vec![loop1, loop2, loop3]);

    let config = CompactionConfig {
        compaction_scope: CompactionScope::TokenBudget,
        keep_first_turns: 1,
        keep_recent_turns: 2,
        max_summary_tokens: 2_000,
        ..CompactionConfig::default()
    };

    // Use a small token budget so not all loops fit
    let small_budget =
        phi_core::context::total_tokens(&session.get_loop("test.model.2").unwrap().messages) + 10; // Just enough for loop2's messages + a little buffer

    compact_session_loops(
        &mut session,
        "test.model.3",
        &DefaultBlockCompaction,
        &config,
        small_budget,
    );

    // Current loop (3) always gets a block
    assert!(session
        .get_loop("test.model.3")
        .unwrap()
        .compaction_block
        .is_some());
    // Loop2 should be in scope (fits in budget)
    assert!(session
        .get_loop("test.model.2")
        .unwrap()
        .compaction_block
        .is_some());
    // Loop1 should NOT be compacted (exceeds remaining budget)
    assert!(session
        .get_loop("test.model.1")
        .unwrap()
        .compaction_block
        .is_none());
}

#[test]
fn test_compaction_scope_token_budget_all_fit() {
    let loop1 = make_loop_record("test.model.1", 2, None);
    let loop2 = make_loop_record("test.model.2", 2, Some("test.model.1"));
    let mut session = make_session(vec![loop1, loop2]);

    let config = CompactionConfig {
        compaction_scope: CompactionScope::TokenBudget,
        ..CompactionConfig::default()
    };

    // Large budget — all loops fit
    compact_session_loops(
        &mut session,
        "test.model.2",
        &DefaultBlockCompaction,
        &config,
        1_000_000,
    );

    assert!(session
        .get_loop("test.model.2")
        .unwrap()
        .compaction_block
        .is_some());
    assert!(session
        .get_loop("test.model.1")
        .unwrap()
        .compaction_block
        .is_some());
}

#[test]
fn test_build_context_token_budget_scope() {
    let mut loop1 = make_loop_record("test.model.1", 5, None);
    let mut loop2 = make_loop_record("test.model.2", 5, Some("test.model.1"));
    let mut loop3 = make_loop_record("test.model.3", 5, Some("test.model.2"));

    let config = CompactionConfig {
        compaction_scope: CompactionScope::TokenBudget,
        keep_first_turns: 1,
        keep_recent_turns: 2,
        max_summary_tokens: 2_000,
        ..CompactionConfig::default()
    };

    // Create blocks on all loops
    loop1.compaction_block = Some(DefaultBlockCompaction.compact(&loop1, &config, false));
    loop2.compaction_block = Some(DefaultBlockCompaction.compact(&loop2, &config, false));
    loop3.compaction_block = Some(DefaultBlockCompaction.compact(&loop3, &config, true));

    let session = make_session(vec![loop1, loop2, loop3]);

    // With a small budget, only recent loops should be loaded
    let small_budget = 200;
    let context = build_context_from_session(&session, "test.model.3", &config, small_budget);

    // With a large budget, all loops should contribute
    let large_budget = 1_000_000;
    let context_large = build_context_from_session(&session, "test.model.3", &config, large_budget);

    // Large budget should load more messages than small budget
    assert!(context_large.len() >= context.len());
}
