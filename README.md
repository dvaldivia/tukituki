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

Pre-built binaries are published for linux × macOS × x86_64 + arm64.

**Homebrew (macOS and Linux):**

```sh
brew tap dvaldivia/tukituki
brew install tukituki
```

**Pre-built tarball:**

Download the right archive for your platform from the
[latest release](https://github.com/dvaldivia/tukituki/releases/latest):

```sh
# Pick your tarball — linux/x86_64, linux/arm64, darwin/x86_64, darwin/arm64
VERSION=1.0.0
OS=linux ARCH=x86_64
curl -sSL -o tukituki.tar.gz \
  "https://github.com/dvaldivia/tukituki/releases/download/v${VERSION}/tukituki_${OS}_${ARCH}.tar.gz"
tar -xzf tukituki.tar.gz
install -m 0755 tukituki /usr/local/bin/tukituki
ln -sf /usr/local/bin/tukituki /usr/local/bin/tktk
```

`checksums.txt` in each release covers all four tarballs.

**Cargo install (from source):**

```sh
cargo install --git https://github.com/dvaldivia/tukituki --tag v1.0.0 tukituki
```

**Build from source:**

```sh
git clone https://github.com/dvaldivia/tukituki
cd tukituki
cargo build --release -p tukituki
install -m 0755 target/release/tukituki ~/.local/bin/tukituki
```

> **Source builds vendor `protoc` automatically.** The bundled OpenTelemetry
> collector compiles its protobufs via `tonic-build` at build time. To avoid
> making users install a system protobuf-compiler package, the build script
> uses [`protobuf-src`](https://crates.io/crates/protobuf-src), which downloads
> and compiles the protobuf C++ source as a build dependency. A C++ toolchain
> + cmake (both already present on most dev systems — `build-essential` +
> `cmake` on Debian / Ubuntu, Xcode CLT on macOS) is the only prerequisite.
>
> First build takes an extra ~90 s to compile protoc; subsequent rebuilds are
> incremental. Set `TUKITUKI_USE_SYSTEM_PROTOC=1` if you already have `protoc`
> on `PATH` and want to skip the vendored build (CI does this).
>
> The Homebrew formula and pre-built tarball paths above don't compile
> anything — the binary they ship is already linked.

The Homebrew formula also installs a short alias, `tktk`, that points at the
same binary. For tarball / cargo / source builds you can create the alias
yourself:

```sh
ln -sf "$(command -v tukituki)" "$(dirname "$(command -v tukituki)")/tktk"
```

Anywhere you'd type `tukituki` you can type `tktk`. Help and usage output adapt
to whichever name you used.

> **Platforms.** Linux and macOS on x86_64 + arm64 are first-class. Windows is
> not supported — process detachment uses `setsid(2)` and the entire stop /
> cleanup path signals process groups via `kill(-pid, …)`, which has no native
> Windows equivalent. WSL works.

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
tukituki logs api             # Print last 100 buffered lines and exit
tukituki logs api --follow    # Print buffered lines, then follow new output
tukituki logs api --tail 50   # Print last 50 lines and exit
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
  "arch": "arm64",
  "os": "darwin",
  "runtime": "rustc 1.85.0",
  "version": "1.2.0"
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
| `↑` / `↓` / `j` / `k` | Select row |
| `Tab` | Cycle to next row |
| `→` / `l` | Expand selected folder |
| `←` / `h` | Collapse selected folder (or jump up to its header) |
| `Enter` / `Space` | Toggle selected folder |
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
| `otel` | no | Enable OpenTelemetry log collection for this target (`true`/`false`) |

### Grouping targets into folders

For projects with many run targets, move related YAML files into a subdirectory of `.run/` and tukituki will display them under a collapsible folder in the TUI:

```
.run/
├── api.yaml
├── worker.yaml
└── kb/
    ├── acme.yaml         # all kb-* targets appear under
    ├── meshlink.yaml     # a single "▶ kb (3)" folder row,
    └── sentinel.yaml     # collapsed by default
```

The arrow flips from `▶` to `▼` when the folder is expanded. Only one level of nesting is honoured; the headless CLI ignores grouping entirely and continues to operate on target names.

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

## OpenTelemetry error collection

tukituki includes a built-in OTLP log receiver that can collect OpenTelemetry log data from your services and surface errors (or any severity you choose) in a single view.

### Enabling OTel for a target

Add `otel: true` to any run target:

```yaml
name: api
command: go
args:
  - run
  - ./cmd/server
otel: true
```

When at least one target has `otel: true`, tukituki automatically:

1. Starts a bundled OTLP log receiver as a background process
2. Injects `OTEL_EXPORTER_OTLP_ENDPOINT` into the target's environment
3. Adds a virtual **otel-errors** entry at the bottom of the TUI sidebar

The collector filters incoming log records by severity and displays matching entries in the format:

```
[api] Connection refused to database at localhost:5432
[worker] Failed to process job batch-1234: timeout
```

### Severity filtering

By default only ERROR-level logs and above are shown. Change the threshold with:

```sh
tukituki --otel-severity warn      # also show warnings
tukituki --otel-severity info      # show info and above
```

Valid severity levels: `trace`, `debug`, `info`, `warn`, `error`, `fatal`.

### Protocol selection

The receiver uses gRPC (port 4317) by default. To use HTTP instead:

```sh
tukituki --otel-protocol http      # listens on port 4318
```

Override the port with `--otel-port`:

```sh
tukituki --otel-port 14317         # custom gRPC port
```

### Headless access

The OTel collector works with all headless subcommands:

```sh
tukituki logs otel-errors               # read collected errors
tukituki status otel-errors             # check collector status
```

### Detach behavior

The OTel collector process survives TUI detach (`q`) just like regular targets. When you reattach, tukituki reconnects to the running collector.

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
| `--otel-protocol` | `TUKITUKI_OTEL_PROTOCOL` | `grpc` | OTel receiver protocol (`grpc` or `http`) |
| `--otel-severity` | `TUKITUKI_OTEL_SEVERITY` | `error` | Minimum OTel log severity to display |
| `--otel-port` | `TUKITUKI_OTEL_PORT` | random | OTel receiver port (0 = random available port) |

You can also place a `.tukitukirc.yaml` in the project root or `$HOME`:

```yaml
run_dir: config/processes
state_dir: .cache/tukituki
```

## Using tukituki from AI agents

tukituki's headless subcommands are designed to be safe and predictable for automated use:

- **Never blocks**: subcommands exit cleanly. `logs` prints buffered lines and exits by default; pass `--follow`/`-f` only when you want to stream.
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
tukituki logs api                # read recent output without blocking
tukituki stop --json             # stop everything when done
```
