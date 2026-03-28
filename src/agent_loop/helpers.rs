use super::config::*;
use crate::types::*;
use tokio::sync::mpsc;

/// Default convert_to_llm: keep only user/assistant/toolResult messages.
pub(super) fn default_convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|m| m.as_llm().cloned())
        .collect()
}

/// Derive a stable config segment string from an [`AgentLoopConfig`].
///
/// Used as the middle segment of each branch's `loop_id`:
///   `"{session_id}.{config_segment}.{N}"`
///
/// When `config.config_id` is set, that value is returned as-is. Otherwise the
/// segment is derived from the provider, model slug, and thinking level --- the same
/// logic used by `BasicAgent::next_loop_id`.
pub(crate) fn derive_config_segment(config: &AgentLoopConfig) -> String {
    if let Some(ref id) = config.config_id {
        return id.clone();
    }
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
}

/// Apply input filters to a batch of messages before injecting them into context.
///
/// Mirrors the pre-run filter logic in `agent_loop()` but is usable at any point:
///
/// - `Ok(messages)` --- all filters passed; any `Warn` text has been appended to the last
///   `Message::User` in the batch.
/// - `Err(reason)` --- a filter rejected the input; `AgentEvent::InputRejected` has already
///   been sent on `tx`. The caller is responsible for any further cleanup (e.g. returning
///   early from the loop).
///
/// Text extraction: only `Content::Text` blocks inside `Message::User` messages are fed to
/// filters. Non-user messages (assistant, tool results, extension) are passed through
/// unchanged.
pub(super) fn apply_input_filters(
    mut messages: Vec<AgentMessage>,
    filters: &[std::sync::Arc<dyn InputFilter>],
    tx: &mpsc::UnboundedSender<AgentEvent>,
    loop_id: &str,
) -> Result<Vec<AgentMessage>, String> {
    if filters.is_empty() || messages.is_empty() {
        return Ok(messages);
    }

    // Extract text from all User messages in the batch
    let user_text: String = messages
        .iter()
        .filter_map(|m| {
            if let AgentMessage::Llm(LlmMessage {
                message: Message::User { content, .. },
                ..
            }) = m
            {
                Some(
                    content
                        .iter()
                        .filter_map(|c| {
                            if let Content::Text { text } = c {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut warnings: Vec<String> = Vec::new();
    for filter in filters {
        match filter.filter(&user_text) {
            FilterResult::Pass => {}
            FilterResult::Warn(w) => warnings.push(w),
            FilterResult::Reject(reason) => {
                tx.send(AgentEvent::InputRejected {
                    loop_id: loop_id.to_string(),
                    reason: reason.clone(),
                })
                .ok();
                return Err(reason);
            }
        }
    }

    // Append accumulated warnings to the last User message
    if !warnings.is_empty() {
        let warning_text = warnings
            .iter()
            .map(|w| format!("[Warning: {}]", w))
            .collect::<Vec<_>>()
            .join("\n");
        for msg in messages.iter_mut().rev() {
            if let AgentMessage::Llm(LlmMessage {
                message: Message::User { content, .. },
                ..
            }) = msg
            {
                content.push(Content::Text { text: warning_text });
                break;
            }
        }
    }

    Ok(messages)
}
