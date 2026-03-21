//! Stateful Agent struct — wraps the agent loop with state management,
//! steering/follow-up queues, and abort support.

use crate::agent_loop::{
    agent_loop, agent_loop_continue, AfterTurnFn, AgentLoopConfig, BeforeTurnFn, OnErrorFn,
};
use crate::context::{CompactionStrategy, ContextConfig, ExecutionLimits};
use crate::mcp::{McpClient, McpError, McpToolAdapter};
use crate::provider::{ModelConfig, StreamProvider};
use crate::types::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/*
ARCHITECTURE: Agent vs agent_loop — stateful wrapper vs stateless functions

The agent loop (agent_loop.rs) is a set of FREE FUNCTIONS — they take all their
inputs as parameters and return outputs. They have no hidden state.

The Agent struct is an OPTIONAL stateful wrapper that owns:
  - Message history (Vec<AgentMessage>) — the conversation so far
  - Tools (Vec<Box<dyn AgentTool>>) — registered capabilities
  - Provider (Box<dyn StreamProvider>) — the LLM backend
  - Steering/follow-up queues (Arc<Mutex<>>) — for mid-run interrupts

Why this separation?
  - Free functions: easier to test, compose, and reason about
  - Agent struct: easier to use in applications (less boilerplate)
  - You can use agent_loop() directly if you need more control

The Agent uses the BUILDER PATTERN for construction:
  Agent::new(provider)
      .with_system_prompt("...")
      .with_model("claude-3")
      .with_tools(vec![...])

Each `with_*` method takes `mut self` and returns `Self` — consuming and
returning the same value. This chains naturally and avoids separate calls.
Python analogy: it's like a fluent API but ownership-safe.
*/

/// Controls how messages are drained from the steering/follow-up queues per turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    /// Deliver one message per turn — allows the LLM to react to each steering message individually.
    OneAtATime,
    /// Deliver all queued messages at once — batches all pending steers into one turn.
    All,
}

/// The main Agent. Owns conversation state, tools, and provider.
pub struct Agent {
    // -- Public configuration (readable/overridable externally) --
    pub system_prompt: String,
    pub model: String,
    pub api_key: String,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>,  // None = use provider default
    pub temperature: Option<f32>, // None = use provider default

    // -- Private configuration (only mutated via builder methods) --
    model_config: Option<ModelConfig>,
    messages: Vec<AgentMessage>, // full conversation history

    /*
    RUST QUIRK: `Box<dyn Trait>` — owned trait objects

    `Box<dyn AgentTool>` means: "I own a heap-allocated value of some type that
    implements AgentTool, but I don't know which type at compile time."

    This is Rust's dynamic dispatch mechanism. The compiler inserts a vtable pointer
    (a fat pointer: data + vtable), and method calls go through the vtable at runtime.

    Why `Box` and not just `dyn AgentTool`?
    Because Rust must know the SIZE of every variable at compile time.
    `dyn AgentTool` has unknown size (different tool structs have different layouts).
    `Box<dyn AgentTool>` has fixed size: one pointer (8 bytes on 64-bit).

    Python analogy: tools are just a list of objects implementing a protocol — Python
    doesn't need explicit boxing because all objects are already heap-allocated.
    */
    tools: Vec<Box<dyn AgentTool>>,

    /*
    RUST QUIRK: `Arc<dyn StreamProvider>` — shared ownership for the provider

    `AgentLoopConfig.provider` requires `Arc<dyn StreamProvider>` (shared ownership)
    because the config is passed into async closures that may outlive the current stack frame.
    Agent therefore stores the provider as `Arc` so it can cheaply clone the pointer
    into `build_config()` without moving or copying the underlying provider.

    `Arc::clone()` just bumps an atomic reference count — cheap, no data duplication.
    Python analogy: storing any object reference; Python objects are already reference-counted.
    */
    provider: Arc<dyn StreamProvider>,

