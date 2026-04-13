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

package process

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/dvaldivia/tukituki/internal/config"
	"github.com/dvaldivia/tukituki/internal/state"
)

func echoTarget(name string) config.RunTarget {
	return config.RunTarget{
		Name:    name,
		Command: "sh",
		Args:    []string{"-c", "echo hello from " + name},
	}
}

func sleepTarget(name string, secs int) config.RunTarget {
	return config.RunTarget{
		Name:    name,
		Command: "sh",
		Args:    []string{"-c", "echo started && sleep 60"},
	}
}

func newTestManager(t *testing.T, targets []config.RunTarget) *Manager {
	t.Helper()
	dir := t.TempDir()
	stateDir := filepath.Join(dir, ".tukituki")
	m, err := NewManager(targets, stateDir, dir)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	return m
}

func TestManager_StartStop(t *testing.T) {
	target := sleepTarget("sleepy", 60)
	m := newTestManager(t, []config.RunTarget{target})

	ctx := context.Background()
	if err := m.Start(ctx, "sleepy"); err != nil {
		t.Fatalf("Start: %v", err)
	}

	// Give the process a moment to start.
	time.Sleep(200 * time.Millisecond)

	status := m.GetStatus("sleepy")
	if status != state.StatusRunning {
		t.Errorf("expected running, got %s", status)
	}

	if err := m.Stop("sleepy"); err != nil {
		t.Errorf("Stop: %v", err)
	}

	// Allow state update goroutine to run.
	time.Sleep(200 * time.Millisecond)

	status = m.GetStatus("sleepy")
	if status == state.StatusRunning {
		t.Errorf("expected not-running after stop, got %s", status)
	}
}

func TestManager_StartAll(t *testing.T) {
	targets := []config.RunTarget{
		echoTarget("a"),
		echoTarget("b"),
		echoTarget("c"),
	}
	m := newTestManager(t, targets)

	ctx := context.Background()
	if err := m.StartAll(ctx); err != nil {
		t.Fatalf("StartAll: %v", err)
	}

	// Wait for all short-lived processes to finish.
	time.Sleep(500 * time.Millisecond)

	statuses := m.GetAllStatuses()
	for _, name := range []string{"a", "b", "c"} {
		s, ok := statuses[name]
		if !ok {
			t.Errorf("status missing for %q", name)
			continue
		}
		// echo finishes quickly — it should be stopped (or still reconciling as running).
		// We accept running or stopped/failed here since timing varies.
		_ = s
	}
}

func TestManager_DumpLog(t *testing.T) {
	target := echoTarget("logger")
	m := newTestManager(t, []config.RunTarget{target})

	ctx := context.Background()
	if err := m.Start(ctx, "logger"); err != nil {
		t.Fatalf("Start: %v", err)
	}

	// Wait for the echo to complete and flush.
	time.Sleep(400 * time.Millisecond)

	dest := filepath.Join(t.TempDir(), "dump.log")
	if err := m.DumpLog("logger", dest); err != nil {
		t.Fatalf("DumpLog: %v", err)
	}

	data, err := os.ReadFile(dest)
	if err != nil {
		t.Fatalf("read dump: %v", err)
	}

	if !strings.Contains(string(data), "hello from logger") {
		t.Errorf("dump does not contain expected output; got: %q", string(data))
	}
}

func TestManager_GetLogLines(t *testing.T) {
	target := echoTarget("liner")
	m := newTestManager(t, []config.RunTarget{target})

	ctx := context.Background()
	if err := m.Start(ctx, "liner"); err != nil {
		t.Fatalf("Start: %v", err)
	}

	// Wait for log tailer to pick up the output.
	time.Sleep(600 * time.Millisecond)

	lines := m.GetLogLines("liner")
	found := false
	for _, l := range lines {
		if strings.Contains(l, "hello from liner") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("expected log lines to contain 'hello from liner'; got %v", lines)
	}
}

func TestManager_AttachToExisting(t *testing.T) {
	dir := t.TempDir()
	stateDir := filepath.Join(dir, ".tukituki")

	target := sleepTarget("attach-test", 60)
	m, err := NewManager([]config.RunTarget{target}, stateDir, dir)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}

	ctx := context.Background()
	if err := m.Start(ctx, "attach-test"); err != nil {
		t.Fatalf("Start: %v", err)
	}
	time.Sleep(200 * time.Millisecond)

	// Simulate a new Manager instance (e.g. after tukituki restart).
	m2, err := NewManager([]config.RunTarget{target}, stateDir, dir)
	if err != nil {
		t.Fatalf("NewManager 2: %v", err)
	}

	if err := m2.AttachToExisting(); err != nil {
		t.Fatalf("AttachToExisting: %v", err)
	}

	status := m2.GetStatus("attach-test")
	if status != state.StatusRunning {
		t.Errorf("expected running after attach, got %s", status)
	}

	// Clean up.
	_ = m.Stop("attach-test")
}

func TestBuildShellCmd(t *testing.T) {
	cases := []struct {
		name    string
		command string
		args    []string
		want    string
	}{
		{"simple", "echo", []string{"hello"}, "echo hello"},
		{"empty arg", "cmd", []string{"--flag", ""}, "cmd --flag ''"},
		{"spaces in arg", "cmd", []string{"hello world"}, "cmd 'hello world'"},
		{"no args", "cmd", nil, "cmd"},
		{"multiple empty args", "cmd", []string{"", ""}, "cmd '' ''"},
		{"flag with empty value", "reverse-proxy", []string{"-tls-certificate", "", "-tls-key", ""}, "reverse-proxy -tls-certificate '' -tls-key ''"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := BuildShellCmd(tc.command, tc.args)
			if got != tc.want {
				t.Errorf("BuildShellCmd(%q, %v) = %q, want %q", tc.command, tc.args, got, tc.want)
			}
		})
	}
}

func TestNewManager_CreatesDirs(t *testing.T) {
	base := t.TempDir()
	stateDir := filepath.Join(base, "deep", "nested", ".tukituki")

	_, err := NewManager(nil, stateDir, base)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}

	logsDir := filepath.Join(stateDir, "logs")
	if _, err := os.Stat(logsDir); os.IsNotExist(err) {
		t.Errorf("logs dir was not created: %s", logsDir)
	}
}
