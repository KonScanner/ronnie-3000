//! The steps of the host test, and the checks of each step.

use std::{
    fs,
    path::{Path, PathBuf},
    thread::sleep,
    time::{Duration, Instant},
};

use eyre::{Result, WrapErr, bail, eyre};
use ronie_core::params::{ParameterCatalog, ParameterSpec};
use ronie_ui::shell::{DESIGN_WIDTH, NativeShellOptions};

use crate::{
    reaper::{Reaper, Setup},
    wav,
    x11::{Display, WindowInfo},
};

pub(crate) const USAGE: &str = "\
usage: ronnie-host-test [options]
  --reaper DIR    the folder with the reaper program
                  (default target/host-test/reaper/reaper_linux_x86_64/REAPER)
  --plugin FILE   the plugin library (default target/debug/libronie_plugin_nice.so)
  --work DIR      the folder for this run (default target/host-test/work)
  --dpi N         the Xft.dpi of the private display (default 144, a scale of 1.5)
  --format F      clap or vst3 (default clap)
  --reset X,Y[;X,Y...]
                  the clicks that reset the session or load a preset, in design points
                  from the top left of the editor. With it, the test clicks these points in
                  order and counts the host undo steps. A screenshot follows each click.";

/// The file that marks a folder that this test made, so that a new run can delete it.
const MARKER: &str = ".ronnie-host-test";

/// How long the test watches a new window for a change of size.
const WATCH: Duration = Duration::from_millis(3_000);

/// The options of one run.
#[derive(Debug)]
pub(crate) struct Options {
    reaper: PathBuf,
    plugin: PathBuf,
    work: PathBuf,
    dpi: u32,
    reset: Vec<(f32, f32)>,
    /// The REAPER name of the plugin format: CLAP or VST3.
    format: &'static str,
}

impl Options {
    pub(crate) fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Self {
            reaper: "target/host-test/reaper/reaper_linux_x86_64/REAPER".into(),
            plugin: "target/debug/libronie_plugin_nice.so".into(),
            work: "target/host-test/work".into(),
            dpi: 144,
            reset: Vec::new(),
            format: "CLAP",
        };
        while let Some(name) = args.next() {
            let mut value = || args.next().ok_or_else(|| eyre!("{name} needs a value"));
            match name.as_str() {
                "--reaper" => options.reaper = value()?.into(),
                "--plugin" => options.plugin = value()?.into(),
                "--work" => options.work = value()?.into(),
                "--dpi" => options.dpi = value()?.parse().wrap_err("--dpi")?,
                "--format" => {
                    options.format = match value()?.to_ascii_lowercase().as_str() {
                        "clap" => "CLAP",
                        "vst3" => "VST3",
                        other => bail!("unknown format {other}"),
                    };
                }
                "--reset" => {
                    for point in value()?.split(';') {
                        let (x, y) = point
                            .split_once(',')
                            .ok_or_else(|| eyre!("--reset needs X,Y"))?;
                        options.reset.push((x.trim().parse()?, y.trim().parse()?));
                    }
                }
                _ => bail!("unknown option {name}"),
            }
        }
        if options.dpi == 0 {
            bail!("--dpi must be more than 0");
        }
        Ok(options)
    }
}

/// The results of the checks.
#[derive(Default)]
struct Checks {
    failed: usize,
}

impl Checks {
    fn check(&mut self, passed: bool, what: impl std::fmt::Display) {
        println!("{} {what}", if passed { "PASS" } else { "FAIL" });
        if !passed {
            self.failed += 1;
        }
    }
}

fn info(what: impl std::fmt::Display) {
    println!("     {what}");
}

