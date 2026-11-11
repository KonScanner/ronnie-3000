//! Bounded local discovery, WAV decoding, and local tone loading tests.

use std::{fs, path::Path};

use ronie_core::{
    AudioError, DspError, DspResult, LocalError, ModelKind, NamDocument, decode_wav,
    discover_local, inspect_local_model, load_local_path, local_file_name_from_url,
};
use tempfile::TempDir;

fn wav_pcm16(samples: &[i16], padless: bool) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data_len as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    if !data_len.is_multiple_of(2) && !padless {
        bytes.push(0);
    }
    bytes
}

fn minimal_nam() -> Vec<u8> {
    br#"{"version":"0.7.0","architecture":"WaveNet","config":{},"weights":[]}"#.to_vec()
}

fn accept_nam(_: &NamDocument) -> DspResult<()> {
    Ok(())
}

#[test]
fn discovery_recurses_filters_majority_and_uses_natural_names() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir(temp.path().join("More")).expect("nested dir");
    for path in ["amp 2.nam", "amp 10.nam", "More/amp 1.nam"] {
        fs::write(temp.path().join(path), b"model").expect("fixture");
    }
    fs::write(temp.path().join("cab.wav"), b"audio").expect("minority");
    fs::write(temp.path().join("notes.txt"), b"ignore").expect("other");

    let inputs = discover_local(temp.path()).expect("discovery should pass");
    let names: Vec<_> = inputs.iter().map(|input| input.name.as_str()).collect();

    assert_eq!(names, ["amp 1.nam", "amp 2.nam", "amp 10.nam"]);
    assert!(inputs.iter().all(|input| input.kind == ModelKind::Nam));
}

#[test]
fn discovery_gives_nam_extension_priority_on_ties() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join("one.nam"), b"model").expect("NAM");
    fs::write(temp.path().join("two.wav"), b"audio").expect("WAV");

    let inputs = discover_local(temp.path()).expect("discovery should pass");

    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].name, "one.nam");
}

#[test]
fn discovery_rejects_301_files_before_reading_contents() {
    let temp = TempDir::new().expect("temp dir");
    for index in 0..301 {
        fs::write(temp.path().join(format!("{index}.nam")), b"").expect("fixture");
    }

    let result = discover_local(temp.path());

    assert!(matches!(
        result,
        Err(LocalError::TooManyFiles {
            actual: 301,
            max: 300
        })
    ));
}

#[test]
fn discovery_rejects_a_file_over_50_mib_from_metadata() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("large.nam");
    let file = fs::File::create(&path).expect("large fixture");
    file.set_len(50 * 1024 * 1024 + 1).expect("set length");

    let result = discover_local(&path);

    assert!(matches!(result, Err(LocalError::InputTooLarge)));
}

#[cfg(unix)]
#[test]
fn discovery_does_not_follow_symlinks_outside_the_root() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let root = temp.path().join("root");
    fs::create_dir(&root).expect("root");
    fs::write(temp.path().join("outside.nam"), b"secret").expect("outside");
    symlink(temp.path().join("outside.nam"), root.join("linked.nam")).expect("symlink");

    let result = discover_local(&root);

    assert!(matches!(result, Err(LocalError::NoSupportedFiles)));
}

#[test]
fn wav_rejects_empty_short_and_truncated_inputs() {
    for bytes in [
        Vec::new(),
        b"RIFF".to_vec(),
        wav_pcm16(&[1], false)[..20].to_vec(),
    ] {
        assert!(decode_wav(&bytes).is_err());
    }
    assert!(matches!(
        decode_wav(&wav_pcm16(&[], false)),
        Err(AudioError::EmptyAudio)
    ));
}

#[test]
fn wav_decodes_padless_pcm_and_float_samples() {
    let pcm = decode_wav(&wav_pcm16(&[0, 16_384, -16_384], true)).expect("padless PCM");
    assert_eq!(pcm.sample_rate, 48_000.0);
    assert_eq!(pcm.channels.len(), 1);
    assert_eq!(pcm.channels[0].len(), 3);
    assert_eq!(pcm.channels[0][0], 0.0);
    assert_eq!(pcm.channels[0][1], 0.5);
    assert_eq!(pcm.channels[0][2], -0.5);

    let mut float = wav_pcm16(&[0], true);
    float[20..22].copy_from_slice(&3_u16.to_le_bytes());
    float[32..34].copy_from_slice(&4_u16.to_le_bytes());
    float[34..36].copy_from_slice(&32_u16.to_le_bytes());
    float.truncate(44);
    float.extend_from_slice(&1.0_f32.to_le_bytes());
    let data_len = 4_u32;
    float[4..8].copy_from_slice(&(36_u32 + data_len).to_le_bytes());
    float[40..44].copy_from_slice(&data_len.to_le_bytes());
    let decoded = decode_wav(&float).expect("float WAV");
    assert_eq!(decoded.channels[0], [1.0]);
}

