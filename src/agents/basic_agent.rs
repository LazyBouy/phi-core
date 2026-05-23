//! The default in-memory `Agent` implementation.
//!
//! [`BasicAgent`] owns a single linear message history and runs the `agent_loop` directly.
//! It is the concrete type most callers will use. Configuration is done via the builder
//! pattern; the runtime interface is provided by the [`Agent`](super::Agent) trait.

use super::agent::{Agent, QueueMode};
use super::profile::AgentProfile;
use crate::agent_loop::{
    agent_loop, agent_loop_continue, AfterCompactionEndFn, AfterLoopFn, AfterToolExecutionFn,
    AfterToolExecutionUpdateFn, AfterTurnFn, AgentLoopConfig, BeforeCompactionStartFn,
    BeforeLoopFn, BeforeToolExecutionFn, BeforeToolExecutionUpdateFn, BeforeTurnFn, ConvertToLlmFn,
    OnErrorFn, TransformContextFn,
};
use crate::context::{CompactionStrategy, ContextConfig, ExecutionLimits};
use crate::mcp::{McpClient, McpError, McpToolAdapter};
use crate::provider::context_translation::ContextTranslationStrategy;
use crate::provider::{ModelConfig, StreamProvider};
use crate::types::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Acquire a `Mutex<Vec<T>>` guard tolerating poisoning.
///
/// `Mutex` poisoning happens when a thread panics while holding the guard.
/// The steering and follow-up queues are recoverable data — a panic in a hook
/// or tool callback should not crash the entire agent session. We log a warning
/// and recover the inner `Vec` via `PoisonError::into_inner()`.
fn lock_queue<T>(q: &Mutex<Vec<T>>) -> std::sync::MutexGuard<'_, Vec<T>> {
    match q.lock() {
        Ok(g) => g,
        Err(poison) => {
            tracing::warn!(
                "BasicAgent: queue mutex was poisoned; recovering inner Vec. \
                 A prior hook or tool callback panicked while holding the lock."
            );
            poison.into_inner()
        }
    }
}

/*
ARCHITECTURE: BasicAgent vs agent_loop — stateful wrapper vs stateless functions

The agent loop (agent_loop.rs) is a set of FREE FUNCTIONS — they take all their
inputs as parameters and return outputs. They have no hidden state.

The BasicAgent struct is an OPTIONAL stateful wrapper that owns:
  - Message history (Vec<AgentMessage>) — the conversation so far
  - Tools (Vec<Arc<dyn AgentTool>>) — registered capabilities
  - ModelConfig — complete provider identity: id, api_key, base_url, api protocol, cost rates
  - Steering/follow-up queues (Arc<Mutex<>>) — for mid-run interrupts

Why this separation?
  - Free functions: easier to test, compose, and reason about
  - BasicAgent struct: easier to use in applications (less boilerplate)
  - You can use agent_loop() directly if you need more control

The BasicAgent uses the BUILDER PATTERN for construction:
  BasicAgent::new(ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &key))
      .with_system_prompt("...")
      .with_tools(vec![...])

Each `with_*` method takes `mut self` and returns `Self` — consuming and
returning the same value. This chains naturally and avoids separate calls.
Python analogy: it's like a fluent API but ownership-safe.
*/

/// Reference implementation of the [`Agent`] trait.
///
/// Custom agents should implement the `Agent` trait directly. New generic agent
/// methods should be defined on the `Agent` trait first, then implemented here —
/// never add public methods to `BasicAgent` without the corresponding trait method.
///
/// Configuration is done via the builder pattern before any prompting. The runtime
/// interface (prompting, state access, control) is provided via the [`Agent`] trait.
pub struct BasicAgent {
    // -- Public configuration (readable/overridable externally) --
    pub system_prompt: String,
    pub model_config: ModelConfig, // complete provider identity: model id, api_key, base_url, cost rates
    /// Optional provider override. When set, bypasses `ProviderRegistry` dispatch.
    /// Used primarily in tests to inject a `MockProvider`.
    pub provider_override: Option<Arc<dyn StreamProvider>>,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>,  // None = use model_config.max_tokens
    pub temperature: Option<f32>, // None = use provider default

    // -- Private configuration (only mutated via builder methods) --
    messages: Vec<AgentMessage>, // full conversation history

    /*
    RUST QUIRK: `Arc<dyn Trait>` — shared trait objects

    `Arc<dyn AgentTool>` means: "I have shared (reference-counted) ownership of a
    heap-allocated value of some type that implements AgentTool."

    Why Arc instead of Box?
    Arc allows the same tool to be shared across parallel agent branches without copying.
    `AgentContext` clones (used for evaluational parallelism) increment each Arc's
    reference count — zero-cost for tools. Tools are immutable during execution
    (execute takes &self), so Arc sharing is semantically correct.

    Python analogy: tools are shared objects — multiple agents can reference the same
    tool instance without transferring ownership.
    */
    tools: Vec<Arc<dyn AgentTool>>,

    /*
    RUST QUIRK: `Arc<Mutex<Vec<AgentMessage>>>` — shared mutable state across threads

    This is the canonical Rust pattern for "I need to mutate this from multiple places."

    Arc  = Atomically Reference Counted — shared ownership (multiple holders, thread-safe)
    Mutex = Mutual Exclusion — only one thread can access the inner value at a time

    The BasicAgent OWNS the queues (Arc keeps them alive as long as BasicAgent is alive).
    The agent loop USES the queues via the closures in build_config() — those closures
    clone the Arc (incrementing the reference count) and lock the Mutex to read/drain.

    Python analogy: threading.Lock() wrapping a shared list, passed to threads via closure.

    Why Arc instead of Rc?
    Rc (Reference Counted) is NOT thread-safe. Since tokio runs on a thread pool,
    closures may execute on any thread, so we need Arc (atomic = thread-safe).

    Queue access goes through `lock_queue()` (see top of file) which tolerates
    poisoning — a panic in a hook or tool callback logs a warning and recovers the
    inner `Vec` rather than crashing the agent session. Poisoning still indicates a
    bug upstream; we surface it via `tracing::warn!`.
    */
    steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,

    // Context, limits & caching
    pub context_config: Option<ContextConfig>,
    pub execution_limits: Option<ExecutionLimits>,
    pub cache_config: CacheConfig,
    pub tool_execution: ToolExecutionStrategy,
    pub tool_timeout: Option<std::time::Duration>,
    pub response_format: crate::provider::ResponseFormat,
    pub retry_config: crate::provider::retry::RetryConfig,

    // Lifecycle callbacks
    before_turn: Option<BeforeTurnFn>,
    after_turn: Option<AfterTurnFn>,
    on_error: Option<OnErrorFn>,

    // Input filters
    input_filters: Vec<Arc<dyn InputFilter>>,