/// Runs all steps. Returns the number of failed checks.
pub(crate) fn run(options: &Options) -> Result<usize> {
    prepare_work(&options.work)?;
    let work = fs::canonicalize(&options.work)?;
    let shots = work.join("shots");
    fs::create_dir_all(&shots)?;
    let display = Display::start(2560, 1600, options.dpi)?;
    info(format!(
        "display {}, Xft.dpi at start: {:?}",
        display.name(),
        display.xft_dpi()?
    ));
    let setup = Setup {
        reaper_dir: options.reaper.clone(),
        work: work.clone(),
        display: display.name(),
    };
    setup.install(&options.plugin)?;
    let scale = options.dpi as f32 / 96.0;
    let mut checks = Checks::default();

    let mut reaper = Reaper::start(&setup, None)?;
    let audio = reaper.one("audio")?;
    info(format!("REAPER audio: {audio}"));
    // With a real audio system, the plugin could reach a sound device, so the run stops here.
    if !audio.starts_with("Dummy Audio") {
        bail!("REAPER uses the audio system {audio:?}, not Dummy Audio");
    }
    let list = reaper.one("list")?;
    checks.check(
        list.contains("CLAP: Ronnie 3000"),
        "REAPER finds the CLAP plugin",
    );
    checks.check(
        list.contains("VST3: Ronnie 3000"),
        "REAPER finds the VST3 plugin",
    );
    let index = reaper.one(&format!("add 0 {}: Ronnie 3000", options.format))?;
    checks.check(
        index == "0",
        format!("the plugin goes on track 1 ({index})"),
    );

    parameter_types(&mut reaper, &mut checks)?;

    // The first window of a new plugin, at zoom 1.
    let expected = design_pixels(1.0, scale);
    info(format!(
        "Xft.dpi before the first open: {:?}",
        display.xft_dpi()?
    ));
    let sizes = open_and_watch(&display, &mut reaper)?;
    write_tree(&display, &work.join("windows.txt"))?;
    display.screenshot(&shots.join("first-open.png"))?;
    checks.check(
        sizes.len() == 1 && sizes[0].0 == expected,
        format!(
            "the first window opens at {expected:?}, and no window changes its size (editor and host sizes {sizes:?})"
        ),
    );

    if !options.reset.is_empty() {
        let clicks = Clicks {
            points: &options.reset,
            shots: &shots,
        };
        undo_steps_of_a_reset(&display, &mut reaper, &mut checks, &clicks)?;
    }

    // A larger window gives a larger zoom, which the project keeps.
    let plugin = plugin_window(&display)?;
    let frame = top_window(&display, &plugin)?;
    display.resize(frame.id, frame.width + 320, frame.height + 200)?;
    sleep(Duration::from_millis(2_000));
    let grown = plugin_window(&display)?;
    checks.check(
        grown.width > plugin.width,
        format!(
            "the editor follows a larger window ({}x{} to {}x{})",
            plugin.width, plugin.height, grown.width, grown.height
        ),
    );
    display.screenshot(&shots.join("grown.png"))?;
    let project = work.join("zoom.rpp");
    reaper.one(&format!("save {}", project.display()))?;
    reaper.stop()?;
    // With no saved window size, REAPER asks the plugin for its size.
    let saved = fs::read_to_string(&project)?;
    let kept: Vec<_> = saved
        .lines()
        .filter(|line| !line.trim_start().starts_with("FLOAT"))
        .collect();
    let removed = saved.lines().count() - kept.len();
    checks.check(
        removed > 0,
        format!("the saved project has window size lines to remove ({removed})"),
    );
    fs::write(&project, kept.join("\n") + "\n")?;

    let mut reaper = Reaper::start(&setup, Some(&project))?;
    let sizes = open_and_watch(&display, &mut reaper)?;
    display.screenshot(&shots.join("restored.png"))?;
    let restored = sizes.last().map(|(editor, _)| *editor).unwrap_or_default();
    checks.check(
        sizes.len() == 1 && restored.0.abs_diff(grown.width) <= 2,
        format!(
            "the project zoom comes back after a restart, and no window changes its size ({sizes:?}, before {}x{})",
            grown.width, grown.height
        ),
    );

    render_after_reload(&mut reaper, &work, &mut checks)?;

    reaper.check_alive()?;
    let log = fs::read_to_string(setup.log()).unwrap_or_default();
    let problems: Vec<_> = log
        .lines()
        .filter(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("panicked") || line.contains("ronnie could not")
        })
        .collect();
    checks.check(
        problems.is_empty(),
        format!("the log has no plugin error {problems:?}"),
    );
    reaper.stop()?;
    info(format!("files of this run: {}", work.display()));
    Ok(checks.failed)
}

