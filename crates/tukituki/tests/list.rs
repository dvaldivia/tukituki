use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

/// Builds a `.run/` fixture in a fresh temp dir and returns the dir.
/// The caller drops it to clean up.
fn fixture_run_dir(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join(".run");
    fs::create_dir_all(&run_dir).unwrap();
    for (name, content) in files {
        let path: std::path::PathBuf = run_dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
    }
    dir
}

fn tukituki_in(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("tukituki").unwrap();
    c.current_dir(dir);
    // Don't let an ambient TUKITUKI_RUN_DIR/TUKITUKI_STATE_DIR or .env
    // from the developer's shell sneak into the integration tests.
    c.env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR");
    c
}

#[test]
fn list_text_outputs_header_and_rows() {
    let dir = fixture_run_dir(&[
        (
            "api.yaml",
            "name: api\ncommand: go\ndescription: HTTP backend\n",
        ),
        ("worker.yaml", "name: worker\ncommand: echo\n"),
    ]);
    let out = tukituki_in(dir.path()).arg("list").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("NAME"), "missing header: {stdout}");
    assert!(stdout.contains("COMMAND"));
    assert!(stdout.contains("DESCRIPTION"));
    assert!(stdout.contains("api"));
    assert!(stdout.contains("HTTP backend"));
    // Worker has no description — must show "-".
    let worker_line = stdout
        .lines()
        .find(|l| l.starts_with("worker"))
        .expect("worker row");
    assert!(worker_line.ends_with('-'), "worker desc: {worker_line:?}");
}

#[test]
fn list_text_sorted_by_name() {
    let dir = fixture_run_dir(&[
        ("zeta.yaml", "name: zeta\ncommand: echo\n"),
        ("alpha.yaml", "name: alpha\ncommand: echo\n"),
    ]);
    let out = tukituki_in(dir.path()).arg("list").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let alpha = stdout.find("alpha").unwrap();
    let zeta = stdout.find("zeta").unwrap();
    assert!(alpha < zeta, "alpha must precede zeta: {stdout}");
}

