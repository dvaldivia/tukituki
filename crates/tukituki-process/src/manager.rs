//! Process manager — Pass A.
//!
//! Single-mutex design: every public method acquires `Mutex<Inner>` once.
//! Spawned children run detached under their own session (setsid via
//! `pre_exec`); a per-child reaper thread waits on the `Child` handle and
//! flips state to Stopped/Failed on exit. We never call `Child::kill`;
//! the Go binary uses `kill(-pid, SIGTERM)` to hit the whole process group
//! and so do we.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use tukituki_config::RunTarget;
use tukituki_state::{ProcessState, State, Status, is_alive};

use crate::otel_port;
use crate::shell::build_shell_cmd;
use crate::tailer;

/// OpenTelemetry collector configuration.
///
/// `port == 0` means "let SetOtelConfig pick one"; any other value is
/// treated as an explicit user choice and persisted as-is.
#[derive(Debug, Clone)]
pub struct OtelConfig {
    pub port: u16,
    pub protocol: String,
    pub severity: String,
}

/// Owns process lifecycle for a `.run/` directory's targets.
///
/// Clone-able by design — every spawn creates a reaper thread that needs
/// shared access to the same state. The inner `Mutex` is acquired briefly;
/// long operations (signal + wait loop) deliberately drop and re-acquire
/// to avoid blocking concurrent status reads.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    targets: Vec<RunTarget>,
    state: State,
    state_dir: PathBuf,
    logs_dir: PathBuf,
    project_root: PathBuf,
    otel_cfg: Option<OtelConfig>,
    /// On-disk port observed before the most recent `set_otel_config`. Phase
    /// 5 (EnsureOtelCollector) uses this to decide whether children need a
    /// restart to pick up a new endpoint.
    previous_otel_port: u16,
    /// Per-target reaper join handles, kept so `Drop` can attempt a clean
    /// detach if the user never calls `stop_all`.
    reapers: HashMap<String, thread::JoinHandle<()>>,
    /// Per-target in-memory ring buffer (1000 lines). Populated by the
    /// tailer thread + diagnostic `append_log_line` calls. Used by the
    /// TUI and by `logs --follow` for backfill.
    log_lines: HashMap<String, VecDeque<String>>,
    /// Subscribers receiving a stream of new lines per target. Bounded
    /// `sync_channel(256)`; slow subscribers get their lines dropped
    /// rather than blocking the tailer (Go does the same with select+default).
    subscribers: HashMap<String, Vec<SyncSender<String>>>,
    /// Cancel handle for each running tailer thread. Sending `()` or
    /// dropping it tells the thread to exit on its next poll.
    tailer_cancels: HashMap<String, Sender<()>>,
}

impl Manager {
    /// Build a Manager: ensure `<state_dir>/logs` exists, load any
    /// existing state file. `project_root` is the directory relative to
    /// which `RunTarget.workdir` is resolved.
    pub fn new(
        targets: Vec<RunTarget>,
        state_dir: impl Into<PathBuf>,
        project_root: impl Into<PathBuf>,
    ) -> io::Result<Self> {
        let state_dir = state_dir.into();
        let project_root = project_root.into();
        let logs_dir = state_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)?;

