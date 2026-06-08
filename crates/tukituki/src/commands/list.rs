use std::io::Write;
use std::process::ExitCode;

use serde::Serialize;
use tabwriter::TabWriter;

use crate::cli::Cli;
use crate::runtime;

/// JSON shape for `list --json`. Field declaration order and
/// `skip_serializing_if` flags mirror Go's `listEntry` struct in
/// `cmd/tukituki/root.go` (with `tags` appended as a new optional field).
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
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    tags: &'a [String],
}

pub fn run(cli: &Cli, tags: &[String]) -> ExitCode {
    let run_dir = runtime::resolve_run_dir(cli);
    let project_root = runtime::resolve_project_root();

    let targets = match runtime::load_targets_or_die(&run_dir, &project_root, cli.json) {
        Ok(t) => t,
        Err(code) => return code,
    };

    let targets = tukituki_config::filter_targets_by_tags(&targets, tags);
    if !tags.is_empty() && targets.is_empty() {
        // Still succeed for list; empty output is clear.
    }

    if cli.json {
        let entries: Vec<ListEntry<'_>> = targets
            .iter()
            .map(|t| ListEntry {
                name: &t.name,
                command: &t.command,
                args: &t.args,
                description: &t.description,
                workdir: &t.workdir,
                tags: &t.tags,
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
    let _ = writeln!(tw, "NAME\tCOMMAND\tTAGS\tDESCRIPTION");
    let _ = writeln!(tw, "----\t-------\t----\t-----------");
    for t in &targets {
        let desc = if t.description.is_empty() {
            "-"
        } else {
            t.description.as_str()
        };
        let tags = if t.tags.is_empty() {
            "-"
        } else {
            &t.tags.join(",")
        };
        let _ = writeln!(tw, "{}\t{}\t{}\t{}", t.name, t.command, tags, desc);
    }
    let _ = tw.flush();
    ExitCode::SUCCESS
}
