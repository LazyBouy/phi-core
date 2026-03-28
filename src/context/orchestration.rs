use super::compaction::*;
use super::config::*;
use super::strategy::*;
use super::token::total_tokens;
use crate::session::Session;
use crate::types::*;

// ---------------------------------------------------------------------------
// Compaction orchestration — cross-loop block creation
// ---------------------------------------------------------------------------

/// Resolve `CompactionScope` to a concrete number of earlier loops to include.
///
/// For `FixedCount(n)`, returns `n` directly.
/// For `TokenBudget`, walks the chain backward from the current loop,
/// accumulating `total_tokens(&record.messages)` per loop, and stops
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
) -> usize {
    match scope {
        CompactionScope::FixedCount(n) => *n,
        CompactionScope::TokenBudget => {
            let mut budget = max_context_tokens;
            let mut count = 0usize;
            // Walk backward from the loop before current (chain.last() is current)
            for loop_id in chain.iter().rev().skip(1) {
                if let Some(record) = session.get_loop(loop_id) {
                    let loop_tokens = total_tokens(&record.messages);
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
/// The caller is responsible for persisting the session to disk afterward.
pub fn compact_session_loops(
    session: &mut Session,
    current_loop_id: &str,
    strategy: &dyn BlockCompactionStrategy,
    config: &CompactionConfig,
    max_context_tokens: usize,
) {
    let chain = session.loop_chain_to(current_loop_id);

    // 1. Compact current loop (most recent — all three sections)
    if let Some(current) = session.get_loop_mut(current_loop_id) {
        current.compaction_block = Some(strategy.compact(current, config, true));
    }

    // 2. Resolve scope, then compact earlier loops on the chain (only keep_compacted)
    let earlier_count = resolve_scope(
        session,
        &chain,
        &config.compaction_scope,
        max_context_tokens,
    )
    .min(chain.len().saturating_sub(1));
    let earlier_start = chain.len().saturating_sub(1 + earlier_count);
    for loop_id in &chain[earlier_start..chain.len().saturating_sub(1)] {
        if let Some(record) = session.get_loop_mut(loop_id) {
            if record.compaction_block.is_none() {
                record.compaction_block = Some(strategy.compact(record, config, false));
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
pub fn build_context_from_session(
    session: &Session,
    current_loop_id: &str,
    config: &CompactionConfig,
    max_context_tokens: usize,
) -> Vec<AgentMessage> {
    let chain = session.loop_chain_to(current_loop_id);
    let mut context = Vec::new();

    let earlier_count = resolve_scope(
        session,
        &chain,
        &config.compaction_scope,
        max_context_tokens,
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
