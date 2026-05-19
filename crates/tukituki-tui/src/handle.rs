//! Trait the TUI uses to talk to the process manager.
//!
//! Mirrors Go's `ManagerInterface`. The concrete impl for
//! [`tukituki_process::Manager`] is supplied below so the binary can
//! pass a real Manager to [`crate::start`]. Tests substitute a fake.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use tukituki_config::RunTarget;
use tukituki_state::Status;

pub trait ManagerHandle: Send + Sync + 'static {
    fn get_all_statuses(&self) -> BTreeMap<String, Status>;
    fn get_targets(&self) -> Vec<RunTarget>;
    fn get_log_lines(&self, name: &str) -> Vec<String>;
    fn watch_log_lines(&self, name: &str) -> Receiver<String>;
    fn start(&self, name: &str) -> std::io::Result<()>;
    fn stop(&self, name: &str) -> std::io::Result<()>;
    fn restart(&self, name: &str) -> std::io::Result<()>;
    fn dump_log(&self, name: &str, dest: &std::path::Path) -> std::io::Result<()>;
    fn clear_log(&self, name: &str) -> std::io::Result<()>;
    fn stop_all(&self) -> std::io::Result<()>;
    fn update_targets(&self, targets: Vec<RunTarget>);
    /// Human-readable description of how the target would be launched.
    /// Mirrors Go's `Manager.Describe`. A blank string is fine for
    /// targets the manager doesn't have details on.
    fn describe(&self, name: &str) -> String;
    /// Used to spawn the OTel collector during reload-on-change so a
    /// new target with `otel: true` brings the collector up.
    fn ensure_otel_collector(&self) -> std::io::Result<()>;
    fn log_file_path(&self, name: &str) -> Option<PathBuf>;
    /// Path to `state.json` on disk. The TUI watches this so external
    /// `tukituki start/stop/restart` invocations are reflected without
    /// a detach/re-attach cycle.
    fn state_file_path(&self) -> PathBuf;
    /// Re-read `state.json` into the manager's in-memory mirror, then
    /// ensure a log tailer is running for each known process. Called
    /// in response to a state-file change event.
    fn reload_state_from_disk(&self);
}

impl ManagerHandle for tukituki_process::Manager {
    fn get_all_statuses(&self) -> BTreeMap<String, Status> {
        self.get_all_statuses()
    }
    fn get_targets(&self) -> Vec<RunTarget> {
        self.get_targets()
    }
    fn get_log_lines(&self, name: &str) -> Vec<String> {
        self.get_log_lines(name)
    }
    fn watch_log_lines(&self, name: &str) -> Receiver<String> {
        self.watch_log_lines(name)
    }
    fn start(&self, name: &str) -> std::io::Result<()> {
        self.start(name)
    }
    fn stop(&self, name: &str) -> std::io::Result<()> {
        self.stop(name)
    }
    fn restart(&self, name: &str) -> std::io::Result<()> {
        self.restart(name)
    }
    fn dump_log(&self, name: &str, dest: &std::path::Path) -> std::io::Result<()> {
        self.dump_log(name, dest)
    }
    fn clear_log(&self, name: &str) -> std::io::Result<()> {
        self.clear_log(name)
    }
    fn stop_all(&self) -> std::io::Result<()> {
        self.stop_all()
    }
    fn update_targets(&self, targets: Vec<RunTarget>) {
        self.update_targets(targets);
    }
    fn describe(&self, name: &str) -> String {
        tukituki_process::Manager::describe(self, name)
    }
    fn ensure_otel_collector(&self) -> std::io::Result<()> {
        tukituki_process::Manager::ensure_otel_collector(self)
    }
    fn log_file_path(&self, name: &str) -> Option<PathBuf> {
        tukituki_process::Manager::log_file_path(self, name)
    }
    fn state_file_path(&self) -> PathBuf {
        tukituki_process::Manager::state_file_path(self)
    }
    fn reload_state_from_disk(&self) {
        tukituki_process::Manager::reload_state_from_disk(self);
    }
}
