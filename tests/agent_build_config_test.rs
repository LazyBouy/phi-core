//! Tests for the `Agent::build_config()` -> `Result` migration (MEDIUM-8).
//!
//! The trait signature changed in 0.7.0 from infallible `AgentLoopConfig` to
//! `Result<AgentLoopConfig, AgentBuildError>`. BasicAgent's override always returns
//! `Ok(...)` because its constructor requires a `ModelConfig`; custom Agent
//! implementors that forget to override `model_config()` receive
//! `Err(AgentBuildError::MissingModelConfig)` instead of a panic.

use async_trait::async_trait;
use phi_core::agent_loop::AgentLoopConfig;
use phi_core::agents::{Agent, AgentBuildError, BasicAgent};
use phi_core::provider::ModelConfig;
use phi_core::*;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Minimal `Agent` stub that intentionally forgets to override `model_config()`.
/// All non-trivial trait methods return empty / `todo!()`-style values; we never
/// actually call them — the test only exercises the defaulted `build_config()`.
struct ForgetfulAgent;

#[async_trait]
impl Agent for ForgetfulAgent {
    async fn prompt_messages_with_sender(
        &mut self,
        _messages: Vec<AgentMessage>,
        _tx: mpsc::UnboundedSender<AgentEvent>,
    ) {
    }
    async fn continue_loop_with_sender(
        &mut self,
        _tx: mpsc::UnboundedSender<AgentEvent>,
        _kind: ContinuationKind,
    ) {
    }
    fn messages(&self) -> &[AgentMessage] {
        &[]
    }
    fn is_streaming(&self) -> bool {
        false
    }
    fn agent_id(&self) -> &str {
        "forgetful"
    }
    fn session_id(&self) -> &str {
        "forgetful"
    }
    fn clear_messages(&mut self) {}
    fn append_message(&mut self, _msg: AgentMessage) {}
    fn replace_messages(&mut self, _msgs: Vec<AgentMessage>) {}
    fn save_messages(&self) -> Result<String, serde_json::Error> {
        Ok(String::new())
    }
    fn restore_messages(&mut self, _json: &str) -> Result<(), serde_json::Error> {
        Ok(())
    }
    fn set_tools(&mut self, _tools: Vec<Arc<dyn AgentTool>>) {}
    fn abort(&self) {}
    fn reset(&mut self) {}
    // Intentionally NOT overriding model_config() — exercises the error path.
}

#[test]
fn custom_agent_without_model_config_returns_err_not_panic() {
    let agent = ForgetfulAgent;
    match agent.build_config() {
        Err(AgentBuildError::MissingModelConfig) => {}
        Ok(_) => panic!("expected Err(MissingModelConfig), got Ok"),
    }
}

#[test]
fn missing_model_config_error_display_is_descriptive() {
    let err = AgentBuildError::MissingModelConfig;
    let s = format!("{}", err);
    assert!(
        s.contains("model_config"),
        "error message should mention model_config; got: {}",
        s
    );
}

#[test]
fn basic_agent_build_config_returns_ok_when_constructed_normally() {
    let agent = BasicAgent::new(ModelConfig::anthropic("m", "n", "k"));
    let cfg: AgentLoopConfig = agent.build_config().unwrap();
    assert_eq!(cfg.model_config.id, "m");
}

#[tokio::test]
async fn basic_agent_prompt_still_works_end_to_end() {
    // Sanity: the Result migration didn't break the BasicAgent prompt path.
    use phi_core::provider::MockProvider;
    let mut agent = BasicAgent::new(ModelConfig::anthropic("m", "n", "k"))
        .with_provider_override(Arc::new(MockProvider::text("hello back")));

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_messages_with_sender(
            vec![AgentMessage::Llm(LlmMessage::new(Message::user("hi")))],
            tx,
        )
        .await;

    let mut saw_end = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AgentEvent::AgentEnd { .. }) {
            saw_end = true;
        }
    }
    assert!(saw_end);
}
