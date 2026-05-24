use super::content::Message;
// Content is used indirectly via Message; not directly imported
use super::extension::ExtensionMessage;
use super::node_tag::{NodeId, NodeTag};
use super::provenance::BlockProvenance;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/*
ARCHITECTURE: AgentMessage — the agent loop's two-lane routing envelope

The agent loop operates with two layers of "messages":

  `Message`      = LLM VOCABULARY — the exact types the provider API understands:
                   User, Assistant, ToolResult. Maps 1:1 to the JSON payload
                   sent to the provider. The LLM only ever sees these.

  `AgentMessage` = AGENT VOCABULARY — a routing envelope around Message.
                   Decides: does this content go INTO the LLM context window,
                   or SIDEWAYS to the UI/app without consuming tokens?

WHY TWO LAYERS?
  The agent needs to track content meaningful to the app that must NEVER reach
  the LLM — UI notifications, debug events, session metadata, progress markers.
  Sending these to the LLM would waste the context budget and confuse the model.

  `AgentMessage` solves this with two variants:
    Llm(Message)               — enters the LLM context; serialized into the API request
    Extension(ExtensionMessage) — NEVER enters the context window; only emitted as AgentEvents

  Think of it as a fork in the road: every message must declare which lane it
  belongs to at construction time. Once tagged Extension, it can never leak
  into the LLM context — the type system enforces this rule at compile time.

RUST QUIRK: `#[serde(untagged)]`
  By default, serde serializes enum variants with a discriminator key:
    {"Llm": {"role": "user", ...}} or {"Extension": {"role": "extension", ...}}
  With `#[serde(untagged)]`, the variant wrapper is omitted — only the inner
  value is serialized:
    {"role": "user", ...}       ← Message (Llm variant)
    {"role": "extension", ...}  ← ExtensionMessage (Extension variant)
  Deserialization tries each variant in order until one succeeds. The `role`
  field acts as the natural discriminator: LLM roles ("user"/"assistant"/
  "tool_result") vs the "extension" sentinel value.

RUST QUIRK: Tuple-style variants (Llm(Message), Extension(ExtensionMessage))
  Both variants are tuple-style — each wraps one existing, fully-formed type.
  No field names are needed because the wrapped type is already self-describing.
  This signals design intent: "I am ROUTING this object, not reshaping it."
  Compare to Content::Text { text: String } (struct-style) — that defines a
  NEW data shape. AgentMessage constructs nothing new; it only classifies.

ONE-WAY CONVERSION: `impl From<Message> for AgentMessage`
  Only `Message → AgentMessage::Llm` exists; there is no path for
  ExtensionMessage to become an Llm variant. This is intentional:
  UI-only content can never accidentally slip into the LLM context.
  The compiler catches the mistake — no runtime guard needed.

WHERE AgentMessage IS USED:
  AgentContext.messages         — full conversation history (includes Extension messages)
  AgentEvent::MessageStart/End  — real-time streaming events carry AgentMessage
  AgentEvent::TurnEnd           — the completed turn message
  AgentEvent::AgentEnd          — all new messages produced in a run
  Session history               — Extension messages are part of the full agent story,
                                  not just the LLM's narrower view of it
*/
// ---------------------------------------------------------------------------
// TurnId — identifies which turn produced a message.
// ---------------------------------------------------------------------------

/// Identifies the turn that produced a message, linking it to a specific
/// loop and turn index within that loop.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId {
    #[serde(rename = "loopId")]
    pub loop_id: String,
    #[serde(rename = "turnIndex")]
    pub turn_index: u32,
}

// ---------------------------------------------------------------------------
// LlmMessage — wraps a Message with optional TurnId metadata.
// ---------------------------------------------------------------------------

