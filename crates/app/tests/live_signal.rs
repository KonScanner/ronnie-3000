//! What the real standalone adapter reports about the audio that runs through it.
//!
//! Each test sends a known signal through the dummy stream of [`StandaloneBackend`], and reads the
//! result through `UiBackend`, as the UI does. The mock supplies fixture values, so it proves
//! nothing here.

use ronie_app::standalone_backend::StandaloneBackend;
use ronie_core::{
    InputMode, ModelBytes, ModelId, PREFERENCES_SETTINGS, Tone,
    params::{ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;
use ronie_dsp::analysis::TunerReading;
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::{
    backend::{DeviceAction, GesturePhase, MeterSnapshot, UiAction, UiBackend},
    default::{TunerFeed, TunerStatus},
};

const RATE: f32 = 48_000.0;

struct Rig {
    backend: StandaloneBackend,
    frames: usize,
    phase: f32,
    _root: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary root");
        let processor = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: f64::from(RATE),
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }));
        let backend = StandaloneBackend::dummy(processor, &root.path().join(PREFERENCES_SETTINGS))
            .expect("dummy backend");
        let frames = backend
            .device_settings()
            .expect("device state")
            .banner
            .buffer_size as usize;
        Self {
            backend,
            frames,
            phase: 0.0,
            _root: root,
        }
    }

    /// Renders one callback of a sine that continues from the previous callback.
    fn render_sine(&mut self, hertz: f32, level: f32) -> Vec<f32> {
        let step = std::f32::consts::TAU * hertz / RATE;
        let input: Vec<f32> = (0..self.frames)
            .map(|_| {
                self.phase = (self.phase + step) % std::f32::consts::TAU;
                level * self.phase.sin()
            })
            .collect();
        self.backend.render_dummy(&input).expect("render")
    }

    fn render_seconds(&mut self, seconds: f32, hertz: f32, level: f32) -> Vec<f32> {
        let callbacks = (seconds * RATE / self.frames as f32).ceil() as usize;
        let mut last = Vec::new();
        for _ in 0..callbacks {
            last = self.render_sine(hertz, level);
        }
        last
    }

    fn meters(&self) -> MeterSnapshot {
        self.backend.meter_levels()
    }

    /// Loads a tone with real bytes into the first slot, and waits for its chain.
    fn load_tone(&self) -> ronie_core::BlockId {
        let target = self.backend.chain_state(0).chain.left[0].id.clone();
        self.backend
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target.clone(),
                tone: ir_tone(),
                model_id: ModelId(8),
                bytes: ir_bytes(),
            })
            .expect("load");
        self.backend.settle_audio().expect("settle");
        target
    }
}

fn ir_tone() -> Tone {
    Tone::from_value(serde_json::json!({
        "id": 6,
        "title": "Identity",
        "format": "ir",
        "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/8.wav"}],
    }))
    .expect("tone")
}

fn ir_bytes() -> ModelBytes {
    let samples = [16_000_i16, 0, 0, 0];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36_u32 + samples.len() as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes.into()
}

fn db(level: f32) -> f32 {
    20.0 * level.log10()
}

fn peak_of_channel(interleaved: &[f32], channel: usize) -> f32 {
    interleaved
        .as_chunks::<2>()
        .0
        .iter()
        .map(|frame| frame[channel].abs())
        .fold(0.0, f32::max)
}

fn close(actual: f32, wanted: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - wanted).abs() <= tolerance,
        "{what}: got {actual}, wanted {wanted} within {tolerance}"
    );
}

#[test]
fn the_meters_start_at_the_floor_and_report_a_known_sine() {
    let mut rig = Rig::new();
    assert_eq!(rig.meters().input_db, [-60.0, -60.0]);
    assert_eq!(rig.meters().output_db, [-60.0, -60.0]);

    let output = rig.render_seconds(0.2, 1_000.0, 0.5);

    let meters = rig.meters();
    close(meters.input_db[0], db(0.5), 0.1, "input");
    close(meters.input_db[1], db(0.5), 0.1, "a mono input fills both");
    close(
        meters.output_db[0],
        db(peak_of_channel(&output, 0)),
        0.5,
        "left output",
    );
    close(
        meters.output_db[1],
        db(peak_of_channel(&output, 1)),
        0.5,
        "right output",
    );
    assert!(meters.output_db[0] > -60.0, "the output has signal");
}

