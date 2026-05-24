//! Tests for SessionRecorder, Session navigation, persistence, and BasicAgent session management.

use phi_core::agent_loop::evaluation::PickFirstEvaluation;
use phi_core::agent_loop::{agent_loop, agent_loop_continue, agent_loop_parallel, AgentLoopConfig};
use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::session::*;
use phi_core::{LlmMessage, *};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_config(provider: Arc<dyn phi_core::provider::StreamProvider>) -> AgentLoopConfig {
    AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock", "test"),
        provider_override: Some(provider),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: None,
        transform_context: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        context_config: None,
        execution_limits: None,
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        tool_timeout: None,
        response_format: phi_core::provider::ResponseFormat::Text,
        retry_config: phi_core::RetryConfig::default(),
        before_turn: None,
        after_turn: None,
        on_error: None,
        before_loop: None,
        after_loop: None,
        before_tool_execution: None,
        after_tool_execution: None,
        before_tool_execution_update: None,
        after_tool_execution_update: None,
        before_compaction_start: None,
        after_compaction_end: None,
        input_filters: vec![],
        first_turn_trigger: TurnTrigger::User,
        config_id: None,
        context_translation: None,
        prun_pending: None,
        revert_pending: None,
    }
}

fn drain(mut rx: mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

fn feed(recorder: &mut SessionRecorder, events: Vec<AgentEvent>) {
    for e in events {
        recorder.on_event(e);
    }
}

// ---------------------------------------------------------------------------
// test_session_recorder_single_loop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_recorder_single_loop() {
    let provider = Arc::new(MockProvider::text("Hello!"));
    let config = make_config(provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext {
        system_prompt: "You are helpful.".into(),
        agent_id: Some("agent-1".into()),
        session_id: Some("session-1".into()),
        ..Default::default()
    };

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("Hello")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.session_id, "session-1");
    assert_eq!(session.agent_id, "agent-1");
    assert_eq!(session.loops.len(), 1);

    let lr = &session.loops[0];
    assert_eq!(lr.status, LoopStatus::Completed);
    assert!(lr.rejection.is_none());
    assert!(!lr.messages.is_empty());
    // Usage should be non-zero (MockProvider uses non-zero synthetic tokens).
    // The message should be an assistant message.
    assert!(lr.messages.iter().any(|m| matches!(
        m,
        AgentMessage::Llm(LlmMessage {
            message: Message::Assistant { .. },
            ..
        })
    )));
}

// ---------------------------------------------------------------------------
// test_session_recorder_continuation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_recorder_continuation() {
    let provider = Arc::new(MockProvider::texts(vec!["First", "Second"]));
    let config = make_config(provider);

    let (tx1, rx1) = mpsc::unbounded_channel();
    let cancel1 = CancellationToken::new();
    let mut context = AgentContext {
        system_prompt: "You are helpful.".into(),
        agent_id: Some("agent-1".into()),
        session_id: Some("session-cont".into()),
        ..Default::default()
    };

    // First loop.
    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user(
            "First question",
        )))],
        &mut context,
        &config,
        tx1,
        cancel1,
    )
    .await;
    let events1 = drain(rx1);

    // Set up continuation with a parent_loop_id.
    let parent_lid = context.loop_id.clone().unwrap();
    context.loop_id = Some(format!(
        "{}.test.2",
        context.session_id.as_deref().unwrap_or("")
    ));
    context.parent_loop_id = Some(parent_lid.clone());
    context.continuation_kind = Some(ContinuationKind::Default);
    context
        .messages
        .push(AgentMessage::Llm(LlmMessage::new(Message::user(
            "Second question",
        ))));

    let (tx2, rx2) = mpsc::unbounded_channel();
    let cancel2 = CancellationToken::new();
    agent_loop_continue(&mut context, &config, tx2, cancel2).await;
    let events2 = drain(rx2);

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events1);
    feed(&mut recorder, events2);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.loops.len(), 2);

    // Find parent and child loops.
    let parent = session
        .get_loop(&parent_lid)
        .expect("parent loop not found");
    assert!(parent.children_loop_ids.len() == 1);

    let child_lid = &parent.children_loop_ids[0];
    let child = session.get_loop(child_lid).expect("child loop not found");
    assert_eq!(child.parent_loop_id.as_deref(), Some(parent_lid.as_str()));
    assert_eq!(child.continuation_kind, ContinuationKind::Default);

    // Tree navigation.
    let roots: Vec<_> = session.root_loops().collect();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].loop_id, parent_lid);

    let children: Vec<_> = session.children_of(&parent_lid).collect();
    assert_eq!(children.len(), 1);
}

// ---------------------------------------------------------------------------
// test_session_recorder_parallel_group
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_recorder_parallel_group() {
    let provider_a = Arc::new(MockProvider::text("Branch A response"));
    let provider_b = Arc::new(MockProvider::text("Branch B response BB"));

    let config_a = AgentLoopConfig {
        provider_override: Some(provider_a),
        ..make_config(Arc::new(MockProvider::text("")))
    };
    let config_b = AgentLoopConfig {
        provider_override: Some(provider_b),
        ..make_config(Arc::new(MockProvider::text("")))
    };

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let base_ctx = AgentContext {
        system_prompt: "Be concise.".into(),
        agent_id: Some("agent-par".into()),
        session_id: Some("session-par".into()),
        ..Default::default()
    };

    agent_loop_parallel(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user(
            "Compare A vs B",
        )))],
        base_ctx.clone(),
        vec![config_a, config_b],
        Arc::new(PickFirstEvaluation),
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.loops.len(), 2);

    // Both branches should have a parallel_group set.
    for lr in &session.loops {
        let pg = lr
            .parallel_group
            .as_ref()
            .expect("parallel_group should be set");
        assert_eq!(pg.all_loop_ids.len(), 2);
    }

    // Exactly one branch should be selected.
    let selected: Vec<_> = session
        .loops
        .iter()
        .filter(|l| {
            l.parallel_group
                .as_ref()
                .map(|pg| pg.is_selected)
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(selected.len(), 1);

    // parallel_siblings returns both branches.
    let winner_id = &selected[0].loop_id;
    let siblings: Vec<_> = session.parallel_siblings(winner_id).collect();
    assert_eq!(siblings.len(), 2);
}

// ---------------------------------------------------------------------------
// test_session_recorder_streaming_events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_recorder_streaming_events() {
    let provider = Arc::new(MockProvider::text("Stream test"));
    let config = make_config(provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext::default();

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user(
            "Stream me",
        )))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);

    // Without streaming events.
    let mut recorder_no_stream = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder_no_stream, events.clone());
    recorder_no_stream.flush();

    // With streaming events.
    let mut recorder_with_stream = SessionRecorder::new(SessionRecorderConfig {
        include_streaming_events: true,
        capture_turn_requests: false,
        before_task: None,
        after_task: None,
    });
    feed(&mut recorder_with_stream, events.clone());
    recorder_with_stream.flush();

    let no_stream_sessions: Vec<_> = recorder_no_stream.sessions().collect();
    let with_stream_sessions: Vec<_> = recorder_with_stream.sessions().collect();

    let lr_no = &no_stream_sessions[0].loops[0];
    let lr_with = &with_stream_sessions[0].loops[0];

    let updates_no = lr_no
        .events
        .iter()
        .filter(|e| matches!(e.event, AgentEvent::MessageUpdate { .. }))
        .count();
    let updates_with = lr_with
        .events
        .iter()
        .filter(|e| matches!(e.event, AgentEvent::MessageUpdate { .. }))
        .count();

    assert_eq!(
        updates_no, 0,
        "no streaming: MessageUpdate events should be absent"
    );
    // MockProvider emits text deltas, so there should be at least one MessageUpdate with streaming on.
    assert!(
        updates_with > 0,
        "with streaming: expected at least one MessageUpdate event"
    );
}

