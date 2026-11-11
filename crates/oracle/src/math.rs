use ronie_core::NamDocument;

use crate::{DifferentialCase, ModelOracle, OracleError};

const KERNELS: [usize; 23] = [
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 15, 15, 6, 6, 6, 6, 6, 6, 6,
];
const DILATIONS: [usize; 23] = [
    1, 3, 7, 17, 41, 101, 239, 1, 3, 7, 17, 41, 101, 239, 1, 13, 1, 3, 7, 17, 41, 101, 239,
];

/// Independent mathematical A2 renderer with no DSP implementation dependency.
pub struct PublicMathOracle;

impl PublicMathOracle {
    /// Creates the public mathematical adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for PublicMathOracle {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelOracle for PublicMathOracle {
    fn render(&mut self, case: &DifferentialCase) -> Result<Vec<f64>, OracleError> {
        NaiveA2::new(case.document.clone()).map(|model| model.render(&case.input()))
    }
}

#[derive(Clone, Copy)]
struct LayerOffsets {
    conv: usize,
    conv_bias: usize,
    mixin: usize,
    layer1x1: usize,
    layer1x1_bias: usize,
}

struct NaiveA2 {
    channels: usize,
    weights: Vec<f32>,
    offsets: Vec<LayerOffsets>,
    head_offset: usize,
    histories: Vec<Vec<[f32; 8]>>,
    head_history: Vec<[f32; 8]>,
}

impl NaiveA2 {
    fn new(document: NamDocument) -> Result<Self, OracleError> {
        let channels = document.config["layers"][0]["channels"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|channels| matches!(channels, 3 | 8))
            .ok_or(OracleError::InvalidCase("public math requires A2 channels"))?;
        let expected = 179 * channels * channels + 86 * channels + 2;
        if document.weights.len() != expected {
            return Err(OracleError::InvalidCase(
                "public math weight count mismatch",
            ));
        }

        let mut offset = channels;
        let mut offsets = Vec::with_capacity(KERNELS.len());
        for kernel in KERNELS {
            let conv = offset;
            let conv_bias = conv + kernel * channels * channels;
            let mixin = conv_bias + channels;
            let layer1x1 = mixin + channels;
            let layer1x1_bias = layer1x1 + channels * channels;
            offsets.push(LayerOffsets {
                conv,
                conv_bias,
                mixin,
                layer1x1,
                layer1x1_bias,
            });
            offset = layer1x1_bias + channels;
        }
        if offset + 16 * channels + 2 != expected {
            return Err(OracleError::InvalidCase("public math head offset mismatch"));
        }

        Ok(Self {
            channels,
            weights: document.weights,
            offsets: offsets.clone(),
            head_offset: offset,
            histories: vec![Vec::new(); offsets.len()],
            head_history: Vec::new(),
        })
    }

    fn render(mut self, input: &[f64]) -> Vec<f64> {
        input
            .iter()
            .map(|sample| {
                let sample = *sample as f32;
                let mut layer_in = [0.0_f32; 8];
                for (channel, value) in layer_in.iter_mut().enumerate().take(self.channels) {
                    *value = self.weights[channel] * sample;
                }
                let mut head_sum = [0.0_f32; 8];

                for (layer_index, offsets) in self.offsets.iter().enumerate() {
                    let kernel = KERNELS[layer_index];
                    let dilation = DILATIONS[layer_index];
                    self.histories[layer_index].push(layer_in);
                    let history = &self.histories[layer_index];
                    let zero_history = [0.0_f32; 8];
                    let mut activated = [0.0_f32; 8];
                    for output_channel in 0..self.channels {
                        let mut value = self.weights[offsets.conv_bias + output_channel];
                        for tap in 0..kernel {
                            let taps_back = (kernel - 1 - tap) * dilation;
                            let history_index = history.len().checked_sub(1 + taps_back);
                            let history_values =
                                history_index.map_or(&zero_history, |index| &history[index]);
                            for (input_channel, history_sample) in
                                history_values.iter().enumerate().take(self.channels)
                            {
                                let weight = offsets.conv
                                    + output_channel * self.channels * kernel
                                    + input_channel * kernel
                                    + tap;
                                value += self.weights[weight] * history_sample;
                            }
                        }
                        value += self.weights[offsets.mixin + output_channel] * sample;
                        activated[output_channel] = if value >= 0.0 { value } else { value * 0.01 };
                        head_sum[output_channel] += activated[output_channel];
                    }
                    for (output_channel, residual) in
                        layer_in.iter_mut().enumerate().take(self.channels)
                    {
                        let mut projected = self.weights[offsets.layer1x1_bias + output_channel];
                        for (input_channel, activated_sample) in
                            activated.iter().enumerate().take(self.channels)
                        {
                            projected += self.weights
                                [offsets.layer1x1 + input_channel * self.channels + output_channel]
                                * activated_sample;
                        }
                        *residual += projected;
                    }
                }

                self.head_history.push(head_sum);
                let zero_history = [0.0_f32; 8];
                let mut output = self.weights[self.head_offset + 16 * self.channels];
                for tap in 0..16 {
                    let history_index = self.head_history.len().checked_sub(1 + 15 - tap);
                    let history_values =
                        history_index.map_or(&zero_history, |index| &self.head_history[index]);
                    for (channel, history_sample) in
                        history_values.iter().enumerate().take(self.channels)
                    {
                        output +=
                            self.weights[self.head_offset + channel * 16 + tap] * history_sample;
                    }
                }
                (output * self.weights[self.head_offset + 16 * self.channels + 1]) as f64
            })
            .collect()
    }
}