#[test]
fn the_output_meter_follows_the_output_level_and_the_input_meter_does_not() {
    let mut rig = Rig::new();
    rig.render_seconds(0.2, 1_000.0, 0.5);
    let loud = rig.meters();

    rig.backend
        .set_parameter(
            ParameterId("outputLevel"),
            ParameterValue(0.25),
            GesturePhase::End,
        )
        .expect("output level");
    rig.render_seconds(0.3, 1_000.0, 0.5);

    let quiet = rig.meters();
    close(quiet.input_db[0], loud.input_db[0], 0.1, "input");
    close(
        quiet.output_db[0],
        loud.output_db[0] - 12.0,
        1.0,
        "a quarter of the knob is 12 dB down",
    );
}

#[test]
fn a_loud_callback_between_two_frames_stays_visible() {
    let mut rig = Rig::new();
    // One frame of the UI is 33 ms, so that a frame sees only some of these callbacks.
    rig.render_sine(1_000.0, 0.5);
    rig.render_sine(1_000.0, 0.01);
    close(
        rig.meters().input_db[0],
        db(0.5),
        0.1,
        "the peak of the frame",
    );

    rig.render_seconds(0.2, 1_000.0, 0.01);
    close(rig.meters().input_db[0], db(0.01), 0.1, "the peak ends");
}

#[test]
fn silence_returns_the_meters_to_the_floor() {
    let mut rig = Rig::new();
    rig.render_seconds(0.2, 1_000.0, 0.5);
    rig.render_seconds(0.5, 1_000.0, 0.0);
    let meters = rig.meters();
    assert_eq!(meters.input_db, [-60.0, -60.0]);
    assert_eq!(meters.output_db, [-60.0, -60.0]);
}

#[test]
fn a_stereo_input_reports_each_channel() {
    let rig = Rig::new();
    rig.backend
        .dispatch(UiAction::Device(DeviceAction::SetInputChannels(vec![0, 1])))
        .expect("stereo input");
    rig.backend.settle_audio().expect("settle");
    let frames = rig.frames;
    for _ in 0..4 {
        let input: Vec<f32> = (0..frames).flat_map(|_| [0.5, 0.125]).collect();
        rig.backend.render_dummy(&input).expect("render");
    }
    let meters = rig.meters();
    close(meters.input_db[0], db(0.5), 0.1, "left");
    close(meters.input_db[1], db(0.125), 0.1, "right");
}

#[test]
fn the_callback_load_is_the_time_that_the_callback_took_over_the_time_it_covers() {
    let mut rig = Rig::new();
    assert_eq!(rig.meters().cpu, 0.0);

    let started = std::time::Instant::now();
    rig.render_sine(1_000.0, 0.5);
    let wall = started.elapsed().as_secs_f32();

    let load = rig.meters().cpu;
    assert!(load > 0.0, "a callback takes time: {load}");
    let covered = rig.frames as f32 / RATE;
    assert!(
        load * covered <= wall,
        "the load {load} says more than the {wall} s that the callback took"
    );
}

#[test]
fn a_new_stream_starts_at_the_floor() {
    let mut rig = Rig::new();
    rig.render_seconds(0.1, 1_000.0, 0.5);
    assert!(rig.meters().input_db[0] > -60.0);

    rig.backend
        .dispatch(UiAction::Device(DeviceAction::Restart))
        .expect("restart");
    rig.backend.settle_audio().expect("settle");

    let meters = rig.meters();
    assert_eq!(meters.input_db, [-60.0, -60.0]);
    assert_eq!(meters.output_db, [-60.0, -60.0]);
    assert_eq!(meters.cpu, 0.0);
}

