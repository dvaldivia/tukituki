mod cli;
mod commands;
mod platform;
mod rcfile;
mod runtime;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::run()
}
