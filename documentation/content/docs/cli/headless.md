---
title: Headless Mode
weight: 1
---

tukituki can manage processes entirely from the command line, without ever opening the TUI. This is useful for:

- CI/CD pipelines that need to bring up service dependencies before running tests.
- Startup scripts and `rc` files that should boot your stack on login.
- `tmux` or `screen` sessions where you want to avoid a full-screen TUI.
- Integration with init systems such as `systemd` or `launchd`.

The four commands you reach for in headless mode are `start`, `status`, `logs`, and `stop`.

---

## The Basic Workflow

### 1. Start all processes

```sh
tukituki start
```

This reads every `.yaml` file in `.run/`, attaches to any already-running processes, and spawns anything that is not yet running. It returns immediately — the processes continue in the background and write their output to `.tukituki/logs/<name>.log`.

### 2. Check what is running

```sh
tukituki status
```

Prints a table of every target with its current status (`running`, `stopped`, `failed`, or `unknown`). Use this after `start` to confirm everything came up cleanly before proceeding.

```
NAME        STATUS    DESCRIPTION
api         running   HTTP API server
worker      running   Background job processor
frontend    running   React dev server
```

### 3. Tail output from a process

```sh
tukituki logs <name>
```

Prints the last 100 buffered lines from the named target's log file, then follows new output until **Ctrl+C** — identical to `tail -n 100 -f`. This works whether the process is currently running or has already stopped.

```sh
# Stream logs from the api process
tukituki logs api
```

### 4. Shut everything down

```sh
tukituki stop
```

Sends SIGTERM to every running process, waits up to 5 seconds for each to exit, then SIGKILL if needed. Any `cleanup` commands defined in the target YAML are run afterward.

---

## Example: Start, Test, Stop Script

A common pattern in CI is to bring up dependencies, run a test suite against them, and then tear everything down regardless of whether the tests passed.

```sh
#!/usr/bin/env bash
set -euo pipefail

# Start all services in the background
tukituki start

# Wait until the API is accepting connections
echo "Waiting for API to be ready..."
for i in $(seq 1 30); do
  if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
    echo "API is ready."
    break
  fi
  if [ "$i" -eq 30 ]; then
    echo "API did not start in time."
    tukituki stop
    exit 1
  fi
  sleep 1
done

# Run the test suite
# Use || to capture the exit code so we can still stop services on failure
test_exit=0
npm test || test_exit=$?

# Always shut down, even if tests failed
tukituki stop

exit "$test_exit"
```

This script is intentionally self-contained: services are started and stopped within the same invocation so the CI environment is left clean.

---

## State Persistence Across Restarts

tukituki records the PID and status of every managed process in `<state-dir>/state.json` (default: `.tukituki/state.json`). This file is read at the beginning of every `start`, `stop`, `status`, and `logs` invocation.

Because of this:

- If your machine reboots or your shell exits unexpectedly, running `tukituki start` again is safe — it will detect which PIDs are no longer alive, mark those targets as `stopped`, and re-spawn them.
- Running `tukituki status` after a reboot accurately reflects which processes are actually running vs. which ones silently died.
- Log files under `.tukituki/logs/` persist across restarts (they are only truncated by `tukituki restart`), so you can inspect output from a previous run with `tukituki logs <name>` even after the process has stopped.

Add `.tukituki/` to your `.gitignore` so these runtime artifacts are never committed:

```sh
echo '.tukituki/' >> .gitignore
```

---

## Starting a Specific Target

You can start or stop individual targets by name instead of operating on the whole group:

```sh
# Start only the worker process
tukituki start worker

# Stop only the worker process
tukituki stop worker

# Restart the worker after a code change
tukituki restart worker
```

This is useful when you are iterating on a single service and do not want to cycle the others.

---

## Integration with systemd or launchd

{{< callout type="tip" >}}
Because `tukituki start` is idempotent and returns immediately, it fits naturally inside a `systemd` service unit or a `launchd` plist. Point the unit at `tukituki start` and let the init system handle restarts on failure.
{{< /callout >}}

### systemd example

Create `/etc/systemd/system/myproject.service`:

```ini
[Unit]
Description=My project dev processes (tukituki)
After=network.target

[Service]
Type=forking
User=youruser
WorkingDirectory=/home/youruser/myproject
ExecStart=/usr/local/bin/tukituki start
ExecStop=/usr/local/bin/tukituki stop
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Enable and start the unit:

```sh
sudo systemctl enable myproject
sudo systemctl start myproject
```

### launchd example (macOS)

Create `~/Library/LaunchAgents/com.myproject.tukituki.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.myproject.tukituki</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/tukituki</string>
    <string>start</string>
  </array>
  <key>WorkingDirectory</key>
  <string>/Users/youruser/myproject</string>
  <key>RunAtLoad</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/Users/youruser/myproject/.tukituki/launchd.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/youruser/myproject/.tukituki/launchd.log</string>
</dict>
</plist>
```

Load it:

```sh
launchctl load ~/Library/LaunchAgents/com.myproject.tukituki.plist
```

In both cases, tukituki writes individual process logs to `.tukituki/logs/` as usual, and you can inspect them with `tukituki logs <name>` at any time.
