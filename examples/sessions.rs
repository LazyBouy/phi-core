//! Session recording and persistence example.
//!
//! Demonstrates:
//! - Wiring [`SessionRecorder`] to a `BasicAgent` event channel
//! - Running two loops (a prompt + continuation) under the same session
//! - Inspecting the session tree (root loops, children, total usage)
//! - Saving a session to disk and reloading it to verify round-trip fidelity
//!
//! Uses `MockProvider` — no API key required.
//!
//! Run with: cargo run --example sessions

use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::session::{
    list_session_ids, load_session, save_session, SessionRecorder, SessionRecorderConfig,
};
use phi_core::types::*;
use phi_core::{Agent, BasicAgent};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn drain(mut rx: mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

#[tokio::main]
async fn main() {
    // ------------------------------------------------------------------
    // Set up agent with two canned responses (prompt + continuation)
    // ------------------------------------------------------------------
    let provider = Arc::new(MockProvider::texts(vec![
        "Rust's ownership system guarantees memory safety without a garbage collector.",
        "The borrow checker enforces these rules at compile time.",
    ]));

    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(provider)
        .with_system_prompt("You are a helpful Rust expert.");

    println!("=== Session ID: {} ===\n", agent.session_id());

    // ------------------------------------------------------------------
    // Loop 1: initial prompt
    // ------------------------------------------------------------------
    let (tx1, rx1) = mpsc::unbounded_channel();
    agent
        .prompt_with_sender("Tell me about Rust's memory model.", tx1)
        .await;
    let events1 = drain(rx1);

    println!("Loop 1 collected {} events", events1.len());

    // ------------------------------------------------------------------
    // Loop 2: continuation (same session, parent_loop_id set automatically)
    // ------------------------------------------------------------------
    let (tx2, rx2) = mpsc::unbounded_channel();
    agent
        .prompt_with_sender("How does the borrow checker help?", tx2)
        .await;
    let events2 = drain(rx2);

    println!("Loop 2 collected {} events\n", events2.len());

    // ------------------------------------------------------------------
    // Feed all events into the SessionRecorder
    // ------------------------------------------------------------------
    let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
    for event in events1.into_iter().chain(events2) {
        recorder.on_event(event);
    }
    recorder.flush();

    // ------------------------------------------------------------------
    // Inspect the session tree
    // ------------------------------------------------------------------
    let mut sessions = recorder.drain_completed();
    assert_eq!(sessions.len(), 1, "expected exactly one session");
    let session = sessions.remove(0);

    println!("=== Session Tree ===");
    println!("  session_id : {}", session.session_id);
    println!("  agent_id   : {}", session.agent_id);
    println!("  loops      : {}", session.loops.len());

    let roots: Vec<_> = session.root_loops().collect();
    println!("\nRoot loops: {}", roots.len());
    for root in &roots {
        println!(
            "  [root] {} — {:?}",
            &root.loop_id[root.loop_id.len().saturating_sub(12)..],
            root.status
        );
        for child in session.children_of(&root.loop_id) {
            println!(
                "    [child] {} — {:?}",
                &child.loop_id[child.loop_id.len().saturating_sub(12)..],
                child.status
            );
        }
    }

    let usage = session.total_usage();
    // MockProvider returns zero usage — a real provider would show non-zero token counts.
    println!(
        "\nTotal usage — input: {}, output: {}",
        usage.input, usage.output
    );

    // Config snapshots show which model ran each loop
    for lr in &session.loops {
        if let Some(ref cfg) = lr.config {
            println!(
                "  loop config — model: {}, config_id: {:?}",
                cfg.model, cfg.config_id
            );
        }
    }

    // ------------------------------------------------------------------
    // Persist and reload
    // ------------------------------------------------------------------
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = save_session(&session, dir.path()).expect("failed to save session");
    println!("\nSaved to: {:?}", path);

    let ids = list_session_ids(dir.path()).expect("failed to list sessions");
    println!("Sessions on disk: {:?}", ids);

    let loaded = load_session(&session.session_id, dir.path()).expect("failed to load session");
    assert_eq!(loaded.session_id, session.session_id);
    assert_eq!(loaded.loops.len(), session.loops.len());
    assert_eq!(loaded.loops[0].status, session.loops[0].status);

    println!("\nRound-trip verified. Session reloaded successfully.");
}
