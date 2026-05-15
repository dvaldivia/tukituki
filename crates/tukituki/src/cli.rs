use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::commands;

#[derive(Debug, Parser)]
#[command(
    name = "tukituki",
    about = "Manage multiple dev processes from a TUI or headless CLI",
    long_about = LONG_ABOUT,
    disable_help_subcommand = true,
    arg_required_else_help = false,
)]
pub struct Cli {
    /// config file (default: .tukitukirc.yaml in cwd, then $HOME/.tukitukirc.yaml)
    #[arg(long, global = true)]
    pub config: Option<String>,

    /// directory containing YAML run definitions (default: .run)
    #[arg(long = "run-dir", global = true, env = "TUKITUKI_RUN_DIR")]
    pub run_dir: Option<String>,

    /// directory for state file and logs (default: .tukituki)
    #[arg(long = "state-dir", global = true, env = "TUKITUKI_STATE_DIR")]
    pub state_dir: Option<String>,

    /// emit machine-readable JSON instead of formatted text
    #[arg(long, global = true)]
    pub json: bool,

    /// OTel receiver protocol: grpc or http (default: grpc)
    #[arg(long = "otel-protocol", global = true, env = "TUKITUKI_OTEL_PROTOCOL")]
    pub otel_protocol: Option<String>,

    /// minimum OTel log severity to display (default: error)
    #[arg(long = "otel-severity", global = true, env = "TUKITUKI_OTEL_SEVERITY")]
    pub otel_severity: Option<String>,

    /// OTel receiver port; 0 = random available port
    #[arg(long = "otel-port", global = true, env = "TUKITUKI_OTEL_PORT")]
    pub otel_port: Option<u16>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print version information
    #[command(after_help = "Examples:\n  tukituki version\n  tukituki version --json")]
    Version,

    /// List all configured run targets
    #[command(
        long_about = "List all run targets defined in .run/*.yaml.\n\nOutputs name, command, and description for each target.\nUse --json for machine-readable output.",
        after_help = "Examples:\n  tukituki list\n  tukituki list --json"
    )]
    List,

    /// Print the status of all targets (or a single target)
    #[command(
        long_about = "Print the runtime status of managed processes.\n\nWith no argument all targets are shown. Pass a target name to query one.\nStatus values: running, stopped, failed, unknown.\nUse --json for machine-readable output.",
        after_help = "Examples:\n  tukituki status\n  tukituki status api\n  tukituki status --json"
    )]
    Status {
        /// Specific target to query (omit to list all)
        target: Option<String>,
    },

    /// Start one or all targets (headless, no TUI)
    #[command(
        long_about = "Start one or all targets as background processes (no TUI).\n\nIf target-name is omitted, all configured targets are started.\nProcesses that are already running are left untouched.\nUse --json for machine-readable output.",
        after_help = "Examples:\n  tukituki start\n  tukituki start api"
    )]
    Start {
        /// Specific target to start (omit to start all)
        target: Option<String>,
    },

    /// Stop one or all targets
    #[command(
        long_about = "Stop one or all running targets.\n\nSends SIGTERM, waits up to 5 seconds, then SIGKILLs if still running.\nIf target-name is omitted, all targets are stopped.\nUse --json for machine-readable output.",
        after_help = "Examples:\n  tukituki stop\n  tukituki stop api"
    )]
    Stop {
        /// Specific target to stop (omit to stop all)
        target: Option<String>,
    },

    /// Restart one or all targets
    #[command(
        long_about = "Stop and then start each named target, in order.\n\nIf no target names are given, all configured targets are restarted.\nIf a process is not currently running, it is simply started.\nAll names are validated up front; if any is unknown the command exits\nbefore restarting anything. Use --json for machine-readable output.",
        after_help = "Examples:\n  tukituki restart\n  tukituki restart api worker"
    )]
    Restart {
        /// Targets to restart (omit to restart all)
        targets: Vec<String>,
    },

    /// Print recent logs for a target
    #[command(
        long_about = "Print recent log lines for a target and optionally follow new output.\n\nBy default prints the last 100 buffered lines and exits (safe for scripts and\nAI agents). Use --follow/-f to stream new lines until Ctrl+C. Use --tail to\ncontrol how many buffered lines are shown.",
        after_help = "Examples:\n  tukituki logs api\n  tukituki logs api --follow\n  tukituki logs api --tail 50 -f"
    )]
    Logs {
        /// Target name
        target: String,
        /// Stream new log lines until Ctrl+C instead of exiting after the buffered lines
        #[arg(short = 'f', long)]
        follow: bool,
        /// Number of buffered log lines to print (0 = all)
        #[arg(long, default_value_t = 100)]
        tail: usize,
    },

    /// Run the bundled OTLP log receiver (internal use)
    #[command(
        hide = true,
        long_about = "Run the bundled OTLP log receiver. Spawned automatically by the Manager when a target has `otel: true`. Not intended for direct invocation."
    )]
    OtelCollector {
        /// Receiver protocol: `grpc` or `http`
        #[arg(long, default_value = "grpc")]
        protocol: String,
        /// Minimum log severity to emit
        #[arg(long, default_value = "error")]
        severity: String,
        /// TCP port to listen on
        #[arg(long, default_value_t = 4317)]
        port: u16,
        /// Unix-socket path to publish error notifications on (empty = disabled)
        #[arg(long, default_value = "")]
        notify_socket: String,
    },
}