// ---------------------------------------------------------------------------
// test_session_save_load_roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_save_load_roundtrip() {
    let provider = Arc::new(MockProvider::text("Roundtrip!"));
    let config = make_config(provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext {
        agent_id: Some("agent-rt".into()),
        session_id: Some("session-rt".into()),
        ..Default::default()
    };

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("save me")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events);
    recorder.flush();
    let mut sessions = recorder.drain_completed();
    assert_eq!(sessions.len(), 1);
    let original = sessions.remove(0);

    let dir = TempDir::new().unwrap();
    let path = save_session(&original, dir.path()).unwrap();
    assert!(path.exists());

    let loaded = load_session("session-rt", dir.path()).unwrap();
    assert_eq!(loaded.session_id, original.session_id);
    assert_eq!(loaded.agent_id, original.agent_id);
    assert_eq!(loaded.loops.len(), original.loops.len());
    assert_eq!(loaded.loops[0].status, LoopStatus::Completed);
}

// ---------------------------------------------------------------------------
// test_session_list_ids
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_list_ids() {
    let dir = TempDir::new().unwrap();

    // Save s0 first, then s1 with a 10ms gap so filesystem mtimes differ.
    // list_session_ids is documented to return newest-first (by mtime).
    let make_session = |id: &str| Session {
        session_id: id.to_string(),
        agent_id: "agent".into(),
        created_at: chrono::Utc::now(),
        last_active_at: chrono::Utc::now(),
        formation: SessionFormation::Explicit {
            timestamp: chrono::Utc::now(),
        },
        parent_spawn_ref: None,
        scope: SessionScope::Ephemeral,
        loops: Vec::new(),
    };

    save_session(&make_session("s0"), dir.path()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    save_session(&make_session("s1"), dir.path()).unwrap();

    let ids = list_session_ids(dir.path()).unwrap();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"s0".to_string()));
    assert!(ids.contains(&"s1".to_string()));
    // s1 was written last — it must appear first (newest-first ordering).
    assert_eq!(ids[0], "s1", "newest session (s1) should be first");
    assert_eq!(ids[1], "s0", "oldest session (s0) should be last");
}

// ---------------------------------------------------------------------------
// test_session_delete
// ---------------------------------------------------------------------------

#[test]
fn test_session_delete() {
    let dir = TempDir::new().unwrap();
    let session = Session {
        session_id: "del-session".into(),
        agent_id: "agent".into(),
        created_at: chrono::Utc::now(),
        last_active_at: chrono::Utc::now(),
        formation: SessionFormation::Explicit {
            timestamp: chrono::Utc::now(),
        },
        parent_spawn_ref: None,
        scope: SessionScope::Ephemeral,
        loops: Vec::new(),
    };
    save_session(&session, dir.path()).unwrap();
    assert!(list_session_ids(dir.path())
        .unwrap()
        .contains(&"del-session".to_string()));

    delete_session("del-session", dir.path()).unwrap();
    assert!(!list_session_ids(dir.path())
        .unwrap()
        .contains(&"del-session".to_string()));

    // Deleting non-existent session returns NotFound.
    let err = delete_session("ghost", dir.path());
    assert!(matches!(err, Err(SessionError::NotFound { .. })));
}

// ---------------------------------------------------------------------------
// test_basic_agent_new_session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_basic_agent_new_session() {
    let provider = Arc::new(MockProvider::text("hi"));
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(provider);

    let original_session_id = agent.session_id().to_string();

    let new_id = agent.new_session();
    assert_ne!(
        new_id, original_session_id,
        "new_session should rotate to a different id"
    );
    assert_eq!(agent.session_id(), new_id.as_str());
}

// ---------------------------------------------------------------------------
// test_basic_agent_check_and_rotate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_basic_agent_check_and_rotate() {
    let provider = Arc::new(MockProvider::text("hi"));
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(provider);

    // Before any prompt, no last_active_at → no rotation.
    let result = agent.check_and_rotate(std::time::Duration::from_secs(1));
    assert!(result.is_none(), "no rotation when never prompted");

    // Run one prompt to set last_active_at.
    let (tx, rx) = mpsc::unbounded_channel();
    agent.prompt_with_sender("hello", tx).await;
    drop(rx);

    // Threshold of 100 seconds should NOT trigger rotation (just ran).
    let result = agent.check_and_rotate(std::time::Duration::from_secs(100));
    assert!(result.is_none(), "should not rotate within threshold");

    // Sleep 1ms to guarantee the clock advances before testing elapsed > threshold.
    std::thread::sleep(std::time::Duration::from_millis(1));

    // Zero-duration threshold SHOULD trigger rotation (elapsed > 0 after the sleep).
    let old_id = agent.session_id().to_string();
    let result = agent.check_and_rotate(std::time::Duration::ZERO);
    assert!(
        result.is_some(),
        "zero-duration threshold should trigger rotation"
    );
    assert_ne!(
        agent.session_id(),
        old_id.as_str(),
        "session_id should have changed"
    );

    // After rotation, last_active_at is cleared — a second check_and_rotate must
    // return None (the new session has never been used, so there is nothing to measure).
    let result = agent.check_and_rotate(std::time::Duration::ZERO);
    assert!(
        result.is_none(),
        "second check_and_rotate after rotation should return None (new session never used)"
    );
}

