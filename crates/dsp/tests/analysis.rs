//! Tuner and auto-balance analysis workflow tests.

use ronie_dsp::analysis::{AutoBalance, AutoBalanceState, TunerDetector};

const FS: f64 = 48_000.0;

fn sine(frequency: f64, seconds: f64, amplitude: f32, sample_rate: f64) -> Vec<f32> {
    let frames = (sample_rate * seconds) as usize;
    (0..frames)
        .map(|index| {
            amplitude
                * (2.0 * std::f64::consts::PI * frequency * index as f64 / sample_rate).sin() as f32
        })
        .collect()
}

fn weak_fundamental(frequency: f64) -> Vec<f32> {
    let amplitudes = [0.08, 0.22, 0.12, 0.06];
    (0..48_000)
        .map(|index| {
            amplitudes
                .iter()
                .enumerate()
                .map(|(harmonic, amplitude)| {
                    amplitude
                        * (2.0
                            * std::f64::consts::PI
                            * frequency
                            * (harmonic + 1) as f64
                            * index as f64
                            / FS)
                            .sin() as f32
                })
                .sum()
        })
        .collect()
}

fn reading(tuner: &mut TunerDetector, samples: &[f32]) -> ronie_dsp::analysis::TunerReading {
    tuner.push_samples(samples);
    tuner.get_reading(30)
}

#[test]
fn detects_guitar_and_bass_fundamentals() {
    for frequency in [82.41, 110.0, 329.63, 30.87, 41.2, 55.0, 98.0] {
        let mut tuner = TunerDetector::new(FS).expect("tuner preparation");
        tuner.set_enabled(true);
        let reading = reading(&mut tuner, &sine(frequency, 1.0, 0.25, FS));
        assert!(
            (reading.frequency_hz - frequency).abs() < frequency * 0.005,
            "{frequency}"
        );
        assert!(reading.confidence > 0.5);
        assert!(reading.in_tune(frequency));
    }
}

#[test]
fn weak_fundamental_avoids_octave_error() {
    let frequency = 41.2;
    let mut tuner = TunerDetector::new(FS).expect("tuner preparation");
    tuner.set_enabled(true);
    let reading = reading(&mut tuner, &weak_fundamental(frequency));
    assert!((reading.frequency_hz - frequency).abs() < frequency * 0.005);
    assert!(reading.confidence > 0.5);
}

#[test]
fn high_sample_rate_keeps_low_bass_reachable() {
    let frequency = 41.2;
    let mut tuner = TunerDetector::new(192_000.0).expect("tuner preparation");
    tuner.set_enabled(true);
    let reading = reading(&mut tuner, &sine(frequency, 1.0, 0.25, 192_000.0));
    assert!((reading.frequency_hz - frequency).abs() < frequency * 0.005);
    assert!(reading.confidence > 0.5);
}

#[test]
fn silence_noise_and_disabled_input_produce_no_pitch() {
    let mut tuner = TunerDetector::new(FS).expect("tuner preparation");
    tuner.set_enabled(true);
    let silent = reading(&mut tuner, &vec![0.0; 48_000]);
    assert_eq!(silent.frequency_hz, 0.0);
    assert_eq!(silent.confidence, 0.0);

    let noise = (0..48_000)
        .map(|index| ((index as f32 * 0.73) % 100.0) / 125.0 - 0.4)
        .collect::<Vec<_>>();
    let noisy = reading(&mut tuner, &noise);
    assert_eq!(noisy.frequency_hz, 0.0);

    tuner.set_enabled(false);
    let before = tuner.get_reading(60);
    tuner.push_samples(&sine(110.0, 1.0, 0.25, FS));
    let after = tuner.get_reading(90);
    assert_eq!(after.frequency_hz, before.frequency_hz);
    assert_eq!(after.level_db, before.level_db);
}

#[test]
fn disabled_tuner_does_not_analyze_buffered_audio() {
    let mut tuner = TunerDetector::new(FS).expect("tuner preparation");
    tuner.set_enabled(true);
    tuner.push_samples(&sine(110.0, 1.0, 0.25, FS));
    tuner.set_enabled(false);
    let reading = tuner.get_reading(100);
    assert_eq!(reading.frequency_hz, 0.0);
    assert_eq!(reading.confidence, 0.0);
    assert_eq!(reading.level_db, -120.0);
}

#[test]
fn cents_display_uses_a_five_cent_window() {
    let mut tuner = TunerDetector::new(FS).expect("tuner preparation");
    tuner.set_enabled(true);
    let reading = reading(&mut tuner, &sine(110.0, 1.0, 0.25, FS));
    assert!(reading.in_tune(110.0));
    assert!(!reading.in_tune(111.0));
    assert!(reading.cents_off(110.0).abs() < 5.0);
}

#[test]
fn auto_balance_measures_once_and_maps_the_trim() {
    let mut balance = AutoBalance::new(FS);
    balance.start();
    let left = vec![0.5; 123_000];
    let right = vec![0.25; 123_000];
    balance.process(&left, &right);
    assert_eq!(balance.state(), AutoBalanceState::Done);
    let result = balance.poll();
    assert_eq!(result.state, AutoBalanceState::Done);
    assert!((result.matched_db.unwrap() - 6.02).abs() < 0.1);
    assert!((result.balance() - 0.5 - 6.02 / 48.0).abs() < 0.005);
    assert_eq!(balance.state(), AutoBalanceState::Idle);
    assert_eq!(balance.poll().state, AutoBalanceState::Idle);
}

#[test]
fn auto_balance_gate_timeout_and_cancel_do_no_work_when_disabled() {
    let mut balance = AutoBalance::new(FS);
    let silent = vec![0.0; 48_000];
    balance.process(&silent, &silent);
    assert_eq!(balance.state(), AutoBalanceState::Idle);

    balance.start();
    balance.process(&silent, &silent);
    assert_eq!(balance.state(), AutoBalanceState::Listening);
    assert_eq!(balance.progress(), 0.0);
    balance.cancel();
    assert_eq!(balance.state(), AutoBalanceState::Idle);
    balance.process(&vec![0.5; 48_000], &vec![0.25; 48_000]);
    assert_eq!(balance.state(), AutoBalanceState::Idle);

    balance.start();
    balance.process(&silent, &silent);
    assert_eq!(balance.state(), AutoBalanceState::Listening);
    balance.process(&silent, &silent);
    assert_eq!(balance.state(), AutoBalanceState::Listening);
    for _ in 0..14 {
        balance.process(&silent, &silent);
    }
    assert_eq!(balance.state(), AutoBalanceState::Timeout);
    assert_eq!(balance.poll().state, AutoBalanceState::Timeout);
    assert_eq!(balance.state(), AutoBalanceState::Idle);
}

#[test]
fn auto_balance_gate_excludes_quiet_blocks() {
    let mut balance = AutoBalance::new(FS);
    balance.start();
    let quiet = vec![0.001; 48_000];
    balance.process(&quiet, &quiet);
    assert_eq!(balance.progress(), 0.0);
    let loud = vec![0.5; 48_000];
    balance.process(&loud, &loud);
    assert!(balance.progress() > 0.0);
}
