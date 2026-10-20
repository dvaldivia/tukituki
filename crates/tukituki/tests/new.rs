use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn tukituki_in(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("tukituki").unwrap();
    c.current_dir(dir);
    c.env_remove("TUKITUKI_RUN_DIR")
        .env_remove("TUKITUKI_STATE_DIR");
    c
}

#[test]
fn new_writes_yaml_and_refuses_overwrite() {
    let dir = TempDir::new().unwrap();
    tukituki_in(dir.path())
        .args([
            "new",
            "site",
            "bun run dev --port 4321 --host 0.0.0.0",
            "-w",
            "site",
            "-e",
            "PUBLIC_APP_ORIGIN=http://127.0.0.1:5174",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Created"));

    let yaml = fs::read_to_string(dir.path().join(".run/site.yaml")).unwrap();
    assert!(yaml.contains("name: site"));
    assert!(yaml.contains("command: bun"));
    assert!(yaml.contains("workdir: site"));
    assert!(yaml.contains("  - run"));
    assert!(yaml.contains("  - --port"));
    assert!(yaml.contains("  - \"4321\"") || yaml.contains("  - 4321"));
    assert!(yaml.contains("PUBLIC_APP_ORIGIN: \"http://127.0.0.1:5174\""));

    tukituki_in(dir.path())
        .args(["new", "site", "echo hi"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));
}

#[test]
fn new_json_reports_file() {
    let dir = TempDir::new().unwrap();
    let out = tukituki_in(dir.path())
        .args(["--json", "new", "api", "go run ./cmd/api"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["name"], "api");
    assert!(v["file"].as_str().unwrap().ends_with("api.yaml"));
}
