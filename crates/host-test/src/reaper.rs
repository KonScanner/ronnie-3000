//! A REAPER process in a private home, with the plugin installed, and the command file that its
//! start script reads.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use eyre::{Result, WrapErr, bail, eyre};

const STARTUP_SCRIPT: &str = include_str!("startup.lua");

/// The files of one test run.
pub(crate) struct Setup {
    /// The folder that holds the `reaper` program.
    pub(crate) reaper_dir: PathBuf,
    /// The folder of this run. A private home and the command files go in it.
    pub(crate) work: PathBuf,
    /// The value for `DISPLAY`.
    pub(crate) display: String,
}

/// The `linux_audio_mode` of the REAPER "Dummy Audio" system. It runs the audio blocks with no
/// sound device, so that the plugin gets host values as in a real session.
const DUMMY_AUDIO: u32 = 2;

impl Setup {
    /// Makes the private home, with the start script and with the plugin as CLAP and as VST3.
    pub(crate) fn install(&self, plugin: &Path) -> Result<()> {
        // A portable REAPER keeps its configuration next to the program and ignores HOME, so it
        // would use and change the real configuration.
        let portable = self.reaper_dir.join("reaper.ini");
        if portable.exists() {
            bail!(
                "{} exists, so this REAPER is a portable install that ignores the private home; \
                 use a REAPER folder with no reaper.ini",
                portable.display()
            );
        }
        let home = self.home();
        let scripts = home.join(".config/REAPER/Scripts");
        let vst3 = home.join(format!(
            ".vst3/Ronnie 3000.vst3/Contents/{}",
            vst3_arch_dir(std::env::consts::ARCH)
        ));
        for dir in [
            &scripts,
            &vst3,
            &home.join(".clap"),
            &self.data_home(),
            &self.cache_home(),
            &self.state_home(),
            &self.run_dir(),
        ] {
            fs::create_dir_all(dir).wrap_err_with(|| format!("create {}", dir.display()))?;
        }
        set_private(&self.run_dir())?;
        fs::write(scripts.join("__startup.lua"), STARTUP_SCRIPT)?;
        fs::copy(plugin, home.join(".clap/Ronnie 3000.clap"))
            .wrap_err_with(|| format!("copy {}", plugin.display()))?;
        fs::copy(plugin, vst3.join("Ronnie 3000.so"))?;
        fs::write(
            home.join(".config/REAPER/reaper.ini"),
            format!("[REAPER]\nlinux_audio_mode={DUMMY_AUDIO}\n"),
        )?;
        // An empty ALSA configuration, so that REAPER reaches no sound device.
        fs::write(self.work.join("no-alsa.conf"), "")?;
        Ok(())
    }

    fn home(&self) -> PathBuf {
        self.work.join("home")
    }

    fn config_home(&self) -> PathBuf {
        self.home().join(".config")
    }

    fn data_home(&self) -> PathBuf {
        self.home().join(".local/share")
    }

    fn cache_home(&self) -> PathBuf {
        self.home().join(".cache")
    }

    fn state_home(&self) -> PathBuf {
        self.home().join(".local/state")
    }

    fn run_dir(&self) -> PathBuf {
        self.work.join("run")
    }

    /// The log of REAPER and of the plugins in it.
    pub(crate) fn log(&self) -> PathBuf {
        self.work.join("reaper.log")
    }
}

/// The folder in a VST3 bundle for the binary of the CPU `arch`.
fn vst3_arch_dir(arch: &str) -> String {
    match arch {
        "x86_64" => "x86_64-linux".to_owned(),
        "aarch64" => "aarch64-linux".to_owned(),
        other => format!("{other}-linux"),
    }
}

