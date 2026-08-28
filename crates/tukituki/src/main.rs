mod cli;
mod commands;
mod fdlimit;
mod platform;
mod rcfile;
mod runtime;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Before anything opens a file: the inherited soft limit is 256 on
    // macOS, which a watching TUI can exhaust on its own.
    fdlimit::raise_nofile();
    cli::run()
}
