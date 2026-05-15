//! Shell command construction + escaping.
//!
//! Direct port of `BuildShellCmd` / `shellEscape` in
//! `internal/process/manager.go`. Output strings must match Go byte-for-byte
//! — they end up as the `-c` argument to `$SHELL -l -c`, and any drift
//! changes how user commands are interpreted.

/// Build a shell command string from a `command` and its `args`, escaping
/// each token so it survives `/bin/sh -c`.
pub fn build_shell_cmd(command: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(1 + args.len());
    parts.push(shell_escape(command));
    for a in args {
        parts.push(shell_escape(a));
    }
    parts.join(" ")
}

/// Wrap `s` in single quotes if it contains any character the shell would
/// otherwise interpret. Internal `'` characters are escaped using the
/// `'\\''` trick (close quote, escaped quote, reopen quote).
pub fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@' | '=' | ',')
    });
    if safe {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_shell_cmd_cases() {
        // Mirrors TestBuildShellCmd in manager_test.go.
        let cases = [
            ("simple", "echo", &["hello"][..], "echo hello"),
            ("empty arg", "cmd", &["--flag", ""][..], "cmd --flag ''"),
            (
                "spaces in arg",
                "cmd",
                &["hello world"][..],
                "cmd 'hello world'",
            ),
            ("no args", "cmd", &[][..], "cmd"),
            ("multiple empty args", "cmd", &["", ""][..], "cmd '' ''"),
            (
                "flag with empty value",
                "reverse-proxy",
                &["-tls-certificate", "", "-tls-key", ""][..],
                "reverse-proxy -tls-certificate '' -tls-key ''",
            ),
        ];
        for (label, command, args, want) in cases {
            let args_owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
            let got = build_shell_cmd(command, &args_owned);
            assert_eq!(got, want, "{label}");
        }
    }

    #[test]
    fn shell_escape_special_chars() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
        assert_eq!(shell_escape("a'b"), "'a'\\''b'");
        assert_eq!(shell_escape(""), "''");
        assert_eq!(
            shell_escape("safe-path/foo.bar:1234@host"),
            "safe-path/foo.bar:1234@host"
        );
    }
}
