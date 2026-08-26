//! KC-06 (i-phi #121, kernel half) — MockProvider opt-in repeat/cycle mode.
//!
//! These loop-driven tests need the full agent loop (`agent_loop` +
//! `agent_loop_continue`), so they live here rather than inline in `mock.rs`
//! (which carries the pure-provider Tier A back-compat + Tier F API-equivalence
//! tests).
//!
//! Tier C (CLOSE-GATE) — a repeating `MockProvider` over `[ToolCalls, Text]`
//! re-emits exactly one tool call on EVERY send-input (`[1, 1, 1]`), the
//! disposition #121 needs.
//! Tier B — one-shot (`repeat` OFF) exhausts after the 2-items-per-turn drain,
//! and the second send hits the `"(no more mock responses)"` fallback.
//! Tier D — a `[ToolCalls]`-only repeating queue is an UNBOUNDED tool loop the
//! kernel does NOT auto-terminate (bounded only by `max_turns`), proving the
//! §D6.4 CONSUMER contract that the repeating unit MUST carry a terminal text.
//! Tier E — cadence is a function of queue COMPOSITION (the CONSUMER owns the
//! `[ToolCalls, Text]` ordering); a leading text or a non-length-2 queue drifts
//! the cadence.

use phi_core::agent_loop::{agent_loop, agent_loop_continue, AgentLoopConfig};
use phi_core::context::ExecutionLimits;
use phi_core::provider::mock::*;
use phi_core::provider::{MockProvider, ModelConfig, StreamProvider};
use phi_core::*;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A trivial tool the scripted `MockResponse::ToolCalls` targets; it ignores its
/// params and returns a fixed result so every scripted call executes cleanly.
struct EchoTool;

#[async_trait::async_trait]
impl AgentTool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn label(&self) -> &str {
        "Echo"
    }
    fn description(&self) -> &str {
        "Echoes back a fixed result"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
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

fn make_config(
    provider: Arc<dyn StreamProvider>,
    execution_limits: Option<ExecutionLimits>,
) -> AgentLoopConfig {
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
        execution_limits,
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        tool_timeout: None,
        response_format: phi_core::provider::ResponseFormat::Text,
        provider_wire_sink: None,
        progressive_tool_catalog: Default::default(),
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
        revert_render_policy: phi_core::RevertRenderPolicy::default(),
    }
}

/// A context wired for continuation: `agent_loop_continue` asserts `agent_id` +
/// `session_id` are set (identity must carry over from the originating loop).
fn fresh_session_context(tools: Vec<Arc<dyn AgentTool>>) -> AgentContext {
    AgentContext {
        system_prompt: "You are a test agent.".into(),
        messages: Vec::new(),
        tools,
        agent_id: Some("kc06-agent".into()),
        session_id: Some("kc06-session".into()),
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
        active_node_id: None,
        next_node_id: 0,
    }
}

fn tool_calls() -> MockResponse {
    MockResponse::ToolCalls(vec![MockToolCall {
        name: "echo".into(),
        arguments: serde_json::json!({}),
    }])
}

fn text(t: &str) -> MockResponse {
    MockResponse::Text(t.into())
}

fn user(t: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::new(Message::user(t)))
}

