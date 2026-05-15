# Plan: Port tukituki from Go to Rust (ratatui)

## Goal

Replace the Go implementation of tukituki with a Rust binary that is feature-equivalent and CLI/file-format-compatible. The `examples/` tree (go-api, go-worker, python-web) stays in its current languages — they are sample run targets, not part of the supervisor itself.

## Compatibility contract (must not break)

The Rust port is a drop-in replacement. End users keep their existing repos as-is.

- **Binary name**: `tukituki` (and `tktk` alias). Same subcommand surface: `version`, `list`, `status`, `start`, `stop`, `restart`, `logs`.
- **Flags**: `--config`, `--run-dir`, `--state-dir`, `--json`, `--otel-protocol`, `--otel-severity`, `--otel-port`. Same env-var equivalents (`TUKITUKI_*`).
- **YAML schema** for `.run/*.yaml`: identical fields (`name`, `command`, `args`, `workdir`, `env`, `description`, `cleanup`, `otel`). `${VAR}` expansion with `.env` semantics preserved (shell exports beat `.env`; per-target `env:` beats both).
- **Folder grouping**: one level of `.run/<group>/*.yaml` nesting, dot-dirs ignored.
- **State file** at `<state-dir>/state.json`: same JSON shape (`processes` map of name → `{name, pid, log_file, started_at, status, exit_code?}`). A Rust binary must be able to attach to a state file written by the Go binary and vice-versa (during the transition window).
- **Logs**: `<state-dir>/logs/<name>.log`, truncated on (re)start, 1000-line in-memory ring buffer.
- **Detach semantics**: spawned children survive `q`/exit (new session / setsid).
- **OTel collector**: same `--otel-port` persistence file (`<state-dir>/otel-port`), same injected env vars (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_METRICS_EXPORTER=none`, `OTEL_TRACES_EXPORTER=none`), virtual `otel-errors` target appearing at the bottom of the sidebar.
- **JSON output shapes** for `version --json`, `list --json`, `status --json`, `start/stop/restart --json`, and stderr error JSON: byte-for-byte identical to Go output where practical (field names, ordering documented in README).
- **TTY guard**: running with no subcommand against a non-TTY exits with the same error message and exit code.

## Crate layout

Single Cargo workspace at the repo root, replacing `go.mod`/`go.sum`:

```
Cargo.toml                          # workspace
crates/
  tukituki/                         # binary crate (cmd/tukituki/)
    src/main.rs
    src/cli.rs                      # clap subcommands & flag plumbing
    src/commands/                   # one file per subcommand
  tukituki-config/                  # internal/config/
    src/lib.rs
    src/dotenv.rs
    src/expand.rs
  tukituki-state/                   # internal/state/
  tukituki-process/                 # internal/process/
  tukituki-otel/                    # internal/otel/
    build.rs                        # tonic-build for notify.proto
    proto/notify.proto              # vendored from Go tree
  tukituki-tui/                     # internal/tui/
examples/                           # UNCHANGED (Go + Python sample targets)
documentation/                      # UNCHANGED (Hugo site)
docs/                               # UNCHANGED (built site)
plans/
  rust-port.md                      # this file
```

Keep `examples/`, `documentation/`, `docs/`, `LICENSE`, `README.md`, `.goreleaser.yaml`, and `.run/docs.yaml` in place. Replace `go.mod`/`go.sum` only after the Rust binary passes the acceptance checklist.

## Dependency mapping

| Go dep                                | Rust replacement                                              | Notes |
|---------------------------------------|---------------------------------------------------------------|-------|
| `spf13/cobra` + `spf13/viper`         | `clap` (derive) + manual layering (file → env → flag)         | viper's precedence is easy to mimic; clap covers help/usage and the `tktk` alias via argv0 inspection. |
| `bubbletea` / `bubbles` / `lipgloss`  | `ratatui` + `crossterm`                                       | Roll our own event loop; ratatui is immediate-mode so the bubbletea Update/View → Rust loop maps cleanly. |
| `charmbracelet/x/ansi`                | `ansi-to-tui` (for styled log rendering) + `vte` if we need a parser | Used for stripping/preserving ANSI in log lines. |
| `gopkg.in/yaml.v3` (KnownFields)      | `serde_yaml` with `deny_unknown_fields`                       | Same strictness behaviour. |
| `fsnotify`                            | `notify` (debouncer flavour)                                  | Used to watch `.run/` for changes. |
| `go.opentelemetry.io/proto/otlp`      | `opentelemetry-proto` crate (or `tonic-build` over vendored protos) | Need logs collector service messages; metrics/traces stubs return `Unimplemented`. |
| `google.golang.org/grpc`              | `tonic` (server + client)                                     | Hosts the OTLP logs gRPC service and the in-house `Notifier` service over UDS. |
| `google.golang.org/protobuf`          | `prost`                                                       | Pulled in by tonic. |
| net/http (OTLP HTTP)                  | `axum` (or `hyper` directly)                                  | OTLP/HTTP needs raw `Content-Type: application/x-protobuf` decoding — axum body extractor + prost decode. |
| syscall `Setpgid` / `signal 0`        | `nix` crate (`setsid`, `kill(pid, None)` for liveness)        | Detach via `unsafe { libc::setsid() }` in `pre_exec`. |
| `text/tabwriter`                      | `tabwriter` crate (same algorithm)                            | For plain-text `list`/`status` tables. |

## Module-by-module port

### 1. `tukituki-config` (internal/config)

Direct translation. Two responsibilities:

- **Target loading** (`config.go`):
  - `load_targets(run_dir) -> Result<Vec<RunTarget>>`
  - Discover `*.yaml` / `*.yml` at the top level (no group) and one level of subdirectories (`group = dir name`, skipping dot-dirs).
  - Parse with `serde_yaml::Deserializer` + `deny_unknown_fields`. On parse failure, emit a `RunTarget` with `parse_error` set rather than aborting (matches Go's "show the broken target in the TUI" behaviour).
  - Sort by name. Stamp `source_file` (absolute) and `group`.

- **dotenv** (`dotenv.go`):
  - `parse_dotenv(path) -> Result<BTreeMap<String, String>>`: blank lines + `#` ignored, optional `export ` prefix stripped, single/double quotes honoured (mirror existing `dotenv_test.go` cases).
  - `load_dotenv(project_root)`: parse, then for each key call `env::set_var` unless already set. Returns the parsed map.
  - `expand_env(targets, dotenv)`: walk every target's `command`, `args`, `workdir`, `env` values; replace `${VAR}` / `$VAR` using shell-like precedence (process env > `.env`).

**Test parity**: port every case from `config_test.go` and `dotenv_test.go` into `#[test]` modules. These tests are cheap and pin the semantics.

### 2. `tukituki-state` (internal/state)

Tiny module — translate verbatim.

- `ProcessState { name, pid, log_file, started_at, status, exit_code }` with `serde(rename_all = "snake_case")` and `serde(skip_serializing_if = "Option::is_none")` on `exit_code`.
- `Status` enum serialised as `running` / `stopped` / `failed` / `unknown`.
- `State::load(path)`: tolerate missing/corrupt files (return empty state, do not error).
- `State::save(path)`: atomic write via `tempfile::NamedTempFile::persist` in the same directory.
- `is_alive(pid)`: `nix::sys::signal::kill(Pid::from_raw(pid), None)` — `Ok(_)` or `EPERM` means alive; `ESRCH` means dead.
- `reconcile_alive(&mut self)`: flip `running` → `stopped` when PID is gone.

JSON output must match Go's `MarshalIndent("", "  ")` (2-space indent, sorted keys via `BTreeMap`). Snapshot-test against a fixture written by the Go binary.

### 3. `tukituki-process` (internal/process) — the hard one

This is the largest Go file (~1200 LOC) and the riskiest port. Plan in two passes.

**Pass A — spawn / track / reconcile (parity, no I/O streaming yet):**

- `Manager` struct owns: `targets`, `state` (mutex), `state_dir`, `logs_dir`, `project_root`, `otel_cfg`, `previous_otel_port`, and a `log_lines: HashMap<String, VecDeque<String>>` of in-memory ring buffers (1000 lines each).
- `Manager::new(targets, state_dir, project_root)` — mkdir logs dir, load state.
- `Manager::set_otel_config(cfg)` — port resolution & persistence in `<state-dir>/otel-port` (re-use prior port if bindable or our PID owns it, else allocate fresh via ephemeral `TcpListener::bind("127.0.0.1:0")`).
- `Manager::start_target(target)`:
  - Refuse if `parse_error` set.
  - If already alive, no-op.
  - Truncate `<logs_dir>/<name>.log`.
  - Build shell line: `$SHELL -l -c <cmd>` (fall back to `/bin/sh`). Use `tokio::process::Command` (or `std::process::Command`).
  - Set `pre_exec` closure calling `libc::setsid()` so the child becomes its own session leader and survives parent death.
  - `cmd.stdout(File)` + `cmd.stderr(same File)` (open log file with `O_TRUNC | O_WRONLY | O_CREAT`, mode `0o644`).
  - `cmd.envs(env::vars())` + per-target overrides, + OTel env if `target.otel && otel_cfg.port != 0`.
  - Resolve `workdir`: absolute as-is, relative joined with `project_root`.
  - Spawn, record PID + StartedAt + Running in state, persist state.json.
  - Spawn a tokio task that waits on the child; on exit, update status (`stopped` on success, `failed` on non-zero) and append `[process exited: code N]` to the log file so the tailer picks it up.

- `Manager::stop_target(name)`:
  - `kill(-pid, SIGTERM)` (negative PID → process group, since we `setsid`'d).
  - Wait up to ~3s for it to die; escalate to `SIGKILL`.
  - Run `cleanup:` commands via `$SHELL -l -c`, cwd = `workdir`, ignoring individual failures (log to ring buffer).
  - Update state.

- `Manager::restart_target(name)` = stop then start, same target object.

- `Manager::status(name)` / `Manager::list_status()` — read state, reconcile alive, return.

- `Manager::attach_to_existing(targets)` — repopulate state knowledge after a fresh process started against an existing `.tukituki/`. Used by every CLI subcommand and TUI startup.

**Pass B — log streaming:**

- `Manager::watch(name) -> tokio::sync::mpsc::Receiver<String>` returns a channel that yields each new line as it is appended to the log file.
- Implement by: opening the log file, seeking to end (or beginning for backfill — Go currently emits the ring buffer first, then tails), then in a tokio task using `tokio::fs::File` + `BufReader::lines` with retry-on-EOF (100ms `tailPollDelay`).
- Watchers are stored in `HashMap<String, Vec<Sender<String>>>` with cancellation via a `oneshot` per target.

- `Manager::append_log_line(name, line)` writes to both the on-disk file (for collector-style virtual targets) and the in-memory ring buffer, broadcasting to subscribers. Used for diagnostic lines emitted by the manager itself instead of writing to stderr (per commit `a3d3cba`).

**Tests**: port `manager_test.go` cases — spawn a `sleep 5`, assert state, kill, assert exit code, restart, etc.

### 4. `tukituki-otel` (internal/otel) — second-hardest

Three concerns: OTLP receiver, severity filter, and Notifier push channel.

**Proto codegen** (`build.rs`):

- Vendor `notify.proto` from `internal/otel/notify/`.
- Pull OTLP `logs/v1/logs.proto`, `collector/logs/v1/logs_service.proto`, `common/v1/common.proto`, `resource/v1/resource.proto` (either from `opentelemetry-proto` crate, or vendor + `tonic-build` ourselves for tighter version control). Same for metrics/traces collector services if we need to stub them to return `Unimplemented`.
- `tonic_build::configure().build_server(true).build_client(true).compile(&["notify.proto", ...], &[...])`.

**Collector** (`collector.go`):

- `Collector { port, protocol, min_severity, output, notify_socket }`.
- `run(cancel_token)`:
  - If `notify_socket` set: bind a `tonic` server on a Unix domain socket exposing the `Notifier` service. Subscribers receive a server-stream of `ErrorEvent`s pushed from `hub`.
  - Then dispatch on `protocol`:
    - `grpc`: bind `127.0.0.1:<port>`, register the `LogsService` (and stubs that return `Unimplemented` for metrics/traces — matches Go behaviour seen in `collector.go` so SDK auto-config doesn't crash).
    - `http`: axum router with `POST /v1/logs` accepting `application/x-protobuf` (and a JSON fallback if Go supports it — check `collector.go` for the JSON path before deciding).
  - Each incoming `ResourceLogs` is unwrapped to `LogRecord`s; filter by `severity_number >= min_severity`; format `[<service.name>] <body>`; write to `output` (defaults to stdout); push to `hub` if a notify socket is open.

- `Severity` parsing (`severity.go`): map `trace|debug|info|warn|error|fatal` → `SeverityNumber` constants. Port `severity_test.go` cases.

- `NotifyHub`: a `tokio::sync::broadcast` channel keyed by nothing (one stream, many receivers); each `Subscribe` RPC consumes the receiver and forwards events. Drop slow subscribers (lossy broadcast semantics match Go's `select { case ch <- ev: default: }`).

**Spawning the collector as a child**: the Go code runs the collector via `tukituki collector run --port X --severity Y --socket Z` as a re-exec of `os.Args[0]` (need to verify against `collector.go`). The Rust port mirrors this: add a hidden `tukituki collector run` subcommand that just constructs a `Collector` and blocks. The `Manager::ensure_otel_collector` spawns the same binary with those flags as a regular detached target named `otel-errors`.

### 5. `tukituki-tui` (internal/tui) — the largest port

Bubbletea is Model/Update/View; ratatui is immediate-mode draw with an explicit event loop. The mapping:

| Bubbletea concept                   | Rust / ratatui equivalent                                     |
|-------------------------------------|---------------------------------------------------------------|
| `tea.Msg` (interface)               | `enum AppEvent { Key(KeyEvent), Tick, LogLine{target, line}, OtelError(ErrorEvent), OtelBlink, FileChange, TargetsReloaded(Result<Vec<RunTarget>>), EditorExited(Result<()>), ActionResult(String), ... }` |
| `Update(msg) -> (model, cmd)`       | `app.handle(event) -> Option<Action>` — mutate `App` in place, return follow-up work. |
| `tea.Cmd`                           | `tokio::spawn` a task that sends `AppEvent`s onto an `mpsc::UnboundedSender<AppEvent>` held by the app. |
| `tea.Tick`                          | `tokio::time::interval` driving Tick events. |
| `viewport.Model`                    | A `LogViewport` struct: holds the wrapped/unwrapped line buffer, scroll offset, "at bottom?" flag, and renders a `Paragraph` (or custom widget for ANSI). |
| `lipgloss.Style`                    | `ratatui::style::Style` + `Modifier` flags. Encapsulate the equivalents of `styles.go` in one `theme.rs`. |
| `key.Binding`                       | A `KeyMap` with `KeyCode` matching; reuse the help layout via a small table. |
| Alt-screen + mouse capture          | `crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)`. |

**Concrete steps:**

1. **Skeleton**: an `App` struct with the same fields as Go's `Model` (targets, manager handle, selected index, computed `rows: Vec<DisplayRow>`, folder-expanded set, log buffers, viewport state, status map, mode flags). The `manager_interface.go` trait becomes `trait ManagerHandle` in Rust so tests can substitute a fake.
2. **Event loop** (`run(app)`):
   ```
   let (tx, rx) = mpsc::unbounded_channel();
   spawn_key_reader(tx.clone());          // crossterm::event::read in a blocking thread
   spawn_status_tick(tx.clone(), 1s);
   spawn_fsnotify(tx.clone(), run_dir);
   for target in subscribed { spawn_log_pump(target, tx.clone()); }
   spawn_otel_pump(tx.clone());           // if collector running
   loop {
       terminal.draw(|f| view(f, &app))?;
       match rx.recv().await { Some(ev) => update(&mut app, ev), None => break }
   }
   ```
3. **Layout** (`view`):
   - Top-level horizontal split: left sidebar (fixed width ~24 cols) + right log pane. When `zoom_logs` is on, sidebar is hidden.
   - Sidebar = list of folder/target rows with status glyphs, plus the keybinding legend at the bottom. Render with `List` widget or a custom one — the row formatting and selection cursor are simple enough that a manual `Paragraph` is fine.
   - Right pane = header line + scrolling log area + status line. Use `ansi-to-tui` to convert ANSI-coloured log bytes into styled `Text`. Soft-wrap when `wrap_logs` is on.
   - Overlays: help (`?`) and describe (`d` on a target) — modal `Block` painted over the right pane.
   - Search bar: input shown at the bottom of the right pane when `search_mode`.
4. **Keybindings**: port `keymap.go` verbatim. The README table is the source of truth.
5. **OTel blink**: a `tokio::time::sleep` task that emits `OtelBlink` every 500ms while `unread_otel_errors > 0`. Cancel/restart whenever the user selects the row.
6. **File-watching**: `notify::recommended_watcher` on `run_dir`, with a 200ms debounce. Emit `FileChange`, trigger a `TargetsReloaded` job, swap targets atomically when it returns clean.
7. **External editor** (`e` to edit YAML — check current code): suspend ratatui (`disable_raw_mode + LeaveAlternateScreen`), run `$EDITOR <source_file>`, re-enter alt screen, emit `EditorExited`.
8. **Detach behaviour**: on `q`, drop the manager *without* killing children — the child PIDs were detached at spawn, so just exit. On `Q` / `Ctrl-C`, walk the target list and stop everything before exiting.

**Where ratatui will hurt**: the bubbletea `viewport` does a lot of work for free (smooth scrolling, mouse wheel, follow-tail). In ratatui you maintain offset state yourself. Plan a half-day to get this feeling right (mouse wheel, `PgUp`/`PgDn`, `b`/`f`, auto-stick-to-bottom).

### 6. `tukituki` binary crate (cmd/tukituki/root.go)

Pure CLI plumbing.

- `clap` derive with the same flag layout (and argv0 inspection for `tktk` to flip the displayed name in help).
- Per-subcommand modules: `commands/version.rs`, `commands/list.rs`, ..., `commands/logs.rs`, `commands/collector.rs` (hidden, used internally to spawn the OTel receiver).
- Each subcommand has a text path and a `--json` path. Centralise JSON error output to stderr in a single helper so it matches Go (`{"error": "...", "available": [...]}`).
- `runRoot` (no subcommand) = `commands/tui.rs::run`. The TTY guard goes here (`std::io::IsTerminal::is_terminal()` on stdout); same exit code and message as Go.
- Config file resolution: re-implement viper's precedence (CLI flag > env > `.tukitukirc.yaml` in cwd > `$HOME/.tukitukirc.yaml` > defaults). A 30-line helper using `figment` is overkill — hand-roll it.

## Acceptance checklist

A user with an existing tukituki repo can:

- [ ] Replace the Go binary with the Rust binary and not change a single `.run/*.yaml`.
- [ ] `tktk` alias still works (binary inspects argv0).
- [ ] `tukituki list --json` produces byte-identical output to the Go version on the same `.run/`.
- [ ] `tukituki start` while a Go-built tukituki has already populated `.tukituki/state.json` correctly reconciles status (alive vs. dead).
- [ ] `tukituki` in a TTY opens the TUI; all README keybindings work; folder grouping renders; `e` opens `$EDITOR`; reload-on-file-change works.
- [ ] OTel collector: `otel: true` on a target launches the collector, injects `OTEL_EXPORTER_OTLP_ENDPOINT`, and the `otel-errors` row receives events.
- [ ] OTel port persistence: changing `--otel-port`, restarting, then running plain `tukituki` reuses the persisted port.
- [ ] Detach: open TUI, hit `q`, confirm targets keep running (`pgrep` for them).
- [ ] All ports of `*_test.go` cases pass under `cargo test`.
- [ ] `cargo build --release` produces a single static-ish binary suitable for the existing `.goreleaser.yaml` flow (we'll need to swap goreleaser for `cross` + GH Actions matrix builds — see Release section).

## Phasing / order of work

Each phase ends with the Rust binary running side-by-side with the Go binary so we can A/B against the same `.run/` repo.

1. **Phase 1 — scaffolding** (~½ day): Cargo workspace, clap skeleton, `version` subcommand (+`--json`), CI building the crate.
2. **Phase 2 — config + state** (~1 day): `tukituki-config` and `tukituki-state` crates with full test port. `list` and `list --json` subcommands working end-to-end against real `.run/` fixtures.
3. **Phase 3 — process manager (pass A)** (~2 days): spawn/stop/restart/status + state reconciliation + `cleanup:` execution + detachment. `start`/`stop`/`restart`/`status` subcommands wired up. No log streaming yet (use `logs` to `cat` the file).
4. **Phase 4 — process manager (pass B)** (~1 day): tailing, ring buffer, `logs --follow`. `tukituki logs <name>` matches Go's headless behaviour.
5. **Phase 5 — OTel collector** (~2 days): proto codegen, gRPC + HTTP receivers, severity filter, notify UDS. Hidden `collector run` subcommand. Manager integration (port resolution, env injection, virtual target). Verify against the existing `examples/go-api` after instrumenting it.
6. **Phase 6 — TUI** (~3–4 days): ratatui event loop, sidebar, log viewport (with ANSI + wrap + zoom), keybindings, search, help/describe overlays, folder grouping, file-watching reload, external editor, OTel blink.
7. **Phase 7 — compatibility hardening** (~1 day): JSON output byte-diff against the Go binary on a corpus of fixtures; argv0 alias; TTY guard; config-file precedence.
8. **Phase 8 — release plumbing** (~½ day): replace `.goreleaser.yaml` with a GH Actions matrix using `cross` for linux/darwin/{x86_64,aarch64}; ship a Homebrew formula update. Keep `tktk` symlink in the formula. Update README install section.
9. **Phase 9 — Go removal**: delete `cmd/`, `internal/`, `go.mod`, `go.sum`. Leave `examples/` and `documentation/` alone. Final README pass.

Total: ~2 weeks of focused work for a single contributor.

## Open questions / decisions to make before starting

- **Async runtime**: tokio is the obvious choice (tonic depends on it). Stick with tokio multi-thread.
- **OTLP types**: vendor protos and codegen ourselves, or depend on `opentelemetry-proto`? Vendoring is more code but insulates us from upstream churn and from pulling the full OTel SDK transitively. **Recommendation: vendor.**
- **ANSI in logs**: `ansi-to-tui` covers SGR sequences. If targets emit cursor moves or other control sequences, we'll need to strip more aggressively. Audit a couple of noisy targets (Vite, npm) before committing.
- **MSRV**: pin to whatever stable shipped ~6 months ago (likely 1.85 by the time this runs).
- **Cross-compilation matrix**: at minimum linux-x86_64, linux-aarch64, darwin-x86_64, darwin-aarch64. Windows is currently unsupported in Go (the syscall code is Unix-only) — keep it that way unless explicitly requested.
- **Backward-compat for `.tukituki/state.json`**: do we accept Go-written state files mid-transition, or wipe on first Rust run? **Recommendation: accept them — the JSON shape is small enough to be schema-stable, and forcing users to stop processes before upgrading is bad UX.**

## Risks

- **Process detachment on macOS**: `setsid` semantics are subtly different from Linux around controlling-TTY inheritance. Test on both before declaring Phase 3 done.
- **Bubbletea → ratatui parity for log viewport**: this is where the port will feel "off" if rushed. Budget extra time for scroll/wrap/follow polish.
- **gRPC + UDS on macOS**: `tonic` supports it but the socket-path-length limit (104 bytes) is tighter than Linux. Keep the notify socket path under `<state-dir>/notify.sock` and validate length on startup.
- **JSON output drift**: easy to introduce field-order changes that break agent scripts. Lock with golden-file snapshot tests fed by the Go binary's output during the transition.

## Out of scope

- Touching `examples/` source. They stay in Go/Python — they're sample run targets, and rewriting them adds risk without value.
- Touching `documentation/` (Hugo site) or `docs/` (built output) beyond updating install instructions.
- Adding new features beyond Go parity. Anything net-new goes in a follow-up plan.
