//! Generic ID reference protocol for config objects.
//!
//! Any config field that references another object uses the `{{...}}` syntax:
//!
//! | Pattern | Meaning |
//! |---------|---------|
//! | `{{type.name}}` | Qualified reference, recreate |
//! | `{{%type.name%}}` | Qualified reference, no recreation if exists |
//! | `{{name}}` | Unqualified (unique resolve), recreate |
//! | `{{%name%}}` | Unqualified (unique resolve), no recreation |
//! | `{{#system_id#}}` | Literal system ID, no recreation |
//!
//! ## Resolution rules
//!
//! - **Qualified**: look up `name` in the specified `type` namespace
//!   (e.g., `agent_profile` → `[[agent.profile.instances]]`)
//! - **Unqualified**: search all namespaces; must resolve uniquely or error
//! - **`%` delimiters**: skip creation if an object with matching description
//!   already exists. If multiple matches, use the one with the latest creation date.
//! - **No `%`**: always create a new instance
//! - **`#` delimiters**: literal system ID (already exists in external system)
//!
//! ## Namespaces
//!
//! | Qualified prefix | Config section |
//! |-----------------|----------------|
//! | `agent_profile` | `[[agent.profile.instances]]` |
//! | `provider` | `[[provider.instances]]` |
//! | `sub_agent` | `[[sub_agents.instances]]` |

use serde::{Deserialize, Serialize};

/// A parsed config object reference.
///
/// Produced by [`parse_config_ref`] from a raw string in a config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigRef {
    /// `{{type.name}}` or `{{%type.name%}}` — qualified reference.
    Qualified {
        /// The namespace type (e.g., `"agent_profile"`, `"provider"`, `"sub_agent"`).
        ref_type: String,
        /// The instance name within that namespace.
        name: String,
        /// Whether to recreate the object if it already exists.
        /// `false` when `%` delimiters are used.
        recreate: bool,
    },
    /// `{{name}}` or `{{%name%}}` — unqualified reference (must resolve uniquely).
    Unqualified {
        /// The instance name (searched across all namespaces).
        name: String,
        /// Whether to recreate the object if it already exists.
        recreate: bool,
    },
    /// `{{#system_id#}}` — literal system ID, no recreation.
    SystemId {
        /// The actual system ID.
        id: String,
    },
    /// Plain string — not a `{{...}}` reference.
    Literal(String),
}

impl ConfigRef {
    /// Returns the effective name for config-internal lookups.
    ///
    /// For `Qualified`, returns the `name`. For `Unqualified`, returns the `name`.
    /// For `SystemId`, returns the `id`. For `Literal`, returns the raw string.
    pub fn effective_name(&self) -> &str {
        match self {
            Self::Qualified { name, .. } => name,
            Self::Unqualified { name, .. } => name,
            Self::SystemId { id } => id,
            Self::Literal(s) => s,
        }
    }

    /// Whether this reference requests recreation.
    pub fn should_recreate(&self) -> bool {
        match self {
            Self::Qualified { recreate, .. } => *recreate,
            Self::Unqualified { recreate, .. } => *recreate,
            Self::SystemId { .. } => false,
            Self::Literal(_) => true,
        }
    }

    /// Whether this is a `{{...}}` reference (not a plain literal).
    pub fn is_reference(&self) -> bool {
        !matches!(self, Self::Literal(_))
    }
}

