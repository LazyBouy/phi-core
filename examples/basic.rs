//! Basic example: simple text prompt with Anthropic.
//!
//! Run with: ANTHROPIC_API_KEY=sk-... cargo run --example basic

use phi_core::provider::ModelConfig;
use phi_core::BasicAgent;
use phi_core::*;

#[tokio::main]
async fn main() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("Set ANTHROPIC_API_KEY");

    let mut agent = BasicAgent::new(ModelConfig::anthropic(
        "claude-sonnet-4-20250514",
        "Claude Sonnet 4",
        &api_key,
    ))
    .with_system_prompt("You are a helpful assistant. Be concise.");

    println!("Sending prompt...");

    let mut rx = agent
        .prompt("What is Rust's ownership model in 2 sentences?")
        .await;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::MessageUpdate {
                delta: StreamDelta::Text { delta },
                ..
            } => {
                print!("{}", delta);
            }
            AgentEvent::AgentEnd { .. } => {
                println!("\n\n--- Done ---");
            }
            _ => {}
        }
    }
}
