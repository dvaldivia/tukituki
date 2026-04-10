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
		t, err := parseFile(file)
		if err != nil {
			return nil, fmt.Errorf("parse %s: %w", file, err)
		}
		targets = append(targets, t)
	}

	sort.Slice(targets, func(i, j int) bool {
		return targets[i].Name < targets[j].Name
	})

	return targets, nil
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
