//! `tukituki` with no subcommand: open the interactive TUI.

use std::io::IsTerminal;
use std::process::ExitCode;

use crate::cli::Cli;
use crate::runtime;

pub fn run(cli: &Cli) -> ExitCode {
    if !std::io::stdout().is_terminal() {
        runtime::exit_error(
            cli.json,
            "no terminal detected — the default command opens an interactive TUI and requires a TTY",
            &[(
                "hint",
                serde_json::Value::String(
                    "use a subcommand for non-interactive use: list, status, start, stop, restart, logs"
                        .into(),
                ),
            )],
        );
        return ExitCode::from(1);
    }

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

    // Start every target that isn't already alive, then bring up the
    // OTel collector if any target wants it. Errors are non-fatal: the
    // TUI still opens so the user can see and fix the problem.
    if let Err(e) = mgr.start_all() {
        eprintln!("Warning: start all: {e}");
    }
    if let Err(e) = mgr.ensure_otel_collector() {
        eprintln!("Warning: ensure otel collector: {e}");
    }
    if let Err(e) = mgr.attach_to_existing() {
        eprintln!("Warning: attach: {e}");
    }

    let final_targets = mgr.get_targets();

    let outcome = match tukituki_tui::start(final_targets, mgr.clone(), run_dir, project_root) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error: TUI: {e}");
            return ExitCode::from(1);
        }
    };

    if outcome.stop_all
        && let Err(e) = mgr.stop_all()
    {
        eprintln!("Warning: stop all: {e}");
    }
    ExitCode::SUCCESS
}
