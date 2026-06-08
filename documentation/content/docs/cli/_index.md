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

## `tukituki new`

```sh
tukituki new <name> '<command> [args...]' [-e KEY=VALUE]... [-w <dir>]
```

Create a new `.run/<name>.yaml` file from a name and command string. The command string is split on whitespace — the first token becomes `command` and the rest become `args`. The `.run/` directory is created automatically if it does not already exist.

**Flags**

| Flag | Description |
|------|-------------|
| `-e`, `--env KEY=VALUE` | Add an environment variable (repeatable) |
| `-w`, `--workdir <dir>` | Set the working directory, relative to the project root |

{{< callout type="warning" >}}
`tukituki new` refuses to overwrite an existing file. If `.run/<name>.yaml` already exists, the command exits with an error.
{{< /callout >}}

**Examples**

```sh
# Create a simple run target
tukituki new api 'go run ./cmd/api -port 8080'

# With environment variables
tukituki new worker 'node worker.js' -e PORT=3000 -e DEBUG=true

# With a working directory
tukituki new docs 'hugo server --buildDrafts' -w documentation

# Combine all options
tukituki new server 'go run ./cmd/server' -w backend -e HTTP_PORT=8182 -e GRPC_PORT=9192
```

The last example produces `.run/server.yaml`:

```yaml
name: server
command: go
workdir: backend
args:
  - run
  - ./cmd/server
env:
  HTTP_PORT: "8182"
  GRPC_PORT: "9192"
```

---

## `tukituki list`

```sh
tukituki list [--config <path>] [--run-dir <dir>] [--tags <tags>]
```

Print a tabular summary of all configured run targets and exit. Unlike `tukituki status`, this command reads only the YAML definitions in `--run-dir` — it does not inspect runtime state or require a state directory.

Use `--tags` to show only targets that have at least one of the specified tags (comma-separated). This is useful when you have many targets and only care about a logical group (e.g. `backend`, `frontend`).

**Output columns**

| Column | Description |
|--------|-------------|
| `NAME` | Target name as defined in the YAML file |
| `COMMAND` | Executable that will be run |
| `TAGS` | Comma-separated list of tags from the YAML definition (or `-` if none) |
| `DESCRIPTION` | Human-readable description from the YAML definition, if present |

**Example**

```sh
tukituki list
# NAME        COMMAND   TAGS       DESCRIPTION
# api         go        backend    HTTP API server
# frontend    npm       frontend   React dev server
# worker      go        backend    Background job processor
```

**Filter by tags**

```sh
tukituki list --tags=backend
# Only shows targets that have the "backend" tag

tukituki list --tags=backend,worker
# Shows targets that have "backend" OR "worker"
```

---

## `tukituki start`

```sh
tukituki start [<name>] [--tags <tags>] [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Start targets headlessly, without opening the TUI. Processes are spawned in the background and log output is written to `<state-dir>/logs/<name>.log`.

Use `--tags` (with no `<name>`) to start only the targets that have at least one of the specified tags. This is an explicit selection — targets will be started even if they have `autorun: false`.

{{< callout type="info" >}}
`tukituki start` is idempotent. It first attaches to existing state just like the TUI does, so targets that are already running are left untouched. Only targets in a `stopped` or `failed` state are (re)started. You can call `tukituki start` as many times as you like without double-starting a process.
{{< /callout >}}

### Start all targets

```sh
tukituki start
```

Attaches to existing state, then spawns every target that is not already running (respecting `autorun: false`). Returns immediately (exit 0) after all processes have been spawned. Exits non-zero if any target fails to start.

### Start a specific target

```sh
tukituki start <name>
```

Starts only the named target. `<name>` must match the `name` field in one of the YAML files under `--run-dir`.

### Start targets matching tags

```sh
tukituki start --tags=backend
```

Starts only the targets that carry at least one of the listed tags. Multiple tags are ORed:

```sh
tukituki start --tags=backend,worker
```

You cannot combine a target name with `--tags` on the same command.

**Examples**

```sh
# Start all targets in the background
tukituki start

# Start only the "api" target
tukituki start api

# Start only backend-tagged targets
tukituki start --tags=backend

# Start against a custom state directory
tukituki start --state-dir /tmp/myproject-state
```

---

## `tukituki stop`

```sh
tukituki stop [<name>] [--tags <tags>] [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Stop one or all running targets gracefully.

Use `--tags` (with no `<name>`) to stop only the targets that have at least one of the specified tags. The virtual `otel-errors` collector is only stopped during a full `tukituki stop` (no tags).

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

Stops every target (plus the `otel-errors` collector if it is running).

### Stop a specific target

```sh
tukituki stop <name>
```

### Stop targets matching tags

```sh
tukituki stop --tags=frontend
```

Stops only the targets matching the given tags. Multiple tags are ORed:

```sh
tukituki stop --tags=backend,worker
```

You cannot combine a target name with `--tags` on the same command.

**Examples**

```sh
# Gracefully stop everything
tukituki stop

# Stop only the "worker" target
tukituki stop worker

# Stop only frontend-tagged targets
tukituki stop --tags=frontend
```

---

## `tukituki restart`

```sh
tukituki restart [<name>...] [--tags <tags>] [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Stop target(s) and then start them again. The log file for each restarted target is truncated before the fresh start so logs from the previous run do not accumulate.

If no names are given, all targets are restarted. Use `--tags` to limit the operation to targets that have at least one of the specified tags.

**What it does, step by step:**

1. Runs the same stop sequence as `tukituki stop` (SIGTERM → wait → SIGKILL → cleanup).
2. Truncates `<state-dir>/logs/<name>.log` for each target.
3. Spawns the target process(es) again.

You cannot combine explicit target names with `--tags` on the same command.

**Examples**

```sh
# Restart the "frontend" target after a config change
tukituki restart frontend

# Restart everything
tukituki restart

# Restart only backend-tagged targets
tukituki restart --tags=backend

# Restart anything tagged backend or api
tukituki restart --tags=backend,api
```

---

## `tukituki status`

```sh
tukituki status [<name>] [--tags <tags>] [--config <path>] [--run-dir <dir>] [--state-dir <dir>]
```

Print a tabular summary of targets and their current status, then exit.

Use `--tags` (with no `<name>`) to show only targets that have at least one of the specified tags.

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

**Filter by tags**

```sh
tukituki status --tags=backend
# Shows status only for targets tagged "backend"

tukituki status --tags=backend,api
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
