use ronie_core::{NamArchitecture, NamDocument, NamMetadata, VersionSupport};
use serde_json::{Value, json};
use thiserror::Error;

const KERNEL_SIZES: [usize; 23] = [
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 15, 15, 6, 6, 6, 6, 6, 6, 6,
];
const DILATIONS: [usize; 23] = [
    1, 3, 7, 17, 41, 101, 239, 1, 3, 7, 17, 41, 101, 239, 1, 13, 1, 3, 7, 17, 41, 101, 239,
];

/// Error returned when a synthetic fixture cannot be generated.
#[derive(Debug, Error)]
pub enum SyntheticError {
    /// A2 fixtures support only three or eight channels.
    #[error("synthetic A2 channels must be 3 or 8, got {0}")]
    UnsupportedChannels(usize),
    /// JSON serialization failed.
    #[error("could not serialize synthetic fixture: {0}")]
    Json(#[from] serde_json::Error),
}

/// Generates a deterministic public A2 document with distinct finite weights.
///
/// # Errors
///
/// Returns [`SyntheticError::UnsupportedChannels`] for counts other than 3 or 8.
pub fn synthetic_a2_document(channels: usize) -> Result<NamDocument, SyntheticError> {
    if channels != 3 && channels != 8 {
        return Err(SyntheticError::UnsupportedChannels(channels));
    }

    let weight_count = 179 * channels * channels + 86 * channels + 2;
    let weights = (0..weight_count)
        .map(|index| ((index + 1) as f64 * 1e-8) as f32)
        .collect();
    let activations = vec![json!({"type": "LeakyReLU", "negative_slope": 0.01}); 23];
    let film = || json!({"active": false});

    Ok(NamDocument {
        version: "0.7.0".to_owned(),
        support: VersionSupport::Yes,
        architecture: NamArchitecture::WaveNet,
        sample_rate: Some(48_000.0),
        metadata: NamMetadata::default(),
        config: json!({
            "layers": [{
                "input_size": 1,
                "condition_size": 1,
                "head": {"out_channels": 1, "kernel_size": 16, "bias": true},
                "channels": channels,
                "bottleneck": channels,
                "kernel_sizes": KERNEL_SIZES,
                "dilations": DILATIONS,
                "activation": activations,
                "head1x1": {"active": false},
                "layer1x1": {"active": true, "groups": 1},
                "groups_input": 1,
                "groups_input_mixin": 1,
                "gating_mode": vec!["none"; 23],
                "secondary_activation": vec![Value::Null; 23],
                "conv_pre_film": film(),
                "conv_post_film": film(),
                "input_mixin_pre_film": film(),
                "input_mixin_post_film": film(),
                "activation_pre_film": film(),
                "activation_post_film": film(),
                "layer1x1_post_film": film(),
                "head1x1_post_film": film()
            }],
            "head": null,
            "head_scale": 1.0,
            "in_channels": 1
        }),
        weights,
    })
}

/// Serializes a deterministic public A2 document as NAM JSON.
///
/// # Errors
///
/// Returns channel validation or JSON serialization errors.
pub fn synthetic_a2_json(channels: usize) -> Result<Vec<u8>, SyntheticError> {
    let document = synthetic_a2_document(channels)?;
    let value = json!({
        "version": document.version,
        "architecture": "WaveNet",
        "config": document.config,
        "weights": document.weights,
        "sample_rate": document.sample_rate,
        "metadata": {}
    });
    Ok(serde_json::to_vec_pretty(&value)?)
}
