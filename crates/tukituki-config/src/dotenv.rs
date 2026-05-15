//! `.env` parsing — port of `internal/config/dotenv.go`.
//!
//! Behaviour matches the Go module exactly:
//! - blank lines and `#`-prefixed lines are ignored
//! - optional `export ` prefix is stripped
//! - values may be unquoted, double-quoted, or single-quoted
//! - unquoted values: inline `#` starts a comment; trailing whitespace trimmed
//! - double-quoted values: `\"` unescapes to `"`; no other escapes processed
//! - single-quoted values: no escape processing at all
//! - missing file returns `Ok(None)` (analogue of Go's `nil, nil`)

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::Path;

/// `LoadDotEnv` — read `.env` from `project_root` and merge unset keys
/// into the running process's environment. Returns the parsed map for
/// callers that want to feed it back into [`crate::expand_env`].
///
/// Shell exports always win over `.env`, matching godotenv.Load semantics.
pub fn load_dotenv<P: AsRef<Path>>(
    project_root: P,
) -> io::Result<Option<BTreeMap<String, String>>> {
    let path = project_root.as_ref().join(".env");
    let Some(vars) = parse_dotenv(&path)? else {
        return Ok(None);
    };
    for (k, v) in &vars {
        if env::var_os(k).is_some() {
            continue;
        }
        // SAFETY: setting env vars at startup, before threads. The
        // Go binary does this too — the process manager spawns
        // children with the full process env, so seeding it makes
        // .env keys visible to every target without per-yaml mapping.
        unsafe {
            env::set_var(k, v);
        }
    }
    Ok(Some(vars))
}

/// `ParseDotEnv` — read and parse a `.env` file.
///
/// Returns `Ok(None)` when the file does not exist (matches Go's
/// `nil, nil` early return). Returns `Ok(Some(empty))` for an existing
/// but empty file.
pub fn parse_dotenv<P: AsRef<Path>>(path: P) -> io::Result<Option<BTreeMap<String, String>>> {
    let data = match fs::read_to_string(path.as_ref()) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io::Error::other(format!("read .env: {e}"))),
    };

    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for raw in data.split('\n') {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();

        let Some(idx) = line.find('=') else {
            continue;
        };
        let key = line[..idx].trim();
        if key.is_empty() {
            continue;
        }
        let value = parse_value(&line[idx + 1..]);
        out.insert(key.to_string(), value);
    }
    Ok(Some(out))
}

