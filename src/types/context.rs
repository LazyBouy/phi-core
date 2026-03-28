use super::agent_message::AgentMessage;
use super::event::ContinuationKind;
use super::tool::AgentTool;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Agent context (passed to the loop)
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct AgentContext {
    /*
    AgentContext vs Agent Session
    AgentContext is a snapshot — the minimal state needed to execute one agent loop call:
    system prompt + message history + available tools . It's stateless and passed around
    functionally.

    An Agent Session (your extension target) would be the lifecycle wrapper —
    tracking session ID, branch tree, timestamps, and multiple AgentContext snapshots over time.
    Think of it as: AgentContext is a single frame, a Session is the whole film reel.
    */
    pub system_prompt: String, // PROMPT — injected at the top of every LLM call (role="system")
    pub messages: Vec<AgentMessage>, // HISTORY — full conversation; grows each turn; includes Extension messages

    // Arc<dyn AgentTool>: shared ownership of type-erased tools. Arc (atomic reference-counted
    // pointer) allows AgentContext to be Clone — cloning a context for parallel branches is a
    // cheap reference-count increment on each tool, not a deep copy. Tools are immutable during
    // execution (execute takes &self), so sharing via Arc is semantically correct.
    pub tools: Vec<Arc<dyn AgentTool>>, // REGISTRY — available tool implementations; converted to ToolDefinition for LLM

    // ── Identity ─────────────────────────────────────────────────────────────
    // Set by callers (e.g. Agent wrapper) before each loop call.
    // agent_loop() auto-generates UUIDs if None and writes them back to context.
    // agent_loop_continue() asserts both are Some — continuations require stable identity.
    /// Stable identifier for the agent instance. Auto-generated UUID v4 if None on first call;
    /// written back to context so continuations inherit it.
    pub agent_id: Option<String>,

    /// Groups related loop calls under one logical session (evaluational parallelism, reruns,
    /// branches). Auto-generated UUID v4 if None on first call; written back to context.
    /// Required (Some) for all `agent_loop_continue()` calls.
    pub session_id: Option<String>,

    /// Unique identifier for this specific loop call: `"{session_id}.{config_id}.{N}"`.
    /// Set by Agent wrapper via `next_loop_id()`; direct callers may supply their own.
    /// Falls back to a UUID at loop entry if still None.
    pub loop_id: Option<String>,

    /// The `loop_id` of the loop this was continued from. None for origin calls.
    /// Set by Agent wrapper for `agent_loop_continue()` calls to enable ancestry tracking.
    pub parent_loop_id: Option<String>,

    /// How this loop relates to prior loops. None for origin calls.
    /// Some(Default|Rerun|Branch) for agent_loop_continue() calls.
    pub continuation_kind: Option<ContinuationKind>,

    /// Optional session for block-based compaction. When `Some`, the agent loop
    /// uses `compact_session_loops()` / `build_context_from_session()` instead of
    /// in-memory `CompactionStrategy::compact()`.
    ///
    /// When `None` (sub-agents, tests, direct callers), the loop falls back to
    /// the existing in-memory compaction path.
    pub session: Option<crate::session::Session>,
}
