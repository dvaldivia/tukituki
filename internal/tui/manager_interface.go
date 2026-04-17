// Copyright 2026 Daniel Valdivia
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package tui

import (
	"context"

	"github.com/dvaldivia/tukituki/internal/config"
	"github.com/dvaldivia/tukituki/internal/state"
)

// ManagerInterface is the contract the TUI uses to interact with the process manager.
// The concrete implementation lives in internal/process and is injected at startup.
type ManagerInterface interface {
	// GetAllStatuses returns the current status of every managed target.
	GetAllStatuses() map[string]state.Status

	// GetLogLines returns the buffered log lines for the named target.
	GetLogLines(name string) []string

	// WatchLogLines returns a channel that emits new log lines for the named
	// target as they arrive. The channel is closed when the target stops or the
	// manager shuts down.
	WatchLogLines(name string) <-chan string

	// Start starts the named target. The context is used for cancellation.
	Start(ctx context.Context, name string) error

	// Stop gracefully stops the named target.
	Stop(name string) error

	// Restart stops and then starts the named target.
	Restart(ctx context.Context, name string) error

	// DumpLog writes the full log of the named target to dest (a file path).
	DumpLog(name string, dest string) error

	// ClearLog discards the in-memory log buffer and truncates the on-disk log
	// file for the named target.
	ClearLog(name string) error

	// StopAll gracefully stops every managed target.
	StopAll() error

	// UpdateTargets replaces the target list so that subsequent Start/Restart
	// calls use the latest configuration from disk.
	UpdateTargets(targets []config.RunTarget)

	// Describe returns a human-readable summary of how the named target is
	// (or would be) launched: command, workdir, injected environment, etc.
	Describe(name string) (string, error)
}
