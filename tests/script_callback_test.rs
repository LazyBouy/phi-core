//! Tests for ScriptCallback and is_script_path.

use phi_core::agent_loop::script_callback::{is_script_path, ScriptCallback, ScriptCallbackError};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tempfile::TempDir;

fn write_executable_script(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).expect("write script");
    let perms = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

// ---------------------------------------------------------------------------
// 1. test_script_callback_execute
// ---------------------------------------------------------------------------

#[test]
fn test_script_callback_execute() {
    let dir = TempDir::new().unwrap();
    let script = write_executable_script(&dir, "test.sh", "#!/bin/sh\necho '{\"allow\": true}'");

    let cb = ScriptCallback::new(&script, None);
    let result = cb.execute_sync(&json!({"hook": "test"})).unwrap();
    assert_eq!(result["allow"], json!(true));
}

// ---------------------------------------------------------------------------
// 2. test_script_callback_working_dir
// ---------------------------------------------------------------------------

#[test]
fn test_script_callback_working_dir() {
    let dir = TempDir::new().unwrap();
    let script = write_executable_script(
        &dir,
        "cwd.sh",
        "#!/bin/sh\nprintf '{\"cwd\": \"%s\"}' \"$(pwd)\"",
    );

    let cb = ScriptCallback::new(&script, Some(dir.path().to_path_buf()));
    let result = cb.execute_sync(&json!({})).unwrap();
    // Canonicalize both paths to handle symlinks (e.g. /tmp -> /private/tmp on macOS)
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    let actual_str = result["cwd"].as_str().unwrap();
    let actual = std::fs::canonicalize(actual_str).unwrap();
    assert_eq!(actual, expected);
}

// ---------------------------------------------------------------------------
// 3. test_script_callback_receives_json_stdin
// ---------------------------------------------------------------------------

#[test]
fn test_script_callback_receives_json_stdin() {
    let dir = TempDir::new().unwrap();
    let script = write_executable_script(&dir, "echo.sh", "#!/bin/sh\ncat");

    let cb = ScriptCallback::new(&script, None);
    let input = json!({"hello": "world"});
    let result = cb.execute_sync(&input).unwrap();
    assert_eq!(result, input);
}

// ---------------------------------------------------------------------------
// 4. test_script_callback_failure_returns_default
// ---------------------------------------------------------------------------

#[test]
fn test_script_callback_failure_returns_default() {
    let dir = TempDir::new().unwrap();
    let script = write_executable_script(&dir, "fail.sh", "#!/bin/sh\nexit 1");

    let cb = ScriptCallback::new(&script, None);
    let result = cb.execute_sync(&json!({}));
    assert!(result.is_err());
    match result.unwrap_err() {
        ScriptCallbackError::NonZeroExit { code, .. } => {
            assert_eq!(code, Some(1));
        }
        other => panic!("expected NonZeroExit, got: {other}"),
    }
}

// ---------------------------------------------------------------------------
// 5. test_is_script_path_detection
// ---------------------------------------------------------------------------

#[test]
fn test_is_script_path_detection() {
    // Contains `/` → true
    assert!(is_script_path("scripts/before_loop.sh"));
    // Ends with `.sh` → true
    assert!(is_script_path("before_loop.sh"));
    // Contains `/` and `.py` → true
    assert!(is_script_path("scripts/hook.py"));
    // No `/`, no `.sh`/`.py` → false (WASM-style reference)
    assert!(!is_script_path("my_plugin::before_loop"));
    // Plain name → false
    assert!(!is_script_path("plain_name"));
}

// ---------------------------------------------------------------------------
// 6. test_config_ignores_wasm_reference
// ---------------------------------------------------------------------------

#[test]
fn test_config_ignores_wasm_reference() {
    assert!(!is_script_path("my_plugin::before_turn"));
}
