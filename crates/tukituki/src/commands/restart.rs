use std::process::ExitCode;

use crate::cli::Cli;
use crate::commands::start::ActionResult;
use crate::runtime;

pub fn run(cli: &Cli, args: &[String]) -> ExitCode {
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

    let names: Vec<String> = if args.is_empty() {
        targets.iter().map(|t| t.name.clone()).collect()
    } else {
        args.to_vec()
    };

    // Validate every name up front so a typo on the last arg doesn't
    // leave earlier targets bounced. Matches Go behaviour.
    for name in &names {
        if let Err(code) = runtime::find_target_or_die(&targets, name, cli.json) {
            return code;
        }
    }

    for name in &names {
        if let Err(e) = mgr.restart(name) {
            runtime::exit_error(cli.json, &format!("restart {name:?}: {e}"), &[]);
            return ExitCode::from(1);
        }
    }

    let statuses = mgr.get_all_statuses();
    if cli.json {
        let results: Vec<ActionResult<'_>> = names
            .iter()
            .map(|n| ActionResult {
                name: n.as_str(),
                status: runtime::status_str(
                    statuses
                        .get(n)
                        .copied()
                        .unwrap_or(tukituki_state::Status::Unknown),
                ),
            })
            .collect();
        let render_ok = if results.len() == 1 && args.len() == 1 {
            runtime::write_json(&results[0])
        } else {
            runtime::write_json(&results)
        };
        if let Err(e) = render_ok {
            runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
            return ExitCode::from(1);
        }
    } else {
        for n in &names {
            match statuses.get(n) {
                Some(s) => println!("Restarted: {n} (status: {})", runtime::status_str(*s)),
                None => println!("Restarted: {n}"),
            }
        }
    }
    ExitCode::SUCCESS
}
