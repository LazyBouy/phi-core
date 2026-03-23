# Quick Start

## Basic Example with Anthropic

```rust
use phi_core::{BasicAgent, AgentEvent, StreamDelta};
use phi_core::provider::ModelConfig;
use phi_core::tools::default_tools;

#[tokio::main]
async fn main() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
    let mut agent = BasicAgent::new(ModelConfig::anthropic(
        "claude-sonnet-4-20250514",
        "Claude Sonnet 4",
        &api_key,
    ))
    .with_system_prompt("You are a helpful coding assistant.")
    .with_tools(default_tools());

    let mut rx = agent.prompt("List the files in the current directory").await;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::MessageUpdate { delta, .. } => match delta {
                StreamDelta::Text { delta } => print!("{}", delta),
                StreamDelta::Thinking { delta } => print!("[thinking] {}", delta),
                _ => {}
            },
            AgentEvent::ToolExecutionStart { tool_name, .. } => {
                println!("\n→ Running tool: {}", tool_name);
            }
            AgentEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
                if is_error {
                    println!("  ✗ {} failed", tool_name);
                } else {
                    println!("  ✓ {} done", tool_name);
                }
            }
            AgentEvent::AgentEnd { .. } => {
                println!("\n\nDone.");
            }
            _ => {}
        }
    }
}
```

## Example with OpenAI-Compatible Provider

For OpenAI, xAI, Groq, or any compatible API, use `ModelConfig::openai()` or `ModelConfig::local()`:

```rust
use phi_core::{BasicAgent, AgentEvent, StreamDelta};
use phi_core::provider::ModelConfig;
use phi_core::tools::default_tools;

#[tokio::main]
async fn main() {
    let api_key = std::env::var("OPENAI_API_KEY").unwrap();
    let mut agent = BasicAgent::new(ModelConfig::openai("gpt-4o", "GPT-4o", &api_key))
        .with_system_prompt("You are a helpful assistant.")
        .with_tools(default_tools());

    let mut rx = agent.prompt("What is 2 + 2?").await;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::MessageUpdate { delta, .. } => {
                if let StreamDelta::Text { delta } = delta {
                    print!("{}", delta);
                }
            }
            AgentEvent::AgentEnd { .. } => println!(),
            _ => {}
        }
    }
}
```

## Real-Time Streaming

By default, `agent.prompt()` blocks until the loop finishes and returns a receiver with all events buffered. To consume events in real-time, use `prompt_with_sender()` with a caller-provided channel:

```rust
use phi_core::{BasicAgent, AgentEvent, StreamDelta};
use phi_core::provider::ModelConfig;
use phi_core::tools::default_tools;

#[tokio::main]
async fn main() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
    let mut agent = BasicAgent::new(ModelConfig::anthropic(
        "claude-sonnet-4-20250514",
        "Claude Sonnet 4",
        &api_key,
    ))
    .with_system_prompt("You are a helpful assistant.")
    .with_tools(default_tools());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Consume events in real-time on a separate task
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::MessageUpdate { delta, .. } => {
                    if let StreamDelta::Text { delta } = delta {
                        print!("{}", delta);
                    }
                }
                AgentEvent::AgentEnd { .. } => println!(),
                _ => {}
            }
        }
    });

    // This blocks until the loop finishes; state is restored automatically
    agent.prompt_with_sender("What is 2 + 2?", tx).await;

    // Agent is ready for another prompt immediately
    let _rx = agent.prompt("Follow up question").await;
}
```

## Using the Low-Level API

For more control, use `agent_loop()` directly:

```rust
use phi_core::agent_loop::{agent_loop, AgentLoopConfig};
use phi_core::provider::ModelConfig;
use phi_core::types::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();

    let mut context = AgentContext {
        system_prompt: "You are helpful.".into(),
        messages: Vec::new(),
        tools: phi_core::tools::default_tools(),
        ..Default::default()
    };

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic(
            "claude-sonnet-4-20250514",
            "Claude Sonnet 4",
            &api_key,
        ),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: None,
        transform_context: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        context_config: None,
        compaction_strategy: None,
        execution_limits: None,
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        retry_config: phi_core::RetryConfig::default(),
        before_turn: None,
        after_turn: None,
        on_error: None,
        input_filters: vec![],
        ..Default::default()
    };

    let prompts = vec![AgentMessage::Llm(Message::user("Hello!"))];
    let new_messages = agent_loop(prompts, &mut context, &config, tx, cancel).await;

    // Drain events
    while let Ok(_event) = rx.try_recv() {
        // handle events...
    }

    println!("Got {} new messages", new_messages.len());
}
```
