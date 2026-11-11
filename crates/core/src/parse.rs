use serde::Deserialize;
use serde_json::Value;

use crate::{
    error::{NamError, NamResult},
    model::{NamArchitecture, NamDocument, NamMetadata, VersionSupport},
};

const MAX_INPUT_BYTES: usize = 50 * 1024 * 1024;
const MAX_NESTING_DEPTH: usize = 64;

#[derive(Debug, Deserialize)]
struct RawNamDocument {
    version: String,
    architecture: String,
    config: Value,
    weights: Vec<f32>,
    #[serde(default, deserialize_with = "deserialize_sample_rate")]
    sample_rate: Option<f64>,
    #[serde(default)]
    metadata: Option<NamMetadata>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

/// Parses and validates a bounded NAM JSON document.
///
/// Input is limited to 50 MiB and JSON nesting is limited to depth 64.
///
/// # Errors
///
/// Returns a typed error for malformed JSON, invalid document fields,
/// unsupported architectures or versions, and exceeded input limits.
pub fn parse_nam(bytes: &[u8]) -> NamResult<NamDocument> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(NamError::InputTooLarge);
    }
    enforce_nesting_depth(bytes)?;

    let raw: RawNamDocument = serde_json::from_slice(bytes)?;
    let support = version_support(&raw.version)?;
    let architecture = match raw.architecture.as_str() {
        "WaveNet" => NamArchitecture::WaveNet,
        "SlimmableContainer" => NamArchitecture::SlimmableContainer,
        other => return Err(NamError::UnsupportedArchitecture(other.to_owned())),
    };

    Ok(NamDocument {
        version: raw.version,
        support,
        architecture,
        sample_rate: raw.sample_rate,
        metadata: raw.metadata.unwrap_or_default(),
        config: raw.config,
        weights: raw.weights,
    })
}

fn deserialize_sample_rate<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    f64::deserialize(deserializer).map(Some)
}

fn version_support(text: &str) -> NamResult<VersionSupport> {
    let parsed = parse_version(text)?;
    let earliest = Version {
        major: 0,
        minor: 5,
        patch: 0,
    };
    let latest = Version {
        major: 0,
        minor: 7,
        patch: 0,
    };

    if parsed < earliest || parsed.major > latest.major || parsed.minor > latest.minor {
        return Err(NamError::UnsupportedVersion(text.to_owned()));
    }
    if parsed > latest {
        return Ok(VersionSupport::Partial);
    }
    Ok(VersionSupport::Yes)
}

fn parse_version(text: &str) -> NamResult<Version> {
    let mut parts = text.split('.');
    let invalid = || NamError::InvalidVersion(text.to_owned());
    let (Some(major), Some(minor), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };

    Ok(Version {
        major: parse_version_component(major).ok_or_else(&invalid)?,
        minor: parse_version_component(minor).ok_or_else(&invalid)?,
        patch: parse_version_component(patch).ok_or_else(&invalid)?,
    })
}

fn parse_version_component(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn enforce_nesting_depth(bytes: &[u8]) -> NamResult<()> {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;

    for &byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else {
                match byte {
                    b'\\' => escaped = true,
                    b'"' => in_string = false,
                    _ => {}
                }
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_NESTING_DEPTH {
                    return Err(NamError::NestingTooDeep);
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}
