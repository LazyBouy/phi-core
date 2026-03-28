use crate::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Compaction Block — non-destructive overlay on LoopRecord
// ---------------------------------------------------------------------------

/// Non-destructive compaction overlay. Stored on `LoopRecord` alongside
/// the original messages. When present, the context loader uses this block
/// instead of raw messages.
///
/// Three sections control what gets loaded into context:
/// - `keep_first`: turns kept verbatim from the start (most recent loop only)
/// - `keep_recent`: recent turns with truncated tool outputs (most recent loop only)
/// - `keep_compacted`: fully summarised section (all loops)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionBlock {
    /// Turns kept verbatim from the start of the loop.
    /// Only populated for the MOST RECENT loop. For older loops this is `None`.
    /// During context load: original messages in this range are used as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_first: Option<TurnRange>,

    /// Recent turns with tool outputs truncated. Rest unchanged.
    /// Only populated for the MOST RECENT loop. For older loops this is `None`.
    /// Invariant: if a ToolCall is in range, its corresponding ToolResult is too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_recent: Option<CompactedSection>,

    /// Fully summarised middle section (most recent loop) or entire loop (older loops).
    /// Relevant for ALL loops — this is what gets loaded from earlier loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_compacted: Option<CompactedSection>,

    /// When this block was created.
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
}

/// A range of turns within a loop, identified by turn indices.
/// Both bounds are inclusive. These correspond to `TurnId.turn_index` values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRange {
    #[serde(rename = "startTurn")]
    pub start_turn: u32,
    #[serde(rename = "endTurn")]
    pub end_turn: u32,
}

/// A range of turns plus the compacted replacement messages for that range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactedSection {
    /// The turn range this section replaces.
    pub range: TurnRange,
    /// Replacement messages loaded into context instead of the originals.
    pub messages: Vec<AgentMessage>,
}

// ---------------------------------------------------------------------------
// TurnMap — maps turn indices to message index ranges
// ---------------------------------------------------------------------------

/// Maps turn indices to message index ranges within a message array.
/// Built from messages by grouping on `TurnId.turn_index`.
pub struct TurnMap {
    /// Indexed by position (0-based). Each entry is `(start_msg_idx, end_msg_idx)` inclusive.
    entries: Vec<(usize, usize)>,
}

impl TurnMap {
    /// Build from messages by grouping on `turn_id.turn_index`.
    /// Messages without a `turn_id` are treated as their own single-message group.
    pub fn from_messages(messages: &[AgentMessage]) -> Self {
        let mut entries: Vec<(usize, usize)> = Vec::new();
        let mut current_turn: Option<u32> = None;

        for (i, msg) in messages.iter().enumerate() {
            let turn_idx = msg.turn_id().map(|t| t.turn_index);
            match (turn_idx, current_turn) {
                (Some(idx), Some(cur)) if idx == cur => {
                    // Same turn — extend end index
                    if let Some(last) = entries.last_mut() {
                        last.1 = i;
                    }
                }
                (Some(idx), _) => {
                    // New turn
                    entries.push((i, i));
                    current_turn = Some(idx);
                }
                (None, _) => {
                    // Legacy message without turn_id — treat as its own group
                    entries.push((i, i));
                    current_turn = None;
                }
            }
        }

        Self { entries }
    }

    /// Number of turn groups.
    pub fn turn_count(&self) -> u32 {
        self.entries.len() as u32
    }

    /// Slice of messages belonging to a `TurnRange`.
    pub fn messages_for_range<'a>(
        &self,
        range: &TurnRange,
        all_msgs: &'a [AgentMessage],
    ) -> &'a [AgentMessage] {
        if range.start_turn as usize >= self.entries.len()
            || range.end_turn as usize >= self.entries.len()
        {
            return &[];
        }
        let start = self.entries[range.start_turn as usize].0;
        let end = self.entries[range.end_turn as usize].1;
        &all_msgs[start..=end]
    }

    /// Message index range `(start, end)` inclusive for a single turn.
    pub fn turn_msg_range(&self, turn_index: u32) -> Option<(usize, usize)> {
        self.entries.get(turn_index as usize).copied()
    }
}
