//! Bounded codec for the binary `ValueTree` file format.

/// Bounded little-endian stream reader and writer.
pub mod binary;
/// Ordered `ValueTree` data.
pub mod tree;
/// Variant values supported by the stream grammar.
pub mod value;

pub use binary::StateTreeCodec;
pub use tree::StateTree;
pub use value::StateValue;
