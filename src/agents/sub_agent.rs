//! Sub-agent tool — delegates tasks to a child agent loop.
//!
//! The `SubAgentTool` implements `AgentTool` and internally runs `agent_loop()`
//! with its own system prompt, tools, and provider. The parent LLM invokes it
//! like any other tool, passing a natural-language `task` string.
//!
//! # Design
//!
//! - **Context isolation**: each invocation starts a fresh conversation
//! - **Depth limiting**: sub-agents are not given other SubAgentTools (static, no runtime counter)
//! - **Cancellation propagation**: the parent's cancel token is forwarded
//! - **Event forwarding**: sub-agent events stream to the parent via `on_update`
//!
//! # Example
//!
//! ```rust,no_run
//! use phi_core::agents::SubAgentTool;
//! use phi_core::provider::ModelConfig;
//!
//! let researcher = SubAgentTool::new(
//!     "researcher",
//!     ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", "sk-..."),
//! )
//! .with_description("Searches codebases and documents")
//! .with_system_prompt("You are a research assistant.");
//! ```

use crate::agent_loop::{agent_loop, AgentLoopConfig};
use crate::context::ExecutionLimits;
use crate::provider::{ModelConfig, StreamProvider};
use crate::types::*;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Default max turns for sub-agents (prevents runaway execution).
const DEFAULT_MAX_TURNS: usize = 10;

/// A tool that delegates work to a child agent loop.
///
/// When the parent LLM calls this tool, it spawns a fresh `agent_loop()` with
/// its own system prompt, tools, and provider. The sub-agent runs to completion
/// and its final text output is returned as the tool result.
pub struct SubAgentTool {
    tool_name: String,
    tool_description: String,
    system_prompt: String,
    model_config: ModelConfig,
    provider_override: Option<Arc<dyn StreamProvider>>,
    tools: Vec<Arc<dyn AgentTool>>,
    thinking_level: ThinkingLevel,
    max_tokens: Option<u32>,
    cache_config: CacheConfig,
    tool_execution: ToolExecutionStrategy,
    retry_config: crate::provider::retry::RetryConfig,
    max_turns: usize,
    /// The `loop_id` of the parent agent loop that spawned this sub-agent.
    /// Passed into the child context as `parent_loop_id` so that the full
    /// parent → child ancestry chain is traceable via `AgentStart` events.
    parent_loop_id: Option<String>,
}

impl SubAgentTool {
    /// Create a new sub-agent tool with a name and model config.
    pub fn new(name: impl Into<String>, model_config: ModelConfig) -> Self {
        let name = name.into();
        Self {
            tool_description: format!("Delegate a task to the '{}' sub-agent", name),
            tool_name: name,
            system_prompt: String::new(),
            model_config,
            provider_override: None,
            tools: Vec::new(),
            thinking_level: ThinkingLevel::Off,
            max_tokens: None,
            cache_config: CacheConfig::default(),
            tool_execution: ToolExecutionStrategy::default(),
            retry_config: crate::provider::retry::RetryConfig::default(),
            max_turns: DEFAULT_MAX_TURNS,
            parent_loop_id: None,
        }
    }

    /// Set the parent loop's `loop_id` for child → parent ancestry tracking.
    ///
    /// When set, this value is placed in the child `AgentContext.parent_loop_id`,
    /// which is then emitted in the child's `AgentStart` event. This creates a
    /// bidirectional link: the parent sees the child's `loop_id` via
    /// `ToolExecutionEnd.child_loop_id`, and the child records the parent via
    /// `AgentStart.parent_loop_id`.
    pub fn with_parent_loop_id(mut self, id: impl Into<String>) -> Self {
        self.parent_loop_id = Some(id.into());
        self
    }