/// Deletes the folder of an earlier run, and makes a new one. A folder with other files stays.
fn prepare_work(work: &Path) -> Result<()> {
    if work.exists() {
        if !work.join(MARKER).exists() {
            bail!(
                "{} exists and is not a folder of this test; remove it or choose another --work",
                work.display()
            );
        }
        fs::remove_dir_all(work)?;
    }
    fs::create_dir_all(work)?;
    fs::write(work.join(MARKER), "")?;
    Ok(())
}

/// The physical size of the design box at `zoom` on a display with `scale`, with the chrome that
/// the plugin view starts with. The test session has no device and no failed block, so it has no
/// banner, as the session of a new plugin.
fn design_pixels(zoom: f32, scale: f32) -> (u32, u32) {
    let options = NativeShellOptions {
        standalone: false,
        ..NativeShellOptions::default()
    };
    let height = options.initial_chrome().design_height() as f32;
    (
        (DESIGN_WIDTH as f32 * zoom * scale).round() as u32,
        (height * zoom * scale).round() as u32,
    )
}

/// How the host must show a catalog entry: 0 for a knob, 1 for a switch, 2 for a choice. `None`
/// for an entry that is not a host parameter.
fn host_kind(spec: &ParameterSpec) -> Option<u8> {
    if !(spec.scope.preset || !spec.scope.machine_default) {
        return None;
    }
    let discrete = !spec.choices.is_empty() || spec.interval == Some(1.0);
    let switch = spec.choices.is_empty() && spec.min == 0.0 && spec.max == 1.0;
    Some(match (discrete, switch) {
        (false, _) => 0,
        (true, true) => 1,
        (true, false) => 2,
    })
}

/// The REAPER index of the host parameter with the catalog id `id`. It is the position of the entry
/// among the catalog entries that are host parameters, in catalog order.
fn host_index(id: &str) -> Result<usize> {
    ParameterCatalog::v1()
        .iter()
        .filter(|spec| host_kind(spec).is_some())
        .position(|spec| spec.id.0 == id)
        .ok_or_else(|| eyre!("{id} is not a host parameter"))
}

fn parameter_types(reaper: &mut Reaper, checks: &mut Checks) -> Result<()> {
    let listed = reaper.one("params 0 0")?;
    let params: Vec<Vec<&str>> = listed
        .split('|')
        .map(|param| param.split('~').collect())
        .collect();
    let expected: Vec<_> = ParameterCatalog::v1()
        .iter()
        .filter_map(|spec| host_kind(spec).map(|kind| (spec, kind)))
        .collect();
    info(format!(
        "REAPER lists {} parameters for {} catalog entries",
        params.len(),
        expected.len()
    ));
    let mut wrong = Vec::new();
    let mut counts = [0_usize; 3];
    for (index, (spec, kind)) in expected.iter().enumerate() {
        let Some(param) = params.get(index) else {
            wrong.push(format!("{} is missing", spec.id.0));
            continue;
        };
        let [name, stepped, toggle, step] = param[..] else {
            bail!("REAPER gave the parameter {param:?}");
        };
        let fits = match kind {
            0 => stepped == "false",
            1 => stepped == "true" && toggle == "true",
            _ => {
                // CLAP gives the step in plain units, and VST3 gives it in the 0 to 1 range.
                let steps = spec.max - spec.min;
                let step: f32 = step.parse().unwrap_or(f32::NAN);
                stepped == "true"
                    && toggle == "false"
                    && ((step - 1.0).abs() < 1e-3 || (step * steps - 1.0).abs() < 1e-3)
            }
        };
        counts[usize::from(*kind)] += 1;
        if !fits {
            wrong.push(format!(
                "{name} ({}): stepped {stepped}, toggle {toggle}, step {step}",
                spec.id.0
            ));
        }
    }
    checks.check(
        wrong.is_empty(),
        format!(
            "REAPER shows {} knobs, {} switches, and {} stepped choices with the right types {wrong:?}",
            counts[0], counts[1], counts[2]
        ),
    );
    Ok(())
}

