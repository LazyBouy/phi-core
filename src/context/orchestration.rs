use super::compaction::*;
use super::config::*;
use super::strategy::*;
use super::token::{resolve_counter, TokenCounter};
use crate::session::Session;
use crate::types::*;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Compaction orchestration — cross-loop block creation
// ---------------------------------------------------------------------------

/// Resolve `CompactionScope` to a concrete number of earlier loops to include.
///
/// For `FixedCount(n)`, returns `n` directly.
/// For `TokenBudget`, walks the chain backward from the current loop,
/// accumulating token estimates per loop, and stops
/// when `max_context_tokens` would be exceeded.
///
/// Note: with `TokenBudget`, the scope can include loops whose raw messages
/// exceed the token budget. This is intentional — the compacted summaries
/// will fit in the window even when the originals don't, enabling richer
/// context for expensive summarisation strategies.
fn resolve_scope(
    session: &Session,
    chain: &[String],
    scope: &CompactionScope,
    max_context_tokens: usize,
    counter: &dyn TokenCounter,
) -> usize {
    match scope {
        CompactionScope::FixedCount(n) => *n,
        CompactionScope::TokenBudget => {
            let mut budget = max_context_tokens;
            let mut count = 0usize;
            // Walk backward from the loop before current (chain.last() is current)
            for loop_id in chain.iter().rev().skip(1) {
                if let Some(record) = session.get_loop(loop_id) {
                    let loop_tokens = counter.estimate_messages(&record.messages);
                    if loop_tokens > budget {
                        break;
                    }
                    budget -= loop_tokens;
                    count += 1;
                }
            }
            count
        }
    }
}

/// Create `CompactionBlock`s for the current loop and earlier loops within scope.
/// Mutates the session in place.
///
/// When `counter` is `None`, uses `HeuristicTokenCounter` (chars/4) as the default.
/// The caller is responsible for persisting the session to disk afterward.
///
/// **0.9.0 breaking change**: this function is now `async fn` so it can drive
/// the async `BlockCompactionStrategy::compact` method. Callers must `.await`
/// the call; synchronous callers can use `tokio::runtime::Handle::current()
/// .block_on(...)` if no awaiter is available (uncommon — session compaction
/// is typically invoked from within an agent loop, which is already async).
pub async fn compact_session_loops(
    session: &mut Session,
    current_loop_id: &str,
    strategy: &dyn BlockCompactionStrategy,
    config: &CompactionConfig,
    max_context_tokens: usize,
    counter: Option<&Arc<dyn TokenCounter>>,
) {
    let counter = resolve_counter(counter);
    let chain = session.loop_chain_to(current_loop_id);

    // 1. Compact current loop (most recent — all three sections)
    //
    // We compute the block first (so the &mut borrow is released before the
    // .await suspension point) to keep the borrow checker happy across the
    // async boundary.
    let current_block = if let Some(current) = session.get_loop(current_loop_id) {
        Some(strategy.compact(current, config, true).await)
    } else {
        None
    };
    if let Some(block) = current_block {
        if let Some(current) = session.get_loop_mut(current_loop_id) {
            current.compaction_block = Some(block);
        }
    }

    // 2. Resolve scope, then compact earlier loops on the chain (only keep_compacted)
    let earlier_count = resolve_scope(
        session,
        &chain,
        &config.compaction_scope,
        max_context_tokens,
        counter,
    )
    .min(chain.len().saturating_sub(1));
    let earlier_start = chain.len().saturating_sub(1 + earlier_count);
    let earlier_ids: Vec<String> = chain[earlier_start..chain.len().saturating_sub(1)].to_vec();
    for loop_id in earlier_ids {
        // Compute block first (immutable borrow) so the .await can run, then
        // re-borrow mutably to assign.
        let needs_block = session
            .get_loop(&loop_id)
            .map(|r| r.compaction_block.is_none())
            .unwrap_or(false);
        if !needs_block {
            continue;
        }
        let block_opt = if let Some(record) = session.get_loop(&loop_id) {
            Some(strategy.compact(record, config, false).await)
        } else {
            None
        };
        if let Some(block) = block_opt {
            if let Some(record) = session.get_loop_mut(&loop_id) {
                record.compaction_block = Some(block);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Context builder — loads from CompactionBlocks when available
// ---------------------------------------------------------------------------

/// Build a compacted context by walking the loop chain and loading from
/// `CompactionBlock`s where available, raw messages otherwise.
///
/// For the most recent loop: loads keep_first + keep_compacted + keep_recent.
/// For older loops: loads only keep_compacted.
/// Loops outside the resolved scope are skipped entirely.
///
/// When `counter` is `None`, uses `HeuristicTokenCounter` (chars/4) as the default.
pub fn build_context_from_session(
    session: &Session,
    current_loop_id: &str,
    config: &CompactionConfig,
    max_context_tokens: usize,
    counter: Option<&Arc<dyn TokenCounter>>,
) -> Vec<AgentMessage> {
    let counter = resolve_counter(counter);
    let chain = session.loop_chain_to(current_loop_id);
    let mut context = Vec::new();

    let earlier_count = resolve_scope(
        session,
        &chain,
        &config.compaction_scope,
        max_context_tokens,
        counter,
    );
    let load_start = chain.len().saturating_sub(earlier_count + 1);

    for (i, loop_id) in chain.iter().enumerate().skip(load_start) {
        let Some(record) = session.get_loop(loop_id) else {
            continue;
        };
        let is_most_recent = i == chain.len() - 1;

        match &record.compaction_block {
            Some(block) => {
                if is_most_recent {
                    // Load keep_first (original messages for that range)
                    if let Some(ref range) = block.keep_first {
                        let turn_map = TurnMap::from_messages(&record.messages);
                        let msgs = turn_map.messages_for_range(range, &record.messages);
                        context.extend_from_slice(msgs);
                    }
                    // Load keep_compacted (summarised middle)
                    if let Some(ref section) = block.keep_compacted {
                        context.extend(section.messages.iter().cloned());
                    }
                    // Load keep_recent (truncated tool outputs)
                    if let Some(ref section) = block.keep_recent {
                        context.extend(section.messages.iter().cloned());
                    }
                } else {
                    // Older loops: only load keep_compacted
                    if let Some(ref section) = block.keep_compacted {
                        context.extend(section.messages.iter().cloned());
                    }
                }
            }
            None => {
                // No compaction block — load raw messages
                context.extend(record.messages.iter().cloned());
            }
        }
    }

    context
}
