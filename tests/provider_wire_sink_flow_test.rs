//! End-to-end plumbing test for the 0.11.1 consumer-facing
//! `provider_wire_sink` path:
//!
//!   `BasicAgent::with_provider_wire_sink(sink)`
//!     → `BasicAgent.provider_wire_sink` field
//!     → `BasicAgent::build_loop_config` propagation
//!     → `AgentLoopConfig.provider_wire_sink`
//!     → `streaming.rs` per-attempt `StreamConfig.provider_wire_sink`
//!     → the provider invokes `ProviderWireSink::on_wire`.
//!
//! CC-09a wired `provider_wire_sink` onto `StreamConfig` but `streaming.rs`
//! hardcoded `None`, so no consumer could reach it. CC-09b (F0) threads it from
//! `AgentLoopConfig` + a `BasicAgent` setter. This test load-bears that a sink
//! installed via the single setter actually fires through a real `agent_loop`
//! run, proving the full consumer plumbing.

use phi_core::provider::{MockProvider, ModelConfig, ProviderWireSink, RawWire};
use phi_core::BasicAgent;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A minimal counting sink: records every `on_wire` call plus the count of
/// `RawWire::Request` frames specifically (proving request bytes were captured).
#[derive(Default)]
struct CountingSink {
    total: AtomicUsize,
    requests: AtomicUsize,
}

impl ProviderWireSink for CountingSink {
    fn on_wire(&self, ev: &RawWire) {
        self.total.fetch_add(1, Ordering::SeqCst);
        if matches!(ev, RawWire::Request { .. }) {
            self.requests.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[tokio::test]
async fn test_provider_wire_sink_flows_from_basic_agent() {
    let sink = Arc::new(CountingSink::default());

    let provider = MockProvider::text("Hello from the model!");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_provider_wire_sink(sink.clone())
        .with_system_prompt("You are helpful.");

    // Drive a real agent_loop run via the canonical prompt path.
    let mut rx = agent.prompt("Hi there").await;
    while rx.try_recv().is_ok() {}

    // The sink must have fired end-to-end: at least one Request frame was
    // captured (proving the consumer-installed sink reached StreamConfig and
    // the provider invoked it), and total wire events are non-zero.
    assert!(
        sink.requests.load(Ordering::SeqCst) >= 1,
        "expected >=1 RawWire::Request captured via BasicAgent::with_provider_wire_sink, got {}",
        sink.requests.load(Ordering::SeqCst)
    );
    assert!(
        sink.total.load(Ordering::SeqCst) >= 1,
        "expected >=1 total RawWire event captured, got {}",
        sink.total.load(Ordering::SeqCst)
    );
}
