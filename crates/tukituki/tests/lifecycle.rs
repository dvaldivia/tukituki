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

// ---- tags targeting --------------------------------------------------

const BACKEND: &str = r#"
name: backend
command: sh
args: ["-c", "echo BACKEND && sleep 60"]
tags: [backend, api]
"#;

const WORKER: &str = r#"
name: worker
command: sh
args: ["-c", "echo WORKER && sleep 60"]
tags: [backend, worker]
"#;

const FRONT: &str = r#"
name: front
command: sh
args: ["-c", "echo FRONT && sleep 60"]
tags: [frontend]
"#;

#[test]
fn restart_with_tags_targets_only_matching() {
    let dir = fixture(&[
        ("backend.yaml", BACKEND),
        ("worker.yaml", WORKER),
        ("front.yaml", FRONT),
    ]);

    // Start all first so we have PIDs to observe.
    tt_in(dir.path()).arg("start").assert().success();
    thread::sleep(Duration::from_millis(300));

    let pid_b = pid_of(dir.path(), "backend");
    let pid_w = pid_of(dir.path(), "worker");
    let pid_f = pid_of(dir.path(), "front");

    // Restart only backend-tagged (backend + worker).
    tt_in(dir.path())
        .args(["restart", "--tags=backend", "--json"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(400));

    let pid_b2 = pid_of(dir.path(), "backend");
    let pid_w2 = pid_of(dir.path(), "worker");
    let pid_f2 = pid_of(dir.path(), "front");

    assert_ne!(pid_b, pid_b2, "backend should have restarted");
    assert_ne!(
        pid_w, pid_w2,
        "worker should have restarted (shares backend tag)"
    );
    assert_eq!(pid_f, pid_f2, "front must be untouched");

    // Cleanup
    let _ = tt_in(dir.path()).args(["stop", "--tags=backend"]).assert();
    let _ = tt_in(dir.path()).args(["stop", "front"]).assert();
}

#[test]
fn start_with_tags_starts_only_matching_and_respects_explicit() {
    let dir = fixture(&[("backend.yaml", BACKEND), ("front.yaml", FRONT)]);

    // Start only backend-tagged; front should remain untouched (and autorun isn't relevant here since explicit tag selection).
    tt_in(dir.path())
        .args(["start", "--tags=backend", "--json"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));

    // backend should be running
    let st = tt_in(dir.path())
        .args(["status", "backend", "--json"])
        .assert()
        .success();
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8(st.get_output().stdout.clone()).unwrap()).unwrap();
    assert_eq!(v["status"], "running");

    // front should not be running (no state or not running)
    let stf = tt_in(dir.path())
        .args(["status", "front", "--json"])
        .assert()
        .success();
    let vf: serde_json::Value =
        serde_json::from_str(&String::from_utf8(stf.get_output().stdout.clone()).unwrap()).unwrap();
    // status may be "unknown" or "stopped"; either way not running
    assert_ne!(
        vf["status"], "running",
        "front must not have been started by --tags=backend"
    );

    let _ = tt_in(dir.path()).args(["stop", "--tags=backend"]).assert();
}

#[test]
fn stop_with_tags_stops_only_matching() {
    let dir = fixture(&[
        ("backend.yaml", BACKEND),
        ("worker.yaml", WORKER),
        ("front.yaml", FRONT),
    ]);

    tt_in(dir.path()).arg("start").assert().success();
    thread::sleep(Duration::from_millis(300));

    // Stop only the frontend-tagged one.
    tt_in(dir.path())
        .args(["stop", "--tags=frontend"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));

    // front should be stopped; the backend ones still running.
    let stf = tt_in(dir.path())
        .args(["status", "front", "--json"])
        .assert()
        .success();
    let vf: serde_json::Value =
        serde_json::from_str(&String::from_utf8(stf.get_output().stdout.clone()).unwrap()).unwrap();
    assert_ne!(vf["status"], "running");

    let stb = tt_in(dir.path())
        .args(["status", "backend", "--json"])
        .assert()
        .success();
    let vb: serde_json::Value =
        serde_json::from_str(&String::from_utf8(stb.get_output().stdout.clone()).unwrap()).unwrap();
    assert_eq!(vb["status"], "running");

    // Cleanup remaining
    let _ = tt_in(dir.path()).args(["stop", "--tags=backend"]).assert();
}

#[test]
fn tags_and_name_together_is_error() {
    let dir = fixture(&[("b.yaml", BACKEND)]);
    // restart
    tt_in(dir.path())
        .args(["restart", "b", "--tags=backend", "--json"])
        .assert()
        .failure();
    // start
    tt_in(dir.path())
        .args(["start", "b", "--tags=backend", "--json"])
        .assert()
        .failure();
    // stop
    tt_in(dir.path())
        .args(["stop", "b", "--tags=backend", "--json"])
        .assert()
        .failure();
    // status
    tt_in(dir.path())
        .args(["status", "b", "--tags=backend", "--json"])
        .assert()
        .failure();
}

// ---- .env pickup -----------------------------------------------------

#[test]
fn start_applies_project_dotenv_with_shell_precedence() {
    let dir = fixture(&[("envy.yaml", "name: envy\ncommand: env\n")]);
    fs::write(
        dir.path().join(".env"),
        "TT_LIFECYCLE_FROM_DOTENV=dotenv_value\nTT_LIFECYCLE_SHELL_WINS=dotenv_value\n",
    )
    .unwrap();

    // The var set on the tukituki process plays the role of a shell
    // export; it must beat the .env entry of the same name.
    tt_in(dir.path())
        .env("TT_LIFECYCLE_SHELL_WINS", "shell_value")
        .args(["start", "envy"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(500));

    let log = fs::read_to_string(dir.path().join(".tukituki/logs/envy.log")).unwrap();
    assert!(
        log.contains("TT_LIFECYCLE_FROM_DOTENV=dotenv_value"),
        "unmapped .env var must reach the child: {log:?}"
    );
    assert!(
        log.contains("TT_LIFECYCLE_SHELL_WINS=shell_value"),
        "shell export must win over .env: {log:?}"
    );
}

#[test]
fn stop_accepts_a_name_whose_run_file_was_deleted() {
    // Deleting a target's YAML while it runs used to strand the
    // process: `stop <name>` failed target resolution and `stop`
    // (all) iterated only the YAML targets, so nothing could signal it.
    let dir = fixture(&[("sleeper.yaml", SLEEPER), ("quick.yaml", QUICK)]);

    tt_in(dir.path())
        .args(["start", "sleeper"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));
    let pid = pid_of(dir.path(), "sleeper");
    assert!(pid > 0, "sleeper should be running");

    // The run file disappears; the process keeps going.
    fs::remove_file(dir.path().join(".run/sleeper.yaml")).unwrap();

    tt_in(dir.path())
        .args(["stop", "sleeper"])
        .assert()
        .success();
    thread::sleep(Duration::from_millis(300));

    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "stop must reach a process whose run file was deleted (pid {pid} still alive)"
    );
}

#[test]
fn stop_still_rejects_an_unknown_name() {
    // The state.json fallback must not swallow typos — an unrecorded,
    // undefined name still has to fail loudly with the available list.
    let dir = fixture(&[("sleeper.yaml", SLEEPER)]);

    tt_in(dir.path())
        .args(["stop", "nonexistent"])
        .assert()
        .failure();
}

fn pid_of(dir: &Path, name: &str) -> i64 {
    let out = tt_in(dir)
        .args(["status", name, "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    v["pid"].as_i64().unwrap_or(0)
}
