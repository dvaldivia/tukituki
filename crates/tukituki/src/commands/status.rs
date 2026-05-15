use std::io::Write;
use std::process::ExitCode;

use serde::Serialize;
use tabwriter::TabWriter;
use tukituki_config::RunTarget;
use tukituki_state::Status;

use crate::cli::Cli;
use crate::runtime;

/// Status JSON shape — declaration order, omitempty semantics match Go's
/// `statusEntry` in `cmd/tukituki/root.go`.
#[derive(Serialize)]
struct StatusEntry<'a> {
    name: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pid: i32,
    #[serde(skip_serializing_if = "str::is_empty")]
    address: &'a str,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
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
    if let Err(e) = mgr.attach_to_existing() {
        eprintln!("Warning: could not attach to existing processes: {e}");
    }

    let mut targets = mgr.get_targets();
    if let Some(name) = target {
        match runtime::find_target_or_die(&targets, name, cli.json) {
            Ok(t) => targets = vec![t],
            Err(code) => return code,
        }
    }

    let statuses = mgr.get_all_statuses();
    let states = mgr.get_all_process_states();
    let otel_port = mgr.otel_receiver_port();

    if cli.json {
        let address_strings: Vec<String> = targets
            .iter()
            .map(|t| {
                let status = statuses.get(&t.name).copied().unwrap_or(Status::Unknown);
                if t.name == tukituki_process::OTEL_TARGET_NAME
                    && otel_port != 0
                    && status == Status::Running
                {
                    format!("127.0.0.1:{otel_port}")
                } else {
                    String::new()
                }
            })
            .collect();

        let entries: Vec<StatusEntry<'_>> = targets
            .iter()
            .zip(&address_strings)
            .map(|(t, addr)| {
                let status = statuses.get(&t.name).copied().unwrap_or(Status::Unknown);
                let pid = states.get(&t.name).map(|ps| ps.pid).unwrap_or(0);
                StatusEntry {
                    name: &t.name,
                    status: runtime::status_str(status),
                    description: &t.description,
                    pid,
                    address: addr.as_str(),
                }
            })
            .collect();

        // Single-target query → object, not array (matches Go).
        let render_ok = if target.is_some() && entries.len() == 1 {
            runtime::write_json(&entries[0])
        } else {
            runtime::write_json(&entries)
        };
        if let Err(e) = render_ok {
            runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }

    print_status_table(&targets, &statuses, otel_port);
    ExitCode::SUCCESS
}

fn print_status_table(
    targets: &[RunTarget],
    statuses: &std::collections::BTreeMap<String, Status>,
    otel_port: u16,
) {
    let stdout = std::io::stdout();
    let handle = stdout.lock();
    let mut tw = TabWriter::new(handle).minwidth(0).padding(3);
    let _ = writeln!(tw, "NAME\tSTATUS\tDESCRIPTION");
    let _ = writeln!(tw, "----\t------\t-----------");
    for t in targets {
        let status = statuses.get(&t.name).copied().unwrap_or(Status::Unknown);
        let status_str = runtime::status_str(status);
        let mut desc = if t.description.is_empty() {
            "-".to_string()
        } else {
            t.description.clone()
        };
        if t.name == tukituki_process::OTEL_TARGET_NAME
            && otel_port != 0
            && status == Status::Running
        {
            desc = format!("{desc} (listening on :{otel_port})");
        }
        let _ = writeln!(tw, "{}\t{}\t{}", t.name, status_str, desc);
    }
    let _ = tw.flush();
}