// ---------------------------------------------------------------------------
// test_session_recorder_bidirectional_tree
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_recorder_bidirectional_tree() {
    // Three loops: parent → child1, parent → child2 (two independent continuations).
    let provider = Arc::new(MockProvider::texts(vec!["P", "C1", "C2"]));

    // Loop 1 (origin).
    let config = make_config(provider.clone());
    let (tx1, rx1) = mpsc::unbounded_channel();
    let mut ctx = AgentContext {
        agent_id: Some("agent-tree".into()),
        session_id: Some("session-tree".into()),
        ..Default::default()
    };
    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("parent")))],
        &mut ctx,
        &config,
        tx1,
        CancellationToken::new(),
    )
    .await;
    let parent_lid = ctx.loop_id.clone().unwrap();
    let events1 = drain(rx1);

    // Loop 2: continuation from parent.
    let config2 = make_config(provider.clone());
    let (tx2, rx2) = mpsc::unbounded_channel();
    let mut ctx2 = ctx.clone();
    ctx2.loop_id = Some(format!(
        "{}.test.2",
        ctx2.session_id.as_deref().unwrap_or("")
    ));
    ctx2.parent_loop_id = Some(parent_lid.clone());
    ctx2.continuation_kind = Some(ContinuationKind::Default);
    ctx2.messages
        .push(AgentMessage::Llm(LlmMessage::new(Message::user("child1"))));
    agent_loop_continue(&mut ctx2, &config2, tx2, CancellationToken::new()).await;
    let child1_lid = ctx2.loop_id.clone().unwrap();
    let events2 = drain(rx2);

    // Loop 3: another continuation from same parent.
    let config3 = make_config(provider);
    let (tx3, rx3) = mpsc::unbounded_channel();
    let mut ctx3 = ctx.clone();
    ctx3.loop_id = Some(format!(
        "{}.test.3",
        ctx3.session_id.as_deref().unwrap_or("")
    ));
    ctx3.parent_loop_id = Some(parent_lid.clone());
    ctx3.continuation_kind = Some(ContinuationKind::Default);
    ctx3.messages
        .push(AgentMessage::Llm(LlmMessage::new(Message::user("child2"))));
    agent_loop_continue(&mut ctx3, &config3, tx3, CancellationToken::new()).await;
    let child2_lid = ctx3.loop_id.clone().unwrap();
    let events3 = drain(rx3);

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events1);
    feed(&mut recorder, events2);
    feed(&mut recorder, events3);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.loops.len(), 3);

    let parent = session.get_loop(&parent_lid).unwrap();
    assert!(parent.children_loop_ids.contains(&child1_lid));
    assert!(parent.children_loop_ids.contains(&child2_lid));

    let c1 = session.get_loop(&child1_lid).unwrap();
    assert_eq!(c1.parent_loop_id.as_deref(), Some(parent_lid.as_str()));

    let c2 = session.get_loop(&child2_lid).unwrap();
    assert_eq!(c2.parent_loop_id.as_deref(), Some(parent_lid.as_str()));

    // children_of returns both children.
    let children: Vec<_> = session.children_of(&parent_lid).collect();
    assert_eq!(children.len(), 2);
}

// ---------------------------------------------------------------------------
// test_session_recorder_continuation_kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_recorder_continuation_kind() {
    let provider = Arc::new(MockProvider::texts(vec!["A", "B"]));
    let config = make_config(provider.clone());

    let (tx1, rx1) = mpsc::unbounded_channel();
    let mut ctx = AgentContext {
        agent_id: Some("agent-ck".into()),
        session_id: Some("session-ck".into()),
        ..Default::default()
    };
    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("origin")))],
        &mut ctx,
        &config,
        tx1,
        CancellationToken::new(),
    )
    .await;
    let parent_lid = ctx.loop_id.clone().unwrap();
    let events1 = drain(rx1);

    // Rerun continuation.
    let config2 = make_config(provider);
    let (tx2, rx2) = mpsc::unbounded_channel();
    let mut ctx2 = ctx.clone();
    ctx2.loop_id = Some(format!(
        "{}.test.2",
        ctx2.session_id.as_deref().unwrap_or("")
    ));
    ctx2.parent_loop_id = Some(parent_lid.clone());
    ctx2.continuation_kind = Some(ContinuationKind::Rerun { tag: "test".into() });
    ctx2.messages
        .push(AgentMessage::Llm(LlmMessage::new(Message::user("rerun"))));
    agent_loop_continue(&mut ctx2, &config2, tx2, CancellationToken::new()).await;
    let rerun_lid = ctx2.loop_id.clone().unwrap();
    let events2 = drain(rx2);

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events1);
    feed(&mut recorder, events2);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    let session = &sessions[0];
    let rerun = session.get_loop(&rerun_lid).unwrap();
    assert!(matches!(
        rerun.continuation_kind,
        ContinuationKind::Rerun { .. }
    ));
    assert!(session
        .get_loop(&parent_lid)
        .unwrap()
        .parent_loop_id
        .is_none());
}

// ---------------------------------------------------------------------------
// test_session_recorder_child_loop_ref
// ---------------------------------------------------------------------------

