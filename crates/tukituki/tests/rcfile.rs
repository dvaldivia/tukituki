//! Config-file precedence tests.
//!
//! Precedence chain (matches Go's viper setup):
//!     CLI flag > env var > .tukitukirc.yaml > built-in default.
//!
//! These tests exercise the precedence by varying which layer supplies
//! `run_dir` and asserting the right targets show up in `list --json`
//! (each layer points at a different fixture directory).

use std::fs;
use std::path::Path;

use assert_cmd::Command;

fn tt(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("tukituki").unwrap();
    c.current_dir(dir);
    c.env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR")
        .env_remove("HOME");
    c
}

/// Builds a fresh tempdir with three potential `.run/` locations:
///   - `default/.run/` named "default-target"
///   - `from-rc/` named "rc-target"
///   - `from-env/` named "env-target"
///   - `from-flag/` named "flag-target"
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let layers = [
        ("default/.run", "default-target"),
        ("from-rc", "rc-target"),
        ("from-env", "env-target"),
        ("from-flag", "flag-target"),
    ];
    for (sub, name) in layers {
        let path = dir.path().join(sub);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join(format!("{name}.yaml")),
            format!("name: {name}\ncommand: echo\n"),
        )
        .unwrap();
    }
    // Layout so `.run` exists in the cwd by default → "default-target".
    let cwd_run = dir.path().join(".run");
    fs::create_dir_all(&cwd_run).unwrap();
    fs::write(
        cwd_run.join("default.yaml"),
        "name: default-target\ncommand: echo\n",
    )
    .unwrap();
    dir
}

fn list_names_json(out: assert_cmd::assert::Assert) -> Vec<String> {
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn default_run_dir_when_no_layer_set() {
    let dir = fixture();
    let names = list_names_json(tt(dir.path()).args(["list", "--json"]).assert().success());
    assert_eq!(names, vec!["default-target".to_string()]);
}

#[test]
fn rc_file_in_cwd_overrides_default() {
    let dir = fixture();
    let rc = dir.path().join(".tukitukirc.yaml");
    fs::write(&rc, "run_dir: from-rc\n").unwrap();

    let names = list_names_json(tt(dir.path()).args(["list", "--json"]).assert().success());
    assert_eq!(names, vec!["rc-target".to_string()]);
}

#[test]
fn env_overrides_rc_file() {
    let dir = fixture();
    let rc = dir.path().join(".tukitukirc.yaml");
    fs::write(&rc, "run_dir: from-rc\n").unwrap();

    let mut cmd = tt(dir.path());
    cmd.env("TUKITUKI_RUN_DIR", dir.path().join("from-env"));
    let names = list_names_json(cmd.args(["list", "--json"]).assert().success());
    assert_eq!(names, vec!["env-target".to_string()]);
}

#[test]
fn flag_overrides_env() {
    let dir = fixture();
    let rc = dir.path().join(".tukitukirc.yaml");
    fs::write(&rc, "run_dir: from-rc\n").unwrap();

    let mut cmd = tt(dir.path());
    cmd.env("TUKITUKI_RUN_DIR", dir.path().join("from-env"));
    let names = list_names_json(
        cmd.args([
            "--run-dir",
            dir.path().join("from-flag").to_str().unwrap(),
            "list",
            "--json",
        ])
        .assert()
        .success(),
    );
    assert_eq!(names, vec!["flag-target".to_string()]);
}

#[test]
fn explicit_config_path_wins_over_cwd_rc() {
    let dir = fixture();
    // cwd rc says from-rc, explicit rc says from-env.
    fs::write(dir.path().join(".tukitukirc.yaml"), "run_dir: from-rc\n").unwrap();
    let explicit = dir.path().join("explicit-rc.yaml");
    fs::write(&explicit, "run_dir: from-env\n").unwrap();

    let mut cmd = tt(dir.path());
    let names = list_names_json(
        cmd.args(["--config", explicit.to_str().unwrap(), "list", "--json"])
            .assert()
            .success(),
    );
    assert_eq!(names, vec!["env-target".to_string()]);
}

#[test]
fn rc_file_in_home_used_when_cwd_has_none() {
    let dir = fixture();
    // No cwd rc. HOME's .tukitukirc.yaml says from-rc.
    fs::write(
        dir.path().join("home-rc.yaml"), // sentinel; the real file is below
        "run_dir: from-rc\n",
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join(".tukitukirc.yaml"), "run_dir: from-rc\n").unwrap();

    // Custom cwd that has no .run override.
    let cwd = tempfile::tempdir().unwrap();
    // Copy the from-rc directory into the same root so the relative
    // "from-rc" path resolves: easiest is to symlink the fixture's
    // from-rc into the new cwd.
    std::os::unix::fs::symlink(dir.path().join("from-rc"), cwd.path().join("from-rc")).unwrap();

    let mut cmd = Command::cargo_bin("tukituki").unwrap();
    cmd.current_dir(cwd.path())
        .env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR")
        .env("HOME", home.path());
    let names = list_names_json(cmd.args(["list", "--json"]).assert().success());
    assert_eq!(names, vec!["rc-target".to_string()]);
}
