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

package config

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

// RunTarget defines a process to be managed by tukituki.
type RunTarget struct {
	Name        string            `yaml:"name"`
	Command     string            `yaml:"command"`
	Workdir     string            `yaml:"workdir"`     // relative to project root (where .run/ lives)
	Args        []string          `yaml:"args"`
	Env         map[string]string `yaml:"env"`
	Description string            `yaml:"description"` // optional human-readable description
	// Cleanup is an optional list of shell commands run (via $SHELL -l -c) after
	// the process is stopped.  Useful for releasing ports, removing PID files,
	// or killing stray child processes.  Each command is run in sequence;
	// failures are logged but do not abort remaining cleanup steps.
	Cleanup []string `yaml:"cleanup"`
	// Otel enables OpenTelemetry log collection for this target.  When true,
	// tukituki injects OTEL_EXPORTER_OTLP_ENDPOINT into the process environment
	// and ensures a bundled OTLP receiver is running.
	Otel bool `yaml:"otel"`
	// ParseError is set when the YAML file could not be parsed. The target
	// will appear in the TUI with the error displayed but cannot be started.
	ParseError string `yaml:"-"`
	// Virtual marks targets that are synthesised by tukituki (e.g. the OTel
	// collector) rather than loaded from a .run/*.yaml file.
	Virtual bool `yaml:"-"`
	// SourceFile is the absolute path to the .run/*.yaml file this target was
	// loaded from.  Empty for virtual targets or targets with parse errors.
	SourceFile string `yaml:"-"`
}

// LoadTargets reads all *.yaml and *.yml files from runDir and returns the
// parsed RunTargets sorted by Name.  It returns an error if runDir does not
// exist.
func LoadTargets(runDir string) ([]RunTarget, error) {
	info, err := os.Stat(runDir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, fmt.Errorf("run directory does not exist: %s", runDir)
		}
		return nil, fmt.Errorf("stat run directory: %w", err)
	}
	if !info.IsDir() {
		return nil, fmt.Errorf("run directory path is not a directory: %s", runDir)
	}

	patterns := []string{
		filepath.Join(runDir, "*.yaml"),
		filepath.Join(runDir, "*.yml"),
	}

	var files []string
	for _, pattern := range patterns {
		matches, err := filepath.Glob(pattern)
		if err != nil {
			return nil, fmt.Errorf("glob %q: %w", pattern, err)
		}
		files = append(files, matches...)
	}

	var targets []RunTarget
	for _, file := range files {
		absFile, _ := filepath.Abs(file)
		t, err := parseFile(file)
		if err != nil {
			// Record the error but keep going so the TUI can display it.
			name := strings.TrimSuffix(filepath.Base(file), filepath.Ext(file))
			targets = append(targets, RunTarget{
				Name:       name,
				ParseError: fmt.Sprintf("%s: %v", filepath.Base(file), err),
				SourceFile: absFile,
			})
			continue
		}
		t.SourceFile = absFile
		targets = append(targets, t)
	}

	sort.Slice(targets, func(i, j int) bool {
		return targets[i].Name < targets[j].Name
	})

	return targets, nil
}

// HasOtelTarget reports whether any target in the list has Otel enabled.
func HasOtelTarget(targets []RunTarget) bool {
	for _, t := range targets {
		if t.Otel {
			return true
		}
	}
	return false
}

func parseFile(path string) (RunTarget, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return RunTarget{}, fmt.Errorf("read file: %w", err)
	}

	var t RunTarget
	dec := yaml.NewDecoder(bytes.NewReader(data))
	dec.KnownFields(true)
	if err := dec.Decode(&t); err != nil {
		return RunTarget{}, fmt.Errorf("yaml decode: %w", err)
	}

	return t, nil
}