#[test]
fn local_inspection_validates_nam_and_computes_content_identity() {
    let bytes = minimal_nam();
    let inspected = inspect_local_model("minimal.nam", &bytes, &accept_nam)
        .expect("minimal document should pass the injected gate");

    assert_eq!(inspected.name, "minimal");
    assert_eq!(inspected.kind, ModelKind::Nam);
    assert!(inspected.id.0 > 0);
    assert!(
        inspected
            .stash_name
            .ends_with(&format!("-{}.nam", bytes.len()))
    );
}

#[test]
fn local_inspection_propagates_the_a2_gate_failure() {
    let reject = |_: &NamDocument| Err(DspError::InvalidA2Shape);

    let result = inspect_local_model("legacy.nam", &minimal_nam(), &reject);

    assert!(matches!(
        result,
        Err(LocalError::Validation(DspError::InvalidA2Shape))
    ));
}

#[test]
fn local_load_stashes_valid_survivors_and_skips_bad_files() {
    let temp = TempDir::new().expect("temp dir");
    let input = temp.path().join("pack");
    fs::create_dir(&input).expect("input dir");
    fs::write(input.join("good.wav"), wav_pcm16(&[1, 2, 3], true)).expect("WAV");
    fs::write(input.join("also good.wav"), wav_pcm16(&[4, 5, 6], true)).expect("WAV");
    fs::write(input.join("bad.wav"), b"not wave").expect("bad WAV");
    fs::write(input.join("bad.nam"), b"not json").expect("bad NAM");
    let stash = ronie_core::LocalStash::new(temp.path().join("stash")).expect("stash");

    let loaded = load_local_path(&input, &stash, &accept_nam).expect("survivors should load");

    assert_eq!(loaded.tone.models.len(), 2);
    assert_eq!(loaded.skipped, 1);
    assert_eq!(loaded.tone.kind, ModelKind::Ir);
    assert!(loaded.tone.local);
    assert!(loaded.stashed[0].path.is_file());
}

#[test]
fn local_load_returns_the_first_error_when_nothing_survives() {
    let temp = TempDir::new().expect("temp dir");
    let input = temp.path().join("pack");
    fs::create_dir(&input).expect("input dir");
    fs::write(input.join("bad.nam"), b"not json").expect("bad NAM");
    let stash = ronie_core::LocalStash::new(temp.path().join("stash")).expect("stash");

    let result = load_local_path(&input, &stash, &accept_nam);

    assert!(matches!(result, Err(LocalError::Nam(_))));
}

#[test]
fn url_names_are_percent_decoded_for_file_and_remote_urls() {
    assert_eq!(
        local_file_name_from_url("file:///tmp/Deluxe%20Reverb%202.nam").expect("file URL"),
        "Deluxe Reverb 2.nam"
    );
    assert_eq!(
        local_file_name_from_url("https://fixtures.local/x/Amplificador%20V%C3%A1lvulas.wav")
            .expect("remote URL"),
        "Amplificador Válvulas.wav"
    );
}

#[test]
fn inspection_rejects_empty_and_unsupported_inputs() {
    assert!(matches!(
        inspect_local_model("empty.wav", b"", &accept_nam),
        Err(LocalError::EmptyInput)
    ));
    assert!(matches!(
        inspect_local_model("notes.txt", b"hello", &accept_nam),
        Err(LocalError::UnsupportedExtension)
    ));
}

#[test]
fn local_load_rejects_a_missing_single_file() {
    let temp = TempDir::new().expect("temp dir");
    let stash = ronie_core::LocalStash::new(temp.path().join("stash")).expect("stash");

    let result = load_local_path(Path::new("/missing/ronie.nam"), &stash, &accept_nam);

    assert!(matches!(result, Err(LocalError::Io(_))));
}
