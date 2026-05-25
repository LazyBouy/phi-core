//! Integration tests for the phi-core 0.10.0 surface additions.
//!
//! Covers:
//!
//! 1. **`BasicAgent::with_revert_render_policy`** — verifies that a custom
//!    `RevertRenderPolicy` configured on the builder actually shapes the
//!    LLM-facing context when revert mode is active (the load-bearing
//!    `stream_assistant_response` wire-through landed at 0.10.0).
//!
//! 2. **`BasicAgent::current_tool_timeout()`** — verifies that the
//!    introspection method surfaces the in-flight tool's effective timeout
//!    while `AgentTool::execute()` is running, and returns `None` before /
//!    after.
//!
//! 3. **Async-migrated tool-update hooks** — verifies the
//!    `BeforeToolExecutionUpdateFn` / `AfterToolExecutionUpdateFn` are
//!    correctly bridged from sync `ToolUpdateFn` via
//!    `futures::executor::block_on` (the on-update hook still fires; the
//!    before-update hook can still veto).
//!
//! 4. **Public `phi_core::agent_loop::script_callback::detect_interpreter`**
//!    — verifies the function is reachable by external callers via its
//!    module-qualified path (0.10.0 pub-ified).

use phi_core::agent_loop::script_callback::detect_interpreter;
use phi_core::agents::{Agent, BasicAgent};
use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::types::*;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// 1. with_revert_render_policy — load-bearing wire-through
// ---------------------------------------------------------------------------

/// Custom render policy lands on the agent's loop config + is read at the
/// streaming dispatch site (`stream_assistant_response` switches to
/// `build_trunk_context_with_policy` when `active_node_id.is_some()`).
///
/// Smoke-level: confirm the setter+propagation path. The per-policy decay
/// semantics are exhaustively unit-tested in `types/context.rs`.
#[test]
fn with_revert_render_policy_propagates_to_loop_config() {
    let tight_policy = RevertRenderPolicy {
        lesson_window_turns: 1,
        lesson_window_count: 1,
    };
    let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock-model", "test-key"))
        .with_revert_tool()
        .with_revert_render_policy(tight_policy);

    let config = agent.build_config().expect("BasicAgent has a model_config");
    assert_eq!(
        config.revert_render_policy.lesson_window_turns, 1,
        "custom lesson_window_turns must propagate to AgentLoopConfig"
    );
    assert_eq!(
        config.revert_render_policy.lesson_window_count, 1,
        "custom lesson_window_count must propagate to AgentLoopConfig"
    );
}