    // ── Hook/callback fields (wired into build_config) ──────────────────
    before_loop: Option<BeforeLoopFn>,
    after_loop: Option<AfterLoopFn>,
    before_tool_execution: Option<BeforeToolExecutionFn>,
    after_tool_execution: Option<AfterToolExecutionFn>,
    before_tool_execution_update: Option<BeforeToolExecutionUpdateFn>,
    after_tool_execution_update: Option<AfterToolExecutionUpdateFn>,
    convert_to_llm: Option<ConvertToLlmFn>,
    transform_context: Option<TransformContextFn>,
    before_compaction_start: Option<BeforeCompactionStartFn>,
    after_compaction_end: Option<AfterCompactionEndFn>,
    context_translation: Option<Arc<dyn ContextTranslationStrategy>>,
    prun_pending: Option<Arc<Mutex<Vec<crate::tools::prun::PrunRequest>>>>,
    revert_pending: Option<Arc<Mutex<Vec<crate::tools::revert::RevertRequest>>>>,

    // ── Profile, config identity, and workspace ──────────────────────────
    config_id: Option<String>,
    profile: Option<AgentProfile>,
    workspace: Option<std::path::PathBuf>,

    // Control — cancel token is Some during a streaming call, None otherwise
    cancel: Option<CancellationToken>,
    is_streaming: bool, // guard against concurrent prompt() calls

    // ── Session identity ─────────────────────────────────────────────────────
    // These fields give every loop call within this BasicAgent a consistent, traceable identity.
    // agent_id and session_id are generated once at BasicAgent::new() and threaded into every
    // AgentContext built by this BasicAgent.
    //
    // loop_counters: HashMap keyed by "{session_id}.{effective_config_id}" — each unique
    // (session, config) combination has its own monotonic counter, so loop IDs self-document
    // which config produced them:
    //   ses_xyz.anthropic.claude-opus-4.1   ← first claude loop
    //   ses_xyz.openai.gpt-4o.1             ← first openai loop (independent counter)
    //   ses_xyz.anthropic.claude-opus-4.2   ← second claude loop
    //
    // last_loop_id: tracks the most recently started loop so agent_loop_continue() can
    // set parent_loop_id automatically, enabling ancestry tracking across reruns/branches.
    //
    /* ROADMAP — future session/identity capabilities:
       - HITL resume: user cancels mid-run, reviews, resumes → use continue_loop_with_sender(Rerun|Branch)
       - Checkpoint restore: context serialised to disk, later restored → continue_loop_with_sender(Branch)
       - Parallel exploration: multiple branches from same checkpoint, concurrent →
             multiple concurrent continue_loop_with_sender(Branch) calls in the same session
       - Auto origin/continue selection: inspect last message role → if ToolResult, auto-continue
       - Sub-agent parent linking (automatic): BasicAgent::with_sub_agent() could auto-pass
             self.last_loop_id as parent_loop_id to SubAgentTool; currently requires
             manual wiring via SubAgentTool::with_parent_loop_id()
    */
    agent_id: String,
    session_id: String,
    loop_counters: HashMap<String, usize>,
    last_loop_id: Option<String>,
    /// Timestamp of the most recent `prompt_messages_with_sender` call.
    /// Used by [`check_and_rotate`][BasicAgent::check_and_rotate] to detect inactivity.
    last_active_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional session for block-based compaction.
    session: Option<crate::session::Session>,
}

impl BasicAgent {
    pub fn new(model_config: ModelConfig) -> Self {
        Self {
            model_config,
            provider_override: None,
            system_prompt: String::new(),
            thinking_level: ThinkingLevel::Off,
            max_tokens: None,
            temperature: None,
            messages: Vec::new(),
            tools: Vec::new(),
            steering_queue: Arc::new(Mutex::new(Vec::new())), // empty, shared with closures
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            context_config: Some(ContextConfig::default()), // enabled by default
            execution_limits: Some(ExecutionLimits::default()), // enabled by default
            cache_config: CacheConfig::default(),
            tool_execution: ToolExecutionStrategy::default(), // Parallel
            tool_timeout: None,
            response_format: crate::provider::ResponseFormat::Text,
            retry_config: crate::provider::retry::RetryConfig::default(), // 3 retries
            before_turn: None,
            after_turn: None,
            on_error: None,
            input_filters: Vec::new(),
            before_loop: None,
            after_loop: None,
            before_tool_execution: None,
            after_tool_execution: None,
            before_tool_execution_update: None,
            after_tool_execution_update: None,
            convert_to_llm: None,
            transform_context: None,
            before_compaction_start: None,
            after_compaction_end: None,
            context_translation: None,
            prun_pending: None,
            revert_pending: None,
            config_id: None,
            profile: None,
            workspace: None,
            cancel: None,
            is_streaming: false,
            agent_id: uuid::Uuid::new_v4().to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            loop_counters: HashMap::new(),
            last_loop_id: None,
            last_active_at: None,
            session: None,
        }
    }

    /// Set a session for block-based compaction.
    pub fn with_session(mut self, session: crate::session::Session) -> Self {
        self.session = Some(session);
        self
    }

    /// Take the session out of the agent, returning ownership.
    pub fn take_session(&mut self) -> Option<crate::session::Session> {
        self.session.take()
    }

    /*
    RUST QUIRK: Builder pattern — `mut self` + return `Self`

    Builder methods take OWNERSHIP of `self` (consume the BasicAgent), modify it, then
    return it. This allows chaining:
      BasicAgent::new(p).with_model("x").with_tools(vec![...])

    `mut self` — self is moved in (consumed), marked mutable for modification.
    `self` in the return — move the (now mutated) value back to the caller.

    NO CLONE IS MADE — ownership transfers in, gets mutated, transfers out.
    This is zero-cost: just a stack value being modified in place.

    Contrast with Python where you'd either mutate self in-place (returning None)
    OR create a copy. Rust's builder pattern gives you chaining WITH ownership safety.
    */

    // -- Builder-style setters --

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn with_thinking(mut self, level: ThinkingLevel) -> Self {
        self.thinking_level = level;
        self
    }

    pub fn with_tools(mut self, tools: Vec<Arc<dyn AgentTool>>) -> Self {
        self.tools = tools;
        self
    }

    /// Read-only view of the currently registered tools. Useful for tests that
    /// assert the LLM-facing tool registry (e.g. the Composition I opt-in
    /// guarantee — `revert_to_state` must NOT appear without an explicit
    /// `with_revert_tool()` call).
    pub fn tools(&self) -> &[Arc<dyn AgentTool>] {
        &self.tools
    }

    pub fn with_model_config(mut self, config: ModelConfig) -> Self {
        self.model_config = config;
        self
    }

