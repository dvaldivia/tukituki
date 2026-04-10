---
title: CLI Reference
weight: 4
---

tukituki exposes a small, focused command surface. The default invocation opens the interactive TUI; subcommands let you drive the same lifecycle from scripts, CI pipelines, or a plain terminal session.

## Global Flags

These flags are accepted by every command, including the root `tukituki` invocation.

| Flag | Default | Description |
|------|---------|-------------|
| `--config string` | `.tukitukirc.yaml` in cwd, then `$HOME` | Path to a configuration file |
| `--run-dir string` | `.run` | Directory containing YAML process definitions |
| `--state-dir string` | `.tukituki` | Directory for `state.json` and per-process log files |

---

## `tukituki`

```sh
tukituki [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Open the interactive TUI, attach to any already-running processes, and start anything that is not yet running.

**What it does, step by step:**

1. Loads every `.yaml` file found in `--run-dir`.
2. Calls `AttachToExisting()` — reads `<state-dir>/state.json`, re-tails log streams for processes that are still alive, and marks any dead PIDs as `stopped`.
3. Calls `StartAll()` — spawns every target that is not already in the `running` state.
4. Opens the TUI in the current terminal.
5. On exit with **`Q`** or **Ctrl+C**: sends SIGTERM (then SIGKILL after 5 s) to all managed processes before returning to the shell.

Pressing lowercase **`q`** closes the TUI while leaving processes running. Run `tukituki` again from the same directory to reattach.

**Example**

```sh
# Launch everything and open the TUI
tukituki

# Use a non-standard run directory
tukituki --run-dir services/run
```

---

## `tukituki start`

```sh
tukituki start [<name>] [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Start targets headlessly, without opening the TUI. Processes are spawned in the background and log output is written to `<state-dir>/logs/<name>.log`.

{{< callout type="info" >}}
`tukituki start` is idempotent. It first attaches to existing state just like the TUI does, so targets that are already running are left untouched. Only targets in a `stopped` or `failed` state are (re)started. You can call `tukituki start` as many times as you like without double-starting a process.
{{< /callout >}}

### Start all targets

```sh
tukituki start
```

Attaches to existing state, then spawns every target that is not already running. Returns immediately (exit 0) after all processes have been spawned. Exits non-zero if any target fails to start.

### Start a specific target

```sh
tukituki start <name>
```

Starts only the named target. `<name>` must match the `name` field in one of the YAML files under `--run-dir`.

**Examples**

```sh
# Start all targets in the background
tukituki start

# Start only the "api" target
tukituki start api

# Start against a custom state directory
tukituki start --state-dir /tmp/myproject-state
```

---

## `tukituki stop`

```sh
tukituki stop [<name>] [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Stop one or all running targets gracefully.

**What it does, step by step:**

1. Reads `<state-dir>/state.json` to find current process PIDs.
2. Sends **SIGTERM** to the target process(es).
3. Waits up to **5 seconds** for the process to exit.
4. If the process is still alive after 5 seconds, sends **SIGKILL**.
5. Runs any `cleanup` commands defined in the target's YAML definition.

### Stop all targets

```sh
tukituki stop
```

### Stop a specific target

```sh
tukituki stop <name>
```

**Examples**

```sh
# Gracefully stop everything
tukituki stop

# Stop only the "worker" target
tukituki stop worker
```

---

## `tukituki restart`

```sh
tukituki restart <name> [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Stop a specific target and then start it again. The log file for the target is truncated before the fresh start so logs from the previous run do not accumulate.

**What it does, step by step:**

1. Runs the same stop sequence as `tukituki stop <name>` (SIGTERM → wait → SIGKILL → cleanup).
2. Truncates `<state-dir>/logs/<name>.log`.
3. Spawns the target process again.

`<name>` is required; restarting all targets at once is not supported.

**Example**

```sh
# Restart the "frontend" target after a config change
tukituki restart frontend
```

---

## `tukituki status`

```sh
tukituki status [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Print a tabular summary of all targets and their current status, then exit.

**Output columns**

| Column | Description |
|--------|-------------|
| `NAME` | Target name as defined in the YAML file |
| `STATUS` | One of `running`, `stopped`, `failed`, or `unknown` |
| `DESCRIPTION` | Human-readable description from the YAML definition, if present |

Status meanings:

- **running** — process is alive and its PID is confirmed in the OS process table.
- **stopped** — process was started previously and has since exited cleanly.
- **failed** — process exited with a non-zero exit code.
- **unknown** — no state information is available (e.g. state file is missing or the entry was never started).

**Example**

```sh
tukituki status
# NAME        STATUS    DESCRIPTION
# api         running   HTTP API server
# worker      stopped   Background job processor
# frontend    running   React dev server
```

---

## `tukituki logs`

```sh
tukituki logs <name> [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Print the last 100 buffered lines from a target's log file, then follow new output until **Ctrl+C** — equivalent to `tail -n 100 -f`.

**Behavior notes:**

- Works for both running and stopped processes. For stopped processes the buffer is printed but no new lines arrive.
- Log files are stored at `<state-dir>/logs/<name>.log`.
- There is no `--lines` flag; the 100-line lookback is fixed.

**Example**

```sh
# Follow logs for the "api" target
tukituki logs api

# Follow logs using a custom state directory
tukituki logs api --state-dir /tmp/myproject-state
```
