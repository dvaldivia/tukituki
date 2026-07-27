//! Ledger of every OTel collector this project has spawned.
//!
//! `state.json` holds exactly one `otel-errors` entry, so the moment a
//! replacement collector is recorded the previous PID is forgotten. If
//! that previous process was still alive — the recorded status drifted
//! from reality, or the state file was reset out from under us — nothing
//! could ever signal it again, and it survived every later stop. Users
//! ended up with several collectors per project, each holding its own
//! OTLP port.
//!
//! `<state-dir>/otel-pids` is an append-only list of decimal PIDs, one
//! per line, that closes that gap: it remembers what we spawned even
//! after `state.json` has moved on.
//!
//! Scoping matters here. The collector's `--notify-socket` argument is
//! a *relative* path (`.tukituki/otel-notify.sock`), so it is identical
//! across every project on the machine and cannot be used to tell one
//! project's collectors from another's. Keeping the ledger inside the
//! project's own state directory makes ownership structural: a sweep
//! can only ever see PIDs this project wrote.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("otel-pids")
}

/// Read the recorded PIDs. Missing or unparseable entries are skipped
/// rather than failing the caller — a corrupt ledger must never block
/// stopping the collector we *can* account for.
pub fn load(state_dir: &Path) -> Vec<i32> {
    let Ok(data) = fs::read_to_string(path(state_dir)) else {
        return Vec::new();
    };
    data.lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .filter(|p| *p > 0)
        .collect()
}

/// Record `pid` as a collector belonging to this project. De-duplicates
/// so repeated starts on a recycled PID don't grow the file without
/// bound. Best-effort, like [`crate::otel_port::save`]: a ledger we
/// can't write must not fail the spawn that just succeeded.
pub fn record(state_dir: &Path, pid: i32) {
    if pid <= 0 {
        return;
    }
    let mut pids: BTreeSet<i32> = load(state_dir).into_iter().collect();
    if !pids.insert(pid) {
        return;
    }
    write(state_dir, &pids.into_iter().collect::<Vec<_>>());
}

/// Replace the ledger with `pids`.
pub fn write(state_dir: &Path, pids: &[i32]) {
    let p = path(state_dir);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body: String = pids
        .iter()
        .map(|p| format!("{p}\n"))
        .collect::<Vec<_>>()
        .join("");
    let _ = fs::write(&p, body);
}

/// Drop the ledger entirely. Paired with [`crate::otel_port::remove`]
/// when the collector is deliberately stopped.
pub fn remove(state_dir: &Path) {
    let _ = fs::remove_file(path(state_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), 42);
        record(dir.path(), 7);
        let mut got = load(dir.path());
        got.sort();
        assert_eq!(got, vec![7, 42]);
    }

    #[test]
    fn record_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), 99);
        record(dir.path(), 99);
        assert_eq!(load(dir.path()), vec![99]);
    }

    #[test]
    fn load_missing_or_garbage_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_empty(), "missing file => no pids");
        fs::write(dir.path().join("otel-pids"), "not-a-pid\n\n-5\n12\n").unwrap();
        assert_eq!(load(dir.path()), vec![12], "skip junk, keep valid pids");
    }

    #[test]
    fn remove_clears_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), 5);
        remove(dir.path());
        assert!(load(dir.path()).is_empty());
    }
}
