use super::model::*;
use std::path::{Path, PathBuf};

/// Save a session to `{dir}/{session_id}.json`, creating `dir` if necessary.
///
/// Returns the path the file was written to.
pub fn save_session(session: &Session, dir: &Path) -> Result<PathBuf, SessionError> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", session.session_id));
    let file = std::fs::File::create(&path)?;
    serde_json::to_writer_pretty(file, session)?;
    Ok(path)
}

/// Load a session from `{dir}/{session_id}.json`.
pub fn load_session(session_id: &str, dir: &Path) -> Result<Session, SessionError> {
    let path = dir.join(format!("{}.json", session_id));
    if !path.exists() {
        return Err(SessionError::NotFound {
            session_id: session_id.to_string(),
        });
    }
    let file = std::fs::File::open(path)?;
    let session: Session = serde_json::from_reader(file)?;
    Ok(session)
}

/// List all session IDs in `dir`, sorted by file modification time (newest first).
pub fn list_session_ids(dir: &Path) -> Result<Vec<String>, SessionError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x == "json")
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let stem = e
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())?;
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, stem))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    Ok(entries.into_iter().map(|(_, id)| id).collect())
}

/// Load all sessions in `dir` that belong to `agent_id`.
pub fn load_sessions_for_agent(agent_id: &str, dir: &Path) -> Result<Vec<Session>, SessionError> {
    let ids = list_session_ids(dir)?;
    let mut sessions = Vec::new();
    for id in ids {
        match load_session(&id, dir) {
            Ok(s) if s.agent_id == agent_id => sessions.push(s),
            Ok(_) => {}
            Err(SessionError::NotFound { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(sessions)
}

/// Delete `{dir}/{session_id}.json`.
pub fn delete_session(session_id: &str, dir: &Path) -> Result<(), SessionError> {
    let path = dir.join(format!("{}.json", session_id));
    if !path.exists() {
        return Err(SessionError::NotFound {
            session_id: session_id.to_string(),
        });
    }
    std::fs::remove_file(path)?;
    Ok(())
}
