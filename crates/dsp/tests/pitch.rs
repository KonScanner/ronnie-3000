//! Correlation-spliced pitch shifting and input-stage compatibility tests.

use ronie_dsp::{
    PlanarBuffer,
    input::{InputControls, InputStage, PitchParams, PitchShifter, PitchWindow},
};

const FS: f64 = 48_000.0;
const BLOCK: usize = 512;

fn sine(frames: usize, hz: f64, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|index| amplitude * (2.0 * std::f64::consts::PI * hz * index as f64 / FS).sin() as f32)
        .collect()
}

fn noise(frames: usize, mut seed: u32, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            amplitude * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

fn goertzel_power(samples: &[f32], hz: f64) -> f64 {
    let omega = 2.0 * std::f64::consts::PI * hz / FS;
    let coefficient = 2.0 * omega.cos();
    let mut previous = 0.0;
    let mut current = 0.0;
    for sample in samples {
        let next = f64::from(*sample) + coefficient * current - previous;
        previous = current;
        current = next;
    }
    let real = current - previous * omega.cos();
    let imaginary = previous * omega.sin();
    (real * real + imaginary * imaginary) / (samples.len() * samples.len()) as f64
}

fn db(power: f64) -> f64 {
    10.0 * power.max(1.0e-30).log10()
}

fn run_pitch(input: &[f32], params: PitchParams) -> Vec<f32> {
    run_pitch_with(input, params, FS, None)
}

fn run_pitch_with(
    input: &[f32],
    params: PitchParams,
    sample_rate: f64,
    change: Option<(PitchParams, usize)>,
) -> Vec<f32> {
    let mut pitch = PitchShifter::new(sample_rate, BLOCK).expect("pitch preparation");
    pitch.set_enabled(true);
    pitch.set_params(params);
    let mut output = Vec::with_capacity(input.len());
    for (index, source) in input.chunks(BLOCK).enumerate() {
        if let Some((new_params, at)) = change
            && index * BLOCK == at
        {
            pitch.set_params(new_params);
        }
        let mut data = source.to_vec();
        pitch
            .process(&mut PlanarBuffer::new(&mut data, 1, source.len()).expect("buffer"))
            .expect("pitch processing");
        output.extend_from_slice(&data);
    }
    output
}

fn expect_shifted_to(output: &[f32], original: f64, expected: f64) {
    const SETTLE: usize = 48_000;
    const WINDOW: usize = 65_536;
    assert!(output.len() >= SETTLE + WINDOW);
    let settled = &output[SETTLE..SETTLE + WINDOW];
    let at_expected = db(goertzel_power(settled, expected));
    let at_original = db(goertzel_power(settled, original));
    assert!(
        at_expected > at_original + 20.0,
        "expected {expected} Hz at {at_expected} dB, not {original} Hz at {at_original} dB",
    );
}

fn max_step(samples: &[f32], from: usize, to: usize) -> f32 {
    samples[from + 1..to]
        .iter()
        .zip(&samples[from..to - 1])
        .map(|(current, previous)| (current - previous).abs())
        .fold(0.0, f32::max)
}

fn rms(samples: &[f32]) -> f64 {
    (samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt()
}

/// Fourth-order Butterworth low-pass at 150 Hz, to see only the bass band.
fn bass_band(samples: &[f32]) -> Vec<f32> {
    let mut output = samples.to_vec();
    for q in [0.541_196_1, 1.306_563] {
        let omega = 2.0 * std::f64::consts::PI * 150.0 / FS;
        let alpha = omega.sin() / (2.0 * q);
        let cosine = omega.cos();
        let a0 = 1.0 + alpha;
        let b0 = (1.0 - cosine) / 2.0 / a0;
        let b1 = (1.0 - cosine) / a0;
        let a1 = -2.0 * cosine / a0;
        let a2 = (1.0 - alpha) / a0;
        let (mut z1, mut z2) = (0.0, 0.0);
        for sample in &mut output {
            let input = f64::from(*sample);
            let filtered = b0 * input + z1;
            z1 = b1 * input - a1 * filtered + z2;
            z2 = b0 * input - a2 * filtered;
            *sample = filtered as f32;
        }
    }
    output
}

#[test]
fn bass_onsets_resplice_without_a_thump() {
    let frames = 5 * 48_000;
    let clean = sine(frames, 41.0, 0.5);
    let mut clicked = clean.clone();
    let burst = 144;
    for start in (48_000..frames - burst).step_by(14_400) {
        for (offset, sample) in clicked[start..start + burst].iter_mut().enumerate() {
            let envelope = (std::f64::consts::PI * offset as f64 / burst as f64).sin();
            let tone = (2.0 * std::f64::consts::PI * 1000.0 * offset as f64 / FS).sin();
            *sample += (0.3 * envelope * tone) as f32;
        }
    }
    let params = PitchParams {
        semitones: -2.0,
        window: PitchWindow::Ms40,
        ..Default::default()
    };
    // The bound comes from the clean signal: the same shifted note with no
    // bursts, regular splices included, has a bass-band step of 0.0024. The
    // bursts add only 1% to the bass band of the input. Measured: the 4 ms
    // span and 2 ms onset fade gave 0.0071 (2.97x). The low-note onset with
    // no correlation floor gave 0.0028 (1.16x); with the floor it gives
    // 0.0024 (1.01x).
    let clean_step = max_step(&bass_band(&run_pitch(&clean, params)), 48_000, frames);
    let clicked_step = max_step(&bass_band(&run_pitch(&clicked, params)), 48_000, frames);
    assert!(
        clicked_step < 1.1 * clean_step,
        "bass-band step {clicked_step} with onsets, {clean_step} without",
    );
}

#[test]
fn shifted_chord_holds_a_steady_level() {
    let frames = 5 * 48_000;
    let mut chord = sine(frames, 110.0, 0.2);
    for hz in [138.6, 164.8] {
        for (sample, note) in chord.iter_mut().zip(sine(frames, hz, 0.2)) {
            *sample += note;
        }
    }
    let output = run_pitch(
        &chord,
        PitchParams {
            semitones: -4.0,
            window: PitchWindow::Ms20,
            ..Default::default()
        },
    );
    let levels = output[48_000..]
        .as_chunks::<2400>()
        .0
        .iter()
        .map(|window| db(rms(window) * rms(window)))
        .collect::<Vec<_>>();
    // The three notes beat, so the input itself spreads 2.17 dB over 50 ms
    // windows. Measured: fades that could fill the whole splice interval and
    // the linear score gave 3.30 dB; the fade cap and squared score give
    // 2.89 dB.
    let loudest = levels.iter().copied().fold(f64::MIN, f64::max);
    let quietest = levels.iter().copied().fold(f64::MAX, f64::min);
    assert!(
        loudest - quietest < 3.0,
        "level spread {} dB",
        loudest - quietest
    );
}

#[test]
fn default_window_is_the_lowest_delay() {
    use ronie_core::params::{ParameterCatalog, ParameterId};

    assert_eq!(PitchWindow::default(), PitchWindow::Ms20);
    assert_eq!(PitchParams::default().window, PitchWindow::Ms20);
    let spec = ParameterCatalog::v1()
        .iter()
        .find(|spec| spec.id == ParameterId("pitchWindow"))
        .expect("pitchWindow parameter");
    assert_eq!(
        PitchWindow::from_index(spec.default.round() as i32),
        PitchWindow::default()
    );
}

#[test]
fn latency_matches_every_window_and_rate() {
    for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
        let mut pitch = PitchShifter::new(sample_rate, BLOCK).expect("pitch preparation");
        pitch.set_enabled(true);
        for index in 0..4 {
            let window = PitchWindow::from_index(index);
            pitch.set_params(PitchParams {
                window,
                ..Default::default()
            });
            let floor = (sample_rate * 0.002) as u64;
            let buffer = (sample_rate * window.milliseconds() as f64 * 0.001) as u64;
            assert_eq!(pitch.latency_samples(), (floor + buffer) / 2);
            assert_eq!(
                pitch.latency_samples(),
                PitchShifter::latency_samples_for(window, sample_rate).expect("valid rate"),
            );
        }
    }
    assert_eq!(
        PitchShifter::latency_samples_for(PitchWindow::Ms30, FS).expect("valid rate"),
        768,
    );
    assert_eq!(PitchWindow::Ms30.latency_milliseconds(), 16.0);
}

#[test]
fn unity_ratio_is_a_pure_delay_at_the_floor() {
    let floor = PitchShifter::min_delay_samples(FS) as usize;
    let input = noise(2 * 48_000, 3, 0.4);
    let output = run_pitch(&input, PitchParams::default());
    for index in 48_000..48_000 + 8192 {
        assert!((output[index] - input[index - floor]).abs() < 1.0e-5);
    }
    let tone = sine(3 * 48_000, 440.0, 0.5);
    let output = run_pitch(&tone, PitchParams::default());
    let gain = db(goertzel_power(&output[96_000..96_000 + 16_384], 440.0))
        - db(goertzel_power(&tone[96_000..96_000 + 16_384], 440.0));
    assert!(gain.abs() < 0.05);
}

#[test]
fn octaves_and_fractional_shifts_land_on_frequency() {
    let input = sine(3 * 48_000, 440.0, 0.5);
    for semitones in [12.0, -12.0, 24.0, -24.0] {
        for index in 0..4 {
            let params = PitchParams {
                semitones,
                window: PitchWindow::from_index(index),
                ..Default::default()
            };
            expect_shifted_to(
                &run_pitch(&input, params),
                440.0,
                440.0 * 2.0_f64.powf(f64::from(semitones) / 12.0),
            );
        }
    }
    let params = PitchParams {
        semitones: -1.5,
        ..Default::default()
    };
    expect_shifted_to(
        &run_pitch(&input, params),
        440.0,
        440.0 * 2.0_f64.powf(-1.5 / 12.0),
    );
}

#[test]
fn shifted_tone_keeps_unity_gain_at_every_window() {
    let input = sine(3 * 48_000, 220.0, 0.5);
    for index in 0..4 {
        let output = run_pitch(
            &input,
            PitchParams {
                semitones: -2.0,
                window: PitchWindow::from_index(index),
                ..Default::default()
            },
        );
        let expected = 220.0 * 2.0_f64.powf(-2.0 / 12.0);
        let gain = db(goertzel_power(&output[48_000..48_000 + 65_536], expected))
            - db(goertzel_power(&input[48_000..48_000 + 65_536], 220.0));
        assert!(gain.abs() < 1.0, "window {index}: {gain} dB");
    }
}

#[test]
fn uncorrelated_splices_hold_level_within_one_db() {
    let input = noise(4 * 48_000, 21, 0.4);
    let output = run_pitch(
        &input,
        PitchParams {
            semitones: -2.0,
            ..Default::default()
        },
    );
    let mut levels = Vec::new();
    for end in (48_000 + 960..=output.len()).step_by(960) {
        levels.push(db(output[end - 960..end]
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>()
            / 960.0));
    }
    let mut sorted = levels.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    for (index, level) in levels.into_iter().enumerate() {
        assert!(
            (level - median).abs() < 1.0,
            "window {index}: {level} vs {median}"
        );
    }
}

#[test]
fn tonality_limit_passes_highs_unshifted() {
    let input = sine(3 * 48_000, 6000.0, 0.5);
    let params = PitchParams {
        semitones: 12.0,
        ..Default::default()
    };
    expect_shifted_to(&run_pitch(&input, params), 6000.0, 12_000.0);
    let params = PitchParams {
        tonality_hz: 2000.0,
        ..params
    };
    expect_shifted_to(&run_pitch(&input, params), 12_000.0, 6000.0);
    let low = sine(3 * 48_000, 440.0, 0.5);
    expect_shifted_to(&run_pitch(&low, params), 440.0, 880.0);
}

#[test]
fn pick_attacks_arrive_within_the_resync_budget() {
    let floor = PitchShifter::min_delay_samples(FS) as usize;
    let span = (FS * 0.004) as usize;
    let fade = (FS * 0.002) as usize;
    for hold_ms in [700, 1150, 1600, 2050] {
        let hold = hold_ms * 48;
        let mut input = sine(hold + 24_000, 110.0, 0.1);
        input[hold..].copy_from_slice(&noise(24_000, 11, 0.9));
        let output = run_pitch(
            &input,
            PitchParams {
                semitones: -2.0,
                ..Default::default()
            },
        );
        let arrival = (hold + 48..hold + 24_000)
            .find(|end| rms(&output[end - 48..*end]) > 0.3)
            .unwrap_or_else(|| panic!("no attack at {hold_ms} ms"));
        let lag = arrival - hold;
        assert!(lag >= floor, "{hold_ms} ms: lag {lag}");
        assert!(lag <= floor + span + fade + 96, "{hold_ms} ms: lag {lag}");
    }
}

#[test]
fn pick_attacks_during_a_splice_fade_keep_the_resync_budget() {
    // At -2 semitones in the 20 ms window, one splice cycle is about 140 ms
    // and its fade fills up to half of it. Attacks every 10 ms over 160 ms
    // thus land in regular fades too. Before an onset could cut a fade
    // short, the attack at 1600 ms arrived after 717 samples.
    let floor = PitchShifter::min_delay_samples(FS) as usize;
    let span = (FS * 0.004) as usize;
    let fade = (FS * 0.002) as usize;
    for hold_ms in (1600..1760).step_by(10) {
        let hold = hold_ms * 48;
        let mut input = sine(hold + 4800, 110.0, 0.1);
        input[hold..].copy_from_slice(&noise(4800, 11, 0.9));
        let output = run_pitch(
            &input,
            PitchParams {
                semitones: -2.0,
                window: PitchWindow::Ms20,
                ..Default::default()
            },
        );
        let arrival = (hold + 48..hold + 4800)
            .find(|end| rms(&output[end - 48..*end]) > 0.3)
            .unwrap_or_else(|| panic!("no attack at {hold_ms} ms"));
        let lag = arrival - hold;
        assert!(lag >= floor, "{hold_ms} ms: lag {lag}");
        assert!(lag <= floor + span + fade + 96, "{hold_ms} ms: lag {lag}");
    }
}

#[test]
fn stereo_channels_share_one_control_tap() {
    let mut pitch = PitchShifter::new(FS, BLOCK).expect("pitch preparation");
    pitch.set_enabled(true);
    pitch.set_params(PitchParams {
        semitones: -3.0,
        ..Default::default()
    });
    let input = noise(192 * BLOCK, 5, 0.4);
    for source in input.chunks(BLOCK) {
        let mut data = Vec::with_capacity(2 * BLOCK);
        data.extend_from_slice(source);
        data.extend(source.iter().map(|sample| sample * 0.5));
        pitch
            .process(&mut PlanarBuffer::new(&mut data, 2, BLOCK).expect("stereo buffer"))
            .expect("pitch processing");
        let (left, right) = data.split_at(BLOCK);
        for index in 0..BLOCK {
            assert!((right[index] - 0.5 * left[index]).abs() < 1.0e-6);
        }
    }
}

#[test]
fn mono_reprepare_and_extreme_shifts_stay_finite() {
    let mut pitch = PitchShifter::new(44_100.0, 256).expect("pitch preparation");
    pitch.set_enabled(true);
    pitch.set_params(PitchParams {
        semitones: 5.0,
        ..Default::default()
    });
    let mut data = noise(256, 1, 0.3);
    pitch
        .process(&mut PlanarBuffer::new(&mut data, 1, 256).expect("mono buffer"))
        .expect("mono processing");
    pitch.prepare(96_000.0, 1024).expect("reprepare");
    for semitones in [-24.0, 24.0] {
        pitch.set_params(PitchParams {
            semitones,
            ..Default::default()
        });
        let mut data = noise(4096, 2, 0.3);
        pitch
            .process(&mut PlanarBuffer::new(&mut data, 1, 4096).expect("large buffer"))
            .expect("large processing");
        assert!(
            data.iter()
                .all(|sample| sample.is_finite() && sample.abs() < 10.0)
        );
    }
}

#[test]
fn smooth_sweep_never_breaks_the_tone() {
    let blocks = 4 * 48_000 / BLOCK;
    let input = sine(blocks * BLOCK, 220.0, 0.5);
    let mut pitch = PitchShifter::new(FS, BLOCK).expect("pitch preparation");
    pitch.set_enabled(true);
    let mut output = Vec::with_capacity(input.len());
    for (block, source) in input.chunks(BLOCK).enumerate() {
        let phase = block as f64 / blocks as f64;
        let semitones = if phase < 0.25 {
            96.0 * phase
        } else if phase < 0.75 {
            24.0 - 96.0 * (phase - 0.25)
        } else {
            -24.0 + 96.0 * (phase - 0.75)
        };
        pitch.set_params(PitchParams {
            semitones: semitones as f32,
            ..Default::default()
        });
        let mut data = source.to_vec();
        pitch
            .process(&mut PlanarBuffer::new(&mut data, 1, BLOCK).expect("buffer"))
            .expect("pitch processing");
        output.extend_from_slice(&data);
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(max_step(&output, 48_000, output.len()) < 0.12);
    for end in (48_000..=output.len()).step_by(480) {
        assert!(rms(&output[end - 480..end]) > 0.15, "hole at {end}");
    }
}

#[test]
fn window_and_tonality_changes_are_seamless() {
    let input = sine(3 * 48_000, 220.0, 0.5);
    let first = PitchParams {
        semitones: -2.0,
        window: PitchWindow::Ms60,
        ..Default::default()
    };
    let second = PitchParams {
        window: PitchWindow::Ms20,
        ..first
    };
    let switch_at = 96 * BLOCK;
    let output = run_pitch_with(&input, first, FS, Some((second, switch_at)));
    assert!(max_step(&output, switch_at - 4800, switch_at + 4800) < 0.03);
    for end in (switch_at + 240..=switch_at + 4800).step_by(240) {
        assert!(rms(&output[end - 240..end]) > 0.25, "hole at {end}");
    }
    let second = PitchParams {
        tonality_hz: 3000.0,
        ..first
    };
    let output = run_pitch_with(&input, first, FS, Some((second, switch_at)));
    assert!(max_step(&output, switch_at - 4800, switch_at + 4800) < 0.03);
}

#[test]
fn power_blends_then_returns_to_bit_exact_bypass() {
    let length = 288 * BLOCK;
    let input = sine(length, 220.0, 0.5);
    let mut pitch = PitchShifter::new(FS, BLOCK).expect("pitch preparation");
    let params = PitchParams {
        semitones: -5.0,
        ..Default::default()
    };
    let mut output = Vec::with_capacity(length);
    let mut stopped_at = None;
    for (block, source) in input.chunks(BLOCK).enumerate() {
        let offset = block * BLOCK;
        if offset == 48 * BLOCK {
            pitch.set_enabled(true);
        }
        if offset == 120 * BLOCK {
            pitch.set_enabled(false);
        }
        if offset == 121 * BLOCK {
            pitch.set_enabled(true);
        }
        if offset == 192 * BLOCK {
            pitch.set_enabled(false);
        }
        let mut data = source.to_vec();
        if pitch.is_running() {
            pitch.set_params(params);
            pitch
                .process(&mut PlanarBuffer::new(&mut data, 1, BLOCK).expect("buffer"))
                .expect("pitch processing");
        } else if offset > 192 * BLOCK && stopped_at.is_none() {
            stopped_at = Some(offset);
        }
        output.extend_from_slice(&data);
    }
    assert!(max_step(&output, 0, length) < 0.03);
    let stopped_at = stopped_at.expect("pitch stopped");
    assert!(stopped_at - 192 * BLOCK < 2400);
    assert_eq!(&output[..48 * BLOCK], &input[..48 * BLOCK]);
    assert_eq!(&output[stopped_at..], &input[stopped_at..]);
    let shifted = 220.0 * 2.0_f64.powf(-5.0 / 12.0);
    assert!(
        db(goertzel_power(
            &output[60 * BLOCK..60 * BLOCK + 32_768],
            shifted
        )) > db(goertzel_power(
            &output[60 * BLOCK..60 * BLOCK + 32_768],
            220.0
        )) + 20.0,
    );
}

#[test]
fn upshift_on_the_smallest_buffer_stays_within_three_cents() {
    let input = sine(3 * 48_000, 440.0, 0.5);
    let output = run_pitch(
        &input,
        PitchParams {
            semitones: 12.0,
            window: PitchWindow::Ms20,
            ..Default::default()
        },
    );
    let mut best_frequency = 0.0;
    let mut best_power = 0.0;
    for step in 0..=80 {
        let frequency = 870.0 + step as f64 * 0.25;
        let power = goertzel_power(&output[48_000..48_000 + 65_536], frequency);
        if power > best_power {
            best_power = power;
            best_frequency = frequency;
        }
    }
    assert!((1200.0 * (best_frequency / 880.0).log2()).abs() < 3.0);
}

#[test]
fn input_stage_rounds_step_and_restores_zero_latency_bypass() {
    for step in [true, false] {
        let mut stage = InputStage::new(FS, BLOCK).expect("input preparation");
        let controls = InputControls {
            gate_enabled: false,
            pitch_enabled: true,
            pitch_step: step,
            pitch: PitchParams {
                semitones: -1.6,
                ..Default::default()
            },
            ..Default::default()
        };
        let input = sine(3 * 48_000, 440.0, 0.5);
        let mut output = Vec::with_capacity(input.len());
        for source in input.chunks(BLOCK) {
            let mut data = source.to_vec();
            stage
                .process(
                    &mut PlanarBuffer::new(&mut data, 1, source.len()).expect("buffer"),
                    controls,
                )
                .expect("input processing");
            output.extend_from_slice(&data);
        }
        let played = 440.0 * 2.0_f64.powf(if step { -2.0 } else { -1.6 } / 12.0);
        let other = 440.0 * 2.0_f64.powf(if step { -1.6 } else { -2.0 } / 12.0);
        expect_shifted_to(&output, other, played);

        let mut controls = controls;
        controls.pitch_enabled = false;
        for _ in 0..4 {
            let mut data = vec![0.25; BLOCK];
            stage
                .process(
                    &mut PlanarBuffer::new(&mut data, 1, BLOCK).expect("buffer"),
                    controls,
                )
                .expect("power-down processing");
        }
        let mut data = noise(BLOCK, 8, 0.4);
        let original = data.clone();
        stage
            .process(
                &mut PlanarBuffer::new(&mut data, 1, BLOCK).expect("buffer"),
                controls,
            )
            .expect("bypass processing");
        assert_eq!(data, original);
        assert_eq!(stage.latency_samples(controls), 0);
    }
}

#[test]
fn duplicated_stereo_input_matches_mono_processing() {
    let input = sine(3 * 48_000, 440.0, 0.5);
    let params = PitchParams {
        semitones: -12.0,
        ..Default::default()
    };
    let mono = run_pitch(&input, params);
    let mut stereo = PitchShifter::new(FS, BLOCK).expect("pitch preparation");
    stereo.set_enabled(true);
    stereo.set_params(params);
    let mut left = Vec::with_capacity(input.len());
    for source in input.chunks(BLOCK) {
        let mut data = Vec::with_capacity(source.len() * 2);
        data.extend_from_slice(source);
        data.extend_from_slice(source);
        stereo
            .process(&mut PlanarBuffer::new(&mut data, 2, source.len()).expect("stereo buffer"))
            .expect("stereo processing");
        let (channel, _) = data.split_at(source.len());
        left.extend_from_slice(channel);
    }
    for index in 0..input.len() {
        assert!(
            (left[index] - mono[index]).abs() < 1.0e-6,
            "sample {index}: stereo={} mono={}",
            left[index],
            mono[index],
        );
    }
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_pitch_matches_reference_frequency_and_latency() {
    use std::collections::BTreeMap;

    use ronie_core::InputMode;
    use ronie_dsp::PrepareSpec;
    use ronie_oracle::{ChainOracle, ChainScenario, OracleError};

    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let mut callback_frames = vec![BLOCK; 281];
    callback_frames.push(128);
    let total_frames = callback_frames.iter().sum::<usize>();
    let input = sine(total_frames, 440.0, 0.5);
    let mut stereo_input = input.clone();
    stereo_input.extend_from_slice(&input);
    let scenario = ChainScenario {
        schema_version: 1,
        spec: PrepareSpec {
            host_sample_rate: FS,
            max_host_frames: BLOCK,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        callback_frames: callback_frames.clone(),
        input: stereo_input.clone(),
        parameters: BTreeMap::from([
            ("gateEnabled".to_owned(), 0.0),
            ("pitchEnabled".to_owned(), 1.0),
            ("pitchSemitones".to_owned(), 0.25),
            ("pitchStep".to_owned(), 1.0),
            ("pitchTonality".to_owned(), 1.0),
            ("pitchWindow".to_owned(), 1.0 / 3.0),
        ]),
        state_changes: Vec::new(),
        snapshot: serde_json::json!({"name": "pitch comparison"}),
    };
    let controls = InputControls {
        gate_enabled: false,
        pitch_enabled: true,
        pitch: PitchParams {
            semitones: -12.0,
            window: PitchWindow::Ms30,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut stage = InputStage::new(FS, BLOCK).expect("input preparation");
    let mut left_output = Vec::with_capacity(total_frames);
    let mut right_output = Vec::with_capacity(total_frames);
    let mut frame_offset = 0;
    for frames in callback_frames {
        let mut data = Vec::with_capacity(frames * 2);
        data.extend_from_slice(&stereo_input[frame_offset..frame_offset + frames]);
        data.extend_from_slice(
            &stereo_input[total_frames + frame_offset..total_frames + frame_offset + frames],
        );
        stage
            .process(
                &mut PlanarBuffer::new(&mut data, 2, frames).expect("stereo buffer"),
                controls,
            )
            .expect("input processing");
        let (left, right) = data.split_at(frames);
        left_output.extend_from_slice(left);
        right_output.extend_from_slice(right);
        frame_offset += frames;
    }
    let mut output = left_output;
    output.extend_from_slice(&right_output);

    let reference = oracle.render(&scenario).expect("Reference pitch render");
    assert_eq!(reference.latency.samples, 768);
    assert_eq!(stage.latency_samples(controls), 768);
    expect_shifted_to(&reference.samples, 440.0, 220.0);
    expect_shifted_to(&output, 440.0, 220.0);

    let reference_power = db(goertzel_power(
        &reference.samples[48_000..48_000 + 65_536],
        220.0,
    ));
    let output_power = db(goertzel_power(&output[48_000..48_000 + 65_536], 220.0));
    assert!(
        (reference_power - output_power).abs() < 0.1,
        "Reference {reference_power} dB, Rust {output_power} dB",
    );
}
