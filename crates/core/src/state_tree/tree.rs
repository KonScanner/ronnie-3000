use super::StateValue;

/// Ordered `ValueTree` with no loss of unknown properties or subtrees.
#[derive(Debug, Clone, PartialEq)]
pub struct StateTree {
    /// Tree type identifier.
    pub identifier: String,
    /// Properties in their original insertion order.
    pub properties: Vec<(String, StateValue)>,
    /// Children in their original order.
    pub children: Vec<StateTree>,
}
