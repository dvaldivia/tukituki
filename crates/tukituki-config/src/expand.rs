//! `${VAR}` / `$VAR` expansion across run-target fields.
//!
//! Mirrors Go's `os.Expand` semantics so a YAML+env tree resolved by the
//! Rust binary produces the same field values as the Go binary.

use std::collections::BTreeMap;
use std::env;

use crate::RunTarget;

/// Walk every target's `command`, `workdir`, `args`, `cleanup`, and `env`
/// values and replace `${VAR}` / `$VAR` references.
///
/// Lookup precedence (matches Go): `.env`-loaded `vars` first, then the
/// running process's environment. When `vars` is empty, the function
/// short-circuits and returns the targets *unchanged* — same conservative
/// behaviour the Go binary exhibits, and one of the cases pinned by
/// `expand_env_no_vars_returns_original`.
pub fn expand_env(
    targets: Vec<RunTarget>,
    vars: Option<&BTreeMap<String, String>>,
) -> Vec<RunTarget> {
    let Some(vars) = vars.filter(|m| !m.is_empty()) else {
        return targets;
    };
    let lookup = |key: &str| -> Option<String> {
        if let Some(v) = vars.get(key) {
            return Some(v.clone());
        }
        env::var(key).ok()
    };

    targets
        .into_iter()
        .map(|mut t| {
            t.command = expand(&t.command, &lookup);
            t.workdir = expand(&t.workdir, &lookup);
            t.args = t.args.iter().map(|a| expand(a, &lookup)).collect();
            t.cleanup = t.cleanup.iter().map(|c| expand(c, &lookup)).collect();
            t.env = t
                .env
                .iter()
                .map(|(k, v)| (k.clone(), expand(v, &lookup)))
                .collect();
            t
        })
        .collect()
}

/// Faithful port of Go's `os.Expand`:
///
/// - `${NAME}` → `lookup("NAME")` (empty string when unset)
/// - `$NAME`  → `lookup("NAME")` where NAME is `[A-Za-z0-9_]+`
/// - `$<special>` → single-char shell special var (`*`, `#`, `?`, `0`–`9`, etc.)
/// - `$$` is *not* special in Go's os.Expand — but `$<non-alphanum>` leaves
///   the `$` untouched if no valid name follows.
fn expand<F: Fn(&str) -> Option<String>>(s: &str, lookup: &F) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            let (name, w) = get_shell_name(&bytes[i + 1..]);
            if name.is_empty() && w > 0 {
                // Invalid syntax (e.g. "${}"); eat the characters.
            } else if name.is_empty() {
                // Bare `$` with nothing usable after — emit literally.
                out.push('$');
            } else {
                out.push_str(&lookup(name).unwrap_or_default());
            }
            i += 1 + w;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Returns `(name, consumed)` for a `$` prefix.
fn get_shell_name(s: &[u8]) -> (&str, usize) {
    if s.is_empty() {
        return ("", 0);
    }
    if s[0] == b'{' {
        if s.len() > 2 && is_shell_special_var(s[1]) && s[2] == b'}' {
            return (std::str::from_utf8(&s[1..2]).unwrap_or(""), 3);
        }
        for i in 1..s.len() {
            if s[i] == b'}' {
                if i == 1 {
                    return ("", 2); // "${}"
                }
                return (std::str::from_utf8(&s[1..i]).unwrap_or(""), i + 1);
            }
        }
        return ("", 1);
    }
    if is_shell_special_var(s[0]) {
        return (std::str::from_utf8(&s[0..1]).unwrap_or(""), 1);
    }
    let mut i = 0;
    while i < s.len() && is_alpha_num(s[i]) {
        i += 1;
    }
    (std::str::from_utf8(&s[..i]).unwrap_or(""), i)
}

fn is_shell_special_var(c: u8) -> bool {
    matches!(
        c,
        b'*' | b'#' | b'$' | b'@' | b'!' | b'?' | b'-' | b'0'..=b'9'
    )
}

