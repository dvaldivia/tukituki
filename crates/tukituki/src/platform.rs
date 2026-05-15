//! Platform identifiers normalised to match the Go binary's output.
//!
//! Go's `runtime.GOOS` / `runtime.GOARCH` values (`darwin`, `linux`,
//! `amd64`, `arm64`) differ from Rust's `std::env::consts::{OS, ARCH}`
//! (`macos`, `linux`, `x86_64`, `aarch64`). For drop-in compatibility we
//! emit the Go names.

pub fn os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// `VERSION` resolves at build time. `TUKITUKI_VERSION` lets the release
/// pipeline inject the tag without editing Cargo.toml; otherwise we fall
/// back to the crate version baked into the binary.
pub const VERSION: &str = match option_env!("TUKITUKI_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Runtime identifier — analogous to Go's `runtime.Version()` (e.g.
/// `"go1.24.2"`). Captured by `build.rs` from `rustc --version`.
pub const RUNTIME: &str = env!("TUKITUKI_RUSTC_VERSION");
