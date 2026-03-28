use super::agent_message::AgentMessage;
use super::context::AgentContext;
use super::usage::Usage;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Evaluational parallelism result types
// ---------------------------------------------------------------------------

/// One branch's outcome from `agent_loop_parallel`.
///
/// Contains the complete context after the branch loop, the new messages it
/// produced, its token usage, and the identifiers linking it back to its config.
pub struct ParallelLoopOutcome {
    /// Position in the input `configs` vec (0-based).
    pub config_index: usize,
    /// The `loop_id` assigned to this branch: `"{session_id}.{config_segment}.{N}"`.
    pub loop_id: String,
    /// Full `AgentContext` after the branch loop completed.
    pub context: AgentContext,
    /// Only the messages produced by this branch's loop (not prior history).
    pub new_messages: Vec<AgentMessage>,
    /// Token usage accumulated across all turns of this branch.
    pub usage: Usage,
    /// Number of messages in the cloned context at the moment the branch was
    /// dispatched. `context.messages[..original_context_len]` is the shared base
    /// context all branches started from; messages at `[original_context_len..]`
    /// are new messages produced by this branch.
    ///
    /// Evaluation strategies use this to extract the original user query and
    /// prior conversation history without separate bookkeeping.
    pub original_context_len: usize,
}

/// Return type of `agent_loop_parallel`.
///
/// Contains the evaluation winner's context and messages, plus all branch
/// outcomes for inspection. Feed `selected_context` into `agent_loop_continue()`
/// to resume the session normally after a parallel evaluation call.
pub struct ParallelLoopResult {
    /// Full context of the selected (winning) branch.
    pub selected_context: AgentContext,
    /// New messages from the selected branch (the agent's delivered response).
    pub selected_messages: Vec<AgentMessage>,
    /// 0-based index into the original `configs` slice identifying the winning branch.
    pub selected_index: usize,
    /// Remaining (non-selected) branch outcomes. The selected branch's context and messages
    /// are available via `selected_context` / `selected_messages`.
    pub all_outcomes: Vec<ParallelLoopOutcome>,
    /// Combined token usage: all branch usages + evaluation (judge) usage.
    pub total_usage: Usage,
}

// ---------------------------------------------------------------------------
// Input filtering
// ---------------------------------------------------------------------------

/// Result of applying an input filter to a user message.
#[derive(Debug, Clone)]
pub enum FilterResult {
    /// Message passes unchanged.
    Pass,
    /// Message passes, but append a warning to context for the LLM to see.
    Warn(String),
    /// Message is rejected. Agent loop returns immediately.
    Reject(String),
}

/// Synchronous filter applied to user input before the LLM call.
///
/// Implement this for injection detection, content moderation, PII redaction, etc.
/// Filters run in the hot path and must be fast — use `before_turn` callbacks
/// for async moderation (external API calls).
/*
RUST QUIRK: Trait as interface (no `async` here — intentional)

InputFilter is deliberately *synchronous*. Why? Filters run in the hot path
before every LLM call. Async would require `.await`, which adds complexity
and forces callers into async context. For CPU-bound work (regex, keyword scan)
synchronous is faster.

For async filtering (external API call to a moderation service), use the
`before_turn` callback hook instead — it's async and can return false to abort.

The `Send + Sync` supertrait bounds mean:
  Send  → the filter can be moved to another thread
  Sync  → the filter can be shared by reference across threads

These are required because filters are stored in `Vec<Arc<dyn InputFilter>>`
inside AgentLoopConfig. The agent loop may run on any tokio thread, so the
filters must be thread-safe.

Python analogy: an abstract base class with one required method:
  class InputFilter(ABC):
      @abstractmethod
      def filter(self, text: str) -> FilterResult: ...
*/
pub trait InputFilter: Send + Sync {
    fn filter(&self, text: &str) -> FilterResult;
}

// ---------------------------------------------------------------------------
// Evaluation strategy (trait + decision type)
// ---------------------------------------------------------------------------
//
// Defined here (not in evaluation.rs) so that `agent_loop_parallel` can accept
// `Arc<dyn EvaluationStrategy>` without creating a circular dependency:
//   agent_loop.rs → types.rs  ✓
//   evaluation.rs → agent_loop.rs + types.rs  ✓  (no cycle)

/// The decision returned by an [`EvaluationStrategy`] after reviewing all branch outcomes.
pub enum EvaluationDecision {
    /// Use the outcome at this 0-based index from the outcomes slice.
    Select(usize),
}

/// Pluggable strategy that selects the best result from a set of parallel branch outcomes.
///
/// Implementations receive all branch outcomes plus the original prompts, then
/// return an [`EvaluationDecision`] and any [`Usage`] incurred during evaluation
/// (e.g., a judge LLM call). The usage is added to [`ParallelLoopResult::total_usage`].
///
/// # Contract
///
/// - `prompts` and `outcomes` are guaranteed non-empty by `agent_loop_parallel`.
/// - The returned index must be `< outcomes.len()`; `agent_loop_parallel` clamps it.
/// - Events may be forwarded to `tx` (e.g., for a judge agent loop); none are required.
///
/// Built-in implementations live in [`crate::evaluation`].
#[async_trait::async_trait]
pub trait EvaluationStrategy: Send + Sync {
    /// Evaluate all branch outcomes and select the best one.
    async fn evaluate(
        &self,
        prompts: &[AgentMessage],
        outcomes: &[ParallelLoopOutcome],
        tx: &tokio::sync::mpsc::UnboundedSender<super::event::AgentEvent>,
        cancel: CancellationToken,
    ) -> (EvaluationDecision, Usage);
}
