//! Tests for [`RevertTool`], [`RevertRequest`], and the opt-in guarantee.
//!
//! Phase 3+ tests (`apply_revert`, `RevertApplied` event, trunk assembly) live
//! in `agent_loop_test.rs` and `prun_test.rs`'s sibling files.

use phi_core::agents::BasicAgent;
use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::tools::revert::{RevertRequest, RevertTool};
use phi_core::types::*;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn ctx(name: &str) -> ToolContext {
    ToolContext {
        tool_call_id: "t1".into(),
        tool_name: name.into(),
        cancel: CancellationToken::new(),
        on_update: None,
        on_progress: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Schema sanity
// ---------------------------------------------------------------------------

#[test]
fn revert_tool_schema_exposes_required_fields() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending);
    assert_eq!(tool.name(), "revert_to_state");
    let schema = tool.parameters_schema();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .expect("required is array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"category"));
    assert!(required.contains(&"step"));
    assert!(!required.contains(&"summary"));

    let category_enum: Vec<&str> = schema["properties"]["category"]["enum"]
        .as_array()
        .expect("category.enum is array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        category_enum,
        vec!["failure", "tangent", "completion", "step-summary"]
    );
}

// ---------------------------------------------------------------------------
// 2. execute() enqueues a well-formed request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revert_execute_enqueues_well_formed_request() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());

    let result = tool
        .execute(
            serde_json::json!({
                "category": "failure",
                "step": "n10",
                "summary": "bubble sort timed out"
            }),
            ctx("revert_to_state"),
        )
        .await
        .expect("execute succeeds on valid args");
    assert!(result.child_loop_id.is_none());

    let queue = pending.lock().unwrap();
    assert_eq!(queue.len(), 1);
    let req: &RevertRequest = &queue[0];
    assert_eq!(req.category, RevertCategory::Failure);
    assert_eq!(req.target, NodeId(10));
    assert_eq!(req.summary.as_deref(), Some("bubble sort timed out"));
}

#[tokio::test]
async fn revert_execute_supports_all_four_categories() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());
    for (label, expected) in [
        ("failure", RevertCategory::Failure),
        ("tangent", RevertCategory::Tangent),
        ("completion", RevertCategory::Completion),
        ("step-summary", RevertCategory::StepSummary),
    ] {
        tool.execute(
            serde_json::json!({"category": label, "step": "n3"}),
            ctx("revert_to_state"),
        )
        .await
        .expect("valid category");
        let q = pending.lock().unwrap();
        assert_eq!(q.last().unwrap().category, expected);
    }
}

#[tokio::test]
async fn revert_execute_parses_step_leniently() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());
    tool.execute(
        serde_json::json!({"category": "tangent", "step": "12"}),
        ctx("revert_to_state"),
    )
    .await
    .expect("bare integer step is accepted");
    tool.execute(
        serde_json::json!({"category": "tangent", "step": "n13"}),
        ctx("revert_to_state"),
    )
    .await
    .expect("n-prefixed step is accepted");
    let q = pending.lock().unwrap();
    assert_eq!(q[0].target, NodeId(12));
    assert_eq!(q[1].target, NodeId(13));
}

#[tokio::test]
async fn revert_execute_summary_is_optional() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());
    tool.execute(
        serde_json::json!({"category": "completion", "step": "n7"}),
        ctx("revert_to_state"),
    )
    .await
    .expect("summary omission is valid");
    let q = pending.lock().unwrap();
    assert!(q[0].summary.is_none());
}

// ---------------------------------------------------------------------------
// 3. execute() rejects malformed input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revert_execute_rejects_missing_category() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());
    let err = tool
        .execute(serde_json::json!({"step": "n1"}), ctx("revert_to_state"))
        .await
        .expect_err("missing category must error");
    assert!(matches!(err, ToolError::InvalidArgs(_)));
    assert!(pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revert_execute_rejects_unknown_category() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());
    let err = tool
        .execute(
            serde_json::json!({"category": "explode", "step": "n1"}),
            ctx("revert_to_state"),
        )
        .await
        .expect_err("unknown category must error");
    assert!(matches!(err, ToolError::InvalidArgs(_)));
    assert!(pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revert_execute_rejects_missing_step() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());
    let err = tool
        .execute(
            serde_json::json!({"category": "failure"}),
            ctx("revert_to_state"),
        )
        .await
        .expect_err("missing step must error");
    assert!(matches!(err, ToolError::InvalidArgs(_)));
}

#[tokio::test]
async fn revert_execute_rejects_malformed_step() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let tool = RevertTool::new(pending.clone());
    let err = tool
        .execute(
            serde_json::json!({"category": "failure", "step": "banana"}),
            ctx("revert_to_state"),
        )
        .await
        .expect_err("non-numeric step must error");
    assert!(matches!(err, ToolError::InvalidArgs(_)));
    assert!(pending.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 4. with_revert_tool() registers the tool and wires the queue
// ---------------------------------------------------------------------------

#[test]
fn with_revert_tool_registers_tool_and_wires_queue() {
    let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(MockProvider::texts(vec!["ack"])))
        .with_revert_tool();

    // (a) tool registry advertises `revert_to_state`
    let names: Vec<&str> = agent.tools().iter().map(|t| t.name()).collect();
    assert!(
        names.contains(&"revert_to_state"),
        "tools list should contain revert_to_state after with_revert_tool; got {:?}",
        names
    );

    // (b) the agent's build_config() carries revert_pending=Some
    let cfg = agent.build_config().expect("build_config succeeds");
    assert!(
        cfg.revert_pending.is_some(),
        "AgentLoopConfig.revert_pending must be Some when with_revert_tool() was called"
    );
}

// ---------------------------------------------------------------------------
// 5. Opt-in hard guarantee — without with_revert_tool(), the tool is unreachable
// ---------------------------------------------------------------------------
//
// D1 guarantee: a BasicAgent constructed without calling .with_revert_tool()
// has no path to invoke `revert_to_state`. Assertions:
//   (a) tool registry does NOT advertise `revert_to_state`
//   (b) AgentLoopConfig.revert_pending is None
//
// (c) "the apply_revert drain never executes" is a Phase 3 assertion (depends
// on the AgentEvent::RevertApplied variant landing); deferred to the
// agent_loop_test.rs MockProvider end-to-end test.

#[test]
fn opt_in_guarantee_without_with_revert_tool() {
    let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(MockProvider::texts(vec!["ack"])));

    // (a) tool registry does not contain `revert_to_state`
    let names: Vec<&str> = agent.tools().iter().map(|t| t.name()).collect();
    assert!(
        !names.contains(&"revert_to_state"),
        "revert_to_state must NOT be registered without with_revert_tool(); got {:?}",
        names
    );

    // (b) AgentLoopConfig.revert_pending is None
    let cfg = agent.build_config().expect("build_config succeeds");
    assert!(
        cfg.revert_pending.is_none(),
        "AgentLoopConfig.revert_pending must be None without with_revert_tool()"
    );
}