/// An LLM-bound message with optional turn-tracking metadata and (Composition I)
/// optional tree-node identity + summary tags.
///
/// Serializes as the same JSON as `Message` with optional `turnId` / `nodeId` /
/// `parentId` / `tags` keys added only when set. Old data without any of these
/// keys deserializes with the corresponding fields defaulting to `None` / empty.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmMessage {
    pub message: Message,
    /// Which turn produced this message. `None` for messages that predate
    /// turn tracking or are created outside the agent loop.
    pub turn_id: Option<TurnId>,
    /// Node identity for Composition I (the opt-in tree braking layer).
    /// `None` unless revert mode is active (i.e. the consumer called
    /// [`BasicAgent::with_revert_tool`](crate::agents::BasicAgent::with_revert_tool)).
    pub node_id: Option<NodeId>,
    /// Parent in the conversation tree (Composition I). `None` for root
    /// messages or when revert mode is off.
    pub parent_id: Option<NodeId>,
    /// Summary annotations attached by `apply_revert`. Empty unless this node
    /// has been the target of a revert that produced a summary.
    pub tags: Vec<NodeTag>,
    /// Provenance hint stamped by upstream consumers (identity loader, memory
    /// store, etc.) before the message is fed into the agent loop. phi-core
    /// reads this stamp during request assembly to populate the parallel
    /// `provenance` vec in [`crate::AnnotatedRequestPayload`]; falls back to
    /// deriving a tag from `turn_id` + role when `None`. Added in 0.9.0;
    /// omitted from serialized output when `None` for back-compat.
    ///
    /// Boxed so `LlmMessage` doesn't bloat `AgentMessage`'s enum size with the
    /// rare-stamped variant data.
    pub provenance_hint: Option<Box<BlockProvenance>>,
}

impl Serialize for LlmMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialize Message to a JSON Value, then inject metadata keys (turnId,
        // nodeId, parentId, tags) only when set / non-empty. Keys are omitted
        // when at their default so old/non-revert-mode JSON round-trips
        // unchanged.
        let mut value = serde_json::to_value(&self.message).map_err(serde::ser::Error::custom)?;
        if let serde_json::Value::Object(ref mut map) = value {
            if let Some(ref tid) = self.turn_id {
                map.insert(
                    "turnId".to_string(),
                    serde_json::to_value(tid).map_err(serde::ser::Error::custom)?,
                );
            }
            if let Some(ref nid) = self.node_id {
                map.insert(
                    "nodeId".to_string(),
                    serde_json::to_value(nid).map_err(serde::ser::Error::custom)?,
                );
            }
            if let Some(ref pid) = self.parent_id {
                map.insert(
                    "parentId".to_string(),
                    serde_json::to_value(pid).map_err(serde::ser::Error::custom)?,
                );
            }
            if !self.tags.is_empty() {
                map.insert(
                    "tags".to_string(),
                    serde_json::to_value(&self.tags).map_err(serde::ser::Error::custom)?,
                );
            }
            if let Some(ref ph) = self.provenance_hint {
                map.insert(
                    "provenanceHint".to_string(),
                    serde_json::to_value(ph).map_err(serde::ser::Error::custom)?,
                );
            }
        }
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LlmMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialize to a JSON Value, extract turnId / nodeId / parentId / tags
        // if present (all are optional and default to None / empty), then
        // deserialize the rest as Message.
        let mut value: serde_json::Value = Deserialize::deserialize(deserializer)?;
        let (turn_id, node_id, parent_id, tags, provenance_hint) =
            if let serde_json::Value::Object(ref mut map) = value {
                let turn_id = map
                    .remove("turnId")
                    .and_then(|v| serde_json::from_value(v).ok());
                let node_id = map
                    .remove("nodeId")
                    .and_then(|v| serde_json::from_value(v).ok());
                let parent_id = map
                    .remove("parentId")
                    .and_then(|v| serde_json::from_value(v).ok());
                let tags = map
                    .remove("tags")
                    .and_then(|v| serde_json::from_value::<Vec<NodeTag>>(v).ok())
                    .unwrap_or_default();
                let provenance_hint = map
                    .remove("provenanceHint")
                    .and_then(|v| serde_json::from_value(v).ok());
                (turn_id, node_id, parent_id, tags, provenance_hint)
            } else {
                (None, None, None, Vec::new(), None)
            };
        let message = Message::deserialize(value).map_err(serde::de::Error::custom)?;
        Ok(LlmMessage {
            message,
            turn_id,
            node_id,
            parent_id,
            tags,
            provenance_hint,
        })
    }
}

impl LlmMessage {
    /// Create a new LlmMessage without turn tracking and without Composition I
    /// node identity. All optional metadata defaults to `None` / empty.
    pub fn new(message: Message) -> Self {
        Self {
            message,
            turn_id: None,
            node_id: None,
            parent_id: None,
            tags: Vec::new(),
            provenance_hint: None,
        }
    }

