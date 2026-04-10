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
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestStateSaveLoad(t *testing.T) {
	dir := t.TempDir()
	stateFile := filepath.Join(dir, "state.json")

	// Create and populate state.
	s := New(stateFile)
	code := 0
	s.Processes["web"] = &ProcessState{
		Name:      "web",
		PID:       12345,
		LogFile:   "/tmp/web.log",
		StartedAt: time.Now().Truncate(time.Second),
		Status:    StatusRunning,
		ExitCode:  &code,
	}

	if err := s.Save(); err != nil {
		t.Fatalf("Save: %v", err)
	}

	// File must exist after save.
	if _, err := os.Stat(stateFile); err != nil {
		t.Fatalf("state file missing after save: %v", err)
	}

	// Reload.
	s2 := New(stateFile)
	ps, ok := s2.Processes["web"]
	if !ok {
		t.Fatal("process 'web' missing after reload")
	}
	if ps.Name != "web" {
		t.Errorf("name: want web, got %s", ps.Name)
	}
	if ps.PID != 12345 {
		t.Errorf("PID: want 12345, got %d", ps.PID)
	}
	if ps.Status != StatusRunning {
		t.Errorf("status: want running, got %s", ps.Status)
	}
	if ps.ExitCode == nil || *ps.ExitCode != 0 {
		t.Errorf("exit_code: want 0, got %v", ps.ExitCode)
	}
}

func TestStateSaveLoad_CorruptFile(t *testing.T) {
	dir := t.TempDir()
	stateFile := filepath.Join(dir, "state.json")

	// Write garbage.
	if err := os.WriteFile(stateFile, []byte("not json {{{"), 0o644); err != nil {
		t.Fatal(err)
	}

	// New should return an empty state rather than error.
	s := New(stateFile)
	if len(s.Processes) != 0 {
		t.Errorf("expected empty processes on corrupt file, got %d", len(s.Processes))
	}
}

func TestIsAlive_RunningProcess(t *testing.T) {
	// Use the current process's PID — definitely alive.
	ps := &ProcessState{
		PID:    os.Getpid(),
		Status: StatusRunning,
	}
	if !IsAlive(ps) {
		t.Error("expected IsAlive to return true for current process")
	}
}

func TestIsAlive_DeadProcess(t *testing.T) {
	// Use PID 0 which is never a valid user process.
	ps := &ProcessState{
		PID:    0,
		Status: StatusRunning,
	}
	if IsAlive(ps) {
		t.Error("expected IsAlive to return false for PID 0")
	}
}

func TestIsAlive_NilProcess(t *testing.T) {
	if IsAlive(nil) {
		t.Error("expected IsAlive to return false for nil ProcessState")
	}
}

func TestReconcileAlive(t *testing.T) {
	dir := t.TempDir()
	stateFile := filepath.Join(dir, "state.json")

	s := New(stateFile)

	// Running but actually dead (PID 0).
	s.Processes["dead"] = &ProcessState{
		Name:   "dead",
		PID:    0,
		Status: StatusRunning,
	}
	// Running and actually alive (current PID).
	s.Processes["alive"] = &ProcessState{
		Name:   "alive",
		PID:    os.Getpid(),
		Status: StatusRunning,
	}
	// Already stopped — should remain stopped.
	s.Processes["stopped"] = &ProcessState{
		Name:   "stopped",
		PID:    0,
		Status: StatusStopped,
	}

	s.ReconcileAlive()

	if s.Processes["dead"].Status != StatusStopped {
		t.Errorf("dead: want stopped, got %s", s.Processes["dead"].Status)
	}
	if s.Processes["alive"].Status != StatusRunning {
		t.Errorf("alive: want running, got %s", s.Processes["alive"].Status)
	}
	if s.Processes["stopped"].Status != StatusStopped {
		t.Errorf("stopped: want stopped, got %s", s.Processes["stopped"].Status)
	}
}