    /// Override the provider used by the agent loop, bypassing `ProviderRegistry` dispatch.
    /// Primarily used in tests to inject a `MockProvider`.
    pub fn with_provider_override(mut self, provider: Arc<dyn StreamProvider>) -> Self {
        self.provider_override = Some(provider);
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    pub fn with_context_config(mut self, config: ContextConfig) -> Self {
        self.context_config = Some(config);
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

    /// Set the per-tool execution timeout. See [`AgentLoopConfig::tool_timeout`].
    pub fn with_tool_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.tool_timeout = Some(timeout);
        self
    }

    /// Set the desired LLM output shape. See [`crate::provider::ResponseFormat`].
    pub fn with_response_format(mut self, format: crate::provider::ResponseFormat) -> Self {
        self.response_format = format;
        self
    }

    pub fn with_retry_config(mut self, config: crate::provider::retry::RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Load skills and append their index to the system prompt.
    ///
    /// The skills index is appended as XML per the [AgentSkills standard](https://agentskills.io).
    /// The agent can then read individual SKILL.md files using the `read_file` tool
    /// when it decides a skill is relevant.
    pub fn with_skills(mut self, skills: crate::context::skills::SkillSet) -> Self {
        let prompt_fragment = skills.format_for_prompt();
        if !prompt_fragment.is_empty() {
            if self.system_prompt.is_empty() {
                self.system_prompt = prompt_fragment;
            } else {
                self.system_prompt = format!("{}\n\n{}", self.system_prompt, prompt_fragment);
            }
        }
        self
    }

    pub fn with_execution_limits(mut self, limits: ExecutionLimits) -> Self {
        self.execution_limits = Some(limits);
        self
    }

    pub fn with_messages(mut self, msgs: Vec<AgentMessage>) -> Self {
        self.messages = msgs;
        self
    }

    /*
    RUST QUIRK: `impl Fn(...) + Send + Sync + 'static` — accepting a callable

    This accepts ANY callable (closure or function) that:
      - Takes (&[AgentMessage], usize) and returns bool   ← the Fn signature
      - Is safe to call from another thread               ← Send + Sync
      - Doesn't borrow from the local stack               ← 'static

    Why not just `Box<dyn Fn(...)>`? Because the compiler can inline impl Fn
    at the call site (monomorphization), while dyn Fn always goes through a vtable.
    Both work; impl Fn is faster for one-time construction.

    `Arc::new(f)` — wrap in Arc so it can be cloned cheaply into each AgentLoopConfig.
    The Arc's type becomes Arc<dyn Fn(...)> (the BeforeTurnFn type alias).
    */
    pub fn on_before_turn(
        mut self,
        f: impl Fn(&[AgentMessage], usize) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.before_turn = Some(Arc::new(f));
        self
    }

    pub fn on_after_turn(
        mut self,
        f: impl Fn(&[AgentMessage], &Usage) + Send + Sync + 'static,
    ) -> Self {
        self.after_turn = Some(Arc::new(f));
        self
    }

    pub fn on_error(mut self, f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_error = Some(Arc::new(f));
        self
    }

    /// Add an input filter. Filters run in order on user messages before the LLM call.
    pub fn with_input_filter(mut self, filter: impl InputFilter + 'static) -> Self {
        self.input_filters.push(Arc::new(filter));
        self
    }

    /// Set a custom in-memory compaction strategy on the context config.
    /// When set, replaces `DefaultCompaction` during context compaction
    /// for sessionless runs. (G5: stored on CompactionConfig, not BasicAgent.)
    pub fn with_compaction_strategy(mut self, strategy: impl CompactionStrategy + 'static) -> Self {
        if let Some(ref mut ctx) = self.context_config {
            ctx.compaction.in_memory_strategy = Some(Arc::new(strategy));
        }
        self
    }

    /// Set the agent profile blueprint. Also copies profile fields into this agent's
    /// public fields for backward compatibility (profile values act as defaults;
    /// existing field values take precedence if already set).
    pub fn with_profile(mut self, profile: AgentProfile) -> Self {
        // Copy profile defaults into pub fields (only if not already set)
        if let Some(ref prompt) = profile.system_prompt {
            if self.system_prompt.is_empty() {
                self.system_prompt = prompt.clone();
            }
        }
        if let Some(level) = profile.thinking_level {
            if self.thinking_level == ThinkingLevel::Off {
                self.thinking_level = level;
            }
        }
        if let Some(temp) = profile.temperature {
            if self.temperature.is_none() {
                self.temperature = Some(temp);
            }
        }
        if let Some(max) = profile.max_tokens {
            if self.max_tokens.is_none() {
                self.max_tokens = Some(max);
            }
        }
        if let Some(ref id) = profile.config_id {
            if self.config_id.is_none() {
                self.config_id = Some(id.clone());
            }
        }
        self.profile = Some(profile);
        self
    }

    /// Set the temperature for LLM calls.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set the config identity, used as the middle segment of `loop_id`.
    pub fn with_config_id(mut self, id: impl Into<String>) -> Self {
        self.config_id = Some(id.into());
        self
    }

    /// Set the agent workspace directory. File paths in system prompt blocks
    /// resolve relative to this directory.
    pub fn with_workspace(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.workspace = Some(path.into());
        self
    }

    /// Set the before-loop hook. Return `false` to abort the loop.
    pub fn on_before_loop(
        mut self,
        f: impl Fn(&[AgentMessage], usize) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.before_loop = Some(Arc::new(f));
        self
    }

    /// Set the after-loop hook.
    pub fn on_after_loop(
        mut self,
        f: impl Fn(&[AgentMessage], &Usage) + Send + Sync + 'static,
    ) -> Self {
        self.after_loop = Some(Arc::new(f));
        self
    }

    /// Set the before-tool-execution hook. Return `false` to skip the tool call.
    pub fn on_before_tool_execution(
        mut self,
        f: impl Fn(&str, &str, &serde_json::Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.before_tool_execution = Some(Arc::new(f));
        self
    }

    /// Set the after-tool-execution hook.
    pub fn on_after_tool_execution(
        mut self,
        f: impl Fn(&str, &str, bool) + Send + Sync + 'static,
    ) -> Self {
        self.after_tool_execution = Some(Arc::new(f));
        self
    }

    /// Set the before-tool-execution-update hook. Return `false` to suppress the event.
    pub fn on_before_tool_execution_update(
        mut self,
        f: impl Fn(&str, &str, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.before_tool_execution_update = Some(Arc::new(f));
        self
    }

    /// Set the after-tool-execution-update hook.
    pub fn on_after_tool_execution_update(
        mut self,
        f: impl Fn(&str, &str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.after_tool_execution_update = Some(Arc::new(f));
        self
    }

    /// Enable the prun tool (both `prun` and `prun_with_memo` variants).
    /// Adds both tool variants to the tool set and wires up the shared pending queue.
    pub fn with_prun_tool(mut self) -> Self {
        let pending = Arc::new(Mutex::new(Vec::new()));
        self.tools.push(Arc::new(crate::tools::PrunTool::new(
            pending.clone(),
            crate::tools::PrunVariant::Prun,
        )));
        self.tools.push(Arc::new(crate::tools::PrunTool::new(
            pending.clone(),
            crate::tools::PrunVariant::PrunWithMemo,
        )));
        self.prun_pending = Some(pending);
        self
    }

    /// Enable the `revert_to_state` tool (Composition I braking layer).
    ///
    /// Registers a [`RevertTool`](crate::tools::RevertTool) on the agent and
    /// wires the shared `revert_pending` queue into the loop config so
    /// `apply_revert` runs between turns. The opt-in guarantee — there is no
    /// other registration path — is the load-bearing safety invariant for
    /// downstream consumers that have not yet adopted Composition I.
    pub fn with_revert_tool(mut self) -> Self {
        let pending = Arc::new(Mutex::new(Vec::new()));
        self.tools
            .push(Arc::new(crate::tools::RevertTool::new(pending.clone())));
        self.revert_pending = Some(pending);
        self
    }

    /// Set a custom convert-to-LLM function.
    pub fn with_convert_to_llm(
        mut self,
        f: impl Fn(&[AgentMessage]) -> Vec<Message> + Send + Sync + 'static,
    ) -> Self {
        self.convert_to_llm = Some(Arc::new(f));
        self
    }

    /// Set a custom transform-context function.
    pub fn with_transform_context(
        mut self,
        f: impl Fn(Vec<AgentMessage>) -> Vec<AgentMessage> + Send + Sync + 'static,
    ) -> Self {
        self.transform_context = Some(Arc::new(f));
        self
    }

    /// Set a custom block compaction strategy for Session-aware compaction.
    /// (G5: stored on CompactionConfig, not BasicAgent.)
    pub fn with_block_compaction_strategy(
        mut self,
        strategy: impl crate::context::BlockCompactionStrategy + 'static,
    ) -> Self {
        if let Some(ref mut ctx) = self.context_config {
            ctx.compaction.block_strategy = Some(Arc::new(strategy));
        }
        self
    }

    /// Set the before-compaction-start hook (G1). Return `false` to skip compaction.
    pub fn on_before_compaction_start(
        mut self,
        f: impl Fn(usize, usize) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.before_compaction_start = Some(Arc::new(f));
        self
    }

    /// Set the after-compaction-end hook (G1).
    pub fn on_after_compaction_end(
        mut self,
        f: impl Fn(usize, usize, usize, usize) + Send + Sync + 'static,
    ) -> Self {
        self.after_compaction_end = Some(Arc::new(f));
        self
    }

    /// Set the context translation strategy (G8) for cross-provider compatibility.
    pub fn with_context_translation(
        mut self,
        strategy: Arc<dyn ContextTranslationStrategy>,
    ) -> Self {
        self.context_translation = Some(strategy);
        self
    }

    /// Add a sub-agent tool. The sub-agent runs its own `agent_loop()` when invoked.
    pub fn with_sub_agent(mut self, sub: crate::agents::SubAgentTool) -> Self {
        self.tools.push(Arc::new(sub));
        self
    }

    /// Disable automatic context compaction
    pub fn without_context_management(mut self) -> Self {
        self.context_config = None;
        self.execution_limits = None;
        self
    }

    // -- OpenAPI integration --

    /// Load tools from an OpenAPI spec file and add them to the agent.
    #[cfg(feature = "openapi")]
    pub async fn with_openapi_file(
        mut self,
        path: impl AsRef<std::path::Path>,
        config: crate::openapi::OpenApiConfig,
        filter: &crate::openapi::OperationFilter,
    ) -> Result<Self, crate::openapi::OpenApiError> {
        let adapters = crate::openapi::OpenApiToolAdapter::from_file(path, config, filter).await?;
        for adapter in adapters {
            self.tools.push(Arc::new(adapter));
        }
        Ok(self)
    }

    /// Fetch an OpenAPI spec from a URL and add its tools to the agent.
    #[cfg(feature = "openapi")]
    pub async fn with_openapi_url(
        mut self,
        url: &str,
        config: crate::openapi::OpenApiConfig,
        filter: &crate::openapi::OperationFilter,
    ) -> Result<Self, crate::openapi::OpenApiError> {
        let adapters = crate::openapi::OpenApiToolAdapter::from_url(url, config, filter).await?;
        for adapter in adapters {
            self.tools.push(Arc::new(adapter));
        }
        Ok(self)
    }

    /// Parse an OpenAPI spec string and add its tools to the agent.
    #[cfg(feature = "openapi")]
    pub fn with_openapi_spec(
        mut self,
        spec_str: &str,
        config: crate::openapi::OpenApiConfig,
        filter: &crate::openapi::OperationFilter,
    ) -> Result<Self, crate::openapi::OpenApiError> {
        let adapters = crate::openapi::OpenApiToolAdapter::from_str(spec_str, config, filter)?;
        for adapter in adapters {
            self.tools.push(Arc::new(adapter));
        }
        Ok(self)
    }

    // -- MCP integration --

    /// Connect to an MCP server via stdio and add its tools to the agent.
    pub async fn with_mcp_server_stdio(
        mut self,
        command: &str, // EXECUTABLE — path or name of the MCP server binary (e.g. "npx", "python")
        args: &[&str], // ARGV — command-line arguments to the binary (e.g. &["-y", "@my/mcp"])
        env: Option<HashMap<String, String>>, // ENV OVERRIDES — extra env vars for the child process; None = inherit parent env
    ) -> Result<Self, McpError> {
        let client = McpClient::connect_stdio(command, args, env).await?;
        let client = Arc::new(tokio::sync::Mutex::new(client));
        let adapters = McpToolAdapter::from_client(client).await?;
        for adapter in adapters {
            self.tools.push(Arc::new(adapter));
        }
        Ok(self)
    }

    /// Connect to an MCP server via HTTP and add its tools to the agent.
    pub async fn with_mcp_server_http(mut self, url: &str) -> Result<Self, McpError> {
        let client = McpClient::connect_http(url).await?;
        let client = Arc::new(tokio::sync::Mutex::new(client));
        let adapters = McpToolAdapter::from_client(client).await?;
        for adapter in adapters {
            self.tools.push(Arc::new(adapter));
        }
        Ok(self)
    }

    // -- Ergonomic prompting wrappers --
    // These inherent methods accept `impl Into<String>` so callers can pass `&str` directly.
    // All other runtime methods (state, mutation, control, queues) are provided solely by
    // the `Agent` trait impl below — import `use phi_core::Agent` (or `use phi_core::*`)
    // to call them on a concrete `BasicAgent`.

    /// Send a text prompt. Returns a stream of `AgentEvent`s.
    ///
    /// Accepts `impl Into<String>` (e.g. `&str`). The trait's [`Agent::prompt`] default
    /// requires an owned `String`; use this inherent method to pass `&str` without `.to_string()`.
    pub async fn prompt(&mut self, text: impl Into<String>) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let msg = AgentMessage::Llm(LlmMessage::new(Message::user(text)));
        self.prompt_messages_with_sender(vec![msg], tx).await;
        rx
    }

    /// Send a text prompt, streaming events to a caller-provided sender.
    ///
    /// Accepts `impl Into<String>` (e.g. `&str`).
    pub async fn prompt_with_sender(
        &mut self,
        text: impl Into<String>,
        tx: mpsc::UnboundedSender<AgentEvent>,
    ) {
        let msg = AgentMessage::Llm(LlmMessage::new(Message::user(text)));
        self.prompt_messages_with_sender(vec![msg], tx).await;
    }

    // -- Internal --

    /*
    next_loop_id — derive the next loop_id for this config within this session.

    DESIGN: loop_id encodes which config produced the loop, making identity self-documenting.
      Format: "{session_id}.{effective_config_id}.{N}"
      effective_config_id = config.config_id if set, else "{provider_id}.{model_slug}[.thinking]"

    COUNTER: HashMap keyed by "{session_id}.{effective_config_id}".
    Each unique (session, config) pair has its own counter — so two different configs
    in the same session get independent counters (both start at .1), while two calls
    with the same config get sequential numbers (.1, .2, .3).

    SLUG: Non-alphanumeric chars in the model name are replaced with '-' so the loop_id
    is a clean, URL-safe identifier. E.g. "claude-opus-4.5" → "claude-opus-4-5".
    Hyphens are kept as-is (they're valid slug separators).
    */
    fn next_loop_id(&mut self, config: &AgentLoopConfig) -> String {
        let effective_config_id = if let Some(ref id) = config.config_id {
            id.clone()
        } else {
            let slugify = |s: &str| -> String {
                s.chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' {
                            c
                        } else {
                            '-'
                        }
                    })
                    .collect()
            };
            let thinking_part = if config.thinking_level != ThinkingLevel::Off {
                ".thinking"
            } else {
                ""
            };
            format!(
                "{}.{}{}",
                config.model_config.provider,
                slugify(&config.model_config.id),
                thinking_part
            )
        };
        let thread_key = format!("{}.{}", self.session_id, effective_config_id);
        let n = self.loop_counters.entry(thread_key.clone()).or_insert(0);
        *n += 1;
        format!("{}.{}", thread_key, n)
    }

    /*
    build_config — assemble AgentLoopConfig from BasicAgent's current state.

    ARCHITECTURE: Why a separate build_config() method?

    AgentLoopConfig is the "parameter bundle" for the stateless agent_loop() function.
    build_config() constructs it fresh each call — it's not stored on BasicAgent.
    This means: AgentLoopConfig borrows from BasicAgent (hence the lifetime `'_`),
    and both share the same Arc<Mutex<>> queues via clone (cheap, no allocation).

    RUST QUIRK: `move` closures for the queue callbacks

    The steering/follow-up closures need to outlive build_config()'s stack frame
    (they're stored in AgentLoopConfig and called later by the agent loop).
    So they use `move` to capture `steering_queue` (Arc clone) and `steering_mode` (Copy).

    We clone the Arc before the move:
      let steering_queue = self.steering_queue.clone();
    This gives the closure its own Arc reference to the same underlying Mutex.
    The BasicAgent still holds its own Arc reference. Both are valid simultaneously.

    `self.provider.clone()` — clone the Arc:
      self.provider is Arc<dyn StreamProvider>
      .clone() bumps the reference count — cheap, no data duplication
    Both BasicAgent and AgentLoopConfig now share ownership of the same underlying provider.
    */

    // ── Standalone compaction API ────────────────────────────────────────

    /// Run block-based compaction on the agent's session and emit the full event lifecycle.
    ///
    /// Emits: `AgentStart(Compaction)` → `CompactionStarted` → `CompactionEnded` → `AgentEnd`.
    ///
    /// Requires `self.session` to be `Some` and `self.context_config` to be `Some`.
    /// Panics if either is missing.
    /// No-op if `self.session` or `self.context_config` is `None`.
    pub fn compact_context_with_sender(
        &mut self,
        tx: &tokio::sync::mpsc::UnboundedSender<AgentEvent>,
    ) {
        let (Some(session), Some(ctx_config)) =
            (self.session.as_mut(), self.context_config.as_ref())
        else {
            return; // No session or config — nothing to compact
        };
        let comp = &ctx_config.compaction;
        let max_tokens = ctx_config.max_context_tokens;

        let loop_id = self
            .last_loop_id
            .clone()
            .unwrap_or_else(|| "compaction".to_string());

        let _ = tx.send(AgentEvent::AgentStart {
            agent_id: self.agent_id.clone(),
            session_id: self.session_id.clone(),
            loop_id: loop_id.clone(),
            parent_loop_id: self.last_loop_id.clone(),
            continuation_kind: ContinuationKind::Compaction,
            timestamp: chrono::Utc::now(),
            metadata: None,
            config_snapshot: None, // Compaction pass — no LLM config relevant
        });

        let msgs_before = self.messages.len();
        let tokens_before = crate::context::total_tokens(&self.messages);

        let _ = tx.send(AgentEvent::CompactionStarted {
            loop_id: loop_id.clone(),
            estimated_tokens: tokens_before,
            message_count: msgs_before,
            timestamp: chrono::Utc::now(),
        });

        let strategy: &dyn crate::context::BlockCompactionStrategy =
            &crate::context::DefaultBlockCompaction;
        let current_lid = self.last_loop_id.as_deref().unwrap_or("");

        // Sync messages into the current loop record
        if let Some(record) = session.get_loop_mut(current_lid) {
            record.messages = self.messages.clone();
        }

        crate::context::compact_session_loops(
            session,
            current_lid,
            strategy,
            comp,
            max_tokens,
            None,
        );
        self.messages = crate::context::build_context_from_session(
            session,
            current_lid,
            comp,
            max_tokens,
            None,
        );

        let msgs_after = self.messages.len();
        let tokens_after = crate::context::total_tokens(&self.messages);
        let chain = session.loop_chain_to(current_lid);
        let loops_compacted = chain
            .iter()
            .filter(|lid| {
                session
                    .get_loop(lid)
                    .map(|r| r.compaction_block.is_some())
                    .unwrap_or(false)
            })
            .count();

        let _ = tx.send(AgentEvent::CompactionEnded {
            loop_id: loop_id.clone(),
            messages_before: msgs_before,
            messages_after: msgs_after,
            estimated_tokens_before: tokens_before,
            estimated_tokens_after: tokens_after,
            loops_compacted,
            timestamp: chrono::Utc::now(),
        });

        let _ = tx.send(AgentEvent::AgentEnd {
            loop_id,
            messages: vec![],
            usage: Usage::default(),
            timestamp: chrono::Utc::now(),
            rejection: None,
        });
    }

    /// Fire-and-forget compaction. Returns the number of loops that received
    /// new `CompactionBlock`s.
    ///
    /// Requires `self.session` to be `Some` and `self.context_config` to be `Some`.
    /// Returns 0 if `self.session` or `self.context_config` is `None`.
    pub fn compact_context(&mut self) -> usize {
        let (Some(session), Some(ctx_config)) =
            (self.session.as_mut(), self.context_config.as_ref())
        else {
            return 0; // No session or config — nothing to compact
        };
        let comp = &ctx_config.compaction;
        let max_tokens = ctx_config.max_context_tokens;

        let strategy: &dyn crate::context::BlockCompactionStrategy =
            &crate::context::DefaultBlockCompaction;
        let current_lid = self.last_loop_id.as_deref().unwrap_or("");

        if let Some(record) = session.get_loop_mut(current_lid) {
            record.messages = self.messages.clone();
        }

        crate::context::compact_session_loops(
            session,
            current_lid,
            strategy,
            comp,
            max_tokens,
            None,
        );
        self.messages = crate::context::build_context_from_session(
            session,
            current_lid,
            comp,
            max_tokens,
            None,
        );

        let chain = session.loop_chain_to(current_lid);
        chain
            .iter()
            .filter(|lid| {
                session
                    .get_loop(lid)
                    .map(|r| r.compaction_block.is_some())
                    .unwrap_or(false)
            })
            .count()
    }

    // -- Internal --

    pub fn build_config(&self) -> Result<AgentLoopConfig, super::agent::AgentBuildError> {
        // Clone Arc handles before the move closures capture them
        let steering_queue = self.steering_queue.clone(); // cheap Arc clone
        let steering_mode = self.steering_mode; // Copy — no clone needed

        let follow_up_queue = self.follow_up_queue.clone();
        let follow_up_mode = self.follow_up_mode;

        // BasicAgent's constructor requires a `ModelConfig`, so this branch is
        // unreachable — wrap in Ok unconditionally. The Result is in the trait
        // signature for the benefit of custom Agent implementors that may not
        // have a model_config.
        Ok(AgentLoopConfig {
            model_config: self.model_config.clone(),
            provider_override: self.provider_override.clone(),
            thinking_level: self.thinking_level,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            convert_to_llm: self.convert_to_llm.clone(),
            transform_context: self.transform_context.clone(),
            get_steering_messages: Some(Box::new(move || {
                // This closure runs each time the agent loop checks for steering messages.
                // `move` captured: steering_queue (Arc clone), steering_mode (Copy)
                let mut queue = lock_queue(&steering_queue); // poison-tolerant lock
                match steering_mode {
                    QueueMode::OneAtATime => {
                        if queue.is_empty() {
                            vec![]
                        } else {
                            vec![queue.remove(0)] // remove and return first element
                        }
                    }
                    QueueMode::All => queue.drain(..).collect(), // drain all and return
                }
            })),
            context_config: self.context_config.clone(),
            execution_limits: self.execution_limits.clone(),
            cache_config: self.cache_config.clone(),
            tool_execution: self.tool_execution.clone(),
            tool_timeout: self.tool_timeout,
            response_format: self.response_format.clone(),
            retry_config: self.retry_config.clone(),
            get_follow_up_messages: Some(Box::new(move || {
                let mut queue = lock_queue(&follow_up_queue);
                match follow_up_mode {
                    QueueMode::OneAtATime => {
                        if queue.is_empty() {
                            vec![]
                        } else {
                            vec![queue.remove(0)]
                        }
                    }
                    QueueMode::All => queue.drain(..).collect(),
                }
            })),
            before_turn: self.before_turn.clone(),
            after_turn: self.after_turn.clone(),
            before_loop: self.before_loop.clone(),
            after_loop: self.after_loop.clone(),
            before_tool_execution: self.before_tool_execution.clone(),
            after_tool_execution: self.after_tool_execution.clone(),
            before_tool_execution_update: self.before_tool_execution_update.clone(),
            after_tool_execution_update: self.after_tool_execution_update.clone(),
            before_compaction_start: self.before_compaction_start.clone(),
            after_compaction_end: self.after_compaction_end.clone(),
            on_error: self.on_error.clone(),
            input_filters: self.input_filters.clone(),
            first_turn_trigger: TurnTrigger::User,
            config_id: self.config_id.clone(),
            context_translation: self.context_translation.clone(),
            prun_pending: self.prun_pending.clone(),
            revert_pending: self.revert_pending.clone(),
        })
    }

    // ── Session management ────────────────────────────────────────────────────

    /// Immediately rotate to a new `session_id`.
    ///
    /// All subsequent loops will belong to the new session. Loop counters are
    /// reset so the new session's loop ids start from `.1`.
    ///
    /// Returns the newly assigned `session_id`.
    pub fn new_session(&mut self) -> String {
        self.session_id = uuid::Uuid::new_v4().to_string();
        self.loop_counters.clear();
        self.last_loop_id = None;
        // Clear last_active_at so the new session is treated as never-used.
        // Without this, a subsequent check_and_rotate would see the old timestamp
        // and immediately rotate again without any prompt having run.
        self.last_active_at = None;
        self.session_id.clone()
    }

    /// Rotate to a new session if the agent has been idle for longer than `threshold`.
    ///
    /// Idleness is measured from the last [`prompt_messages_with_sender`][Self::prompt_messages_with_sender]
    /// call. If no prompt has ever been issued, returns `None` (no rotation needed
    /// — the session has never been used).
    ///
    /// Returns `Some(new_session_id)` if rotation happened, `None` otherwise.
    pub fn check_and_rotate(&mut self, threshold: std::time::Duration) -> Option<String> {
        let last = self.last_active_at?;
        let elapsed = (chrono::Utc::now() - last)
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        if elapsed > threshold {
            Some(self.new_session())
        } else {
            None
        }
    }
}

// ── Agent trait implementation ────────────────────────────────────────────────

#[async_trait::async_trait]
impl Agent for BasicAgent {
    // ── Core async implementations ────────────────────────────────────────────

