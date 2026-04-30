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
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/key"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/dvaldivia/tukituki/internal/config"
	notifypb "github.com/dvaldivia/tukituki/internal/otel/notify"
	"github.com/dvaldivia/tukituki/internal/state"
	"github.com/fsnotify/fsnotify"
)

const tuiRingBufferSize = 10000

// otelTargetName mirrors process.OtelTargetName — the fixed name of the
// virtual otel-errors row. Hardcoded here to keep the TUI package free of a
// dependency on internal/process.
const otelTargetName = "otel-errors"

// otelBlinkInterval is how fast the unread-otel-errors row pulses on/off.
const otelBlinkInterval = 500 * time.Millisecond

// logBuffer holds the accumulated log lines for a single target.
// It acts as a ring buffer, dropping the oldest lines when the limit is reached.
type logBuffer struct {
	lines []string
}

// append adds a line and returns how many old lines were dropped.
func (b *logBuffer) append(line string) int {
	b.lines = append(b.lines, line)
	if len(b.lines) > tuiRingBufferSize {
		dropped := len(b.lines) - tuiRingBufferSize
		b.lines = b.lines[dropped:]
		return dropped
	}
	return 0
}

func (b *logBuffer) content() string {
	return strings.Join(b.lines, "\n")
}

// ─── Msg types ───────────────────────────────────────────────────────────────

// logLineMsg carries a single new log line from a watched target.
type logLineMsg struct {
	target string
	line   string
}

// statusTickMsg is sent on each periodic status refresh tick.
type statusTickMsg time.Time

// actionResultMsg carries the outcome of an async action (start/stop/restart).
type actionResultMsg struct {
	msg string
}

// quitMsg signals that the user wants to quit.
type quitMsg struct{ stopAll bool }

// clearLogMsg is sent after ClearLog completes so the TUI can flush its buffer.
type clearLogMsg struct{ target string }

// fileChangeMsg signals that one or more run-definition files changed on disk.
type fileChangeMsg struct{}

// targetsReloadedMsg carries the result of re-parsing the run directory.
type targetsReloadedMsg struct {
	targets []config.RunTarget
	err     error
}

// editorFinishedMsg is sent when the external editor exits.
type editorFinishedMsg struct{ err error }

// otelErrorMsg carries a single error event pushed by the otel-collector
// over the notify socket. Triggers the unread-count + blink behaviour on
// the otel-errors row.
type otelErrorMsg struct{ ev *notifypb.ErrorEvent }

// otelBlinkMsg toggles the otel-errors row's blink phase. Re-armed only
// while there is at least one unread error.
type otelBlinkMsg struct{}

// ─── Commands ────────────────────────────────────────────────────────────────

// waitForLogLine blocks on the channel and returns a logLineMsg when a line
// arrives. It is re-queued after every receipt so we keep listening.
func waitForLogLine(ch <-chan string, target string) tea.Cmd {
	return func() tea.Msg {
		line, ok := <-ch
		if !ok {
			return nil
		}
		return logLineMsg{target: target, line: line}
	}
}

// statusTick returns a command that fires a statusTickMsg every second.
func statusTick() tea.Cmd {
	return tea.Tick(time.Second, func(t time.Time) tea.Msg {
		return statusTickMsg(t)
	})
}

// waitForFileChange blocks until a .yaml/.yml file change is detected,
// debounces for 200ms, then returns a fileChangeMsg.
func waitForFileChange(watcher *fsnotify.Watcher) tea.Cmd {
	return func() tea.Msg {
		for {
			select {
			case ev, ok := <-watcher.Events:
				if !ok {
					return nil
				}
				ext := filepath.Ext(ev.Name)
				if ext == ".yaml" || ext == ".yml" {
					// Debounce: wait for events to settle.
					timer := time.NewTimer(200 * time.Millisecond)
					for {
						select {
						case <-timer.C:
							return fileChangeMsg{}
						case _, ok := <-watcher.Events:
							if !ok {
								timer.Stop()
								return fileChangeMsg{}
							}
							timer.Reset(200 * time.Millisecond)
						case <-watcher.Errors:
						}
					}
				}
			case <-watcher.Errors:
			}
		}
	}
}

// waitForOtelEvent blocks on the events channel and returns an otelErrorMsg
// when the collector pushes one. It is re-queued after every receipt so we
// keep listening for the rest of the session.
func waitForOtelEvent(ch <-chan *notifypb.ErrorEvent) tea.Cmd {
	return func() tea.Msg {
		ev, ok := <-ch
		if !ok {
			return nil
		}
		return otelErrorMsg{ev: ev}
	}
}

// otelBlinkTick fires after otelBlinkInterval and produces an otelBlinkMsg.
func otelBlinkTick() tea.Cmd {
	return tea.Tick(otelBlinkInterval, func(time.Time) tea.Msg {
		return otelBlinkMsg{}
	})
}

// reloadTargets re-parses run files from disk and returns the result.
func reloadTargets(runDir, projectRoot string) tea.Cmd {
	return func() tea.Msg {
		targets, err := config.LoadTargets(runDir)
		if err != nil {
			return targetsReloadedMsg{err: err}
		}
		dotenv, _ := config.ParseDotEnv(filepath.Join(projectRoot, ".env"))
		targets = config.ExpandEnv(targets, dotenv)
		return targetsReloadedMsg{targets: targets}
	}
}

// ─── Model ───────────────────────────────────────────────────────────────────

