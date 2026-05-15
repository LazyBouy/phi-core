use super::model::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Total budget for waiting on an exclusive advisory lock before giving up with
/// [`SessionError::Locked`]. 5 s is enough to ride out brief contention from a
/// peer writer while still failing fast on stuck locks.
const LOCK_RETRY_BUDGET: Duration = Duration::from_secs(5);
/// Interval between exclusive-lock retry attempts during the budget window.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// Save a session to `{dir}/{session_id}.json`, creating `dir` if necessary.
///
/// Returns the path the file was written to.
///
/// This is the legacy sync entry point retained for backward compatibility. New code
/// should prefer [`SessionStore`] + [`FileSystemSessionStore`] which adds advisory
/// file locking and atomic rename semantics so concurrent writers cannot corrupt the
/// session file.
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
    entries.sort_by_key(|e| std::cmp::Reverse(e.0)); // newest first
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

// ---------------------------------------------------------------------------
// SessionStore — async, concurrency-safe persistence
// ---------------------------------------------------------------------------

/// Async pluggable persistence backend for sessions.
///
/// Implementors are free to back this with the filesystem, an object store, a database,
/// etc. The default in-tree implementation is [`FileSystemSessionStore`] which guards
/// every write with an exclusive advisory lock + atomic rename so concurrent processes
/// cannot corrupt session files.
///
/// `list_for_agent` carries a default implementation in terms of the other methods,
/// so most implementors only need to provide the four required methods.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist a session, replacing any existing record with the same `session_id`.
    async fn save(&self, session: &Session) -> Result<(), SessionError>;

    /// Load a session by id. Returns `SessionError::NotFound` if absent.
    async fn load(&self, session_id: &str) -> Result<Session, SessionError>;

    /// List all session ids known to this store.
    async fn list_ids(&self) -> Result<Vec<String>, SessionError>;

    /// Delete a session by id. Returns `SessionError::NotFound` if absent.
    async fn delete(&self, session_id: &str) -> Result<(), SessionError>;

    /// Load every session belonging to `agent_id`. Default impl iterates `list_ids` +
    /// `load`; override for stores that can serve this from an index.
    async fn list_for_agent(&self, agent_id: &str) -> Result<Vec<Session>, SessionError> {
        let mut sessions = Vec::new();
        for id in self.list_ids().await? {
            match self.load(&id).await {
                Ok(s) if s.agent_id == agent_id => sessions.push(s),
                Ok(_) => {}
                Err(SessionError::NotFound { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(sessions)
    }
}

/// Filesystem-backed [`SessionStore`] with advisory locking and atomic writes.
///
/// Writes go through a tmp file in the same directory followed by `std::fs::rename`,
/// which is atomic on POSIX. An exclusive advisory lock on a `.lock` sidecar serialises
/// writers; readers are unsynchronised because the atomic rename guarantees a complete
/// file is always visible. On contention, `save()` retries for up to
/// `LOCK_RETRY_BUDGET` (5 s) before returning `SessionError::Locked`.
pub struct FileSystemSessionStore {
    dir: PathBuf,
}

impl FileSystemSessionStore {
    /// Construct a store rooted at `dir`. The directory is created lazily on first save.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Return the storage root directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Map a `tokio::task::JoinError` into a `SessionError::Task` while preserving the message.
fn map_join_err(e: tokio::task::JoinError) -> SessionError {
    SessionError::Task(e.to_string())
}

#[async_trait::async_trait]
impl SessionStore for FileSystemSessionStore {
    async fn save(&self, session: &Session) -> Result<(), SessionError> {
        let dir = self.dir.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || save_with_lock(&dir, &session))
            .await
            .map_err(map_join_err)?
    }

    async fn load(&self, session_id: &str) -> Result<Session, SessionError> {
        let dir = self.dir.clone();
        let id = session_id.to_string();
        tokio::task::spawn_blocking(move || load_session(&id, &dir))
            .await
            .map_err(map_join_err)?
    }

    async fn list_ids(&self) -> Result<Vec<String>, SessionError> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || list_session_ids(&dir))
            .await
            .map_err(map_join_err)?
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionError> {
        let dir = self.dir.clone();
        let id = session_id.to_string();
        tokio::task::spawn_blocking(move || delete_session(&id, &dir))
            .await
            .map_err(map_join_err)?
    }
}

/// Internal: perform a concurrency-safe save inside a blocking thread.
///
/// 1. Create the directory if missing.
/// 2. Open / create the `.lock` sidecar and acquire an exclusive advisory lock
///    (retrying within `LOCK_RETRY_BUDGET`).
/// 3. Write JSON to `{dir}/{id}.json.tmp.{pid}.{nonce}`, fsync.
/// 4. Atomically rename onto the final `{dir}/{id}.json`.
/// 5. Release the lock by dropping the file handle.
fn save_with_lock(dir: &Path, session: &Session) -> Result<(), SessionError> {
    use fs2::FileExt;
    use std::time::Instant;

    std::fs::create_dir_all(dir)?;
    let session_id = &session.session_id;
    let final_path = dir.join(format!("{}.json", session_id));
    let lock_path = dir.join(format!("{}.json.lock", session_id));

    // Open (or create) the lock sidecar. Truncation is irrelevant — we only use the
    // file descriptor as the lock target.
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    // Acquire exclusive lock with bounded retry. On Windows + POSIX, fs2 uses the
    // platform-native primitive (LockFileEx / fcntl) so this is cross-platform.
    let start = Instant::now();
    loop {
        match lock_file.try_lock_exclusive() {
            Ok(()) => break,
            Err(_) if start.elapsed() < LOCK_RETRY_BUDGET => {
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(_) => {
                return Err(SessionError::Locked {
                    session_id: session_id.clone(),
                });
            }
        }
    }

    // Atomic write: tmp → rename. The tmp name carries pid + nanos to avoid collision
    // between concurrent processes that might share a directory.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = dir.join(format!(
        "{}.json.tmp.{}.{}",
        session_id,
        std::process::id(),
        nonce
    ));

    {
        let tmp_file = std::fs::File::create(&tmp_path)?;
        serde_json::to_writer_pretty(&tmp_file, session)?;
        // fsync to make the bytes durable before the rename, so a crash here cannot
        // leave a half-written file behind a fresh rename.
        tmp_file.sync_all()?;
    }

    if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
        // Clean up the tmp file on rename failure; the lock will be released by drop.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(SessionError::Io(e));
    }

    // Releasing the lock happens when `lock_file` goes out of scope (drop closes the fd
    // which releases the OS-level advisory lock). The lock sidecar file is left behind
    // intentionally — recreating it on every save would defeat the lock semantics under
    // contention (a concurrent writer might recreate-and-lock between drop and unlink).
    drop(lock_file);
    Ok(())
}
