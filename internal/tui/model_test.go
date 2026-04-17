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
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/dvaldivia/tukituki/internal/config"
	"github.com/dvaldivia/tukituki/internal/state"
)

// ─── Mock manager ─────────────────────────────────────────────────────────────

type mockManager struct {
	statuses map[string]state.Status
	logs     map[string][]string
	channels map[string]chan string
}

func newMockManager(targetNames []string) *mockManager {
	m := &mockManager{
		statuses: make(map[string]state.Status),
		logs:     make(map[string][]string),
		channels: make(map[string]chan string),
	}
	for _, name := range targetNames {
		m.statuses[name] = state.StatusRunning
		m.logs[name] = []string{"line 1", "line 2"}
		m.channels[name] = make(chan string, 8)
	}
	return m
}

func (m *mockManager) GetAllStatuses() map[string]state.Status {
	out := make(map[string]state.Status, len(m.statuses))
	for k, v := range m.statuses {
		out[k] = v
	}
	return out
}

func (m *mockManager) GetLogLines(name string) []string {
	return m.logs[name]
}

func (m *mockManager) WatchLogLines(name string) <-chan string {
	ch, ok := m.channels[name]
	if !ok {
		ch = make(chan string)
		m.channels[name] = ch
	}
	return ch
}

func (m *mockManager) Start(_ context.Context, _ string) error  { return nil }
func (m *mockManager) Stop(_ string) error                       { return nil }
func (m *mockManager) Restart(_ context.Context, _ string) error { return nil }
func (m *mockManager) DumpLog(_ string, _ string) error          { return nil }
func (m *mockManager) ClearLog(_ string) error                   { return nil }
func (m *mockManager) StopAll() error                            { return nil }
func (m *mockManager) UpdateTargets(_ []config.RunTarget)        {}
func (m *mockManager) Describe(_ string) (string, error)         { return "", nil }

// ─── Tests ────────────────────────────────────────────────────────────────────

func TestNewModel_InitializesCorrectly(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "backend", Command: "go", Args: []string{"run", "./cmd/..."}},
		{Name: "worker", Command: "go", Args: []string{"run", "./worker/..."}},
	}
	mgr := newMockManager([]string{"backend", "worker"})

	m := NewModel(targets, mgr, "", "")

	if len(m.targets) != 2 {
		t.Errorf("expected 2 targets, got %d", len(m.targets))
	}
	if m.selected != 0 {
		t.Errorf("expected selected=0, got %d", m.selected)
	}
	if m.logs["backend"] == nil {
		t.Error("expected log buffer for 'backend'")
	}
	if m.logs["worker"] == nil {
		t.Error("expected log buffer for 'worker'")
	}
	if m.manager == nil {
		t.Error("expected manager to be set")
	}
	if m.ctx == nil {
		t.Error("expected context to be set")
	}
	if m.cancel == nil {
		t.Error("expected cancel func to be set")
	}
}

func TestModel_Init_ReturnsCmds(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "alpha", Command: "true"},
	}
	mgr := newMockManager([]string{"alpha"})
	m := NewModel(targets, mgr, "", "")

	cmd := m.Init()
	if cmd == nil {
		t.Error("Init() should return a non-nil Cmd (status tick + log watchers)")
	}
}

func TestModel_WindowResize_UpdatesDimensions(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "svc", Command: "true"},
	}
	mgr := newMockManager([]string{"svc"})
	m := NewModel(targets, mgr, "", "")

	// Simulate a window size message.
	newModel, _ := m.Update(tea.WindowSizeMsg{Width: 120, Height: 40})
	updated := newModel.(Model)

	if updated.width != 120 {
		t.Errorf("expected width=120, got %d", updated.width)
	}
	if updated.height != 40 {
		t.Errorf("expected height=40, got %d", updated.height)
	}

	// Viewport should have been resized too.
	if updated.viewport.Width == 0 {
		t.Error("viewport width should not be 0 after resize")
	}
	if updated.viewport.Height == 0 {
		t.Error("viewport height should not be 0 after resize")
	}
}

