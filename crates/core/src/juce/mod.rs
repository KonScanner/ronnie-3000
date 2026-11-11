//! Bounded JUCE variant and ValueTree binary compatibility.

/// Bounded little-endian stream reader and writer.
pub mod binary;
/// Ordered `ValueTree` data.
pub mod tree;
/// Variant values supported by the stream grammar.
pub mod value;

pub use binary::JuceCodec;
pub use tree::JuceTree;
pub use value::JuceValue;
