---
title: Keybindings Reference
weight: 1
---

Quick reference for every key recognised by the tukituki TUI. Keys are case-sensitive.

## Navigation

| Key | Action |
|-----|--------|
| `↑` / `k` | Move selection up in the process list. |
| `↓` / `j` | Move selection down in the process list. |
| `Tab` | Cycle to the next process (wraps around to the top). |
| `PgUp` / `b` | Scroll the log viewport up one page. |
| `PgDn` / `f` | Scroll the log viewport down one page. |

Both arrow keys and vim-style `j`/`k` move the process selection. Use `Tab` to step forward through the list without leaving the home row.

## Process Control

| Key | Action |
|-----|--------|
| `r` | Restart the selected process. |
| `s` | Stop the selected process. |
| `S` | Start the selected process (when stopped or failed). |

`r` is equivalent to `s` followed by `S` — it stops the running process and immediately starts it again. Use `S` (uppercase) to start a process that was previously stopped with `s` or that exited with a failure (`!`).

## Log Management

| Key | Action |
|-----|--------|
| `d` | Dump the log buffer to `<name>-YYYYMMDD-HHMMSS.log` in the current working directory. |
| `c` | Clear the in-memory log buffer and truncate the on-disk log file. |

`d` writes a point-in-time snapshot of the last 1 000 lines to a timestamped file and leaves the live buffer untouched. `c` is a clean-slate operation — it wipes both the screen buffer and the underlying file, which is useful immediately after a restart when you want only fresh output.

## Exiting

| Key | Action |
|-----|--------|
| `q` | Detach — close the TUI, leave all processes running. |
| `Q` / `Ctrl+C` | Stop all processes and exit. |

Prefer `q` when you just want to close the window and come back later. Run `tukituki` again in the same directory to reattach and resume tailing logs. Use `Q` or `Ctrl+C` only when you want to shut everything down.

## Status Icons

The left panel prefixes each process name with a status icon:

| Icon | Meaning |
|------|---------|
| `*` | Running — the process is active. |
| `-` | Stopped — the process was stopped cleanly. |
| `!` | Failed — the process exited with a non-zero exit code. |
| `?` | Unknown — no state recorded yet. |

Icons refresh every second. A `!` icon means the process needs attention; press `S` to restart it or check the log panel for the error output.
