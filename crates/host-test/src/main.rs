//! Tests the Ronnie 3000 plugin in REAPER, on a private X display.
//!
//! The test starts Xvfb and REAPER with a private home, so it opens no window on the desktop and
//! reaches no sound device. REAPER runs a start script that reads commands from a file. The test
//! checks the host parameter types, the editor size at the first open and after a restart, and
//! that a render after a project reload gives the same audio.
//!
//! Build the plugin first. The allocation check stops REAPER when the audio thread allocates:
//!
//! ```text
//! cargo build -p ronie-plugin-nice --features assert_process_allocs
//! cargo run -p ronie-host-test -- --reaper <folder with the reaper program>
//! ```
//!
//! REAPER is not part of this repository. Use the Linux download from reaper.fm.
//!
//! If you stop a run with Ctrl-C, Xvfb and REAPER can continue to run. Stop them by hand:
//! `pkill -f ronnie-host-test` does not reach them. Stop only the Xvfb of the display that the run
//! printed at start, for example `pkill -f '^Xvfb :99 '`, and the REAPER that uses it.

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod wav;

#[cfg(target_os = "linux")]
mod reaper;
#[cfg(target_os = "linux")]
mod scenario;
#[cfg(target_os = "linux")]
mod x11;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let options = match scenario::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\n\n{}", scenario::USAGE);
            return ExitCode::from(2);
        }
    };
    match scenario::run(&options) {
        Ok(0) => {
            println!("All checks passed.");
            ExitCode::SUCCESS
        }
        Ok(failed) => {
            println!("{failed} checks failed.");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("The test stopped: {error:?}");
            ExitCode::from(2)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("The host test runs only on Linux, because it uses Xvfb.");
    ExitCode::from(2)
}
