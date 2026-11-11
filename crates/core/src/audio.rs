use crate::AudioError;

const MAX_CHANNELS: usize = 64;
const MAX_WAV_BYTES: usize = 50 * 1024 * 1024;

/// Decoded planar audio. Every channel has the same nonzero frame count.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDocument {
    /// Source sample rate in hertz.
    pub sample_rate: f64,
    /// Planar `f32` samples indexed by channel, then frame.
    pub channels: Vec<Vec<f32>>,
}

/// Decodes the PCM, IEEE-float, extensible, A-law, and mu-law WAV forms used by Reference.
///
/// Chunk sizes, frame sizes, channel counts, and sample counts are checked before
/// any sample allocation. A final odd-sized chunk does not require a pad byte.
///
/// # Errors
///
/// Returns [`AudioError`] for malformed framing, unsupported encodings,
/// overflowing sizes, inconsistent data, or empty audio.
pub fn decode_wav(bytes: &[u8]) -> Result<AudioDocument, AudioError> {
    if bytes.len() > MAX_WAV_BYTES {
        return Err(AudioError::InputTooLarge);
    }
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AudioError::InvalidHeader);
    }

    let mut format = None;
    let mut data = None;
    let mut offset = 12_usize;
    while offset < bytes.len() {
        if bytes.len() - offset < 8 {
            return Err(AudioError::Truncated);
        }
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .map_err(|_| AudioError::Truncated)?,
        ) as usize;
        let start = offset.checked_add(8).ok_or(AudioError::SizeOverflow)?;
        let end = start.checked_add(size).ok_or(AudioError::SizeOverflow)?;
        if end > bytes.len() {
            return Err(AudioError::Truncated);
        }
        match id {
            b"fmt " => format = Some(parse_format(&bytes[start..end])?),
            b"data" if data.is_none() => data = Some((start, end)),
            _ => {}
        }
        let padded = end.checked_add(size % 2).ok_or(AudioError::SizeOverflow)?;
        if padded > bytes.len() {
            if end == bytes.len() {
                break;
            }
            return Err(AudioError::Truncated);
        }
        offset = padded;
    }

    let format = format.ok_or(AudioError::MissingFormat)?;
    let (data_start, data_end) = data.ok_or(AudioError::EmptyAudio)?;
    let data = &bytes[data_start..data_end];
    if data.is_empty() {
        return Err(AudioError::EmptyAudio);
    }
    if !data.len().is_multiple_of(format.block_align) {
        return Err(AudioError::InvalidDataSize);
    }
    let frames = data.len() / format.block_align;
    if frames == 0 {
        return Err(AudioError::EmptyAudio);
    }
    let sample_count = frames
        .checked_mul(format.channels)
        .ok_or(AudioError::SizeOverflow)?;
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(sample_count)
        .map_err(|_| AudioError::SizeOverflow)?;
    samples.resize(sample_count, 0.0_f32);
    let sample_bytes = format.bytes_per_sample;
    for frame in 0..frames {
        let frame_start = frame * format.block_align;
        for channel in 0..format.channels {
            let start = frame_start + channel * sample_bytes;
            let end = start + sample_bytes;
            samples[channel * frames + frame] = format.decode(&data[start..end]);
        }
    }

    let mut channels = Vec::new();
    channels
        .try_reserve_exact(format.channels)
        .map_err(|_| AudioError::SizeOverflow)?;
    for channel in 0..format.channels {
        channels.push(samples[channel * frames..(channel + 1) * frames].to_vec());
    }
    Ok(AudioDocument {
        sample_rate: format.sample_rate,
        channels,
    })
}

#[derive(Clone, Copy)]
struct WavFormat {
    encoding: WavEncoding,
    channels: usize,
    sample_rate: f64,
    block_align: usize,
    bytes_per_sample: usize,
}

