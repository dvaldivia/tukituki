//! `.tukitukirc.yaml` loader — middle layer of the config precedence
//! chain.
//!
//! Search order (mirrors Go's viper setup):
//!   1. `--config <path>` (if set)
//!   2. `.tukitukirc.yaml` in the cwd
//!   3. `$HOME/.tukitukirc.yaml`
//!
//! Any unset key falls through to the next layer (or to the default).
//! Final precedence: CLI flag > env var > rc file > built-in default.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct RcFile {
    pub run_dir: Option<String>,
    pub state_dir: Option<String>,
    pub otel_protocol: Option<String>,
    pub otel_severity: Option<String>,
    pub otel_port: Option<u16>,
}

/// Process-wide cache. The rc file is small and immutable for a
/// single invocation, so reading it once is plenty.
static CACHE: OnceLock<RcFile> = OnceLock::new();

/// Resolve + parse the rc file, caching the result across calls.
/// `explicit` is the value passed to `--config`, when set.
pub fn load(explicit: Option<&str>) -> &'static RcFile {
    CACHE.get_or_init(|| read(explicit).unwrap_or_default())
}

/// Locate + parse the rc file fresh (no cache). Used by tests that
/// need to bypass the OnceLock.
pub fn read(explicit: Option<&str>) -> Option<RcFile> {
    let path = resolve_path(explicit)?;
    let data = std::fs::read_to_string(&path).ok()?;
    serde_yaml_ng::from_str::<RcFile>(&data).ok()
}

fn resolve_path(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = explicit
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    let cwd = std::env::current_dir().ok()?;
    let cwd_rc = cwd.join(".tukitukirc.yaml");
    if cwd_rc.is_file() {
        return Some(cwd_rc);
    }
    let home = std::env::var_os("HOME")?;
    let home_rc = PathBuf::from(home).join(".tukitukirc.yaml");
    if home_rc.is_file() {
        return Some(home_rc);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_explicit_path_returns_parsed_rc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rc.yaml");
        std::fs::write(
            &path,
            "run_dir: config/processes\nstate_dir: .cache/tt\notel_port: 4321\n",
        )
        .unwrap();
        let rc = read(Some(path.to_str().unwrap())).expect("parsed");
        assert_eq!(rc.run_dir.as_deref(), Some("config/processes"));
        assert_eq!(rc.state_dir.as_deref(), Some(".cache/tt"));
        assert_eq!(rc.otel_port, Some(4321));
    }

    #[test]
    fn read_missing_file_returns_none() {
        assert!(read(Some("/nonexistent/path/no-rc.yaml")).is_none());
    }

    #[test]
    fn read_empty_yaml_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rc.yaml");
        std::fs::write(&path, "").unwrap();
        let rc = read(Some(path.to_str().unwrap())).expect("parsed");
        assert!(rc.run_dir.is_none());
        assert!(rc.state_dir.is_none());
        assert!(rc.otel_port.is_none());
    }
}