#[cfg(unix)]
fn set_private(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// A running REAPER. It stops when this drops.
pub(crate) struct Reaper {
    child: Child,
    work: PathBuf,
}

impl Reaper {
    /// Starts REAPER with a new project, or with `project`, and waits until its script runs.
    pub(crate) fn start(setup: &Setup, project: Option<&Path>) -> Result<Self> {
        let log = File::options()
            .create(true)
            .append(true)
            .open(setup.log())?;
        let ready_before = ready_count(&setup.work);
        let mut command = Command::new(setup.reaper_dir.join("reaper"));
        command.arg("-nosplash");
        match project {
            Some(project) => command.arg(project),
            None => command.arg("-new"),
        };
        // Only these variables reach REAPER, so that no setting of the desktop session, sound
        // server, or real home gets to it.
        command.env_clear();
        for name in ["PATH", "LANG", "LC_ALL"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let child = command
            .env("DISPLAY", &setup.display)
            .env("HOME", setup.home())
            .env("XDG_CONFIG_HOME", setup.config_home())
            .env("XDG_DATA_HOME", setup.data_home())
            .env("XDG_CACHE_HOME", setup.cache_home())
            .env("XDG_STATE_HOME", setup.state_home())
            .env("XDG_RUNTIME_DIR", setup.run_dir())
            .env("ALSA_CONFIG_PATH", setup.work.join("no-alsa.conf"))
            .env("RONNIE_HOST_TEST_DIR", &setup.work)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .wrap_err("start REAPER")?;
        let mut reaper = Self {
            child,
            work: setup.work.clone(),
        };
        let started = Instant::now();
        while ready_count(&setup.work) <= ready_before {
            reaper.check_alive()?;
            if started.elapsed() > Duration::from_secs(60) {
                bail!("the REAPER start script did not run");
            }
            sleep(Duration::from_millis(100));
        }
        Ok(reaper)
    }

    /// Fails when REAPER stopped, for example because the plugin aborted on an allocation.
    pub(crate) fn check_alive(&mut self) -> Result<()> {
        match self.child.try_wait()? {
            Some(status) => bail!("REAPER stopped: {status}"),
            None => Ok(()),
        }
    }

    /// Runs commands in REAPER and returns one result for each.
    pub(crate) fn send(&mut self, commands: &[&str]) -> Result<Vec<String>> {
        let output = self.work.join("out.txt");
        let before = line_count(&output);
        let pending = self.work.join("cmd.tmp");
        fs::write(&pending, commands.join("\n") + "\n")?;
        fs::rename(&pending, self.work.join("cmd.txt"))?;
        let started = Instant::now();
        loop {
            let text = fs::read_to_string(&output).unwrap_or_default();
            // REAPER can write a long line in parts, so a line counts only when it is complete.
            let complete = text.rfind('\n').map_or("", |end| &text[..=end]);
            let lines: Vec<_> = complete.lines().skip(before).collect();
            if lines.len() >= commands.len() {
                return lines
                    .into_iter()
                    .zip(commands)
                    .map(|(line, command)| match line.split_once(' ') {
                        Some(("ok", result)) => Ok(result.to_owned()),
                        _ => Err(eyre!("{command}: {line}")),
                    })
                    .collect();
            }
            self.check_alive()?;
            if started.elapsed() > Duration::from_secs(120) {
                bail!("REAPER gave no answer to {commands:?}");
            }
            sleep(Duration::from_millis(50));
        }
    }

    /// Runs one command in REAPER and returns its result.
    pub(crate) fn one(&mut self, command: &str) -> Result<String> {
        self.send(&[command])?
            .pop()
            .ok_or_else(|| eyre!("no result for {command}"))
    }

    /// Stops REAPER with no save.
    pub(crate) fn stop(mut self) -> Result<()> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn line_count(path: &Path) -> usize {
    fs::read_to_string(path).map_or(0, |text| text.matches('\n').count())
}

fn ready_count(work: &Path) -> usize {
    fs::read_to_string(work.join("out.txt")).map_or(0, |text| {
        text.lines().filter(|line| *line == "ok ready").count()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vst3_folder_follows_the_cpu() {
        assert_eq!(vst3_arch_dir("x86_64"), "x86_64-linux");
        assert_eq!(vst3_arch_dir("aarch64"), "aarch64-linux");
        assert_eq!(vst3_arch_dir("riscv64"), "riscv64-linux");
    }
}
