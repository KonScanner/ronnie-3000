use ronie_core::{DspError, DspResult, NamArchitecture, NamDocument};

use super::{CompiledA2, CompiledA2Layer, HEAD_KERNEL_SIZE, KERNEL_SIZES, NUM_LAYERS, detect_a2};

/// Compiles a validated A2 document into immutable column-major-per-tap weights.
///
/// # Errors
///
/// Returns an error for non-A2 documents and short or trailing weight streams.
pub fn compile_a2(document: &NamDocument) -> DspResult<CompiledA2> {
    if document.architecture != NamArchitecture::WaveNet {
        return Err(DspError::InvalidA2Shape);
    }
    let channels = detect_a2(&document.config).ok_or(DspError::InvalidA2Shape)?;
    let expected = channels.expected_weight_count();
    if document.weights.len() != expected {
        return Err(DspError::WeightCount {
            expected,
            actual: document.weights.len(),
        });
    }

    let mut reader = WeightReader::new(&document.weights);
    let count = channels.count();
    let mut rechannel_weights = Vec::with_capacity(count);
    for _ in 0..count {
        rechannel_weights.push(reader.take()?);
    }

    let mut layers = Vec::with_capacity(NUM_LAYERS);
    for (kernel_size, dilation) in KERNEL_SIZES.into_iter().zip(super::DILATIONS) {
        let mut conv_weights = vec![0.0; kernel_size * count * count];
        for output in 0..count {
            for input in 0..count {
                for tap in 0..kernel_size {
                    let index = tap * count * count + input * count + output;
                    conv_weights[index] = reader.take()?;
                }
            }
        }
        let conv_bias = reader.take_many(count)?;
        let mixin_weights = reader.take_many(count)?;
        let mut layer1x1_weights = vec![0.0; count * count];
        for output in 0..count {
            for input in 0..count {
                layer1x1_weights[input * count + output] = reader.take()?;
            }
        }
        let layer1x1_bias = reader.take_many(count)?;
        layers.push(CompiledA2Layer {
            kernel_size,
            dilation,
            conv_weights: conv_weights.into_boxed_slice(),
            conv_bias,
            mixin_weights,
            layer1x1_weights: layer1x1_weights.into_boxed_slice(),
            layer1x1_bias,
        });
    }

    let mut head_weights = vec![0.0; HEAD_KERNEL_SIZE * count];
    for input in 0..count {
        for tap in 0..HEAD_KERNEL_SIZE {
            head_weights[tap * count + input] = reader.take()?;
        }
    }
    let head_bias = reader.take()?;
    let head_scale = reader.take()?;
    reader.finish()?;

    Ok(CompiledA2 {
        channels,
        rechannel_weights: rechannel_weights.into_boxed_slice(),
        layers: layers.into_boxed_slice(),
        head_weights: head_weights.into_boxed_slice(),
        head_bias,
        head_scale,
        expected_sample_rate: document.sample_rate.unwrap_or(-1.0),
        metadata: document.metadata.clone(),
    })
}

struct WeightReader<'a> {
    weights: &'a [f32],
    position: usize,
}

impl<'a> WeightReader<'a> {
    fn new(weights: &'a [f32]) -> Self {
        Self {
            weights,
            position: 0,
        }
    }

    fn take(&mut self) -> DspResult<f32> {
        let value = self
            .weights
            .get(self.position)
            .copied()
            .ok_or(DspError::WeightCount {
                expected: self.position + 1,
                actual: self.position,
            })?;
        self.position += 1;
        Ok(value)
    }

    fn take_many(&mut self, count: usize) -> DspResult<Box<[f32]>> {
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.take()?);
        }
        Ok(values.into_boxed_slice())
    }

    fn finish(self) -> DspResult<()> {
        if self.position == self.weights.len() {
            return Ok(());
        }
        Err(DspError::WeightCount {
            expected: self.position,
            actual: self.weights.len(),
        })
    }
}
