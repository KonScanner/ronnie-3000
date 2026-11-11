//! Host-neutral processor and shared plugin contracts for Ronnie.

/// Audio-facing view of the control session.
pub mod audio_session;
/// UI backend adapter.
pub mod backend;
/// Shared parameter catalog and action re-exports.
pub mod params;
mod persistence;
/// Control-side processor state and operations.
pub mod processor;

pub use audio_session::{AudioSessionState, SessionChain, SessionRestore, StreamFormat};
pub use backend::{ProcessorBackend, UiBackend};
pub use processor::{RonieProcessor, SessionChainPlan};
