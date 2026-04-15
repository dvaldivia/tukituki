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
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/dvaldivia/tukituki/internal/config"
	"github.com/dvaldivia/tukituki/internal/state"
)

const (
	ringBufferSize = 1000
	tailPollDelay  = 100 * time.Millisecond
)

// OtelConfig holds the OpenTelemetry collector configuration.
type OtelConfig struct {
	Port     int
	Protocol string
	Severity string
}

// OtelTargetName is the fixed name used for the virtual OTel collector target.
const OtelTargetName = "otel-errors"

// Manager owns the lifecycle of all managed processes.
type Manager struct {
	targets     []config.RunTarget
	st          *state.State
	stateDir    string // .tukituki/ directory
	logsDir     string // .tukituki/logs/ directory
	projectRoot string // absolute path where tukituki was invoked (workdirs are relative to this)

	otelCfg *OtelConfig

	mu       sync.RWMutex
	logLines map[string][]string           // in-memory ring buffer of last 1000 lines per target
	watchers map[string][]chan string       // per-target subscriber channels
	watchCtx map[string]context.CancelFunc // cancel funcs for log-tail goroutines
}

// NewManager creates a Manager, ensures state/log directories exist, and loads
// existing state from disk. projectRoot is the directory from which workdir
// values in .run/*.yaml are resolved (typically the cwd at invocation time).
func NewManager(targets []config.RunTarget, stateDir string, projectRoot string) (*Manager, error) {
	logsDir := filepath.Join(stateDir, "logs")
	if err := os.MkdirAll(logsDir, 0o755); err != nil {
		return nil, fmt.Errorf("create logs dir: %w", err)
	}

	stateFile := filepath.Join(stateDir, "state.json")
	st := state.New(stateFile)

	m := &Manager{
		targets:     targets,
		st:          st,
		stateDir:    stateDir,
		logsDir:     logsDir,
		projectRoot: projectRoot,
		logLines:    make(map[string][]string),
		watchers:    make(map[string][]chan string),
		watchCtx:    make(map[string]context.CancelFunc),
	}

	return m, nil
}

// SetOtelConfig sets the OpenTelemetry collector configuration.
func (m *Manager) SetOtelConfig(cfg OtelConfig) {
	m.otelCfg = &cfg
}

// UpdateTargets replaces the target list so that subsequent Start/Restart
// calls use the latest configuration.
func (m *Manager) UpdateTargets(targets []config.RunTarget) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.targets = targets
}

// targetByName returns the RunTarget with the given name, or an error.
func (m *Manager) targetByName(name string) (config.RunTarget, error) {
	for _, t := range m.targets {
		if t.Name == name {
			return t, nil
		}
	}
	return config.RunTarget{}, fmt.Errorf("unknown target: %q", name)
}

// Start starts a specific target as a detached background process.
// If the process is already running, it returns nil without re-starting.
func (m *Manager) Start(ctx context.Context, name string) error {
	target, err := m.targetByName(name)
	if err != nil {
		return err
	}
	return m.StartTarget(ctx, target)
}

