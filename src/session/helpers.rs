use super::model::*;
use crate::types::*;

/// Extract `session_id` from a `loop_id`.
///
/// Loop ids follow the format `{session_id}.{config_segment}.{N}`. `session_id`
/// is a UUID (e.g. `550e8400-e29b-41d4-a716-446655440000`) — it contains hyphens
/// but no dots. The first `.` in the `loop_id` is always the boundary between the
/// UUID and the rest.
pub(super) fn session_id_from_loop_id(loop_id: &str) -> String {
    match loop_id.find('.') {
        Some(pos) => loop_id[..pos].to_string(),
        None => loop_id.to_string(),
    }
}

/// Return the `loop_id` from events that carry one but are handled by the catch-all arm.
pub(super) fn loop_id_of(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::TurnStart { loop_id, .. } => Some(loop_id),
        AgentEvent::MessageStart { loop_id, .. } => Some(loop_id),
        AgentEvent::MessageEnd { loop_id, .. } => Some(loop_id),
        AgentEvent::ToolExecutionStart { loop_id, .. } => Some(loop_id),
        AgentEvent::ToolExecutionUpdate { loop_id, .. } => Some(loop_id),
        AgentEvent::ProgressMessage { loop_id, .. } if !loop_id.is_empty() => Some(loop_id),
        AgentEvent::InputRejected { loop_id, .. } if !loop_id.is_empty() => Some(loop_id),
        AgentEvent::CompactionStarted { loop_id, .. } => Some(loop_id),
        AgentEvent::CompactionEnded { loop_id, .. } => Some(loop_id),
        _ => None,
    }
}

/// Extract the config segment from a `loop_id` of the form
/// `{session_id}.{config_segment}.{N}`.
///
/// Returns `None` if the `loop_id` does not contain at least two `.` separators.
pub(super) fn config_segment_from_loop_id(loop_id: &str) -> Option<String> {
    let first = loop_id.find('.')?;
    let after = &loop_id[first + 1..];
    let last = after.rfind('.')?;
    Some(after[..last].to_string())
}

/// Extract a [`LoopConfigSnapshot`] from a slice of messages, using the first
/// `Message::Assistant` found.
///
/// `loop_id` is used to populate [`LoopConfigSnapshot::config_id`] by parsing
/// the `config_segment` component of the `{session_id}.{config_segment}.{N}` format.
pub(super) fn extract_config_snapshot(
    messages: &[AgentMessage],
    loop_id: &str,
) -> Option<LoopConfigSnapshot> {
    messages.iter().find_map(|m| {
        if let AgentMessage::Llm(LlmMessage {
            message: Message::Assistant {
                model, provider, ..
            },
            ..
        }) = m
        {
            Some(LoopConfigSnapshot {
                model: model.clone(),
                provider: provider.clone(),
                config_id: config_segment_from_loop_id(loop_id),
            })
        } else {
            None
        }
    })
}