/// Verify cross-session sub-agent tracking:
/// - Parent LoopRecord.child_loop_refs has one entry pointing to the child loop
/// - Child Session.parent_spawn_ref points back to the parent loop
/// - Parent LoopRecord.children_loop_ids does NOT contain the child loop_id
///   (cross-session children must NOT appear in the same-session tree)
#[test]
fn test_session_recorder_child_loop_ref() {
    let parent_session_id = "sess-parent";
    let parent_loop_id = format!("{}.mock.1", parent_session_id);
    let child_session_id = "sess-child";
    let child_loop_id = format!("{}.mock.1", child_session_id);

    let now = chrono::Utc::now();

    // Realistic interleaving:
    //   1. parent AgentStart
    //   2. parent ToolExecutionStart (sub-agent tool call begins)
    //   3. child AgentStart (child loop starts inside the tool)
    //   4. child AgentEnd  (child loop finishes)
    //   5. parent ToolExecutionEnd (tool returns, child_loop_id set)
    //   6. parent AgentEnd
    let parent_start = AgentEvent::AgentStart {
        agent_id: "parent-agent".into(),
        session_id: parent_session_id.into(),
        loop_id: parent_loop_id.clone(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    };
    let tool_start = AgentEvent::ToolExecutionStart {
        loop_id: parent_loop_id.clone(),
        tool_call_id: "tc-1".into(),
        tool_name: "sub_agent".into(),
        args: serde_json::json!({"task": "do work"}),
    };
    let child_start = AgentEvent::AgentStart {
        agent_id: "child-agent".into(),
        session_id: child_session_id.into(),
        loop_id: child_loop_id.clone(),
        parent_loop_id: Some(parent_loop_id.clone()),
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    };
    let child_end = AgentEvent::AgentEnd {
        loop_id: child_loop_id.clone(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    };
    let tool_end = AgentEvent::ToolExecutionEnd {
        loop_id: parent_loop_id.clone(),
        tool_call_id: "tc-1".into(),
        tool_name: "sub_agent".into(),
        result: ToolResult {
            content: vec![Content::Text {
                text: "done".into(),
            }],
            details: serde_json::Value::Null,
            child_loop_id: Some(child_loop_id.clone()),
        },
        is_error: false,
        child_loop_id: Some(child_loop_id.clone()),
    };
    let parent_end = AgentEvent::AgentEnd {
        loop_id: parent_loop_id.clone(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    };

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    for event in [
        parent_start,
        tool_start,
        child_start,
        child_end,
        tool_end,
        parent_end,
    ] {
        recorder.on_event(event);
    }
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 2, "expected 2 sessions (parent + child)");

    let parent_sess = recorder
        .get_session(parent_session_id)
        .expect("parent session not found");
    let child_sess = recorder
        .get_session(child_session_id)
        .expect("child session not found");

    // 1. Parent LoopRecord has the ChildLoopRef.
    let parent_loop = parent_sess
        .get_loop(&parent_loop_id)
        .expect("parent loop not found");
    assert_eq!(
        parent_loop.child_loop_refs.len(),
        1,
        "expected one ChildLoopRef on the parent loop"
    );
    let clr = &parent_loop.child_loop_refs[0];
    assert_eq!(clr.tool_call_id, "tc-1");
    assert_eq!(clr.tool_name, "sub_agent");
    assert_eq!(clr.child_loop_id, child_loop_id);
    assert_eq!(clr.child_session_id, child_session_id);

    // 2. Parent's children_loop_ids must NOT contain the cross-session child.
    assert!(
        !parent_loop.children_loop_ids.contains(&child_loop_id),
        "children_loop_ids must not contain a cross-session child"
    );

    // 3. Child session has parent_spawn_ref pointing back to the parent.
    let sr = child_sess
        .parent_spawn_ref
        .as_ref()
        .expect("child session should have parent_spawn_ref");
    assert_eq!(sr.parent_session_id, parent_session_id);
    assert_eq!(sr.parent_loop_id, parent_loop_id);
    assert_eq!(sr.tool_call_id, "tc-1");
    assert_eq!(sr.tool_name, "sub_agent");
}

// ---------------------------------------------------------------------------
// test_session_recorder_child_loop_ref_tool_end_before_child_end
// ---------------------------------------------------------------------------

/// Same cross-session scenario as above but with a reversed event ordering:
/// tool_end fires before child_end (can happen when events from two channels
/// are interleaved). Verifies ChildLoopRef and parent_spawn_ref are still
/// correctly wired.
#[test]
fn test_session_recorder_child_loop_ref_tool_end_before_child_end() {
    let parent_session_id = "sess-parent-rev";
    let parent_loop_id = format!("{}.mock.1", parent_session_id);
    let child_session_id = "sess-child-rev";
    let child_loop_id = format!("{}.mock.1", child_session_id);
    let now = chrono::Utc::now();

    let parent_start = AgentEvent::AgentStart {
        agent_id: "parent-agent".into(),
        session_id: parent_session_id.into(),
        loop_id: parent_loop_id.clone(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    };
    let child_start = AgentEvent::AgentStart {
        agent_id: "child-agent".into(),
        session_id: child_session_id.into(),
        loop_id: child_loop_id.clone(),
        parent_loop_id: Some(parent_loop_id.clone()),
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    };
    // tool_end arrives BEFORE child_end (reversed ordering).
    let tool_end = AgentEvent::ToolExecutionEnd {
        loop_id: parent_loop_id.clone(),
        tool_call_id: "tc-rev".into(),
        tool_name: "sub_agent".into(),
        result: ToolResult {
            content: vec![],
            details: serde_json::Value::Null,
            child_loop_id: Some(child_loop_id.clone()),
        },
        is_error: false,
        child_loop_id: Some(child_loop_id.clone()),
    };
    let child_end = AgentEvent::AgentEnd {
        loop_id: child_loop_id.clone(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    };
    let parent_end = AgentEvent::AgentEnd {
        loop_id: parent_loop_id.clone(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    };

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    for event in [parent_start, child_start, tool_end, child_end, parent_end] {
        recorder.on_event(event);
    }
    recorder.flush();

    let parent_sess = recorder
        .get_session(parent_session_id)
        .expect("parent session not found");
    let parent_loop = parent_sess
        .get_loop(&parent_loop_id)
        .expect("parent loop not found");

    // ChildLoopRef must be recorded regardless of ordering.
    assert_eq!(parent_loop.child_loop_refs.len(), 1);
    assert_eq!(parent_loop.child_loop_refs[0].tool_call_id, "tc-rev");
    assert_eq!(
        parent_loop.child_loop_refs[0].child_session_id,
        child_session_id
    );

    // Cross-session child must not appear in same-session tree.
    assert!(!parent_loop.children_loop_ids.contains(&child_loop_id));

    // Child session's parent_spawn_ref should be enriched (child was still open
    // in open_sessions when tool_end fired).
    let child_sess = recorder
        .get_session(child_session_id)
        .expect("child session not found");
    let sr = child_sess
        .parent_spawn_ref
        .as_ref()
        .expect("child session should have parent_spawn_ref");
    assert_eq!(sr.tool_call_id, "tc-rev");
    assert_eq!(sr.tool_name, "sub_agent");
}

// ---------------------------------------------------------------------------
// test_session_recorder_spawn_ref_enrichment_after_flush
// ---------------------------------------------------------------------------

/// REQ-221 regression: SpawnRef must be enriched even when the child session
/// has already been promoted to `completed` by a checkpoint() call.
///
/// Sequence:
///   1. parent AgentStart
///   2. child  AgentStart   (parent_loop_id = parent loop)
///   3. child  AgentEnd     (child loop closes; child session has no open loops)
///   4. recorder.checkpoint()  → child session promoted to completed;
///      parent session remains open (parent loop still live)
///   5. parent ToolExecutionEnd (result.child_loop_id = child loop)
///   6. parent AgentEnd
///   7. recorder.flush()   → parent session promoted to completed
///
/// Without the fix, enrichment in step 5 would find the child only in
/// `completed` and silently skip it → tool_call_id stays "".
/// With the fix the fallback search through `completed` enriches it correctly.
#[test]
fn test_session_recorder_spawn_ref_enrichment_after_flush() {
    let parent_session_id = "sess-parent-221";
    let parent_loop_id = format!("{}.mock.1", parent_session_id);
    let child_session_id = "sess-child-221";
    let child_loop_id = format!("{}.mock.1", child_session_id);
    let now = chrono::Utc::now();

    let parent_start = AgentEvent::AgentStart {
        agent_id: "parent-agent-221".into(),
        session_id: parent_session_id.into(),
        loop_id: parent_loop_id.clone(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    };
    let child_start = AgentEvent::AgentStart {
        agent_id: "child-agent-221".into(),
        session_id: child_session_id.into(),
        loop_id: child_loop_id.clone(),
        parent_loop_id: Some(parent_loop_id.clone()),
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    };
    let child_end = AgentEvent::AgentEnd {
        loop_id: child_loop_id.clone(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    };
    let tool_end = AgentEvent::ToolExecutionEnd {
        loop_id: parent_loop_id.clone(),
        tool_call_id: "tc-221".into(),
        tool_name: "sub_agent_221".into(),
        result: ToolResult {
            content: vec![],
            details: serde_json::Value::Null,
            child_loop_id: Some(child_loop_id.clone()),
        },
        is_error: false,
        child_loop_id: Some(child_loop_id.clone()),
    };
    let parent_end = AgentEvent::AgentEnd {
        loop_id: parent_loop_id.clone(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    };

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());

    // Steps 1–3: feed parent start, child start, child end.
    recorder.on_event(parent_start);
    recorder.on_event(child_start);
    recorder.on_event(child_end);

    // Step 4: checkpoint promotes child session (no open loops) to completed,
    // leaving the parent session (still has an open loop) untouched.
    let promoted = recorder.checkpoint();
    assert_eq!(
        promoted, 1,
        "exactly one session (child) should have been promoted"
    );

    // Step 5: parent processes ToolExecutionEnd — child is now in `completed`,
    // not in `open_sessions`. Without the fix this enrichment is skipped.
    recorder.on_event(tool_end);

    // Step 6-7: close parent loop and flush.
    recorder.on_event(parent_end);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 2, "expected 2 sessions (parent + child)");

    let parent_sess = recorder
        .get_session(parent_session_id)
        .expect("parent session not found");
    let child_sess = recorder
        .get_session(child_session_id)
        .expect("child session not found");

    // Parent loop must have a ChildLoopRef pointing to the child.
    let parent_loop = parent_sess
        .get_loop(&parent_loop_id)
        .expect("parent loop not found");
    assert_eq!(parent_loop.child_loop_refs.len(), 1);
    assert_eq!(parent_loop.child_loop_refs[0].tool_call_id, "tc-221");
    assert_eq!(
        parent_loop.child_loop_refs[0].child_session_id,
        child_session_id
    );

    // Child session's SpawnRef must be fully enriched — the key assertion for REQ-221.
    let sr = child_sess
        .parent_spawn_ref
        .as_ref()
        .expect("child session should have parent_spawn_ref");
    assert_eq!(sr.parent_session_id, parent_session_id);
    assert_eq!(sr.parent_loop_id, parent_loop_id);
    assert_eq!(
        sr.tool_call_id, "tc-221",
        "SpawnRef must be enriched even after checkpoint() moved child to completed"
    );
    assert_eq!(sr.tool_name, "sub_agent_221");
}

// ---------------------------------------------------------------------------
// test_session_recorder_flush_aborts_open_loop
// ---------------------------------------------------------------------------

/// flush() called while a loop is still open must mark it Aborted.
#[tokio::test]
async fn test_session_recorder_flush_aborts_open_loop() {
    let provider = Arc::new(MockProvider::text("hi"));
    let config = make_config(provider);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext {
        agent_id: Some("agent-flush".into()),
        session_id: Some("session-flush".into()),
        ..Default::default()
    };

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("test")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    // Drain all events but deliberately drop AgentEnd so the loop stays open.
    let events: Vec<_> = {
        let mut v = Vec::new();
        while let Ok(e) = rx.try_recv() {
            v.push(e);
        }
        v
    };
    let events_without_end: Vec<_> = events
        .into_iter()
        .filter(|e| !matches!(e, AgentEvent::AgentEnd { .. }))
        .collect();

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events_without_end);
    // Loop is still open — flush must abort it.
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].loops.len(), 1);
    assert_eq!(
        sessions[0].loops[0].status,
        LoopStatus::Aborted,
        "loop missing AgentEnd should be marked Aborted by flush()"
    );
}

// ---------------------------------------------------------------------------
// test_session_recorder_current_loop
// ---------------------------------------------------------------------------

/// current_loop() returns the in-progress LoopRecord while the loop is open,
/// and None after it closes.
#[tokio::test]
async fn test_session_recorder_current_loop() {
    let provider = Arc::new(MockProvider::text("hi"));
    let config = make_config(provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext {
        agent_id: Some("agent-cl".into()),
        session_id: Some("session-cl".into()),
        ..Default::default()
    };

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("test")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);
    let loop_id = context.loop_id.clone().unwrap();

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());

    // Feed all events up to (but not including) AgentEnd.
    let (before_end, rest): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| !matches!(e, AgentEvent::AgentEnd { .. }));

    feed(&mut recorder, before_end);
    assert!(
        recorder.current_loop(&loop_id).is_some(),
        "current_loop should be Some while loop is open"
    );

    // Feed the AgentEnd.
    feed(&mut recorder, rest);
    assert!(
        recorder.current_loop(&loop_id).is_none(),
        "current_loop should be None after AgentEnd"
    );
}

// ---------------------------------------------------------------------------
// test_load_sessions_for_agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_load_sessions_for_agent() {
    let dir = TempDir::new().unwrap();

    // Save two sessions for "agent-a" and one for "agent-b".
    let sessions_a: Vec<Session> = (0..2)
        .map(|i| Session {
            session_id: format!("agent-a-sess-{i}"),
            agent_id: "agent-a".into(),
            created_at: chrono::Utc::now(),
            last_active_at: chrono::Utc::now(),
            formation: SessionFormation::Explicit {
                timestamp: chrono::Utc::now(),
            },
            parent_spawn_ref: None,
            scope: SessionScope::Ephemeral,
            loops: Vec::new(),
        })
        .collect();
    let session_b = Session {
        session_id: "agent-b-sess-0".into(),
        agent_id: "agent-b".into(),
        created_at: chrono::Utc::now(),
        last_active_at: chrono::Utc::now(),
        formation: SessionFormation::Explicit {
            timestamp: chrono::Utc::now(),
        },
        parent_spawn_ref: None,
        scope: SessionScope::Ephemeral,
        loops: Vec::new(),
    };

    for s in &sessions_a {
        save_session(s, dir.path()).unwrap();
    }
    save_session(&session_b, dir.path()).unwrap();

    let loaded = load_sessions_for_agent("agent-a", dir.path()).unwrap();
    assert_eq!(loaded.len(), 2, "should load exactly agent-a's 2 sessions");
    assert!(loaded.iter().all(|s| s.agent_id == "agent-a"));

    let loaded_b = load_sessions_for_agent("agent-b", dir.path()).unwrap();
    assert_eq!(loaded_b.len(), 1);

    let loaded_none = load_sessions_for_agent("agent-z", dir.path()).unwrap();
    assert!(loaded_none.is_empty(), "unknown agent returns empty vec");
}

