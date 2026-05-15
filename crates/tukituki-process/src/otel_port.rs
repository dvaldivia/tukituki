//! OTel collector port resolution & persistence.
//!
//! Direct port of the OTel-port logic in `manager.go`:
//!   - `<state-dir>/otel-port` holds the active port as decimal text.
//!   - `allocate_free_port()` asks the OS for an unused TCP port.
//!   - `port_bindable(port)` checks whether a port can be bound now,
//!     which is how we detect that a previously-saved port has been
//!     taken by something else while tukituki was down.

use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::Path;

/// Persist `port` to `<state_dir>/otel-port`. Errors are intentionally
/// swallowed — the live in-memory port is what matters; the file is best
/// effort. Matches the Go `_ = os.WriteFile(...)` pattern.
pub fn save(state_dir: &Path, port: u16) {
    let path = state_dir.join("otel-port");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, port.to_string());
}

/// Read the persisted port. Returns `0` when the file is missing or
/// unparseable, matching Go's behaviour (the caller treats `0` as "no
/// previously-known port").
pub fn load(state_dir: &Path) -> u16 {
    let path = state_dir.join("otel-port");
    let Ok(data) = fs::read_to_string(&path) else {
        return 0;
    };
    data.trim().parse().unwrap_or(0)
}

/// Remove the persisted port file. Used when stopping the collector so
/// the next `tukituki start` doesn't reuse a port that nothing of ours
/// holds anymore.
pub fn remove(state_dir: &Path) {
    let _ = fs::remove_file(state_dir.join("otel-port"));
}

/// Ask the OS for an available TCP port on loopback.
pub fn allocate_free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Can we bind port `port` on 127.0.0.1 right now?
pub fn port_bindable(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Convenience: write the port file via a buffered writer (kept here so
/// the manager only ever talks to the module API).
#[allow(dead_code)] // available for callers who want the io::Result back
pub fn save_strict(state_dir: &Path, port: u16) -> std::io::Result<()> {
    let path = state_dir.join("otel-port");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::File::create(&path)?;
    f.write_all(port.to_string().as_bytes())?;
    Ok(())
}