        let state_file = state_dir.join("state.json");
        let state = State::load(&state_file);

        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                targets,
                state,
                state_dir,
                logs_dir,
                project_root,
                otel_cfg: None,
                previous_otel_port: 0,
                reapers: HashMap::new(),
                log_lines: HashMap::new(),
                subscribers: HashMap::new(),
                tailer_cancels: HashMap::new(),
            })),
        })
    }

    /// Replace the target list. Used after a TUI-side reload.
    pub fn update_targets(&self, targets: Vec<RunTarget>) {
        let mut inner = self.lock();
        inner.targets = targets;
    }

    /// Current target list (cloned).
    pub fn get_targets(&self) -> Vec<RunTarget> {
        self.lock().targets.clone()
    }

    /// Path to the Unix-domain socket the otel-collector uses to push
    /// error events to attached TUIs. Lives inside the state directory
    /// so each invocation in a project shares it; the collector
    /// recreates the file on startup.
    pub fn otel_notify_socket(&self) -> PathBuf {
        self.lock().state_dir.join("otel-notify.sock")
    }

    /// Configure the OTel collector and resolve its port. Mirrors Go's
    /// SetOtelConfig: explicit user port wins; else reuse the previously
    /// persisted port if it is still bindable (or owned by an alive
    /// otel-errors process recorded in state); else allocate fresh.
    pub fn set_otel_config(&self, mut cfg: OtelConfig) {
        let mut inner = self.lock();

        // Snapshot the on-disk port BEFORE any potential overwrite so
        // EnsureOtelCollector can detect drift later (Phase 5 hookup).
        inner.previous_otel_port = otel_port::load(&inner.state_dir);

        if cfg.port != 0 {
            otel_port::save(&inner.state_dir, cfg.port);
            inner.otel_cfg = Some(cfg);
            return;
        }

        if inner.previous_otel_port != 0 && saved_port_reusable(&inner, inner.previous_otel_port) {
            cfg.port = inner.previous_otel_port;
            inner.otel_cfg = Some(cfg);
            return;
        }

        let port = match otel_port::allocate_free_port() {
            Ok(p) => p,
            Err(_) => {
                // Fall back to OTLP well-known ports — matches Go's
                // "give the rest of the pipeline *some* port" approach.
                if cfg.protocol == "http" { 4318 } else { 4317 }
            }
        };
        cfg.port = port;
        otel_port::save(&inner.state_dir, port);
        inner.otel_cfg = Some(cfg);
    }

    /// Port the OTel receiver is (or would be) bound to. `0` when OTel
    /// is unconfigured.
    pub fn otel_receiver_port(&self) -> u16 {
        self.lock().otel_cfg.as_ref().map(|c| c.port).unwrap_or(0)
    }

    /// Start the bundled OTel collector if any target has `otel: true`
    /// and the collector is not already alive. The port was resolved
    /// and persisted by [`Manager::set_otel_config`]; this method only
    /// re-picks a port if the resolved one became unbindable since
    /// construction (and nothing of ours owns it).
    ///
    /// Spawns the running binary with the `otel-collector` subcommand
    /// as a regular detached child named `otel-errors`.
    pub fn ensure_otel_collector(&self) -> io::Result<()> {
        let has_otel_cfg = self.lock().otel_cfg.is_some();
        if !has_otel_cfg {
            return Ok(());
        }
        let any_otel = self.lock().targets.iter().any(|t| t.otel);
        if !any_otel {
            return Ok(());
        }

        // Snapshot current state for decisions made under the lock,
        // then release it before potentially long operations.
        let (alive, original_port, previous_port) = {
            let inner = self.lock();
            let cfg_port = inner.otel_cfg.as_ref().map(|c| c.port).unwrap_or(0);
            let alive = inner
                .state
                .processes
                .get(crate::OTEL_TARGET_NAME)
                .map(|ps| ps.status == Status::Running && is_alive(Some(ps)))
                .unwrap_or(false);
            (alive, cfg_port, inner.previous_otel_port)
        };

        // If the resolved port was claimed by something else and we
        // don't already own a collector on it, pick a new port. Any
        // otel:true children started against the old port will need a
        // restart to pick up the new endpoint.
        if !alive && !otel_port::port_bindable(original_port) {
            let new_port = otel_port::allocate_free_port()?;
            self.append_log_line(
                crate::OTEL_TARGET_NAME,
                &format!(
                    "otel-errors: previous port {original_port} is no longer available; switching to {new_port}"
                ),
            );
            {
                let mut inner = self.lock();
                if let Some(cfg) = inner.otel_cfg.as_mut() {
                    cfg.port = new_port;
                }
                otel_port::save(&inner.state_dir, new_port);
            }
        }

        let target = self.build_otel_target()?;
        self.upsert_target(target.clone());

        if alive {
            return Ok(());
        }

        self.start_target(target)?;

        // Restart running otel:true children if the port drifted from
        // the previously-persisted value, so they re-read
        // OTEL_EXPORTER_OTLP_ENDPOINT.
        let effective_port = self.otel_receiver_port();
        if previous_port != effective_port || original_port != effective_port {
            self.restart_running_otel_targets();
        }
        Ok(())
    }

    /// Stop the OTel collector if it's recorded in state, and remove
    /// the persisted port file so the next invocation can pick fresh.
    pub fn stop_otel_collector(&self) -> io::Result<()> {
        let recorded = self
            .lock()
            .state
            .processes
            .contains_key(crate::OTEL_TARGET_NAME);
        if !recorded {
            return Ok(());
        }
        otel_port::remove(&self.lock().state_dir);
        self.stop(crate::OTEL_TARGET_NAME)
    }

    /// Build the virtual `otel-errors` RunTarget for the current
    /// OtelConfig and the current binary's `otel-collector` subcommand.
    ///
    /// Returns an `io::Error` if the running executable's path can't
    /// be resolved or no OtelConfig has been set.
    pub fn build_otel_target(&self) -> io::Result<RunTarget> {
        let exe = std::env::current_exe()?;
        let inner = self.lock();
        let cfg = inner
            .otel_cfg
            .as_ref()
            .ok_or_else(|| io::Error::other("otel config not set"))?;
        let socket = inner.state_dir.join("otel-notify.sock");
        Ok(RunTarget {
            name: crate::OTEL_TARGET_NAME.to_string(),
            description: "OpenTelemetry error collector".into(),
            is_virtual: true,
            command: exe.to_string_lossy().to_string(),
            args: vec![
                "otel-collector".into(),
                "--protocol".into(),
                cfg.protocol.clone(),
                "--severity".into(),
                cfg.severity.clone(),
                "--port".into(),
                cfg.port.to_string(),
                "--notify-socket".into(),
                socket.to_string_lossy().to_string(),
            ],
            ..Default::default()
        })
    }

    /// Return the virtual `otel-errors` target for display. When the
    /// OTel config is incomplete a stub target is returned (Command
    /// empty); read-only callers like `status` can still list the
    /// entry without crashing.
    pub fn virtual_otel_target(&self) -> RunTarget {
        match self.build_otel_target() {
            Ok(t) => t,
            Err(_) => RunTarget {
                name: crate::OTEL_TARGET_NAME.to_string(),
                description: "OpenTelemetry error collector".into(),
                is_virtual: true,
                ..Default::default()
            },
        }
    }

    /// Insert or replace a target with matching name in the in-memory
    /// targets list. Used to keep the virtual otel-errors entry in
    /// sync with the current configuration so TUI restart reuses the
    /// correct args.
    pub fn upsert_target(&self, t: RunTarget) {
        let mut inner = self.lock();
        if let Some(slot) = inner.targets.iter_mut().find(|x| x.name == t.name) {
            *slot = t;
        } else {
            inner.targets.push(t);
        }
    }

    /// Restart every non-virtual target with `otel: true` that is
    /// currently alive, so they re-read OTEL_EXPORTER_OTLP_ENDPOINT.
    fn restart_running_otel_targets(&self) {
        let names: Vec<String> = {
            let inner = self.lock();
            inner
                .targets
                .iter()
                .filter(|t| t.otel && t.name != crate::OTEL_TARGET_NAME)
                .filter_map(|t| {
                    inner.state.processes.get(&t.name).and_then(|ps| {
                        (ps.status == Status::Running && is_alive(Some(ps))).then(|| t.name.clone())
                    })
                })
                .collect()
        };
        for name in names {
            self.append_log_line(
                &name,
                &format!("otel-errors: restarting {name} to pick up new collector endpoint"),
            );
            if let Err(e) = self.restart(&name) {
                self.append_log_line(&name, &format!("otel-errors: restart {name}: {e}"));
            }
        }
    }

    /// Start a named target. No-op if already running and alive.
    pub fn start(&self, name: &str) -> io::Result<()> {
        let target = self
            .find_target(name)
            .ok_or_else(|| io::Error::other(format!("unknown target: {name:?}")))?;
        self.start_target(target)
    }

    /// Start a `RunTarget` directly. Used for virtual targets that
    /// aren't in `targets` (e.g. the OTel collector, once Phase 5 lands).
    pub fn start_target(&self, target: RunTarget) -> io::Result<()> {
        if !target.parse_error.is_empty() {
            return Err(io::Error::other(format!(
                "target {:?} has a config error: {}",
                target.name, target.parse_error
            )));
        }

        let name = target.name.clone();

        // Build the spawn parameters before acquiring the lock, except
        // for the "already running" check which needs state access.
        {
            let inner = self.lock();
            if let Some(ps) = inner.state.processes.get(&name) {
                if ps.status == Status::Running && is_alive(Some(ps)) {
                    return Ok(());
                }
            }
        }

        let (log_file_path, child, otel_endpoint_port) = {
            let inner = self.lock();
            let log_path = inner.logs_dir.join(format!("{name}.log"));

            // Truncate the log on every (re)start so output reflects the
            // current run — same semantics as Go's O_TRUNC|O_CREATE flags.
            let log_file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&log_path)?;

            let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));
            let shell_line = build_shell_cmd(&target.command, &target.args);

            let mut cmd = Command::new(&shell);
            cmd.arg("-l").arg("-c").arg(&shell_line);

            // stdout + stderr both feed the log file. Each gets its own
            // FD via try_clone so closing one in the parent doesn't kill
            // the other in the child.
            let stderr_file = log_file.try_clone()?;
            cmd.stdout(log_file).stderr(stderr_file);
            // Stdin must NOT inherit from the parent — `std::process::
            // Command`'s default is Stdio::inherit(), which would leave
            // every spawned backend holding an open fd to the user's
            // terminal (a tmux pane PTY when run under tmux). After
            // detach, those backends keep that fd; every keystroke into
            // the now-shared PTY wakes every blocked `read(0)` across
            // them (thundering herd), which manifests as severe input
            // lag in the tmux pane. Pipe to /dev/null instead.
            cmd.stdin(Stdio::null());

            // Workdir resolution: absolute as-is, relative joined with
            // project_root.
            if !target.workdir.is_empty() {
                let workdir = Path::new(&target.workdir);
                if workdir.is_absolute() {
                    cmd.current_dir(workdir);
                } else {
                    cmd.current_dir(inner.project_root.join(&target.workdir));
                }
            }

            // Env: parent env, then target overlay. Command::env replaces
            // matching keys, which matches Go's append-based semantics.
            for (k, v) in std::env::vars() {
                cmd.env(k, v);
            }
            for (k, v) in &target.env {
                cmd.env(k, v);
            }

            let otel_port_used = if target.otel {
                inner.otel_cfg.as_ref().map(|c| c.port).unwrap_or(0)
            } else {
                0
            };
            if otel_port_used != 0 {
                let endpoint = format!("http://127.0.0.1:{otel_port_used}");
                cmd.env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint);
                cmd.env("OTEL_METRICS_EXPORTER", "none");
                cmd.env("OTEL_TRACES_EXPORTER", "none");
            }

            // SAFETY: the closure runs after fork, before exec. It must
            // be async-signal-safe — `setsid` is. We do not allocate or
            // touch a Mutex here.
            unsafe {
                cmd.pre_exec(|| {
                    nix::unistd::setsid().map_err(io::Error::from)?;
                    Ok(())
                });
            }

            let child = cmd.spawn()?;
            (log_path, child, otel_port_used)
        };
        let _ = otel_endpoint_port; // kept for future telemetry; intentionally unused now

        let pid = child.id() as i32;
        {
            let mut inner = self.lock();
            inner.state.processes.insert(
                name.clone(),
                ProcessState {
                    name: name.clone(),
                    pid,
                    log_file: log_file_path.display().to_string(),
                    started_at: Utc::now(),
                    status: Status::Running,
                    exit_code: None,
                },
            );
            let _ = inner.state.save();

            // Clear the in-memory ring buffer so a (re)start shows a fresh
            // log surface in the TUI; the on-disk file was just truncated.
            inner.log_lines.entry(name.clone()).or_default().clear();

            // Start the reaper thread. It owns the Child for the duration
            // of the process; we never call .kill() on it (we signal by
            // PID instead, which is the only way to hit the whole group).
            let manager_clone = self.clone();
            let name_clone = name.clone();
            let log_path_clone = log_file_path.clone();
            let handle = thread::spawn(move || {
                reaper_loop(manager_clone, name_clone, child, log_path_clone);
            });
            inner.reapers.insert(name.clone(), handle);
        }

        // Tailer thread for new output. Created outside the lock so the
        // tailer doesn't have to wait on the manager during startup.
        self.start_log_tailer(&name, &log_file_path);

        Ok(())
    }

    /// `StartAll`: start every target that isn't already running. Targets
    /// with parse errors are silently skipped. Stops on first error.
    pub fn start_all(&self) -> io::Result<()> {
        let names: Vec<String> = self
            .lock()
            .targets
            .iter()
            .filter(|t| t.parse_error.is_empty())
            .map(|t| t.name.clone())
            .collect();
        for name in names {
            self.start(&name)
                .map_err(|e| io::Error::other(format!("start {name}: {e}")))?;
        }
        Ok(())
    }

    /// Stop a target: SIGTERM the process group, wait up to 5s for the
    /// group to drain, escalate to SIGKILL. Run cleanup commands after.
    pub fn stop(&self, name: &str) -> io::Result<()> {
        let pid = {
            let inner = self.lock();
            match inner.state.processes.get(name) {
                Some(ps) => ps.pid,
                None => {
                    return Err(io::Error::other(format!("no state for process {name:?}")));
                }
            }
        };

        // Cancel the tailer for this target so we stop polling a file
        // that's about to be replaced (on a follow-on start).
        {
            let mut inner = self.lock();
            inner.tailer_cancels.remove(name);
        }

        if pid <= 0 {
            // Nothing to signal — just mark stopped and run cleanup.
            self.mark_stopped(name);
            self.run_cleanup(name);
            return Ok(());
        }

        // SIGTERM the whole process group (negative PID).
        let group = Pid::from_raw(-pid);
        let leader = Pid::from_raw(pid);
        match kill(group, Signal::SIGTERM) {
            Ok(_) => {}
            Err(_) => {
                // Group signal failed — try the leader directly.
                if let Err(e) = kill(leader, Signal::SIGTERM) {
                    if e != Errno::ESRCH {
                        return Err(io::Error::other(format!("SIGTERM to {pid}: {e}")));
                    }
                    // Already gone — proceed straight to cleanup.
                    self.mark_stopped(name);
                    self.run_cleanup(name);
                    return Ok(());
                }
            }
        }

        // Wait up to 5s for the *whole group* to drain. Checking only the
        // leader is wrong: a fast-dying shell can leave longer-lived
        // descendants behind as orphans.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
            if !group_alive(pid) {
                self.mark_stopped(name);
                self.run_cleanup(name);
                return Ok(());
            }
        }

        // SIGKILL the group. If that fails, fall back to the leader.
        if kill(group, Signal::SIGKILL).is_err()
            && let Err(e) = kill(leader, Signal::SIGKILL)
            && e != Errno::ESRCH
        {
            return Err(io::Error::other(format!("SIGKILL to {pid}: {e}")));
        }

        // Briefly poll for the group to fully drain. SIGKILL is
        // immediate kernel-side but exit dispatch takes a moment;
        // returning early can race a follow-on Start for ports.
        let reap_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < reap_deadline {
            if !group_alive(pid) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        self.mark_stopped(name);
        self.run_cleanup(name);
        Ok(())
    }

    /// Stop every target. Errors per-target are intentionally swallowed
    /// (they go to stderr in the Go binary) so one bad target doesn't
    /// prevent stopping the rest.
    pub fn stop_all(&self) -> io::Result<()> {
        let names: Vec<String> = self.lock().targets.iter().map(|t| t.name.clone()).collect();
        for name in names {
            let _ = self.stop(&name);
        }
        // Also stop a virtual OTel collector if recorded in state.
        let has_otel = self
            .lock()
            .state
            .processes
            .contains_key(crate::OTEL_TARGET_NAME);
        if has_otel {
            otel_port::remove(&self.lock().state_dir);
            let _ = self.stop(crate::OTEL_TARGET_NAME);
        }
        Ok(())
    }

    /// Restart = stop + start. A "wasn't running" stop error is non-fatal.
    pub fn restart(&self, name: &str) -> io::Result<()> {
        let _ = self.stop(name); // tolerated; Go logs but continues
        self.start(name)
    }

    /// Current status for a named process, reconciling Running→Stopped
    /// when the recorded PID is no longer alive.
    pub fn get_status(&self, name: &str) -> Status {
        let inner = self.lock();
        match inner.state.processes.get(name) {
            None => Status::Unknown,
            Some(ps) if ps.status == Status::Running && !is_alive(Some(ps)) => Status::Stopped,
            Some(ps) => ps.status,
        }
    }

    /// Per-name status map for every recorded process.
    pub fn get_all_statuses(&self) -> std::collections::BTreeMap<String, Status> {
        let inner = self.lock();
        inner
            .state
            .processes
            .iter()
            .map(|(name, ps)| {
                let status = if ps.status == Status::Running && !is_alive(Some(ps)) {
                    Status::Stopped
                } else {
                    ps.status
                };
                (name.clone(), status)
            })
            .collect()
    }

    /// Snapshot of every recorded process state.
    pub fn get_all_process_states(&self) -> std::collections::BTreeMap<String, ProcessState> {
        self.lock().state.processes.clone()
    }

    /// Headless attach: after a fresh tukituki process starts against an
    /// existing state file, reconcile alive/dead and persist. Also
    /// starts a log tailer for each still-running process so the ring
    /// buffer fills with the on-disk history. Registers the virtual
    /// `otel-errors` target so display surfaces (status, TUI sidebar)
    /// can show it even when no `.run/*.yaml` declares it.
    pub fn attach_to_existing(&self) -> io::Result<()> {
        let mut still_running: Vec<(String, PathBuf)> = Vec::new();
        let needs_virtual_otel;
        {
            let mut inner = self.lock();
            inner.state.reconcile_alive();
            for (name, ps) in &inner.state.processes {
                if ps.status == Status::Running {
                    still_running.push((name.clone(), PathBuf::from(&ps.log_file)));
                }
            }
            needs_virtual_otel = inner.state.processes.contains_key(crate::OTEL_TARGET_NAME);
            inner.state.save()?;
        }
        if needs_virtual_otel {
            let target = self.virtual_otel_target();
            self.upsert_target(target);
        }
        for (name, path) in still_running {
            self.start_log_tailer(&name, &path);
        }
        Ok(())
    }

    /// Snapshot of the in-memory ring buffer for `name`. Returns an
    /// empty Vec when there are no buffered lines.
    pub fn get_log_lines(&self, name: &str) -> Vec<String> {
        let inner = self.lock();
        inner
            .log_lines
            .get(name)
            .map(|b| b.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Subscribe to new log lines for `name`. Returns a `Receiver` that
    /// yields every line appended after this call. The channel is
    /// bounded at 256 entries — if the consumer is slow, lines are
    /// dropped instead of blocking the tailer (matches Go's
    /// `select { case ch <- l: default: }`).
    pub fn watch_log_lines(&self, name: &str) -> Receiver<String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(256);
        let mut inner = self.lock();
        inner
            .subscribers
            .entry(name.to_string())
            .or_default()
            .push(tx);
        rx
    }

    /// Truncate the on-disk log file and clear the in-memory ring
    /// buffer.  The tailer's offset will reset itself on the next poll
    /// (it detects truncation via `size < offset`).
    pub fn clear_log(&self, name: &str) -> io::Result<()> {
        let log_path = {
            let mut inner = self.lock();
            if let Some(b) = inner.log_lines.get_mut(name) {
                b.clear();
            }
            inner
                .state
                .processes
                .get(name)
                .map(|ps| PathBuf::from(&ps.log_file))
        };
        if let Some(path) = log_path
            && let Err(e) = std::fs::File::create(&path)
            && e.kind() != io::ErrorKind::NotFound
        {
            return Err(io::Error::other(format!("truncate log file: {e}")));
        }
        Ok(())
    }

    /// Append a diagnostic line to the named target's ring buffer (and
    /// broadcast it to subscribers). Used by the manager itself in
    /// place of stderr writes, so the TUI's alt-screen renderer is
    /// never corrupted by inline terminal output. Multi-line input is
    /// split so each ring-buffer entry stays a single line.
    pub fn append_log_line(&self, name: &str, line: &str) {
        let mut inner = self.lock();
        append_locked(&mut inner, name, line);
    }

    /// Copy the named target's log file to `dest`.
    pub fn dump_log(&self, name: &str, dest: &Path) -> io::Result<()> {
        let src = {
            let inner = self.lock();
            inner
                .state
                .processes
                .get(name)
                .map(|ps| PathBuf::from(&ps.log_file))
                .ok_or_else(|| io::Error::other(format!("no state for process {name:?}")))?
        };
        std::fs::copy(&src, dest)?;
        Ok(())
    }

    /// Human-readable summary of how the named target is (or would be)
    /// launched: shell invocation, workdir, target-configured env, OTel
    /// injection, and current status / PID / log file. Mirrors Go's
    /// `Manager.Describe`. Returns an empty string when the target is
    /// unknown, so the TUI can render a "target not found" pane rather
    /// than crashing.
    pub fn describe(&self, name: &str) -> String {
        let (target, ps, otel_port, project_root) = {
            let inner = self.lock();
            let Some(t) = inner.targets.iter().find(|t| t.name == name).cloned() else {
                return String::new();
            };
            let ps = inner.state.processes.get(name).cloned();
            let otel_port = inner.otel_cfg.as_ref().map(|c| c.port).unwrap_or(0);
            (t, ps, otel_port, inner.project_root.clone())
        };
        let shell =
            std::env::var_os("SHELL").unwrap_or_else(|| std::ffi::OsString::from("/bin/sh"));
        let shell_disp = shell.to_string_lossy().to_string();

        let workdir = if target.workdir.is_empty() {
            project_root.display().to_string()
        } else if Path::new(&target.workdir).is_absolute() {
            target.workdir.clone()
        } else {
            project_root.join(&target.workdir).display().to_string()
        };

        let mut out = String::new();
        use std::fmt::Write as _;
        let _ = writeln!(out, "Target:       {}", target.name);
        if !target.description.is_empty() {
            let _ = writeln!(out, "Description:  {}", target.description);
        }
        if target.is_virtual {
            let _ = writeln!(out, "Virtual:      true (managed by tukituki)");
        }
        if let Some(ps) = &ps {
            let mut status = ps.status;
            if status == Status::Running && !is_alive(Some(ps)) {
                status = Status::Stopped;
            }
            let _ = writeln!(
                out,
                "Status:       {}",
                match status {
                    Status::Running => "running",
                    Status::Stopped => "stopped",
                    Status::Failed => "failed",
                    Status::Unknown => "unknown",
                }
            );
            if ps.pid != 0 {
                let _ = writeln!(out, "PID:          {}", ps.pid);
            }
            let _ = writeln!(
                out,
                "Started:      {}",
                ps.started_at.format("%+") // RFC3339-ish
            );
            if !ps.log_file.is_empty() {
                let _ = writeln!(out, "Log file:     {}", ps.log_file);
            }
            if let Some(code) = ps.exit_code {
                let _ = writeln!(out, "Exit code:    {code}");
            }
        } else {
            let _ = writeln!(out, "Status:       (never started)");
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "Shell:        {shell_disp} -l -c");
        let _ = writeln!(out, "Command:      {}", target.command);
        if !target.args.is_empty() {
            let _ = writeln!(out, "Args:");
            for a in &target.args {
                let _ = writeln!(out, "  - {a}");
            }
        }
        let _ = writeln!(
            out,
            "Shell line:   {}",
            crate::shell::build_shell_cmd(&target.command, &target.args)
        );
        let _ = writeln!(out, "Workdir:      {workdir}");

        let _ = writeln!(out);
        let _ = write!(out, "OTel:         {}", target.otel);
        if target.otel && otel_port != 0 {
            let _ = write!(out, " (endpoint: http://127.0.0.1:{otel_port})");
        }
        let _ = writeln!(out);

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Injected environment (parent env is inherited separately):"
        );
        let mut envs: Vec<(String, String)> = target
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if target.otel && otel_port != 0 {
            envs.push((
                "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
                format!("http://127.0.0.1:{otel_port}"),
            ));
            envs.push(("OTEL_METRICS_EXPORTER".into(), "none".into()));
            envs.push(("OTEL_TRACES_EXPORTER".into(), "none".into()));
        }
        if envs.is_empty() {
            let _ = writeln!(out, "  (none)");
        } else {
            for (k, v) in envs {
                let _ = writeln!(out, "  {k}={v}");
            }
        }

        if !target.cleanup.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "Cleanup commands:");
            for c in &target.cleanup {
                let _ = writeln!(out, "  - {c}");
            }
        }
        out
    }

    /// Path to the log file for a named target, if known.
    pub fn log_file_path(&self, name: &str) -> Option<PathBuf> {
        self.lock()
            .state
            .processes
            .get(name)
            .map(|ps| PathBuf::from(&ps.log_file))
    }

    // ---- internals -----------------------------------------------------

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn find_target(&self, name: &str) -> Option<RunTarget> {
        self.lock().targets.iter().find(|t| t.name == name).cloned()
    }

    fn mark_stopped(&self, name: &str) {
        let mut inner = self.lock();
        if let Some(ps) = inner.state.processes.get_mut(name) {
            ps.status = Status::Stopped;
            let _ = inner.state.save();
        }
    }

    /// Run a target's `cleanup:` commands via `$SHELL -l -c`. Each runs in
    /// the target's workdir (when set); failures are intentionally
    /// non-fatal but routed through [`Manager::append_log_line`] so the
    /// message lands in the in-memory ring buffer + subscriber stream
    /// rather than corrupting the TUI's alt-screen renderer.
    fn run_cleanup(&self, name: &str) {
        let (target, project_root) = {
            let inner = self.lock();
            let Some(t) = inner.targets.iter().find(|t| t.name == name).cloned() else {
                return;
            };
            (t, inner.project_root.clone())
        };
        if target.cleanup.is_empty() {
            return;
        }

        let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));
        let workdir = if target.workdir.is_empty() {
            None
        } else if Path::new(&target.workdir).is_absolute() {
            Some(PathBuf::from(&target.workdir))
        } else {
            Some(project_root.join(&target.workdir))
        };

        for cmd_str in &target.cleanup {
            let mut cmd = Command::new(&shell);
            cmd.arg("-l").arg("-c").arg(cmd_str);
            if let Some(w) = &workdir {
                cmd.current_dir(w);
            }
            match cmd.output() {
                Ok(out) if out.status.success() => {}
                Ok(out) => {
                    let mut msg = format!(
                        "cleanup {name}: {cmd_str:?}: exit status {}",
                        out.status.code().unwrap_or(-1)
                    );
                    if !out.stdout.is_empty() {
                        msg.push('\n');
                        msg.push_str(&String::from_utf8_lossy(&out.stdout));
                    }
                    if !out.stderr.is_empty() {
                        msg.push('\n');
                        msg.push_str(&String::from_utf8_lossy(&out.stderr));
                    }
                    self.append_log_line(name, msg.trim_end());
                }
                Err(e) => {
                    self.append_log_line(name, &format!("cleanup {name}: {cmd_str:?}: {e}"));
                }
            }
        }
    }

    /// Spawn (or replace) a tailer thread for the named target.
    ///
    /// The thread polls `log_path` every 100ms, hands each new line to
    /// the manager, and exits when its cancel sender is dropped.
    /// Replacing an existing tailer drops the old cancel sender, which
    /// the old thread observes on its next poll.
    fn start_log_tailer(&self, name: &str, log_path: &Path) {
        let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
        {
            let mut inner = self.lock();
            // Replace any previous cancel handle; its associated thread
            // will exit on its next poll observing the disconnect.
            inner.tailer_cancels.insert(name.to_string(), cancel_tx);
            // Make sure the ring-buffer slot exists so consumers don't
            // race the tailer's first append.
            inner.log_lines.entry(name.to_string()).or_default();
        }

        let manager = self.clone();
        let name_owned = name.to_string();
        let path_owned = log_path.to_path_buf();
        thread::Builder::new()
            .name(format!("tukituki-tailer-{name}"))
            .spawn(move || {
                tailer::run(path_owned, cancel_rx, |line| {
                    let mut inner = manager.inner.lock().unwrap_or_else(|p| p.into_inner());
                    append_locked(&mut inner, &name_owned, &line);
                });
            })
            .ok();
    }
}