    /// Send messages as a prompt, streaming events to a caller-provided sender.
    async fn prompt_messages_with_sender(
        &mut self,
        messages: Vec<AgentMessage>,
        tx: mpsc::UnboundedSender<AgentEvent>,
    ) {
        /*
        RUST QUIRK: `assert!()` — panic with a message if condition is false

        `assert!(condition, "message")` panics if condition is false.
        This is a "programmer error" guard (not a runtime error) — you should
        never call prompt() on an already-streaming BasicAgent. If you do, it's a bug.

        Python analogy: `assert not self.is_streaming, "..."` (but assert can be
        disabled with -O in Python; Rust's assert! is ALWAYS enabled in production.
        For debug-only assertions, use `debug_assert!()` in Rust.)
        */
        assert!(
            !self.is_streaming,
            "Agent is already streaming. Use steer() or follow_up()."
        );

        self.last_active_at = Some(chrono::Utc::now());
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone()); // store a clone so abort() can cancel it
        self.is_streaming = true;

        /*
        RUST QUIRK: `std::mem::take(&mut self.tools)` — efficient ownership transfer

        `std::mem::take(dest)` replaces `*dest` with its Default value and returns
        the original. For Vec, Default is an empty Vec (no allocation).

        Why not `self.tools.clone()`?
        For single-loop execution we MOVE the tools into the context (zero allocation).
        Arc::clone is cheap (just a reference-count increment), but we still prefer
        a move here since BasicAgent temporarily relinquishes the tools anyway.
        We want to MOVE the tools into the context, not copy them.

        Why not just `self.tools` (move out)?
        You can't partially move out of a struct that you still have a &mut reference to.
        `mem::take` is the safe way to move a field out, leaving a valid default behind.

        After the loop, we move the tools BACK: `self.tools = context.tools;`
        So the BasicAgent relinquishes ownership for the duration of the loop,
        then reclaims it afterward. Zero allocation.

        Python analogy: temporarily `tools = self.tools; self.tools = []` — then restore.
        */
        // Build config first (only borrows self), then derive loop_id (mutates loop_counters).
        // `.expect` is safe: BasicAgent always supplies a model_config (required by ctor).
        let config = self
            .build_config()
            .expect("BasicAgent always provides a model_config");
        let loop_id = self.next_loop_id(&config);
        self.last_loop_id = Some(loop_id.clone());