    /*
    RUST QUIRK: `Arc<Mutex<Vec<AgentMessage>>>` — shared mutable state across threads

    This is the canonical Rust pattern for "I need to mutate this from multiple places."

    Arc  = Atomically Reference Counted — shared ownership (multiple holders, thread-safe)
    Mutex = Mutual Exclusion — only one thread can access the inner value at a time

    The Agent OWNS the queues (Arc keeps them alive as long as Agent is alive).
    The agent loop USES the queues via the closures in build_config() — those closures
    clone the Arc (incrementing the reference count) and lock the Mutex to read/drain.

    Python analogy: threading.Lock() wrapping a shared list, passed to threads via closure.

    Why Arc instead of Rc?
    Rc (Reference Counted) is NOT thread-safe. Since tokio runs on a thread pool,
    closures may execute on any thread, so we need Arc (atomic = thread-safe).

    `.lock().unwrap()` — acquire the Mutex lock, panic if the Mutex is "poisoned"
    (poisoning happens if another thread panicked while holding the lock).
    In practice, unwrap() on a Mutex is acceptable because Mutex poisoning indicates
    a bug, and panicking is the right response.
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
    pub retry_config: crate::retry::RetryConfig,

    // Lifecycle callbacks
    before_turn: Option<BeforeTurnFn>,
    after_turn: Option<AfterTurnFn>,
    on_error: Option<OnErrorFn>,

    // Input filters
    input_filters: Vec<Arc<dyn InputFilter>>,

    // Custom compaction strategy
    compaction_strategy: Option<Arc<dyn CompactionStrategy>>,

    // Control — cancel token is Some during a streaming call, None otherwise
    cancel: Option<CancellationToken>,
    is_streaming: bool, // guard against concurrent prompt() calls
}

impl Agent {
    /*
    RUST QUIRK: `impl StreamProvider + 'static` — accepting any concrete type

    `impl Trait` in function parameters means "accept any type that implements this trait"
    without needing a generic type parameter `<T: StreamProvider>`.

    `+ 'static` means "the type must not contain any non-static references" —
    i.e., it must own all its data (no borrowed references that could dangle).
    Required because we store it in `Box<dyn StreamProvider>`, which may outlive
    the call site. All owned types (structs with no &-references) are 'static.

    `Box::new(provider)` — move the provider onto the heap and erase its concrete type.
    After this, we only know it's "some StreamProvider" — dynamic dispatch from here on.

    Python analogy:
      def __init__(self, provider: StreamProvider):
          self.provider = provider
    No boxing needed because Python objects are already heap-allocated.
    */
    pub fn new(provider: impl StreamProvider + 'static) -> Self {
        Self {
            system_prompt: String::new(),
            model: String::new(),
            api_key: String::new(),
            thinking_level: ThinkingLevel::Off,
            max_tokens: None,
            temperature: None,
            model_config: None,
            messages: Vec::new(),
            tools: Vec::new(),
            provider: Arc::new(provider), // erase concrete type, store as trait object
            steering_queue: Arc::new(Mutex::new(Vec::new())), // empty, shared with closures
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            context_config: Some(ContextConfig::default()), // enabled by default
            execution_limits: Some(ExecutionLimits::default()), // enabled by default
            cache_config: CacheConfig::default(),
            tool_execution: ToolExecutionStrategy::default(), // Parallel
            retry_config: crate::retry::RetryConfig::default(), // 3 retries
            before_turn: None,
            after_turn: None,
            on_error: None,
            input_filters: Vec::new(),
            compaction_strategy: None,
            cancel: None,
            is_streaming: false,
        }
    }