func TestModel_LogLineMsg_AppendsToBuffer(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "svc", Command: "true"},
	}
	mgr := newMockManager([]string{"svc"})
	m := NewModel(targets, mgr, "", "")

	// Trigger resize so viewport has usable dimensions.
	m, _ = func() (Model, tea.Cmd) {
		nm, cmd := m.Update(tea.WindowSizeMsg{Width: 120, Height: 40})
		return nm.(Model), cmd
	}()

	// Send a log line message for the selected target.
	newModel, _ := m.Update(logLineMsg{target: "svc", line: "hello world"})
	updated := newModel.(Model)

	buf := updated.logs["svc"]
	if buf == nil {
		t.Fatal("expected log buffer for 'svc'")
	}
	found := false
	for _, l := range buf.lines {
		if l == "hello world" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("expected 'hello world' in log buffer, got: %v", buf.lines)
	}
}

func TestModel_StatusTick_RefreshesStatuses(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "svc", Command: "true"},
	}
	mgr := newMockManager([]string{"svc"})
	mgr.statuses["svc"] = state.StatusStopped

	m := NewModel(targets, mgr, "", "")

	// Simulate a status tick.
	newModel, _ := m.Update(statusTickMsg{})
	updated := newModel.(Model)

	if updated.statuses["svc"] != state.StatusStopped {
		t.Errorf("expected status=stopped, got %s", updated.statuses["svc"])
	}
}

func TestModel_QuitKey_SetsQuitting(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "svc", Command: "true"},
	}
	mgr := newMockManager([]string{"svc"})
	m := NewModel(targets, mgr, "", "")

	newModel, _ := m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("q")})
	updated := newModel.(Model)

	if !updated.quitting {
		t.Error("expected quitting=true after pressing q")
	}
	if updated.stopAll {
		t.Error("expected stopAll=false after pressing q (not Q)")
	}
}

func TestModel_QuitAllKey_SetsStopAll(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "svc", Command: "true"},
	}
	mgr := newMockManager([]string{"svc"})
	m := NewModel(targets, mgr, "", "")

	newModel, _ := m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("Q")})
	updated := newModel.(Model)

	if !updated.quitting {
		t.Error("expected quitting=true after pressing Q")
	}
	if !updated.stopAll {
		t.Error("expected stopAll=true after pressing Q")
	}
}

func TestModel_Navigation_ChangesSelection(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "alpha", Command: "true"},
		{Name: "beta", Command: "true"},
		{Name: "gamma", Command: "true"},
	}
	mgr := newMockManager([]string{"alpha", "beta", "gamma"})
	m := NewModel(targets, mgr, "", "")

	// Press down.
	newModel, _ := m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("j")})
	updated := newModel.(Model)
	if updated.selected != 1 {
		t.Errorf("expected selected=1 after pressing j, got %d", updated.selected)
	}

	// Press up.
	newModel, _ = updated.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("k")})
	updated = newModel.(Model)
	if updated.selected != 0 {
		t.Errorf("expected selected=0 after pressing k, got %d", updated.selected)
	}

	// Tab wraps around.
	m3 := updated
	m3.selected = len(targets) - 1
	newModel, _ = m3.Update(tea.KeyMsg{Type: tea.KeyTab})
	updated = newModel.(Model)
	if updated.selected != 0 {
		t.Errorf("expected tab to wrap to 0, got %d", updated.selected)
	}
}

func TestModel_View_DoesNotPanic(t *testing.T) {
	targets := []config.RunTarget{
		{Name: "svc", Command: "true"},
	}
	mgr := newMockManager([]string{"svc"})
	m := NewModel(targets, mgr, "", "")

	// Resize first so width/height are set.
	nm, _ := m.Update(tea.WindowSizeMsg{Width: 120, Height: 40})
	m = nm.(Model)

	// View() must not panic.
	defer func() {
		if r := recover(); r != nil {
			t.Errorf("View() panicked: %v", r)
		}
	}()
	_ = m.View()
}

func TestStatusIcon(t *testing.T) {
	cases := []struct {
		status state.Status
	}{
		{state.StatusRunning},
		{state.StatusStopped},
		{state.StatusFailed},
		{state.StatusUnknown},
		{"bogus"},
	}
	for _, tc := range cases {
		icon := statusIcon(tc.status)
		if icon == "" {
			t.Errorf("statusIcon(%q) returned empty string", tc.status)
		}
	}
}
