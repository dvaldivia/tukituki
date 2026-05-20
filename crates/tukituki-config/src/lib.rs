//! YAML run-target loading + `.env` parsing + `${VAR}` expansion.
//!
//! Direct port of `internal/config` from the Go binary; field semantics
//! and parse behaviour are intentionally identical so the Rust binary can
//! load any `.run/*.yaml` tree the Go binary loads.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub mod dotenv;
pub mod expand;

pub use dotenv::{load_dotenv, parse_dotenv};
pub use expand::expand_env;

/// One managed process, as declared in a `.run/*.yaml` file.
///
/// Field set matches `config.RunTarget` in the Go binary. `BTreeMap` is
/// used for `env` to keep iteration order deterministic — useful for
/// `--json` output and for byte-stable test assertions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTarget {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub cleanup: Vec<String>,
    #[serde(default)]
    pub otel: bool,
    /// Default `true`. When `false`, the target is loaded and visible in
    /// the TUI but is excluded from bulk auto-start (`StartAll`, TUI
    /// startup, restart-all). Single-target start/restart/stop still
    /// work — useful for opt-in helpers you don't want running by
    /// default.
    #[serde(default = "default_autorun")]
    pub autorun: bool,

    // Runtime-only fields, never read from YAML.
    #[serde(skip)]
    pub group: String,
    #[serde(skip)]
    pub parse_error: String,
    #[serde(skip)]
    pub is_virtual: bool,
    #[serde(skip)]
    pub source_file: String,
}

fn default_autorun() -> bool {
    true
}

impl Default for RunTarget {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            workdir: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            description: String::new(),
            cleanup: Vec::new(),
            otel: false,
            autorun: true,
            group: String::new(),
            parse_error: String::new(),
            is_virtual: false,
            source_file: String::new(),
        }
    }
}

/// `LoadTargets` analogue: read every `.yaml`/`.yml` file under `run_dir`
/// and one level of immediate subdirectories.  Files at the top level
/// produce ungrouped targets; files under `<run_dir>/<group>/` get
/// `group = <group>`.  Dot-directories (e.g. `.git`) are skipped.
///
/// On per-file parse failure, an error-marked [`RunTarget`] is emitted
/// (named after the file basename) — matches the Go behaviour where the
/// TUI surfaces broken files instead of aborting the whole load.
///
/// Returns an `io::Error` if `run_dir` is missing or not a directory.
pub fn load_targets<P: AsRef<Path>>(run_dir: P) -> io::Result<Vec<RunTarget>> {
    let run_dir = run_dir.as_ref();
    let meta = match fs::metadata(run_dir) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("run directory does not exist: {}", run_dir.display()),
            ));
        }
        Err(e) => {
            return Err(io::Error::other(format!("stat run directory: {e}")));
        }
    };
    if !meta.is_dir() {
        return Err(io::Error::other(format!(
            "run directory path is not a directory: {}",
            run_dir.display()
        )));
    }

    let mut entries: Vec<(PathBuf, String)> = Vec::new();

    // Top-level files, sorted by name so per-file ordering is stable
    // before the final sort-by-target-name.
    for path in glob_yaml(run_dir)? {
        entries.push((path, String::new()));
    }

    // One level of immediate subdirectories. Each becomes a group.
    let mut dirs: Vec<PathBuf> = fs::read_dir(run_dir)?
        .filter_map(|de| de.ok())
        .filter(|de| de.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|de| de.path())
        .collect();
    dirs.sort();
    for sub in dirs {
        let group = sub
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if group.starts_with('.') {
            continue;
        }
        for path in glob_yaml(&sub)? {
            entries.push((path, group.clone()));
        }
    }

    let mut targets: Vec<RunTarget> = Vec::new();
    for (path, group) in entries {
        let abs = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let abs_str = abs.to_string_lossy().to_string();
        let base = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        match parse_file(&path) {
            Ok(mut t) => {
                t.group = group;
                t.source_file = abs_str;
                targets.push(t);
            }
            Err(e) => {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                targets.push(RunTarget {
                    name,
                    group,
                    parse_error: format!("{base}: {e}"),
                    source_file: abs_str,
                    ..Default::default()
                });
            }
        }
    }

    targets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(targets)
}

/// Reports whether any target in the list has `otel: true`.
pub fn has_otel_target(targets: &[RunTarget]) -> bool {
    targets.iter().any(|t| t.otel)
}

