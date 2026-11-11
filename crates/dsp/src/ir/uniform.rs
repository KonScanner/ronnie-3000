#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Complex32 {
    pub(crate) re: f32,
    pub(crate) im: f32,
}

impl Complex32 {
    fn multiply_add(self, other: Self, output: &mut Self) {
        let re = self.re * other.re - self.im * other.im;
        let im = self.re * other.im + self.im * other.re;
        output.re += re;
        output.im += im;
    }

    fn multiply(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}

pub(crate) struct FftPlan {
    size: usize,
    bit_reversal: Vec<usize>,
    twiddles: Vec<Vec<Complex32>>,
}

impl FftPlan {
    pub(crate) fn new(size: usize) -> Self {
        let levels = size.ilog2() as usize;
        let mut bit_reversal = Vec::with_capacity(size);
        for index in 0..size {
            bit_reversal.push(index.reverse_bits() >> (usize::BITS as usize - levels));
        }
        let mut twiddles = Vec::with_capacity(levels + 1);
        twiddles.push(Vec::new());
        for level in 1..=levels {
            let length = 1 << level;
            let mut stage = Vec::with_capacity(length / 2);
            for index in 0..length / 2 {
                let angle = -2.0 * std::f64::consts::PI * index as f64 / length as f64;
                stage.push(Complex32 {
                    re: angle.cos() as f32,
                    im: angle.sin() as f32,
                });
            }
            twiddles.push(stage);
        }
        Self {
            size,
            bit_reversal,
            twiddles,
        }
    }

    pub(crate) fn forward(&self, input: &[f32], output: &mut [Complex32]) {
        for (index, value) in input.iter().enumerate() {
            output[index] = Complex32 {
                re: *value,
                im: 0.0,
            };
        }
        self.transform(output, false);
    }

    pub(crate) fn inverse(&self, input: &mut [Complex32], output: &mut [f32]) {
        self.transform(input, true);
        for (index, value) in input.iter().enumerate() {
            output[index] = value.re / self.size as f32;
        }
    }

