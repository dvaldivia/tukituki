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

import "github.com/charmbracelet/bubbles/key"

// keyMap holds all key bindings used by the TUI.
type keyMap struct {
	Up      key.Binding
	Down    key.Binding
	Tab     key.Binding
	Restart key.Binding
	Stop    key.Binding
	Start   key.Binding
	Dump    key.Binding
	Clear   key.Binding
	// Detach quits the TUI but keeps all processes running.
	Detach key.Binding
	// KillAll stops all processes and exits.
	KillAll key.Binding

	// Viewport scroll bindings (passed through to the viewport)
	PageUp   key.Binding
	PageDown key.Binding
}

// defaultKeyMap returns the default key bindings.
func defaultKeyMap() keyMap {
	return keyMap{
		Up: key.NewBinding(
			key.WithKeys("up", "k"),
			key.WithHelp("↑/k", "move up"),
		),
		Down: key.NewBinding(
			key.WithKeys("down", "j"),
			key.WithHelp("↓/j", "move down"),
		),
		Tab: key.NewBinding(
			key.WithKeys("tab"),
			key.WithHelp("tab", "next target"),
		),
		Restart: key.NewBinding(
			key.WithKeys("r"),
			key.WithHelp("r", "restart"),
		),
		Stop: key.NewBinding(
			key.WithKeys("s"),
			key.WithHelp("s", "stop"),
		),
		Start: key.NewBinding(
			key.WithKeys("S"),
			key.WithHelp("S", "start"),
		),
		Dump: key.NewBinding(
			key.WithKeys("d"),
			key.WithHelp("d", "dump logs"),
		),
		Clear: key.NewBinding(
			key.WithKeys("c"),
			key.WithHelp("c", "clear logs"),
		),
		Detach: key.NewBinding(
			key.WithKeys("q"),
			key.WithHelp("q", "detach (keep procs)"),
		),
		KillAll: key.NewBinding(
			// Q or ctrl+c both stop all processes and exit.
			key.WithKeys("Q", "ctrl+c"),
			key.WithHelp("Q/^C", "stop all & exit"),
		),
		PageUp: key.NewBinding(
			key.WithKeys("pgup", "b"),
			key.WithHelp("pgup/b", "page up"),
		),
		PageDown: key.NewBinding(
			key.WithKeys("pgdown", "f"),
			key.WithHelp("pgdn/f", "page down"),
		),
	}
}