// ---------------------------------------------------------------------------
// test_malformed_loop_id_handling
// ---------------------------------------------------------------------------

/// Verifies the documented fallback behaviour of loop_id helpers when loop_ids
/// don't follow the `{session_id}.{config_segment}.{N}` format.
///
/// - `session_id_from_loop_id`: returns the whole string if there is no dot.
/// - `config_segment_from_loop_id`: returns None if there are fewer than two dots.
///
/// These are private functions so we exercise them through the public
/// SessionRecorder API by feeding events with abnormal loop_id values and
/// asserting the derived fields on the resulting records.
#[test]
fn test_malformed_loop_id_handling() {
    let now = chrono::Utc::now();

    // ── Case 1: loop_id with no dots ─────────────────────────────────────────
    // session_id_from_loop_id("nodots") == "nodots" (whole string)
    // config_segment_from_loop_id("nodots") == None
    {
        let loop_id = "nodots".to_string();
        let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
        recorder.on_event(AgentEvent::AgentStart {
            agent_id: "a".into(),
            session_id: "nodots".into(), // must match what session_id_from_loop_id returns
            loop_id: loop_id.clone(),
            parent_loop_id: None,
            continuation_kind: ContinuationKind::Initial,
            timestamp: now,
            metadata: None,
            config_snapshot: None,
        });
        recorder.on_event(AgentEvent::AgentEnd {
            loop_id: loop_id.clone(),
            messages: vec![],
            usage: Usage::default(),
            timestamp: now,
            rejection: None,
        });
        recorder.flush();

        let sess = recorder.get_session("nodots").expect("session not created");
        assert_eq!(sess.loops.len(), 1);
        // config_id should be None — no two dots means no config segment
        assert!(
            sess.loops[0].config.is_none(),
            "no assistant message → config is None"
        );
    }

    // ── Case 2: loop_id with exactly one dot ─────────────────────────────────
    // session_id_from_loop_id("sess.1") == "sess"
    // config_segment_from_loop_id("sess.1") == None (only one dot)
    {
        let loop_id = "sess.1".to_string();
        let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
        recorder.on_event(AgentEvent::AgentStart {
            agent_id: "a".into(),
            session_id: "sess".into(),
            loop_id: loop_id.clone(),
            parent_loop_id: None,
            continuation_kind: ContinuationKind::Initial,
            timestamp: now,
            metadata: None,
            config_snapshot: None,
        });
        recorder.on_event(AgentEvent::AgentEnd {
            loop_id: loop_id.clone(),
            messages: vec![],
            usage: Usage::default(),
            timestamp: now,
            rejection: None,
        });
        recorder.flush();

        let sess = recorder.get_session("sess").expect("session not created");
        assert_eq!(sess.loops.len(), 1);
        assert_eq!(sess.loops[0].loop_id, loop_id);
    }

    // ── Case 3: cross-session child with a no-dot loop_id ────────────────────
    // When ToolExecutionEnd carries child_loop_id="child-nodots",
    // session_id_from_loop_id returns "child-nodots" as the child_session_id.
    {
        let parent_loop_id = "parent.cfg.1".to_string();
        let child_loop_id = "child-nodots".to_string();
        let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
        recorder.on_event(AgentEvent::AgentStart {
            agent_id: "a".into(),
            session_id: "parent".into(),
            loop_id: parent_loop_id.clone(),
            parent_loop_id: None,
            continuation_kind: ContinuationKind::Initial,
            timestamp: now,
            metadata: None,
            config_snapshot: None,
        });
        recorder.on_event(AgentEvent::ToolExecutionEnd {
            loop_id: parent_loop_id.clone(),
            tool_call_id: "tc".into(),
            tool_name: "tool".into(),
            result: ToolResult {
                content: vec![],
                details: serde_json::Value::Null,
                child_loop_id: Some(child_loop_id.clone()),
            },
            is_error: false,
            child_loop_id: Some(child_loop_id.clone()),
        });
        recorder.on_event(AgentEvent::AgentEnd {
            loop_id: parent_loop_id.clone(),
            messages: vec![],
            usage: Usage::default(),
            timestamp: now,
            rejection: None,
        });
        recorder.flush();

        let sess = recorder.get_session("parent").expect("session not found");
        let lr = sess.get_loop(&parent_loop_id).unwrap();
        assert_eq!(lr.child_loop_refs.len(), 1);
        // Fallback: whole string becomes the child_session_id
        assert_eq!(
            lr.child_loop_refs[0].child_session_id, "child-nodots",
            "no-dot loop_id should use whole string as child_session_id"
        );
    }
}