impl WavFormat {
    fn decode(self, bytes: &[u8]) -> f32 {
        match self.encoding {
            WavEncoding::Pcm8 => (f32::from(bytes[0]) - 128.0) / 128.0,
            WavEncoding::Pcm16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
            WavEncoding::Pcm24 => {
                let raw = (i32::from(bytes[0]) << 8)
                    | (i32::from(bytes[1]) << 16)
                    | (i32::from(bytes[2]) << 24);
                (raw >> 8) as f32 / 8_388_608.0
            }
            WavEncoding::Pcm32 => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / 2_147_483_648.0
            }
            WavEncoding::Float32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            WavEncoding::Float64 => f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]) as f32,
            WavEncoding::ALaw => decode_a_law(bytes[0]),
            WavEncoding::MuLaw => decode_mu_law(bytes[0]),
        }
    }
}

#[derive(Clone, Copy)]
enum WavEncoding {
    Pcm8,
    Pcm16,
    Pcm24,
    Pcm32,
    Float32,
    Float64,
    ALaw,
    MuLaw,
}

fn parse_format(bytes: &[u8]) -> Result<WavFormat, AudioError> {
    if bytes.len() < 16 {
        return Err(AudioError::Truncated);
    }
    let tag = read_u16(bytes, 0)?;
    let channels = read_u16(bytes, 2)? as usize;
    let sample_rate = read_u32(bytes, 4)?;
    let block_align = read_u16(bytes, 12)? as usize;
    let bits = read_u16(bytes, 14)? as usize;
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AudioError::InvalidChannelCount);
    }
    if sample_rate == 0 {
        return Err(AudioError::InvalidSampleRate);
    }
    let encoding_tag = if tag == 0xfffe {
        if bytes.len() < 40 {
            return Err(AudioError::Truncated);
        }
        let guid = &bytes[24..40];
        if guid[2..] != [0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113] {
            return Err(AudioError::UnsupportedFormat(tag));
        }
        read_u16(bytes, 24)?
    } else {
        tag
    };
    let (encoding, bytes_per_sample) = match (encoding_tag, bits) {
        (1, 8) => (WavEncoding::Pcm8, 1),
        (1, 16) => (WavEncoding::Pcm16, 2),
        (1, 24) => (WavEncoding::Pcm24, 3),
        (1, 32) => (WavEncoding::Pcm32, 4),
        (3, 32) => (WavEncoding::Float32, 4),
        (3, 64) => (WavEncoding::Float64, 8),
        (6, 8) => (WavEncoding::ALaw, 1),
        (7, 8) => (WavEncoding::MuLaw, 1),
        _ => return Err(AudioError::UnsupportedFormat(encoding_tag)),
    };
    let minimum_align = channels
        .checked_mul(bytes_per_sample)
        .ok_or(AudioError::SizeOverflow)?;
    if block_align < minimum_align {
        return Err(AudioError::InvalidDataSize);
    }
    Ok(WavFormat {
        encoding,
        channels,
        sample_rate: f64::from(sample_rate),
        block_align,
        bytes_per_sample,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, AudioError> {
    let slice = bytes.get(offset..offset + 2).ok_or(AudioError::Truncated)?;
    Ok(u16::from_le_bytes(
        slice.try_into().map_err(|_| AudioError::Truncated)?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, AudioError> {
    let slice = bytes.get(offset..offset + 4).ok_or(AudioError::Truncated)?;
    Ok(u32::from_le_bytes(
        slice.try_into().map_err(|_| AudioError::Truncated)?,
    ))
}

fn decode_a_law(value: u8) -> f32 {
    let inverted = value ^ 0x55;
    let exponent = i32::from((inverted & 0x70) >> 4);
    let mantissa = i32::from(inverted & 0x0f);
    let magnitude = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    let sample = if inverted & 0x80 == 0 {
        -magnitude
    } else {
        magnitude
    };
    sample as f32 / 32_768.0
}

fn decode_mu_law(value: u8) -> f32 {
    let inverted = !value;
    let exponent = i32::from((inverted & 0x70) >> 4);
    let mantissa = i32::from(inverted & 0x0f);
    let magnitude = (((mantissa << 3) + 0x84) << exponent) - 0x84;
    let sample = if inverted & 0x80 == 0 {
        magnitude
    } else {
        -magnitude
    };
    sample as f32 / 32_768.0
}
