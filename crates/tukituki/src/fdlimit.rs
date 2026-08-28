//! Descriptor headroom for the process.
//!
//! macOS ships a soft `RLIMIT_NOFILE` of 256 with an unlimited hard cap
//! (`launchctl limit maxfiles` → `256 unlimited`). A session holds
//! descriptors for the terminal, each target's log file and tailer, the
//! OTel collector's sockets, and — on the kqueue backend — one per
//! watched path. 256 is not much room, and running out is silent and
//! confusing rather than loud: `Command::spawn` starts returning
//! `EMFILE`, so targets stop restarting and the only clue is a
//! transient flash in the footer.
//!
//! Raising the soft limit toward the hard one costs nothing and is
//! inherited by every child we spawn.

use nix::sys::resource::{Resource, getrlimit, setrlimit};

/// Ceiling for the bump. High enough that no realistic project runs
/// out, low enough to stay well under `kern.maxfilesperproc` on macOS
/// and `OPEN_MAX` on the platforms that still enforce one.
const DESIRED_NOFILE: u64 = 8192;

/// Raise the soft `RLIMIT_NOFILE` toward [`DESIRED_NOFILE`], never
/// above the hard limit and never downward.
///
/// Best-effort throughout: a platform that refuses either call is no
/// reason not to start, it just leaves us on the inherited limit.
pub fn raise_nofile() {
    let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) else {
        return;
    };

    // `RLIM_INFINITY` is `!0` on every target we build for, so an
    // unlimited hard cap simply means "take the ceiling".
    let target = if hard == u64::MAX {
        DESIRED_NOFILE
    } else {
        hard.min(DESIRED_NOFILE)
    };

    if target > soft {
        let _ = setrlimit(Resource::RLIMIT_NOFILE, target, hard);
    }
}
