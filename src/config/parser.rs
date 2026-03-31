//! Multi-format config parser with environment variable substitution.

use super::builder::ConfigError;
use super::schema::AgentConfig;
use std::path::Path;

/// Supported config file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Json,
    Yaml,
}

/// Parse a config string in the specified format.
pub fn parse_config(input: &str, format: ConfigFormat) -> Result<AgentConfig, ConfigError> {
    let substituted = substitute_env_vars(input)?;
    match format {
        ConfigFormat::Toml => {
            toml::from_str(&substituted).map_err(|e| ConfigError::Parse(e.to_string()))
        }
        ConfigFormat::Json => {
            serde_json::from_str(&substituted).map_err(|e| ConfigError::Parse(e.to_string()))
        }
        ConfigFormat::Yaml => {
            serde_yaml::from_str(&substituted).map_err(|e| ConfigError::Parse(e.to_string()))
        }
    }
}

/// Parse a config string, auto-detecting the format.
///
/// Tries TOML first, then JSON, then YAML. Returns the first successful parse.
pub fn parse_config_auto(input: &str) -> Result<AgentConfig, ConfigError> {
    // Try TOML first (most likely for phi-core configs)
    if let Ok(config) = parse_config(input, ConfigFormat::Toml) {
        return Ok(config);
    }
    // Try JSON
    if let Ok(config) = parse_config(input, ConfigFormat::Json) {
        return Ok(config);
    }
    // Try YAML
    parse_config(input, ConfigFormat::Yaml)
}

/// Parse a config file, detecting format from the file extension.
///
/// Supported extensions: `.toml`, `.json`, `.yaml`, `.yml`
pub fn parse_config_file(path: &Path) -> Result<AgentConfig, ConfigError> {
    let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
    let format = match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => ConfigFormat::Toml,
        Some("json") => ConfigFormat::Json,
        Some("yaml" | "yml") => ConfigFormat::Yaml,
        Some(ext) => {
            return Err(ConfigError::Parse(format!(
                "Unsupported config file extension: .{ext}"
            )))
        }
        None => {
            return Err(ConfigError::Parse(
                "Config file has no extension; use .toml, .json, or .yaml".to_string(),
            ))
        }
    };
    parse_config(&content, format)
}

/// Substitute `${VAR}` patterns with environment variable values.
///
/// Returns `ConfigError::MissingEnvVar` if a referenced variable is not set.
fn substitute_env_vars(input: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            let mut found_close = false;
            for ch in chars.by_ref() {
                if ch == '}' {
                    found_close = true;
                    break;
                }
                var_name.push(ch);
            }
            if !found_close {
                // Malformed ${...} — pass through literally
                result.push('$');
                result.push('{');
                result.push_str(&var_name);
            } else if var_name.is_empty() {
                // ${} — pass through literally
                result.push_str("${}");
            } else {
                let value = std::env::var(&var_name).map_err(|_| ConfigError::MissingEnvVar {
                    var: var_name.clone(),
                })?;
                result.push_str(&value);
            }
        } else {
            result.push(c);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_var_substitution() {
        std::env::set_var("PHI_TEST_KEY", "test-value-123");
        let input = "api_key = \"${PHI_TEST_KEY}\"";
        let result = substitute_env_vars(input).unwrap();
        assert_eq!(result, "api_key = \"test-value-123\"");
        std::env::remove_var("PHI_TEST_KEY");
    }

    #[test]
    fn test_missing_env_var() {
        let input = "key = \"${DEFINITELY_NOT_SET_PHI_TEST}\"";
        let result = substitute_env_vars(input);
        assert!(matches!(result, Err(ConfigError::MissingEnvVar { .. })));
    }

    #[test]
    fn test_no_substitution_needed() {
        let input = "key = \"plain value\"";
        let result = substitute_env_vars(input).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_malformed_env_var() {
        let input = "key = \"${UNCLOSED";
        let result = substitute_env_vars(input).unwrap();
        assert_eq!(result, "key = \"${UNCLOSED");
    }
}
