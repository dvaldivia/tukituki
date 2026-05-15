//! Tailer / ring buffer / watch_log_lines integration tests.

use std::thread;
use std::time::Duration;

use tempfile::TempDir;
use tukituki_config::RunTarget;
use tukituki_process::Manager;

fn echo_target(name: &str, body: &str) -> RunTarget {
    RunTarget {
        name: name.into(),
        command: "sh".into(),
        args: vec!["-c".into(), body.into()],
        ..Default::default()
    }
}

fn new_manager(targets: Vec<RunTarget>) -> (TempDir, Manager) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state_dir = dir.path().join(".tukituki");
    let m = Manager::new(targets, state_dir, dir.path().to_path_buf()).expect("manager");
    (dir, m)
}

/// Poll the in-memory ring buffer for a target until `pred` matches
/// any line, or the timeout expires.
fn wait_for_line<F: Fn(&str) -> bool>(
    m: &Manager,
    name: &str,
    pred: F,
    timeout: Duration,
) -> Vec<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let lines = m.get_log_lines(name);
        if lines.iter().any(|l| pred(l)) {
            return lines;
        }
        if std::time::Instant::now() >= deadline {
            return lines;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn tailer_populates_ring_buffer() {
    let (_dir, m) = new_manager(vec![echo_target(
        "speaker",
        "echo one; echo two; echo three",
    )]);
    m.start("speaker").expect("start");

    let lines = wait_for_line(&m, "speaker", |l| l == "three", Duration::from_secs(3));
    assert!(
        lines.iter().any(|l| l == "one"),
        "ring buffer missing 'one': {lines:?}"
    );
    assert!(lines.iter().any(|l| l == "two"));
    assert!(lines.iter().any(|l| l == "three"));
}

#[test]
fn watch_log_lines_streams_new_output() {
    let (_dir, m) = new_manager(vec![echo_target(
        "streamer",
        // Sleep gives the subscriber time to register before the
        // shell starts producing output.
        "sleep 0.3; for i in 1 2 3; do echo line-$i; done",
    )]);

    m.start("streamer").expect("start");
    let rx = m.watch_log_lines("streamer");

    let mut received: Vec<String> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && received.len() < 3 {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            received.push(line);
        }
    }

    assert!(
        received.iter().any(|l| l == "line-1"),
        "did not see line-1: {received:?}"
    );
    assert!(received.iter().any(|l| l == "line-2"));
    assert!(received.iter().any(|l| l == "line-3"));
}

#[test]
fn append_log_line_writes_to_buffer_and_subscribers() {
    let (_dir, m) = new_manager(vec![]);
    let rx = m.watch_log_lines("diag");

    m.append_log_line("diag", "synthesised diagnostic");

    let lines = m.get_log_lines("diag");
    assert_eq!(lines, vec!["synthesised diagnostic"]);

    let received = rx.recv_timeout(Duration::from_millis(500)).expect("recv");
    assert_eq!(received, "synthesised diagnostic");
}

#[test]
fn append_log_line_splits_multiline_input() {
    let (_dir, m) = new_manager(vec![]);
    m.append_log_line("multi", "first\nsecond\nthird");
    let lines = m.get_log_lines("multi");
    assert_eq!(lines, vec!["first", "second", "third"]);
}

#[test]
fn clear_log_truncates_file_and_buffer() {
    let (_dir, m) = new_manager(vec![echo_target("clearable", "echo before; sleep 5")]);
    m.start("clearable").expect("start");
    let _ = wait_for_line(&m, "clearable", |l| l == "before", Duration::from_secs(2));
    assert!(!m.get_log_lines("clearable").is_empty());

    m.clear_log("clearable").expect("clear_log");
    assert!(
        m.get_log_lines("clearable").is_empty(),
        "ring buffer should be empty after clear"
    );

    // Truncate on disk too: log file size goes back to 0.
    let log_path = m.log_file_path("clearable").expect("log path");
    let size = std::fs::metadata(&log_path).unwrap().len();
    assert_eq!(size, 0, "log file should be 0 bytes after clear");

    let _ = m.stop("clearable");
}

#[test]
fn restart_clears_in_memory_buffer() {
    let (_dir, m) = new_manager(vec![echo_target("restartable", "echo round-one; sleep 5")]);
    m.start("restartable").expect("start");
    let _ = wait_for_line(
        &m,
        "restartable",
        |l| l == "round-one",
        Duration::from_secs(2),
    );

    m.stop("restartable").expect("stop");
    // After stop the buffer still has the previous run's lines.
    assert!(
        m.get_log_lines("restartable")
            .iter()
            .any(|l| l == "round-one"),
        "previous run lines should remain in buffer after stop"
    );

    // start_target replaces the target body — reuse the same RunTarget
    // by calling start (which picks up the targets list).
    m.start("restartable").expect("start 2");
    // The buffer should be cleared immediately on (re)start; even before
    // the new run's output lands, the previous lines must be gone.
    assert!(
        !m.get_log_lines("restartable")
            .iter()
            .any(|l| l == "round-one"),
        "round-one should be cleared on restart"
    );

    let _ = m.stop("restartable");
}

#[test]
fn watch_subscriber_disconnect_is_pruned() {
    // When a subscriber drops their receiver, subsequent appends must
    // not panic and the sender must be pruned. This is a smoke test —
    // we can't observe pruning directly without exposing internals.
    let (_dir, m) = new_manager(vec![]);
    {
        let _rx = m.watch_log_lines("prunable");
        // _rx dropped here — sender's other end is now disconnected.
    }
    // The first append after disconnect prunes the sender.
    m.append_log_line("prunable", "after-disconnect");
    // Subsequent appends should still work and stay snappy.
    for i in 0..10 {
        m.append_log_line("prunable", &format!("line-{i}"));
    }
    let lines = m.get_log_lines("prunable");
    assert_eq!(lines.len(), 11);
}