    /// Create a new LlmMessage with a specific turn.
    pub fn with_turn(message: Message, turn_id: TurnId) -> Self {
        Self {
            message,
            turn_id: Some(turn_id),
            node_id: None,
            parent_id: None,
            tags: Vec::new(),
            provenance_hint: None,
        }
    }

    /// Stamp the block-provenance hint on this message. Consuming builder.
    ///
    /// Use this from upstream consumers (identity loaders, memory stores,
    /// steering interceptors) to label messages with their origin BEFORE
    /// emitting them into the agent loop. phi-core reads the stamp during
    /// request assembly to populate the parallel provenance vec in
    /// [`crate::AnnotatedRequestPayload`]. Added in 0.9.0.
    pub fn with_provenance_hint(mut self, hint: BlockProvenance) -> Self {
        self.provenance_hint = Some(Box::new(hint));
        self
    }

    /// Stamp the Composition I node identity (`node_id` + optional `parent_id`)
    /// onto this message. Consuming builder. Used by the agent loop when revert
    /// mode is active (gated on `config.revert_pending.is_some()` — Phase 4).
    pub fn with_node_identity(mut self, node_id: NodeId, parent_id: Option<NodeId>) -> Self {
        self.node_id = Some(node_id);
        self.parent_id = parent_id;
        self
    }

    /// Append a summary tag to this message. Used by `apply_revert` (Phase 3)
    /// to attach a Lesson / Finding / Outcome / Checkpoint to the
    /// revert-target node.
    pub fn add_tag(&mut self, tag: NodeTag) {
        self.tags.push(tag);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentMessage {
    /// LLM-bound message — enters the context window and is sent to the provider API.
    Llm(LlmMessage),
    /// App-only message — streamed as events but never enters the LLM context window.
    Extension(ExtensionMessage),
}

impl AgentMessage {
    pub fn role(&self) -> &str {
        match self {
            Self::Llm(lm) => lm.message.role(),
            Self::Extension(ext) => &ext.role,
        }
    }

    pub fn as_llm(&self) -> Option<&Message> {
        match self {
            Self::Llm(lm) => Some(&lm.message),
            Self::Extension(_) => None,
        }
    }

    /// Returns the TurnId if this is an LLM message with turn tracking.
    pub fn turn_id(&self) -> Option<&TurnId> {
        match self {
            Self::Llm(lm) => lm.turn_id.as_ref(),
            Self::Extension(_) => None,
        }
    }

    /// Returns the timestamp (unix millis) of the underlying message.
    /// Returns 0 for Extension messages or if no timestamp is available.
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::Llm(lm) => match &lm.message {
                Message::User { timestamp, .. } => *timestamp,
                Message::Assistant { timestamp, .. } => *timestamp,
                Message::ToolResult { timestamp, .. } => *timestamp,
            },
            Self::Extension(_) => 0,
        }
    }

    /// Set the turn_id on this message. No-op for Extension messages.
    pub fn with_turn_id(self, turn_id: Option<TurnId>) -> Self {
        match self {
            Self::Llm(mut lm) => {
                lm.turn_id = turn_id;
                Self::Llm(lm)
            }
            other => other,
        }
    }

    /// Stamp the Composition I node identity on this message. No-op for
    /// Extension messages (they never enter the LLM context and so are not
    /// part of the conversation tree).
    pub fn with_node_identity(self, node_id: NodeId, parent_id: Option<NodeId>) -> Self {
        match self {
            Self::Llm(lm) => Self::Llm(lm.with_node_identity(node_id, parent_id)),
            other => other,
        }
    }

    /// Returns the node_id if this is an LLM message with node identity
    /// stamped (Composition I). `None` for Extension messages and for LLM
    /// messages constructed outside revert mode.
    pub fn node_id(&self) -> Option<NodeId> {
        match self {
            Self::Llm(lm) => lm.node_id,
            Self::Extension(_) => None,
        }
    }

    /// Returns the parent_id if this is an LLM message with parent linkage
    /// stamped (Composition I). `None` for Extension messages, root messages,
    /// and LLM messages constructed outside revert mode.
    pub fn parent_id(&self) -> Option<NodeId> {
        match self {
            Self::Llm(lm) => lm.parent_id,
            Self::Extension(_) => None,
        }
    }

