/// Scalar, array, and binary variants emitted by JUCE `ValueTree` persistence.
#[derive(Debug, Clone, PartialEq)]
pub enum JuceValue {
    /// Void encoded as a zero compressed length.
    Void,
    /// Explicit undefined marker 9.
    Undefined,
    /// Little-endian signed 32-bit integer, marker 1.
    Int(i32),
    /// Little-endian signed 64-bit integer, marker 6.
    Int64(i64),
    /// Boolean markers 2 and 3.
    Bool(bool),
    /// Little-endian IEEE-754 double, marker 4.
    Double(f64),
    /// NUL-terminated UTF-8 string, marker 5.
    String(String),
    /// Ordered variant array, marker 7.
    Array(Vec<JuceValue>),
    /// Raw bytes, marker 8.
    Binary(Vec<u8>),
}
