//! Save and restore agent conversation state.
//!
//! Demonstrates:
//! - Running a conversation with MockProvider
//! - Saving messages to JSON
//! - Restoring into a fresh agent
//! - Continuing the conversation from saved state

use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::types::*;
use phi_core::{Agent, BasicAgent};

#[tokio::main]
async fn main() {
    // --- Phase 1: Initial conversation ---
    let provider = MockProvider::text("The capital of France is Paris.");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(std::sync::Arc::new(provider))
        .with_system_prompt("You are a helpful assistant.");

    println!("=== Phase 1: Initial conversation ===");
    let mut rx = agent.prompt("What is the capital of France?").await;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::MessageUpdate {
            delta: StreamDelta::Text { delta },
            ..
        } = event
        {
            print!("{}", delta);
        }
    }
    println!("\n");

    // Save state
    let json = agent.save_messages().expect("Failed to save");
    println!(
        "Saved {} messages ({} bytes)\n",
        agent.messages().len(),
        json.len()
    );

    // --- Phase 2: Restore and continue ---
    let provider2 = MockProvider::text("Paris is also known as the City of Light.");
    let mut agent2 = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(std::sync::Arc::new(provider2))
        .with_system_prompt("You are a helpful assistant.");

    agent2.restore_messages(&json).expect("Failed to restore");
    println!(
        "=== Phase 2: Restored {} messages, continuing... ===",
        agent2.messages().len()
    );

    let mut rx = agent2.prompt("Tell me more about it.").await;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::MessageUpdate {
            delta: StreamDelta::Text { delta },
            ..
        } = event
        {
            print!("{}", delta);
        }
    }
    println!("\n");

    println!("Final message count: {}", agent2.messages().len());
    println!(
        "Messages match original + new: {}",
        agent2.messages().len() == 4
    );
}