/// Extract the value portion of a `KEY=VALUE` line.
fn parse_value(v: &str) -> String {
    let v = v.trim();
    if v.is_empty() {
        return String::new();
    }
    let bytes = v.as_bytes();
    match bytes[0] {
        b'"' => {
            // Double-quoted: scan for closing unescaped quote, with \" → ".
            let mut out = String::new();
            let mut i = 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                    out.push('"');
                    i += 2;
                } else if bytes[i] == b'"' {
                    break;
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            out
        }
        b'\'' => {
            // Single-quoted: no escape processing.
            // Find the next unescaped `'` after position 1, or take the rest.
            match v[1..].find('\'') {
                Some(end) => v[1..1 + end].to_string(),
                None => v[1..].to_string(),
            }
        }
        _ => {
            // Unquoted: strip inline `#` comment + trailing whitespace.
            let trimmed = match v.find('#') {
                Some(i) => &v[..i],
                None => v,
            };
            trimmed.trim().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_env(content: &str) -> (TempDir, PathBuf) {
        let dir = tempdir();
        let path = dir.path().join(".env");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn assert_env(env: &BTreeMap<String, String>, key: &str, want: &str) {
        let got = env
            .get(key)
            .unwrap_or_else(|| panic!("key {key:?} missing"));
        assert_eq!(got, want, "env[{key:?}]");
    }

    #[test]
    fn parse_dotenv_basic_key_value() {
        let (_d, path) = write_env(
            "
APP_DOMAIN=myhost
PORT=8080
",
        );
        let vars = parse_dotenv(&path).unwrap().unwrap();
        assert_env(&vars, "APP_DOMAIN", "myhost");
        assert_env(&vars, "PORT", "8080");
    }

    #[test]
    fn parse_dotenv_quoted() {
        let (_d, path) = write_env(
            r#"
DOUBLE="hello world"
SINGLE='foo bar'
ESCAPED="she said \"hi\""
"#,
        );
        let vars = parse_dotenv(&path).unwrap().unwrap();
        assert_env(&vars, "DOUBLE", "hello world");
        assert_env(&vars, "SINGLE", "foo bar");
        assert_env(&vars, "ESCAPED", r#"she said "hi""#);
    }

    #[test]
    fn parse_dotenv_comments() {
        let (_d, path) = write_env(
            "
# this is a comment
KEY=value # inline comment
OTHER=plain
",
        );
        let vars = parse_dotenv(&path).unwrap().unwrap();
        assert_env(&vars, "KEY", "value");
        assert_env(&vars, "OTHER", "plain");
        assert!(!vars.contains_key("# this is a comment"));
    }

    #[test]
    fn parse_dotenv_export_prefix() {
        let (_d, path) = write_env("export APP_DOMAIN=remotehost");
        let vars = parse_dotenv(&path).unwrap().unwrap();
        assert_env(&vars, "APP_DOMAIN", "remotehost");
    }

    #[test]
    fn parse_dotenv_missing_file() {
        let vars = parse_dotenv("/nonexistent/path/.env").unwrap();
        assert!(vars.is_none(), "missing file should yield None");
    }

    #[test]
    fn parse_dotenv_empty_file() {
        let (_d, path) = write_env("");
        let vars = parse_dotenv(&path).unwrap().unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn parse_dotenv_blank_lines() {
        let (_d, path) = write_env(
            "

KEY=value

OTHER=123

",
        );
        let vars = parse_dotenv(&path).unwrap().unwrap();
        assert_eq!(vars.len(), 2, "expected 2 keys, got {vars:?}");
    }

    // ---- LoadDotEnv ----------------------------------------------------

    // These tests mutate process env. They are explicitly serialized via a
    // module-level Mutex because cargo test runs them in parallel by default,
    // and concurrent env::set_var/env::var checks would race.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn load_dotenv_sets_unset_vars() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let key = "TUKITUKI_TEST_NEW_KEY";
        // SAFETY: serialized with ENV_LOCK; we own this key in tests.
        unsafe {
            env::remove_var(key);
        }
        let (_d, path) = write_env(&format!("{key}=fromdotenv\n"));
        let root = path.parent().unwrap().to_path_buf();

        let vars = load_dotenv(&root).unwrap().unwrap();
        assert_env(&vars, key, "fromdotenv");
        assert_eq!(env::var(key).unwrap(), "fromdotenv");

        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn load_dotenv_does_not_override_existing_vars() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let key = "TUKITUKI_TEST_EXISTING";
        // SAFETY: serialized with ENV_LOCK.
        unsafe {
            env::set_var(key, "fromshell");
        }
        let (_d, path) = write_env(&format!("{key}=fromdotenv\n"));
        let root = path.parent().unwrap().to_path_buf();

        let vars = load_dotenv(&root).unwrap().unwrap();
        // Returned map still reflects .env (used downstream for expansion).
        assert_env(&vars, key, "fromdotenv");
        // But the process env keeps the pre-existing value (shell wins).
        assert_eq!(env::var(key).unwrap(), "fromshell");

        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn load_dotenv_missing_file() {
        let dir = tempdir();
        let vars = load_dotenv(dir.path()).unwrap();
        assert!(vars.is_none());
    }

    // ---- tempdir helper (kept local; see lib.rs for rationale) --------

    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &std::path::Path {
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
        p.push(format!("tukituki-config-dotenv-test-{pid}-{nonce}"));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}
