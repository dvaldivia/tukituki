use std::collections::BTreeMap;
use std::process::ExitCode;

use crate::platform;

/// Print version information. Matches the Go binary's `version` subcommand:
///
/// - Text: `tukituki <version> (<os>/<arch>, <runtime>)`
/// - JSON: `{"arch": "...", "os": "...", "runtime": "...", "version": "..."}`
///   (keys sorted alphabetically, 2-space indented, trailing newline.)
///
/// The Go binary emits `"go_version"`; the Rust port emits `"runtime"` —
/// see plans/rust-port.md for the documented field-name drift.
pub fn run(json: bool) -> ExitCode {
    if json {
        let mut obj: BTreeMap<&str, &str> = BTreeMap::new();
        obj.insert("arch", platform::arch());
        obj.insert("os", platform::os());
        obj.insert("runtime", platform::RUNTIME);
        obj.insert("version", platform::VERSION);
        match serde_json::to_string_pretty(&obj) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("Error: marshal JSON: {e}");
                return ExitCode::from(1);
            }
        }
    } else {
        println!(
            "tukituki {} ({}/{}, {})",
            platform::VERSION,
            platform::os(),
            platform::arch(),
            platform::RUNTIME,
        );
    }
    ExitCode::SUCCESS
}
