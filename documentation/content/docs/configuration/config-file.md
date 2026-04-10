---
title: Config File & Flags
weight: 2
---

tukituki's own behavior — where it looks for process definitions and where it writes state — is controlled through CLI flags, environment variables, and a config file. All three sources are optional; reasonable defaults apply when nothing is specified.

## CLI Flags

| Flag | Env Variable | Default | Description |
|---|---|---|---|
| `--config` | — | `.tukitukirc.yaml` in cwd, then `$HOME` | Path to the tukituki config file. When set, the default search path is bypassed entirely. |
| `--run-dir` | `TUKITUKI_RUN_DIR` | `.run` | Directory containing `*.yaml` / `*.yml` run target definitions. Relative paths are resolved from the current working directory. |
| `--state-dir` | `TUKITUKI_STATE_DIR` | `.tukituki` | Directory where tukituki stores process state, PID files, and log output. Created automatically if it does not exist. |

## Config File

The config file lets you persist flag values so you do not need to repeat them on every invocation.

### Search Path

When `--config` is not set, tukituki looks for `.tukitukirc.yaml` in this order:

1. The **current working directory** (the directory where you run `tukituki`)
2. `$HOME`

The first file found wins. If neither exists, all settings fall back to their defaults.

### Format

```yaml
# .tukitukirc.yaml

# Directory containing *.yaml / *.yml run target definitions.
run_dir: config/processes

# Directory for state files and log output.
state_dir: .cache/tukituki
```

Both fields are optional. Omit a field to accept the built-in default for that setting.

## Precedence

When the same setting is provided through multiple sources, the following precedence applies (highest to lowest):

1. **CLI flag** — always wins
2. **Environment variable** (`TUKITUKI_RUN_DIR`, `TUKITUKI_STATE_DIR`)
3. **Config file** (`.tukitukirc.yaml`)
4. **Built-in default**

### Example

Given this setup:

```bash
# .tukitukirc.yaml sets run_dir to "config/processes"
# but the env var overrides it:
export TUKITUKI_RUN_DIR=my-overrides

# and the flag overrides everything:
tukituki --run-dir=local-run
```

tukituki will use `local-run` as the run directory.

## Environment Variable Prefix

All environment variables use the `TUKITUKI_` prefix. The variable name is the flag name in upper-snake-case with the prefix prepended:

| Flag | Environment Variable |
|---|---|
| `--run-dir` | `TUKITUKI_RUN_DIR` |
| `--state-dir` | `TUKITUKI_STATE_DIR` |

The `--config` flag does not have a corresponding environment variable; use the flag directly when you need to point to a non-default config file location.
