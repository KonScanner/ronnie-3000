//! A2 shape and weight compilation tests.

use ronie_core::{DspError, NamArchitecture, NamDocument};
use ronie_dsp::{A2Channels, compile_a2, detect_a2};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json::{Value, json};

fn document(channels: usize) -> NamDocument {
    synthetic_a2_document(channels).expect("valid synthetic channels")
}

fn layer_mut(document: &mut NamDocument) -> &mut Value {
    &mut document.config["layers"][0]
}

#[test]
fn compiles_nano_with_exact_weights_layout_and_prewarm() {
    let document = document(3);
    assert_eq!(document.weights.len(), 1_871);
    assert_eq!(detect_a2(&document.config), Some(A2Channels::Nano));

    let compiled = compile_a2(&document).expect("valid Nano model should compile");

    assert_eq!(compiled.channels(), A2Channels::Nano);
    assert_eq!(compiled.layers().len(), 23);
    assert_eq!(compiled.prewarm_samples(), 6_346);
    assert_eq!(compiled.expected_sample_rate(), 48_000.0);
    assert_eq!(compiled.rechannel_weights()[0].to_bits(), 0x322b_cc77);
    assert_eq!(compiled.head_scale().to_bits(), 0x379c_f36b);

    let first_layer = &compiled.layers()[0];
    assert_eq!(first_layer.conv_weights()[43], document.weights[37]);
    assert_eq!(first_layer.layer1x1_weights()[7], document.weights[68]);
    assert_eq!(first_layer.kernel_size(), 6);
    assert_eq!(first_layer.dilation(), 1);

    let head_start = document.weights.len() - (16 * 3 + 2);
    assert_eq!(
        compiled.head_weights()[2 * 3 + 1],
        document.weights[head_start + 16 + 2]
    );
    assert_eq!(
        compiled.head_bias(),
        document.weights[document.weights.len() - 2]
    );
}

#[test]
fn compiles_standard_with_exact_weights_and_prewarm() {
    let document = document(8);
    assert_eq!(document.weights.len(), 12_146);
    assert_eq!(detect_a2(&document.config), Some(A2Channels::Standard));

    let compiled = compile_a2(&document).expect("valid Standard model should compile");

    assert_eq!(compiled.channels(), A2Channels::Standard);
    assert_eq!(compiled.prewarm_samples(), 6_346);
    assert_eq!(compiled.rechannel_weights()[0].to_bits(), 0x322b_cc77);
    assert_eq!(compiled.head_scale().to_bits(), 0x38fe_b857);
}

type Mutation = (&'static str, fn(&mut NamDocument));

#[test]
fn rejects_every_non_a2_shape_variant() {
    let cases: &[Mutation] = &[
        ("layer count", |document| {
            document.config["layers"] = json!([])
        }),
        ("post-stack head", |document| {
            document.config["head"] = json!({})
        }),
        ("condition DSP", |document| {
            document.config["condition_dsp"] = json!({})
        }),
        ("head scale", |document| {
            document.config["head_scale"] = json!(null)
        }),
        ("input channels", |document| {
            document.config["in_channels"] = json!(2)
        }),
        ("channels", |document| {
            let layer = layer_mut(document);
            layer["channels"] = json!(4);
            layer["bottleneck"] = json!(4);
        }),
        ("bottleneck", |document| {
            layer_mut(document)["bottleneck"] = json!(2)
        }),
        ("kernels", |document| {
            layer_mut(document)["kernel_sizes"][0] = json!(7)
        }),
        ("dilations", |document| {
            layer_mut(document)["dilations"][0] = json!(2)
        }),
        ("activation", |document| {
            layer_mut(document)["activation"][0]["negative_slope"] = json!(0.02)
        }),
        ("gating", |document| {
            layer_mut(document)["gated"] = json!(true)
        }),
        ("secondary activation", |document| {
            layer_mut(document)["secondary_activation"][0] = json!({})
        }),
        ("head 1x1", |document| {
            layer_mut(document)["head1x1"]["active"] = json!(true)
        }),
        ("layer 1x1", |document| {
            layer_mut(document)["layer1x1"]["active"] = json!(false)
        }),
        ("head kernel", |document| {
            layer_mut(document)["head"]["kernel_size"] = json!(15)
        }),
        ("head bias", |document| {
            layer_mut(document)["head"]["bias"] = json!(false)
        }),
        ("groups", |document| {
            layer_mut(document)["groups_input"] = json!(2)
        }),
        ("mixin groups", |document| {
            layer_mut(document)["groups_input_mixin"] = json!(2)
        }),
        ("slimmable", |document| {
            layer_mut(document)["slimmable"] = json!({})
        }),
    ];

    for (name, mutate) in cases {
        let mut document = document(8);
        mutate(&mut document);
        assert_eq!(detect_a2(&document.config), None, "accepted {name}");
        assert!(
            matches!(compile_a2(&document), Err(DspError::InvalidA2Shape)),
            "compiled {name}"
        );
    }
}

#[test]
fn rejects_every_active_film_variant() {
    for key in [
        "conv_pre_film",
        "conv_post_film",
        "input_mixin_pre_film",
        "input_mixin_post_film",
        "activation_pre_film",
        "activation_post_film",
        "layer1x1_post_film",
        "head1x1_post_film",
    ] {
        let mut document = document(3);
        layer_mut(&mut document)[key] = json!({"active": true});
        assert_eq!(detect_a2(&document.config), None, "accepted {key}");
    }
}

#[test]
fn rejects_short_and_trailing_weight_streams() {
    let mut short = document(8);
    short.weights.pop();
    assert!(matches!(
        compile_a2(&short),
        Err(DspError::WeightCount {
            expected: 12_146,
            actual: 12_145
        })
    ));

    let mut trailing = document(8);
    trailing.weights.push(0.0);
    assert!(matches!(
        compile_a2(&trailing),
        Err(DspError::WeightCount {
            expected: 12_146,
            actual: 12_147
        })
    ));
}

#[test]
fn rejects_non_wavenet_documents() {
    let mut document = document(3);
    document.architecture = NamArchitecture::SlimmableContainer;

    assert!(matches!(
        compile_a2(&document),
        Err(DspError::InvalidA2Shape)
    ));
}