// Model is the root bubbletea model for the tukituki TUI.
type Model struct {
	targets    []config.RunTarget
	manager    ManagerInterface
	selected   int // index into targets
	logs       map[string]*logBuffer
	viewport   viewport.Model
	width      int
	height     int
	logWatches map[string]<-chan string
	ctx        context.Context
	cancel     context.CancelFunc
	quitting   bool
	stopAll    bool
	statusMsg  string // transient message shown in the header
	statuses   map[string]state.Status
	keys       keyMap

	// atBottom tracks whether the viewport was at the bottom before the last
	// update, so we can decide to auto-scroll.
	atBottom bool

	// search state
	searchMode     bool
	searchQuery    string
	searchMatches  []int // line indices (in logBuffer) matching the query
	searchMatchIdx int   // index into searchMatches for the current match

	// helpMode shows the keybinding help overlay in the right panel.
	helpMode bool

	// describeMode shows the launch description overlay in the right panel.
	describeMode    bool
	describeContent string

	// mouseEnabled tracks whether bubbletea's mouse capture is on.
	// When false the terminal regains native text-selection behaviour.
	mouseEnabled bool

	// wrapLogs enables soft word-wrap in the log viewport.
	wrapLogs bool

	// zoomLogs hides the left panel so logs use the full terminal width.
	zoomLogs bool
	// mouseBeforeZoom remembers whether mouse was enabled before zoom,
	// so we can restore it when leaving zoom mode.
	mouseBeforeZoom bool

	// runDir and projectRoot are used to reload targets from disk.
	runDir      string
	projectRoot string

	// fsWatcher watches the run directory for file changes.
	fsWatcher *fsnotify.Watcher

	// otelEvents receives ErrorEvent pushes from the otel-collector over the
	// notify socket. Nil when no socket path was configured.
	otelEvents <-chan *notifypb.ErrorEvent

	// unreadOtelErrors is the count of error events that have arrived since
	// the user last had the otel-errors row selected. Reset to 0 when the
	// user selects the row.
	unreadOtelErrors int

	// otelBlinkOn toggles each blink tick while unreadOtelErrors > 0; used
	// by renderLeft to alternate the row's foreground colour.
	otelBlinkOn bool

	// otelBlinking is true while a blink-tick chain is in flight; prevents
	// stacking multiple ticker chains when more events arrive.
	otelBlinking bool
}

// NewModel constructs a Model ready for use with bubbletea.
func NewModel(targets []config.RunTarget, manager ManagerInterface, runDir, projectRoot string) Model {
	ctx, cancel := context.WithCancel(context.Background())

	logs := make(map[string]*logBuffer, len(targets))
	logWatches := make(map[string]<-chan string, len(targets))
	statuses := make(map[string]state.Status, len(targets))

	for _, t := range targets {
		logs[t.Name] = &logBuffer{}
		statuses[t.Name] = state.StatusUnknown
	}

	vp := viewport.New(80, 24) // placeholder; resized on WindowSizeMsg
	vp.SetContent("")

	// Watch the run directory for config file changes.
	var watcher *fsnotify.Watcher
	if runDir != "" {
		if w, err := fsnotify.NewWatcher(); err == nil {
			if err := w.Add(runDir); err == nil {
				watcher = w
			} else {
				w.Close()
			}
		}
	}

	// Subscribe to error notifications from the otel-collector. The dialer
	// retries forever so we are tolerant of the collector starting after
	// the TUI (e.g. detach/reattach scenarios).
	var otelEvents <-chan *notifypb.ErrorEvent
	if socket := manager.OtelNotifySocket(); socket != "" && hasOtelTarget(targets) {
		ch := make(chan *notifypb.ErrorEvent, 256)
		go runOtelNotifySubscriber(ctx, socket, ch)
		otelEvents = ch
	}

	return Model{
		targets:      targets,
		manager:      manager,
		selected:     0,
		logs:         logs,
		viewport:     vp,
		logWatches:   logWatches,
		ctx:          ctx,
		cancel:       cancel,
		statuses:     statuses,
		keys:         defaultKeyMap(),
		atBottom:     true,
		mouseEnabled: true,
		runDir:       runDir,
		projectRoot:  projectRoot,
		fsWatcher:    watcher,
		otelEvents:   otelEvents,
	}
}

// hasOtelTarget reports whether the target list includes the virtual
// otel-errors row — the only target whose notifications we surface.
func hasOtelTarget(targets []config.RunTarget) bool {
	for _, t := range targets {
		if t.Name == otelTargetName {
			return true
		}
	}
	return false
}

// ─── Init ────────────────────────────────────────────────────────────────────

func (m Model) Init() tea.Cmd {
	cmds := []tea.Cmd{
		statusTick(),
	}

	// Start watching the run directory for config file changes.
	if m.fsWatcher != nil {
		cmds = append(cmds, waitForFileChange(m.fsWatcher))
	}

	// Start consuming otel-collector error notifications.
	if m.otelEvents != nil {
		cmds = append(cmds, waitForOtelEvent(m.otelEvents))
	}

	// Start watching logs for each target and seed the buffer from existing lines.
	for _, t := range m.targets {
		if t.ParseError != "" {
			m.logs[t.Name].append(fmt.Sprintf("ERROR: failed to load config — %s", t.ParseError))
			continue
		}
		existing := m.manager.GetLogLines(t.Name)
		for _, line := range existing {
			m.logs[t.Name].append(line)
		}

		ch := m.manager.WatchLogLines(t.Name)
		m.logWatches[t.Name] = ch
		cmds = append(cmds, waitForLogLine(ch, t.Name))
	}

	// Seed statuses.
	for name, st := range m.manager.GetAllStatuses() {
		m.statuses[name] = st
	}

	// Load selected target's logs into the viewport.
	if len(m.targets) > 0 {
		m.viewport.SetContent(m.logs[m.targets[0].Name].content())
		m.viewport.GotoBottom()
	}

	return tea.Batch(cmds...)
}

// ─── Update ──────────────────────────────────────────────────────────────────