const LONG_ABOUT: &str = "\
tukituki reads process definitions from .run/*.yaml and lets you
start, stop, restart, and tail their logs.

INTERACTIVE MODE (default, requires a terminal):
  Run with no arguments to open the interactive TUI. All processes are started
  automatically; the TUI lets you watch logs, restart, and stop them.

HEADLESS / SCRIPTED MODE (safe for automation and AI agents):
  Use subcommands for non-interactive control:
    tukituki list              - list configured targets
    tukituki status            - show runtime status of all targets
    tukituki start [name]      - start one or all targets
    tukituki stop  [name]      - stop  one or all targets
    tukituki restart [name]    - restart one or all targets
    tukituki logs <name>       - print recent logs (use --follow/-f to stream)

  Add --json to any subcommand for machine-readable JSON output.

CONFIGURATION:
  Process definitions live in .run/*.yaml (configurable via --run-dir or
  TUKITUKI_RUN_DIR). Runtime state is stored in .tukituki/ (configurable via
  --state-dir or TUKITUKI_STATE_DIR).";

/// Inspect argv[0] so help text reflects whichever name the user
/// invoked. Mirrors Go's `rootCmd.Use = filepath.Base(os.Args[0])`
/// trick: when the Homebrew formula installs a `tktk` symlink, every
/// help/example line reads `tktk` instead of `tukituki`.
fn invocation_name() -> String {
    let argv0 = std::env::args().next().unwrap_or_default();
    std::path::Path::new(&argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "/")
        .unwrap_or("tukituki")
        .to_string()
}

pub fn run() -> ExitCode {
    // clap 4.6's `Str` accepts `&'static str` only, so leak the name —
    // it lives for the program's lifetime anyway and the allocation
    // happens once per invocation.
    let name: &'static str = Box::leak(invocation_name().into_boxed_str());
    let cmd = Cli::command().name(name).bin_name(name);
    let matches = cmd.get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => {
            // Mirrors clap's normal failure rendering when `parse()`
            // would have errored.
            e.exit();
        }
    };

    match cli.command {
        Some(Command::Version) => commands::version::run(cli.json),
        Some(Command::List) => commands::list::run(&cli),
        Some(Command::Status { ref target }) => commands::status::run(&cli, target.as_deref()),
        Some(Command::Start { ref target }) => commands::start::run(&cli, target.as_deref()),
        Some(Command::Stop { ref target }) => commands::stop::run(&cli, target.as_deref()),
        Some(Command::Restart { ref targets }) => commands::restart::run(&cli, targets),
        Some(Command::Logs {
            ref target,
            follow,
            tail,
        }) => commands::logs::run(&cli, target, follow, tail),
        Some(Command::OtelCollector {
            ref protocol,
            ref severity,
            port,
            ref notify_socket,
        }) => commands::otel_collector::run(protocol, severity, port, notify_socket),
        None => commands::tui::run(&cli),
    }
}
