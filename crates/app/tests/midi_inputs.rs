//! MIDI packet filtering and connection-queue contract tests.

use ronie_app::{
    audio::AudioError,
    midi::{MidiEvent, MidiInputManager},
};

#[test]
fn channel_messages_become_packets_while_sysex_and_realtime_are_ignored() {
    assert_eq!(
        MidiEvent::from_message("keyboard", &[0x90, 60, 100]),
        Some(MidiEvent::new("keyboard", [0x90, 60, 100]))
    );
    assert_eq!(
        MidiEvent::from_message("keyboard", &[0xC0, 12]),
        Some(MidiEvent::new("keyboard", [0xC0, 12, 0]))
    );
    assert_eq!(MidiEvent::from_message("keyboard", &[0xF8]), None);
    assert_eq!(
        MidiEvent::from_message("keyboard", &[0xF0, 0x7E, 0xF7]),
        None
    );
    assert_eq!(
        MidiEvent::from_message("keyboard", &[0x90, 60, 100, 0]),
        None
    );
}

#[test]
fn midi_queue_rejects_a_zero_capacity() {
    assert!(matches!(
        MidiInputManager::new(0),
        Err(AudioError::InvalidConfig(_))
    ));
    assert!(MidiInputManager::new(64).is_ok());
}