    /*
    RUST QUIRK: Builder pattern — `mut self` + return `Self`

    Builder methods take OWNERSHIP of `self` (consume the Agent), modify it, then
    return it. This allows chaining:
      Agent::new(p).with_model("x").with_tools(vec![...])

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

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = key.into();
        self
    }

    pub fn with_thinking(mut self, level: ThinkingLevel) -> Self {
        self.thinking_level = level;
        self
    }

    pub fn with_tools(mut self, tools: Vec<Box<dyn AgentTool>>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_model_config(mut self, config: ModelConfig) -> Self {
        self.model_config = Some(config);
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

    pub fn with_retry_config(mut self, config: crate::retry::RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Load skills and append their index to the system prompt.
    ///
    /// The skills index is appended as XML per the [AgentSkills standard](https://agentskills.io).
    /// The agent can then read individual SKILL.md files using the `read_file` tool
    /// when it decides a skill is relevant.
    pub fn with_skills(mut self, skills: crate::skills::SkillSet) -> Self {
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

    /// Set a custom compaction strategy. When set, replaces the default
    /// `compact_messages()` call during context compaction.
    pub fn with_compaction_strategy(mut self, strategy: impl CompactionStrategy + 'static) -> Self {
        self.compaction_strategy = Some(Arc::new(strategy));
        self
    }

    /// Add a sub-agent tool. The sub-agent runs its own `agent_loop()` when invoked.
    pub fn with_sub_agent(mut self, sub: crate::sub_agent::SubAgentTool) -> Self {
        self.tools.push(Box::new(sub));
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
            self.tools.push(Box::new(adapter));
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
            self.tools.push(Box::new(adapter));
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
            self.tools.push(Box::new(adapter));
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
            self.tools.push(Box::new(adapter));
        }
        Ok(self)
    }

    /// Connect to an MCP server via HTTP and add its tools to the agent.
    pub async fn with_mcp_server_http(mut self, url: &str) -> Result<Self, McpError> {
        let client = McpClient::connect_http(url).await?;
        let client = Arc::new(tokio::sync::Mutex::new(client));
        let adapters = McpToolAdapter::from_client(client).await?;
        for adapter in adapters {
            self.tools.push(Box::new(adapter));
        }
        Ok(self)
    }

    // -- State access --

    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    pub fn set_tools(&mut self, tools: Vec<Box<dyn AgentTool>>) {
        self.tools = tools;
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    pub fn append_message(&mut self, msg: AgentMessage) {
        self.messages.push(msg);
    }

    pub fn replace_messages(&mut self, msgs: Vec<AgentMessage>) {
        self.messages = msgs;
    }

    pub fn save_messages(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.messages)
    }

    pub fn restore_messages(&mut self, json: &str) -> Result<(), serde_json::Error> {
        let msgs: Vec<AgentMessage> = serde_json::from_str(json)?;
        self.messages = msgs;
        Ok(())
    }

    // -- Queue management --

    /// Queue a steering message — interrupts the agent mid-tool-execution.
    ///
    /// The agent loop checks the steering queue after each tool completes.
    /// When it finds messages, it stops executing remaining tools and injects
    /// these messages before the next LLM call. This allows human-in-the-loop
    /// corrections without aborting the entire session.
    /*
    RUST QUIRK: `&self` vs `&mut self` — `steer()` takes shared reference

    Usually, methods that modify the struct take `&mut self` (exclusive borrow).
    But `steer()` takes `&self` (shared borrow). How can it modify the queue?

    Answer: Interior mutability via `Arc<Mutex<...>>`.
    The Mutex provides runtime-checked exclusive access inside a shared reference.
    You call `.lock()` to acquire the lock (blocks until available), then mutate.

    This design allows `steer()` to be called from OTHER threads or closures
    that only have &-access to the Agent (e.g., a button click handler).

    `.lock().unwrap()` — unwrap because Mutex poisoning (from a panicking thread)
    is a programming bug, not a runtime error we should handle gracefully.
    */
    pub fn steer(&self, msg: AgentMessage) {
        self.steering_queue.lock().unwrap().push(msg); // acquire lock, push, auto-release
    }

    /// Queue a follow-up message — processed after the current agent turn completes.
    ///
    /// Unlike steering (which interrupts mid-execution), follow-ups are injected
    /// after the agent reaches a natural stopping point (StopReason::Stop).
    /// Use for chaining tasks: "after you finish X, also do Y."
    pub fn follow_up(&self, msg: AgentMessage) {
        self.follow_up_queue.lock().unwrap().push(msg);
    }

    pub fn clear_steering_queue(&self) {
        self.steering_queue.lock().unwrap().clear();
    }