fn glob_yaml(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext == "yaml" || ext == "yml" {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn parse_file(path: &Path) -> Result<RunTarget, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("read file: {e}"))?;
    let t: RunTarget = serde_yaml_ng::from_str(&data).map_err(|e| format!("yaml decode: {e}"))?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_yaml(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).expect("write yaml");
    }

    #[test]
    fn load_targets_success() {
        let dir = tempdir();
        write_yaml(
            dir.path(),
            "web.yaml",
            r#"
name: web
command: go
args: ["run", "."]
workdir: ./backend
description: "Web server"
env:
  PORT: "8080"
"#,
        );
        write_yaml(
            dir.path(),
            "worker.yml",
            r#"
name: worker
command: ./worker
"#,
        );

        let targets = load_targets(dir.path()).expect("load");
        assert_eq!(targets.len(), 2, "expected 2 targets, got {targets:?}");
        assert_eq!(targets[0].name, "web", "sorted by name");
        assert_eq!(targets[1].name, "worker");
        assert_eq!(targets[0].command, "go");
        assert_eq!(targets[0].env.get("PORT").map(String::as_str), Some("8080"));
    }

    #[test]
    fn load_targets_empty_dir() {
        let dir = tempdir();
        let targets = load_targets(dir.path()).expect("load");
        assert!(targets.is_empty());
    }

    #[test]
    fn load_targets_invalid_yaml() {
        let dir = tempdir();
        write_yaml(
            dir.path(),
            "bad.yaml",
            "name: [this is: {not: valid yaml for a string\n",
        );
        let targets = load_targets(dir.path()).expect("load");
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].name, "bad",
            "name derived from filename on parse error"
        );
        assert!(
            !targets[0].parse_error.is_empty(),
            "expected parse_error to be set"
        );
    }

    #[test]
    fn load_targets_missing_dir() {
        let err = load_targets("/nonexistent/path/that/does/not/exist").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn load_targets_otel_field() {
        let dir = tempdir();
        write_yaml(
            dir.path(),
            "svc.yaml",
            "name: svc\ncommand: echo\notel: true\n",
        );
        let targets = load_targets(dir.path()).expect("load");
        assert_eq!(targets.len(), 1);
        assert!(targets[0].otel);
    }

    #[test]
    fn load_targets_otel_default_false() {
        let dir = tempdir();
        write_yaml(dir.path(), "plain.yaml", "name: plain\ncommand: echo\n");
        let targets = load_targets(dir.path()).expect("load");
        assert!(!targets[0].otel);
    }

    #[test]
    fn load_targets_subdirectories() {
        let dir = tempdir();
        write_yaml(dir.path(), "api.yaml", "name: api\ncommand: echo\n");

        let subdir = dir.path().join("kb");
        std::fs::create_dir_all(&subdir).unwrap();
        write_yaml(
            &subdir,
            "sentinel.yaml",
            "name: kb-sentinel\ncommand: echo\n",
        );
        write_yaml(&subdir, "acme.yaml", "name: kb-acme\ncommand: echo\n");

        let targets = load_targets(dir.path()).expect("load");
        assert_eq!(targets.len(), 3);

        let groups: BTreeMap<&str, &str> = targets
            .iter()
            .map(|t| (t.name.as_str(), t.group.as_str()))
            .collect();
        assert_eq!(groups["api"], "");
        assert_eq!(groups["kb-sentinel"], "kb");
        assert_eq!(groups["kb-acme"], "kb");
    }

    #[test]
    fn load_targets_ignores_dot_dirs() {
        let dir = tempdir();
        let hidden = dir.path().join(".git");
        std::fs::create_dir_all(&hidden).unwrap();
        write_yaml(
            &hidden,
            "config.yaml",
            "name: should-be-ignored\ncommand: echo\n",
        );
        write_yaml(dir.path(), "real.yaml", "name: real\ncommand: echo\n");

        let targets = load_targets(dir.path()).expect("load");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "real");
    }

    #[test]
    fn load_targets_autorun_default_true() {
        let dir = tempdir();
        write_yaml(dir.path(), "svc.yaml", "name: svc\ncommand: echo\n");
        let targets = load_targets(dir.path()).expect("load");
        assert!(
            targets[0].autorun,
            "autorun should default to true when omitted from YAML"
        );
    }

    #[test]
    fn load_targets_autorun_explicit_false() {
        let dir = tempdir();
        write_yaml(
            dir.path(),
            "svc.yaml",
            "name: svc\ncommand: echo\nautorun: false\n",
        );
        let targets = load_targets(dir.path()).expect("load");
        assert!(!targets[0].autorun);
    }

    #[test]
    fn has_otel_target_works() {
        let none = vec![
            RunTarget {
                name: "a".into(),
                ..Default::default()
            },
            RunTarget {
                name: "b".into(),
                ..Default::default()
            },
        ];
        assert!(!has_otel_target(&none));

        let some = vec![
            RunTarget {
                name: "a".into(),
                ..Default::default()
            },
            RunTarget {
                name: "b".into(),
                otel: true,
                ..Default::default()
            },
        ];
        assert!(has_otel_target(&some));
    }

    // Minimal in-tree tempdir helper so the config crate doesn't have to
    // pull in `tempfile` for tests alone — Phase 2 only needs one
    // dependency-light fixture. Workspace already depends on `tempfile`
    // for the state crate's atomic writes; we just don't wire it here.
    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        let mut p = std::env::temp_dir();
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("tukituki-config-test-{pid}-{nonce}"));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}