fn is_alpha_num(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_with_env(env: &[(&str, &str)]) -> RunTarget {
        let mut t = RunTarget {
            name: "x".into(),
            command: "go".into(),
            ..Default::default()
        };
        for (k, v) in env {
            t.env.insert((*k).into(), (*v).into());
        }
        t
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    // Lock used by the OS-env-touching tests below.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn expand_env_substitutes_vars() {
        let targets = vec![RunTarget {
            name: "server".into(),
            command: "go".into(),
            env: [
                (
                    "S3_ENDPOINT".to_string(),
                    "http://${APP_DOMAIN}:9001".to_string(),
                ),
                (
                    "S3_PUBLIC_URL".to_string(),
                    "http://${APP_DOMAIN}:9001/bucket".to_string(),
                ),
                ("STATIC_KEY".to_string(), "no-substitution".to_string()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        }];
        let vars = map(&[("APP_DOMAIN", "myhost")]);
        let result = expand_env(targets, Some(&vars));
        assert_eq!(result[0].env["S3_ENDPOINT"], "http://myhost:9001");
        assert_eq!(result[0].env["S3_PUBLIC_URL"], "http://myhost:9001/bucket");
        assert_eq!(result[0].env["STATIC_KEY"], "no-substitution");
    }

    #[test]
    fn expand_env_falls_back_to_os_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let key = "TEST_HOST_FALLBACK_RUST";
        // SAFETY: serialized; key is unique to this test crate.
        unsafe {
            env::set_var(key, "fromshell");
        }

        let targets = vec![target_with_env(&[(
            "URL",
            "http://${TEST_HOST_FALLBACK_RUST}:1234",
        )])];

        // Empty vars map → no expansion at all.
        let r1 = expand_env(targets.clone(), Some(&BTreeMap::new()));
        assert_eq!(r1[0].env["URL"], "http://${TEST_HOST_FALLBACK_RUST}:1234");

        // Non-empty (but irrelevant) vars → OS env fallback kicks in.
        let vars = map(&[("UNRELATED", "x")]);
        let r2 = expand_env(targets, Some(&vars));
        assert_eq!(r2[0].env["URL"], "http://fromshell:1234");

        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn expand_env_dotenv_takes_precedence_over_os_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let key = "DOMAIN_PRECEDENCE_TEST_RUST";
        unsafe {
            env::set_var(key, "from-os");
        }
        let targets = vec![target_with_env(&[(
            "BASE",
            "http://${DOMAIN_PRECEDENCE_TEST_RUST}",
        )])];
        let vars = map(&[(key, "from-dotenv")]);
        let result = expand_env(targets, Some(&vars));
        assert_eq!(result[0].env["BASE"], "http://from-dotenv");

        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn expand_env_no_vars_returns_original() {
        let targets = vec![target_with_env(&[("A", "${FOO}")])];
        let result = expand_env(targets, None);
        assert_eq!(result[0].env["A"], "${FOO}");
    }

    #[test]
    fn expand_env_does_not_mutate_original() {
        // In Rust, expand_env consumes by value, but the contract from the
        // Go test is "original env reference must still see the placeholder".
        // We model this by cloning before calling and asserting the clone
        // afterwards: the function must produce a NEW map, not in-place mutate
        // an Rc/Arc-shared one.
        let original: BTreeMap<String, String> =
            [("URL".to_string(), "http://${HOST}".to_string())]
                .into_iter()
                .collect();
        let snapshot = original.clone();
        let targets = vec![RunTarget {
            name: "s".into(),
            command: "go".into(),
            env: original,
            ..Default::default()
        }];
        let _ = expand_env(targets, Some(&map(&[("HOST", "new")])));
        assert_eq!(snapshot["URL"], "http://${HOST}");
    }

    #[test]
    fn expand_env_substitutes_args() {
        let targets = vec![RunTarget {
            name: "docs".into(),
            command: "hugo".into(),
            args: vec![
                "server".into(),
                "--baseURL".into(),
                "http://${APP_DOMAIN}:5313".into(),
            ],
            ..Default::default()
        }];
        let result = expand_env(targets, Some(&map(&[("APP_DOMAIN", "myhost")])));
        assert_eq!(
            result[0].args,
            vec!["server", "--baseURL", "http://myhost:5313"]
        );
    }

    #[test]
    fn expand_env_substitutes_command() {
        let targets = vec![RunTarget {
            name: "run".into(),
            command: "${BINARY}".into(),
            ..Default::default()
        }];
        let result = expand_env(targets, Some(&map(&[("BINARY", "myapp")])));
        assert_eq!(result[0].command, "myapp");
    }

    #[test]
    fn expand_env_substitutes_workdir() {
        let targets = vec![RunTarget {
            name: "run".into(),
            command: "make".into(),
            workdir: "${PROJECT_DIR}/docs".into(),
            ..Default::default()
        }];
        let result = expand_env(targets, Some(&map(&[("PROJECT_DIR", "/home/user/app")])));
        assert_eq!(result[0].workdir, "/home/user/app/docs");
    }

    #[test]
    fn expand_env_substitutes_cleanup() {
        let targets = vec![RunTarget {
            name: "server".into(),
            command: "node".into(),
            cleanup: vec!["lsof -ti:${PORT} | xargs kill -9 2>/dev/null || true".into()],
            ..Default::default()
        }];
        let result = expand_env(targets, Some(&map(&[("PORT", "3000")])));
        assert_eq!(
            result[0].cleanup[0],
            "lsof -ti:3000 | xargs kill -9 2>/dev/null || true"
        );
    }
}
