//! End-to-end tests for the `logs` subcommand.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

fn fixture(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join(".run");
    fs::create_dir_all(&run_dir).unwrap();
    for (name, content) in files {
        fs::write(run_dir.join(name), content).unwrap();
    }
    dir
}

fn tt_in(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("tukituki").unwrap();
    c.current_dir(dir);
    c.env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR");
    c
}

/// Poll the on-disk log file until it contains `needle`. Replaces a
/// fixed `sleep` after `start` so the tests don't race the child on
/// slow runners.
fn wait_for_log_content(dir: &Path, name: &str, needle: &str) {
    let log_path = dir
        .join(".tukituki")
        .join("logs")
        .join(format!("{name}.log"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(content) = fs::read_to_string(&log_path) {
            if content.contains(needle) {
                return;
            }
        }
        if Instant::now() >= deadline {
            let observed = fs::read_to_string(&log_path).unwrap_or_default();
            panic!(
                "timed out waiting for {needle:?} in {}: {observed:?}",
                log_path.display()
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

const TALKATIVE: &str = r#"
name: talkative
command: sh
args: ["-c", "for i in 1 2 3 4 5 6 7 8 9 10; do echo line-$i; done"]
"#;

const SLEEPER: &str = r#"
name: sleeper
command: sh
args: ["-c", "echo start; sleep 60"]
"#;

#[test]
fn logs_oneshot_prints_buffered_lines() {
    let dir = fixture(&[("talkative.yaml", TALKATIVE)]);

    // Start and let the child finish writing.
    tt_in(dir.path())
        .args(["start", "talkative"])
        .assert()
        .success();
    wait_for_log_content(dir.path(), "talkative", "line-10");

    let out = tt_in(dir.path())
        .args(["logs", "talkative"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    for i in 1..=10 {
        assert!(
            stdout.contains(&format!("line-{i}")),
            "missing line-{i}: {stdout}"
        );
    }
}

#[test]
fn logs_tail_caps_output() {
    let dir = fixture(&[("talkative.yaml", TALKATIVE)]);
    tt_in(dir.path())
        .args(["start", "talkative"])
        .assert()
        .success();
    wait_for_log_content(dir.path(), "talkative", "line-10");

    let out = tt_in(dir.path())
        .args(["logs", "talkative", "--tail", "3"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 lines, got {lines:?}");
    assert_eq!(lines, vec!["line-8", "line-9", "line-10"]);
}

#[test]
fn logs_tail_zero_prints_all() {
    let dir = fixture(&[("talkative.yaml", TALKATIVE)]);
    tt_in(dir.path())
        .args(["start", "talkative"])
        .assert()
        .success();
    wait_for_log_content(dir.path(), "talkative", "line-10");

    let out = tt_in(dir.path())
        .args(["logs", "talkative", "--tail", "0"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    // 10 echoed lines + the appended `(Process exited...)` marker line.
    assert!(
        lines.len() >= 10,
        "expected ≥10 lines, got {n}: {lines:?}",
        n = lines.len()
    );
}

#[test]
fn logs_unknown_target_errors() {
    let dir = fixture(&[("talkative.yaml", TALKATIVE)]);
    let out = tt_in(dir.path())
        .args(["logs", "no-such", "--json"])
        .assert()
        .failure();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON in --json mode");
    assert!(v.get("error").is_some(), "error key missing: {stderr}");
    assert!(
        v.get("available").is_some(),
        "available list missing: {stderr}"
    );
}

#[test]
fn logs_no_state_is_quiet() {
    // Process never started → no state. Should exit cleanly with no
    // output rather than erroring.
    let dir = fixture(&[("talkative.yaml", TALKATIVE)]);
    let out = tt_in(dir.path())
        .args(["logs", "talkative"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.is_empty(), "expected empty stdout: {stdout:?}");
}

#[test]
fn logs_follow_streams_until_process_exits() {
    // `sh -c "echo start; sleep 60"` keeps the channel open; we kill
    // the `tukituki logs` child after grabbing the buffered backlog.
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);
    tt_in(dir.path())
        .args(["start", "sleeper"])
        .assert()
        .success();
    wait_for_log_content(dir.path(), "sleeper", "start");

    // Spawn `tukituki logs sleeper --follow` and read its stdout
    // for ~600ms, then kill the child. Use std::process::Command
    // directly — assert_cmd's wrapper doesn't expose stdio builders.
    let bin = assert_cmd::cargo::cargo_bin("tukituki");
    let mut child = std::process::Command::new(&bin)
        .current_dir(dir.path())
        .env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR")
        .args(["logs", "sleeper", "--follow"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("spawn logs --follow");
    let mut stdout = child.stdout.take().expect("piped stdout");

    let reader = thread::spawn(move || {
        let mut buf = Vec::with_capacity(4096);
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    thread::sleep(Duration::from_millis(700));

    // Kill the follower so reader returns.
    let pid = child.id() as i32;
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    );
    let _ = child.wait();
    let buf = reader.join().expect("reader join");
    let text = String::from_utf8_lossy(&buf);

    // The buffered "start" line should have been streamed.
    assert!(
        text.contains("start"),
        "follow output missing 'start': {text:?}"
    );

    // Cleanup.
    tt_in(dir.path())
        .args(["stop", "sleeper"])
        .assert()
        .success();
}

#[test]
fn logs_strips_null_bytes_from_file() {
    // Manually drop a log file with NULs to verify the filter. We use
    // a state.json that points at this hand-crafted file.
    let dir = fixture(&[("ghost.yaml", "name: ghost\ncommand: true\n")]);
    let state = dir.path().join(".tukituki");
    let logs = state.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let log_path = logs.join("ghost.log");
    fs::write(&log_path, b"hello\x00world\n").unwrap();

    let state_json = serde_json::json!({
        "processes": {
            "ghost": {
                "name": "ghost",
                "pid": 0,
                "log_file": log_path.to_string_lossy().to_string(),
                "started_at": "2026-05-15T00:00:00Z",
                "status": "stopped",
            }
        },
        "updated_at": "2026-05-15T00:00:00Z"
    });
    fs::write(state.join("state.json"), state_json.to_string()).unwrap();

    let out = tt_in(dir.path()).args(["logs", "ghost"]).assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(
        !stdout.contains('\0'),
        "stdout should not contain NUL bytes: {stdout:?}"
    );
    assert!(
        stdout.contains("helloworld"),
        "null-strip output: {stdout:?}"
    );
}
