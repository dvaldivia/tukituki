---
title: State & Log Persistence
weight: 2
---

tukituki is designed to survive restarts. When you close the TUI (press `q`) or the terminal session ends, your managed processes keep running. When you open tukituki again, it re-discovers those processes and resumes tailing their logs. This page explains the mechanics: what gets stored, where files live, and what happens at reattach time.

## Directory Layout

Everything tukituki writes at runtime lives under `.tukituki/` in your project root:

```
.tukituki/
├── state.json        # Process registry — one entry per managed process
└── logs/
    ├── api.log       # Log output for the process named "api"
    ├── worker.log    # Log output for the process named "worker"
    └── frontend.log  # Log output for the process named "frontend"
```

{{< callout >}}
Add `.tukituki/` to your `.gitignore`. State and logs are runtime artifacts, not source files — committing them causes spurious diffs and exposes local process IDs and file paths.

```sh
echo '.tukituki/' >> .gitignore
```
{{< /callout >}}

## state.json

`state.json` is the process registry. It is written every time a process starts, stops, or changes status. The file stores one entry per managed process with the following fields:

| Field | Type | Description |
|---|---|---|
| `pid` | integer | Operating system PID of the running process. |
| `log_path` | string | Absolute path to the log file for this process. |
| `start_time` | RFC3339 timestamp | When the process was last started. |
| `status` | string | `running`, `stopped`, or `failed`. |
| `exit_code` | integer | Exit code from the last run. `0` for processes still running. |

Example entry:

```json
{
  "api": {
    "pid": 48271,
    "log_path": "/home/user/myproject/.tukituki/logs/api.log",
    "start_time": "2025-11-04T09:14:33Z",
    "status": "running",
    "exit_code": 0
  }
}
```

`state.json` is the source of truth that `AttachToExisting` reads on startup. It is always written atomically so a crash mid-write cannot corrupt it.

## Log File Lifecycle

Each process gets its own log file at `.tukituki/logs/<name>.log`.

- **On start**: the log file is **truncated** (emptied). Each run begins with a clean file.
- **During run**: stdout and stderr from the process are appended to the file in real time.
- **After stop**: the file remains on disk and can be read with any tool (`less`, `cat`, `tail -f`, etc.).

Because the log file is truncated on each start, it always contains output from the most recent run only. If you need to retain logs across runs, copy or rotate the file before restarting the process.

## In-Memory Ring Buffer

In addition to the log file, tukituki maintains an **in-memory ring buffer** of the last **1000 lines** per process. The ring buffer is what powers the TUI log pane — scrolling up in the log view reads from this buffer, not from the file.

Key behaviors:

- The ring buffer is populated by tailing the log file, not by reading it upfront. This means the buffer contains the most recent 1000 lines seen since tukituki attached to the process.
- The buffer is **cleared when tukituki restarts** (it is in-memory, not persisted). After a restart, the buffer is refilled by re-tailing the log file from the current offset.
- Pressing **`c`** (clear) in the TUI log pane clears the visible ring buffer for the selected process. This only affects what is displayed in the TUI — it does not truncate or modify the log file on disk.

If you need to see output older than the last 1000 lines, open the log file directly:

```sh
less .tukituki/logs/api.log
```

## Reattach Flow (AttachToExisting)

When you run `tukituki` in a project where processes are already running, the startup routine calls `AttachToExisting()` instead of starting everything from scratch. Here is what happens:

1. **Read state.json** — load the persisted process registry.
2. **For each process marked `running`**: send signal 0 to the stored PID. Signal 0 does not kill the process — it is a probe that succeeds if the PID exists and tukituki has permission to signal it.
   - **PID is alive**: resume log tailing from the current file offset. The TUI shows the process as `running` and begins streaming new output immediately.
   - **PID is dead** (process exited while tukituki was closed): mark the process as `stopped` in state.json and update the TUI status accordingly. The log file still contains output from the last run.
3. **For each process marked `stopped` or `failed`**: display the persisted status without probing — no signal is sent.
4. Open the TUI. All previously-running processes appear with live log streams; stopped processes appear with their final status.

This flow means you can close the TUI, go to lunch, come back, and run `tukituki` again — any processes that stayed up will have their logs and status seamlessly restored without any manual intervention.

## Relationship Between `c` (Clear) and the Log File

The **`c`** key in the TUI log pane clears the in-memory ring buffer for the selected process. After pressing `c`:

- The log pane becomes empty.
- New lines written by the process continue to appear as they arrive.
- The log file on disk is **unchanged** — all prior output is still there.

Think of `c` as a "scroll to bottom and hide history" action rather than a delete action. To permanently discard the log file contents, restart the process (which truncates the file) or delete the file manually while the process is stopped.
