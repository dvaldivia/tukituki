//! Integration tests for the process Manager.
//!
//! These tests spawn real subprocesses and verify lifecycle behaviour.
//! Run serially under cargo's default parallelism — they don't share
//! state directories and each one writes to its own tempdir.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tukituki_config::RunTarget;
use tukituki_process::{Manager, OtelConfig};
use tukituki_state::Status;

fn echo_target(name: &str) -> RunTarget {
    RunTarget {
        name: name.into(),
        command: "sh".into(),
        args: vec!["-c".into(), format!("echo hello from {name}")],
        ..Default::default()
    }
}

fn sleep_target(name: &str) -> RunTarget {
    RunTarget {
        name: name.into(),
        command: "sh".into(),
        args: vec!["-c".into(), "echo started && sleep 60".into()],
        ..Default::default()
    }
}

fn new_test_manager(targets: Vec<RunTarget>) -> (TempDir, Manager) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state_dir = dir.path().join(".tukituki");
    let m = Manager::new(targets, state_dir, dir.path().to_path_buf()).expect("manager");
    (dir, m)
}

// ---- spawn / stop ----------------------------------------------------

#[test]
fn start_stop() {
    let (_dir, m) = new_test_manager(vec![sleep_target("sleepy")]);

    m.start("sleepy").expect("start");
    // Give the shell time to exec the sleep.
    thread::sleep(Duration::from_millis(200));

    assert_eq!(m.get_status("sleepy"), Status::Running);

    m.stop("sleepy").expect("stop");
    thread::sleep(Duration::from_millis(200));

    let s = m.get_status("sleepy");
    assert_ne!(s, Status::Running, "expected stopped, got {s:?}");
}

#[test]
fn start_all_runs_each_target() {
    let targets = vec![echo_target("a"), echo_target("b"), echo_target("c")];
    let (_dir, m) = new_test_manager(targets);

    m.start_all().expect("start_all");
    thread::sleep(Duration::from_millis(500));

    let statuses = m.get_all_statuses();
    for name in ["a", "b", "c"] {
        assert!(statuses.contains_key(name), "missing status for {name}");
    }
}

#[test]
fn start_all_skips_autorun_false() {
    let mut manual = sleep_target("manual");
    manual.autorun = false;
    let targets = vec![sleep_target("auto"), manual];
    let (_dir, m) = new_test_manager(targets);

    m.start_all().expect("start_all");
    thread::sleep(Duration::from_millis(300));

    assert_eq!(m.get_status("auto"), Status::Running);
    assert_ne!(
        m.get_status("manual"),
        Status::Running,
        "autorun: false target must not be started by start_all"
    );

    // Targeted start still works — the manual target is reachable by name.
    m.start("manual").expect("start by name");
    thread::sleep(Duration::from_millis(300));
    assert_eq!(m.get_status("manual"), Status::Running);

    let _ = m.stop("auto");
    let _ = m.stop("manual");
}

