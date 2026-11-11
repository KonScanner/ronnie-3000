use super::JuceValue;

/// Ordered JUCE `ValueTree` with no loss of unknown properties or subtrees.
#[derive(Debug, Clone, PartialEq)]
pub struct JuceTree {
    /// Tree type identifier.
    pub identifier: String,
    /// Properties in their original insertion order.
    pub properties: Vec<(String, JuceValue)>,
    /// Children in their original order.
    pub children: Vec<JuceTree>,
}