// StartTarget starts a RunTarget as a detached background process.
// Unlike Start, it does not look up the target by name, so it can be used
// for synthetic (virtual) targets that are not in the target list.
func (m *Manager) StartTarget(ctx context.Context, target config.RunTarget) error {
	if target.ParseError != "" {
		return fmt.Errorf("target %q has a config error: %s", target.Name, target.ParseError)
	}

	name := target.Name
	m.mu.Lock()
	defer m.mu.Unlock()

	// Check if already running.
	if ps, ok := m.st.Processes[name]; ok && ps.Status == state.StatusRunning {
		if state.IsAlive(ps) {
			return nil // already running
		}
	}

	// Truncate the log file on each (re)start so output is always fresh.
	logFile := filepath.Join(m.logsDir, name+".log")
	f, err := os.OpenFile(logFile, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o644)
	if err != nil {
		return fmt.Errorf("open log file: %w", err)
	}

	// Clear the in-memory log buffer so the TUI shows a fresh start.
	m.logLines[name] = nil

	// Use the user's login shell ($SHELL -l -c) so that shell-managed tools
	// (nvm, pyenv, rbenv, Homebrew, etc.) and their PATH additions are
	// available, exactly as they are in the user's interactive terminal.
	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "/bin/sh"
	}
	shellLine := BuildShellCmd(target.Command, target.Args)
	// We intentionally use exec.Command (not exec.CommandContext) so that
	// the spawned process is NOT killed when the TUI or CLI exits.
	cmd := exec.Command(shell, "-l", "-c", shellLine)
	cmd.Stdout = f
	cmd.Stderr = f
	cmd.SysProcAttr = &syscall.SysProcAttr{
		Setpgid: true, // detach from parent process group
	}

	// Resolve workdir relative to the project root (where tukituki was invoked).
	if target.Workdir != "" {
		if filepath.IsAbs(target.Workdir) {
			cmd.Dir = target.Workdir
		} else {
			cmd.Dir = filepath.Join(m.projectRoot, target.Workdir)
		}
	}

	// Always inherit the parent environment, then overlay target-specific vars.
	cmd.Env = os.Environ()
	for k, v := range target.Env {
		cmd.Env = append(cmd.Env, k+"="+v)
	}

	// Inject OpenTelemetry environment variables when OTel is enabled.
	if target.Otel && m.otelCfg != nil {
		endpoint := fmt.Sprintf("http://127.0.0.1:%d", m.otelCfg.Port)
		cmd.Env = append(cmd.Env, "OTEL_EXPORTER_OTLP_ENDPOINT="+endpoint)
	}

	if err := cmd.Start(); err != nil {
		f.Close()
		return fmt.Errorf("start process: %w", err)
	}

	ps := &state.ProcessState{
		Name:      name,
		PID:       cmd.Process.Pid,
		LogFile:   logFile,
		StartedAt: time.Now(),
		Status:    state.StatusRunning,
	}
	m.st.Processes[name] = ps

	if err := m.st.Save(); err != nil {
		// Non-fatal — process is running, we just couldn't persist state.
		fmt.Fprintf(os.Stderr, "warning: save state: %v\n", err)
	}

	// Goroutine that waits for the process to exit and updates state.
	go func() {
		f.Close() // the goroutine below opens its own handle for tailing
		err := cmd.Wait()

		code := 0
		var newStatus state.Status
		if err != nil {
			if exitErr, ok2 := err.(*exec.ExitError); ok2 {
				code = exitErr.ExitCode()
				newStatus = state.StatusFailed
			} else {
				newStatus = state.StatusStopped
			}
		} else {
			newStatus = state.StatusStopped
		}

		// Append exit message to the log file so the tailer picks it up
		// in order, after all process output.
		m.mu.RLock()
		logFilePath := ""
		if ps, ok := m.st.Processes[name]; ok {
			logFilePath = ps.LogFile
		}
		m.mu.RUnlock()

		if logFilePath != "" {
			if lf, openErr := os.OpenFile(logFilePath, os.O_APPEND|os.O_WRONLY, 0o644); openErr == nil {
				fmt.Fprintf(lf, "\n(Process exited at %s, exit code: %d)\n",
					time.Now().Format("2006-01-02 15:04:05"), code)
				lf.Close()
			}
		}

		m.mu.Lock()
		defer m.mu.Unlock()

		p, ok := m.st.Processes[name]
		if !ok {
			return
		}
		p.Status = newStatus
		p.ExitCode = &code
		_ = m.st.Save()
	}()

	// Start log file tailer.
	m.startLogTailer(name, logFile)

	return nil
}