#[test]
fn a_tone_block_reports_its_levels_and_an_open_slot_does_not() {
    let mut rig = Rig::new();
    assert!(rig.meters().blocks.is_empty());
    let block = rig.load_tone();

    rig.render_seconds(0.3, 1_000.0, 0.5);

    let meters = rig.meters();
    assert_eq!(meters.blocks.len(), 1, "{:?}", meters.blocks);
    let meter = &meters.blocks[0];
    assert_eq!(meter.block_id, block);
    close(meter.input_db, db(0.5), 3.0, "block input");
    assert!(meter.output_db > -60.0, "block output: {}", meter.output_db);

    rig.render_seconds(0.5, 1_000.0, 0.0);
    let meters = rig.meters();
    assert_eq!(
        meters.blocks[0].input_db, -60.0,
        "silence reaches the block"
    );
}

#[test]
fn nothing_invents_a_stereo_correlation() {
    let mut rig = Rig::new();
    rig.render_seconds(0.1, 1_000.0, 0.5);
    assert_eq!(
        rig.meters().correlation,
        MeterSnapshot::default().correlation
    );
}

/// What the tuner page does: it polls while audio runs, and it waits for one full window.
fn tuner_reading_of(rig: &mut Rig, hertz: f32, level: f32) -> TunerReading {
    let mut reading = rig.backend.tuner_reading();
    for _ in 0..12 {
        rig.render_seconds(0.04, hertz, level);
        std::thread::sleep(std::time::Duration::from_millis(26));
        reading = rig.backend.tuner_reading();
    }
    reading
}

#[test]
fn the_tuner_finds_a_known_pitch_in_the_raw_input() {
    // The detector loses precision as the pitch rises, because the period gets short at its
    // analysis rate. The top note of a guitar is the edge of what it reads.
    for (hertz, level, tolerance) in [
        (82.41, 0.25, 5.0),
        (110.0, 0.1, 5.0),
        (329.63, 0.25, 5.0),
        (440.0, 0.25, 5.0),
        (659.26, 0.25, 5.0),
        (1_318.5, 0.25, 15.0),
    ] {
        let mut rig = Rig::new();
        let reading = tuner_reading_of(&mut rig, hertz, level);
        assert!(
            reading.confidence > 0.5,
            "{hertz} Hz: confidence {}",
            reading.confidence
        );
        assert!(
            reading.cents_off(f64::from(hertz)).abs() < tolerance,
            "{hertz} Hz: read {} Hz",
            reading.frequency_hz
        );
    }
}

#[test]
fn the_tuner_reads_before_the_input_level_and_the_output_level() {
    let mut rig = Rig::new();
    for id in ["inputLevel", "outputLevel"] {
        rig.backend
            .set_parameter(ParameterId(id), ParameterValue(0.0), GesturePhase::End)
            .expect("level");
    }
    let reading = tuner_reading_of(&mut rig, 440.0, 0.25);
    assert!(reading.in_tune(440.0), "read {} Hz", reading.frequency_hz);
}

#[test]
fn the_tuner_page_shows_the_note_and_how_far_it_is_from_true() {
    let mut rig = Rig::new();
    let mut feed = TunerFeed::default();
    let mut now_ms = 0;
    rig.backend.tuner_reading();
    // Nineteen cents above A4. The page smooths the cents, so it needs some readings.
    for _ in 0..24 {
        rig.render_seconds(0.04, 445.0, 0.25);
        std::thread::sleep(std::time::Duration::from_millis(26));
        now_ms += 33;
        feed.update(rig.backend.tuner_reading(), now_ms);
    }
    let state = feed.state();
    assert_eq!(state.note, "A");
    assert_eq!(state.octave, 4);
    assert_eq!(state.status(), TunerStatus::Sharp);
    assert!((state.cents - 19.56).abs() < 5.0, "{} cents", state.cents);
}

