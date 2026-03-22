//! The default in-memory `Agent` implementation.
//!
//! [`BasicAgent`] owns a single linear message history and runs the `agent_loop` directly.
//! It is the concrete type most callers will use. Configuration is done via the builder
//! pattern; the runtime interface is provided by the [`Agent`](super::Agent) trait.

use super::agent::{Agent, QueueMode};
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
ARCHITECTURE: BasicAgent vs agent_loop — stateful wrapper vs stateless functions

The agent loop (agent_loop.rs) is a set of FREE FUNCTIONS — they take all their
inputs as parameters and return outputs. They have no hidden state.

The BasicAgent struct is an OPTIONAL stateful wrapper that owns:
  - Message history (Vec<AgentMessage>) — the conversation so far
  - Tools (Vec<Box<dyn AgentTool>>) — registered capabilities
  - Provider (Box<dyn StreamProvider>) — the LLM backend
  - Steering/follow-up queues (Arc<Mutex<>>) — for mid-run interrupts

Why this separation?
  - Free functions: easier to test, compose, and reason about
  - BasicAgent struct: easier to use in applications (less boilerplate)
  - You can use agent_loop() directly if you need more control

The BasicAgent uses the BUILDER PATTERN for construction:
  BasicAgent::new(provider)
      .with_system_prompt("...")
      .with_model("claude-3")
      .with_tools(vec![...])

Each `with_*` method takes `mut self` and returns `Self` — consuming and
returning the same value. This chains naturally and avoids separate calls.
Python analogy: it's like a fluent API but ownership-safe.
*/

/// The default in-memory agent. Owns conversation state, tools, and provider.
///
/// Configuration is done via the builder pattern before any prompting. The runtime
/// interface (prompting, state access, control) is provided via the [`Agent`] trait.
pub struct BasicAgent {
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
    BasicAgent therefore stores the provider as `Arc` so it can cheaply clone the pointer
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

    The BasicAgent OWNS the queues (Arc keeps them alive as long as BasicAgent is alive).
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
}

impl BasicAgent {
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
            agent_id: uuid::Uuid::new_v4().to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            loop_counters: HashMap::new(),
            last_loop_id: None,
        }
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
    pub fn with_sub_agent(mut self, sub: crate::agents::SubAgentTool) -> Self {
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

    // -- Ergonomic prompting wrappers --
    // These inherent methods accept `impl Into<String>` so callers can pass `&str` directly.
    // All other runtime methods (state, mutation, control, queues) are provided solely by
    // the `Agent` trait impl below — import `use phi_core::Agent` (or `use phi_core::*`)
    // to call them on a concrete `BasicAgent`.

    /// Send a text prompt. Returns a stream of `AgentEvent`s.
    ///
    /// Accepts `impl Into<String>` (e.g. `&str`). The trait's [`Agent::prompt`] default
    /// requires an owned `String`; use this inherent method to pass `&str` without `.to_string()`.
    pub async fn prompt(
        &mut self,
        text: impl Into<String>,
    ) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let msg = AgentMessage::Llm(Message::user(text));
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
        let msg = AgentMessage::Llm(Message::user(text));
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
                config.provider.provider_id(),
                slugify(&config.model),
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
            before_tool_execution: None,
            after_tool_execution: None,
            before_tool_execution_update: None,
            after_tool_execution_update: None,
            on_error: self.on_error.clone(),
            input_filters: self.input_filters.clone(),
            cost_config: None,
            first_turn_trigger: TurnTrigger::User,
            config_id: None, // auto-derived in next_loop_id() from provider + model + thinking_level
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
        So the BasicAgent relinquishes ownership for the duration of the loop,
        then reclaims it afterward. Zero allocation.

        Python analogy: temporarily `tools = self.tools; self.tools = []` — then restore.
        */
        // Build config first (only borrows self), then derive loop_id (mutates loop_counters).
        let config = self.build_config();
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
        };

        let _new_messages = agent_loop(messages, &mut context, &config, tx, cancel).await;

        self.tools = context.tools;
        self.messages = context.messages;
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
        let config = self.build_config();
        let loop_id = self.next_loop_id(&config);
        let parent_loop_id = self.last_loop_id.clone(); // points to the loop this continues from
        self.last_loop_id = Some(loop_id.clone());

        // Auto-generate the timestamp tag for Rerun/Branch (RFC 3339 UTC).
        let tag = chrono::Utc::now().to_rfc3339();
        let kind_with_tag = match kind {
            ContinuationKind::Default => ContinuationKind::Default,
            ContinuationKind::Rerun { .. } => ContinuationKind::Rerun { tag },
            ContinuationKind::Branch { .. } => ContinuationKind::Branch { tag },
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
        };

        let _new_messages = agent_loop_continue(&mut context, &config, tx, cancel).await;

        self.tools = context.tools;
        self.messages = context.messages;
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

    fn set_tools(&mut self, tools: Vec<Box<dyn AgentTool>>) {
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

    `.lock().unwrap()` — unwrap because Mutex poisoning (from a panicking thread)
    is a programming bug, not a runtime error we should handle gracefully.
    */
    fn steer(&self, msg: AgentMessage) {
        self.steering_queue.lock().unwrap().push(msg);
    }

    fn follow_up(&self, msg: AgentMessage) {
        self.follow_up_queue.lock().unwrap().push(msg);
    }

    fn clear_steering_queue(&self) {
        self.steering_queue.lock().unwrap().clear();
    }

    fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().unwrap().clear();
    }

    fn set_steering_mode(&mut self, mode: QueueMode) {
        self.steering_mode = mode;
    }

    fn set_follow_up_mode(&mut self, mode: QueueMode) {
        self.follow_up_mode = mode;
    }
}
