# tukituki

A terminal UI for managing multiple dev processes. Define your processes in `.run/*.yaml`, then run `tukituki` to start them all and watch their logs in one place.

```
┌──────────────────────┬─────────────────────────────────────────────────┐
│ * frontend           │ frontend                                         │
│ * marketing-site     │ ────────────────────────────────────────────────│
│ ▶ * server           │  VITE v6.3.1  ready in 312 ms                  │
│ * worker             │                                                  │
│                      │  ➜  Local:   http://localhost:5276/             │
│                      │  ➜  Network: http://0.0.0.0:5276/              │
│──────────────────────│                                                  │
│ r restart  s stop    │                                                  │
│ S start    d dump    │                                                  │
│ c clear    q detach  │                                                  │
│ Q/^C stop all        │                                                  │
└──────────────────────┴─────────────────────────────────────────────────┘
```

## Installation

**Homebrew (macOS and Linux):**

```sh
brew tap dvaldivia/tukituki
brew install tukituki
```

**Go install:**

```sh
go install github.com/dvaldivia/tukituki/cmd/tukituki@latest
```

**Build from source:**

```sh
cd tukituki
go install ./cmd/tukituki/
```

## Usage

### Interactive TUI

```sh
tukituki
```

Opens the TUI, starts all processes, and lets you watch their logs interactively. **Requires a terminal (TTY).** If stdout is not a terminal (e.g. in a script or CI), tukituki exits with a clear error and suggests using a subcommand instead.

### Headless / scripted mode

All subcommands work without a TUI and are safe for use in scripts, CI pipelines, and AI agent workflows.

```sh
tukituki version              # Print version information
tukituki list                 # List all configured run targets
tukituki status               # Print status of all targets
tukituki status api           # Print status of a single target
tukituki start                # Start all processes in the background
tukituki start api            # Start a single target
tukituki stop                 # Stop all processes
tukituki stop api             # Stop a single target
tukituki restart api          # Restart a target
tukituki logs api             # Tail logs (last 100 lines, then follow)
tukituki logs api --no-follow # Print buffered logs and exit (safe for scripts)
tukituki logs api --tail 50   # Print last 50 lines, then follow
```

### Machine-readable output

Add `--json` to any subcommand to get structured JSON instead of formatted text. Errors are also written as JSON to stderr when `--json` is set.

```sh
tukituki version --json
tukituki list --json
tukituki status --json
tukituki status api --json
tukituki start api --json
tukituki stop api --json
tukituki restart api --json
```

**`version --json`**
```json
{
  "version": "1.2.0",
  "go_version": "go1.24.2",
  "os": "darwin",
  "arch": "arm64"
}
```

**`status --json`**
```json
[
  { "name": "api",    "status": "running", "description": "HTTP backend", "pid": 12345 },
  { "name": "worker", "status": "stopped", "description": "Background worker" }
]
```

**`list --json`**
```json
[
  {
    "name": "api",
    "command": "go",
    "args": ["run", "./cmd/server"],
    "description": "HTTP backend",
    "workdir": "backend"
  }
]
```

**`start` / `stop` / `restart --json`**
```json
{ "name": "api", "status": "running" }
```

**Error output (stderr, with `--json`)**
```json
{ "error": "target \"foo\" not found", "available": ["api", "worker"] }
```

## TUI keybindings

| Key | Action |
|-----|--------|
| `↑` / `↓` / `j` / `k` | Select process |
| `Tab` | Cycle to next process |
| `r` | Restart selected process |
| `s` | Stop selected process |
| `S` | Start selected process |
| `d` | Dump logs to a timestamped file |
| `c` | Clear log buffer (in-memory + on-disk) |
| `q` | Detach — quit TUI, leave processes running |
| `Q` / `Ctrl+C` | Stop all processes and exit |
| `PgUp` / `b` | Scroll log up |
| `PgDn` / `f` | Scroll log down |

## Defining processes

Create `.run/*.yaml` files in your project root. Each file defines one process:

```yaml
name: api
description: "HTTP/gRPC backend"
command: go
workdir: backend          # relative to project root
args:
  - run
  - ./cmd/server
env:
  HTTP_PORT: "8080"
  DB_HOST: localhost
cleanup:
  - "lsof -ti:8080 | xargs kill -9 2>/dev/null || true"
```

### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Unique identifier shown in the TUI |
| `command` | yes | Executable to run |
| `args` | no | List of arguments |
| `workdir` | no | Working directory, relative to project root |
| `env` | no | Extra environment variables (merged with the parent env) |
| `description` | no | Human-readable label shown in `list` and `status` |
| `cleanup` | no | Shell commands run after the process stops |

### How processes run

Each process is started via your login shell (`$SHELL -l -c "..."`) so tools managed by nvm, pyenv, rbenv, Homebrew, etc. are available exactly as they are in your terminal. Processes are **detached from the tukituki process group** — they keep running if you close the TUI with `q`.

### Cleanup commands

`cleanup` entries run via `$SHELL -l -c` after the process is stopped, in the target's `workdir` if set. Use them to release ports or kill stray children:

```yaml
cleanup:
  - "lsof -ti:8080 | xargs kill -9 2>/dev/null || true"
  - "pkill -f 'some-pattern' || true"
```

Failures are logged but do not abort remaining cleanup steps.

## State and logs

| Path | Purpose |
|------|---------|
| `.tukituki/state.json` | PID and status of each process (survives restarts) |
| `.tukituki/logs/<name>.log` | stdout+stderr for each process |

The log file is **truncated on each (re)start** so it always reflects the current run. The in-memory ring buffer holds the last 1000 lines.

Add `.tukituki/` to your `.gitignore`.

## Configuration

Flags and environment variables (via `TUKITUKI_` prefix) override defaults:

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--run-dir` | `TUKITUKI_RUN_DIR` | `.run` | Directory containing YAML definitions |
| `--state-dir` | `TUKITUKI_STATE_DIR` | `.tukituki` | Directory for state file and logs |
| `--config` | — | `.tukitukirc.yaml` | Config file path |
| `--json` | — | false | Emit JSON output (all subcommands) |

You can also place a `.tukitukirc.yaml` in the project root or `$HOME`:

```yaml
run_dir: config/processes
state_dir: .cache/tukituki
```

## Using tukituki from AI agents

tukituki's headless subcommands are designed to be safe and predictable for automated use:

- **Never blocks**: subcommands exit cleanly. Use `logs --no-follow` instead of `logs` (which follows forever).
- **Structured output**: `--json` on any subcommand gives parseable JSON; errors go to stderr as JSON too.
- **Self-describing**: `--help` on any command lists all flags and examples. `version --json` lets agents confirm tool identity and version at startup.
- **No interactive prompts**: all operations are fully flag-driven.
- **TTY guard**: running `tukituki` with no subcommand in a non-TTY context exits immediately with a clear error rather than hanging.

Recommended agent workflow:

```sh
tukituki version --json          # confirm tool is present and get version
tukituki list --json             # discover available targets
tukituki start --json            # start all targets
tukituki status --json           # poll for running/stopped/failed
tukituki logs api --no-follow    # read recent output without blocking
tukituki stop --json             # stop everything when done
```