#[test]
fn the_tuner_reports_no_pitch_for_silence_and_after_the_stream_stops() {
    let mut rig = Rig::new();
    assert!(tuner_reading_of(&mut rig, 440.0, 0.25).frequency_hz > 0.0);

    let reading = tuner_reading_of(&mut rig, 440.0, 0.0);
    assert_eq!(reading.frequency_hz, 0.0);
    assert_eq!(reading.confidence, 0.0);

    assert!(tuner_reading_of(&mut rig, 440.0, 0.25).frequency_hz > 0.0);
    // The page polls and no callback runs, so that a held window must not read as a note.
    let mut reading = rig.backend.tuner_reading();
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        reading = rig.backend.tuner_reading();
    }
    assert_eq!(reading.frequency_hz, 0.0);
    assert_eq!(reading.confidence, 0.0);
}

#[test]
fn a_tuner_that_opens_again_does_not_show_the_note_from_before() {
    let mut rig = Rig::new();
    assert!(tuner_reading_of(&mut rig, 440.0, 0.25).frequency_hz > 0.0);

    // The page is closed, so that nothing polls, and the player stops.
    std::thread::sleep(std::time::Duration::from_millis(450));
    rig.render_seconds(0.3, 440.0, 0.0);

    let reading = rig.backend.tuner_reading();
    assert_eq!(reading.frequency_hz, 0.0, "the page opens with no note");
}

#[test]
fn the_tuner_works_at_another_stream_rate_and_after_a_new_stream() {
    let mut rig = Rig::new();
    assert!(tuner_reading_of(&mut rig, 440.0, 0.25).frequency_hz > 0.0);
    rig.backend
        .dispatch(UiAction::Device(DeviceAction::Configure(
            ronie_ui::backend::AudioDeviceConfig {
                sample_rate: Some(96_000.0),
                buffer_frames: Some(256),
                ..Default::default()
            },
        )))
        .expect("configure");
    rig.backend.settle_audio().expect("settle");
    // The helper counts frames at 48 kHz, so that the same samples are one octave up at 96 kHz.
    let reading = tuner_reading_of(&mut rig, 440.0, 0.25);
    assert!(reading.in_tune(880.0), "read {} Hz", reading.frequency_hz);
}

#[test]
fn each_change_of_a_knob_gesture_reaches_the_audio_at_once() {
    let mut rig = Rig::new();
    let id = ParameterId("outputLevel");
    let start = rig.backend.parameter(id).expect("known").0;
    rig.render_seconds(0.2, 1_000.0, 0.5);
    let loud = rig.meters().output_db[0];

    rig.backend
        .set_parameter(id, ParameterValue(start), GesturePhase::Begin)
        .expect("begin");
    rig.backend
        .set_parameter(id, ParameterValue(0.25), GesturePhase::Change)
        .expect("change");
    rig.render_seconds(0.3, 1_000.0, 0.5);

    assert!(
        rig.meters().output_db[0] < loud - 6.0,
        "the audio follows the knob before the gesture ends"
    );
}

/// What the EQ page does: it polls while audio runs, and it waits for one full window.
fn spectrum_of(rig: &mut Rig, block: &ronie_core::BlockId, hertz: f32, level: f32) -> [f32; 64] {
    spectrum_until(rig, block, hertz, level, |_| true)
}

/// Polls as [`spectrum_of`] does, and then polls on until `ready` holds. A busy test machine can
/// render slower than real time, so the first window can come later. The polls stop after
/// 10 seconds, and the caller then fails on the bins.
fn spectrum_until(
    rig: &mut Rig,
    block: &ronie_core::BlockId,
    hertz: f32,
    level: f32,
    ready: impl Fn(&[f32; 64]) -> bool,
) -> [f32; 64] {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut bins = rig.backend.spectrum_bins(block);
    for poll in 1.. {
        rig.render_seconds(0.04, hertz, level);
        std::thread::sleep(std::time::Duration::from_millis(31));
        bins = rig.backend.spectrum_bins(block);
        if poll >= 16 && (ready(&bins) || std::time::Instant::now() >= deadline) {
            break;
        }
    }
    bins
}

/// Whether the loudest bin is within a fifth of an octave of `hertz`, and loud.
fn shows_sine(bins: &[f32; 64], hertz: f32) -> bool {
    (loudest_hertz(bins) / f64::from(hertz)).log2().abs() < 0.2
        && bins.iter().copied().fold(-100.0, f32::max) > -40.0
}

