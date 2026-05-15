//! Cross-cutting helpers shared by subcommands: path resolution,
//! target loading + dotenv expansion, JSON error reporting.
//!
//! Mirrors the helpers in Go's `cmd/tukituki/root.go` (`loadTargetsOrDie`,
//! `exitError`, `resolveRunDir`, etc.).

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use tukituki_config::RunTarget;
use tukituki_process::{Manager, OtelConfig};
use tukituki_state::Status;

/// Effective `.run/` directory: CLI flag → env → rc file → default.
/// `cli.run_dir` already collapses flag+env (clap resolves both).
pub fn resolve_run_dir(cli: &crate::cli::Cli) -> PathBuf {
    if let Some(s) = cli.run_dir.as_deref()
        && !s.is_empty()
    {
        return PathBuf::from(s);
    }
    let rc = crate::rcfile::load(cli.config.as_deref());
    if let Some(s) = rc.run_dir.as_deref()
        && !s.is_empty()
    {
        return PathBuf::from(s);
    }
    PathBuf::from(".run")
}

/// Effective state directory: CLI flag → env → rc file → `.tukituki`.
pub fn resolve_state_dir(cli: &crate::cli::Cli) -> PathBuf {
    if let Some(s) = cli.state_dir.as_deref()
        && !s.is_empty()
    {
        return PathBuf::from(s);
    }
    let rc = crate::rcfile::load(cli.config.as_deref());
    if let Some(s) = rc.state_dir.as_deref()
        && !s.is_empty()
    {
        return PathBuf::from(s);
    }
    PathBuf::from(".tukituki")
}

/// Effective OTel protocol: CLI/env → rc file → `grpc`.
pub fn resolve_otel_protocol(cli: &crate::cli::Cli) -> String {
    if let Some(s) = cli.otel_protocol.as_deref()
        && !s.is_empty()
    {
        return s.to_string();
    }
    let rc = crate::rcfile::load(cli.config.as_deref());
    if let Some(s) = rc.otel_protocol.as_deref()
        && !s.is_empty()
    {
        return s.to_string();
    }
    "grpc".to_string()
}

/// Effective OTel severity: CLI/env → rc file → `error`.
pub fn resolve_otel_severity(cli: &crate::cli::Cli) -> String {
    if let Some(s) = cli.otel_severity.as_deref()
        && !s.is_empty()
    {
        return s.to_string();
    }
    let rc = crate::rcfile::load(cli.config.as_deref());
    if let Some(s) = rc.otel_severity.as_deref()
        && !s.is_empty()
    {
        return s.to_string();
    }
    "error".to_string()
}

/// Effective OTel port: CLI/env → rc file → `0` (Manager will allocate).
pub fn resolve_otel_port(cli: &crate::cli::Cli) -> u16 {
    if let Some(p) = cli.otel_port {
        return p;
    }
    let rc = crate::rcfile::load(cli.config.as_deref());
    rc.otel_port.unwrap_or(0)
}

/// Absolute project root — the cwd at invocation. Workdirs in
/// `.run/*.yaml` are resolved relative to this.
pub fn resolve_project_root() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// `loadTargetsOrDie` analogue.  On a missing run-dir, emits the same
/// helpful message + `run_dir` JSON field the Go binary produces and
/// returns `Err(ExitCode)` for the caller to propagate. Other errors
/// are wrapped with `loading targets:`.
pub fn load_targets_or_die(
    run_dir: &std::path::Path,
    project_root: &std::path::Path,
    json: bool,
) -> Result<Vec<RunTarget>, ExitCode> {
    let targets = match tukituki_config::load_targets(run_dir) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            exit_error(
                json,
                &format!(
                    "no .run/ directory found at {:?} — create .run/*.yaml files to define your processes",
                    run_dir.display().to_string()
                ),
                &[(
                    "run_dir",
                    serde_json::Value::String(run_dir.display().to_string()),
                )],
            );
            return Err(ExitCode::from(1));
        }
        Err(e) => {
            exit_error(json, &format!("loading targets: {e}"), &[]);
            return Err(ExitCode::from(1));
        }
    };

    let dotenv = match tukituki_config::load_dotenv(project_root) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Warning: could not parse .env: {e}");
            None
        }
    };
    Ok(tukituki_config::expand_env(targets, dotenv.as_ref()))
}

/// Write a JSON object as pretty-printed JSON on stdout, followed by a
/// newline (Go's `writeJSON` ends with `fmt.Println`).
pub fn write_json<T: Serialize>(v: &T) -> std::io::Result<()> {
    let s = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    println!("{s}");
    Ok(())
}

/// Build a Manager rooted in `state_dir`, applying the OTel config
/// from CLI flags. Exits with a JSON-aware error if the state dir
/// can't be created.
pub fn new_manager_or_die(
    targets: Vec<RunTarget>,
    state_dir: &std::path::Path,
    project_root: &std::path::Path,
    cli: &crate::cli::Cli,
) -> Result<Manager, ExitCode> {
    let mgr = match Manager::new(targets, state_dir, project_root) {
        Ok(m) => m,
        Err(e) => {
            exit_error(cli.json, &format!("creating process manager: {e}"), &[]);
            return Err(ExitCode::from(1));
        }
    };
    mgr.set_otel_config(OtelConfig {
        port: resolve_otel_port(cli),
        protocol: resolve_otel_protocol(cli),
        severity: resolve_otel_severity(cli),
    });
    Ok(mgr)
}

/// Look up a target by name. On miss, exit with a JSON-aware error
/// listing available names — same shape Go's `findTarget` produces.
pub fn find_target_or_die(
    targets: &[RunTarget],
    name: &str,
    json: bool,
) -> Result<RunTarget, ExitCode> {
    if let Some(t) = targets.iter().find(|t| t.name == name) {
        return Ok(t.clone());
    }
    // Recognise the virtual otel-errors target even when it isn't in
    // the YAML-loaded list — the Manager registers it at runtime.
    if name == tukituki_process::OTEL_TARGET_NAME {
        return Ok(RunTarget {
            name: name.into(),
            ..Default::default()
        });
    }
    let available: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    exit_error(
        json,
        &format!("target {name:?} not found"),
        &[(
            "available",
            serde_json::Value::Array(
                available
                    .into_iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect(),
            ),
        )],
    );
    Err(ExitCode::from(1))
}

/// Lower-snake status string (matches Go's `string(state.Status)`).
pub fn status_str(s: Status) -> &'static str {
    match s {
        Status::Running => "running",
        Status::Stopped => "stopped",
        Status::Failed => "failed",
        Status::Unknown => "unknown",
    }
}

/// `exitError` analogue.  Writes to stderr, never stdout.  In JSON
/// mode it emits a single-line `{"error": "...", ...extra}` object;
/// otherwise a plain `Error: ...` line.  Does **not** exit — the
/// caller returns the ExitCode so destructors run.
pub fn exit_error(json: bool, msg: &str, extra: &[(&str, serde_json::Value)]) {
    if json {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "error".to_string(),
            serde_json::Value::String(msg.to_string()),
        );
        for (k, v) in extra {
            obj.insert((*k).to_string(), v.clone());
        }
        let s = serde_json::to_string(&serde_json::Value::Object(obj))
            .unwrap_or_else(|_| String::from(r#"{"error":"unprintable"}"#));
        eprintln!("{s}");
    } else {
        eprintln!("Error: {msg}");
    }
}
