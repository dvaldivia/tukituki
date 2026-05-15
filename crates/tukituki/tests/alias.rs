//! `tktk` alias: the binary inspects argv[0] and uses it as both the
//! clap `name` and `bin_name`, so help/usage output reflects whichever
//! invocation name the user typed. Mirrors the Go binary's
//! `rootCmd.Use = filepath.Base(os.Args[0])`.

use std::os::unix::fs::symlink;

use assert_cmd::Command;

#[test]
fn invoked_as_tktk_shows_tktk_in_usage() {
    let bin = assert_cmd::cargo::cargo_bin("tukituki");
    let dir = tempfile::tempdir().unwrap();
    let alias = dir.path().join("tktk");
    symlink(&bin, &alias).expect("symlink");

    let out = Command::new(&alias).arg("--help").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Usage: tktk"),
        "expected `Usage: tktk` in --help, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Usage: tukituki"),
        "tktk help must not say 'Usage: tukituki': \n{stdout}"
    );
}

#[test]
fn invoked_as_tktk_subcommand_help_adapts() {
    let bin = assert_cmd::cargo::cargo_bin("tukituki");
    let dir = tempfile::tempdir().unwrap();
    let alias = dir.path().join("tktk");
    symlink(&bin, &alias).expect("symlink");

    let out = Command::new(&alias)
        .args(["list", "--help"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Usage: tktk list"),
        "expected `Usage: tktk list` in `tktk list --help`, got:\n{stdout}"
    );
}

#[test]
fn invoked_as_tukituki_shows_tukituki_in_usage() {
    let out = Command::cargo_bin("tukituki")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Usage: tukituki"),
        "tukituki --help should say 'Usage: tukituki': \n{stdout}"
    );
}

#[test]
fn tktk_alias_runs_subcommands() {
    let bin = assert_cmd::cargo::cargo_bin("tukituki");
    let dir = tempfile::tempdir().unwrap();
    let alias = dir.path().join("tktk");
    symlink(&bin, &alias).expect("symlink");

    // Sanity: subcommands work under the alias too.
    let out = Command::new(&alias).arg("version").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.starts_with("tukituki "), "version line: {stdout:?}");
}