func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmds []tea.Cmd

	switch msg := msg.(type) {

	// ── Terminal resize ──────────────────────────────────────────────────────
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		m.resizeViewport()

	// ── Keyboard input ───────────────────────────────────────────────────────
	case tea.KeyMsg:
		if m.helpMode {
			// Any key dismisses the help overlay; ? toggles it off explicitly.
			m.helpMode = false
			break
		}
		if m.describeMode {
			// Any key dismisses the describe overlay.
			m.describeMode = false
			m.describeContent = ""
			break
		}
		if m.searchMode {
			switch {
			case msg.Type == tea.KeyEscape:
				m.searchMode = false
				m.searchQuery = ""
				m.searchMatches = nil
				m.searchMatchIdx = 0
				m.resizeViewport()
				m.refreshViewportContent()

			case msg.Type == tea.KeyEnter:
				m.nextSearchMatch()

			case msg.Type == tea.KeyRunes && string(msg.Runes) == "/":
				m.nextSearchMatch()

			case msg.Type == tea.KeyBackspace || msg.Type == tea.KeyCtrlH:
				if len(m.searchQuery) > 0 {
					runes := []rune(m.searchQuery)
					m.searchQuery = string(runes[:len(runes)-1])
					m.updateSearchMatches()
					m.refreshViewportContent()
				}

			case msg.Type == tea.KeyRunes:
				m.searchQuery += string(msg.Runes)
				m.updateSearchMatches()
				if len(m.searchMatches) > 0 {
					m.jumpToCurrentMatch()
				}
				m.refreshViewportContent()
			}
		} else {
			switch {
			case matchKey(msg, m.keys.KillAll):
				m.quitting = true
				m.stopAll = true
				m.cancel()
				if m.fsWatcher != nil {
					m.fsWatcher.Close()
				}
				return m, tea.Quit

			case matchKey(msg, m.keys.Detach):
				m.quitting = true
				m.cancel()
				if m.fsWatcher != nil {
					m.fsWatcher.Close()
				}
				return m, tea.Quit

			case matchKey(msg, m.keys.Up):
				if m.selected > 0 {
					m.selected--
					m.loadSelectedLogs()
					m.markOtelSeenIfSelected()
				}

			case matchKey(msg, m.keys.Down):
				if m.selected < len(m.targets)-1 {
					m.selected++
					m.loadSelectedLogs()
					m.markOtelSeenIfSelected()
				}

			case matchKey(msg, m.keys.Tab):
				if len(m.targets) > 0 {
					m.selected = (m.selected + 1) % len(m.targets)
					m.loadSelectedLogs()
					m.markOtelSeenIfSelected()
				}

			case matchKey(msg, m.keys.Restart):
				if len(m.targets) > 0 {
					name := m.targets[m.selected].Name
					cmds = append(cmds, m.doRestart(name))
				}

			case matchKey(msg, m.keys.Stop):
				if len(m.targets) > 0 {
					name := m.targets[m.selected].Name
					cmds = append(cmds, m.doStop(name))
				}

			case matchKey(msg, m.keys.Start):
				if len(m.targets) > 0 {
					name := m.targets[m.selected].Name
					cmds = append(cmds, m.doStart(name))
				}

			case matchKey(msg, m.keys.Dump):
				if len(m.targets) > 0 {
					name := m.targets[m.selected].Name
					cmds = append(cmds, m.doDump(name))
				}

			case matchKey(msg, m.keys.Clear):
				if len(m.targets) > 0 {
					name := m.targets[m.selected].Name
					cmds = append(cmds, m.doClear(name))
				}

			case matchKey(msg, m.keys.Search):
				m.searchMode = true
				m.searchQuery = ""
				m.searchMatches = nil
				m.searchMatchIdx = 0
				m.resizeViewport()

			case matchKey(msg, m.keys.Help):
				m.helpMode = true

			case matchKey(msg, m.keys.Describe):
				if len(m.targets) > 0 {
					name := m.targets[m.selected].Name
					content, err := m.manager.Describe(name)
					if err != nil {
						m.statusMsg = fmt.Sprintf("describe %s: %s", name, err)
					} else {
						m.describeMode = true
						m.describeContent = content
					}
				}

			case matchKey(msg, m.keys.ToggleMouse):
				if m.mouseEnabled {
					m.mouseEnabled = false
					m.statusMsg = "mouse off – select text freely (M to re-enable)"
					cmds = append(cmds, tea.DisableMouse)
				} else {
					m.mouseEnabled = true
					m.statusMsg = "mouse on"
					cmds = append(cmds, tea.EnableMouseCellMotion)
				}

			case matchKey(msg, m.keys.ToggleWrap):
				m.wrapLogs = !m.wrapLogs
				m.refreshViewportContent()

			case matchKey(msg, m.keys.ZoomLogs):
				m.zoomLogs = !m.zoomLogs
				if m.zoomLogs {
					// Remember mouse state so we can restore it on unzoom.
					m.mouseBeforeZoom = m.mouseEnabled
					if m.mouseEnabled {
						m.mouseEnabled = false
						cmds = append(cmds, tea.DisableMouse)
					}
				} else if m.mouseBeforeZoom {
					m.mouseEnabled = true
					cmds = append(cmds, tea.EnableMouseCellMotion)
				}
				m.resizeViewport()
				m.refreshViewportContent()

			case matchKey(msg, m.keys.EditFile):
				if len(m.targets) > 0 {
					t := m.targets[m.selected]
					if t.SourceFile != "" {
						editor := os.Getenv("EDITOR")
						if editor == "" {
							editor = "vim"
						}
						c := exec.Command(editor, t.SourceFile)
						return m, tea.ExecProcess(c, func(err error) tea.Msg {
							return editorFinishedMsg{err: err}
						})
					}
					m.statusMsg = "no source file for this target"
				}

			default:
				// Forward scroll keys to the viewport.
				wasAtBottom := m.viewport.AtBottom()
				var vpCmd tea.Cmd
				m.viewport, vpCmd = m.viewport.Update(msg)
				if vpCmd != nil {
					cmds = append(cmds, vpCmd)
				}
				// If the user scrolled back to the bottom, refresh viewport
				// content so any lines that arrived while scrolled up are shown.
				if !wasAtBottom && m.viewport.AtBottom() {
					m.refreshViewportContent()
				}
			}
		}

	// ── Mouse scroll ─────────────────────────────────────────────────────────
	case tea.MouseMsg:
		wasAtBottom := m.viewport.AtBottom()
		var vpCmd tea.Cmd
		m.viewport, vpCmd = m.viewport.Update(msg)
		if vpCmd != nil {
			cmds = append(cmds, vpCmd)
		}
		// If the user scrolled back to the bottom, refresh viewport content.
		if !wasAtBottom && m.viewport.AtBottom() {
			m.refreshViewportContent()
		}

	// ── Log line arrived ─────────────────────────────────────────────────────
	case logLineMsg:
		buf, ok := m.logs[msg.target]
		if ok {
			dropped := buf.append(msg.line)
			// If this is the currently selected target, update viewport.
			if len(m.targets) > 0 && m.targets[m.selected].Name == msg.target {
				// Adjust search match indices when old lines were dropped.
				if dropped > 0 && len(m.searchMatches) > 0 {
					adjusted := m.searchMatches[:0]
					for _, idx := range m.searchMatches {
						if newIdx := idx - dropped; newIdx >= 0 {
							adjusted = append(adjusted, newIdx)
						}
					}
					m.searchMatches = adjusted
					if m.searchMatchIdx >= len(m.searchMatches) {
						m.searchMatchIdx = 0
					}
				}
				wasAtBottom := m.viewport.AtBottom()
				// Track new matching line when search is active.
				if m.searchMode && m.searchQuery != "" {
					if strings.Contains(strings.ToLower(msg.line), strings.ToLower(m.searchQuery)) {
						m.searchMatches = append(m.searchMatches, len(buf.lines)-1)
					}
				}
				// Only rebuild viewport content when auto-scrolling (at bottom)
				// or when search is active (highlights need to be up to date).
				// When scrolled up, we skip the expensive rebuild and defer it
				// until the user returns to the bottom or switches targets.
				if wasAtBottom || m.searchMode {
					m.viewport.SetContent(m.viewportContent(buf))
					if wasAtBottom {
						m.viewport.GotoBottom()
					}
				}
			}
		}
		// Re-queue so we keep receiving from this target's channel.
		if ch, ok := m.logWatches[msg.target]; ok {
			cmds = append(cmds, waitForLogLine(ch, msg.target))
		}

	// ── Periodic status refresh ──────────────────────────────────────────────
	case statusTickMsg:
		for name, st := range m.manager.GetAllStatuses() {
			m.statuses[name] = st
		}
		cmds = append(cmds, statusTick())

	// ── Log cleared ──────────────────────────────────────────────────────────
	case clearLogMsg:
		if buf, ok := m.logs[msg.target]; ok {
			buf.lines = nil
		}
		if len(m.targets) > 0 && m.targets[m.selected].Name == msg.target {
			m.viewport.SetContent("")
			if m.searchMode {
				m.searchMatches = nil
				m.searchMatchIdx = 0
			}
		}

	// ── Action result (start/stop/restart/dump) ──────────────────────────────
	case actionResultMsg:
		m.statusMsg = msg.msg
		// Clear the status message after 3 seconds.
		cmds = append(cmds, tea.Tick(3*time.Second, func(time.Time) tea.Msg {
			return actionResultMsg{msg: ""}
		}))

	// ── Editor exited ───────────────────────────────────────────────────────
	case editorFinishedMsg:
		if msg.err != nil {
			m.statusMsg = fmt.Sprintf("editor error: %s", msg.err)
		} else {
			m.statusMsg = "editor closed"
		}
		cmds = append(cmds, tea.Tick(3*time.Second, func(time.Time) tea.Msg {
			return actionResultMsg{msg: ""}
		}))

	// ── Run-file changed on disk ────────────────────────────────────────────
	case fileChangeMsg:
		cmds = append(cmds, reloadTargets(m.runDir, m.projectRoot))
		if m.fsWatcher != nil {
			cmds = append(cmds, waitForFileChange(m.fsWatcher))
		}

	// ── OTel error event arrived ────────────────────────────────────────────
	case otelErrorMsg:
		if !m.isOtelSelected() {
			m.unreadOtelErrors++
			if !m.otelBlinking {
				m.otelBlinking = true
				m.otelBlinkOn = true
				cmds = append(cmds, otelBlinkTick())
			}
		}
		// Re-queue so we keep receiving from the channel.
		if m.otelEvents != nil {
			cmds = append(cmds, waitForOtelEvent(m.otelEvents))
		}

	// ── Blink tick ──────────────────────────────────────────────────────────
	case otelBlinkMsg:
		if m.unreadOtelErrors > 0 && !m.isOtelSelected() {
			m.otelBlinkOn = !m.otelBlinkOn
			cmds = append(cmds, otelBlinkTick())
		} else {
			m.otelBlinking = false
			m.otelBlinkOn = false
		}

	// ── Targets reloaded from disk ──────────────────────────────────────────
	case targetsReloadedMsg:
		if msg.err != nil {
			m.statusMsg = fmt.Sprintf("reload error: %s", msg.err)
		} else {
			reloadCmds := m.applyNewTargets(msg.targets)
			cmds = append(cmds, reloadCmds...)
			m.statusMsg = "config reloaded"
		}
		cmds = append(cmds, tea.Tick(3*time.Second, func(time.Time) tea.Msg {
			return actionResultMsg{msg: ""}
		}))
	}

	return m, tea.Batch(cmds...)
}