// ===========================================================================
// Turn materialization tests
// ===========================================================================

/// Single-turn loop: recorder produces exactly one Turn with correct fields.
#[tokio::test]
async fn test_turn_materialization_single_turn() {
    let provider = Arc::new(MockProvider::text("Hello!"));
    let config = make_config(provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext {
        system_prompt: "You are helpful.".into(),
        agent_id: Some("agent-turn".into()),
        session_id: Some("session-turn".into()),
        ..Default::default()
    };

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("Hi")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    let lr = &sessions[0].loops[0];

    // Should have exactly one turn.
    assert_eq!(lr.turn_count(), 1);

    let turn = lr.get_turn(0).expect("turn 0 should exist");
    assert_eq!(turn.index(), 0);
    assert!(matches!(turn.triggered_by, TurnTrigger::User));
    assert!(!turn.has_tool_calls());

    // input_messages should contain the user message.
    assert!(
        !turn.input_messages.is_empty(),
        "expected at least one input message (user prompt)"
    );
    assert_eq!(turn.input_messages[0].role(), "user");

    // output_message should be an assistant message.
    assert_eq!(turn.output_message.role(), "assistant");

    // Timestamps should be reasonable.
    assert!(turn.ended_at >= turn.started_at);
    assert!(turn.duration() >= chrono::Duration::zero());
}

/// Multi-turn loop with tool calls: recorder produces multiple turns.
#[tokio::test]
async fn test_turn_materialization_multi_turn() {
    use phi_core::provider::mock::{MockResponse, MockToolCall};

    // Define a simple tool inline.
    struct TestTool;

    #[async_trait::async_trait]
    impl phi_core::AgentTool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }
        fn label(&self) -> &str {
            "Test Tool"
        }
        fn description(&self) -> &str {
            "A test tool"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: phi_core::ToolContext,
        ) -> Result<phi_core::ToolResult, phi_core::ToolError> {
            Ok(phi_core::ToolResult {
                content: vec![Content::Text { text: "ok".into() }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    // MockProvider with tool call then text response → 2 turns.
    let provider = Arc::new(MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "test_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("Done!".into()),
    ]));
    let config = make_config(provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext {
        system_prompt: "You are helpful.".into(),
        agent_id: Some("agent-mt".into()),
        session_id: Some("session-mt".into()),
        tools: vec![Arc::new(TestTool)],
        ..Default::default()
    };

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user(
            "Do something",
        )))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    let lr = &sessions[0].loops[0];

    // Should have 2 turns: tool call turn + final text turn.
    assert_eq!(lr.turn_count(), 2);

    // Turn 0: triggered by User, has tool calls.
    let t0 = lr.get_turn(0).unwrap();
    assert_eq!(t0.index(), 0);
    assert!(matches!(t0.triggered_by, TurnTrigger::User));
    assert!(t0.has_tool_calls());
    assert!(!t0.tool_results.is_empty());

    // Turn 1: triggered by Continuation (tool round-trip), no tool calls.
    let t1 = lr.get_turn(1).unwrap();
    assert_eq!(t1.index(), 1);
    assert!(matches!(t1.triggered_by, TurnTrigger::Continuation));
    assert!(!t1.has_tool_calls());

    // all_messages covers everything.
    let all = t0.all_messages();
    assert!(all.len() >= 2); // at least input + output
}

/// Turn serde roundtrip.
#[test]
fn test_turn_serde_roundtrip() {
    use phi_core::Turn;

    let turn = Turn {
        turn_id: TurnId {
            loop_id: "loop-1".into(),
            turn_index: 0,
        },
        triggered_by: TurnTrigger::User,
        usage: Usage::default(),
        input_messages: vec![AgentMessage::from(Message::user("Hi"))],
        output_message: AgentMessage::from(Message::Assistant {
            content: vec![Content::Text {
                text: "Hello".into(),
            }],
            stop_reason: StopReason::Stop,
            model: "test".into(),
            provider: "test".into(),
            usage: Usage::default(),
            timestamp: 0,
            error_message: None,
        }),
        tool_results: vec![],
        started_at: chrono::Utc::now(),
        ended_at: chrono::Utc::now(),
        request_payload: None,
    };

    let json = serde_json::to_string(&turn).expect("serialize");
    let roundtripped: Turn = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(roundtripped.turn_id.loop_id, "loop-1");
    assert_eq!(roundtripped.turn_id.turn_index, 0);
    assert_eq!(roundtripped.output_message.role(), "assistant");
}

/// LoopRecord without `turns` field deserializes with empty turns (backward compat).
#[test]
fn test_loop_record_backward_compat_no_turns() {
    // Minimal LoopRecord JSON without the `turns` field.
    let json = r#"{
        "loop_id": "test-loop",
        "session_id": "test-session",
        "agent_id": "test-agent",
        "parent_loop_id": null,
        "continuation_kind": null,
        "started_at": "2025-01-01T00:00:00Z",
        "ended_at": null,
        "status": "Running",
        "rejection": null,
        "config": null,
        "messages": [],
        "usage": {"input": 0, "output": 0, "reasoning": 0, "cache_read": 0, "cache_write": 0, "total_tokens": 0},
        "metadata": null,
        "events": [],
        "children_loop_ids": [],
        "child_loop_refs": [],
        "parallel_group": null
    }"#;

    let lr: phi_core::LoopRecord = serde_json::from_str(json).expect("deserialize");
    assert_eq!(lr.loop_id, "test-loop");
    assert!(lr.turns.is_empty(), "turns should default to empty vec");
    assert_eq!(lr.turn_count(), 0);
    assert!(lr.get_turn(0).is_none());
}

