---
title: Run Target Reference
weight: 1
---

Each process that tukituki manages is described by a YAML file placed in the run directory (default: `.run/`). You can have one process per file or several — the only constraint is that every `name` must be unique across all files in the directory.

## File Discovery

tukituki loads every file matching `*.yaml` or `*.yml` inside the run directory and sorts the resulting processes alphabetically by their `name` field. The sort order affects the default display order in the TUI process list.

{{< callout type="warning" >}}
`name` must be unique across **all** YAML files in the run directory, including those nested under group subdirectories. Duplicate names will cause a startup error.
{{< /callout >}}

## Grouping Targets Into Folders

When the number of run targets grows, you can keep the TUI tidy by moving related targets into a subdirectory of the run directory. Every YAML file inside an immediate subdirectory becomes part of a **group** named after that subdirectory:

```
.run/
├── api.yaml              # ungrouped — appears at the top of the list
├── worker.yaml           # ungrouped
└── kb/                   # group "kb"
    ├── acme.yaml         # collapsed under the "kb" folder
    ├── meshlink.yaml
    └── sentinel.yaml
```

Behaviour:

- Targets inside a subdirectory are loaded and started exactly like top-level targets — grouping is purely a display concern.
- In the TUI each group is rendered as a single folder row (`▶ kb (3)`). The arrow points **right** when collapsed and **down** when expanded (`▼ kb (3)`).
- Folders are **collapsed by default**, so a project with many group members has the same visual footprint as one with a single row per group.
- Expand or collapse a folder with `→`/`l` or `←`/`h` while it is selected, or press `Enter`/`Space` to toggle it. See the [TUI keybindings reference]({{< relref "/docs/tui/keybindings" >}}) for the full list.
- Only one level of nesting is honoured. Files in `.run/foo/bar/*.yaml` are ignored. Hidden subdirectories (those whose name starts with `.`) are skipped.
- The headless CLI (`tukituki list`, `start`, `stop`, etc.) is unaffected — it operates on target **names** and ignores group membership entirely.

{{< callout type="info" >}}
The group name is derived from the subdirectory; it cannot be set in the YAML file. To move a target between groups, move (or rename) its YAML file.
{{< /callout >}}

## Field Reference

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | string | yes | — | Unique identifier for the process. Used in CLI commands and the TUI list. |
| `command` | string | yes | — | The executable to run. Resolved through `$PATH` inside a login shell. |
| `workdir` | string | no | project root | Directory to run the command in. Resolved relative to the directory where `tukituki` is invoked. |
| `args` | list of strings | no | `[]` | Positional arguments passed to `command`. |
| `env` | map of strings | no | `{}` | Additional environment variables injected into the process. Added on top of the full parent environment. |
| `description` | string | no | `""` | Human-readable description shown in the TUI and `tukituki status` output. |
| `cleanup` | list of strings | no | `[]` | Shell commands run sequentially after the process stops. Useful for releasing ports or removing lock files. |
| `otel` | bool | no | `false` | Enable OpenTelemetry log collection. When `true`, tukituki injects `OTEL_EXPORTER_OTLP_ENDPOINT` into the process and starts a bundled OTLP receiver. See the [OpenTelemetry guide]({{< relref "/docs/guides/opentelemetry" >}}). |

## Annotated Example

The following is a real run target from the project. It starts the Go gRPC/HTTP backend and frees the ports it occupies on shutdown.

```yaml
name: server
description: "Go gRPC/HTTP backend server"

# The executable. Resolved via $PATH in a login shell.
command: go

# Relative to the directory where tukituki is invoked.
workdir: backend

# Passed directly to `command` as positional arguments.
args:
  - run
  - ./cmd/server

# Merged on top of the inherited parent environment.
env:
  HTTP_PORT: "8182"
  GRPC_PORT: "9192"
  DB_HOST: localhost

# Cleanup hooks run after the process exits, in order.
cleanup:
  - "lsof -ti:8182 | xargs kill -9 2>/dev/null || true"
  - "lsof -ti:9192 | xargs kill -9 2>/dev/null || true"
```

## Shell Execution

Every command is launched as:

```
$SHELL -l -c "<command> <args...>"
```

The `-l` flag starts a **login shell**, which means your shell's profile files (`.bash_profile`, `.zprofile`, etc.) are sourced before the command runs. Version managers that hook into the shell profile — such as **nvm**, **pyenv**, and **rbenv** — work without any extra configuration.

## Environment Variables

The process inherits the **complete environment** of the tukituki parent process. The `env` map adds or overrides specific variables on top of that inherited environment. All values must be strings; quote numeric values explicitly (e.g. `"8182"`).

## Cleanup Hooks

The `cleanup` list contains shell commands that run **sequentially** after the managed process stops — whether it exited on its own, was stopped via the TUI, or killed by a CLI command.

Key behaviors:

- Hooks run in the target's `workdir` if one is set, otherwise in the project root.
- Each hook runs in its own subshell (`$SHELL -c`). Failures are logged but are **non-fatal** — remaining hooks continue to run.
- Hooks do not receive any output from the stopped process.

### Common patterns

Free a specific port:

```yaml
cleanup:
  - "lsof -ti:8080 | xargs kill -9 2>/dev/null || true"
```

Kill all processes matching a name:

```yaml
cleanup:
  - "pkill -f 'my-dev-server' 2>/dev/null || true"
```

Remove a PID file or socket:

```yaml
cleanup:
  - "rm -f /tmp/myapp.pid"
  - "rm -f /tmp/myapp.sock"
```

The `|| true` suffix is a common idiom to prevent a non-zero exit code from being treated as a hook failure when the target resource no longer exists.
