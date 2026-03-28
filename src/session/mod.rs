//! Persistent session layer for phi-core agents.
//!
//! Sub-modules:
//! - [`model`] — Session, LoopRecord, and all session data types
//! - [`recorder`] — SessionRecorder event-driven state machine
//! - [`storage`] — File I/O (save, load, list, delete)
//! - [`helpers`] — Internal utilities

mod helpers;
pub mod model;
pub mod recorder;
pub mod storage;

// Re-export all public items for backward compatibility
pub use model::*;
pub use recorder::{SessionRecorder, SessionRecorderConfig};
pub use storage::*;
