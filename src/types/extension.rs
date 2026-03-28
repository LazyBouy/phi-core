use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// AgentMessage — LLM messages + extensible custom types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionMessage {
    pub role: String,
    // Here it'S more like a channel or category for the message, not diretcly "user"/"assistant"/"toolResult".
    // This allows flexibility for app-specific messages (e.g., "notification", "system", "debug") that aren't part of the LLM conversation
    // but still need to be handled in the agent loop and UI.
    // A proper naming should have been "type" or "category" instead of "role",
    // but we use "role" here for consistency with the Message enum's role field and to leverage the same serde tagging mechanism.
    pub kind: String,
    pub data: serde_json::Value,
}

impl ExtensionMessage {
    pub fn new(kind: impl Into<String>, data: impl Serialize) -> Self {
        Self {
            role: "extension".into(),
            kind: kind.into(), // .into() makes a ownable String from the input (which can be &str or String)
            data: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
            // Convert the input data to a serde_json::Value. If serialization fails, use Null as a fallback.
            // to_value() returns a Result<Value, Error>. unwrap_or(fallback) means "give me the Ok value, or use Null if it failed" — no panic
        }
    }
}