// ─── View ────────────────────────────────────────────────────────────────────

func (m Model) View() string {
	if m.quitting {
		return ""
	}
	// Don't render until the terminal size is known.
	if m.width == 0 || m.height == 0 {
		return "starting…"
	}

	header := m.renderHeader()
	right := m.renderRight()

	var body string
	if m.zoomLogs {
		body = right
	} else {
		left := m.renderLeft()
		sep := m.renderSeparator()
		body = lipgloss.JoinHorizontal(lipgloss.Top, left, sep, right)
	}
	parts := []string{header}
	if m.searchMode {
		parts = append(parts, m.renderSearchBar())
	}
	parts = append(parts, body)
	return lipgloss.JoinVertical(lipgloss.Left, parts...)
}

// renderSeparator renders a 1-char wide vertical divider between the two panels.
func (m Model) renderSeparator() string {
	h := m.height - 1 - m.searchBarHeight() // subtract header + search bar
	if h < 1 {
		h = 1
	}
	lines := make([]string, h)
	for i := range lines {
		lines[i] = "│"
	}
	return separatorStyle.Render(strings.Join(lines, "\n"))
}

// searchBarHeight returns 1 when the search bar is visible, 0 otherwise.
func (m Model) searchBarHeight() int {
	if m.searchMode {
		return 1
	}
	return 0
}

