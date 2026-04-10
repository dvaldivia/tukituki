---
title: Quick Start
weight: 2
---

This guide takes you from zero to a running process in under five minutes. You will create a process definition, launch tukituki, and learn the essential keyboard shortcuts.

{{< steps >}}

### Create a `.run/` directory

Inside your project root, create the directory where tukituki looks for process definitions:

```sh
mkdir .run
```

Every `.yaml` file placed here defines one managed process.

### Write your first process definition

The fastest way is to use the `new` command:

```sh
tukituki new api 'go run ./cmd/api/' -e PORT=8080 -e ENV=development
```

This creates `.run/api.yaml` for you. You can also create the file by hand with the following content, adjusting the command to match your project:

```yaml
name: api
command: go
args:
  - run
  - ./cmd/api/
env:
  PORT: "8080"
  ENV: development
```

- `name` — display name shown in the TUI and used for the log file.
- `command` — the executable to run (resolved via your login shell, so `$PATH` from nvm/pyenv/rbenv is available).
- `args` — positional arguments passed to `command`.
- `env` — additional environment variables merged into the process environment.

You can add as many `.yaml` files as you have processes (e.g. `.run/worker.yaml`, `.run/frontend.yaml`).

### Start tukituki

From your project root, run:

```sh
tukituki
```

The TUI opens in your terminal and all processes defined in `.run/` start automatically. Each process appears as a row showing its name and current status.

### Navigate and view logs

Use the keyboard to move around the TUI:

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move selection between processes |
| `Enter` | Open the log pane for the selected process |
| `Esc` | Close the log pane |

Live log output streams in real time. Logs are also written to `.tukituki/logs/<name>.log` so you can inspect them later with any tool.

### Detach — leave processes running

Press **`q`** to close the TUI and return to your shell prompt. All managed processes continue running in the background. Their state is tracked in `.tukituki/state.json`.

Reopen the TUI at any time by running `tukituki` again from the same project root.

### Stop everything

Press **`Q`** (capital Q) or **`Ctrl+C`** to stop all managed processes and exit tukituki.

{{< /steps >}}

{{< callout type="info" >}}
tukituki stores runtime state and logs under `.tukituki/` in your project root. Add this directory to your `.gitignore` so it is never committed:

```sh
echo '.tukituki/' >> .gitignore
```
{{< /callout >}}

## Running Without the TUI

If you need to start all processes headlessly — for example inside a script or a CI step — use the `start` subcommand:

```sh
tukituki start
```

This starts every process defined in `.run/*.yaml` without opening the TUI. Logs are still written to `.tukituki/logs/`.
