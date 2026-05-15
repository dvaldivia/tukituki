use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn version_text_contains_expected_fields() {
    Command::cargo_bin("tukituki")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("tukituki "))
        .stdout(predicate::str::contains("rustc"));
}

#[test]
fn version_text_shape_matches_go_format() {
    // Expected format: `tukituki <ver> (<os>/<arch>, <runtime>)\n`
    let output = Command::cargo_bin("tukituki")
        .unwrap()
        .arg("version")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("tukituki "),
        "missing prefix: {stdout:?}"
    );
    assert!(
        stdout.ends_with(")\n"),
        "missing trailing ')\\n': {stdout:?}"
    );
    let open = stdout.find('(').expect("'(' not found");
    let inner = &stdout[open + 1..stdout.len() - 2]; // strip "(" and ")\n"
    let (osarch, runtime) = inner.split_once(", ").expect("missing ', '");
    assert!(
        osarch.contains('/'),
        "missing os/arch separator: {osarch:?}"
    );
    assert!(
        runtime.starts_with("rustc "),
        "runtime not rustc: {runtime:?}"
    );
}

#[test]
fn version_json_has_required_keys() {
    let output = Command::cargo_bin("tukituki")
        .unwrap()
        .args(["version", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "non-zero exit: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let obj = v.as_object().expect("top-level object");
    assert!(obj.contains_key("version"), "missing version key");
    assert!(obj.contains_key("os"), "missing os key");
    assert!(obj.contains_key("arch"), "missing arch key");
    assert!(obj.contains_key("runtime"), "missing runtime key");

    // Trailing newline like Go's fmt.Println.
    assert!(stdout.ends_with('\n'));
}

#[test]
fn version_json_keys_are_alphabetical() {
    // Go's encoding/json sorts map keys; serde_json BTreeMap preserves the
    // same property. Lock it in so future refactors don't accidentally
    // switch to a struct with field-declaration order.
    let output = Command::cargo_bin("tukituki")
        .unwrap()
        .args(["version", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let arch = stdout.find("\"arch\"").expect("arch key");
    let os = stdout.find("\"os\"").expect("os key");
    let runtime = stdout.find("\"runtime\"").expect("runtime key");
    let version = stdout.find("\"version\"").expect("version key");
    assert!(
        arch < os && os < runtime && runtime < version,
        "keys out of order: {stdout}"
    );
}

#[test]
fn version_json_uses_two_space_indent() {
    let output = Command::cargo_bin("tukituki")
        .unwrap()
        .args(["version", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Each entry should be indented with two spaces — matches Go's
    // json.MarshalIndent("", "  ").
    assert!(
        stdout.contains("\n  \"arch\""),
        "expected 2-space indent: {stdout}"
    );
}

#[test]
fn no_subcommand_in_non_tty_exits_with_error() {
    // Phase 1 stubs the TUI path; just confirm it exits non-zero with a
    // helpful message rather than panicking.
    Command::cargo_bin("tukituki")
        .unwrap()
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("not yet implemented")
                .or(predicate::str::contains("no terminal")),
        );
}