// ─── Rendering helpers ───────────────────────────────────────────────────────

func (m Model) renderHeader() string {
	// Build the header as a single plain string of exactly m.width visible chars,
	// then apply one style.Render() — no JoinHorizontal, no nested padding.
	// This guarantees the header is always 1 row and never overflows the terminal.
	left := " tukituki"
	if m.statusMsg != "" {
		left += "  " + m.statusMsg
	}
	right := "?=help  q=detach  Q/^C=stop all "

	gap := m.width - len(left) - len(right)
	if gap < 1 {
		gap = 1
	}
	line := left + strings.Repeat(" ", gap) + right
	// Hard-clamp to m.width so a very narrow terminal can't wrap us.
	if len(line) > m.width {
		line = line[:m.width]
	}

	return lipgloss.NewStyle().
		Background(colorHeader).
		Foreground(lipgloss.Color("#ECEFF1")).
		Bold(true).
		Render(line)
}

func (m Model) renderLeft() string {
	// Available inner height: total height minus header (1), search bar, minus panel borders (2).
	innerHeight := m.height - 1 - m.searchBarHeight() - 2
	if innerHeight < 1 {
		innerHeight = 1
	}

	// Inner width: leftPanelWidth minus border (1 each side). No horizontal
	// padding on the container — rows are pre-padded to this exact width so
	// the selected-row highlight fills edge-to-edge and no lipgloss Width()
	// constraint triggers word-wrap.
	innerWidth := leftPanelWidth - 2
	if innerWidth < 4 {
		innerWidth = 4
	}

	// fitLine pads or truncates s to exactly innerWidth *visible* columns.
	// It measures with lipgloss.Width (which strips ANSI codes) so pre-coloured
	// strings are handled correctly without double-counting escape bytes.
	fitLine := func(s string) string {
		w := lipgloss.Width(s)
		switch {
		case w < innerWidth:
			return s + strings.Repeat(" ", innerWidth-w)
		case w > innerWidth:
			// Trim runes until we fit — handles multi-byte and wide glyphs.
			rs := []rune(s)
			for lipgloss.Width(string(rs)) > innerWidth && len(rs) > 0 {
				rs = rs[:len(rs)-1]
			}
			return string(rs)
		}
		return s
	}

	// ── Target list ──────────────────────────────────────────────────────────
	// Build each row as a plain string (icon rendered, text plain), then apply
	// the row style ONCE — this avoids nested ANSI that confuses lipgloss's
	// width measurement and causes spurious line-wraps inside the container.
	virtualSepStyle := lipgloss.NewStyle().Foreground(colorDim).Italic(true)
	var rows []string
	virtualSepDone := false
	for i, t := range m.targets {
		// Render a thin separator before the first virtual target.
		if t.Virtual && !virtualSepDone {
			rows = append(rows, virtualSepStyle.Render(fitLine("  ─ collectors ─")))
			virtualSepDone = true
		}
		iconStr := statusIconChar(m.statuses[t.Name]) // raw char, no ANSI
		label := t.Name
		if t.Name == otelTargetName && m.unreadOtelErrors > 0 {
			label = fmt.Sprintf("%s (%d)", label, m.unreadOtelErrors)
		}
		// Reserve: cursor(2) + icon(1) + space(1) = 4 chars of prefix.
		maxLabel := innerWidth - 4
		if maxLabel < 1 {
			maxLabel = 1
		}
		if len(label) > maxLabel {
			label = label[:maxLabel]
		}
		switch {
		case i == m.selected:
			raw := fitLine("▶ " + iconStr + " " + label)
			rows = append(rows, selectedItemStyle.Render(raw))
		case t.Name == otelTargetName && m.unreadOtelErrors > 0:
			raw := fitLine("  " + iconStr + " " + label)
			style := otelAlertStyle
			if !m.otelBlinkOn {
				style = otelAlertOffStyle
			}
			rows = append(rows, style.Render(raw))
		default:
			raw := fitLine("  " + iconStr + " " + label)
			rows = append(rows, normalItemStyle.Render(raw))
		}
	}

	// ── Blank fill + separator + hints ───────────────────────────────────────
	rawHints := []string{
		"r restart  s stop",
		"S start    d dump",
		"c clear    E edit",
		"q detach   Q/^C stop",
	}
	// Total fixed lines at the bottom: separator(1) + hints.
	fixedLines := 1 + len(rawHints)
	blanks := innerHeight - len(rows) - fixedLines
	if blanks < 0 {
		blanks = 0
	}

	// Assemble all rows as plain strings (no nested styling).
	allRows := make([]string, 0, innerHeight)
	allRows = append(allRows, rows...)
	for range blanks {
		allRows = append(allRows, strings.Repeat(" ", innerWidth))
	}
	allRows = append(allRows, strings.Repeat("─", innerWidth)) // separator
	for _, h := range rawHints {
		allRows = append(allRows, fitLine(h)) // plain text, styled by outer container
	}
	// Clip to exactly innerHeight lines so the panel never exceeds terminal height.
	if len(allRows) > innerHeight {
		allRows = allRows[:innerHeight]
	}
	for len(allRows) < innerHeight {
		allRows = append(allRows, strings.Repeat(" ", innerWidth))
	}

	// No Width() constraint — avoids lipgloss word-wrap at (Width - 2*padding).
	// Content is already pre-padded to innerWidth rows; the border-only style
	// wraps it cleanly without any word-wrap interference.
	return leftPanelStyle.Render(strings.Join(allRows, "\n"))
}

