---
title: Installation
weight: 1
---

## Prerequisites

- **Go 1.21 or later** — tukituki is distributed as a Go module. Install Go from [go.dev/dl](https://go.dev/dl/).

Verify your Go version:

```sh
go version
# go version go1.21.0 darwin/arm64 (or similar)
```

## Install via go install

The fastest way to install tukituki is directly from the module path:

```sh
go install github.com/dvaldivia/tukituki/cmd/tukituki@latest
```

Go places the binary in `$GOPATH/bin` (or `$HOME/go/bin` by default). Make sure that directory is on your `PATH`:

```sh
export PATH="$HOME/go/bin:$PATH"
```

Add that line to your shell profile (`.zshrc`, `.bashrc`, `.bash_profile`, etc.) to make it permanent.

## Build from Source

If you want to pin to a specific commit or contribute to tukituki, build from source:

```sh
git clone https://github.com/dvaldivia/tukituki.git
cd tukituki
go install ./cmd/tukituki/
```

## Verify the Installation

```sh
tukituki --help
```

You should see the help output listing available commands and flags. If the shell reports `command not found`, double-check that `$GOPATH/bin` is in your `PATH`.

## Shell Compatibility

tukituki launches every managed process through a **login shell** (`$SHELL -l -c`). This means version managers that modify `PATH` at login time — such as **nvm**, **pyenv**, and **rbenv** — work automatically without any extra configuration. tukituki reads `$SHELL` from your environment and honours whatever shell you use: zsh, bash, fish, and others are all supported.
