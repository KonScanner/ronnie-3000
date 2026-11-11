//! Standalone command-line tests. No test here opens a window.

#[test]
fn dummy_cli_opens_renders_and_closes() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ronie"))
        .args(["--smoke", "--dummy", "--seconds", "0"])
        .output()
        .expect("standalone command runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("output is UTF-8");
    assert!(text.contains("smoke run completed"), "{text}");
}

#[test]
fn help_includes_native_window_and_capture_options() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ronie"))
        .arg("--help")
        .output()
        .expect("standalone help runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("help is UTF-8");
    assert!(text.contains("--ui"));
    assert!(text.contains("--smoke"));
    assert!(text.contains("--capture"));
    assert!(text.contains("--report"));
}

/// The old form of the smoke run must stop with an error, and not open the window.
#[test]
fn seconds_with_no_smoke_flag_is_an_error() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ronie"))
        .args(["--dummy", "--seconds", "0"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("standalone command runs");
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("--seconds is only for --smoke"), "{text}");
}

/// A driver that the operating system does not have stops the run with a clear error.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_linux_driver_is_refused_on_another_system() {
    for driver in ["alsa", "jack"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ronie"))
            .args(["--smoke", "--backend", driver, "--seconds", "0"])
            .output()
            .expect("standalone command runs");
        assert!(!output.status.success());
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(text.contains("only on Linux"), "{text}");
    }
}