func (m Model) renderRight() string {
	panelWidth := m.rightPanelOuterWidth()
	panelHeight := m.height - 1 - m.searchBarHeight() - 2 // total - header - search bar - borders

	if m.helpMode {
		return rightPanelStyle.
			Width(panelWidth).
			Height(panelHeight).
			Render(m.renderHelp())
	}

	if m.describeMode {
		return rightPanelStyle.
			Width(panelWidth).
			Height(panelHeight).
			Render(m.renderDescribe())
	}

	// In zoom mode, render just the viewport with no chrome so text
	// can be selected freely.
	if m.zoomLogs {
		return m.viewport.View()
	}

	// Panel title.
	targetName := "(none)"
	if len(m.targets) > 0 {
		targetName = m.targets[m.selected].Name
	}
	title := rightPanelTitleStyle.Render(targetName)
	sep := strings.Repeat("─", m.rightPanelInnerWidth())

	titleBlock := lipgloss.JoinVertical(lipgloss.Left, title, sep)
	vpView := m.viewport.View()

	content := lipgloss.JoinVertical(lipgloss.Left, titleBlock, vpView)

	return rightPanelStyle.
		Width(panelWidth).
		Height(panelHeight).
		Render(content)
}

// renderDescribe returns the launch description shown when describeMode
// is active: shell invocation, workdir, injected env vars, etc.
func (m Model) renderDescribe() string {
	innerWidth := m.rightPanelInnerWidth()
	titleStyle := lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("#ECEFF1")).Padding(0, 1)
	dimStyle := lipgloss.NewStyle().Foreground(colorDim).Italic(true)

	name := "(none)"
	if len(m.targets) > 0 {
		name = m.targets[m.selected].Name
	}
	title := titleStyle.Render("Launch details: " + name)
	divider := strings.Repeat("─", innerWidth)
	hint := dimStyle.Render("(press any key to dismiss)")

	return lipgloss.JoinVertical(
		lipgloss.Left,
		title,
		divider,
		m.describeContent,
		hint,
	)
}

// renderHelp returns the keybinding reference shown when helpMode is active.
func (m Model) renderHelp() string {
	innerWidth := m.rightPanelInnerWidth()

	titleStyle := lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("#ECEFF1")).Padding(0, 1)
	keyStyle := lipgloss.NewStyle().Foreground(colorBlue).Bold(true)
	descStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#B0BEC5"))
	dimStyle := lipgloss.NewStyle().Foreground(colorDim).Italic(true)

	title := titleStyle.Render("Keyboard shortcuts")
	divider := strings.Repeat("─", innerWidth)

	type entry struct{ keys, desc string }
	sections := []struct {
		heading string
		entries []entry
	}{
		{
			"Navigation",
			[]entry{
				{"↑ / k", "move up"},
				{"↓ / j", "move down"},
				{"tab", "next target (wrap)"},
			},
		},
		{
			"Process control",
			[]entry{
				{"r", "restart selected"},
				{"s", "stop selected"},
				{"S", "start selected"},
				{"E", "edit run file"},
				{"D", "describe launch (env, cmd, workdir)"},
			},
		},
		{
			"Logs",
			[]entry{
				{"pgup / b", "page up"},
				{"pgdn / f", "page down"},
				{"/", "search logs"},
				{"d", "dump logs to file"},
				{"c", "clear logs"},
				{"M", "toggle mouse (for text select)"},
				{"w", "toggle line wrap"},
				{"z", "zoom logs (full width)"},
			},
		},
		{
			"Application",
			[]entry{
				{"q", "detach (keep processes)"},
				{"Q / ^C", "stop all & exit"},
				{"?", "toggle this help"},
			},
		},
	}

	var lines []string
	lines = append(lines, title, divider, "")

	for _, sec := range sections {
		lines = append(lines, dimStyle.Render(" "+sec.heading))
		for _, e := range sec.entries {
			keyCol := keyStyle.Render(fmt.Sprintf("  %-12s", e.keys))
			descCol := descStyle.Render(e.desc)
			lines = append(lines, keyCol+descCol)
		}
		lines = append(lines, "")
	}

	lines = append(lines, dimStyle.Render(" press any key to dismiss"))

	return strings.Join(lines, "\n")
}

// ─── Layout math ─────────────────────────────────────────────────────────────

// rightPanelOuterWidth returns the content width passed to lipgloss Width() for
// the right panel. Total rendered = this + 2 (left+right border chars).
// Layout: leftPanelWidth(22) + separator(1) + rightBorders(2) + rightContent = m.width
func (m Model) rightPanelOuterWidth() int {
	var w int
	if m.zoomLogs {
		w = m.width - 2 // just borders
	} else {
		const separatorWidth = 1
		w = m.width - leftPanelWidth - separatorWidth - 2
	}
	if w < 10 {
		w = 10
	}
	return w
}

// rightPanelInnerWidth returns the usable width inside the right panel.
func (m Model) rightPanelInnerWidth() int {
	// outer - 2 border chars - 0 padding (rightPanelStyle has none).
	w := m.rightPanelOuterWidth() - 2
	if w < 8 {
		w = 8
	}
	return w
}

// resizeViewport recalculates viewport dimensions after a terminal resize.
func (m *Model) resizeViewport() {
	var vpHeight, vpWidth int
	if m.zoomLogs {
		// No borders, no title — just header and search bar.
		vpHeight = m.height - 1 - m.searchBarHeight()
		vpWidth = m.width
	} else {
		// Height: total - header (1) - search bar - right panel borders (2) - title line (1) - separator (1).
		vpHeight = m.height - 1 - m.searchBarHeight() - 2 - 1 - 1
		vpWidth = m.rightPanelInnerWidth()
	}
	if vpHeight < 1 {
		vpHeight = 1
	}

	m.viewport.Width = vpWidth
	m.viewport.Height = vpHeight
}

// loadSelectedLogs replaces the viewport content with the selected target's
// accumulated log buffer and scrolls to the bottom.
func (m *Model) loadSelectedLogs() {
	if len(m.targets) == 0 {
		return
	}
	name := m.targets[m.selected].Name
	buf, ok := m.logs[name]
	if !ok {
		m.viewport.SetContent("")
		return
	}
	if m.searchMode {
		m.updateSearchMatches()
	}
	m.viewport.SetContent(m.viewportContent(buf))
	m.viewport.GotoBottom()
	m.atBottom = true
}