/// The baseview window of the plugin. baseview gives it the title of the editor.
fn plugin_window(display: &Display) -> Result<WindowInfo> {
    display
        .windows()?
        .into_iter()
        .filter(|window| window.name == "Ronnie 3000")
        .max_by_key(|window| window.width * window.height)
        .ok_or_else(|| eyre!("no plugin window"))
}

/// The top-level window that holds `window`.
fn top_window(display: &Display, window: &WindowInfo) -> Result<WindowInfo> {
    let top = *window
        .ancestors
        .first()
        .ok_or_else(|| eyre!("the plugin window has no parent"))?;
    display
        .windows()?
        .into_iter()
        .find(|candidate| candidate.id == top)
        .ok_or_else(|| eyre!("no top-level window"))
}

/// The size of the editor window and of the host window around it.
type Sizes = ((u32, u32), (u32, u32));

/// Shows the editor and returns each pair of sizes that the editor and the host window had, in
/// order. One pair means that no window changed its size.
fn open_and_watch(display: &Display, reaper: &mut Reaper) -> Result<Vec<Sizes>> {
    reaper.one("show 0 0")?;
    let started = Instant::now();
    let mut seen: Vec<Sizes> = Vec::new();
    while started.elapsed() < WATCH {
        // A sample with no host window would look like a change of size, so it does not count.
        if let Ok(window) = plugin_window(display)
            && let Ok(frame) = top_window(display, &window)
        {
            let sizes = ((window.width, window.height), (frame.width, frame.height));
            if seen.last() != Some(&sizes) {
                seen.push(sizes);
            }
        }
        sleep(Duration::from_millis(10));
    }
    reaper.check_alive()?;
    Ok(seen)
}

fn write_tree(display: &Display, path: &Path) -> Result<()> {
    let mut text = String::new();
    for window in display.windows()? {
        text.push_str(&format!(
            "{}0x{:x} {},{} {}x{} {:?}\n",
            "  ".repeat(window.depth),
            window.id,
            window.x,
            window.y,
            window.width,
            window.height,
            window.name
        ));
    }
    fs::write(path, text)?;
    Ok(())
}

/// Clicks in the editor, in design points.
struct Clicks<'a> {
    points: &'a [(f32, f32)],
    /// The folder for a screenshot after each click.
    shots: &'a Path,
}

/// Sets some host values away from their defaults, resets the session in the editor, and counts
/// the undo steps that the host made.
fn undo_steps_of_a_reset(
    display: &Display,
    reaper: &mut Reaper,
    checks: &mut Checks,
    clicks: &Clicks<'_>,
) -> Result<()> {
    let changed = [
        "toneBass",
        "toneMid",
        "toneTreble",
        "outputLevel",
        "gateThreshold",
    ]
    .map(host_index)
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    let commands: Vec<String> = changed
        .iter()
        .map(|index| format!("setparam 0 0 {index} 0.9"))
        .collect();
    let commands: Vec<&str> = commands.iter().map(String::as_str).collect();
    reaper.send(&commands)?;
    // The plugin worker gives the session the host values.
    sleep(Duration::from_millis(500));
    let mut before = Vec::new();
    for index in &changed {
        before.push(reaper.one(&format!("getparam 0 0 {index}"))?);
    }
    reaper.one("mark")?;
    for (index, (x, y)) in clicks.points.iter().enumerate() {
        let window = plugin_window(display)?;
        let pixels = window.width as f32 / DESIGN_WIDTH as f32;
        display.click(
            window.x + (x * pixels).round() as i32,
            window.y + (y * pixels).round() as i32,
        )?;
        sleep(Duration::from_millis(800));
        display.screenshot(&clicks.shots.join(format!("click-{}.png", index + 1)))?;
    }
    sleep(Duration::from_millis(1_000));
    let mut after = Vec::new();
    for index in &changed {
        after.push(reaper.one(&format!("getparam 0 0 {index}"))?);
    }
    let reset = before
        .iter()
        .zip(&after)
        .all(|(before, after)| before != after);
    checks.check(
        reset,
        format!("the clicks reset the host values ({before:?} to {after:?})"),
    );
    let undos = reaper.one("undos")?;
    let (count, names) = undos.split_once('~').unwrap_or((&undos, ""));
    let count: usize = count.parse().unwrap_or(usize::MAX);
    checks.check(
        count <= 1,
        format!(
            "a reset of {} values is one host undo step ({count}: {names})",
            changed.len()
        ),
    );
    Ok(())
}