        let mut context = AgentContext {
            system_prompt: self.system_prompt.clone(),
            messages: self.messages.clone(),
            tools: std::mem::take(&mut self.tools), // MOVE tools out, leaving self.tools = []
            agent_id: Some(self.agent_id.clone()),
            session_id: Some(self.session_id.clone()),
            loop_id: Some(loop_id),
            parent_loop_id: None, // origin — no parent
            continuation_kind: None,
            session: self.session.take(), // Move session into context for block-based compaction
            user_context: Vec::new(),
            inrun_context: Vec::new(),
            active_node_id: None,
            next_node_id: 0,
        };

        let _new_messages = agent_loop(messages, &mut context, &config, tx, cancel).await;

        self.tools = context.tools;
        self.messages = context.messages;
        self.session = context.session; // Reclaim session after loop
        self.is_streaming = false;
        self.cancel = None;
    }

    /// Continue from current context, streaming events to a caller-provided sender.
    ///
    /// `kind` describes how this continuation relates to prior loops:
    /// - `Default` — unspecified continuation (preserves current semantics; use when the
    ///   Rerun/Branch distinction is not relevant to the caller)
    /// - `Rerun { tag }` — retry from the same context state (auto-generates a UTC timestamp tag)
    /// - `Branch { tag }` — explore a different path from the same starting point (same tag)
    async fn continue_loop_with_sender(
        &mut self,
        tx: mpsc::UnboundedSender<AgentEvent>, // OBSERVER — events from this continuation pushed here
        kind: ContinuationKind,                // CONTINUATION KIND — Default | Rerun | Branch
    ) {
        assert!(!self.is_streaming, "Agent is already streaming.");
        assert!(!self.messages.is_empty(), "No messages to continue from.");

        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.is_streaming = true;

        // Build config first (only borrows self), then derive loop_id (mutates loop_counters).
        // `.expect` is safe: BasicAgent always supplies a model_config (required by ctor).
        let config = self
            .build_config()
            .expect("BasicAgent always provides a model_config");
        let loop_id = self.next_loop_id(&config);
        let parent_loop_id = self.last_loop_id.clone(); // points to the loop this continues from
        self.last_loop_id = Some(loop_id.clone());

        // Auto-generate the timestamp tag for Rerun/Branch (RFC 3339 UTC).
        let tag = chrono::Utc::now().to_rfc3339();
        let kind_with_tag = match kind {
            ContinuationKind::Initial => ContinuationKind::Default, // Initial → Default when continuing
            ContinuationKind::Default => ContinuationKind::Default,
            ContinuationKind::Rerun { .. } => ContinuationKind::Rerun { tag },
            ContinuationKind::Branch { .. } => ContinuationKind::Branch { tag },
            ContinuationKind::Compaction => ContinuationKind::Compaction,
        };

        // Move tools temporarily into context for the loop; restored after
        let mut context = AgentContext {
            system_prompt: self.system_prompt.clone(),
            messages: self.messages.clone(),
            tools: std::mem::take(&mut self.tools),
            agent_id: Some(self.agent_id.clone()),
            session_id: Some(self.session_id.clone()),
            loop_id: Some(loop_id),
            parent_loop_id,
            continuation_kind: Some(kind_with_tag),
            session: self.session.take(),
            user_context: Vec::new(),
            inrun_context: Vec::new(),
            active_node_id: None,
            next_node_id: 0,
        };

        let _new_messages = agent_loop_continue(&mut context, &config, tx, cancel).await;

        self.tools = context.tools;
        self.messages = context.messages;
        self.session = context.session;
        self.is_streaming = false;
        self.cancel = None;
    }