// startLogTailer spawns a goroutine that polls logFile for new lines and
// appends them to the in-memory ring buffer and any subscriber channels.
// Must be called with m.mu held (write).
func (m *Manager) startLogTailer(name, logFile string) {
	// Cancel any existing tailer.
	if cancel, ok := m.watchCtx[name]; ok {
		cancel()
	}

	tailCtx, cancel := context.WithCancel(context.Background())
	m.watchCtx[name] = cancel

	go func() {
		var offset int64

		for {
			select {
			case <-tailCtx.Done():
				return
			case <-time.After(tailPollDelay):
			}

			f, err := os.Open(logFile)
			if err != nil {
				continue
			}

			fi, err := f.Stat()
			if err != nil {
				f.Close()
				continue
			}

			size := fi.Size()
			if size <= offset {
				f.Close()
				continue
			}

			if _, err := f.Seek(offset, io.SeekStart); err != nil {
				f.Close()
				continue
			}

			buf := make([]byte, size-offset)
			n, err := f.Read(buf)
			f.Close()
			if err != nil && err != io.EOF {
				continue
			}
			if n == 0 {
				continue
			}
			offset += int64(n)

			chunk := string(buf[:n])
			lines := strings.Split(chunk, "\n")
			// If the last element is empty (trailing newline), drop it.
			if len(lines) > 0 && lines[len(lines)-1] == "" {
				lines = lines[:len(lines)-1]
			}

			m.mu.Lock()
			for _, line := range lines {
				// Append to ring buffer.
				buf := m.logLines[name]
				buf = append(buf, line)
				if len(buf) > ringBufferSize {
					buf = buf[len(buf)-ringBufferSize:]
				}
				m.logLines[name] = buf

				// Broadcast to subscribers (non-blocking).
				for _, ch := range m.watchers[name] {
					select {
					case ch <- line:
					default:
					}
				}
			}
			m.mu.Unlock()
		}
	}()
}

// StartAll starts all targets that aren't already running.
// Targets with parse errors are silently skipped.
func (m *Manager) StartAll(ctx context.Context) error {
	for _, t := range m.targets {
		if t.ParseError != "" {
			continue
		}
		if err := m.Start(ctx, t.Name); err != nil {
			return fmt.Errorf("start %s: %w", t.Name, err)
		}
	}
	return nil
}

// Stop sends SIGTERM to the named process, waits up to 5 seconds, then SIGKILLs.
// After the process is gone, any Cleanup commands defined in the target's
// RunTarget are executed in sequence.
func (m *Manager) Stop(name string) error {
	m.mu.Lock()
	ps, ok := m.st.Processes[name]
	if !ok {
		m.mu.Unlock()
		return fmt.Errorf("no state for process %q", name)
	}
	pid := ps.PID
	m.mu.Unlock()

	proc, err := os.FindProcess(pid)
	if err != nil {
		return fmt.Errorf("find process %d: %w", pid, err)
	}

	// Cancel the log tailer for this process.
	m.mu.Lock()
	if cancel, ok := m.watchCtx[name]; ok {
		cancel()
		delete(m.watchCtx, name)
	}
	m.mu.Unlock()

	// Send SIGTERM.
	if err := proc.Signal(syscall.SIGTERM); err != nil {
		// Process may already be dead.
		if !isAlreadyDone(err) {
			return fmt.Errorf("SIGTERM to %d: %w", pid, err)
		}
		m.runCleanup(name)
		return nil
	}

	// Wait up to 5 seconds for the process to exit.
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		time.Sleep(100 * time.Millisecond)
		if !state.IsAlive(ps) {
			m.runCleanup(name)
			return nil
		}
	}

	// Force kill.
	if err := proc.Signal(syscall.SIGKILL); err != nil && !isAlreadyDone(err) {
		return fmt.Errorf("SIGKILL to %d: %w", pid, err)
	}

	m.mu.Lock()
	if p, ok2 := m.st.Processes[name]; ok2 {
		p.Status = state.StatusStopped
		_ = m.st.Save()
	}
	m.mu.Unlock()

	m.runCleanup(name)
	return nil
}

// runCleanup executes the Cleanup commands defined for the named target.
// Each command runs via the user's login shell; failures are logged but do not
// abort remaining cleanup steps.
func (m *Manager) runCleanup(name string) {
	target, err := m.targetByName(name)
	if err != nil || len(target.Cleanup) == 0 {
		return
	}

	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "/bin/sh"
	}

	var workdir string
	if target.Workdir != "" {
		if filepath.IsAbs(target.Workdir) {
			workdir = target.Workdir
		} else {
			workdir = filepath.Join(m.projectRoot, target.Workdir)
		}
	}

	for _, cmdStr := range target.Cleanup {
		cmd := exec.Command(shell, "-l", "-c", cmdStr)
		if workdir != "" {
			cmd.Dir = workdir
		}
		if out, err := cmd.CombinedOutput(); err != nil {
			fmt.Fprintf(os.Stderr, "cleanup %s: %q: %v\n%s\n", name, cmdStr, err, out)
		}
	}
}

