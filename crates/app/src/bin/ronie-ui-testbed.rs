//! Neutral UI scenario and native foundation runner.

use std::{path::PathBuf, time::Instant};

use ronie_ui::{
    native_scenario::run_logic_selftest,
    shell::{NativeShellOptions, run_native},
    testbed::{UiScenario, load_scenarios, run, run_all},
};
use serde_json::json;

fn scenarios() -> Result<Vec<UiScenario>, String> {
    load_scenarios(include_str!("../../../../fixtures/ui-scenarios.json"))
        .map_err(|error| error.to_string())
}

fn select(id: Option<&str>) -> Result<Vec<UiScenario>, String> {
    let scenarios = scenarios()?;
    if let Some(id) = id {
        let scenario = scenarios
            .into_iter()
            .find(|scenario| scenario.id == id)
            .ok_or_else(|| format!("unknown scenario: {id}"))?;
        Ok(vec![scenario])
    } else {
        Ok(scenarios)
    }
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn parse_window_size(text: &str) -> Result<[f32; 2], String> {
    let (width, height) = text
        .split_once('x')
        .ok_or_else(|| format!("window size needs WIDTHxHEIGHT: {text}"))?;
    let parse = |value: &str| {
        let side = value
            .parse::<f32>()
            .map_err(|error| format!("window size {text}: {error}"))?;
        if side.is_finite() && side >= 1.0 {
            Ok(side)
        } else {
            Err(format!("window size {text}: each side must be 1 or more"))
        }
    };
    Ok([parse(width)?, parse(height)?])
}

fn execute() -> Result<(), String> {
    let mut scenario = None;
    let mut capture = None;
    let mut compare = None;
    let mut native_scenario = None;
    let mut native_capture = None;
    let mut native_report = None;
    let mut native_window = None;
    let mut compare_directory = None;
    let mut selftest = false;
    let mut bench = false;
    let mut shell_selftest = false;
    let mut shell_capture = None;
    let mut shell_report = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--scenario" => {
                scenario = Some(args.next().ok_or("--scenario needs an ID")?);
            }
            "--capture" => {
                capture = Some(PathBuf::from(args.next().ok_or("--capture needs a path")?));
            }
            "--compare" => {
                compare = Some(PathBuf::from(args.next().ok_or("--compare needs a path")?));
            }
            "--native-scenario" => {
                native_scenario = Some(args.next().ok_or("--native-scenario needs an ID")?);
            }
            "--native-capture" => {
                native_capture = Some(PathBuf::from(
                    args.next().ok_or("--native-capture needs a path")?,
                ));
            }
            "--native-report" => {
                native_report = Some(PathBuf::from(
                    args.next().ok_or("--native-report needs a path")?,
                ));
            }
            "--native-window" => {
                native_window = Some(parse_window_size(
                    &args.next().ok_or("--native-window needs WIDTHxHEIGHT")?,
                )?);
            }
            "--compare-dir" => {
                let reference =
                    PathBuf::from(args.next().ok_or("--compare-dir needs a reference path")?);
                let candidate =
                    PathBuf::from(args.next().ok_or("--compare-dir needs a candidate path")?);
                let report = PathBuf::from(args.next().ok_or("--compare-dir needs a report path")?);
                compare_directory = Some((reference, candidate, report));
            }
            "--selftest" => selftest = true,
            "--bench" => bench = true,
            "--shell-selftest" => shell_selftest = true,
            "--shell-capture" => {
                shell_capture = Some(PathBuf::from(
                    args.next().ok_or("--shell-capture needs a path")?,
                ));
            }
            "--shell-report" => {
                shell_report = Some(PathBuf::from(
                    args.next().ok_or("--shell-report needs a path")?,
                ));
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if let Some(id) = native_scenario {
        let scenario = select(Some(&id))?.remove(0);
        let mut run = ronie_ui::native_scenario::NativeScenario::new(
            &scenario,
            native_capture,
            native_report,
        )?;
        if let Some(size) = native_window {
            run.use_window_size(size);
        }
        run.run().map_err(|error| error.to_string())?;
        return Ok(());
    }
    if let Some((reference, candidate, report)) = compare_directory {
        let scenarios = select(scenario.as_deref())?;
        ronie_ui::native_scenario::compare_suite(&reference, &candidate, &report, &scenarios)?;
        println!(
            "{}",
            serde_json::json!({"passed": true, "scenarios": scenarios.len()})
        );
        return Ok(());
    }
    if shell_selftest {
        println!("{}", run_logic_selftest());
    }
    if let Some(path) = shell_capture {
        run_native(NativeShellOptions {
            selftest: true,
            capture: Some(path),
            report: shell_report,
            ..NativeShellOptions::default()
        })
        .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if shell_selftest {
        return Ok(());
    }
    if compare.is_some() {
        return Err("visual comparison is unresolved until the native renderer and Reference captures exist".to_owned());
    }
    if !selftest && !bench && capture.is_none() && scenario.is_none() {
        return Err(
            "use --selftest, --bench, --scenario ID [--capture PATH], --compare PATH, --shell-selftest, or --shell-capture PATH".to_owned(),
        );
    }
    let selected = select(scenario.as_deref())?;
    if selftest || bench {
        let started = Instant::now();
        let report = run_all(&selected).map_err(|error| error.to_string())?;
        let elapsed = started.elapsed();
        println!(
            "{}",
            json!({
                "passed": report.passed,
                "failed": report.failed,
                "blocked": report.blocked,
                "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
                "backend_effects_verified": report.failed == 0,
            })
        );
        if !report.blocked.is_empty() {
            return Err(format!(
                "the real UI cannot finish: {}",
                report.blocked.join(", ")
            ));
        }
    }
    if let Some(ref path) = capture {
        let mut reports = Vec::new();
        for scenario in &selected {
            run(scenario).map_err(|error| error.to_string())?;
            reports.push(json!({
                "id": scenario.id,
                "backend_effects_verified": true,
                "visual_status": "unavailable_renderer_not_implemented",
                "viewport": {
                    "width": scenario.viewport.width,
                    "height": scenario.viewport.height,
                    "scale": scenario.viewport.scale,
                },
            }));
        }
        let bytes = serde_json::to_vec_pretty(&json!({"scenarios": reports}))
            .map_err(|error| error.to_string())?;
        std::fs::write(path, bytes).map_err(|error| error.to_string())?;
    }
    if scenario.is_some() && capture.is_none() && !selftest && !bench {
        for scenario in &selected {
            run(scenario).map_err(|error| error.to_string())?;
            println!(
                "{}",
                json!({"id": scenario.id, "backend_effects_verified": true})
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_window_size;

    #[test]
    fn a_window_size_needs_two_real_sides() {
        assert_eq!(parse_window_size("1024x658"), Ok([1024.0, 658.0]));
        for bad in [
            "NaNxNaN", "0x0", "-5x10", "10x-5", "infxinf", "1024", "x", "ax1",
        ] {
            assert!(parse_window_size(bad).is_err(), "{bad}");
        }
    }
}
