//! Byte-for-byte golden-file diff for every JSON path.
//!
//! Captured from the Go binary against the canonical fixture in
//! `tests/golden/_fixture/` so a refactor that touches field order,
//! indent, or omitempty semantics fails loudly.
//!
//! Drift policy: the goldens are the contract. If a real behaviour
//! change requires updating a golden, update the file in the same
//! commit so reviewers see the diff.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const FIXTURE: &[(&str, &str)] = &[
    (
        ".run/api.yaml",
        "name: api\ncommand: sh\nargs: [\"-c\", \"sleep 60\"]\ndescription: HTTP API\nworkdir: backend\n",
    ),
    (
        ".run/worker.yaml",
        "name: worker\ncommand: echo\nargs: [\"hello\"]\n",
    ),
    (
        ".run/kb/acme.yaml",
        "name: kb-acme\ncommand: sh\nargs: [\"-c\", \"sleep 60\"]\n",
    ),
];

fn fixture_dir() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, body) in FIXTURE {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, body).unwrap();
    }
    dir
}

fn tt(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("tukituki").unwrap();
    c.current_dir(dir);
    c.env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR");
    c
}

fn read_golden(name: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = Path::new(manifest_dir).join("tests/golden").join(name);
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing golden: {}", path.display()))
}

#[test]
fn list_json_matches_golden() {
    let dir = fixture_dir();
    let out = tt(dir.path()).args(["list", "--json"]).assert().success();
    let got = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let want = read_golden("list.json");
    assert_eq!(got, want, "list --json drift\nGOT:\n{got}\n\nWANT:\n{want}");
}

#[test]
fn status_all_json_matches_golden() {
    let dir = fixture_dir();
    let out = tt(dir.path()).args(["status", "--json"]).assert().success();
    let got = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let want = read_golden("status_all.json");
    assert_eq!(
        got, want,
        "status --json drift\nGOT:\n{got}\n\nWANT:\n{want}"
    );
}

#[test]
fn status_single_target_json_matches_golden() {
    let dir = fixture_dir();
    let out = tt(dir.path())
        .args(["status", "api", "--json"])
        .assert()
        .success();
    let got = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let want = read_golden("status_single.json");
    assert_eq!(
        got, want,
        "status <target> --json drift\nGOT:\n{got}\n\nWANT:\n{want}"
    );
}

#[test]
fn error_unknown_target_json_matches_golden() {
    let dir = fixture_dir();
    let out = tt(dir.path())
        .args(["status", "no-such", "--json"])
        .assert()
        .failure();
    let got = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    let want = read_golden("error_unknown_target.json");
    assert_eq!(
        got, want,
        "unknown-target error drift\nGOT:\n{got}\n\nWANT:\n{want}"
    );
}

#[test]
fn error_missing_run_dir_json_matches_golden() {
    // Run in an empty dir with no `.run/` so `list` errors out.
    let dir = tempfile::tempdir().unwrap();
    let out = tt(dir.path()).args(["list", "--json"]).assert().failure();
    let got = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    let want = read_golden("error_missing_run_dir.json");
    assert_eq!(
        got, want,
        "missing run-dir error drift\nGOT:\n{got}\n\nWANT:\n{want}"
    );
}

// --- version --json: documented field-name drift -----------------
//
// Go emits `go_version`; the Rust port emits `runtime` (see
// plans/rust-port.md and the doc comment on `commands/version.rs`).
// The test pins everything else: the four required keys, their
// alphabetical ordering, the two-space indent, the trailing newline.
#[test]
fn version_json_shape_with_documented_drift() {
    let out = Command::cargo_bin("tukituki")
        .unwrap()
        .args(["version", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();

    assert!(stdout.ends_with('\n'), "trailing newline: {stdout:?}");
    assert!(stdout.contains("\n  \"arch\""), "2-space indent: {stdout}");

    // arch < os < runtime < version, all on their own line. Locks the
    // Rust shape; the only difference vs Go is `runtime` vs `go_version`.
    for key in ["arch", "os", "runtime", "version"] {
        assert!(
            stdout.contains(&format!("\"{key}\":")),
            "missing {key}: {stdout}"
        );
    }
    assert!(
        !stdout.contains("go_version"),
        "Rust port must not emit go_version: {stdout}"
    );

    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let obj = v.as_object().expect("object");
    let keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    assert_eq!(keys, vec!["arch", "os", "runtime", "version"]);
}