#[test]
fn dump_log_writes_child_output() {
    let (_dir, m) = new_test_manager(vec![echo_target("logger")]);
    m.start("logger").expect("start");
    thread::sleep(Duration::from_millis(400));

    let dest_dir = tempfile::tempdir().unwrap();
    let dest = dest_dir.path().join("dump.log");
    m.dump_log("logger", &dest).expect("dump_log");

    let data = fs::read_to_string(&dest).unwrap();
    assert!(
        data.contains("hello from logger"),
        "dump missing expected output: {data:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn spawned_child_stdin_is_null_not_inherited() {
    // Regression guard for the "tmux pane becomes super-slow after
    // detach" bug. Children must NOT inherit the parent's stdin
    // (which under tmux is the pane's PTY) — otherwise every backend
    // is blocked on read(0) against the user's terminal after detach
    // and the kernel wakes them all up on every keystroke. The shell
    // prints what `/proc/$$/fd/0` resolves to; we expect /dev/null,
    // not /dev/pts/<N> or any inherited PTY.
    let target = RunTarget {
        name: "stdin-check".into(),
        command: "sh".into(),
        args: vec!["-c".into(), "readlink /proc/$$/fd/0".into()],
        ..Default::default()
    };
    let (_dir, m) = new_test_manager(vec![target]);
    m.start("stdin-check").expect("start");
    thread::sleep(Duration::from_millis(400));

    let dest_dir = tempfile::tempdir().unwrap();
    let dest = dest_dir.path().join("dump.log");
    m.dump_log("stdin-check", &dest).expect("dump_log");
    let data = fs::read_to_string(&dest).unwrap();
    assert!(
        data.contains("/dev/null"),
        "spawned child's fd 0 should be /dev/null, got: {data:?}"
    );
    assert!(
        !data.contains("/dev/pts/"),
        "spawned child's fd 0 must not be a PTY (would freeze tmux post-detach): {data:?}"
    );
}

#[test]
fn attach_to_existing_reconciles_alive() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join(".tukituki");

    let target = sleep_target("attach-test");
    let m1 = Manager::new(vec![target.clone()], &state_dir, dir.path().to_path_buf()).unwrap();
    m1.start("attach-test").expect("start");
    thread::sleep(Duration::from_millis(200));

    // Simulate a new tukituki invocation against the same state file.
    let m2 = Manager::new(vec![target], &state_dir, dir.path().to_path_buf()).unwrap();
    m2.attach_to_existing().expect("attach");

    assert_eq!(m2.get_status("attach-test"), Status::Running);

    // Clean up so the test doesn't leak a child.
    let _ = m1.stop("attach-test");
}

// ---- process-group drain --------------------------------------------

/// Reproduces the `go run` orphan scenario from the Go test of the same
/// name. The shell backgrounds a SIGTERM-ignoring subshell and execs
/// `sleep`; the leader dies fast on SIGTERM but the subshell survives.
/// `stop` must wait for the whole group to drain — anything less leaks
/// orphans into the user's process list.
#[test]
fn stop_drains_process_group() {
    let target = RunTarget {
        name: "group-drain".into(),
        command: "sh".into(),
        args: vec![
            "-c".into(),
            "{ trap '' TERM; sleep 30; } & exec sleep 30".into(),
        ],
        ..Default::default()
    };
    let (_dir, m) = new_test_manager(vec![target]);

    m.start("group-drain").expect("start");
    thread::sleep(Duration::from_millis(300));

    let leader_pid = m
        .get_all_process_states()
        .get("group-drain")
        .map(|ps| ps.pid)
        .expect("leader pid");
    assert!(leader_pid > 0, "leader pid not set");
    assert!(
        group_alive(leader_pid),
        "group {leader_pid} should be alive after start"
    );

    let start = Instant::now();
    m.stop("group-drain").expect("stop");
    let elapsed = start.elapsed();

    // Allow the kernel a moment to reap stragglers, then re-check.
    if group_alive(leader_pid) {
        thread::sleep(Duration::from_millis(200));
        if group_alive(leader_pid) {
            // Best-effort cleanup so the test doesn't pollute the
            // user's process list.
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-leader_pid),
                nix::sys::signal::Signal::SIGKILL,
            );
            panic!(
                "process group {leader_pid} still has members after stop (elapsed {elapsed:?}) — orphans leaked"
            );
        }
    }

    // The SIGTERM-trap branch forces us into the SIGKILL path, so stop
    // must take at least the 5s SIGTERM grace period.
    assert!(
        elapsed >= Duration::from_secs(4),
        "stop returned in {elapsed:?}; expected ≥5s because SIGKILL path is required"
    );
}

