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

import "github.com/charmbracelet/lipgloss"

const (
	leftPanelWidth = 22 // characters wide (including border)
	borderWidth    = 1  // single-char border on each side
)

// colours
var (
	colorGreen     = lipgloss.Color("#00E676")
	colorYellow    = lipgloss.Color("#FFD600")
	colorRed       = lipgloss.Color("#FF1744")
	colorGray      = lipgloss.Color("#78909C")
	colorBlue      = lipgloss.Color("#40C4FF")
	colorDim       = lipgloss.Color("#546E7A")
	colorBorder    = lipgloss.Color("#37474F")
	colorHeader    = lipgloss.Color("#263238")
	colorSearchBar = lipgloss.Color("#004D40")
)

var (
	// headerStyle styles the top header bar.
	headerStyle = lipgloss.NewStyle().
			Background(colorHeader).
			Foreground(lipgloss.Color("#ECEFF1")).
			Bold(true).
			Padding(0, 1)

	// headerHintStyle styles the key hint portion of the header (right-aligned).
	headerHintStyle = lipgloss.NewStyle().
			Background(colorHeader).
			Foreground(colorDim)

	// leftPanelStyle is the container for the target list.
	// No horizontal padding — rows are pre-padded to full innerWidth so the
	// selected-row highlight spans edge-to-edge inside the border.
	leftPanelStyle = lipgloss.NewStyle().
			Border(lipgloss.NormalBorder()).
			BorderForeground(colorBorder)

	// rightPanelStyle is the container for the log viewport.
	rightPanelStyle = lipgloss.NewStyle().
			Border(lipgloss.NormalBorder()).
			BorderForeground(colorBorder)

	// selectedItemStyle highlights the currently selected target with a
	// filled background row so it's unambiguous even on low-contrast terminals.
	selectedItemStyle = lipgloss.NewStyle().
				Background(lipgloss.Color("#1565C0")).
				Foreground(lipgloss.Color("#FFFFFF")).
				Bold(true)

	// normalItemStyle is the default target list item style.
	normalItemStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#B0BEC5"))

	// statusMsgStyle styles the transient status message shown in the header.
	statusMsgStyle = lipgloss.NewStyle().
			Foreground(colorDim).
			Italic(true)

	// keyHintStyle styles the bottom key-hint bar in the left panel.
	keyHintStyle = lipgloss.NewStyle().
			Foreground(colorDim)

	// Status icon styles — pre-rendered with ANSI colour for use outside lipgloss containers.
	iconRunning = lipgloss.NewStyle().Foreground(colorGreen).Render("●")
	iconStopped = lipgloss.NewStyle().Foreground(colorYellow).Render("○")
	iconFailed  = lipgloss.NewStyle().Foreground(colorRed).Render("✗")
	iconUnknown = lipgloss.NewStyle().Foreground(colorGray).Render("?")

	// rightPanelTitleStyle styles the selected target name shown at the top of
	// the right panel.
	rightPanelTitleStyle = lipgloss.NewStyle().
				Bold(true).
				Foreground(lipgloss.Color("#ECEFF1")).
				Padding(0, 1)

	// separatorStyle is the vertical divider column between the two panels.
	separatorStyle = lipgloss.NewStyle().Foreground(colorBlue)

	// searchMatchStyle highlights all occurrences of the search query.
	searchMatchStyle = lipgloss.NewStyle().
				Background(lipgloss.Color("#FFD600")).
				Foreground(lipgloss.Color("#000000")).
				Bold(true)

	// searchCurrentMatchStyle highlights the currently active search match.
	searchCurrentMatchStyle = lipgloss.NewStyle().
				Background(lipgloss.Color("#FF6D00")).
				Foreground(lipgloss.Color("#FFFFFF")).
				Bold(true)
)