/// Renders the project, saves it, opens it again, and renders it again. Both renders must match.
fn render_after_reload(reaper: &mut Reaper, work: &Path, checks: &mut Checks) -> Result<()> {
    let sine = work.join("sine.wav");
    wav::write_sine(&sine, 48_000, 3.0, 220.0, 0.5)?;
    let renders = work.join("render");
    fs::create_dir_all(&renders)?;
    // The output level goes low, so that a level from a default session would show.
    reaper.send(&[
        &format!("media 0 {}", sine.display()),
        &format!("setparam 0 0 {} 0.1", host_index("outputLevel")?),
        "online",
    ])?;
    let first = render(reaper, &renders, "a")?;
    let project = work.join("render.rpp");
    reaper.one(&format!("save {}", project.display()))?;
    let loaded = Instant::now();
    reaper.one(&format!("open {}", project.display()))?;
    info(format!(
        "the project opened in {:.2} s",
        loaded.elapsed().as_secs_f32()
    ));
    sleep(Duration::from_millis(2_000));
    reaper.one("online")?;
    let second = render(reaper, &renders, "b")?;

    let (a, rate) = wav::read_first_channel(&first)?;
    let (b, _) = wav::read_first_channel(&second)?;
    let a = wav::window_levels(&a, rate, 0.1);
    let b = wav::window_levels(&b, rate, 0.1);
    let widest = a
        .iter()
        .zip(&b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    let loudest = a.iter().copied().fold(f32::MIN, f32::max);
    info(format!(
        "render levels: {} windows, loudest {loudest:.1} dBFS, largest difference {widest:.3} dB",
        a.len()
    ));
    checks.check(
        a.len() >= 20 && a.len() == b.len() && loudest > -60.0 && widest < 0.1,
        "a render after a project reload gives the same audio",
    );
    Ok(())
}

/// Renders the project to `dir/name.wav` and waits until the file is complete.
fn render(reaper: &mut Reaper, dir: &Path, name: &str) -> Result<PathBuf> {
    reaper.one(&format!("render {} {name}", dir.display()))?;
    let path = dir.join(format!("{name}.wav"));
    let started = Instant::now();
    let mut last = None;
    loop {
        let size = fs::metadata(&path).map(|metadata| metadata.len()).ok();
        if size.is_some() && size == last {
            return Ok(path);
        }
        last = size;
        if started.elapsed() > Duration::from_secs(60) {
            bail!("no render at {}", path.display());
        }
        reaper.check_alive()?;
        sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_options_read_the_reset_point() {
        let options = Options::parse(
            ["--reset", "12.5, 30", "--dpi", "96"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("options");
        assert_eq!(options.reset, [(12.5, 30.0)]);
        assert_eq!(options.dpi, 96);
        let options =
            Options::parse(["--reset", "1,2;3,4"].into_iter().map(str::to_owned)).expect("options");
        assert_eq!(options.reset, [(1.0, 2.0), (3.0, 4.0)]);
        assert!(Options::parse(["--dpi"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--other"].into_iter().map(str::to_owned)).is_err());
    }

    #[test]
    fn each_kind_of_host_parameter_is_in_the_catalog() {
        let mut counts = [0; 3];
        for spec in ParameterCatalog::v1().iter() {
            if let Some(kind) = host_kind(spec) {
                counts[usize::from(kind)] += 1;
            }
        }
        assert!(counts.iter().all(|count| *count > 0), "{counts:?}");
    }

    #[test]
    fn the_host_index_follows_the_catalog() {
        assert_eq!(host_index("outputLevel").expect("index"), 4);
        assert!(host_index("noSuchParameter").is_err());
    }

    #[test]
    fn a_folder_of_another_owner_stays() {
        let dir = tempfile::tempdir().expect("dir");
        let dir = dir.path();
        fs::write(dir.join("keep.txt"), "").expect("file");
        assert!(prepare_work(dir).is_err());
        assert!(dir.join("keep.txt").exists());
    }
}