fn loudest_hertz(bins: &[f32; 64]) -> f64 {
    let index = bins
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .expect("bins")
        .0;
    20.0 * (1000.0_f64.ln() * (index as f64 + 0.5) / 64.0).exp()
}

#[test]
fn the_eq_spectrum_of_a_block_finds_a_known_sine() {
    let mut rig = Rig::new();
    let block = rig.load_tone();
    assert_eq!(rig.backend.spectrum_bins(&block), [-100.0; 64]);

    for hertz in [250.0_f32, 1_000.0, 4_000.0] {
        let bins = spectrum_until(&mut rig, &block, hertz, 0.5, |bins| shows_sine(bins, hertz));
        let found = loudest_hertz(&bins);
        assert!(
            (found / f64::from(hertz)).log2().abs() < 0.2,
            "{hertz} Hz reads as {found} Hz: {bins:?}"
        );
        assert!(
            bins.iter().copied().fold(-100.0, f32::max) > -40.0,
            "{bins:?}"
        );
    }
}

#[test]
fn the_eq_spectrum_falls_to_the_floor_for_silence_and_when_the_audio_stops() {
    let mut rig = Rig::new();
    let block = rig.load_tone();
    let bins = spectrum_until(&mut rig, &block, 1_000.0, 0.5, |bins| bins[40] > -100.0);
    assert!(bins[40] > -100.0, "{bins:?}");

    let bins = spectrum_until(&mut rig, &block, 1_000.0, 0.0, |bins| {
        bins.iter().all(|bin| *bin < -60.0)
    });
    assert!(bins.iter().all(|bin| *bin < -60.0), "{bins:?}");

    spectrum_of(&mut rig, &block, 1_000.0, 0.5);
    // No callback runs from here, so that an old window must not stay on the screen.
    let mut bins = rig.backend.spectrum_bins(&block);
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(31));
        bins = rig.backend.spectrum_bins(&block);
    }
    assert!(bins.iter().all(|bin| *bin < -60.0), "{bins:?}");
}

#[test]
fn the_eq_spectrum_follows_the_block_to_a_new_chain_and_a_new_stream() {
    let mut rig = Rig::new();
    let block = rig.load_tone();
    spectrum_of(&mut rig, &block, 1_000.0, 0.5);

    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("a new chain");
    let bins = spectrum_until(&mut rig, &block, 1_000.0, 0.5, |bins| {
        shows_sine(bins, 1_000.0)
    });
    assert!(loudest_hertz(&bins) > 700.0 && loudest_hertz(&bins) < 1_400.0);

    rig.backend
        .dispatch(UiAction::Device(DeviceAction::Restart))
        .expect("a new stream");
    rig.backend.settle_audio().expect("settle");
    let bins = spectrum_until(&mut rig, &block, 1_000.0, 0.5, |bins| {
        shows_sine(bins, 1_000.0)
    });
    assert!(loudest_hertz(&bins) > 700.0 && loudest_hertz(&bins) < 1_400.0);
}

#[test]
fn an_open_slot_has_no_spectrum() {
    let mut rig = Rig::new();
    let slot = rig.backend.chain_state(0).chain.left[0].id.clone();
    assert_eq!(spectrum_of(&mut rig, &slot, 1_000.0, 0.5), [-100.0; 64]);
}

#[test]
fn a_reader_at_the_rate_of_a_hidden_window_keeps_the_tuner_window() {
    let mut rig = Rig::new();
    assert!(tuner_reading_of(&mut rig, 440.0, 0.25).frequency_hz > 0.0);

    // A hidden window reads at 4 Hz. Audio continues, so that the note must stay.
    let mut reading = rig.backend.tuner_reading();
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        rig.render_seconds(0.2, 440.0, 0.25);
        reading = rig.backend.tuner_reading();
    }
    assert!(reading.in_tune(440.0), "read {} Hz", reading.frequency_hz);
}