// StopAll stops all running processes.
func (m *Manager) StopAll() error {
	for _, t := range m.targets {
		if err := m.Stop(t.Name); err != nil {
			// Log but continue stopping others.
			fmt.Fprintf(os.Stderr, "stop %s: %v\n", t.Name, err)
		}
	}
	return nil
}

// Restart stops then starts the named target.
func (m *Manager) Restart(ctx context.Context, name string) error {
	if err := m.Stop(name); err != nil {
		// If the process wasn't running that's fine — just start it.
		fmt.Fprintf(os.Stderr, "restart: stop %s: %v\n", name, err)
	}
	return m.Start(ctx, name)
}

// GetStatus returns the current status for a named process.
func (m *Manager) GetStatus(name string) state.Status {
	m.mu.RLock()
	defer m.mu.RUnlock()

	ps, ok := m.st.Processes[name]
	if !ok {
		return state.StatusUnknown
	}
	if ps.Status == state.StatusRunning && !state.IsAlive(ps) {
		return state.StatusStopped
	}
	return ps.Status
}

// GetAllProcessStates returns a snapshot of the raw process state for every
// managed process.  Callers must not mutate the returned values.
func (m *Manager) GetAllProcessStates() map[string]*state.ProcessState {
	m.mu.RLock()
	defer m.mu.RUnlock()

	out := make(map[string]*state.ProcessState, len(m.st.Processes))
	for name, ps := range m.st.Processes {
		out[name] = ps
	}
	return out
}

// GetAllStatuses returns a map of process name → current status.
func (m *Manager) GetAllStatuses() map[string]state.Status {
	m.mu.RLock()
	defer m.mu.RUnlock()

	out := make(map[string]state.Status, len(m.st.Processes))
	for name, ps := range m.st.Processes {
		if ps.Status == state.StatusRunning && !state.IsAlive(ps) {
			out[name] = state.StatusStopped
		} else {
			out[name] = ps.Status
		}
	}
	return out
}

// GetLogLines returns the in-memory ring-buffer log lines for a target.
func (m *Manager) GetLogLines(name string) []string {
	m.mu.RLock()
	defer m.mu.RUnlock()

	lines := m.logLines[name]
	if len(lines) == 0 {
		return nil
	}
	out := make([]string, len(lines))
	copy(out, lines)
	return out
}

// WatchLogLines returns a channel that receives new log lines for the named target.
// The caller should drain the channel; lines are dropped if the channel is full.
func (m *Manager) WatchLogLines(name string) <-chan string {
	ch := make(chan string, 256)

	m.mu.Lock()
	m.watchers[name] = append(m.watchers[name], ch)
	m.mu.Unlock()

	return ch
}

// ClearLog discards the in-memory ring buffer and truncates the on-disk log
// file for the named target.
func (m *Manager) ClearLog(name string) error {
	m.mu.Lock()
	m.logLines[name] = nil
	ps, hasPState := m.st.Processes[name]
	m.mu.Unlock()

	if hasPState && ps.LogFile != "" {
		if err := os.Truncate(ps.LogFile, 0); err != nil && !os.IsNotExist(err) {
			return fmt.Errorf("truncate log file: %w", err)
		}
	}
	return nil
}

// DumpLog copies the log file for the named target to dest.
func (m *Manager) DumpLog(name string, dest string) error {
	m.mu.RLock()
	ps, ok := m.st.Processes[name]
	m.mu.RUnlock()

	if !ok {
		return fmt.Errorf("no state for process %q", name)
	}

	src, err := os.Open(ps.LogFile)
	if err != nil {
		return fmt.Errorf("open log file: %w", err)
	}
	defer src.Close()

	dst, err := os.Create(dest)
	if err != nil {
		return fmt.Errorf("create dest file: %w", err)
	}
	defer dst.Close()

	if _, err := io.Copy(dst, src); err != nil {
		return fmt.Errorf("copy log: %w", err)
	}

	return nil
}