/// Tool results on Turn carry the correct turn_id (consistency with LoopRecord.messages).
#[tokio::test]
async fn test_turn_tool_results_carry_turn_id() {
    use phi_core::provider::mock::{MockResponse, MockToolCall};

    struct EchoTool;

    #[async_trait::async_trait]
    impl phi_core::AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn label(&self) -> &str {
            "Echo"
        }
        fn description(&self) -> &str {
            "echo"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: phi_core::ToolContext,
        ) -> Result<phi_core::ToolResult, phi_core::ToolError> {
            Ok(phi_core::ToolResult {
                content: vec![Content::Text {
                    text: "echoed".into(),
                }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let provider = Arc::new(MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "echo".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("Done".into()),
    ]));
    let config = make_config(provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut context = AgentContext {
        system_prompt: "test".into(),
        agent_id: Some("a".into()),
        session_id: Some("s".into()),
        tools: vec![Arc::new(EchoTool)],
        ..Default::default()
    };

    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("go")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    let events = drain(rx);
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    feed(&mut recorder, events);
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    let lr = &sessions[0].loops[0];
    let t0 = lr.get_turn(0).expect("turn 0");

    // Tool results should carry the same turn_id as the turn itself.
    assert!(t0.has_tool_calls());
    for tr in &t0.tool_results {
        let tid = tr.turn_id().expect("tool result should have turn_id");
        assert_eq!(tid.loop_id, t0.turn_id.loop_id);
        assert_eq!(tid.turn_index, t0.turn_id.turn_index);
    }

    // output_message should also carry the turn_id.
    let out_tid = t0
        .output_message
        .turn_id()
        .expect("output_message should have turn_id");
    assert_eq!(out_tid.turn_index, 0);
}

/// Recorder handles aborted loop gracefully — partial turns are discarded on flush.
#[test]
fn test_turn_aborted_loop_partial_turn_discarded() {
    let now = chrono::Utc::now();
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());

    // Simulate AgentStart → TurnStart → (no TurnEnd) → flush.
    recorder.on_event(AgentEvent::AgentStart {
        agent_id: "a".into(),
        session_id: "s".into(),
        loop_id: "loop-abort".into(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    });
    recorder.on_event(AgentEvent::TurnStart {
        loop_id: "loop-abort".into(),
        turn_index: 0,
        timestamp: now,
        triggered_by: TurnTrigger::User,
    });
    // No TurnEnd or AgentEnd — simulate crash/abort.
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    assert_eq!(sessions.len(), 1);
    let lr = &sessions[0].loops[0];
    assert_eq!(lr.status, LoopStatus::Aborted);
    // Turns should be empty — the partial turn was discarded.
    assert_eq!(lr.turn_count(), 0);
}

/// Recorder handles AgentEnd arriving while a partial turn is open.
#[test]
fn test_turn_agent_end_cleans_orphaned_partial_turn() {
    let now = chrono::Utc::now();
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());

    // AgentStart → TurnStart → AgentEnd (without TurnEnd).
    recorder.on_event(AgentEvent::AgentStart {
        agent_id: "a".into(),
        session_id: "s".into(),
        loop_id: "loop-orphan".into(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    });
    recorder.on_event(AgentEvent::TurnStart {
        loop_id: "loop-orphan".into(),
        turn_index: 0,
        timestamp: now,
        triggered_by: TurnTrigger::User,
    });
    // AgentEnd without TurnEnd (abnormal termination).
    recorder.on_event(AgentEvent::AgentEnd {
        loop_id: "loop-orphan".into(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    });
    recorder.flush();

    let sessions: Vec<_> = recorder.sessions().collect();
    let lr = &sessions[0].loops[0];
    // No turns should be materialized.
    assert_eq!(lr.turn_count(), 0);
}

// ---------------------------------------------------------------------------
// test_session_scope_serde_roundtrip
// ---------------------------------------------------------------------------

#[test]
fn test_session_scope_serde_roundtrip() {
    use phi_core::session::SessionScope;

    // Persistent round-trip
    let persistent = SessionScope::Persistent;
    let json = serde_json::to_string(&persistent).unwrap();
    let deserialized: SessionScope = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, SessionScope::Persistent);

    // Ephemeral round-trip
    let ephemeral = SessionScope::Ephemeral;
    let json = serde_json::to_string(&ephemeral).unwrap();
    let deserialized: SessionScope = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, SessionScope::Ephemeral);
}

// ---------------------------------------------------------------------------
// test_session_new_fields_backward_compat
// ---------------------------------------------------------------------------

#[test]
fn test_session_new_fields_backward_compat() {
    use phi_core::session::{Session, SessionScope};

    // A Session JSON with NO model_config, thinking_level, temperature, or scope fields.
    // This simulates loading a session persisted before those fields were added.
    let json = serde_json::json!({
        "session_id": "s-compat",
        "agent_id": "a-1",
        "created_at": "2025-01-01T00:00:00Z",
        "last_active_at": "2025-01-01T00:00:00Z",
        "formation": { "FirstLoop": { "timestamp": "2025-01-01T00:00:00Z" } },
        "parent_spawn_ref": null,
        "loops": []
    });

    let session: Session =
        serde_json::from_value(json).expect("backward-compat deserialization should succeed");

    assert_eq!(session.session_id, "s-compat");
    // model_config, thinking_level, temperature removed from Session —
    // these are now tracked per-loop in LoopConfigSnapshot.
    assert_eq!(
        session.scope,
        SessionScope::Ephemeral,
        "scope should default to Ephemeral"
    );
}

// ---------------------------------------------------------------------------
// test_before_task_fires_on_new_session
// ---------------------------------------------------------------------------

#[test]
fn test_before_task_fires_on_new_session() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let fired = Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();

    let config = SessionRecorderConfig {
        include_streaming_events: false,
        capture_turn_requests: false,
        before_task: Some(Arc::new(move |_session: &Session| {
            fired_clone.store(true, Ordering::SeqCst);
            true
        })),
        after_task: None,
    };

    let mut recorder = SessionRecorder::new(config);
    let now = chrono::Utc::now();

    recorder.on_event(AgentEvent::AgentStart {
        agent_id: "agent-bt".into(),
        session_id: "session-bt".into(),
        loop_id: "session-bt.default.0".into(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    });

    assert!(
        fired.load(Ordering::SeqCst),
        "before_task should fire on new session creation"
    );
}

// ---------------------------------------------------------------------------
// test_after_task_fires_on_flush
// ---------------------------------------------------------------------------

#[test]
fn test_after_task_fires_on_flush() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let fired = Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();

    let config = SessionRecorderConfig {
        include_streaming_events: false,
        capture_turn_requests: false,
        before_task: None,
        after_task: Some(Arc::new(move |_session: &Session| {
            fired_clone.store(true, Ordering::SeqCst);
        })),
    };

    let mut recorder = SessionRecorder::new(config);
    let now = chrono::Utc::now();

    recorder.on_event(AgentEvent::AgentStart {
        agent_id: "agent-at".into(),
        session_id: "session-at".into(),
        loop_id: "session-at.default.0".into(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    });

    recorder.on_event(AgentEvent::AgentEnd {
        loop_id: "session-at.default.0".into(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    });

    assert!(
        !fired.load(Ordering::SeqCst),
        "after_task should NOT fire before flush"
    );

    recorder.flush();

    assert!(
        fired.load(Ordering::SeqCst),
        "after_task should fire after flush"
    );
}

// ---------------------------------------------------------------------------
// test_task_callbacks_not_fired_on_continuation
// ---------------------------------------------------------------------------

#[test]
fn test_task_callbacks_not_fired_on_continuation() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let config = SessionRecorderConfig {
        include_streaming_events: false,
        capture_turn_requests: false,
        before_task: Some(Arc::new(move |_session: &Session| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            true
        })),
        after_task: None,
    };

    let mut recorder = SessionRecorder::new(config);
    let now = chrono::Utc::now();

    // First AgentStart — creates new session, should fire before_task.
    recorder.on_event(AgentEvent::AgentStart {
        agent_id: "agent-cont".into(),
        session_id: "session-cont".into(),
        loop_id: "session-cont.default.0".into(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    });

    recorder.on_event(AgentEvent::AgentEnd {
        loop_id: "session-cont.default.0".into(),
        messages: vec![],
        usage: Usage::default(),
        timestamp: now,
        rejection: None,
    });

    // Second AgentStart — same session_id (continuation), should NOT fire before_task.
    recorder.on_event(AgentEvent::AgentStart {
        agent_id: "agent-cont".into(),
        session_id: "session-cont".into(),
        loop_id: "session-cont.default.1".into(),
        parent_loop_id: Some("session-cont.default.0".into()),
        continuation_kind: ContinuationKind::Default,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    });

    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "before_task should fire only once for the same session_id"
    );
}

// ---------------------------------------------------------------------------
// G2 — test_before_task_can_abort
// ---------------------------------------------------------------------------

/// The before_task hook fires on new session creation but its return value
/// does not abort session creation (the recorder calls the hook but still
/// creates the session). This test verifies:
/// 1. The hook fires and receives the session.
/// 2. The session is created regardless of the return value.
#[test]
fn test_before_task_can_abort() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let hook_fired = Arc::new(AtomicBool::new(false));
    let hook_fired_clone = hook_fired.clone();

    let config = SessionRecorderConfig {
        include_streaming_events: false,
        capture_turn_requests: false,
        before_task: Some(Arc::new(move |_session: &Session| {
            hook_fired_clone.store(true, Ordering::SeqCst);
            false // return false — attempt to "abort"
        })),
        after_task: None,
    };

    let mut recorder = SessionRecorder::new(config);
    let now = chrono::Utc::now();

    recorder.on_event(AgentEvent::AgentStart {
        agent_id: "agent-abort".into(),
        session_id: "session-abort".into(),
        loop_id: "session-abort.default.0".into(),
        parent_loop_id: None,
        continuation_kind: ContinuationKind::Initial,
        timestamp: now,
        metadata: None,
        config_snapshot: None,
    });

    // Hook should have fired.
    assert!(
        hook_fired.load(Ordering::SeqCst),
        "before_task hook should fire on new session"
    );

    // Session should still exist (returning false does not abort creation).
    assert!(
        recorder.get_session("session-abort").is_some(),
        "session should be created even when before_task returns false"
    );
}

