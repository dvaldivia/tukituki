//! Process lifecycle management — Pass A of the `internal/process` port.
//!
//! Spawns detached children (own session via `setsid`), tracks them via
//! [`tukituki_state::State`], signals their process group on stop, runs
//! per-target cleanup commands, and persists the OpenTelemetry collector
//! port across invocations.
//!
//! Log streaming (`startLogTailer`, `watch`, ring buffer) and the
//! collector spawn-as-self path (`EnsureOtelCollector`, `virtualOtelTarget`,
//! `Describe`) are explicitly deferred — see `plans/rust-port.md` Phase 4
//! and Phase 5 respectively.

mod manager;
mod otel_port;
mod shell;
mod tailer;

pub use manager::{Manager, OtelConfig};
pub use shell::{build_shell_cmd, shell_escape};

/// The fixed name used for the virtual OTel collector target. Mirrors the
/// Go constant of the same name so state files round-trip cleanly.
pub const OTEL_TARGET_NAME: &str = "otel-errors";
