---
title: Guides
weight: 5
toc: false
---

These guides go deeper on two behaviors that matter most once your processes are running day-to-day: cleaning up resources when a process stops, and understanding how tukituki preserves state across restarts.

{{< cards >}}
  {{< card link="cleanup" title="Cleanup Commands" icon="trash" subtitle="Run shell commands after a process stops — free ports, kill stragglers, remove PID files and sockets." >}}
  {{< card link="state-persistence" title="State & Log Persistence" icon="database" subtitle="How tukituki survives restarts: state.json, log file lifecycle, the in-memory ring buffer, and the reattach flow." >}}
{{< /cards >}}