// applyNewTargets reconciles the in-memory target list with a freshly loaded
// set from disk. It sets up log buffers / watches for added targets, cleans up
// removed targets, and updates the manager's target list.
func (m *Model) applyNewTargets(newTargets []config.RunTarget) []tea.Cmd {
	oldByName := make(map[string]config.RunTarget, len(m.targets))
	for _, t := range m.targets {
		oldByName[t.Name] = t
	}
	newNames := make(map[string]bool, len(newTargets))
	for _, t := range newTargets {
		newNames[t.Name] = true
	}

	var cmds []tea.Cmd

	// Set up log buffers and watches for added targets, and handle
	// transitions between errored ↔ valid for existing targets.
	for _, t := range newTargets {
		old, existed := oldByName[t.Name]
		if !existed {
			// Brand-new target.
			m.logs[t.Name] = &logBuffer{}
			m.statuses[t.Name] = state.StatusUnknown
			if t.ParseError != "" {
				m.logs[t.Name].append(fmt.Sprintf("ERROR: failed to load config — %s", t.ParseError))
			} else {
				ch := m.manager.WatchLogLines(t.Name)
				m.logWatches[t.Name] = ch
				cmds = append(cmds, waitForLogLine(ch, t.Name))
			}
		} else if old.ParseError != "" && t.ParseError == "" {
			// Was broken, now fixed — reset buffer and start watching.
			m.logs[t.Name] = &logBuffer{}
			ch := m.manager.WatchLogLines(t.Name)
			m.logWatches[t.Name] = ch
			cmds = append(cmds, waitForLogLine(ch, t.Name))
		} else if old.ParseError == "" && t.ParseError != "" {
			// Was valid, now broken — replace buffer with error.
			m.logs[t.Name] = &logBuffer{}
			m.logs[t.Name].append(fmt.Sprintf("ERROR: failed to load config — %s", t.ParseError))
			delete(m.logWatches, t.Name)
		} else if old.ParseError != "" && t.ParseError != "" {
			// Still broken — update the error message.
			m.logs[t.Name] = &logBuffer{}
			m.logs[t.Name].append(fmt.Sprintf("ERROR: failed to load config — %s", t.ParseError))
		}
	}

	// Clean up removed targets (but not virtual ones — they survive reloads).
	for _, t := range m.targets {
		if !newNames[t.Name] && !t.Virtual {
			delete(m.logs, t.Name)
			delete(m.statuses, t.Name)
			delete(m.logWatches, t.Name)
		}
	}

	// Preserve virtual targets (e.g. otel-errors) across config reloads.
	for _, t := range m.targets {
		if t.Virtual {
			newTargets = append(newTargets, t)
		}
	}

	m.targets = newTargets
	m.manager.UpdateTargets(newTargets)

	// Adjust selected index if out of bounds.
	if m.selected >= len(m.targets) {
		m.selected = len(m.targets) - 1
		if m.selected < 0 {
			m.selected = 0
		}
	}

	// Refresh the viewport for the current selection.
	if len(m.targets) > 0 {
		m.loadSelectedLogs()
	}

	return cmds
}

// isOtelSelected reports whether the currently selected target is the
// virtual otel-errors row.
func (m *Model) isOtelSelected() bool {
	if len(m.targets) == 0 {
		return false
	}
	return m.targets[m.selected].Name == otelTargetName
}

// markOtelSeenIfSelected zeros the unread count and stops the blink when
// the user has navigated onto the otel-errors row. Safe to call from any
// selection-changing key handler.
func (m *Model) markOtelSeenIfSelected() {
	if m.isOtelSelected() {
		m.unreadOtelErrors = 0
		m.otelBlinkOn = false
	}
}

// ─── Status icons ─────────────────────────────────────────────────────────────

// statusIcon returns a pre-ANSI-coloured icon string (for use outside lipgloss containers).
func statusIcon(s state.Status) string {
	switch s {
	case state.StatusRunning:
		return iconRunning
	case state.StatusStopped:
		return iconStopped
	case state.StatusFailed:
		return iconFailed
	default:
		return iconUnknown
	}
}

// statusIconChar returns a plain single ASCII character for the status,
// with no ANSI codes. Safe to embed inside a lipgloss Render() call without
// confusing the container's internal width measurement.
func statusIconChar(s state.Status) string {
	switch s {
	case state.StatusRunning:
		return "*"
	case state.StatusStopped:
		return "-"
	case state.StatusFailed:
		return "!"
	default:
		return "?"
	}
}

// ─── Search ───────────────────────────────────────────────────────────────────

// renderSearchBar renders the search input bar shown under the header when
// search mode is active.
func (m Model) renderSearchBar() string {
	cursor := "█"
	matchInfo := ""
	if len(m.searchMatches) > 0 {
		matchInfo = fmt.Sprintf("  [%d/%d]", m.searchMatchIdx+1, len(m.searchMatches))
	} else if m.searchQuery != "" {
		matchInfo = "  [no matches]"
	}

	left := " /" + m.searchQuery + cursor + matchInfo
	right := "esc=close  enter|/=next "

	gap := m.width - lipgloss.Width(left) - len(right)
	if gap < 1 {
		gap = 1
	}
	line := left + strings.Repeat(" ", gap) + right
	if len(line) > m.width {
		line = line[:m.width]
	}

	return lipgloss.NewStyle().
		Background(colorSearchBar).
		Foreground(lipgloss.Color("#ECEFF1")).
		Render(line)
}

// updateSearchMatches rebuilds the list of matching line indices for the
// current query and selected target.
func (m *Model) updateSearchMatches() {
	m.searchMatches = nil
	m.searchMatchIdx = 0
	if m.searchQuery == "" || len(m.targets) == 0 {
		return
	}
	name := m.targets[m.selected].Name
	buf, ok := m.logs[name]
	if !ok {
		return
	}
	q := strings.ToLower(m.searchQuery)
	for i, line := range buf.lines {
		if strings.Contains(strings.ToLower(line), q) {
			m.searchMatches = append(m.searchMatches, i)
		}
	}
}

