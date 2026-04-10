---
title: TUI Guide
weight: 3
---

tukituki's interactive terminal UI gives you a live view of every managed process in one window. You can tail logs, restart or stop individual processes, dump log files to disk, and exit without killing anything — all without leaving the terminal.

## Layout

```
 tukituki                              q=detach  Q/^C=stop all
┌──────────────────────┬─────────────────────────────────────────────┐
│ * frontend           │ server                                       │
│ * marketing-site     │ ────────────────────────────────────────────│
│ ▶ * server           │ 2026/01/15 10:23:01 INFO starting HTTP...   │
│ * worker             │ 2026/01/15 10:23:01 INFO gRPC listening...  │
│                      │                                              │
│──────────────────────│                                              │
│ r restart  s stop    │                                              │
│ S start    d dump    │                                              │
│ c clear    q detach  │                                              │
│ Q/^C stop all        │                                              │
└──────────────────────┴─────────────────────────────────────────────┘
```

The TUI is divided into three areas: the **header bar**, the **left panel** (process list), and the **right panel** (log viewport).

## Header Bar

The header bar runs across the top of the window and contains:

- **tukituki** — the application name on the left.
- **q=detach  Q/^C=stop all** — a persistent reminder of the two most important exit keys, always visible at the top right.

## Left Panel — Process List

The left panel lists every process defined in your `.run/*.yaml` files. Each row shows:

- A **status icon** indicating the current state of the process (see [Status Icons](#status-icons) below).
- The **process name** as defined in the run target file.
- A `▶` marker on the currently selected row.

Below the process list, a horizontal rule separates the list from the **key hint area**, which displays the most commonly used keybindings as a quick reference without leaving the screen.

### Status Icons

| Icon | Meaning |
|------|---------|
| `*` | Running — the process is currently active. |
| `-` | Stopped — the process was stopped cleanly. |
| `!` | Failed — the process exited with a non-zero exit code. |
| `?` | Unknown — no state has been recorded yet (e.g. just started). |

The status icons update on a one-second refresh cycle, so they reflect the true state of the underlying process manager at all times.

## Right Panel — Log Viewport

The right panel is dedicated to the **currently selected process**. It shows:

- The **process name** as a title at the top of the panel.
- A **separator line** beneath the title.
- A scrollable **log viewport** containing the most recent output from that process.

The log buffer holds the last **1 000 lines** in memory. When you switch between processes in the left panel, the right panel immediately switches to that process's log buffer.

{{< callout type="info" >}}
The log viewport **auto-scrolls to the bottom** whenever new output arrives — as long as you have not manually scrolled up. Once you scroll up (with `PgUp` or `b`), auto-scroll is paused so you can read historical output without losing your place. Scrolling back to the bottom resumes auto-scroll.
{{< /callout >}}

## Keybindings

### Navigation

| Key | Action |
|-----|--------|
| `↑` / `k` | Move selection up in the process list. |
| `↓` / `j` | Move selection down in the process list. |
| `Tab` | Cycle to the next process (wraps around). |
| `PgUp` / `b` | Scroll the log viewport up one page. |
| `PgDn` / `f` | Scroll the log viewport down one page. |

### Process Control

| Key | Action |
|-----|--------|
| `r` | Restart the selected process. |
| `s` | Stop the selected process. |
| `S` | Start the selected process (if stopped or failed). |

### Log Management

| Key | Action |
|-----|--------|
| `d` | Dump the current log buffer to a file named `<name>-YYYYMMDD-HHMMSS.log` in the current working directory. |
| `c` | Clear the log buffer — wipes the in-memory buffer and truncates the on-disk log file. |

{{< callout type="warning" >}}
`c` (clear) is destructive: it truncates the on-disk log file as well as the in-memory buffer. Use it when you want a clean slate after a restart. If you need to keep historical logs, use `d` (dump) first to save them to a timestamped file.
{{< /callout >}}

### Exiting

| Key | Action |
|-----|--------|
| `q` | Detach — quit the TUI and leave all processes running in the background. |
| `Q` / `Ctrl+C` | Stop all processes and exit. |

## Detach and Reattach

tukituki separates the **TUI** from the **process manager**. The process manager runs independently; the TUI is just a view into it.

{{< callout type="info" >}}
Pressing `q` closes the terminal UI but leaves every process running. You can close your terminal window, come back later, and run `tukituki` again in the same project directory to reattach. tukituki reads the state file and re-tails the existing log files from where they left off.
{{< /callout >}}

Use `Q` or `Ctrl+C` only when you actually want to stop everything — for example, before shutting down your machine or switching to a different project entirely.
