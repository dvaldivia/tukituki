use std::io::Write;
use std::process::ExitCode;

use serde::Serialize;
use tabwriter::TabWriter;

use crate::cli::Cli;
use crate::runtime;

/// JSON shape for `list --json`. Field declaration order and
/// `skip_serializing_if` flags mirror Go's `listEntry` struct in
/// `cmd/tukituki/root.go`, so the byte output stays drop-in compatible.
#[derive(Serialize)]
struct ListEntry<'a> {
    name: &'a str,
    command: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    args: &'a [String],
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    workdir: &'a str,
}

pub fn run(cli: &Cli) -> ExitCode {
    let run_dir = runtime::resolve_run_dir(cli);
    let project_root = runtime::resolve_project_root();

    let targets = match runtime::load_targets_or_die(&run_dir, &project_root, cli.json) {
        Ok(t) => t,
        Err(code) => return code,
    };

    if cli.json {
        let entries: Vec<ListEntry<'_>> = targets
            .iter()
            .map(|t| ListEntry {
                name: &t.name,
                command: &t.command,
                args: &t.args,
                description: &t.description,
                workdir: &t.workdir,
            })
            .collect();
        if let Err(e) = runtime::write_json(&entries) {
            runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }

    // Text mode: tabwriter with the same column padding (3) as Go's
    // text/tabwriter.NewWriter(os.Stdout, 0, 0, 3, ' ', 0).
    let stdout = std::io::stdout();
    let handle = stdout.lock();
    let mut tw = TabWriter::new(handle).minwidth(0).padding(3);
    let _ = writeln!(tw, "NAME\tCOMMAND\tDESCRIPTION");
    let _ = writeln!(tw, "----\t-------\t-----------");
    for t in &targets {
        let desc = if t.description.is_empty() {
            "-"
        } else {
            t.description.as_str()
        };
        let _ = writeln!(tw, "{}\t{}\t{}", t.name, t.command, desc);
    }
    let _ = tw.flush();
    ExitCode::SUCCESS
}