    /// Returns the tags attached to this message (Composition I annotation
    /// layer). Empty for Extension messages and for nodes that have not been
    /// the target of a revert.
    pub fn tags(&self) -> &[NodeTag] {
        match self {
            Self::Llm(lm) => &lm.tags,
            Self::Extension(_) => &[],
        }
    }
}

// One-way: Message → AgentMessage::Llm only (with turn_id: None).
impl From<Message> for AgentMessage {
    fn from(m: Message) -> Self {
        Self::Llm(LlmMessage::new(m))
    }
}

#[cfg(test)]
mod node_identity_tests {
    //! Backward-compat tests for the Composition I node-identity fields on
    //! [`LlmMessage`] (Phase 1). Confirms old 0.7.x-shaped JSON still loads
    //! cleanly under 0.8.0 and that defaults are omitted from output.
    use super::*;
    use crate::types::content::{Content, Message};
    use crate::types::node_tag::{NodeId, NodeTag, TagKind};

    #[test]
    fn old_llm_message_json_deserializes_with_defaults() {
        // 0.7.x-shaped JSON: no turnId, no nodeId/parentId/tags.
        let old_json =
            r#"{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":12345}"#;
        let lm: LlmMessage = serde_json::from_str(old_json).unwrap();
        assert!(lm.turn_id.is_none());
        assert!(lm.node_id.is_none());
        assert!(lm.parent_id.is_none());
        assert!(lm.tags.is_empty());
        // Underlying Message is preserved.
        match &lm.message {
            Message::User { content, timestamp } => {
                assert_eq!(*timestamp, 12345);
                assert_eq!(content.len(), 1);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn old_llm_message_with_turn_id_still_works() {
        // 0.7.x-shaped JSON with the existing turnId only — no node identity.
        let old_json = r#"{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":12345,"turnId":{"loopId":"abc","turnIndex":3}}"#;
        let lm: LlmMessage = serde_json::from_str(old_json).unwrap();
        let tid = lm.turn_id.as_ref().expect("turn_id present");
        assert_eq!(tid.loop_id, "abc");
        assert_eq!(tid.turn_index, 3);
        assert!(lm.node_id.is_none());
        assert!(lm.parent_id.is_none());
        assert!(lm.tags.is_empty());
    }

    #[test]
    fn defaults_omitted_from_serialized_json() {
        // A bare LlmMessage (no opt-in metadata) should serialize WITHOUT any
        // of the new keys — byte-for-byte compatible with 0.7.x readers.
        let lm = LlmMessage::new(Message::User {
            content: vec![Content::Text {
                text: "hi".to_string(),
            }],
            timestamp: 100,
        });
        let json = serde_json::to_string(&lm).unwrap();
        assert!(!json.contains("turnId"));
        assert!(!json.contains("nodeId"));
        assert!(!json.contains("parentId"));
        assert!(!json.contains("tags"));
    }

    #[test]
    fn full_roundtrip_with_all_new_fields() {
        let mut lm = LlmMessage::new(Message::User {
            content: vec![Content::Text {
                text: "hi".to_string(),
            }],
            timestamp: 100,
        })
        .with_node_identity(NodeId(5), Some(NodeId(4)));
        lm.add_tag(NodeTag::new(
            TagKind::Lesson,
            "test lesson".to_string(),
            2,
            vec![NodeId(3)],
        ));
        let json = serde_json::to_string(&lm).unwrap();
        let back: LlmMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.node_id, Some(NodeId(5)));
        assert_eq!(back.parent_id, Some(NodeId(4)));
        assert_eq!(back.tags.len(), 1);
        assert_eq!(back.tags[0].kind, TagKind::Lesson);
        assert_eq!(back.tags[0].text, "test lesson");
        assert_eq!(back.tags[0].abandoned_node_ids, vec![NodeId(3)]);
    }

    #[test]
    fn agent_message_with_node_identity_helper() {
        let am: AgentMessage = Message::User {
            content: vec![Content::Text {
                text: "hi".to_string(),
            }],
            timestamp: 100,
        }
        .into();
        let am = am.with_node_identity(NodeId(7), Some(NodeId(6)));
        assert_eq!(am.node_id(), Some(NodeId(7)));
        assert_eq!(am.parent_id(), Some(NodeId(6)));
        assert!(am.tags().is_empty());
    }
}