// ---------------------------------------------------------------------------
// TurnTrigger::Continuation serde tests
// ---------------------------------------------------------------------------

#[test]
fn test_turn_trigger_continuation_serializes_as_continuation() {
    let trigger = TurnTrigger::Continuation;
    let json = serde_json::to_string(&trigger).unwrap();
    assert_eq!(json, "\"Continuation\"");
}

#[test]
fn test_turn_trigger_followup_deserializes_as_continuation() {
    // Backward compat: old sessions serialized "FollowUp"
    let trigger: TurnTrigger = serde_json::from_str("\"FollowUp\"").unwrap();
    assert!(matches!(trigger, TurnTrigger::Continuation));
}

#[test]
fn test_turn_trigger_continuation_roundtrips() {
    let original = TurnTrigger::Continuation;
    let json = serde_json::to_string(&original).unwrap();
    let deserialized: TurnTrigger = serde_json::from_str(&json).unwrap();
    assert!(matches!(deserialized, TurnTrigger::Continuation));
}

#[tokio::test]
async fn test_tool_use_turn_has_continuation_trigger() {
    // A tool-use cycle should produce turn 0 (User) and turn 1 (Continuation).
    use phi_core::provider::mock::{MockResponse, MockToolCall};

    struct NoopTool;

    #[async_trait::async_trait]
    impl phi_core::AgentTool for NoopTool {
        fn name(&self) -> &str {
            "test_tool"
        }
        fn label(&self) -> &str {
            "Test"
        }
        fn description(&self) -> &str {
            "noop"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: phi_core::ToolContext,
        ) -> Result<phi_core::ToolResult, phi_core::ToolError> {
            Ok(phi_core::ToolResult {
                content: vec![Content::Text { text: "ok".into() }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "test_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("Done.".into()),
    ]);

    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test".to_string());
    agent.set_tools(vec![Arc::new(NoopTool)]);

    let mut rx = agent.prompt("hi".to_string()).await;

    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    while let Some(event) = rx.recv().await {
        recorder.on_event(event);
    }
    recorder.flush();

    let sessions = recorder.drain_completed();
    assert_eq!(sessions.len(), 1);
    let lr = &sessions[0].loops[0];
    assert!(
        lr.turns.len() >= 2,
        "expected at least 2 turns, got {}",
        lr.turns.len()
    );

    assert!(matches!(lr.turns[0].triggered_by, TurnTrigger::User));
    assert!(
        matches!(lr.turns[1].triggered_by, TurnTrigger::Continuation),
        "tool-use continuation turn should have Continuation trigger, got {:?}",
        lr.turns[1].triggered_by
    );
}
