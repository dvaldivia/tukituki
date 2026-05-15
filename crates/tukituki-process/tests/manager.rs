//! Integration tests for the process Manager.
//!
//! These tests spawn real subprocesses and verify lifecycle behaviour.
//! Run serially under cargo's default parallelism — they don't share
//! state directories and each one writes to its own tempdir.

use std::fs;
use std::path::PathBuf;
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

fn read_port_file(state_dir: &std::path::Path) -> u16 {
    let raw = fs::read_to_string(state_dir.join("otel-port")).expect("otel-port file");
    raw.trim().parse().expect("port int")
}

#[test]
fn set_otel_config_picks_and_persists_port() {
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

    assert_eq!(
        m2.otel_receiver_port(),
        first,
        "port drifted across Manager instances"
    );
}

#[test]
fn set_otel_config_explicit_port_persists() {
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
    let (base, state_dir) = fresh_state_dir();
    fs::create_dir_all(&state_dir).unwrap();

    // Seed the port file with a port we then occupy with an unrelated
    // listener. With no otel-errors process recorded, the saved port
    // must be treated as unusable and a fresh one allocated.
    let stolen = tukituki_process_test_helpers::allocate_port();
    fs::write(state_dir.join("otel-port"), stolen.to_string()).unwrap();
    let _listener = std::net::TcpListener::bind(("127.0.0.1", stolen)).expect("occupy port");

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

mod tukituki_process_test_helpers {
    /// Local helper: allocate a free TCP port without depending on the
    /// crate's private otel_port module.
    pub fn allocate_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }
}