fn group_alive(leader_pid: i32) -> bool {
    if leader_pid <= 0 {
        return false;
    }
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    match kill(Pid::from_raw(-leader_pid), None) {
        Ok(_) => true,
        Err(Errno::ESRCH) => false,
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

// ---- Manager::new ----------------------------------------------------

#[test]
fn new_manager_creates_dirs() {
    let base = tempfile::tempdir().unwrap();
    let state_dir = base.path().join("deep").join("nested").join(".tukituki");

    let _m = Manager::new(vec![], &state_dir, base.path().to_path_buf()).expect("new");
    let logs_dir = state_dir.join("logs");
    assert!(
        logs_dir.is_dir(),
        "logs dir was not created: {}",
        logs_dir.display()
    );
}

// ---- OTel port resolution -------------------------------------------

fn fresh_state_dir() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join(".tukituki");
    (dir, state_dir)
}

/// Serialises the OTel port tests against each other.
///
/// `allocate_free_port` hands back a port and drops its listener, so the
/// number is only advisory — any concurrent binder can take it first.
/// These four tests are the heaviest port churners in the binary, and
/// letting them interleave made them steal each other's ports. Holding
/// this for the duration of each removes that class of failure; the
/// guard ignores poisoning so one test's panic doesn't cascade.
static PORT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn port_test_guard() -> std::sync::MutexGuard<'static, ()> {
    PORT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn read_port_file(state_dir: &std::path::Path) -> u16 {
    let raw = fs::read_to_string(state_dir.join("otel-port")).expect("otel-port file");
    raw.trim().parse().expect("port int")
}

#[test]
fn set_otel_config_picks_and_persists_port() {
    let _port_guard = port_test_guard();
    let (base, state_dir) = fresh_state_dir();
    let m = Manager::new(vec![], &state_dir, base.path().to_path_buf()).unwrap();

    m.set_otel_config(OtelConfig {
        port: 0,
        protocol: "grpc".into(),
        severity: "error".into(),
    });

    let assigned = m.otel_receiver_port();
    assert!(assigned > 0, "expected a non-zero assigned port");
    assert_eq!(
        read_port_file(&state_dir),
        assigned,
        "persisted port must match in-memory port"
    );
}

#[test]
fn set_otel_config_reuses_persisted_port() {
    let _port_guard = port_test_guard();
    let (base, state_dir) = fresh_state_dir();

    let m1 = Manager::new(vec![], &state_dir, base.path().to_path_buf()).unwrap();
    m1.set_otel_config(OtelConfig {
        port: 0,
        protocol: "grpc".into(),
        severity: "error".into(),
    });
    let first = m1.otel_receiver_port();

    let m2 = Manager::new(vec![], &state_dir, base.path().to_path_buf()).unwrap();
    m2.set_otel_config(OtelConfig {
        port: 0,
        protocol: "grpc".into(),
        severity: "error".into(),
    });

    let second = m2.otel_receiver_port();
    if second == first {
        return;
    }

    // Declining to reuse is correct in exactly one case: the persisted
    // port stopped being bindable. `allocate_free_port` returns a number
    // without reserving it, so any process on the machine — including a
    // sibling cargo test binary — can claim it between the two Managers.
    // Asserting the port simply stayed free asserts something this test
    // does not control, which is what made it flaky.
    //
    // Checking bindability still catches the regression that matters: if
    // the reuse path breaks, the port is free and this fails.
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", first)).is_err(),
        "port drifted from {first} to {second} while {first} was still \
         bindable — the persisted port should have been reused"
    );
}

#[test]
fn set_otel_config_explicit_port_persists() {
    let _port_guard = port_test_guard();
    let (base, state_dir) = fresh_state_dir();
    let m = Manager::new(vec![], &state_dir, base.path().to_path_buf()).unwrap();

    let explicit = tukituki_process_test_helpers::allocate_port();
    m.set_otel_config(OtelConfig {
        port: explicit,
        protocol: "grpc".into(),
        severity: "error".into(),
    });

    assert_eq!(m.otel_receiver_port(), explicit, "explicit port honoured");
    assert_eq!(
        read_port_file(&state_dir),
        explicit,
        "explicit port persisted"
    );
}

#[test]
fn set_otel_config_stolen_port_fallback() {
    let _port_guard = port_test_guard();
    let (base, state_dir) = fresh_state_dir();
    fs::create_dir_all(&state_dir).unwrap();

    // Seed the port file with a port we occupy for the whole test. Bind
    // first and read the port back off the live listener: allocating a
    // port and *then* re-binding it leaves a window where another test
    // can take it, which made this line fail intermittently.
    let _listener = std::net::TcpListener::bind("127.0.0.1:0").expect("occupy port");
    let stolen = _listener.local_addr().unwrap().port();
    fs::write(state_dir.join("otel-port"), stolen.to_string()).unwrap();

    let m = Manager::new(vec![], &state_dir, base.path().to_path_buf()).unwrap();
    m.set_otel_config(OtelConfig {
        port: 0,
        protocol: "grpc".into(),
        severity: "error".into(),
    });

    let assigned = m.otel_receiver_port();
    assert_ne!(assigned, stolen, "must not reuse the stolen port");
    assert!(assigned > 0, "fallback should pick a fresh port");
}

