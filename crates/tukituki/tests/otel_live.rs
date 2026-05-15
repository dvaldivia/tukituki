//! Live end-to-end test for the OTel collector path.
//!
//! Mirrors Go's `TestLiveTukituki_OtelCollector`. Spawns the real
//! tukituki binary against a fresh `.run/emitter.yaml` that points at
//! the `otel-emitter` example (which ships log records over the
//! injected `OTEL_EXPORTER_OTLP_ENDPOINT`). Reads the resulting
//! `otel-errors.log` and asserts the ERROR was captured and the
//! INFO lines weren't.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use assert_cmd::Command as AssertCommand;
use tempfile::TempDir;

fn cargo_bin(name: &str) -> PathBuf {
    assert_cmd::cargo::cargo_bin(name)
}

/// Locate the `otel-emitter` example binary in target/<profile>/examples.
/// Built on demand at test start.
fn emitter_path() -> PathBuf {
    // `cargo build --example otel-emitter` puts the binary at
    // `target/<profile>/examples/otel-emitter`. The same parent
    // directory as `target/<profile>/deps/` where assert_cmd looks for
    // `tukituki`. Derive it from the test binary location.
    let exe = std::env::current_exe().expect("current_exe");
    // .../target/debug/deps/<test>
    let deps = exe.parent().expect("deps dir");
    // .../target/debug
    let profile = deps.parent().expect("profile dir");
    profile.join("examples").join("otel-emitter")
}

fn ensure_emitter_built() {
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "tukituki-otel", "--example", "otel-emitter"])
        .status()
        .expect("spawn cargo build");
    assert!(status.success(), "build otel-emitter");
}

fn tt() -> AssertCommand {
    AssertCommand::new(cargo_bin("tukituki"))
}

#[test]
fn live_otel_collector_filters_and_captures_error() {
    ensure_emitter_built();
    let emitter = emitter_path();
    assert!(
        emitter.is_file(),
        "emitter binary missing at {}",
        emitter.display()
    );

    let project: TempDir = tempfile::tempdir().expect("project dir");
    let run_dir = project.path().join(".run");
    let state_dir = project.path().join(".tukituki");
    fs::create_dir_all(&run_dir).unwrap();

    fs::write(
        run_dir.join("emitter.yaml"),
        format!(
            "name: emitter\ncommand: {}\notel: true\n",
            emitter.display()
        ),
    )
    .unwrap();

    // Pin a free port so the collector and the otel:true child agree.
    let port = free_port();

    let out = tt()
        .current_dir(project.path())
        .env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR")
        .args([
            "start",
            "--run-dir",
            run_dir.to_str().unwrap(),
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--otel-port",
            &port.to_string(),
        ])
        .assert();
    let out = out.get_output();
    if !out.status.success() {
        panic!(
            "tukituki start failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // The emitter retries gRPC connection for up to 10s, then ships
    // logs. Give it plenty of time.
    let otel_log = state_dir.join("logs/otel-errors.log");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut content = String::new();
    while std::time::Instant::now() < deadline {
        if let Ok(s) = fs::read_to_string(&otel_log)
            && s.contains("[emitter] database connection refused")
        {
            content = s;
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    // Diagnostics on failure.
    if content.is_empty() {
        let dump = fs::read_to_string(&otel_log).unwrap_or_default();
        let emitter_log =
            fs::read_to_string(state_dir.join("logs/emitter.log")).unwrap_or_default();
        let status_out = tt()
            .current_dir(project.path())
            .args([
                "status",
                "--run-dir",
                run_dir.to_str().unwrap(),
                "--state-dir",
                state_dir.to_str().unwrap(),
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let _ = stop_all(project.path(), &run_dir, &state_dir);
        panic!(
            "otel-errors.log does not contain the expected error within 15s.\n\
             status:\n{status_out}\n\
             emitter.log:\n{emitter_log}\n\
             otel-errors.log:\n{dump}"
        );
    }

    // Info lines must NOT have made it through the severity filter.
    for i in 0..20 {
        let info = format!("info log {i}");
        assert!(
            !content.contains(&info),
            "otel-errors.log should not contain {info:?}; full:\n{content}"
        );
    }

    // Cleanup.
    let _ = stop_all(project.path(), &run_dir, &state_dir);
}

fn stop_all(cwd: &Path, run_dir: &Path, state_dir: &Path) -> std::io::Result<()> {
    AssertCommand::new(cargo_bin("tukituki"))
        .current_dir(cwd)
        .env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR")
        .args([
            "stop",
            "--run-dir",
            run_dir.to_str().unwrap(),
            "--state-dir",
            state_dir.to_str().unwrap(),
        ])
        .ok()
        .map(|_| ())
        .map_err(|e| std::io::Error::other(format!("stop: {e}")))
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}
