---
title: Guides
weight: 5
toc: false
---

These guides go deeper on behaviors that matter once your processes are running day-to-day: cleaning up resources, state persistence, and collecting OpenTelemetry errors.

{{< cards >}}
  {{< card link="cleanup" title="Cleanup Commands" icon="trash" subtitle="Run shell commands after a process stops — free ports, kill stragglers, remove PID files and sockets." >}}
  {{< card link="state-persistence" title="State & Log Persistence" icon="database" subtitle="How tukituki survives restarts: state.json, log file lifecycle, the in-memory ring buffer, and the reattach flow." >}}
  {{< card link="opentelemetry" title="OpenTelemetry Error Collection" icon="eye" subtitle="Collect OTel log records from your services and surface errors in one place — no external collector needed." >}}
{{< /cards >}}
