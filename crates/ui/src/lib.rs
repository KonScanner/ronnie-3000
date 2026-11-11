//! Portable native UI contracts and deterministic scenario verification for Ronnie.

/// Accessible metadata, focus, and announcements.
pub mod accessibility;
/// Host-neutral backend actions, receipts, and mock services.
pub mod backend;
/// Shared UI scheduler and hidden-window low-power policy.
pub mod clock;
/// Reference Default chrome and workflows.
pub mod default;
/// Real-input driver for the Default UI in a headless context.
pub mod driver;
/// Embedded font assets and CSS-compatible metrics.
pub mod fonts;
/// Neutral cached icon assets.
pub mod icons;
/// Drives a running editor to each named state of the complete Default scenario suite.
pub mod native_scenario;
/// Common one-axis scrolling.
pub mod scroller;
/// Host-neutral catalog, artwork, and load services.
pub mod services;
/// The session edit rules that each backend shares.
pub mod session_edit;
/// Native design-space layout and shared services.
pub mod shell;
/// Neutral scenario schema and backend-effect testbed.
pub mod testbed;
/// Shared dark theme tokens.
pub mod theme;
/// Deterministic visual comparison and worst-tile reporting.
pub mod visual;
/// Reusable parameter and meter widgets.
pub mod widgets;

pub use backend::{UiBackend, UiError};