// AttachToExisting is called when processes were started by a previous
// tukituki invocation.  It reads the state file, starts log file watchers
// for still-running processes, and marks dead ones as stopped.
func (m *Manager) AttachToExisting() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	m.st.ReconcileAlive()

	for name, ps := range m.st.Processes {
		if ps.Status == state.StatusRunning {
			m.startLogTailer(name, ps.LogFile)
		}
	}

	return m.st.Save()
}

// EnsureOtelCollector starts the bundled OTel collector if any target has
// Otel enabled and the collector is not already running. It adds a virtual
// "otel-errors" target to the Manager's target list.
func (m *Manager) EnsureOtelCollector(ctx context.Context) error {
	if m.otelCfg == nil || !config.HasOtelTarget(m.targets) {
		return nil
	}

	target := config.RunTarget{
		Name:        OtelTargetName,
		Description: "OpenTelemetry error collector",
		Virtual:     true,
	}

	// Always add the virtual target to the list so the TUI shows it,
	// even when the collector is already running from a previous session.
	found := false
	for _, t := range m.targets {
		if t.Name == OtelTargetName {
			found = true
			break
		}
	}
	if !found {
		m.targets = append(m.targets, target)
	}

	// Already running (e.g. reattaching after detach)?
	m.mu.RLock()
	alive := false
	if ps, ok := m.st.Processes[OtelTargetName]; ok && ps.Status == state.StatusRunning && state.IsAlive(ps) {
		alive = true
	}
	m.mu.RUnlock()
	if alive {
		return nil
	}

	exe, err := os.Executable()
	if err != nil {
		return fmt.Errorf("resolve executable path: %w", err)
	}

	target.Command = exe
	target.Args = []string{"otel-collector", "--protocol", m.otelCfg.Protocol, "--severity", m.otelCfg.Severity, "--port", fmt.Sprintf("%d", m.otelCfg.Port)}

	return m.StartTarget(ctx, target)
}

// StopOtelCollector stops the OTel collector if it is running.
func (m *Manager) StopOtelCollector() error {
	m.mu.RLock()
	_, exists := m.st.Processes[OtelTargetName]
	m.mu.RUnlock()

	if !exists {
		return nil
	}
	return m.Stop(OtelTargetName)
}

// GetTargets returns the current target list (including virtual targets).
func (m *Manager) GetTargets() []config.RunTarget {
	m.mu.RLock()
	defer m.mu.RUnlock()
	out := make([]config.RunTarget, len(m.targets))
	copy(out, m.targets)
	return out
}

// BuildShellCmd builds a shell command string from a command and its arguments,
// properly escaping each argument for safe use with /bin/sh -c.
func BuildShellCmd(command string, args []string) string {
	parts := make([]string, 0, 1+len(args))
	parts = append(parts, shellEscape(command))
	for _, a := range args {
		parts = append(parts, shellEscape(a))
	}
	return strings.Join(parts, " ")
}

// shellEscape wraps a string in single quotes if it contains any characters
// that the shell would otherwise interpret specially.
func shellEscape(s string) string {
	if s == "" {
		return "''"
	}
	for _, r := range s {
		safe := (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') ||
			(r >= '0' && r <= '9') ||
			r == '-' || r == '_' || r == '.' || r == '/' ||
			r == ':' || r == '@' || r == '=' || r == ','
		if !safe {
			// Wrap in single quotes; escape internal single quotes.
			return "'" + strings.ReplaceAll(s, "'", "'\\''") + "'"
		}
	}
	return s
}

// isAlreadyDone reports whether err signals that the process is already gone.
func isAlreadyDone(err error) bool {
	if err == nil {
		return false
	}
	// os.ErrProcessDone or ESRCH
	return err == os.ErrProcessDone ||
		strings.Contains(err.Error(), "process already finished") ||
		strings.Contains(err.Error(), "no such process")
}
