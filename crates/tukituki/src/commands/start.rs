use std::process::ExitCode;

use serde::Serialize;

use crate::cli::Cli;
use crate::runtime;

/// JSON shape for start/stop/restart. Matches Go's `actionResult` struct.
#[derive(Serialize)]
pub(crate) struct ActionResult<'a> {
    pub name: &'a str,
    pub status: &'a str,
}

pub fn run(cli: &Cli, target: Option<&str>) -> ExitCode {
    let run_dir = runtime::resolve_run_dir(cli);
    let state_dir = runtime::resolve_state_dir(cli);
    let project_root = runtime::resolve_project_root();

    let targets = match runtime::load_targets_or_die(&run_dir, &project_root, cli.json) {
        Ok(t) => t,
        Err(code) => return code,
    };
    let mgr = match runtime::new_manager_or_die(targets.clone(), &state_dir, &project_root, cli) {
        Ok(m) => m,
        Err(code) => return code,
    };

    if let Some(name) = target {
        if let Err(code) = runtime::find_target_or_die(&targets, name, cli.json) {
            return code;
        }
        if let Err(e) = mgr.start(name) {
            runtime::exit_error(cli.json, &format!("start {name:?}: {e}"), &[]);
            return ExitCode::from(1);
        }
        return print_one(&mgr, name, cli.json);
    }

    if let Err(e) = mgr.start_all() {
        runtime::exit_error(cli.json, &format!("start all: {e}"), &[]);
        return ExitCode::from(1);
    }

    // Start the OTel collector if any target has `otel: true`. Non-fatal:
    // if the spawn fails the rest of the start has already succeeded;
    // surface the error to stderr and continue (matches Go).
    if let Err(e) = mgr.ensure_otel_collector() {
        eprintln!("Warning: could not start OTel collector: {e}");
    }

    let all_targets = mgr.get_targets();
    if cli.json {
        let statuses = mgr.get_all_statuses();
        let results: Vec<ActionResult<'_>> = all_targets
            .iter()
            .map(|t| ActionResult {
                name: &t.name,
                status: runtime::status_str(
                    statuses
                        .get(&t.name)
                        .copied()
                        .unwrap_or(tukituki_state::Status::Unknown),
                ),
            })
            .collect();
        if let Err(e) = runtime::write_json(&results) {
            runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
            return ExitCode::from(1);
        }
    } else {
        let statuses = mgr.get_all_statuses();
        for t in &all_targets {
            match statuses.get(&t.name) {
                Some(s) => println!("Started: {} (status: {})", t.name, runtime::status_str(*s)),
                None => println!("Started: {}", t.name),
            }
        }
    }
    ExitCode::SUCCESS
}

fn print_one(mgr: &tukituki_process::Manager, name: &str, json: bool) -> ExitCode {
    let statuses = mgr.get_all_statuses();
    let s = statuses
        .get(name)
        .copied()
        .unwrap_or(tukituki_state::Status::Unknown);
    if json {
        let result = ActionResult {
            name,
            status: runtime::status_str(s),
        };
        if let Err(e) = runtime::write_json(&result) {
            runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
            return ExitCode::from(1);
        }
    } else {
        println!("Started: {name} (status: {})", runtime::status_str(s));
    }
    ExitCode::SUCCESS
}
