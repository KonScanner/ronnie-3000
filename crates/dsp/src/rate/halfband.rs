/// First allpass branch coefficients for each half-band doubling.
pub const COEFFICIENTS_A: [f32; 6] = [
    f32::from_bits(0x3d163f58),
    f32::from_bits(0x3e8c9c89),
    f32::from_bits(0x3f0fa42f),
    f32::from_bits(0x3f450dcd),
    f32::from_bits(0x3f646b34),
    f32::from_bits(0x3f764bd4),
];

/// Second allpass branch coefficients for each half-band doubling.
pub const COEFFICIENTS_B: [f32; 6] = [
    f32::from_bits(0x3e0bd324),
    f32::from_bits(0x3ed8a5a0),
    f32::from_bits(0x3f2d7344),
    f32::from_bits(0x3f570302),
    f32::from_bits(0x3f6e7989),
    f32::from_bits(0x3f7ce189),
];

pub(crate) const CHANNELS: usize = 2;
pub(crate) const SECTIONS: usize = 6;
pub(crate) const MAX_STAGES: usize = 3;

#[derive(Clone, Copy, Default)]
pub(crate) struct AllpassState {
    x1: f32,
    y1: f32,
}

pub(crate) type BranchStates = [AllpassState; MAX_STAGES * CHANNELS * SECTIONS];

pub(crate) const EMPTY_STATES: BranchStates =
    [AllpassState { x1: 0.0, y1: 0.0 }; MAX_STAGES * CHANNELS * SECTIONS];

fn process_allpass(x: f32, coefficient: f32, state: &mut AllpassState) -> f32 {
    let y = coefficient * x + state.x1 - coefficient * state.y1;
    state.x1 = x;
    state.y1 = y;
    y
}

fn process_branch(
    x: f32,
    coefficients: &[f32; SECTIONS],
    states: &mut BranchStates,
    stage: usize,
    channel: usize,
) -> f32 {
    let start = (stage * CHANNELS + channel) * SECTIONS;
    let mut y = x;
    for (index, coefficient) in coefficients.iter().enumerate() {
        y = process_allpass(y, *coefficient, &mut states[start + index]);
    }
    y
}

pub(crate) fn upsample_stage(
    stage: usize,
    channel: usize,
    input: &[f32],
    output: &mut [f32],
    up_a: &mut BranchStates,
    up_b: &mut BranchStates,
) {
    debug_assert_eq!(output.len(), input.len() * 2);
    for (index, sample) in input.iter().enumerate() {
        output[index * 2] = process_branch(*sample, &COEFFICIENTS_A, up_a, stage, channel);
        output[index * 2 + 1] = process_branch(*sample, &COEFFICIENTS_B, up_b, stage, channel);
    }
}

pub(crate) fn decimate_stage(
    stage: usize,
    channel: usize,
    input: &[f32],
    output: &mut [f32],
    down_even: &mut BranchStates,
    down_odd: &mut BranchStates,
) {
    debug_assert_eq!(output.len() * 2, input.len());
    for (index, pair) in input.as_chunks::<2>().0.iter().enumerate() {
        let even = process_branch(pair[0], &COEFFICIENTS_B, down_even, stage, channel);
        let odd = process_branch(pair[1], &COEFFICIENTS_A, down_odd, stage, channel);
        output[index] = 0.5 * (even + odd);
    }
}
