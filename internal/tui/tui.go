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
	tea "github.com/charmbracelet/bubbletea"
	"github.com/dvaldivia/tukituki/internal/config"
)

// Start runs the bubbletea program with the given targets and manager.
// It blocks until the user quits.
// Returns whether the user chose to also stop all processes (Q vs q).
func Start(targets []config.RunTarget, manager ManagerInterface) (stopAll bool, err error) {
	m := NewModel(targets, manager)

	p := tea.NewProgram(
		m,
		tea.WithAltScreen(),       // use the alternate screen buffer
		tea.WithMouseCellMotion(), // enable mouse for viewport scrolling
	)

	finalModel, err := p.Run()
	if err != nil {
		return false, err
	}

	if fm, ok := finalModel.(Model); ok {
		return fm.stopAll, nil
	}
	return false, nil
}