fn drain(mut rx: mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

fn tool_starts(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .count()
}

/// Runs one simulated "send-input" against a single session and returns the
/// number of tool calls that turn emitted. Send #0 uses `agent_loop` with a
/// fresh user prompt; every later send appends a user message (so the last
/// message is not an assistant one — `agent_loop_continue`'s precondition) and
/// continues the SAME session, so the provider's queue state carries across.
async fn send(context: &mut AgentContext, config: &AgentLoopConfig, index: usize) -> usize {
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    if index == 0 {
        agent_loop(vec![user("go")], context, config, tx, cancel).await;
    } else {
        context.messages.push(user("again"));
        agent_loop_continue(context, config, tx, cancel).await;
    }
    tool_starts(&drain(rx))
}

/// Drives `sends` send-inputs against one repeating session over the given queue
/// and returns the per-send tool-call counts (the cadence).
async fn cadence(queue: Vec<MockResponse>, repeat: bool, sends: usize) -> Vec<usize> {
    let provider = Arc::new(MockProvider::with_repeat(queue, repeat));
    let config = make_config(provider, None);
    let mut context = fresh_session_context(vec![Arc::new(EchoTool) as Arc<dyn AgentTool>]);
    let mut counts = Vec::with_capacity(sends);
    for i in 0..sends {
        counts.push(send(&mut context, &config, i).await);
    }
    counts
}

// ---------------------------------------------------------------------------
// Tier C — CLOSE-GATE: repeat sustains one tool call per send-input
// ---------------------------------------------------------------------------

/// The KERNEL close-gate. A repeating `[ToolCalls, Text]` session must re-emit
/// exactly ONE tool call on every send-input across ≥ 3 turns — the disposition
/// #121 requires (`send → Ask → approve → send → Ask …` in one session), not
/// merely "no panic".
#[tokio::test]
async fn close_gate_repeat_sustains_one_tool_call_per_send() {
    let counts = cadence(vec![tool_calls(), text("done")], true, 3).await;
    assert_eq!(
        counts,
        vec![1, 1, 1],
        "repeating [ToolCalls, Text] must re-emit exactly one tool call on EVERY send-input"
    );
}

// ---------------------------------------------------------------------------
// Tier B — one-shot exhausts (repeat OFF) after the 2-items-per-turn drain
// ---------------------------------------------------------------------------

/// With `repeat` OFF, a `[ToolCalls, Text]` queue is fully drained by ONE
/// interactive send (a turn consumes 2 items: the tool round + the terminal
/// round). The second send finds an empty queue → the `"(no more mock
/// responses)"` fallback, 0 tool calls: the exact #121 "dead session".
#[tokio::test]
async fn one_shot_exhausts_to_fallback_on_second_send() {
    let provider = Arc::new(MockProvider::new(vec![tool_calls(), text("done")]));
    let config = make_config(provider, None);
    let mut context = fresh_session_context(vec![Arc::new(EchoTool) as Arc<dyn AgentTool>]);

    // Send #1: the 2-item queue drains in one turn → exactly one tool call.
    let (tx, rx) = mpsc::unbounded_channel();
    agent_loop(
        vec![user("go")],
        &mut context,
        &config,
        tx,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(tool_starts(&drain(rx)), 1);

    // Send #2: queue empty → fallback text, 0 tool calls.
    context.messages.push(user("again"));
    let (tx, rx) = mpsc::unbounded_channel();
    let new_messages =
        agent_loop_continue(&mut context, &config, tx, CancellationToken::new()).await;
    assert_eq!(tool_starts(&drain(rx)), 0);

    // Disposition: the assistant reply on send #2 is the exhaustion fallback.
    let last_assistant_text = new_messages.iter().rev().find_map(|m| match m.as_llm() {
        Some(Message::Assistant { content, .. }) => content.iter().find_map(|c| match c {
            Content::Text { text } => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(
        last_assistant_text.as_deref(),
        Some("(no more mock responses)"),
        "one-shot exhaustion must surface the fallback text on the dead send"
    );
}

// ---------------------------------------------------------------------------
// Tier D — terminal-text-required CONSUMER contract (§D6.4)
// ---------------------------------------------------------------------------

/// A `[ToolCalls]`-only repeating queue never yields a terminal (non-tool)
/// `Text`, so the inner loop never satisfies `!has_tool_calls` and runs
/// UNBOUNDED — halted only by `max_turns`. This is the §D6.4 contract: under
/// repeat the empty→fallback safety-net is bypassed, so the repeating unit MUST
/// include a terminal text; the kernel does NOT auto-synthesize one.
#[tokio::test]
async fn toolcalls_only_repeat_is_bounded_only_by_max_turns() {
    let provider = Arc::new(MockProvider::repeating(vec![tool_calls()]));
    let limits = ExecutionLimits {
        max_turns: 4,
        ..Default::default()
    };
    let config = make_config(provider, Some(limits));
    let mut context = fresh_session_context(vec![Arc::new(EchoTool) as Arc<dyn AgentTool>]);

    let (tx, rx) = mpsc::unbounded_channel();
    agent_loop(
        vec![user("go")],
        &mut context,
        &config,
        tx,
        CancellationToken::new(),
    )
    .await;

    // Exactly `max_turns` tool rounds inside ONE send-input — proving the loop
    // is NEITHER one-and-done (a terminal text would give 1) NOR infinite.
    assert_eq!(
        tool_starts(&drain(rx)),
        4,
        "a [ToolCalls]-only repeat must loop until max_turns (unbounded without a terminal text)"
    );
}

// ---------------------------------------------------------------------------
// Tier E — cadence is a function of queue COMPOSITION (CONSUMER ordering)
// ---------------------------------------------------------------------------

/// The kernel cycles the queue VERBATIM, so correct one-tool-call-per-turn
/// cadence is the CONSUMER's responsibility. `[ToolCalls, Text]` is perfect;
/// a leading text makes send #1 a dud; an odd-length queue alternates.
#[tokio::test]
async fn cadence_depends_on_queue_shape() {
    // [ToolCalls, Text] — every send parks a tool call.
    assert_eq!(
        cadence(vec![tool_calls(), text("t")], true, 4).await,
        vec![1, 1, 1, 1],
        "[ToolCalls, Text] is the correct repeating unit"
    );
    // [Text, ToolCalls] (i-phi's CURRENT ordering) — send #1 is a dud, then self-corrects.
    assert_eq!(
        cadence(vec![text("t"), tool_calls()], true, 4).await,
        vec![0, 1, 1, 1],
        "a leading text ends turn #1 before the tool is reached"
    );
    // [Text, ToolCalls, Text] length-3 — cadence alternates dud/park.
    assert_eq!(
        cadence(vec![text("t"), tool_calls(), text("u")], true, 4).await,
        vec![0, 1, 0, 1],
        "a non-length-2 queue is not a clean multiple of the 2-items-per-send consumption"
    );
}