// jumpToCurrentMatch scrolls the viewport so the current match is visible.
func (m *Model) jumpToCurrentMatch() {
	if len(m.searchMatches) == 0 {
		return
	}
	m.viewport.SetYOffset(m.searchMatches[m.searchMatchIdx])
}

// nextSearchMatch advances to the next match (wrapping) and refreshes the
// viewport so the current-match highlight moves.
func (m *Model) nextSearchMatch() {
	if len(m.searchMatches) == 0 {
		return
	}
	m.searchMatchIdx = (m.searchMatchIdx + 1) % len(m.searchMatches)
	m.jumpToCurrentMatch()
	m.refreshViewportContent()
}

// refreshViewportContent re-renders the viewport content in place, preserving
// the scroll position unless the viewport was at the bottom.
func (m *Model) refreshViewportContent() {
	if len(m.targets) == 0 {
		return
	}
	name := m.targets[m.selected].Name
	buf, ok := m.logs[name]
	if !ok {
		return
	}
	wasAtBottom := m.viewport.AtBottom()
	m.viewport.SetContent(m.viewportContent(buf))
	if wasAtBottom {
		m.viewport.GotoBottom()
	}
}

// viewportContent returns the log content to set in the viewport, applying
// search highlighting and/or line wrapping as configured.
func (m Model) viewportContent(buf *logBuffer) string {
	var content string
	if !m.searchMode || m.searchQuery == "" {
		content = buf.content()
	} else {
		content = m.renderLogsWithHighlight(buf)
	}
	if m.wrapLogs {
		content = wrapContent(content, m.viewport.Width)
	}
	return content
}

// wrapContent soft-wraps every line in content at width visible columns.
// It delegates to ansi.Hardwrap which handles ANSI escape codes, grapheme
// clusters, and wide characters in a single O(n) pass.
func wrapContent(content string, width int) string {
	return ansi.Hardwrap(content, width, true)
}

// renderLogsWithHighlight returns the log lines joined with search matches
// highlighted. The current match line uses a distinct accent colour.
func (m Model) renderLogsWithHighlight(buf *logBuffer) string {
	currentMatchLine := -1
	if len(m.searchMatches) > 0 {
		currentMatchLine = m.searchMatches[m.searchMatchIdx]
	}

	lines := make([]string, len(buf.lines))
	q := strings.ToLower(m.searchQuery)
	for i, line := range buf.lines {
		if strings.Contains(strings.ToLower(line), q) {
			lines[i] = highlightOccurrences(line, m.searchQuery, i == currentMatchLine)
		} else {
			lines[i] = line
		}
	}
	return strings.Join(lines, "\n")
}

// highlightOccurrences wraps every case-insensitive occurrence of query in line
// with the appropriate lipgloss style. isCurrent selects the accent colour for
// the active match line.
func highlightOccurrences(line, query string, isCurrent bool) string {
	if query == "" {
		return line
	}
	lowerLine := strings.ToLower(line)
	lowerQuery := strings.ToLower(query)
	ql := len(lowerQuery)

	style := searchMatchStyle
	if isCurrent {
		style = searchCurrentMatchStyle
	}

	var result strings.Builder
	offset := 0
	for offset < len(line) {
		idx := strings.Index(lowerLine[offset:], lowerQuery)
		if idx < 0 {
			result.WriteString(line[offset:])
			break
		}
		result.WriteString(line[offset : offset+idx])
		result.WriteString(style.Render(line[offset+idx : offset+idx+ql]))
		offset += idx + ql
	}
	return result.String()
}

// ─── Key match helper ─────────────────────────────────────────────────────────

func matchKey(msg tea.KeyMsg, binding key.Binding) bool {
	return key.Matches(msg, binding)
}

// ─── Async action commands ────────────────────────────────────────────────────

func (m Model) doRestart(name string) tea.Cmd {
	mgr := m.manager
	return func() tea.Msg {
		// Use context.Background() so the restarted process is not killed
		// when the TUI context is cancelled (e.g. user presses q).
		if err := mgr.Restart(context.Background(), name); err != nil {
			return actionResultMsg{msg: fmt.Sprintf("restart %s: %s", name, err)}
		}
		return actionResultMsg{msg: fmt.Sprintf("Restarted %s", name)}
	}
}

func (m Model) doStop(name string) tea.Cmd {
	mgr := m.manager
	return func() tea.Msg {
		if err := mgr.Stop(name); err != nil {
			return actionResultMsg{msg: fmt.Sprintf("stop %s: %s", name, err)}
		}
		return actionResultMsg{msg: fmt.Sprintf("Stopped %s", name)}
	}
}

func (m Model) doStart(name string) tea.Cmd {
	mgr := m.manager
	return func() tea.Msg {
		// Use context.Background() so the process is not killed when the TUI exits.
		if err := mgr.Start(context.Background(), name); err != nil {
			return actionResultMsg{msg: fmt.Sprintf("start %s: %s", name, err)}
		}
		return actionResultMsg{msg: fmt.Sprintf("Started %s", name)}
	}
}

func (m Model) doClear(name string) tea.Cmd {
	mgr := m.manager
	return func() tea.Msg {
		if err := mgr.ClearLog(name); err != nil {
			return actionResultMsg{msg: fmt.Sprintf("clear %s: %s", name, err)}
		}
		return clearLogMsg{target: name}
	}
}

func (m Model) doDump(name string) tea.Cmd {
	mgr := m.manager
	dest := fmt.Sprintf("%s-%s.log", name, time.Now().Format("20060102-150405"))
	return func() tea.Msg {
		if err := mgr.DumpLog(name, dest); err != nil {
			return actionResultMsg{msg: fmt.Sprintf("dump %s: %s", name, err)}
		}
		return actionResultMsg{msg: fmt.Sprintf("Logs dumped to %s", dest)}
	}
}