    // ── State ─────────────────────────────────────────────────────────────────

    fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn last_loop_id(&self) -> Option<&str> {
        self.last_loop_id.as_deref()
    }

    // ── Message mutation ──────────────────────────────────────────────────────

    fn clear_messages(&mut self) {
        self.messages.clear();
    }

    fn append_message(&mut self, msg: AgentMessage) {
        self.messages.push(msg);
    }

    fn replace_messages(&mut self, msgs: Vec<AgentMessage>) {
        self.messages = msgs;
    }

    fn save_messages(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.messages)
    }

    fn restore_messages(&mut self, json: &str) -> Result<(), serde_json::Error> {
        let msgs: Vec<AgentMessage> = serde_json::from_str(json)?;
        self.messages = msgs;
        Ok(())
    }

    fn set_tools(&mut self, tools: Vec<Arc<dyn AgentTool>>) {
        self.tools = tools;
    }

    // ── Control ───────────────────────────────────────────────────────────────

    fn abort(&self) {
        if let Some(ref cancel) = self.cancel {
            cancel.cancel();
        }
    }

    fn reset(&mut self) {
        self.messages.clear();
        self.clear_all_queues();
        self.is_streaming = false;
        self.cancel = None;
    }

    // ── Steering/follow-up queues ─────────────────────────────────────────────

    /*
    RUST QUIRK: `&self` vs `&mut self` — `steer()` takes shared reference

    Usually, methods that modify the struct take `&mut self` (exclusive borrow).
    But `steer()` takes `&self` (shared borrow). How can it modify the queue?

    Answer: Interior mutability via `Arc<Mutex<...>>`.
    The Mutex provides runtime-checked exclusive access inside a shared reference.
    You call `.lock()` to acquire the lock (blocks until available), then mutate.

    This design allows `steer()` to be called from OTHER threads or closures
    that only have &-access to the BasicAgent (e.g., a button click handler).

    Lock acquisition uses `lock_queue()` (see top of file) which tolerates
    `Mutex` poisoning. A panic in a hook or tool callback would otherwise crash
    every subsequent `steer()` / `follow_up()` call even though the underlying
    queue is recoverable data.
    */
    fn steer(&self, msg: AgentMessage) {
        lock_queue(&self.steering_queue).push(msg);
    }

    fn follow_up(&self, msg: AgentMessage) {
        lock_queue(&self.follow_up_queue).push(msg);
    }

    fn clear_steering_queue(&self) {
        lock_queue(&self.steering_queue).clear();
    }

    fn clear_follow_up_queue(&self) {
        lock_queue(&self.follow_up_queue).clear();
    }

    fn set_steering_mode(&mut self, mode: QueueMode) {
        self.steering_mode = mode;
    }

    fn set_follow_up_mode(&mut self, mode: QueueMode) {
        self.follow_up_mode = mode;
    }

    // ── Configuration access ─────────────────────────────────────────────

    fn profile(&self) -> Option<&AgentProfile> {
        self.profile.as_ref()
    }

    fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    fn model_config(&self) -> Option<&ModelConfig> {
        Some(&self.model_config)
    }

    fn thinking_level(&self) -> ThinkingLevel {
        self.thinking_level
    }

    fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }

    fn context_config(&self) -> Option<&ContextConfig> {
        self.context_config.as_ref()
    }

    fn execution_limits(&self) -> Option<&ExecutionLimits> {
        self.execution_limits.as_ref()
    }

    fn cache_config(&self) -> CacheConfig {
        self.cache_config.clone()
    }

    fn tool_execution(&self) -> ToolExecutionStrategy {
        self.tool_execution.clone()
    }

    fn tool_timeout(&self) -> Option<std::time::Duration> {
        self.tool_timeout
    }

    fn response_format(&self) -> crate::provider::ResponseFormat {
        self.response_format.clone()
    }

    fn retry_config(&self) -> crate::provider::retry::RetryConfig {
        self.retry_config.clone()
    }

    // ── Session ──────────────────────────────────────────────────────────

    fn session(&self) -> Option<&crate::session::Session> {
        self.session.as_ref()
    }

    fn workspace(&self) -> Option<&std::path::Path> {
        self.workspace.as_deref()
    }

    // ── Hook setters ─────────────────────────────────────────────────────

    fn set_before_turn(&mut self, f: Option<BeforeTurnFn>) {
        self.before_turn = f;
    }

    fn set_after_turn(&mut self, f: Option<AfterTurnFn>) {
        self.after_turn = f;
    }

    fn set_before_loop(&mut self, f: Option<BeforeLoopFn>) {
        self.before_loop = f;
    }

    fn set_after_loop(&mut self, f: Option<AfterLoopFn>) {
        self.after_loop = f;
    }

    fn set_before_tool_execution(&mut self, f: Option<BeforeToolExecutionFn>) {
        self.before_tool_execution = f;
    }

    fn set_after_tool_execution(&mut self, f: Option<AfterToolExecutionFn>) {
        self.after_tool_execution = f;
    }

    fn set_before_tool_execution_update(&mut self, f: Option<BeforeToolExecutionUpdateFn>) {
        self.before_tool_execution_update = f;
    }

    fn set_after_tool_execution_update(&mut self, f: Option<AfterToolExecutionUpdateFn>) {
        self.after_tool_execution_update = f;
    }

    fn set_convert_to_llm(&mut self, f: Option<ConvertToLlmFn>) {
        self.convert_to_llm = f;
    }

    fn set_transform_context(&mut self, f: Option<TransformContextFn>) {
        self.transform_context = f;
    }

    fn set_block_compaction_strategy(
        &mut self,
        s: Option<Arc<dyn crate::context::BlockCompactionStrategy>>,
    ) {
        // G5: delegate to context_config.compaction
        if let Some(ref mut ctx) = self.context_config {
            ctx.compaction.block_strategy = s;
        }
    }

    fn set_before_compaction_start(&mut self, f: Option<BeforeCompactionStartFn>) {
        self.before_compaction_start = f;
    }

    fn set_after_compaction_end(&mut self, f: Option<AfterCompactionEndFn>) {
        self.after_compaction_end = f;
    }

    fn set_context_translation(&mut self, s: Option<Arc<dyn ContextTranslationStrategy>>) {
        self.context_translation = s;
    }

    fn context_translation(&self) -> Option<Arc<dyn ContextTranslationStrategy>> {
        self.context_translation.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: poison a `Mutex<Vec<u32>>` by panicking inside a guard on a child thread.
    fn poison_mutex<T: Send + 'static>(m: Arc<Mutex<T>>) {
        let m = m.clone();
        let _ = std::thread::spawn(move || {
            let _guard = m.lock().unwrap();
            panic!("intentional panic to poison the mutex");
        })
        .join(); // join — captures and drops the panic
    }

    #[test]
    fn lock_queue_recovers_inner_vec_after_poison() {
        let q: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(vec![1, 2, 3]));
        poison_mutex(q.clone());
        assert!(
            q.is_poisoned(),
            "test pre-condition: mutex should be poisoned"
        );

        // lock_queue must not panic; it must surface the original Vec.
        let guard = lock_queue(&q);
        assert_eq!(*guard, vec![1, 2, 3]);
    }

    #[test]
    fn basic_agent_steer_survives_queue_poison() {
        let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock-model", "test-key"));

        // Poison the steering queue by panicking inside a guard.
        let q = agent.steering_queue.clone();
        let _ = std::thread::spawn(move || {
            let _g = q.lock().unwrap();
            panic!("poison the steering queue");
        })
        .join();
        assert!(agent.steering_queue.is_poisoned());

        // The public steer/clear API should still work.
        agent.steer(AgentMessage::Llm(LlmMessage::new(Message::user("hi"))));
        assert_eq!(lock_queue(&agent.steering_queue).len(), 1);
        agent.clear_steering_queue();
        assert_eq!(lock_queue(&agent.steering_queue).len(), 0);
    }

    #[test]
    fn basic_agent_follow_up_survives_queue_poison() {
        let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock-model", "test-key"));

        let q = agent.follow_up_queue.clone();
        let _ = std::thread::spawn(move || {
            let _g = q.lock().unwrap();
            panic!("poison the follow-up queue");
        })
        .join();
        assert!(agent.follow_up_queue.is_poisoned());

        agent.follow_up(AgentMessage::Llm(LlmMessage::new(Message::user("more"))));
        assert_eq!(lock_queue(&agent.follow_up_queue).len(), 1);
        agent.clear_follow_up_queue();
        assert_eq!(lock_queue(&agent.follow_up_queue).len(), 0);
    }
}