// ---- .env overlay ----------------------------------------------------

/// Target that dumps its environment into its log.
fn env_dump_target(name: &str) -> RunTarget {
    RunTarget {
        name: name.into(),
        command: "env".into(),
        ..Default::default()
    }
}

/// Read the child's log via the public dump_log API.
fn child_log(m: &Manager, name: &str) -> String {
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = dest_dir.path().join("dump.log");
    m.dump_log(name, &dest).expect("dump_log");
    fs::read_to_string(&dest).unwrap()
}

#[test]
fn dotenv_reaches_spawned_child_without_yaml_mapping() {
    let (dir, m) = new_test_manager(vec![env_dump_target("envy")]);
    fs::write(
        dir.path().join(".env"),
        "TUKITUKI_TEST_DOTENV_PLAIN=from_dotenv\n",
    )
    .unwrap();

    m.start("envy").expect("start");
    thread::sleep(Duration::from_millis(400));

    let log = child_log(&m, "envy");
    assert!(
        log.contains("TUKITUKI_TEST_DOTENV_PLAIN=from_dotenv"),
        "child env missing .env var: {log:?}"
    );
}

#[test]
fn target_env_overrides_dotenv() {
    let mut t = env_dump_target("envy");
    t.env
        .insert("TUKITUKI_TEST_DOTENV_SHADOWED".into(), "from_target".into());
    let (dir, m) = new_test_manager(vec![t]);
    fs::write(
        dir.path().join(".env"),
        "TUKITUKI_TEST_DOTENV_SHADOWED=from_dotenv\n",
    )
    .unwrap();

    m.start("envy").expect("start");
    thread::sleep(Duration::from_millis(400));

    let log = child_log(&m, "envy");
    assert!(
        log.contains("TUKITUKI_TEST_DOTENV_SHADOWED=from_target"),
        "target env block must win over .env: {log:?}"
    );
}

#[test]
fn restart_rereads_dotenv() {
    let (dir, m) = new_test_manager(vec![env_dump_target("envy")]);
    let env_path = dir.path().join(".env");
    fs::write(&env_path, "TUKITUKI_TEST_DOTENV_FRESH=first\n").unwrap();

    m.start("envy").expect("start");
    thread::sleep(Duration::from_millis(400));
    assert!(child_log(&m, "envy").contains("TUKITUKI_TEST_DOTENV_FRESH=first"));

    // Edit .env while tukituki keeps running; the next spawn must see
    // the new value — this is the whole point of reading at spawn time
    // instead of seeding the manager's own environment once.
    fs::write(&env_path, "TUKITUKI_TEST_DOTENV_FRESH=second\n").unwrap();
    m.restart("envy").expect("restart");
    thread::sleep(Duration::from_millis(400));

    let log = child_log(&m, "envy");
    assert!(
        log.contains("TUKITUKI_TEST_DOTENV_FRESH=second"),
        "restart must re-read .env: {log:?}"
    );
}

// ---- stranded state entries -----------------------------------------

#[test]
fn stop_all_sweeps_processes_whose_target_was_deleted() {
    // Models the real failure: a target is started, then removed from
    // `.run/*.yaml` (a refactor that folded two workers into one). Its
    // process keeps running and stays recorded in state.json, but every
    // stop path keys off the target list — so before this sweep it
    // survived every subsequent `stop`/`restart`, indefinitely.
    let (_dir, m) = new_test_manager(vec![sleep_target("keeper"), sleep_target("removed")]);

    m.start_all().expect("start_all");
    thread::sleep(Duration::from_millis(300));
    assert_eq!(m.get_status("removed"), Status::Running);

    // The target disappears from the YAML; the process does not.
    m.update_targets(vec![sleep_target("keeper")]);
    assert_eq!(
        m.orphaned_process_names(),
        vec!["removed".to_string()],
        "a recorded process with no target must be reported as orphaned"
    );

    m.stop_all().expect("stop_all");
    thread::sleep(Duration::from_millis(300));

    let s = m.get_status("removed");
    assert_ne!(
        s,
        Status::Running,
        "stop_all must sweep a process whose target was deleted, got {s:?}"
    );
}

