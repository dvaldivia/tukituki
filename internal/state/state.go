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

package state

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

// Status represents the lifecycle status of a managed process.
type Status string

const (
	StatusRunning Status = "running"
	StatusStopped Status = "stopped"
	StatusFailed  Status = "failed"
	StatusUnknown Status = "unknown"
)

// ProcessState holds runtime information for a single managed process.
type ProcessState struct {
	Name      string    `json:"name"`
	PID       int       `json:"pid"`
	LogFile   string    `json:"log_file"`
	StartedAt time.Time `json:"started_at"`
	Status    Status    `json:"status"`
	ExitCode  *int      `json:"exit_code,omitempty"`
}

// State is the top-level struct persisted to disk.
type State struct {
	Processes map[string]*ProcessState `json:"processes"`
	UpdatedAt time.Time                `json:"updated_at"`
	StateFile string                   `json:"-"` // path to this file, not serialized
}

// New loads state from stateFile if it exists; otherwise creates an empty State.
func New(stateFile string) *State {
	s := &State{
		Processes: make(map[string]*ProcessState),
		StateFile: stateFile,
	}

	data, err := os.ReadFile(stateFile)
	if err != nil {
		// File doesn't exist yet or unreadable — start fresh.
		return s
	}

	var loaded State
	if err := json.Unmarshal(data, &loaded); err != nil {
		// Corrupt file — start fresh.
		return s
	}

	if loaded.Processes == nil {
		loaded.Processes = make(map[string]*ProcessState)
	}
	loaded.StateFile = stateFile
	return &loaded
}

// Save atomically writes the State to its StateFile.
func (s *State) Save() error {
	s.UpdatedAt = time.Now()

	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal state: %w", err)
	}

	dir := filepath.Dir(s.StateFile)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("mkdir state dir: %w", err)
	}

	// Write to a temp file in the same directory so rename is atomic.
	tmp, err := os.CreateTemp(dir, ".state-*.json.tmp")
	if err != nil {
		return fmt.Errorf("create temp file: %w", err)
	}
	tmpName := tmp.Name()

	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmpName)
		return fmt.Errorf("write temp file: %w", err)
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmpName)
		return fmt.Errorf("close temp file: %w", err)
	}

	if err := os.Rename(tmpName, s.StateFile); err != nil {
		os.Remove(tmpName)
		return fmt.Errorf("rename temp file: %w", err)
	}

	return nil
}

// IsAlive returns true if the process represented by ps is still running.
func IsAlive(ps *ProcessState) bool {
	if ps == nil || ps.PID <= 0 {
		return false
	}

	proc, err := os.FindProcess(ps.PID)
	if err != nil {
		return false
	}

	// On Unix, FindProcess always succeeds; we need to send signal 0 to check
	// whether the process actually exists.
	err = proc.Signal(syscall.Signal(0))
	if err == nil {
		return true
	}
	if errors.Is(err, os.ErrProcessDone) || errors.Is(err, syscall.ESRCH) {
		return false
	}
	// EPERM means the process exists but is owned by another user.
	if errors.Is(err, syscall.EPERM) {
		return true
	}
	return false
}

// ReconcileAlive updates the Status field of every ProcessState by checking
// whether its PID is still alive.
func (s *State) ReconcileAlive() {
	for _, ps := range s.Processes {
		if ps.Status == StatusRunning {
			if !IsAlive(ps) {
				ps.Status = StatusStopped
			}
		}
	}
}
