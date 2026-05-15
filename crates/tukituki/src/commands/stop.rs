use std::process::ExitCode;

use crate::cli::Cli;
use crate::commands::start::ActionResult;
use crate::runtime;

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
    if let Err(e) = mgr.attach_to_existing() {
        eprintln!("Warning: could not attach to existing processes: {e}");
    }

    if let Some(name) = target {
        if let Err(code) = runtime::find_target_or_die(&targets, name, cli.json) {
            return code;
        }
        if let Err(e) = mgr.stop(name) {
            runtime::exit_error(cli.json, &format!("stop {name:?}: {e}"), &[]);
            return ExitCode::from(1);
        }
        if cli.json {
            let r = ActionResult {
                name,
                status: "stopped",
            };
            if let Err(e) = runtime::write_json(&r) {
                runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
                return ExitCode::from(1);
            }
        } else {
            println!("Stopped: {name}");
        }
        return ExitCode::SUCCESS;
    }

    if let Err(e) = mgr.stop_all() {
        runtime::exit_error(cli.json, &format!("stop all: {e}"), &[]);
        return ExitCode::from(1);
    }

    if cli.json {
        let results: Vec<ActionResult<'_>> = targets
            .iter()
            .map(|t| ActionResult {
                name: &t.name,
                status: "stopped",
            })
            .collect();
        if let Err(e) = runtime::write_json(&results) {
            runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
            return ExitCode::from(1);
        }
    } else {
        for t in &targets {
            println!("Stopped: {}", t.name);
        }
    }
    ExitCode::SUCCESS
}
