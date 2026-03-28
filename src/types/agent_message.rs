use super::content::Message;
// Content is used indirectly via Message; not directly imported
use super::extension::ExtensionMessage;
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

/// An LLM-bound message with optional turn tracking metadata.
/// Serializes as the same JSON as `Message` with an optional `turnId` field added.
/// Old data without `turnId` deserializes with `turn_id: None`.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmMessage {
    pub message: Message,
    /// Which turn produced this message. `None` for messages that predate
    /// turn tracking or are created outside the agent loop.
    pub turn_id: Option<TurnId>,
}

impl Serialize for LlmMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialize Message to a JSON Value, then inject turnId if present.
        // This maintains the same serialization format as bare Message
        // with an optional turnId field added.
        let mut value = serde_json::to_value(&self.message).map_err(serde::ser::Error::custom)?;
        if let Some(ref tid) = self.turn_id {
            if let serde_json::Value::Object(ref mut map) = value {
                map.insert(
                    "turnId".to_string(),
                    serde_json::to_value(tid).map_err(serde::ser::Error::custom)?,
                );
            }
        }
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LlmMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialize to a JSON Value, extract turnId if present,
        // then deserialize the rest as Message.
        let mut value: serde_json::Value = Deserialize::deserialize(deserializer)?;
        let turn_id = if let serde_json::Value::Object(ref mut map) = value {
            map.remove("turnId")
                .and_then(|v| serde_json::from_value(v).ok())
        } else {
            None
        };
        let message = Message::deserialize(value).map_err(serde::de::Error::custom)?;
        Ok(LlmMessage { message, turn_id })
    }
}

impl LlmMessage {
    /// Create a new LlmMessage without turn tracking.
    pub fn new(message: Message) -> Self {
        Self {
            message,
            turn_id: None,
        }
    }

    /// Create a new LlmMessage with a specific turn.
    pub fn with_turn(message: Message, turn_id: TurnId) -> Self {
        Self {
            message,
            turn_id: Some(turn_id),
        }
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
}

// One-way: Message → AgentMessage::Llm only (with turn_id: None).
impl From<Message> for AgentMessage {
    fn from(m: Message) -> Self {
        Self::Llm(LlmMessage::new(m))
    }
}
