---
title: tukituki
layout: hextra-home
---

{{< hextra/hero-badge >}}
  <div class="hx-w-2 hx-h-2 hx-rounded-full hx-bg-primary-400"></div>
  <span>Free & Open Source</span>
  {{< icon name="arrow-circle-right" attributes="height=14" >}}
{{< /hextra/hero-badge >}}

<div class="hx-mt-6 hx-mb-6">
{{< hextra/hero-headline >}}
  Manage dev processes&nbsp;from&nbsp;your terminal
{{< /hextra/hero-headline >}}
</div>

<div class="hx-mb-12">
{{< hextra/hero-subtitle >}}
  A lightweight TUI for starting, stopping, and tailing multiple&nbsp;processes — with a single command.
{{< /hextra/hero-subtitle >}}
</div>

<div class="hx-mb-6">
{{< hextra/hero-button text="Get Started" link="docs/getting-started" >}}
{{< hextra/hero-button text="GitHub" link="https://github.com/dvaldivia/tukituki" style="secondary" >}}
</div>

{{< cards >}}
  {{< card title="Single Command" icon="terminal" subtitle="Run `tukituki` in any project to launch every process defined in `.run/*.yaml` at once." >}}
  {{< card title="Detached Processes" icon="bookmark" subtitle="Processes survive TUI exit. Close the terminal and come back — everything is still running." >}}
  {{< card title="Login Shell" icon="cog" subtitle="Processes run via `$SHELL -l -c`, so nvm, pyenv, rbenv, and your full PATH are always available." >}}
  {{< card title="Headless CLI" icon="cog" subtitle="Skip the TUI entirely. Use `start`, `stop`, `restart`, `status`, and `logs` subcommands from scripts or CI." >}}
  {{< card title="Log Management" icon="document-text" subtitle="Per-process log files written to `.tukituki/logs/<name>.log`. Tail any process live from the TUI or CLI." >}}
  {{< card title="Persistent State" icon="light-bulb" subtitle="Process state is stored in `.tukituki/state.json` so tukituki always knows what is running, even after restarts." >}}
{{< /cards >}}