    /// Override the provider used by this sub-agent, bypassing `ProviderRegistry` dispatch.
    /// Primarily used in tests to inject a `MockProvider`.
    pub fn with_provider_override(mut self, provider: Arc<dyn StreamProvider>) -> Self {
        self.provider_override = Some(provider);
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.tool_description = desc.into();
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn with_tools(mut self, tools: Vec<Arc<dyn AgentTool>>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_thinking(mut self, level: ThinkingLevel) -> Self {
        self.thinking_level = level;
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    pub fn with_cache_config(mut self, config: CacheConfig) -> Self {
        self.cache_config = config;
        self
    }

    pub fn with_tool_execution(mut self, strategy: ToolExecutionStrategy) -> Self {
        self.tool_execution = strategy;
        self
    }

    pub fn with_retry_config(mut self, config: crate::provider::retry::RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    pub fn with_max_turns(mut self, max: usize) -> Self {
        self.max_turns = max;
        self
    }
}

/*
Both `SubAgentTool.tools` and `AgentContext.tools` now use `Vec<Arc<dyn AgentTool>>`,
so tools can be passed directly — no adapter needed. Arc::clone on each tool just
increments the reference count (cheap), and the sub-agent's context shares the same
underlying tool instances as the parent.
*/

#[async_trait::async_trait]
impl AgentTool for SubAgentTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task to delegate to this sub-agent"
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value, // LLM INPUT — expects `{"task": "..."}` — the natural-language task to delegate
        ctx: ToolContext, // SYSTEM ENV — cancel token + on_update/on_progress from parent agent loop
    ) -> Result<ToolResult, ToolError> {
        let cancel = ctx.cancel; // forwarded to the child agent_loop as its abort signal
        let on_update = ctx.on_update; // forwarded to child for event fan-out (parent sees child progress)
        let on_progress = ctx.on_progress;
        /*
        RUST QUIRK: Chaining on serde_json::Value — `get()`, `and_then()`, `ok_or_else()`

        `params` is `serde_json::Value` — a dynamically typed JSON value.
        Extracting nested values requires a chain of Option-returning methods:

          .get("task")       → Option<&Value> (None if key absent)
          .and_then(|v| ...) → flatMap: if Some, apply f and return its Option; if None, stay None
          .as_str()          → Option<&str> (None if value is not a JSON string)
          .ok_or_else(|| ..) → convert Option → Result: None → Err(ToolError::...)
          ?                  → propagate the Err if still None
          .to_string()       → convert &str → owned String

        Python analogy:
          task = params.get("task")
          if not isinstance(task, str):
              raise ToolError("Missing required 'task' parameter")

        `.ok_or_else(|| ...)` uses a closure (lazy): the error is only constructed
        if we actually need it (i.e., when the Option is None). vs `.ok_or(error)` which
        eagerly constructs the error even when Ok — wasteful if construction is expensive.
        */
        // Extract the task parameter
        let task = params
            .get("task") // Option<&Value>
            .and_then(|v| v.as_str()) // Option<&str> — None if not a string
            .ok_or_else(|| ToolError::InvalidArgs("Missing required 'task' parameter".into()))?
            .to_string(); // &str → owned String

        // Clone Arc references — increments reference count, no deep copy.
        let tools: Vec<Arc<dyn AgentTool>> = self.tools.iter().map(Arc::clone).collect();

        // Generate stable identity for the child loop.
        // Each sub-agent invocation is its own independent session: fresh agent_id,
        // session_id, and loop_id. The parent's loop_id is carried as parent_loop_id
        // so the ancestry chain is traceable via AgentStart events.
        let child_agent_id = uuid::Uuid::new_v4().to_string();
        let child_session_id = uuid::Uuid::new_v4().to_string();
        // ".sub.1" — ".sub" marks this as a sub-agent loop (distinguishes from top-level loops
        // in the parent session), ".1" is the loop counter (fresh session → always starts at 1).
        let child_loop_id = format!("{}.sub.1", child_session_id);

        // Fresh context for the sub-agent
        let mut context = AgentContext {
            system_prompt: self.system_prompt.clone(),
            messages: Vec::new(),
            tools,
            agent_id: Some(child_agent_id),
            session_id: Some(child_session_id),
            loop_id: Some(child_loop_id),
            parent_loop_id: self.parent_loop_id.clone(), // links child back to parent
            continuation_kind: None,
            session: None,
            user_context: Vec::new(),
            inrun_context: Vec::new(),
        };

        // Config for the sub-agent loop
        let config = AgentLoopConfig {
            model_config: self.model_config.clone(),
            provider_override: self.provider_override.clone(),
            thinking_level: self.thinking_level,
            max_tokens: self.max_tokens,
            temperature: None,
            convert_to_llm: None,
            transform_context: None,
            get_steering_messages: None,
            get_follow_up_messages: None,
            context_config: None,
            execution_limits: Some(ExecutionLimits {
                max_turns: self.max_turns,
                // Generous token/duration limits — turn limit is the primary guard
                max_total_tokens: 1_000_000,
                max_duration: std::time::Duration::from_secs(300),
                max_cost: None,
            }),
            cache_config: self.cache_config.clone(),
            tool_execution: self.tool_execution.clone(),
            tool_timeout: None,
            retry_config: self.retry_config.clone(),
            before_turn: None,
            after_turn: None,
            before_loop: None,
            after_loop: None,
            before_tool_execution: None,
            after_tool_execution: None,
            before_tool_execution_update: None,
            after_tool_execution_update: None,
            before_compaction_start: None,
            after_compaction_end: None,
            on_error: None,
            input_filters: vec![],
            first_turn_trigger: TurnTrigger::SubAgent,
            config_id: None,
            context_translation: None,
            prun_pending: None,
        };

        /*
        RUST QUIRK: `tokio::spawn` — spawning a concurrent async task

        `tokio::spawn(async move { ... })` launches an async task that runs
        CONCURRENTLY with the current code. It returns a `JoinHandle<T>` —
        a handle you can `.await` to get the task's return value.

        `async move { ... }` — an async block that OWNS (moves) its captured values.
        The block is a "future" that gets polled by the tokio runtime.

        Why spawn a separate task for event forwarding?
        We need to RECEIVE events from `rx` while SIMULTANEOUSLY running `agent_loop()`.
        If we ran both sequentially, agent_loop() would block waiting for someone to drain rx
        (an unbounded channel will buffer, but we want real-time forwarding).
        By spawning a task, the event forwarding runs in parallel with the agent loop.

        Python analogy:
          asyncio.create_task(forward_events(rx, on_update, on_progress))
        */
        // Channel for sub-agent events
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Forward sub-agent events to parent via on_update and on_progress callbacks
        let forward_handle = if on_update.is_some() || on_progress.is_some() {
            let tool_name = self.tool_name.clone();
            Some(tokio::spawn(async move {
                // `while let Some(event) = rx.recv().await` — receive events until channel closes.
                // `rx.recv()` returns None when all senders (tx) are dropped.
                // When agent_loop() returns, it drops tx, which closes the channel, which breaks this loop.
                while let Some(event) = rx.recv().await {
                    // Forward progress messages via on_progress
                    if let AgentEvent::ProgressMessage { text, .. } = &event {
                        if let Some(ref cb) = on_progress {
                            cb(text.clone());
                        }
                    }

                    // Convert interesting events to ToolResult updates for the parent
                    if let Some(ref on_update) = on_update {
                        let update_text = match &event {
                            AgentEvent::MessageUpdate {
                                delta: StreamDelta::Text { delta },
                                ..
                            } => Some(delta.clone()),
                            AgentEvent::ToolExecutionStart { tool_name, .. } => {
                                Some(format!("[sub-agent calling tool: {}]", tool_name))
                            }
                            _ => None,
                        };

                        if let Some(text) = update_text {
                            on_update(ToolResult {
                                content: vec![Content::Text { text }],
                                details: serde_json::json!({ "sub_agent": tool_name }),
                                child_loop_id: None,
                            });
                        }
                    }
                }
            }))
        } else {
            None
        };

        // Run the sub-agent loop. We capture context.loop_id after the call to surface it
        // in ToolExecutionEnd.child_loop_id. The loop_id is already Some (we set it above);
        // agent_loop only writes it when None, so our value is preserved.
        let prompt = AgentMessage::Llm(LlmMessage::new(Message::user(task)));
        let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
        let returned_child_loop_id = context.loop_id.clone();

        /*
        RUST QUIRK: `let _ = handle.await` — explicitly discarding a Result

        `handle.await` returns `Result<(), JoinError>` — it can fail if the task panicked.
        `let _ = ...` explicitly ignores the result. This is idiomatic for "I don't care
        about this result" and suppresses the "unused Result" compiler warning.

        Why not just `handle.await.ok()`? Both work; `let _ =` is slightly more explicit
        about intentional discard. `handle.await?` would propagate the JoinError, but
        we're in execute() which returns ToolError, not JoinError — type mismatch.
        */
        // Wait for event forwarding to complete
        if let Some(handle) = forward_handle {
            let _ = handle.await; // wait for the spawned task to finish (ignoring panic errors)
        }

        // Extract final assistant text from the returned messages
        let result_text = extract_final_text(&new_messages);

        // Include full sub-agent conversation in details for debugging
        let details = serde_json::json!({
            "sub_agent": self.tool_name,
            "turns": new_messages.len(),
        });

        Ok(ToolResult {
            content: vec![Content::Text { text: result_text }],
            details,
            child_loop_id: returned_child_loop_id,
        })
    }
}

/// Extract the final assistant text from agent messages.
/// Collects text from the last assistant message, or returns a fallback.
fn extract_final_text(messages: &[AgentMessage]) -> String {
    for msg in messages.iter().rev() {
        if let AgentMessage::Llm(LlmMessage {
            message: Message::Assistant { content, .. },
            ..
        }) = msg
        {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            if !texts.is_empty() {
                return texts.join("\n");
            }
        }
    }
    "(sub-agent produced no text output)".to_string()
}
