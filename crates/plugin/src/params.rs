//! Re-exported parameter catalog and shared UI action types.

pub use ronie_core::params::{ParameterCatalog, ParameterId, ParameterSpec, ParameterValue};
pub use ronie_ui::backend::{GesturePhase, UiAction, UiEffect, UiError, UiReceipt};

/// Returns the single versioned catalog used by processor and views.
#[must_use]
pub const fn catalog() -> &'static [ParameterSpec] {
    ParameterCatalog::v1()
}
