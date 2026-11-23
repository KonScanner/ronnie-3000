use serde_json::{Map, Value};

use super::{A2Channels, DILATIONS, HEAD_KERNEL_SIZE, KERNEL_SIZES, NUM_LAYERS};

const FILM_KEYS: [&str; 8] = [
    "conv_pre_film",
    "conv_post_film",
    "input_mixin_pre_film",
    "input_mixin_post_film",
    "activation_pre_film",
    "activation_post_film",
    "layer1x1_post_film",
    "head1x1_post_film",
];

/// Returns the A2 specialization when every reference shape invariant matches.
#[must_use]
pub fn detect_a2(config: &Value) -> Option<A2Channels> {
    let config = config.as_object()?;
    let layers = config.get("layers")?.as_array()?;
    if layers.len() != 1 {
        return None;
    }
    if config.get("head").is_some_and(|value| !value.is_null())
        || config
            .get("condition_dsp")
            .is_some_and(|value| !value.is_null())
        || !config.get("head_scale").is_some_and(Value::is_number)
        || integer_field(config, "in_channels", 1)? != 1
    {
        return None;
    }

    let layer = layers.first()?.as_object()?;
    if integer_field(layer, "input_size", 0)? != 1
        || integer_field(layer, "condition_size", 0)? != 1
    {
        return None;
    }

    let channels = integer_field(layer, "channels", 0)?;
    if channels != integer_field(layer, "bottleneck", 0)? {
        return None;
    }
    let channels = match channels {
        3 => A2Channels::Nano,
        8 => A2Channels::Standard,
        _ => return None,
    };

    if !matches_integer_array(layer.get("kernel_sizes"), &KERNEL_SIZES)
        || !matches_integer_array(layer.get("dilations"), &DILATIONS)
        || !valid_activations(layer.get("activation"))
        || !valid_gating(layer)
        || !valid_secondary_activation(layer.get("secondary_activation"))
        || !valid_head1x1(layer.get("head1x1"))
        || !valid_layer1x1(layer)
        || !valid_head(layer)
        || !FILM_KEYS.iter().all(|key| film_inactive(layer.get(*key)))
        || integer_field(layer, "groups_input", 1)? != 1
        || integer_field(layer, "groups_input_mixin", 1)? != 1
        || layer.get("slimmable").is_some_and(|value| !value.is_null())
    {
        return None;
    }

    Some(channels)
}

fn integer_field(value: &Map<String, Value>, key: &str, default: i64) -> Option<i64> {
    match value.get(key) {
        Some(value) => value.as_i64(),
        None => Some(default),
    }
}

fn bool_field(value: &Map<String, Value>, key: &str, default: bool) -> Option<bool> {
    match value.get(key) {
        Some(value) => value.as_bool(),
        None => Some(default),
    }
}

fn matches_integer_array(value: Option<&Value>, expected: &[usize; NUM_LAYERS]) -> bool {
    value.and_then(Value::as_array).is_some_and(|values| {
        values.len() == NUM_LAYERS
            && values
                .iter()
                .zip(expected)
                .all(|(value, expected)| value.as_i64() == Some(*expected as i64))
    })
}

fn valid_activations(value: Option<&Value>) -> bool {
    value.and_then(Value::as_array).is_some_and(|values| {
        values.len() == NUM_LAYERS
            && values.iter().all(|value| {
                value.as_object().is_some_and(|activation| {
                    activation.get("type").and_then(Value::as_str) == Some("LeakyReLU")
                        && activation
                            .get("negative_slope")
                            .and_then(Value::as_f64)
                            .is_some_and(|slope| ((slope as f32) - 0.01).abs() <= 1e-7)
                })
            })
    })
}

fn valid_gating(layer: &Map<String, Value>) -> bool {
    let valid_modes = match layer.get("gating_mode") {
        None | Some(Value::Null) => true,
        Some(value) => value.as_array().is_some_and(|values| {
            values.len() == NUM_LAYERS && values.iter().all(|value| value.as_str() == Some("none"))
        }),
    };
    valid_modes && layer.get("gated").and_then(Value::as_bool) != Some(true)
}

fn valid_secondary_activation(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(value) => value
            .as_array()
            .is_some_and(|values| values.len() == NUM_LAYERS && values.iter().all(Value::is_null)),
    }
}

fn valid_head1x1(value: Option<&Value>) -> bool {
    match value.and_then(Value::as_object) {
        Some(head) => bool_field(head, "active", false) == Some(false),
        None => true,
    }
}

fn valid_layer1x1(layer: &Map<String, Value>) -> bool {
    layer
        .get("layer1x1")
        .and_then(Value::as_object)
        .is_some_and(|value| {
            bool_field(value, "active", false) == Some(true)
                && integer_field(value, "groups", 1) == Some(1)
        })
}

fn valid_head(layer: &Map<String, Value>) -> bool {
    layer
        .get("head")
        .and_then(Value::as_object)
        .is_some_and(|head| {
            integer_field(head, "out_channels", 0) == Some(1)
                && integer_field(head, "kernel_size", 0) == Some(HEAD_KERNEL_SIZE as i64)
                && integer_field(head, "head_dilation", 1) == Some(1)
                && bool_field(head, "bias", false) == Some(true)
        })
}

fn film_inactive(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(active)) => !active,
        Some(Value::Object(object)) => match object.get("active") {
            None => true,
            Some(active) => active.as_bool() == Some(false),
        },
        _ => false,
    }
}