    pub fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().unwrap().clear();
    }

    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    pub fn set_steering_mode(&mut self, mode: QueueMode) {
        self.steering_mode = mode;
    }

    pub fn set_follow_up_mode(&mut self, mode: QueueMode) {
        self.follow_up_mode = mode;
    }

    // -- Control --

    pub fn abort(&self) {
        if let Some(ref cancel) = self.cancel {
            cancel.cancel();
        }
    }

    pub fn reset(&mut self) {
        self.messages.clear();
        self.clear_all_queues();
        self.is_streaming = false;
        self.cancel = None;
    }

    // -- Prompting --

    /// Send a text prompt. Returns a stream of AgentEvents.
    pub async fn prompt(&mut self, text: impl Into<String>) -> mpsc::UnboundedReceiver<AgentEvent> {
        let msg = AgentMessage::Llm(Message::user(text));
        self.prompt_messages(vec![msg]).await
    }

    /// Send messages as a prompt. Convenience wrapper around
    /// [`prompt_messages_with_sender()`](Self::prompt_messages_with_sender)
    /// that creates a channel internally and returns the receiver.
    pub async fn prompt_messages(
        &mut self,
        messages: Vec<AgentMessage>,
    ) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.prompt_messages_with_sender(messages, tx).await;
        rx
    }

    /// Send a text prompt, streaming events to a caller-provided sender.
    ///
    /// Unlike [`prompt()`](Self::prompt), this accepts an external sender so
    /// the caller can consume events in real-time on another task:
    ///
    /// ```rust,no_run
    /// # use phi_core::Agent;
    /// # use phi_core::provider::MockProvider;
    /// # async fn example() {
    /// let mut agent = Agent::new(MockProvider::text("hi"))
    ///     .with_model("mock").with_api_key("test");
    /// let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    /// tokio::spawn(async move {
    ///     while let Some(event) = rx.recv().await { /* real-time */ }
    /// });
    /// agent.prompt_with_sender("hello", tx).await;
    /// # }
    /// ```
    /*
    DESIGN: prompt() vs prompt_with_sender() — channel ownership models
      `prompt()`             = INTERNAL CHANNEL — creates (tx, rx) internally; returns rx to caller
                               Caller pulls events from rx in a loop: `while let Some(e) = rx.recv().await`
      `prompt_with_sender()` = EXTERNAL CHANNEL — caller provides their own tx
                               Useful when the caller already has a task consuming from rx,
                               and wants to merge this agent's events into that same stream
    */
    pub async fn prompt_with_sender(
        &mut self,
        text: impl Into<String>, // TEXT INPUT — converted to AgentMessage::Llm(Message::user(...))
        tx: mpsc::UnboundedSender<AgentEvent>, // OBSERVER — caller-owned sender; events pushed here during the loop
    ) {
        let msg = AgentMessage::Llm(Message::user(text));
        self.prompt_messages_with_sender(vec![msg], tx).await;
    }

    /// Send messages as a prompt, streaming events to a caller-provided sender.
    pub async fn prompt_messages_with_sender(
        &mut self,
        messages: Vec<AgentMessage>, // NEW INPUT — owned; appended to context before the loop starts
        tx: mpsc::UnboundedSender<AgentEvent>, // OBSERVER — caller-provided; events pushed here during the loop
    ) {
        /*
        RUST QUIRK: `assert!()` — panic with a message if condition is false

        `assert!(condition, "message")` panics if condition is false.
        This is a "programmer error" guard (not a runtime error) — you should
        never call prompt() on an already-streaming Agent. If you do, it's a bug.

        Python analogy: `assert not self.is_streaming, "..."` (but assert can be
        disabled with -O in Python; Rust's assert! is ALWAYS enabled in production.
        For debug-only assertions, use `debug_assert!()` in Rust.)
        */
        assert!(
            !self.is_streaming,
            "Agent is already streaming. Use steer() or follow_up()."
        );

        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone()); // store a clone so abort() can cancel it
        self.is_streaming = true;

        /*
        RUST QUIRK: `std::mem::take(&mut self.tools)` — efficient ownership transfer

        `std::mem::take(dest)` replaces `*dest` with its Default value and returns
        the original. For Vec, Default is an empty Vec (no allocation).

        Why not `self.tools.clone()`?
        Clone would copy every Box<dyn AgentTool> — expensive and unnecessary.
        We want to MOVE the tools into the context, not copy them.

        Why not just `self.tools` (move out)?
        You can't partially move out of a struct that you still have a &mut reference to.
        `mem::take` is the safe way to move a field out, leaving a valid default behind.

        After the loop, we move the tools BACK: `self.tools = context.tools;`
        So the Agent relinquishes ownership for the duration of the loop,
        then reclaims it afterward. Zero allocation.

        Python analogy: temporarily `tools = self.tools; self.tools = []` — then restore.
        */
        let mut context = AgentContext {
            system_prompt: self.system_prompt.clone(),
            messages: self.messages.clone(),
            tools: std::mem::take(&mut self.tools), // MOVE tools out, leaving self.tools = []
            agent_id: None,
            session_id: None,
        };

        let config = self.build_config();

        let _new_messages = agent_loop(messages, &mut context, &config, tx, cancel).await;

        self.tools = context.tools;
        self.messages = context.messages;
        self.is_streaming = false;
        self.cancel = None;
    }

    /// Continue from current context (for retries after errors). Convenience
    /// wrapper around [`continue_loop_with_sender()`](Self::continue_loop_with_sender).
    pub async fn continue_loop(&mut self) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.continue_loop_with_sender(tx).await;
        rx
    }

    /// Continue from current context, streaming events to a caller-provided sender.
    pub async fn continue_loop_with_sender(
        &mut self,
        tx: mpsc::UnboundedSender<AgentEvent>, // OBSERVER — events from this continuation pushed here
    ) {
        assert!(!self.is_streaming, "Agent is already streaming.");
        assert!(!self.messages.is_empty(), "No messages to continue from.");

        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.is_streaming = true;

        // Move tools temporarily into context for the loop; restored after
        let mut context = AgentContext {
            system_prompt: self.system_prompt.clone(),
            messages: self.messages.clone(),
            tools: std::mem::take(&mut self.tools),
            agent_id: None,
            session_id: None,
        };

        let config = self.build_config();

        let _new_messages = agent_loop_continue(&mut context, &config, tx, cancel).await;

        self.tools = context.tools;
        self.messages = context.messages;
        self.is_streaming = false;
        self.cancel = None;
    }

    // -- Internal --

    /*
    build_config — assemble AgentLoopConfig from Agent's current state.

    ARCHITECTURE: Why a separate build_config() method?

    AgentLoopConfig is the "parameter bundle" for the stateless agent_loop() function.
    build_config() constructs it fresh each call — it's not stored on Agent.
    This means: AgentLoopConfig borrows from Agent (hence the lifetime `'_`),
    and both share the same Arc<Mutex<>> queues via clone (cheap, no allocation).

    RUST QUIRK: `move` closures for the queue callbacks

    The steering/follow-up closures need to outlive build_config()'s stack frame
    (they're stored in AgentLoopConfig and called later by the agent loop).
    So they use `move` to capture `steering_queue` (Arc clone) and `steering_mode` (Copy).

    We clone the Arc before the move:
      let steering_queue = self.steering_queue.clone();
    This gives the closure its own Arc reference to the same underlying Mutex.
    The Agent still holds its own Arc reference. Both are valid simultaneously.

    `self.provider.clone()` — clone the Arc:
      self.provider is Arc<dyn StreamProvider>
      .clone() bumps the reference count — cheap, no data duplication
    Both Agent and AgentLoopConfig now share ownership of the same underlying provider.
    */
    fn build_config(&self) -> AgentLoopConfig {
        // Clone Arc handles before the move closures capture them
        let steering_queue = self.steering_queue.clone(); // cheap Arc clone
        let steering_mode = self.steering_mode; // Copy — no clone needed

        let follow_up_queue = self.follow_up_queue.clone();
        let follow_up_mode = self.follow_up_mode;

        AgentLoopConfig {
            provider: self.provider.clone(), // Arc::clone — cheap reference count bump
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            thinking_level: self.thinking_level,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            model_config: self.model_config.clone(),
            convert_to_llm: None,
            transform_context: None,
            get_steering_messages: Some(Box::new(move || {
                // This closure runs each time the agent loop checks for steering messages.
                // `move` captured: steering_queue (Arc clone), steering_mode (Copy)
                let mut queue = steering_queue.lock().unwrap(); // acquire Mutex lock
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
            compaction_strategy: self.compaction_strategy.clone(),
            execution_limits: self.execution_limits.clone(),
            cache_config: self.cache_config.clone(),
            tool_execution: self.tool_execution.clone(),
            retry_config: self.retry_config.clone(),
            get_follow_up_messages: Some(Box::new(move || {
                let mut queue = follow_up_queue.lock().unwrap();
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
            before_loop: None,
            after_loop: None,
            on_error: self.on_error.clone(),
            input_filters: self.input_filters.clone(),
            first_turn_trigger: TurnTrigger::User,
        }
    }
}
