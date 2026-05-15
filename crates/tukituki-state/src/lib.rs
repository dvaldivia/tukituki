//! On-disk state file + process-liveness check.
//!
//! Direct port of `internal/state`. The JSON shape must stay
//! byte-stable so a Go-built tukituki and a Rust-built tukituki can
//! attach to the same `.tukituki/state.json` during the transition.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};

/// Lifecycle status of a managed process. Serialised in lower-snake
/// form matching the Go `Status` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Stopped,
    Failed,
    #[default]
    Unknown,
}

/// Runtime information for a single managed process.  Field names and
/// `omitempty` behaviour are tuned to match Go's `json` tags exactly so
/// a state file produced by either binary round-trips through the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessState {
    pub name: String,
    pub pid: i32,
    pub log_file: String,
    pub started_at: DateTime<Utc>,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exit_code: Option<i32>,
}

/// Top-level persisted document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    /// BTreeMap for stable key ordering on save — Go uses a map, but
    /// `encoding/json` sorts map keys alphabetically before writing.
    pub processes: BTreeMap<String, ProcessState>,
    pub updated_at: DateTime<Utc>,
    /// Path to this file on disk; populated by [`State::load`] /
    /// [`State::new`], not serialised. Matches Go's `StateFile` with
    /// `json:"-"`.
    #[serde(skip)]
    pub state_file: PathBuf,
}

impl State {
    /// Build an empty in-memory State pointed at `state_file`. Use
    /// [`State::load`] to also read from disk.
    pub fn new<P: Into<PathBuf>>(state_file: P) -> Self {
        Self {
            processes: BTreeMap::new(),
            updated_at: chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default(),
            state_file: state_file.into(),
        }
    }

    /// Load state from `state_file` if it exists. Missing or corrupt
    /// files yield an empty State pointed at the same path — same
    /// fail-soft behaviour as the Go binary.
    pub fn load<P: Into<PathBuf>>(state_file: P) -> Self {
        let state_file = state_file.into();
        let Ok(data) = fs::read(&state_file) else {
            return Self::new(state_file);
        };
        match serde_json::from_slice::<State>(&data) {
            Ok(mut s) => {
                s.state_file = state_file;
                s
            }
            Err(_) => Self::new(state_file),
        }
    }

    /// Atomically persist the state to `self.state_file`. Writes to a
    /// sibling tempfile then renames, so a crash mid-write can never
    /// leave a half-written `state.json`.
    pub fn save(&mut self) -> io::Result<()> {
        self.updated_at = Utc::now();

        let mut data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        // Go's json.MarshalIndent does not append a trailing newline,
        // and Go writes the bytes verbatim. Match exactly.
        data.shrink_to_fit();

        let dir = self.state_file.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "state_file has no parent dir")
        })?;
        fs::create_dir_all(dir)?;

        let mut tmp = tempfile::Builder::new()
            .prefix(".state-")
            .suffix(".json.tmp")
            .tempfile_in(dir)?;
        tmp.write_all(&data)?;
        tmp.as_file_mut().sync_all()?;
        tmp.persist(&self.state_file)
            .map_err(|e| io::Error::other(format!("rename temp file: {e}")))?;

        Ok(())
    }

    /// Flip every `Running` process to `Stopped` when its PID is gone.
    pub fn reconcile_alive(&mut self) {
        for ps in self.processes.values_mut() {
            if ps.status == Status::Running && !is_alive(Some(ps)) {
                ps.status = Status::Stopped;
            }
        }
    }
}

/// Returns true if `ps` represents a process that is still alive.
///
/// Implementation detail: on Unix, we use `kill(pid, 0)` to probe the
/// PID. `Ok(_)` and `EPERM` mean "exists" (we may not own it); `ESRCH`
/// means "gone". `PID <= 0` is always treated as dead — Go's `os.FindProcess`
/// accepts negative PIDs but `kill(-1, 0)` would broadcast, which is never
/// what we want.
pub fn is_alive(ps: Option<&ProcessState>) -> bool {
    let Some(ps) = ps else {
        return false;
    };
    is_alive_pid(ps.pid)
}