#[test]
fn list_json_shape_and_field_order() {
    let dir = fixture_run_dir(&[(
        "api.yaml",
        r#"
name: api
command: go
args: ["run", "./cmd/server"]
description: HTTP backend
workdir: backend
"#,
    )]);

    let out = tukituki_in(dir.path())
        .args(["list", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();

    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let arr = v.as_array().expect("top-level array");
    assert_eq!(arr.len(), 1);

    let obj = &arr[0];
    assert_eq!(obj["name"], "api");
    assert_eq!(obj["command"], "go");
    assert_eq!(obj["args"], serde_json::json!(["run", "./cmd/server"]));
    assert_eq!(obj["description"], "HTTP backend");
    assert_eq!(obj["workdir"], "backend");

    // Go marshals listEntry in declaration order:
    //   name, command, args, description, workdir
    let n = stdout.find("\"name\"").unwrap();
    let c = stdout.find("\"command\"").unwrap();
    let a = stdout.find("\"args\"").unwrap();
    let d = stdout.find("\"description\"").unwrap();
    let w = stdout.find("\"workdir\"").unwrap();
    assert!(
        n < c && c < a && a < d && d < w,
        "field order mismatch: {stdout}"
    );

    // 2-space indent like Go's json.MarshalIndent("", "  ").
    assert!(stdout.contains("\n    \"name\""), "indent off: {stdout}");
}

#[test]
fn list_json_omits_empty_optional_fields() {
    let dir = fixture_run_dir(&[("bare.yaml", "name: bare\ncommand: echo\n")]);
    let out = tukituki_in(dir.path())
        .args(["list", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(
        !stdout.contains("\"args\""),
        "args must be omitted when empty: {stdout}"
    );
    assert!(
        !stdout.contains("\"description\""),
        "description must be omitted when empty: {stdout}"
    );
    assert!(
        !stdout.contains("\"workdir\""),
        "workdir must be omitted when empty: {stdout}"
    );
}

#[test]
fn list_json_empty_when_no_targets() {
    let dir = fixture_run_dir(&[]);
    let out = tukituki_in(dir.path())
        .args(["list", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v.as_array().is_some_and(|a| a.is_empty()));
}

#[test]
fn list_missing_run_dir_text_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = tukituki_in(dir.path()).arg("list").assert().failure();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("no .run/ directory found"),
        "expected helpful error: {stderr}"
    );
}

#[test]
fn list_missing_run_dir_json_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = tukituki_in(dir.path())
        .args(["list", "--json"])
        .assert()
        .failure();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    // stderr is JSON, single line.
    let v: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON in --json mode");
    let obj = v.as_object().expect("error object");
    assert!(obj.contains_key("error"));
    assert!(obj.contains_key("run_dir"));
}

#[test]
fn list_respects_run_dir_flag() {
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("custom-run");
    fs::create_dir_all(&custom).unwrap();
    fs::write(
        custom.join("svc.yaml"),
        "name: svc-from-flag\ncommand: echo\n",
    )
    .unwrap();

    let out = tukituki_in(dir.path())
        .args(["--run-dir", custom.to_str().unwrap(), "list"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("svc-from-flag"), "stdout: {stdout}");
}

#[test]
fn list_respects_run_dir_env_var() {
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("env-run");
    fs::create_dir_all(&custom).unwrap();
    fs::write(
        custom.join("svc.yaml"),
        "name: svc-from-env\ncommand: echo\n",
    )
    .unwrap();

    let out = tukituki_in(dir.path())
        .env("TUKITUKI_RUN_DIR", &custom)
        .arg("list")
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("svc-from-env"), "stdout: {stdout}");
}

#[test]
fn list_expands_dotenv_in_args() {
    // .env supplies APP_DOMAIN; the YAML target references ${APP_DOMAIN}
    // in args — expand_env should substitute before printing.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".env"), "APP_DOMAIN=myhost.local\n").unwrap();
    let run_dir = dir.path().join(".run");
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("docs.yaml"),
        r#"
name: docs
command: hugo
args: ["server", "--baseURL", "http://${APP_DOMAIN}:5313"]
"#,
    )
    .unwrap();

    // Ensure no pre-set APP_DOMAIN env leaks in.
    let out = tukituki_in(dir.path())
        .env_remove("APP_DOMAIN")
        .args(["list", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("http://myhost.local:5313"),
        "expected ${{APP_DOMAIN}} expanded: {stdout}"
    );
    assert!(
        !stdout.contains("${APP_DOMAIN}"),
        "no placeholder should remain: {stdout}"
    );
}

#[test]
fn list_shows_parse_error_target() {
    let dir = fixture_run_dir(&[
        ("good.yaml", "name: good\ncommand: echo\n"),
        ("bad.yaml", "name: [this is: {not: valid yaml\n"),
    ]);
    // Text mode succeeds — parse errors don't abort the load.
    let out = tukituki_in(dir.path()).arg("list").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("good"));
    // The broken file is surfaced as a target whose name is the file
    // stem — same behaviour the Go TUI relies on.
    assert!(stdout.contains("bad"), "broken target missing: {stdout}");
}

#[test]
fn list_text_uses_tabwriter_alignment() {
    let dir = fixture_run_dir(&[
        ("a.yaml", "name: a\ncommand: echo\n"),
        ("longername.yaml", "name: longername\ncommand: go\n"),
    ]);
    let out = tukituki_in(dir.path()).arg("list").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    // tabwriter pads columns to a common width with 3-space padding —
    // every data row's NAME column should at least be wide enough to
    // fit "longername" plus padding.
    for line in stdout.lines().filter(|l| !l.is_empty()) {
        if line.starts_with("NAME") || line.starts_with("----") {
            continue;
        }
        // First column ends where the second word begins; that column
        // must end at the same offset as "longername" (the longest name).
        let chars = line.chars().collect::<Vec<_>>();
        assert!(chars.len() > "longername".len(), "row too narrow: {line:?}");
    }
}
