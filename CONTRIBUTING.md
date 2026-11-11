# Contributing to Ronnie 3000

Thank you for your help. Bug reports, tone tests on real hardware, and pull requests are all
welcome.

## Report a bug

Open an issue with the bug report form. Include:

- Your system (Linux distribution, macOS, or Windows version) and your audio interface.
- The audio driver (PipeWire, PulseAudio, ALSA, JACK, CoreAudio, or WASAPI) and the buffer size.
- What you did, what you expected, and what happened.
- The diagnostic log: **Settings → Plugin Settings → Diagnostics → Copy Logs**.

Never paste an API key, a token, or a private file path into an issue.

## Set up a development build

You need Rust (the exact version comes from `rust-toolchain.toml` through
[rustup](https://rustup.rs)), `pkg-config`, and the ALSA development files on Linux:

```bash
# Debian / Ubuntu
sudo apt install build-essential pkg-config libasound2-dev
# Fedora
sudo dnf install gcc pkg-config alsa-lib-devel

git clone https://github.com/KonScanner/ronnie-3000.git
cd ronnie-3000
make doctor   # checks the build tools
make run      # builds and opens Ronnie
```

On macOS and Windows, install Rust with rustup and run
`cargo run --release -p ronie-app --bin ronie`.

The `ronie` binary opens the window when it gets no arguments. Other modes:

```bash
ronie --help                                  # all options
ronie --dummy                                 # the window with a silent dummy audio device
ronie --smoke --dummy --seconds 2             # a headless audio run, then exit
```

## Project layout

| Crate | What it does |
|---|---|
| `crates/dsp` | The real-time audio engine: NAM, impulse responses, EQ, gate, pitch, oversampling |
| `crates/core` | Presets, settings, files, and the community API client |
| `crates/plugin` | The processor and the session state that the UI edits |
| `crates/ui` | The egui interface |
| `crates/app` | The standalone app: audio devices, MIDI, sign-in, and the `ronie` binary |
| `crates/oracle` | Test helpers: fixture generators and optional comparison tools |

## Check your change

Run the full check before you open a pull request. CI runs the same command:

```bash
make check
```

It runs `cargo check`, `clippy` with warnings as errors, all tests, `rustfmt`, and `cargo deny`.
Install `cargo-deny` with `cargo install --locked cargo-deny`.

The audio tests use a dummy device, so no sound card is necessary and no sound plays.

### Visual check of the interface

The pixel suite captures every UI scenario on a private display, so no window opens on your
desktop. Save a baseline before your change, then compare after it:

```bash
script/check-ui.sh --save-baseline        # on the commit before your change
script/check-ui.sh --suite ronnie-baseline
```

Look at each changed image under `target/ui-suite/`. See `fixtures/README.md` for the details.

## Sign-in during development

The app asks for your personal TONE3000 API key the first time you click **Sign in**, and keeps
it in your settings folder. For a development build, you can also put your own key in
a `.env` file at the repository root. Git ignores this file:

```bash
RONIE_OAUTH_ORIGIN=https://www.tone3000.com
RONIE_OAUTH_CLIENT_ID=t3k_pub_your_key
```

`make build` and `make run` read `.env`. Never commit a key, and never put a secret key in
`.env`. The same two values in the environment at launch replace any other key.

## Pull requests

Fork the repository and open a pull request against `main`. Only the maintainer can push to
`main` and merge pull requests. Each pull request needs the maintainer's approval and a passing
Linux check. The first workflow run of a new contributor waits for the maintainer's approval.

- Keep each pull request to one topic, and add tests for new behaviour.
- Use short commit subjects in the imperative mood, for example `Add a bass tuning`.
- Do not change existing UI scenarios, masks, or thresholds to make a check pass.

By contributing, you agree that your work is licensed under the [MIT License](LICENSE).