/// Underlying liveness check, exposed for tests that don't want to
/// construct a full ProcessState.
pub fn is_alive_pid(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match kill(Pid::from_raw(pid), None) {
        Ok(_) => true,
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixed_time() -> DateTime<Utc> {
        // Use second-precision like the Go test, which Truncate(time.Second)s.
        DateTime::<Utc>::from_timestamp(1_715_792_096, 0).expect("valid ts")
    }

    #[test]
    fn state_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("state.json");

        let mut s = State::new(&state_file);
        s.processes.insert(
            "web".into(),
            ProcessState {
                name: "web".into(),
                pid: 12345,
                log_file: "/tmp/web.log".into(),
                started_at: fixed_time(),
                status: Status::Running,
                exit_code: Some(0),
            },
        );
        s.save().expect("save");

        assert!(Path::new(&state_file).exists(), "state file missing");

        let s2 = State::load(&state_file);
        let ps = s2.processes.get("web").expect("missing web");
        assert_eq!(ps.name, "web");
        assert_eq!(ps.pid, 12345);
        assert_eq!(ps.status, Status::Running);
        assert_eq!(ps.exit_code, Some(0));
    }

    #[test]
    fn state_save_load_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("state.json");
        fs::write(&state_file, b"not json {{{").unwrap();

        let s = State::load(&state_file);
        assert!(
            s.processes.is_empty(),
            "expected empty state on corrupt file, got {:?}",
            s.processes
        );
    }

    #[test]
    fn is_alive_running_process() {
        let pid = std::process::id() as i32;
        let ps = ProcessState {
            name: "self".into(),
            pid,
            log_file: String::new(),
            started_at: Utc::now(),
            status: Status::Running,
            exit_code: None,
        };
        assert!(is_alive(Some(&ps)));
    }

    #[test]
    fn is_alive_dead_process() {
        let ps = ProcessState {
            name: "dead".into(),
            pid: 0,
            log_file: String::new(),
            started_at: Utc::now(),
            status: Status::Running,
            exit_code: None,
        };
        assert!(!is_alive(Some(&ps)));
    }

    #[test]
    fn is_alive_nil_process() {
        assert!(!is_alive(None));
    }

    #[test]
    fn reconcile_alive_flips_dead_to_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("state.json");
        let mut s = State::new(&state_file);

        s.processes.insert(
            "dead".into(),
            ProcessState {
                name: "dead".into(),
                pid: 0,
                log_file: String::new(),
                started_at: Utc::now(),
                status: Status::Running,
                exit_code: None,
            },
        );
        s.processes.insert(
            "alive".into(),
            ProcessState {
                name: "alive".into(),
                pid: std::process::id() as i32,
                log_file: String::new(),
                started_at: Utc::now(),
                status: Status::Running,
                exit_code: None,
            },
        );
        s.processes.insert(
            "stopped".into(),
            ProcessState {
                name: "stopped".into(),
                pid: 0,
                log_file: String::new(),
                started_at: Utc::now(),
                status: Status::Stopped,
                exit_code: None,
            },
        );

        s.reconcile_alive();

        assert_eq!(s.processes["dead"].status, Status::Stopped);
        assert_eq!(s.processes["alive"].status, Status::Running);
        assert_eq!(s.processes["stopped"].status, Status::Stopped);
    }

    #[test]
    fn state_json_shape_matches_go() {
        // Lock the on-disk byte shape so a Go-built tukituki could load
        // the file we write. The critical bits:
        //   - 2-space indent (json.MarshalIndent("", "  "))
        //   - keys alphabetical within ProcessState
        //   - exit_code omitted when None
        //   - status serialised lowercase
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("state.json");
        let mut s = State::new(&state_file);
        s.processes.insert(
            "api".into(),
            ProcessState {
                name: "api".into(),
                pid: 42,
                log_file: "/tmp/api.log".into(),
                started_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
                status: Status::Running,
                exit_code: None,
            },
        );
        s.save().unwrap();
        let raw = fs::read_to_string(&state_file).unwrap();
        assert!(raw.contains("\"name\": \"api\""), "name field: {raw}");
        assert!(
            raw.contains("\"status\": \"running\""),
            "status case: {raw}"
        );
        assert!(
            !raw.contains("exit_code"),
            "exit_code must be omitted: {raw}"
        );
        // Go marshals structs in declaration order. ProcessState in the Go
        // binary is declared as: name, pid, log_file, started_at, status,
        // exit_code. Lock that exact byte order so a Go-built tukituki could
        // also load our file.
        let name_pos = raw.find("\"name\"").unwrap();
        let pid_pos = raw.find("\"pid\"").unwrap();
        let log_pos = raw.find("\"log_file\"").unwrap();
        let started_pos = raw.find("\"started_at\"").unwrap();
        let status_pos = raw.find("\"status\"").unwrap();
        assert!(
            name_pos < pid_pos
                && pid_pos < log_pos
                && log_pos < started_pos
                && started_pos < status_pos,
            "field order off: {raw}"
        );

        // Top-level State: declaration order is processes, updated_at.
        let proc_pos = raw.find("\"processes\"").unwrap();
        let updated_pos = raw.find("\"updated_at\"").unwrap();
        assert!(proc_pos < updated_pos, "top-level order off: {raw}");
    }
}
