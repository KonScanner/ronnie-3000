//! Tone knob changes keep the filter history and do not click.

use ronie_dsp::{
    PlanarBuffer,
    tone_stack::{ToneControls, ToneStack},
};

const FS: f64 = 48_000.0;
const BLOCK: usize = 512;

fn max_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn bass_change_mid_stream_does_not_click() {
    let change_block = 40;
    let blocks = 80;
    let input = (0..blocks * BLOCK)
        .map(|index| 0.5 * (2.0 * std::f64::consts::PI * 200.0 * index as f64 / FS).sin() as f32)
        .collect::<Vec<_>>();
    let mut stack = ToneStack::new(FS).expect("tone preparation");
    let mut output = Vec::with_capacity(input.len());
    for (block, source) in input.chunks(BLOCK).enumerate() {
        let bass = if block < change_block { 5.0 } else { 8.0 };
        stack.set_controls(ToneControls {
            bass,
            ..ToneControls::default()
        });
        let mut data = source.to_vec();
        stack
            .process(&mut PlanarBuffer::new(&mut data, 1, BLOCK).expect("buffer"))
            .expect("tone processing");
        output.extend_from_slice(&data);
    }
    // Measured at 48 kHz: the steady step is 0.0131 before and 0.0189 after
    // (+12 dB bass lifts 200 Hz). Restarting the filters from zero state gave
    // 0.0204 in the first 50 ms; the 20 ms ramp gives 0.0191.
    let change = change_block * BLOCK;
    let before = max_step(&output[change - 4800..change]);
    let transition = max_step(&output[change - 1..change + 2400]);
    let settled = max_step(&output[change + 9600..]);
    assert!(
        transition <= 1.5 * before,
        "step {transition} after the change, {before} before it"
    );
    assert!(
        transition <= 1.02 * before.max(settled),
        "step {transition} after the change, steady steps {before} and {settled}"
    );
}
