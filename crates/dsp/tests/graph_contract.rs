//! Planar layout and typed graph contract tests.

use ronie_core::InputMode;
use ronie_dsp::{
    ChainError, PlanarBuffer, PrepareSpec,
    block::BlockControls,
    lane::{Lane, LaneSnapshot},
};

#[test]
fn planar_buffer_rejects_empty_and_overflowing_layouts_without_mutation() {
    let mut data = [0.25, 0.5, 0.75];
    let original = data;

    for (channels, frames) in [(0, 3), (1, 0), (usize::MAX, 2), (2, usize::MAX)] {
        let result = PlanarBuffer::new(&mut data, channels, frames);
        assert!(
            matches!(result, Err(ChainError::InvalidLayout(_))),
            "layout {channels}x{frames} must be rejected"
        );
        assert_eq!(data, original, "a rejected layout must not mutate audio");
    }
}

#[test]
fn planar_buffer_rejects_non_divisible_and_wrong_length_data() {
    let mut data = [1.0, 2.0, 3.0, 4.0, 5.0];
    let original = data;

    for (channels, frames) in [(2, 2), (2, 3), (1, 4)] {
        let result = PlanarBuffer::new(&mut data, channels, frames);
        assert!(matches!(result, Err(ChainError::InvalidLayout(_))));
        assert_eq!(data, original);
    }
}

#[test]
fn planar_buffer_exposes_checked_channel_views() {
    let mut data = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let mut buffer = PlanarBuffer::new(&mut data, 2, 3).expect("valid planar layout");

    assert_eq!(buffer.channels(), 2);
    assert_eq!(buffer.frames(), 3);
    assert_eq!(buffer.channel(1).expect("right channel"), &[4.0, 5.0, 6.0]);
    buffer.channel_mut(0).expect("left channel")[1] = 9.0;
    assert_eq!(buffer.channel(0).expect("left channel"), &[1.0, 9.0, 3.0]);
    assert!(matches!(
        buffer.channel(2),
        Err(ChainError::InvalidLayout(_))
    ));
}

#[test]
fn preparation_contract_uses_typed_input_routing() {
    let spec = ronie_dsp::PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::DualMono,
        stereo_input: true,
        stereo_output: false,
    };

    assert_eq!(spec.input_mode, InputMode::DualMono);
    assert!(!spec.stereo_output);
}

#[test]
fn lane_ranges_are_validated_before_slicing() {
    let spec = PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 1,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: false,
        stereo_output: false,
    };
    let mut lane = Lane::from_snapshot(
        LaneSnapshot {
            blocks: vec![],
            models: vec![],
        },
        spec,
    )
    .expect("empty lane");
    let mut data = [0.25];
    assert!(matches!(
        lane.process_range(
            &mut PlanarBuffer::new(&mut data, 1, 1).expect("audio"),
            1,
            0,
            BlockControls::default(),
        ),
        Err(ChainError::InvalidLayout(_))
    ));
    assert!(matches!(
        lane.process_range(
            &mut PlanarBuffer::new(&mut data, 1, 1).expect("audio"),
            0,
            1,
            BlockControls::default(),
        ),
        Err(ChainError::InvalidLayout(_))
    ));
    assert_eq!(data, [0.25]);
}
