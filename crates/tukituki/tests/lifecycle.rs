//! End-to-end tests for the start/stop/restart/status subcommands.
//!
//! Each test sets up a fresh tempdir with `.run/` and `.tukituki/` so
//! no two tests share state. Targets use `sh -c sleep ...` so they
//! survive across multiple subcommand invocations.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use tempfile::TempDir;

/// Build a tempdir with a `.run/` containing the given files. Returns
/// the dir handle (drop = cleanup).
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

const SLEEPER: &str = r#"
name: sleeper
command: sh
args: ["-c", "echo started && sleep 60"]
description: long-running sleeper
"#;

const QUICK: &str = r#"
name: quick
command: sh
args: ["-c", "echo done"]
"#;

// ---- start / stop ----------------------------------------------------

#[test]
fn start_then_status_then_stop() {
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);

    tt_in(dir.path())
        .args(["start", "sleeper"])
        .assert()
        .success();
    // Give the shell time to exec the sleep.
    thread::sleep(Duration::from_millis(300));

    let out = tt_in(dir.path())
        .args(["status", "sleeper", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["name"], "sleeper");
    assert_eq!(v["status"], "running");
    assert!(v["pid"].as_i64().unwrap_or(0) > 0);

    tt_in(dir.path())
        .args(["stop", "sleeper"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));

    let out2 = tt_in(dir.path())
        .args(["status", "sleeper", "--json"])
        .assert()
        .success();
    let stdout2 = String::from_utf8(out2.get_output().stdout.clone()).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&stdout2).unwrap();
    assert_ne!(v2["status"], "running", "expected non-running: {stdout2}");
}

#[test]
fn start_all_then_status_array() {
    let dir = fixture(&[
        (
            "a.yaml",
            "name: a\ncommand: sh\nargs: [\"-c\", \"sleep 60\"]\n",
        ),
        (
            "b.yaml",
            "name: b\ncommand: sh\nargs: [\"-c\", \"sleep 60\"]\n",
        ),
    ]);

    tt_in(dir.path()).arg("start").assert().success();
    thread::sleep(Duration::from_millis(300));

    let out = tt_in(dir.path())
        .args(["status", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    for e in arr {
        assert_eq!(e["status"], "running", "all should be running: {stdout}");
    }

    // Cleanup.
    tt_in(dir.path()).arg("stop").assert().success();
}

#[test]
fn status_json_object_vs_array() {
    // With a target argument: single object. Without: array.
    let dir = fixture(&[("quick.yaml", QUICK)]);

    let out_one = tt_in(dir.path())
        .args(["status", "quick", "--json"])
        .assert()
        .success();
    let s_one = String::from_utf8(out_one.get_output().stdout.clone()).unwrap();
    let v_one: serde_json::Value = serde_json::from_str(&s_one).unwrap();
    assert!(
        v_one.is_object(),
        "single-target status must be object: {s_one}"
    );

    let out_all = tt_in(dir.path())
        .args(["status", "--json"])
        .assert()
        .success();
    let s_all = String::from_utf8(out_all.get_output().stdout.clone()).unwrap();
    let v_all: serde_json::Value = serde_json::from_str(&s_all).unwrap();
    assert!(v_all.is_array(), "all-target status must be array: {s_all}");
}

#[test]
fn status_text_has_columns() {
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);
    let out = tt_in(dir.path()).arg("status").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("NAME"));
    assert!(stdout.contains("STATUS"));
    assert!(stdout.contains("DESCRIPTION"));
    assert!(stdout.contains("sleeper"));
}

// ---- restart ---------------------------------------------------------

#[test]
fn restart_changes_pid() {
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);

    tt_in(dir.path())
        .args(["start", "sleeper"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));

    let out_a = tt_in(dir.path())
        .args(["status", "sleeper", "--json"])
        .assert()
        .success();
    let v_a: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out_a.get_output().stdout.clone()).unwrap())
            .unwrap();
    let pid_a = v_a["pid"].as_i64().unwrap();

    tt_in(dir.path())
        .args(["restart", "sleeper"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));

    let out_b = tt_in(dir.path())
        .args(["status", "sleeper", "--json"])
        .assert()
        .success();
    let v_b: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out_b.get_output().stdout.clone()).unwrap())
            .unwrap();
    let pid_b = v_b["pid"].as_i64().unwrap();

    assert_ne!(pid_a, pid_b, "restart must produce a new PID");
    assert_eq!(v_b["status"], "running");

    tt_in(dir.path())
        .args(["stop", "sleeper"])
        .assert()
        .success();
}

#[test]
fn restart_unknown_target_validated_before_acting() {
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);
    // The restart should fail with a target-not-found error and NOT
    // bounce the sleeper that we never started.
    let out = tt_in(dir.path())
        .args(["restart", "no-such", "--json"])
        .assert()
        .failure();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).expect("stderr should be JSON");
    assert!(v.get("error").is_some());
    assert!(
        v.get("available").is_some(),
        "available list missing: {stderr}"
    );
}

// ---- start/stop JSON shapes -----------------------------------------

#[test]
fn start_single_target_json_returns_object() {
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);
    let out = tt_in(dir.path())
        .args(["start", "sleeper", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(v.is_object());
    assert_eq!(v["name"], "sleeper");
    // Field order must match Go's actionResult: name, status.
    let n = stdout.find("\"name\"").unwrap();
    let s = stdout.find("\"status\"").unwrap();
    assert!(n < s, "field order off: {stdout}");

    tt_in(dir.path())
        .args(["stop", "sleeper"])
        .assert()
        .success();
}

#[test]
fn stop_single_target_json_returns_object() {
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);
    tt_in(dir.path())
        .args(["start", "sleeper"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(200));

    let out = tt_in(dir.path())
        .args(["stop", "sleeper", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["name"], "sleeper");
    assert_eq!(v["status"], "stopped");
}

#[test]
fn start_idempotent_when_already_running() {
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);
    tt_in(dir.path())
        .args(["start", "sleeper"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));

    let pid_a = pid_of(dir.path(), "sleeper");
    // Second start should be a no-op — same PID.
    tt_in(dir.path())
        .args(["start", "sleeper"])
        .assert()
        .success();
    let pid_b = pid_of(dir.path(), "sleeper");
    assert_eq!(pid_a, pid_b, "second start must not respawn");

    tt_in(dir.path())
        .args(["stop", "sleeper"])
        .assert()
        .success();
}

fn pid_of(dir: &Path, name: &str) -> i64 {
    let out = tt_in(dir)
        .args(["status", name, "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    v["pid"].as_i64().unwrap_or(0)
}
