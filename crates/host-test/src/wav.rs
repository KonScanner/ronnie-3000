//! The WAV files of the test: a sine to play, and the level of a render.

use eyre::{Result, WrapErr, bail, eyre};
use std::path::Path;

/// Writes a stereo 16-bit sine of `seconds` at `frequency` and `level` (0 to 1).
pub(crate) fn write_sine(
    path: &Path,
    sample_rate: u32,
    seconds: f32,
    frequency: f32,
    level: f32,
) -> Result<()> {
    let frames = (sample_rate as f32 * seconds) as u32;
    let data_size = frames * 4;
    let mut bytes = Vec::with_capacity(44 + data_size as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for frame in 0..frames {
        let phase = frame as f32 * frequency / sample_rate as f32;
        let value = (level * (phase * std::f32::consts::TAU).sin() * 32_767.0) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).wrap_err_with(|| format!("write {}", path.display()))
}

/// The samples of the first channel of a WAV file, from -1 to 1, and the sample rate.
pub(crate) fn read_first_channel(path: &Path) -> Result<(Vec<f32>, u32)> {
    let bytes = std::fs::read(path).wrap_err_with(|| format!("read {}", path.display()))?;
    parse_first_channel(&bytes).wrap_err_with(|| format!("parse {}", path.display()))
}

fn parse_first_channel(bytes: &[u8]) -> Result<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let mut format = None;
    let mut position = 12;
    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let size = u32_at(position + 4) as usize;
        let body = position + 8;
        let end = body.saturating_add(size).min(bytes.len());
        // A chunk can claim more bytes than the file has, so only the bytes that exist count.
        let available = end - body;
        if id == b"fmt " && available >= 16 {
            let mut kind = u16_at(body);
            if kind == 0xFFFE && available >= 26 {
                kind = u16_at(body + 24);
            }
            format = Some((kind, u32_at(body + 4), u16_at(body + 12), u16_at(body + 14)));
        } else if id == b"data" {
            let (kind, rate, block_align, bits) = format.ok_or_else(|| eyre!("data before fmt"))?;
            // The block align is the size of one frame, with all channels.
            let width = usize::from(bits / 8);
            let stride = usize::from(block_align);
            if width == 0 || stride < width {
                bail!("a frame of {stride} bytes cannot hold a sample of {bits} bits");
            }
            let samples = bytes[body..end]
                .chunks_exact(stride)
                .map(|frame| sample(kind, bits, &frame[..width]))
                .collect::<Result<_>>()?;
            return Ok((samples, rate));
        }
        position = body.saturating_add(size).saturating_add(size % 2);
    }
    bail!("no data chunk")
}

fn sample(kind: u16, bits: u16, bytes: &[u8]) -> Result<f32> {
    Ok(match (kind, bits) {
        (1, 16) => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
        (1, 24) => {
            let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
            value as f32 / 8_388_608.0
        }
        (1, 32) => {
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 / 2_147_483_648.0
        }
        (3, 32) => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        (3, 64) => {
            let mut raw = [0_u8; 8];
            raw.copy_from_slice(&bytes[..8]);
            f64::from_le_bytes(raw) as f32
        }
        _ => bail!("format {kind} with {bits} bits is not known"),
    })
}

/// The RMS level in dBFS of each window of `window_seconds`.
pub(crate) fn window_levels(samples: &[f32], rate: u32, window_seconds: f32) -> Vec<f32> {
    let window = ((rate as f32 * window_seconds) as usize).max(1);
    samples
        .chunks(window)
        .filter(|chunk| chunk.len() == window)
        .map(|chunk| {
            let power = chunk.iter().map(|value| value * value).sum::<f32>() / chunk.len() as f32;
            10.0 * power.max(1e-20).log10()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_sine_reads_back_at_its_level() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("sine.wav");
        write_sine(&path, 48_000, 0.5, 220.0, 0.5).expect("write");

        let (samples, rate) = read_first_channel(&path).expect("read");
        let levels = window_levels(&samples, rate, 0.1);

        assert_eq!(rate, 48_000);
        assert_eq!(levels.len(), 5);
        // A sine at half scale has an RMS of 0.5 / sqrt(2), which is -9.03 dBFS.
        for level in levels {
            assert!((level + 9.03).abs() < 0.05, "{level}");
        }
    }

    #[test]
    fn a_block_align_smaller_than_one_sample_is_an_error() {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        // PCM, 2 channels, 48 kHz, 192000 bytes a second, block align 1, 16 bits.
        for value in [1_u16, 2] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&192_000_u32.to_le_bytes());
        for value in [1_u16, 16] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        assert!(parse_first_channel(&bytes).is_err());
    }

    #[test]
    fn a_short_or_damaged_file_is_an_error() {
        assert!(parse_first_channel(b"RIFF").is_err());
        let mut header = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        header.extend_from_slice(&40_u32.to_le_bytes());
        header.extend_from_slice(&[1, 0, 2, 0]);
        assert!(parse_first_channel(&header).is_err());
    }

    #[test]
    fn a_float_sample_and_a_24_bit_sample_read_as_their_values() {
        assert!((sample(3, 32, &0.25_f32.to_le_bytes()).expect("float") - 0.25).abs() < 1e-6);
        let half = sample(1, 24, &[0x00, 0x00, 0x40]).expect("24 bit");
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        let negative = sample(1, 24, &[0x00, 0x00, 0xC0]).expect("24 bit");
        assert!((negative + 0.5).abs() < 1e-6, "{negative}");
        assert!(sample(2, 8, &[0]).is_err());
    }
}
