//! Tests for the async `SessionStore` trait + `FileSystemSessionStore` impl (MEDIUM-6).

use phi_core::session::{
    save_session, FileSystemSessionStore, Session, SessionError, SessionFormation, SessionScope,
    SessionStore,
};
use std::sync::Arc;
use tempfile::TempDir;

fn make_session(id: &str, agent: &str) -> Session {
    Session {
        session_id: id.into(),
        agent_id: agent.into(),
        created_at: chrono::Utc::now(),
        last_active_at: chrono::Utc::now(),
        formation: SessionFormation::Explicit {
            timestamp: chrono::Utc::now(),
        },
        parent_spawn_ref: None,
        scope: SessionScope::Ephemeral,
        loops: Vec::new(),
    }
}

#[tokio::test]
async fn round_trip_save_load_delete_list() {
    let dir = TempDir::new().unwrap();
    let store = FileSystemSessionStore::new(dir.path());

    let s = make_session("round-trip-1", "agent-x");
    store.save(&s).await.expect("save");

    let loaded = store.load("round-trip-1").await.expect("load");
    assert_eq!(loaded.session_id, "round-trip-1");
    assert_eq!(loaded.agent_id, "agent-x");

    let ids = store.list_ids().await.expect("list_ids");
    assert!(ids.contains(&"round-trip-1".to_string()));

    store.delete("round-trip-1").await.expect("delete");
    match store.load("round-trip-1").await {
        Err(SessionError::NotFound { .. }) => {}
        other => panic!("expected NotFound after delete, got {:?}", other),
    }
}

#[tokio::test]
async fn list_for_agent_filters_correctly() {
    let dir = TempDir::new().unwrap();
    let store = FileSystemSessionStore::new(dir.path());

    store.save(&make_session("a1", "agent-a")).await.unwrap();
    store.save(&make_session("a2", "agent-a")).await.unwrap();
    store.save(&make_session("b1", "agent-b")).await.unwrap();

    let a = store.list_for_agent("agent-a").await.unwrap();
    assert_eq!(a.len(), 2);
    assert!(a.iter().all(|s| s.agent_id == "agent-a"));
    let b = store.list_for_agent("agent-b").await.unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].session_id, "b1");
}

#[tokio::test]
async fn concurrent_saves_serialize_via_advisory_lock() {
    // N concurrent saves of distinct payloads against the same session_id.
    // Advisory lock serialises them — each completes Ok; the final on-disk JSON is
    // exactly one of the writers' payloads (atomic rename guarantees no torn writes).
    let dir = TempDir::new().unwrap();
    let store = Arc::new(FileSystemSessionStore::new(dir.path()));
    const N: u32 = 8;

    let mut handles = Vec::new();
    for i in 0..N {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            let mut s = make_session("contended", "agent-c");
            // Distinguish payloads by stuffing the iteration index into agent_id.
            s.agent_id = format!("agent-c-{i}");
            store.save(&s).await
        }));
    }
    for h in handles {
        let res = h.await.unwrap();
        assert!(
            res.is_ok(),
            "save under contention should succeed: {:?}",
            res
        );
    }

    // The final file must be a valid Session — never half-written JSON.
    let loaded = store.load("contended").await.expect("final load");
    assert_eq!(loaded.session_id, "contended");
    assert!(loaded.agent_id.starts_with("agent-c-"));
}

#[tokio::test]
async fn atomic_rename_visible_after_save() {
    // Single save must surface as a parseable Session — i.e. the rename completes
    // before save() returns, so load() never sees a half-written file.
    let dir = TempDir::new().unwrap();
    let store = FileSystemSessionStore::new(dir.path());

    let s = make_session("atomic", "agent-a");
    store.save(&s).await.unwrap();

    // No tmp files should remain after a clean save.
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains(".tmp."))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        entries.is_empty(),
        "no .tmp files should linger after save, found: {:?}",
        entries.iter().map(|e| e.path()).collect::<Vec<_>>()
    );

    // The data file should be loadable via the legacy free function too — backward compat.
    let loaded = phi_core::session::load_session("atomic", dir.path()).unwrap();
    assert_eq!(loaded.session_id, "atomic");
}

#[tokio::test]
async fn legacy_save_session_remains_compatible() {
    // The legacy sync free function still works and writes a file readable by the new
    // async store — no regression on existing callers.
    let dir = TempDir::new().unwrap();
    let s = make_session("legacy", "agent-l");
    save_session(&s, dir.path()).unwrap();

    let store = FileSystemSessionStore::new(dir.path());
    let loaded = store.load("legacy").await.unwrap();
    assert_eq!(loaded.session_id, "legacy");
}