    fn transform(&self, values: &mut [Complex32], inverse: bool) {
        for (index, reversed) in self.bit_reversal.iter().enumerate() {
            if index < *reversed {
                values.swap(index, *reversed);
            }
        }
        let mut length = 2;
        let mut stage = 1;
        while length <= self.size {
            let half = length / 2;
            for start in (0..self.size).step_by(length) {
                for index in 0..half {
                    let left = values[start + index];
                    let mut twiddle = self.twiddles[stage][index];
                    if inverse {
                        twiddle.im = -twiddle.im;
                    }
                    let right = values[start + index + half].multiply(twiddle);
                    values[start + index] = Complex32 {
                        re: left.re + right.re,
                        im: left.im + right.im,
                    };
                    values[start + index + half] = Complex32 {
                        re: left.re - right.re,
                        im: left.im - right.im,
                    };
                }
            }
            length *= 2;
            stage += 1;
        }
    }
}

pub(crate) struct PartitionEngine {
    fft: FftPlan,
    block_size: usize,
    fft_size: usize,
    num_segments: usize,
    num_input_segments: usize,
    index_step: usize,
    current_segment: usize,
    input_position: usize,
    input: Vec<f32>,
    overlap: Vec<f32>,
    input_segments: Vec<Vec<Complex32>>,
    impulse_segments: Vec<Vec<Complex32>>,
    current_spectrum: Vec<Complex32>,
    history_spectrum: Vec<Complex32>,
    output_spectrum: Vec<Complex32>,
    time_output: Vec<f32>,
}

impl PartitionEngine {
    pub(crate) fn new(samples: &[f32], maximum_block_size: usize) -> Self {
        let block_size = maximum_block_size.next_power_of_two();
        let fft_size = if block_size > 128 {
            block_size * 2
        } else {
            block_size * 4
        };
        let segment_size = fft_size - block_size;
        let num_segments = samples.len() / segment_size + 1;
        let num_input_segments = if block_size > 128 {
            num_segments
        } else {
            3 * num_segments
        };
        let fft = FftPlan::new(fft_size);
        let mut impulse_segments = vec![vec![Complex32::default(); fft_size]; num_segments];
        for (segment, impulse) in impulse_segments.iter_mut().enumerate() {
            let start = segment * segment_size;
            let end = samples.len().min(start + segment_size);
            let mut time = vec![0.0; fft_size];
            if start < samples.len() {
                time[..end - start].copy_from_slice(&samples[start..end]);
            }
            fft.forward(&time, impulse);
        }
        Self {
            fft,
            block_size,
            fft_size,
            num_segments,
            num_input_segments,
            index_step: num_input_segments / num_segments,
            current_segment: 0,
            input_position: 0,
            input: vec![0.0; fft_size],
            overlap: vec![0.0; fft_size],
            input_segments: vec![vec![Complex32::default(); fft_size]; num_input_segments],
            impulse_segments,
            current_spectrum: vec![Complex32::default(); fft_size],
            history_spectrum: vec![Complex32::default(); fft_size],
            output_spectrum: vec![Complex32::default(); fft_size],
            time_output: vec![0.0; fft_size],
        }
    }

    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32], added_latency: bool) {
        debug_assert_eq!(input.len(), output.len());
        if added_latency {
            self.process_added_latency(input, output);
            return;
        }
        let mut processed = 0;
        while processed < input.len() {
            let was_empty = self.input_position == 0;
            let count = (input.len() - processed).min(self.block_size - self.input_position);
            self.input[self.input_position..self.input_position + count]
                .copy_from_slice(&input[processed..processed + count]);
            self.compute_time_output(was_empty);
            for index in 0..count {
                let time_index = self.input_position + index;
                output[processed + index] = self.time_output[time_index] + self.overlap[time_index];
            }
            self.input_position += count;
            if self.input_position == self.block_size {
                self.input.fill(0.0);
                self.input_position = 0;
                self.advance_segments();
            }
            processed += count;
        }
    }

    fn process_added_latency(&mut self, input: &[f32], output: &mut [f32]) {
        let mut processed = 0;
        while processed < input.len() {
            let count = (input.len() - processed).min(self.block_size - self.input_position);
            self.input[self.input_position..self.input_position + count]
                .copy_from_slice(&input[processed..processed + count]);
            for index in 0..count {
                output[processed + index] = self.time_output[self.input_position + index];
            }
            self.input_position += count;
            if self.input_position == self.block_size {
                self.compute_time_output(true);
                for index in 0..self.block_size {
                    self.time_output[index] += self.overlap[index];
                }
                self.input.fill(0.0);
                self.input_position = 0;
                self.advance_segments();
            }
            processed += count;
        }
    }

    fn compute_time_output(&mut self, was_empty: bool) {
        self.fft.forward(&self.input, &mut self.current_spectrum);
        self.input_segments[self.current_segment].copy_from_slice(&self.current_spectrum);
        if was_empty {
            self.history_spectrum.fill(Complex32::default());
            let mut index = self.current_segment;
            for segment in 1..self.num_segments {
                index += self.index_step;
                if index >= self.num_input_segments {
                    index -= self.num_input_segments;
                }
                accumulate(
                    &self.input_segments[index],
                    &self.impulse_segments[segment],
                    &mut self.history_spectrum,
                );
            }
        }
        self.output_spectrum.copy_from_slice(&self.history_spectrum);
        accumulate(
            &self.current_spectrum,
            &self.impulse_segments[0],
            &mut self.output_spectrum,
        );
        self.fft
            .inverse(&mut self.output_spectrum, &mut self.time_output);
    }

    fn advance_segments(&mut self) {
        for index in self.block_size..self.fft_size - self.block_size {
            self.time_output[index] += self.overlap[index];
        }
        for index in 0..self.fft_size - self.block_size {
            self.overlap[index] = self.time_output[self.block_size + index];
        }
        self.current_segment = if self.current_segment > 0 {
            self.current_segment - 1
        } else {
            self.num_input_segments - 1
        };
    }
}

fn accumulate(input: &[Complex32], impulse: &[Complex32], output: &mut [Complex32]) {
    for (index, value) in input.iter().enumerate() {
        value.multiply_add(impulse[index], &mut output[index]);
    }
}