/// Parse a string that may contain a `{{...}}` reference pattern.
///
/// # Examples
///
/// ```
/// use phi_core::config::reference::{parse_config_ref, ConfigRef};
///
/// // Qualified, recreate
/// assert_eq!(
///     parse_config_ref("{{agent_profile.coder}}"),
///     ConfigRef::Qualified { ref_type: "agent_profile".into(), name: "coder".into(), recreate: true }
/// );
///
/// // Qualified, no recreation
/// assert_eq!(
///     parse_config_ref("{{%provider.openai%}}"),
///     ConfigRef::Qualified { ref_type: "provider".into(), name: "openai".into(), recreate: false }
/// );
///
/// // Unqualified, recreate
/// assert_eq!(
///     parse_config_ref("{{coder}}"),
///     ConfigRef::Unqualified { name: "coder".into(), recreate: true }
/// );
///
/// // System ID
/// assert_eq!(
///     parse_config_ref("{{#fctsidd-abc-123#}}"),
///     ConfigRef::SystemId { id: "fctsidd-abc-123".into() }
/// );
///
/// // Plain string
/// assert_eq!(
///     parse_config_ref("just-a-string"),
///     ConfigRef::Literal("just-a-string".into())
/// );
/// ```
pub fn parse_config_ref(s: &str) -> ConfigRef {
    let trimmed = s.trim();

    // Must start with {{ and end with }}
    if !trimmed.starts_with("{{") || !trimmed.ends_with("}}") {
        return ConfigRef::Literal(s.to_string());
    }

    let inner = &trimmed[2..trimmed.len() - 2];

    if inner.is_empty() {
        return ConfigRef::Literal(s.to_string());
    }

    // {{#system_id#}} — literal system ID
    if inner.starts_with('#') && inner.ends_with('#') && inner.len() > 2 {
        let id = &inner[1..inner.len() - 1];
        return ConfigRef::SystemId { id: id.to_string() };
    }

    // {{%name%}} or {{%type.name%}} — no recreation
    if inner.starts_with('%') && inner.ends_with('%') && inner.len() > 2 {
        let body = &inner[1..inner.len() - 1];
        return parse_qualified_or_unqualified(body, false);
    }

    // {{name}} or {{type.name}} — recreate
    parse_qualified_or_unqualified(inner, true)
}

/// Parse the inner body (after stripping `{{`, `}}`, and optional `%`) into
/// either a Qualified or Unqualified ref.
fn parse_qualified_or_unqualified(body: &str, recreate: bool) -> ConfigRef {
    // Check for type.name pattern (first dot separates type from name)
    if let Some(dot_pos) = body.find('.') {
        let ref_type = &body[..dot_pos];
        let name = &body[dot_pos + 1..];
        if !ref_type.is_empty() && !name.is_empty() {
            return ConfigRef::Qualified {
                ref_type: ref_type.to_string(),
                name: name.to_string(),
                recreate,
            };
        }
    }

    // Unqualified — just a name
    ConfigRef::Unqualified {
        name: body.to_string(),
        recreate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qualified_recreate() {
        assert_eq!(
            parse_config_ref("{{agent_profile.coder}}"),
            ConfigRef::Qualified {
                ref_type: "agent_profile".into(),
                name: "coder".into(),
                recreate: true,
            }
        );
    }

    #[test]
    fn test_qualified_no_recreate() {
        assert_eq!(
            parse_config_ref("{{%provider.openai%}}"),
            ConfigRef::Qualified {
                ref_type: "provider".into(),
                name: "openai".into(),
                recreate: false,
            }
        );
    }

    #[test]
    fn test_unqualified_recreate() {
        assert_eq!(
            parse_config_ref("{{coder}}"),
            ConfigRef::Unqualified {
                name: "coder".into(),
                recreate: true,
            }
        );
    }

    #[test]
    fn test_unqualified_no_recreate() {
        assert_eq!(
            parse_config_ref("{{%coder%}}"),
            ConfigRef::Unqualified {
                name: "coder".into(),
                recreate: false,
            }
        );
    }

    #[test]
    fn test_system_id() {
        assert_eq!(
            parse_config_ref("{{#fctsidd-abc-123#}}"),
            ConfigRef::SystemId {
                id: "fctsidd-abc-123".into(),
            }
        );
    }

    #[test]
    fn test_literal() {
        assert_eq!(
            parse_config_ref("just-a-string"),
            ConfigRef::Literal("just-a-string".into())
        );
    }

    #[test]
    fn test_empty_braces() {
        assert_eq!(parse_config_ref("{{}}"), ConfigRef::Literal("{{}}".into()));
    }

    #[test]
    fn test_effective_name() {
        let r = parse_config_ref("{{agent_profile.coder}}");
        assert_eq!(r.effective_name(), "coder");

        let r = parse_config_ref("{{%coder%}}");
        assert_eq!(r.effective_name(), "coder");

        let r = parse_config_ref("{{#sys-id#}}");
        assert_eq!(r.effective_name(), "sys-id");
    }

    #[test]
    fn test_should_recreate() {
        assert!(parse_config_ref("{{coder}}").should_recreate());
        assert!(parse_config_ref("{{agent_profile.coder}}").should_recreate());
        assert!(!parse_config_ref("{{%coder%}}").should_recreate());
        assert!(!parse_config_ref("{{#sys-id#}}").should_recreate());
    }
}
