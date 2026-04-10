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

```sh
go install github.com/dvaldivia/tukituki/cmd/tukituki@latest
```

Or build from source:

```sh
cd tukituki
go install ./cmd/tukituki/
```

## Usage

```sh
tukituki              # Open TUI, start all processes
tukituki start        # Start all processes headlessly (no TUI)
tukituki start api    # Start a single target
tukituki stop         # Stop all processes
tukituki stop api     # Stop a single target
tukituki restart api  # Restart a target
tukituki status       # Print status of all targets
tukituki logs api     # Tail logs for a target (last 100 lines, then follow)
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
| `description` | no | Human-readable label shown in `tukituki status` |
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

You can also place a `.tukitukirc.yaml` in the project root or `$HOME`:

```yaml
run_dir: config/processes
state_dir: .cache/tukituki
```