/// End-to-end: when an agent runs with revert mode active AND a tight
/// `RevertRenderPolicy`, the prompt visible to the LLM strips decay-able
/// tags outside the policy window.
///
/// Setup: hand-construct an `AgentContext` with on-trunk `Lesson` tags at old
/// turns, then run one mock turn at a later turn index. With a tight policy
/// (1 turn window, 1 count cap) the old `Lesson` tag should NOT appear in
/// the provider's captured `system_prompt`-derived context messages. Note:
/// `Outcome` / `Checkpoint` tags always render — only decay-able kinds are
/// affected. The CapturingMockProvider sees exactly what the policy allowed
/// through.
#[tokio::test]
async fn revert_render_policy_strips_old_lesson_tags_from_llm_prompt() {
    use phi_core::agent_loop::{agent_loop, AgentLoopConfig};
    use phi_core::provider::{
        mock::MockResponse, ProviderError, StreamConfig, StreamEvent, StreamProvider,
    };
    use std::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    // CapturingMockProvider — records exactly what `StreamConfig.messages`
    // the provider received, so the test can assert on the LLM-facing prompt.
    struct CapturingMockProvider {
        inner: MockProvider,
        captured: Mutex<Vec<Vec<Message>>>,
    }

    impl CapturingMockProvider {
        fn new(responses: Vec<MockResponse>) -> Self {
            Self {
                inner: MockProvider::new(responses),
                captured: Mutex::new(Vec::new()),
            }
        }

        fn captured_messages(&self) -> Vec<Vec<Message>> {
            self.captured.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl StreamProvider for CapturingMockProvider {
        fn provider_id(&self) -> &str {
            "capturing-mock"
        }
        async fn stream(
            &self,
            config: StreamConfig,
            tx: mpsc::UnboundedSender<StreamEvent>,
            cancel: CancellationToken,
        ) -> Result<Message, ProviderError> {
            self.captured.lock().unwrap().push(config.messages.clone());
            self.inner.stream(config, tx, cancel).await
        }
    }

    // Build a trunk with 3 lessons at turns 0, 1, 2 + active pointer set,
    // then drive one turn at turn_index=10. With a 1-turn window + 1-count
    // cap, only the newest lesson (turn 2) is allowed to render.
    fn assistant_with_lesson_tag(
        text: &str,
        ts: u64,
        node: NodeId,
        parent: Option<NodeId>,
        tag_turn: u32,
    ) -> AgentMessage {
        let mut am = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: text.to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: ts,
                error_message: None,
            })
            .with_node_identity(node, parent),
        );
        if let AgentMessage::Llm(ref mut lm) = am {
            lm.tags.push(NodeTag::new(
                TagKind::Lesson,
                format!("L-at-turn-{}", tag_turn),
                tag_turn,
                vec![],
            ));
        }
        am
    }

    // Drive 2 turns: turn 0 calls a no-op tool, turn 1 emits final text.
    // The probe captures the policy-filtered context at EACH turn; we
    // assert against the LAST one (turn 1), where decay-window math is
    // non-trivial: at current_turn=1, only tags ≤ 1 turn old render.
    let provider = Arc::new(CapturingMockProvider::new(vec![
        MockResponse::ToolCalls(vec![phi_core::provider::mock::MockToolCall {
            name: "noop".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("final".into()),
    ]));

    // Tight policy: 0-turn window, 1-count cap. At turn_index=1, a tag at
    // turn 0 is OUT of the window (distance=1 > 0); only the count_cap=1
    // saves the newest decay-able tag.
    let policy = RevertRenderPolicy {
        lesson_window_turns: 0,
        lesson_window_count: 1,
    };

    // Hook into `convert_to_llm` to capture the EXACT post-policy
    // `AgentMessage[]` that the streaming pipeline produced — including
    // their `LlmMessage.tags` vectors. This is the only place where tag
    // attachments are still visible (provider wire-format only carries
    // `Message`, not `LlmMessage`).
    //
    // The probe overwrites on each invocation; after 2 turns it holds the
    // turn-1 snapshot.
    let captured_post_policy: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured_post_policy.clone();
    let turn_snapshots: Arc<Mutex<Vec<Vec<AgentMessage>>>> = Arc::new(Mutex::new(Vec::new()));
    let snapshots_clone = turn_snapshots.clone();
    let probe_convert = Arc::new(move |msgs: &[AgentMessage]| -> Vec<Message> {
        // Snapshot what we received at every turn.
        *captured_clone.lock().unwrap() = msgs.to_vec();
        snapshots_clone.lock().unwrap().push(msgs.to_vec());
        // Default behaviour: keep only LLM-visible messages.
        msgs.iter().filter_map(|m| m.as_llm().cloned()).collect()
    });

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock-model", "test-key"),
        provider_override: Some(provider.clone()),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: Some(probe_convert),
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
        current_tool: None,
        revert_render_policy: policy,
    };

    // A no-op tool so turn 0 can complete its tool call cleanly.
    struct NoopTool;
    #[async_trait::async_trait]
    impl AgentTool for NoopTool {
        fn name(&self) -> &str {
            "noop"
        }
        fn label(&self) -> &str {
            "Noop"
        }
        fn description(&self) -> &str {
            "no-op"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![Content::Text { text: "ok".into() }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    // Seed the context's history with a trunk that has Lesson-tagged
    // assistants at turns 0/1, then drive `agent_loop()` with a new user
    // message at the tail. The agent loop appends this user message into
    // `messages` and runs TWO turns — `stream_assistant_response` reads via
    // the policy-aware dispatch because `active_node_id.is_some()`.
    let mut context = AgentContext {
        system_prompt: "system".into(),
        messages: vec![
            // A user prompt at node 0 (the trunk root — required for the
            // walk to surface at least something).
            AgentMessage::Llm(
                LlmMessage::new(Message::User {
                    content: vec![Content::Text {
                        text: "start".into(),
                    }],
                    timestamp: 0,
                })
                .with_node_identity(NodeId(0), None),
            ),
            assistant_with_lesson_tag("a", 1, NodeId(1), Some(NodeId(0)), 0),
            assistant_with_lesson_tag("b", 2, NodeId(2), Some(NodeId(1)), 1),
        ],
        tools: vec![Arc::new(NoopTool)],
        agent_id: Some("agent-1".into()),
        session_id: Some("session-1".into()),
        loop_id: Some("session-1.test.1".into()),
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
        active_node_id: Some(NodeId(2)), // revert mode active — newest trunk tip
        next_node_id: 3,
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    // Drive 2 turns via agent_loop() with a fresh user message — this
    // appends to `messages` and runs the 2 mock turns (tool call then text).
    let new_user = vec![AgentMessage::Llm(LlmMessage::new(Message::user(
        "next-turn",
    )))];
    agent_loop(new_user, &mut context, &config, tx, cancel).await;

    // Inspect what `convert_to_llm` received — these are the EXACT
    // policy-filtered `AgentMessage`s that the streaming pipeline produced.
    // Their `LlmMessage.tags` vectors reveal whether the policy applied.
    let snapshot = captured_post_policy.lock().unwrap().clone();
    assert!(
        !snapshot.is_empty(),
        "convert_to_llm probe must have received at least one message"
    );

    // Sanity: confirm at least one wire-format StreamConfig reached the
    // provider (the loop made a real turn).
    let captured_wire = provider.captured_messages();
    assert!(
        !captured_wire.is_empty(),
        "provider must have received at least one StreamConfig"
    );

    // Count the surviving Lesson tags across the policy-filtered messages.
    // With a 1-turn window + 1-count cap at turn_index=0, the newest lesson
    // (created at turn 2) is within `count_cap=1`'s force-keep set, while
    // older lessons (turns 0, 1) fall outside both gates. The newest lesson
    // survives only via the count cap → exactly 1 tag total.
    let surviving_lesson_count: usize = snapshot
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(lm) => {
                Some(lm.tags.iter().filter(|t| t.kind == TagKind::Lesson).count())
            }
            _ => None,
        })
        .sum();
    assert_eq!(
        surviving_lesson_count, 1,
        "tight RevertRenderPolicy must strip the 2 older Lesson tags and keep only the newest one \
         (window=1, count_cap=1, current_turn=0, trunk lesson turns=[0,1,2])"
    );

    // Cross-check against the standalone `build_trunk_context_with_policy`
    // at the same `current_turn` the loop's last invocation used. The probe
    // captured 2 invocations (one per turn); the LAST capture is at
    // `turn_index = 1`. The standalone filter at turn 1 must match.
    let all_snapshots = turn_snapshots.lock().unwrap();
    assert_eq!(
        all_snapshots.len(),
        2,
        "the loop must have invoked convert_to_llm once per turn (2 turns: tool call + text)"
    );
    let manual = context.build_trunk_context_with_policy(&policy, 1);
    let manual_lesson_count: usize = manual
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(lm) => {
                Some(lm.tags.iter().filter(|t| t.kind == TagKind::Lesson).count())
            }
            _ => None,
        })
        .sum();
    assert_eq!(
        surviving_lesson_count, manual_lesson_count,
        "policy applied by the loop's streaming dispatch at turn_index=1 must match the standalone filter at current_turn=1"
    );
}

// ---------------------------------------------------------------------------
// 2. current_tool_timeout() — end-to-end through a running tool
// ---------------------------------------------------------------------------

/// While a tool is executing, `BasicAgent::current_tool_timeout()` reflects
/// the resolved effective timeout. Before / after, it returns `None`.
#[tokio::test]
async fn current_tool_timeout_visible_during_tool_execution() {
    use phi_core::provider::mock::{MockResponse, MockToolCall};

    /// A tool that exposes its own timeout via the `AgentTool::timeout()`
    /// override AND uses a small sleep so the test can sample the slot
    /// while the tool is in-flight.
    struct SleepyTool {
        /// Used by the test to observe `current_tool_timeout()` mid-run.
        observed_during_exec: Arc<std::sync::Mutex<Option<Duration>>>,
        /// A clone of the agent's current-tool shared slot, captured before
        /// the loop starts, so the tool body can read it from inside execute.
        agent_slot: Arc<std::sync::Mutex<Option<phi_core::context::CurrentToolExecution>>>,
    }

    #[async_trait::async_trait]
    impl AgentTool for SleepyTool {
        fn name(&self) -> &str {
            "sleepy"
        }
        fn label(&self) -> &str {
            "Sleepy"
        }
        fn description(&self) -> &str {
            "sleeps briefly then returns"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn timeout(&self) -> Option<Duration> {
            Some(Duration::from_secs(7))
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            // Sample the agent's shared slot from inside the tool execution.
            // This is what `BasicAgent::current_tool_timeout()` would read
            // if called from another task at the same moment.
            tokio::time::sleep(Duration::from_millis(5)).await;
            {
                let guard = self.agent_slot.lock().unwrap();
                let observed = guard.as_ref().and_then(|t| t.timeout);
                let mut out = self.observed_during_exec.lock().unwrap();
                *out = observed;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(ToolResult {
                content: vec![Content::Text {
                    text: "done".into(),
                }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let observed = Arc::new(std::sync::Mutex::new(None::<Duration>));

    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "sleepy".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("done".into()),
    ]);

    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock-model", "test-key"))
        .with_provider_override(Arc::new(provider));

    // Before any prompt: slot is None.
    assert!(agent.current_tool_timeout().is_none());

    // Capture the agent's shared slot Arc so the tool body can read it from
    // inside execute(). `BasicAgent` exposes the slot only through the
    // getter, not as a field — we use `build_config()` to get the same Arc
    // the loop will install.
    let config_slot = agent
        .build_config()
        .unwrap()
        .current_tool
        .expect("BasicAgent installs the shared slot");

    let tool = Arc::new(SleepyTool {
        observed_during_exec: observed.clone(),
        agent_slot: config_slot,
    });
    agent.set_tools(vec![tool]);

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_messages_with_sender(
            vec![AgentMessage::Llm(LlmMessage::new(Message::user("go")))],
            tx,
        )
        .await;

    // During execution: tool's own timeout (7s) overrode the absent
    // config-level default → effective timeout = 7s.
    let mid_run = *observed.lock().unwrap();
    assert_eq!(
        mid_run,
        Some(Duration::from_secs(7)),
        "during tool execution the agent's current_tool_timeout slot must reflect the tool's effective timeout"
    );

    // After execution: slot is cleared.
    assert!(
        agent.current_tool_timeout().is_none(),
        "after tool execution the slot must be cleared back to None"
    );
}

// ---------------------------------------------------------------------------
// 3. Async-migrated tool-update hooks — sync bridging through block_on
// ---------------------------------------------------------------------------

/// The 0.10.0 async migration of `BeforeToolExecutionUpdateFn` and
/// `AfterToolExecutionUpdateFn` is bridged from the sync `ToolUpdateFn`
/// callback via `futures::executor::block_on` inside `tools.rs`. This test
/// confirms that:
///
/// - Sync hook bodies (the common case via `on_*_tool_execution_update`
///   setters that wrap user closures in `Box::pin(async move { ... })`) fire
///   correctly.
/// - The before-update hook's `false` return still vetoes the
///   `ToolExecutionUpdate` event AND the after-update hook is skipped (matching
///   pre-0.10 sync semantics).
#[tokio::test]
async fn async_update_hooks_fire_through_sync_bridge() {
    use phi_core::provider::mock::{MockResponse, MockToolCall};

    /// A tool that emits 3 `on_update` calls then completes.
    struct UpdatingTool;
    #[async_trait::async_trait]
    impl AgentTool for UpdatingTool {
        fn name(&self) -> &str {
            "updater"
        }
        fn label(&self) -> &str {
            "Updater"
        }
        fn description(&self) -> &str {
            "emits 3 updates"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            if let Some(ref on_update) = ctx.on_update {
                for i in 0..3 {
                    on_update(ToolResult {
                        content: vec![Content::Text {
                            text: format!("partial-{}", i),
                        }],
                        details: serde_json::Value::Null,
                        child_loop_id: None,
                    });
                }
            }
            Ok(ToolResult {
                content: vec![Content::Text {
                    text: "done".into(),
                }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let before_count = Arc::new(AtomicU32::new(0));
    let after_count = Arc::new(AtomicU32::new(0));

    // Case 1: before-update hook always allows → both before + after fire 3×.
    {
        let provider = MockProvider::new(vec![
            MockResponse::ToolCalls(vec![MockToolCall {
                name: "updater".into(),
                arguments: serde_json::json!({}),
            }]),
            MockResponse::Text("done".into()),
        ]);
        let bc = before_count.clone();
        let ac = after_count.clone();
        let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock-model", "test-key"))
            .with_provider_override(Arc::new(provider))
            .on_before_tool_execution_update(move |_name, _id, _text| {
                bc.fetch_add(1, Ordering::SeqCst);
                true // always allow
            })
            .on_after_tool_execution_update(move |_name, _id, _text| {
                ac.fetch_add(1, Ordering::SeqCst);
            });
        agent.set_tools(vec![Arc::new(UpdatingTool)]);

        let (tx, _rx) = mpsc::unbounded_channel();
        agent
            .prompt_messages_with_sender(
                vec![AgentMessage::Llm(LlmMessage::new(Message::user("go")))],
                tx,
            )
            .await;

        assert_eq!(
            before_count.load(Ordering::SeqCst),
            3,
            "before-update hook must fire once per on_update call"
        );
        assert_eq!(
            after_count.load(Ordering::SeqCst),
            3,
            "after-update hook must fire once per emitted ToolExecutionUpdate event"
        );
    }

    // Case 2: before-update hook vetoes → after-update never fires.
    let veto_before = Arc::new(AtomicU32::new(0));
    let veto_after = Arc::new(AtomicU32::new(0));
    {
        let provider = MockProvider::new(vec![
            MockResponse::ToolCalls(vec![MockToolCall {
                name: "updater".into(),
                arguments: serde_json::json!({}),
            }]),
            MockResponse::Text("done".into()),
        ]);
        let bc = veto_before.clone();
        let ac = veto_after.clone();
        let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock-model", "test-key"))
            .with_provider_override(Arc::new(provider))
            .on_before_tool_execution_update(move |_name, _id, _text| {
                bc.fetch_add(1, Ordering::SeqCst);
                false // veto every update
            })
            .on_after_tool_execution_update(move |_name, _id, _text| {
                ac.fetch_add(1, Ordering::SeqCst);
            });
        agent.set_tools(vec![Arc::new(UpdatingTool)]);

        let (tx, _rx) = mpsc::unbounded_channel();
        agent
            .prompt_messages_with_sender(
                vec![AgentMessage::Llm(LlmMessage::new(Message::user("go")))],
                tx,
            )
            .await;

        assert_eq!(
            veto_before.load(Ordering::SeqCst),
            3,
            "before-update hook fires once per on_update even when vetoing"
        );
        assert_eq!(
            veto_after.load(Ordering::SeqCst),
            0,
            "after-update hook is skipped when before-update vetoes"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Public `detect_interpreter`
// ---------------------------------------------------------------------------

/// The function was pub-ified at 0.10.0 so external consumers (i-phi) can
/// adopt the same script-extension dispatch table rather than re-deriving it.
#[test]
fn detect_interpreter_is_publicly_reachable_and_correct() {
    // Reachable via module-qualified path (the 0.10.0 visibility flip).
    assert_eq!(
        detect_interpreter(Path::new("hook.py")),
        vec!["python3".to_string()],
    );
    assert_eq!(
        detect_interpreter(Path::new("hook.sh")),
        vec!["sh".to_string()],
    );
    // Default branch — anything else / no extension defaults to shell.
    assert_eq!(
        detect_interpreter(Path::new("hook")),
        vec!["sh".to_string()],
    );
    assert_eq!(
        detect_interpreter(Path::new("hook.unknown")),
        vec!["sh".to_string()],
    );
}
