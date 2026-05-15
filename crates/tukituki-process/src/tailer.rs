//! Log-file tailer.
//!
//! Port of `startLogTailer` in `internal/process/manager.go`. Polls the
//! file every `POLL_INTERVAL`; on growth, reads the new bytes, strips
//! null bytes, splits into lines (dropping a trailing empty), and hands
//! each line to the supplied `on_line` callback. The callback is
//! responsible for appending to the ring buffer and broadcasting to
//! subscribers; that policy lives in the manager.
//!
//! The tailer thread exits when `cancel_rx` is signalled OR
//! disconnected (i.e. the Manager dropped the sender).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

pub const POLL_INTERVAL: Duration = Duration::from_millis(100);
pub const RING_BUFFER_SIZE: usize = 1000;

/// Body of the tailer thread. Re-opens the file on every poll cycle —
/// the Go tailer does the same so the file can be rotated underneath
/// it (e.g. on restart's truncate-on-open).
pub fn run<F>(log_path: PathBuf, cancel_rx: Receiver<()>, mut on_line: F)
where
    F: FnMut(String),
{
    let mut offset: u64 = 0;
    loop {
        // Cancellable sleep. Disconnect = Manager dropped → exit.
        match cancel_rx.recv_timeout(POLL_INTERVAL) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let Ok(mut f) = File::open(&log_path) else {
            continue;
        };
        let Ok(meta) = f.metadata() else { continue };
        let size = meta.len();

        // Truncate detection: if the file is smaller than where we
        // think we are, the file was rotated/cleared. Reset to 0.
        if size < offset {
            offset = 0;
        }
        if size <= offset {
            continue;
        }

        if f.seek(SeekFrom::Start(offset)).is_err() {
            continue;
        }
        let mut buf = vec![0u8; (size - offset) as usize];
        let n = match f.read(&mut buf) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if n == 0 {
            continue;
        }
        offset += n as u64;

        // Strip null bytes — children can emit NULs (e.g. partial UTF-8
        // recovery) and the TUI's renderer barfs on them.
        let chunk_owned: Vec<u8> = buf[..n].iter().copied().filter(|b| *b != 0).collect();
        if chunk_owned.is_empty() {
            continue;
        }
        let chunk = String::from_utf8_lossy(&chunk_owned).into_owned();

        let mut lines: Vec<&str> = chunk.split('\n').collect();
        if let Some(last) = lines.last()
            && last.is_empty()
        {
            lines.pop();
        }
        for line in lines {
            on_line(line.to_string());
        }
    }
}
