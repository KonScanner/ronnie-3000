use std::collections::{BTreeMap, BTreeSet};

/// One three-byte channel-message packet from an identified MIDI input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiEvent {
    /// Stable operating-system source identifier.
    pub source: String,
    /// Status and up to two data bytes; unused trailing bytes are zero.
    pub data: [u8; 3],
}

impl MidiEvent {
    /// Creates one packet.
    #[must_use]
    pub fn new(source: impl Into<String>, data: [u8; 3]) -> Self {
        Self {
            source: source.into(),
            data,
        }
    }

    /// Converts one complete channel message, ignoring system and variable-length messages.
    #[must_use]
    pub fn from_message(source: impl Into<String>, message: &[u8]) -> Option<Self> {
        if message.is_empty() || message.len() > 3 || !(0x80..0xF0).contains(&message[0]) {
            return None;
        }
        let mut data = [0; 3];
        data[..message.len()].copy_from_slice(message);
        Some(Self::new(source, data))
    }
}

/// Enablement filter and deterministic merge point for all MIDI inputs.
#[derive(Debug, Default)]
pub struct MidiRouter {
    enabled: BTreeSet<String>,
    events: Vec<MidiEvent>,
}

impl MidiRouter {
    /// Creates an empty router with every source disabled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables or disables one source.
    pub fn set_enabled(&mut self, source: impl Into<String>, enabled: bool) {
        let source = source.into();
        if enabled {
            self.enabled.insert(source);
        } else {
            self.enabled.remove(&source);
        }
    }

    /// Accepts a packet only when its source is enabled.
    pub fn push(&mut self, event: MidiEvent) {
        if self.enabled.contains(&event.source) {
            self.events.push(event);
        }
    }

    /// Takes all merged packets in arrival order.
    #[must_use]
    pub fn drain(&mut self) -> Vec<MidiEvent> {
        std::mem::take(&mut self.events)
    }

    /// Replaces the complete enabled-source set without retaining stale inputs.
    pub fn retain_enabled(&mut self, sources: &BTreeMap<String, bool>) {
        self.enabled = sources
            .iter()
            .filter(|(_, enabled)| **enabled)
            .map(|(source, _)| source.clone())
            .collect();
    }
}

use std::sync::mpsc::{Receiver, SyncSender};

use midir::{MidiInput, MidiInputConnection};

use crate::audio::AudioError;

/// Discoverable MIDI input port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiPortDescriptor {
    /// Stable operating-system port identifier.
    pub id: String,
    /// Human-readable port name.
    pub name: String,
}

/// Real MIDI connections feeding one bounded control-thread queue.
pub struct MidiInputManager {
    sender: SyncSender<MidiEvent>,
    receiver: Receiver<MidiEvent>,
    connections: Vec<MidiInputConnection<()>>,
}

impl MidiInputManager {
    /// Creates a manager with a fixed event capacity.
    ///
    /// # Errors
    /// Returns [`AudioError::InvalidConfig`] for zero capacity.
    pub fn new(capacity: usize) -> Result<Self, AudioError> {
        if capacity == 0 {
            return Err(AudioError::InvalidConfig(
                "MIDI queue capacity must be nonzero".to_owned(),
            ));
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(capacity);
        Ok(Self {
            sender,
            receiver,
            connections: Vec::new(),
        })
    }

    /// Enumerates currently available MIDI inputs without connecting them.
    ///
    /// # Errors
    /// Returns an operating-system MIDI initialization or port-query error.
    pub fn ports() -> Result<Vec<MidiPortDescriptor>, AudioError> {
        let input = MidiInput::new("Ronnie MIDI discovery")
            .map_err(|error| AudioError::Backend(error.to_string()))?;
        input
            .ports()
            .into_iter()
            .map(|port| {
                let id = port.id();
                let name = input
                    .port_name(&port)
                    .map_err(|error| AudioError::Backend(error.to_string()))?;
                Ok(MidiPortDescriptor { id, name })
            })
            .collect()
    }

    /// Replaces all connections with the enabled subset of currently available ports.
    ///
    /// # Errors
    /// Returns an error when an enabled port disappeared or cannot be connected. No partial
    /// connection set is retained after an error.
    pub fn connect_enabled(&mut self, enabled: &BTreeMap<String, bool>) -> Result<(), AudioError> {
        self.connections.clear();
        let result = (|| {
            for (source, is_enabled) in enabled {
                if !is_enabled {
                    continue;
                }
                let input = MidiInput::new("Ronnie MIDI input")
                    .map_err(|error| AudioError::Backend(error.to_string()))?;
                let port = input
                    .find_port_by_id(source)
                    .ok_or(AudioError::Disconnected)?;
                let sender = self.sender.clone();
                let source = source.clone();
                let connection = input
                    .connect(
                        &port,
                        "Ronnie MIDI connection",
                        move |_, message, _| {
                            if let Some(event) = MidiEvent::from_message(source.clone(), message) {
                                let _ = sender.try_send(event);
                            }
                        },
                        (),
                    )
                    .map_err(|error| AudioError::Backend(error.to_string()))?;
                self.connections.push(connection);
            }
            Ok(())
        })();
        if result.is_err() {
            self.connections.clear();
        }
        result
    }

    /// Drains queued hardware packets through the enabled-source router.
    pub fn drain_into(&self, router: &mut MidiRouter) {
        while let Ok(event) = self.receiver.try_recv() {
            router.push(event);
        }
    }
}
