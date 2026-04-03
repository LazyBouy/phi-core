//! Model-invocable tool for surgical context pruning (2-stream architecture).

use crate::types::*;
use std::sync::{Arc, Mutex};

/// A request from the prun tool to remove in-run context.
#[derive(Debug, Clone)]
pub struct PrunRequest {
    pub tokens_to_remove: usize,
    pub memo: Option<String>,
}

/// Structured metadata stored in prun ToolResult for session reconstruction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrunRecord {
    pub pruned_timestamps: Vec<u64>,
    pub tokens_removed: usize,
    pub memo: Option<String>,
}

/// Which variant of the prun tool this instance represents.
#[derive(Debug, Clone, Copy)]
pub enum PrunVariant {
    /// Remove without memo.
    Prun,
    /// Remove with summary memo.
    PrunWithMemo,
}

/// Model-invocable tool for surgical context pruning.
pub struct PrunTool {
    pending: Arc<Mutex<Vec<PrunRequest>>>,
    variant: PrunVariant,
}

impl PrunTool {
    pub fn new(pending: Arc<Mutex<Vec<PrunRequest>>>, variant: PrunVariant) -> Self {
        Self { pending, variant }
    }
}

#[async_trait::async_trait]
impl AgentTool for PrunTool {
    fn name(&self) -> &str {
        match self.variant {
            PrunVariant::Prun => "prun",
            PrunVariant::PrunWithMemo => "prun_with_memo",
        }
    }

    fn label(&self) -> &str {
        match self.variant {
            PrunVariant::Prun => "Prun",
            PrunVariant::PrunWithMemo => "Prun with Memo",
        }
    }

    fn description(&self) -> &str {
        match self.variant {
            PrunVariant::Prun => "Surgically remove the last N tokens of model-generated (in-run) context. Use when exploration or tool results waste context length. Pruned content is preserved in session log.",
            PrunVariant::PrunWithMemo => "Surgically remove the last N tokens of in-run context and replace with a summary memo. Use when exploration had findings worth remembering but full content is too verbose.",
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        match self.variant {
            PrunVariant::Prun => serde_json::json!({
                "type": "object",
                "properties": {
                    "tokens": {"type": "integer", "description": "Tokens to remove from tail of in-run context"}
                },
                "required": ["tokens"]
            }),
            PrunVariant::PrunWithMemo => serde_json::json!({
                "type": "object",
                "properties": {
                    "tokens": {"type": "integer", "description": "Tokens to remove from tail of in-run context"},
                    "memo": {"type": "string", "description": "Summary to insert in place of pruned content"}
                },
                "required": ["tokens", "memo"]
            }),
        }
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let tokens = params.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        if tokens == 0 {
            return Err(ToolError::InvalidArgs("tokens must be > 0".to_string()));
        }
        let memo = match self.variant {
            PrunVariant::PrunWithMemo => params
                .get("memo")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            PrunVariant::Prun => None,
        };
        self.pending.lock().unwrap().push(PrunRequest {
            tokens_to_remove: tokens,
            memo,
        });
        Ok(ToolResult {
            content: vec![Content::Text {
                text: format!(
                    "Prun request recorded: {} tokens will be removed before next turn.",
                    tokens
                ),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}
