---
title: Cleanup Commands
weight: 1
---

The `cleanup` field in a run target specifies shell commands that run automatically every time a process stops — whether it exits on its own, is stopped from the TUI, or killed via a CLI command. Use cleanup hooks to free ports, kill stray child processes, and remove temporary files that the process leaves behind.

## When Cleanup Runs

Cleanup executes **after the managed process has been terminated** (SIGTERM followed by SIGKILL if necessary) and **before `Stop()` returns**. The sequence looks like this:

1. tukituki sends SIGTERM to the process group.
2. If the process does not exit within the grace period, SIGKILL is sent.
3. Once the process is confirmed dead, each cleanup command runs in order.
4. `Stop()` returns — the TUI updates status, and CLI commands complete.

This ordering means cleanup commands can safely assume the process is no longer running when they execute.

## Shell Execution Context

Each cleanup command is executed as:

```
$SHELL -l -c "<cleanup command>"
```

The `-l` flag starts a **login shell**, so your shell profile (`.bash_profile`, `.zprofile`, etc.) is sourced before the command runs. Tools that hook into the shell profile — such as **nvm**, **pyenv**, and **rbenv** — are available inside cleanup commands without any extra configuration.

If the run target has a `workdir` set, the cleanup command runs in that directory. Otherwise it runs in the project root (the directory where `tukituki` was invoked).

Each command runs in its own subshell — environment variables set in one cleanup step are not visible to subsequent steps.

## Making Cleanup Non-Fatal

Cleanup commands frequently target resources that may or may not exist at the time they run. `lsof` finds nothing if the port is already free; `pkill` exits non-zero if no matching process exists; `rm` fails if the file was never created. A non-zero exit code from a cleanup command will be logged, but does not abort subsequent cleanup steps.

The idiomatic way to suppress spurious failures is to append `|| true`:

```yaml
cleanup:
  - "lsof -ti:8080 | xargs kill -9 2>/dev/null || true"
```

The `2>/dev/null` suppresses stderr from the command itself; `|| true` ensures the overall exit code is always zero. Use both together when you expect the resource to be absent in some scenarios.

{{< callout type="warning" >}}
Cleanup failures are logged to stderr but are **non-fatal**. A failing cleanup step does not abort the remaining steps in the list and does not cause `tukituki stop` to return a non-zero exit code. Always check logs under `.tukituki/logs/<name>.log` if you suspect a cleanup hook is not behaving as expected.
{{< /callout >}}

## Common Patterns

### Kill a process by port

When a server holds a TCP port open, use `lsof` to find and kill the PID:

```yaml
cleanup:
  - "lsof -ti:8080 | xargs kill -9 2>/dev/null || true"
```

`lsof -ti:PORT` prints only the PID (the `-t` flag suppresses all other output), which is piped directly into `kill -9`. If the port is already free, `lsof` exits with no output and `xargs` does nothing.

To free multiple ports, add one line per port:

```yaml
cleanup:
  - "lsof -ti:8080 | xargs kill -9 2>/dev/null || true"
  - "lsof -ti:9090 | xargs kill -9 2>/dev/null || true"
```

### Kill stray child processes by name pattern

Some commands (e.g. `go run`, compilers, or bundlers) spawn child processes that outlive the parent. Use `pkill -f` to match against the full command line:

```yaml
# Kill lingering go-build binaries left by `go run`
cleanup:
  - "pkill -f 'go-build.*cmd/worker' 2>/dev/null || true"

# Kill a bundler watcher that was spawned by the dev server
cleanup:
  - "pkill -f 'esbuild.*--watch' 2>/dev/null || true"
```

`pkill -f` matches against the entire argument string, not just the executable name, which makes it useful for targeting specific invocations when multiple instances of the same binary are running.

### Remove PID files and sockets

Daemons that write a PID file or Unix socket on startup will refuse to start again if those files are present. Clean them up on stop:

```yaml
cleanup:
  - "rm -f /tmp/myapp.pid"
  - "rm -f /tmp/myapp.sock"
```

The `-f` flag makes `rm` succeed silently if the file does not exist, so `|| true` is not required here — though it does no harm to include it.

### Multiple cleanup steps in sequence

Cleanup steps run in the order they are listed. Combine multiple patterns freely:

```yaml
name: api
command: go
args: [run, ./cmd/api]
env:
  HTTP_PORT: "8082"
  GRPC_PORT: "9092"
cleanup:
  # Free both ports
  - "lsof -ti:8082 | xargs kill -9 2>/dev/null || true"
  - "lsof -ti:9092 | xargs kill -9 2>/dev/null || true"
  # Kill any stray child processes
  - "pkill -f 'go-build.*cmd/api' 2>/dev/null || true"
  # Remove the PID file written by the server on startup
  - "rm -f /tmp/api.pid"
```

Steps are independent — each runs in a fresh subshell. A failure in an earlier step does not prevent later steps from running.

## Full Field Reference

See the [Run Target Reference](../configuration/run-targets#cleanup-hooks) for the complete field definition, including the `workdir` interaction and a list of all supported fields.
