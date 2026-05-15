use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cli::Cli;
use crate::runtime;

/// `tukituki logs <target> [--follow] [--tail N]`.
///
/// Without `--follow`: reads the on-disk log file directly, strips null
/// bytes, drops a trailing empty split, and prints the last `tail` lines
/// (default 100, `0` = all). Faithful port of the Go behaviour — the
/// async tailer may not have populated the ring buffer yet by the time
/// a short-lived agent invocation runs, so we go to disk.
///
/// With `--follow`: prints the buffered ring-buffer lines (subject to
/// `tail`), then subscribes to the manager's broadcast and streams new
/// lines until Ctrl+C, channel close, or EOF on stdin.
pub fn run(cli: &Cli, target: &str, follow: bool, tail: usize) -> ExitCode {
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

    if let Err(code) = runtime::find_target_or_die(&targets, target, cli.json) {
        return code;
    }

    if !follow {
        // One-shot: read the file directly. The Go binary does the same;
        // an agent might invoke `tukituki logs` faster than the async
        // tailer can populate the ring buffer.
        return print_file_tail(&mgr, target, tail);
    }

    // Follow path: print buffered lines first, then stream new ones.
    let stdout = std::io::stdout();
    let mut w = BufWriter::new(stdout.lock());

    let buffered = mgr.get_log_lines(target);
    let start = if tail > 0 && buffered.len() > tail {
        buffered.len() - tail
    } else {
        0
    };
    for line in &buffered[start..] {
        if writeln!(w, "{line}").is_err() {
            return ExitCode::SUCCESS;
        }
    }
    let _ = w.flush();

    // Subscribe + install Ctrl+C handler that flips a flag the loop
    // checks. Avoids the need for an async runtime.
    let rx = mgr.watch_log_lines(target);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc::set_handler(move || {
            stop.store(true, Ordering::SeqCst);
        });
    }

    loop {
        if stop.load(Ordering::SeqCst) {
            return ExitCode::SUCCESS;
        }
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(line) => {
                if writeln!(w, "{line}").is_err() {
                    return ExitCode::SUCCESS;
                }
                let _ = w.flush();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Re-check the cancel flag.
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Manager dropped the sender — process is gone.
                return ExitCode::SUCCESS;
            }
        }
    }
}

fn print_file_tail(mgr: &tukituki_process::Manager, target: &str, tail: usize) -> ExitCode {
    let Some(path) = mgr.log_file_path(target) else {
        // No state for this target — print nothing, exit cleanly.
        return ExitCode::SUCCESS;
    };
    let data = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: read log file: {e}");
            return ExitCode::from(1);
        }
    };

    print_buffer(&data, tail, &path)
}

fn print_buffer(data: &[u8], tail: usize, _path: &Path) -> ExitCode {
    // Strip null bytes — children can emit them and downstream consumers
    // (TUI renderers, agent prompt parsers) choke.
    let filtered: Vec<u8> = data.iter().copied().filter(|b| *b != 0).collect();
    let content = String::from_utf8_lossy(&filtered).into_owned();
    let mut lines: Vec<&str> = content.split('\n').collect();
    if let Some(last) = lines.last()
        && last.is_empty()
    {
        lines.pop();
    }
    let start = if tail > 0 && lines.len() > tail {
        lines.len() - tail
    } else {
        0
    };
    let stdout = std::io::stdout();
    let mut w = BufWriter::new(stdout.lock());
    for line in &lines[start..] {
        if writeln!(w, "{line}").is_err() {
            return ExitCode::SUCCESS;
        }
    }
    let _ = w.flush();
    ExitCode::SUCCESS
}
