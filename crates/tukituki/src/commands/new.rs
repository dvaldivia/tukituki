use std::fs;
use std::io::ErrorKind;
use std::process::ExitCode;

use crate::cli::Cli;
use crate::runtime;

/// Scaffold `.run/<name>.yaml` from a name + command string.
///
/// Mirrors the Go `tukituki new` subcommand: split the command on
/// whitespace, optional `-e KEY=VALUE` and `-w <dir>`, refuse to
/// overwrite an existing file.
pub fn run(
    cli: &Cli,
    name: &str,
    command: &str,
    env_vars: &[String],
    workdir: Option<&str>,
) -> ExitCode {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        runtime::exit_error(cli.json, "command string is empty", &[]);
        return ExitCode::from(1);
    }
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name == "."
        || name == ".."
    {
        runtime::exit_error(cli.json, &format!("invalid target name {name:?}"), &[]);
        return ExitCode::from(1);
    }

    let run_dir = runtime::resolve_run_dir(cli);
    if let Err(e) = fs::create_dir_all(&run_dir) {
        runtime::exit_error(cli.json, &format!("create run directory: {e}"), &[]);
        return ExitCode::from(1);
    }

    let file_path = run_dir.join(format!("{name}.yaml"));
    match fs::metadata(&file_path) {
        Ok(_) => {
            runtime::exit_error(
                cli.json,
                &format!("file already exists: {}", file_path.display()),
                &[],
            );
            return ExitCode::from(1);
        }
        Err(e) if e.kind() != ErrorKind::NotFound => {
            runtime::exit_error(cli.json, &format!("stat {}: {e}", file_path.display()), &[]);
            return ExitCode::from(1);
        }
        Err(_) => {}
    }

    let mut buf = String::new();
    buf.push_str(&format!("name: {name}\n"));
    buf.push_str(&format!("command: {}\n", parts[0]));
    if let Some(dir) = workdir.filter(|s| !s.is_empty()) {
        buf.push_str(&format!("workdir: {dir}\n"));
    }
    if parts.len() > 1 {
        buf.push_str("args:\n");
        for a in &parts[1..] {
            buf.push_str(&format!("  - {a}\n"));
        }
    }
    if !env_vars.is_empty() {
        buf.push_str("env:\n");
        for e in env_vars {
            let Some((k, v)) = e.split_once('=') else {
                runtime::exit_error(
                    cli.json,
                    &format!("invalid env var format {e:?} (expected KEY=VALUE)"),
                    &[],
                );
                return ExitCode::from(1);
            };
            buf.push_str(&format!("  {k}: {}\n", yaml_double_quote(v)));
        }
    }

    if let Err(e) = fs::write(&file_path, buf) {
        runtime::exit_error(cli.json, &format!("write file: {e}"), &[]);
        return ExitCode::from(1);
    }

    if cli.json {
        let payload = serde_json::json!({
            "file": file_path.display().to_string(),
            "name": name,
        });
        if let Err(e) = runtime::write_json(&payload) {
            runtime::exit_error(true, &format!("marshal JSON: {e}"), &[]);
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }

    println!("Created {}", file_path.display());
    ExitCode::SUCCESS
}

fn yaml_double_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
