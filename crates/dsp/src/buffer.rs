use crate::ChainError;

/// A validated channel-major mutable audio view.
///
/// Construction proves the complete layout before any sample can be changed.
pub struct PlanarBuffer<'a> {
    data: &'a mut [f32],
    channels: usize,
    frames: usize,
}

impl<'a> PlanarBuffer<'a> {
    /// Creates a planar view over exactly `channels * frames` samples.
    ///
    /// # Errors
    ///
    /// Returns [`ChainError::InvalidLayout`] for zero dimensions, arithmetic
    /// overflow, or a data length that does not exactly match the dimensions.
    pub fn new(data: &'a mut [f32], channels: usize, frames: usize) -> Result<Self, ChainError> {
        if channels == 0 || frames == 0 {
            return Err(ChainError::InvalidLayout(
                "channels and frames must be nonzero",
            ));
        }
        let expected = channels
            .checked_mul(frames)
            .ok_or(ChainError::InvalidLayout(
                "channel and frame dimensions overflow",
            ))?;
        if data.len() != expected {
            return Err(ChainError::InvalidLayout(
                "audio length does not match the planar dimensions",
            ));
        }
        Ok(Self {
            data,
            channels,
            frames,
        })
    }

    /// Number of planar channels.
    #[must_use]
    pub const fn channels(&self) -> usize {
        self.channels
    }

    /// Number of frames in each channel.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames
    }

    /// Returns the complete channel-major sample view.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        self.data
    }

    /// Returns the complete mutable channel-major sample view.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        self.data
    }

    /// Returns one read-only channel.
    ///
    /// # Errors
    ///
    /// Returns [`ChainError::InvalidLayout`] when `index` is out of range.
    pub fn channel(&self, index: usize) -> Result<&[f32], ChainError> {
        if index >= self.channels {
            return Err(ChainError::InvalidLayout("channel index is out of range"));
        }
        let start = index * self.frames;
        Ok(&self.data[start..start + self.frames])
    }

    /// Returns one mutable channel.
    ///
    /// # Errors
    ///
    /// Returns [`ChainError::InvalidLayout`] when `index` is out of range.
    pub fn channel_mut(&mut self, index: usize) -> Result<&mut [f32], ChainError> {
        if index >= self.channels {
            return Err(ChainError::InvalidLayout("channel index is out of range"));
        }
        let start = index * self.frames;
        Ok(&mut self.data[start..start + self.frames])
    }
}