/// Lock-free append helper.  Splits multi-line input on `\n`, evicts
/// the oldest entry when the ring buffer reaches its 1000-line cap,
/// and broadcasts each line to subscribers — dropping any sender
/// whose receiver is gone OR whose 256-deep queue is full.
fn append_locked(inner: &mut Inner, name: &str, line: &str) {
    let buf = inner.log_lines.entry(name.to_string()).or_default();
    for piece in line.trim_end_matches('\n').split('\n') {
        buf.push_back(piece.to_string());
        while buf.len() > tailer::RING_BUFFER_SIZE {
            buf.pop_front();
        }
    }
    if let Some(subs) = inner.subscribers.get_mut(name) {
        let mut alive: Vec<SyncSender<String>> = Vec::with_capacity(subs.len());
        for tx in subs.drain(..) {
            let mut disconnected = false;
            for piece in line.trim_end_matches('\n').split('\n') {
                match tx.try_send(piece.to_string()) {
                    Ok(_) => {}
                    Err(TrySendError::Full(_)) => {
                        // Slow consumer — drop the line, keep the sender.
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if !disconnected {
                alive.push(tx);
            }
        }
        *subs = alive;
    }
}

/// `groupAlive` analogue: any member of the process group still around?
///
/// `kill(-pid, 0)` checks: `ESRCH` ⇒ no member, `EPERM` ⇒ has members
/// (we lack permission), otherwise has members.
pub(crate) fn group_alive(leader_pid: i32) -> bool {
    if leader_pid <= 0 {
        return false;
    }
    match kill(Pid::from_raw(-leader_pid), None) {
        Ok(_) => true,
        Err(Errno::ESRCH) => false,
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// Decide whether a persisted OTel port can still be used.  Reusable
/// when the port is freely bindable *now*, OR our recorded otel-errors
/// PID is alive (i.e. we own whatever is bound to it).
fn saved_port_reusable(inner: &Inner, port: u16) -> bool {
    if otel_port::port_bindable(port) {
        return true;
    }
    inner
        .state
        .processes
        .get(crate::OTEL_TARGET_NAME)
        .map(|ps| is_alive(Some(ps)))
        .unwrap_or(false)
}

/// Append `(Process exited at ..., exit code: N)` to the log file in the
/// same format Go's `Manager.Start` reaper goroutine writes.
fn append_exit_marker(log_path: &Path, code: i32) {
    if let Ok(mut f) = OpenOptions::new().append(true).open(log_path) {
        use std::io::Write as _;
        let _ = writeln!(
            f,
            "\n(Process exited at {}, exit code: {})",
            Utc::now().format("%Y-%m-%d %H:%M:%S"),
            code
        );
        let _ = f.sync_data();
    }
    let _ = File::open(log_path); // suppress unused-import warnings under cfg
}

/// Body of the reaper thread. Waits for the child, updates state on
/// exit, appends the exit marker to the log file.
fn reaper_loop(manager: Manager, name: String, mut child: std::process::Child, log_path: PathBuf) {
    let exit = child.wait();
    let (status, code) = match exit {
        Ok(es) => {
            let code = es.code().unwrap_or(-1);
            let st = if es.success() {
                Status::Stopped
            } else {
                Status::Failed
            };
            (st, code)
        }
        Err(_) => (Status::Stopped, -1),
    };

    append_exit_marker(&log_path, code);

    let mut inner = manager.inner.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ps) = inner.state.processes.get_mut(&name) {
        // If `stop` already wrote `Stopped`, preserve it: the user's
        // intent (stop) wins over the natural exit signal.
        if ps.status == Status::Running {
            ps.status = status;
        }
        ps.exit_code = Some(code);
        let _ = inner.state.save();
    }
    inner.reapers.remove(&name);
}