#[test]
fn orphan_sweep_ignores_targets_that_still_exist() {
    // The sweep must not double-stop live targets or claim the virtual
    // collector — otherwise stop_all would report bogus orphans.
    let (_dir, m) = new_test_manager(vec![sleep_target("alpha")]);
    m.start("alpha").expect("start");
    thread::sleep(Duration::from_millis(200));

    assert!(
        m.orphaned_process_names().is_empty(),
        "a target that still exists is not an orphan: {:?}",
        m.orphaned_process_names()
    );
    assert!(m.has_recorded_process("alpha"));
    assert!(!m.has_recorded_process("never-started"));

    m.stop_all().expect("stop_all");
}

// ---- superseded otel collectors --------------------------------------

/// Read the collector ledger the manager maintains at
/// `<state_dir>/otel-pids`.
fn ledger(dir: &TempDir) -> Vec<i32> {
    let p = dir.path().join(".tukituki/otel-pids");
    fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .collect()
}

fn write_ledger(dir: &TempDir, pids: &[i32]) {
    let state_dir = dir.path().join(".tukituki");
    fs::create_dir_all(&state_dir).unwrap();
    let body: String = pids.iter().map(|p| format!("{p}\n")).collect();
    fs::write(state_dir.join("otel-pids"), body).unwrap();
}

#[test]
fn sweep_reaps_a_superseded_collector() {
    // state.json holds one `otel-errors` entry, so recording a
    // replacement collector forgets the previous PID. When that PID is
    // still alive it becomes unreachable and keeps its OTLP port — the
    // real symptom was several collectors per project, the oldest days
    // old. The ledger is what lets the sweep still find it.
    let (dir, m) = new_test_manager(vec![sleep_target("ghost")]);
    m.start("ghost").expect("start");
    thread::sleep(Duration::from_millis(250));

    let pid = m.get_all_process_states()["ghost"].pid;
    assert!(pid > 0);

    // Model a collector this project spawned that state no longer names.
    write_ledger(&dir, &[pid]);
    m.sweep_stale_otel_collectors();
    thread::sleep(Duration::from_millis(300));

    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "superseded collector (pid {pid}) must be reaped"
    );
    assert!(
        ledger(&dir).is_empty(),
        "reaped pid must be dropped from the ledger, got {:?}",
        ledger(&dir)
    );
}

#[test]
fn sweep_spares_the_currently_tracked_collector() {
    // The sweep must never kill the collector state.json actively
    // points at — stop_otel_collector owns that one. Getting this wrong
    // would tear down the live collector on every start.
    let (dir, m) = new_test_manager(vec![sleep_target(tukituki_process::OTEL_TARGET_NAME)]);
    m.start(tukituki_process::OTEL_TARGET_NAME).expect("start");
    thread::sleep(Duration::from_millis(250));

    let pid = m.get_all_process_states()[tukituki_process::OTEL_TARGET_NAME].pid;
    assert!(pid > 0);

    write_ledger(&dir, &[pid]);
    m.sweep_stale_otel_collectors();
    thread::sleep(Duration::from_millis(250));

    assert!(
        Path::new(&format!("/proc/{pid}")).exists(),
        "the tracked collector (pid {pid}) must survive the sweep"
    );
    assert_eq!(
        ledger(&dir),
        vec![pid],
        "the tracked collector stays in the ledger"
    );

    m.stop(tukituki_process::OTEL_TARGET_NAME).ok();
}

#[test]
fn sweep_prunes_dead_ledger_entries_without_signalling() {
    // A PID that already exited should just fall out of the ledger.
    // Left in place the file would grow forever, and a recycled PID
    // could eventually be signalled by mistake.
    let (dir, m) = new_test_manager(vec![sleep_target("unused")]);
    write_ledger(&dir, &[9_999_992]);

    m.sweep_stale_otel_collectors();

    assert!(
        ledger(&dir).is_empty(),
        "dead pid should be pruned, got {:?}",
        ledger(&dir)
    );
}

mod tukituki_process_test_helpers {
    /// Local helper: allocate a free TCP port without depending on the
    /// crate's private otel_port module.
    pub fn allocate_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }
}
