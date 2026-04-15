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
	"os"
	"path/filepath"
	"testing"
)

func TestLoadTargets_Success(t *testing.T) {
	dir := t.TempDir()

	writeYAML(t, filepath.Join(dir, "web.yaml"), `
name: web
command: go
args: ["run", "."]
workdir: ./backend
description: "Web server"
env:
  PORT: "8080"
`)
	writeYAML(t, filepath.Join(dir, "worker.yml"), `
name: worker
command: ./worker
`)

	targets, err := LoadTargets(dir)
	if err != nil {
		t.Fatalf("LoadTargets error: %v", err)
	}

	if len(targets) != 2 {
		t.Fatalf("expected 2 targets, got %d", len(targets))
	}

	// Results must be sorted by name.
	if targets[0].Name != "web" {
		t.Errorf("expected first target name=web, got %q", targets[0].Name)
	}
	if targets[1].Name != "worker" {
		t.Errorf("expected second target name=worker, got %q", targets[1].Name)
	}

	// Spot-check fields.
	if targets[0].Command != "go" {
		t.Errorf("expected command=go, got %q", targets[0].Command)
	}
	if targets[0].Env["PORT"] != "8080" {
		t.Errorf("expected PORT=8080, got %q", targets[0].Env["PORT"])
	}
}

func TestLoadTargets_EmptyDir(t *testing.T) {
	dir := t.TempDir()

	targets, err := LoadTargets(dir)
	if err != nil {
		t.Fatalf("unexpected error on empty dir: %v", err)
	}
	if len(targets) != 0 {
		t.Errorf("expected 0 targets, got %d", len(targets))
	}
}

func TestLoadTargets_InvalidYAML(t *testing.T) {
	dir := t.TempDir()
	writeYAML(t, filepath.Join(dir, "bad.yaml"), `
name: [this is: {not: valid yaml for a string
`)

	targets, err := LoadTargets(dir)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if len(targets) != 1 {
		t.Fatalf("expected 1 target, got %d", len(targets))
	}
	if targets[0].Name != "bad" {
		t.Errorf("expected name derived from filename %q, got %q", "bad", targets[0].Name)
	}
	if targets[0].ParseError == "" {
		t.Fatal("expected ParseError to be set for invalid YAML")
	}
}

func TestLoadTargets_MissingDir(t *testing.T) {
	_, err := LoadTargets("/nonexistent/path/that/does/not/exist")
	if err == nil {
		t.Fatal("expected error for missing directory, got nil")
	}
}

func TestLoadTargets_OtelField(t *testing.T) {
	dir := t.TempDir()
	writeYAML(t, filepath.Join(dir, "svc.yaml"), `
name: svc
command: echo
otel: true
`)
	targets, err := LoadTargets(dir)
	if err != nil {
		t.Fatalf("LoadTargets: %v", err)
	}
	if len(targets) != 1 {
		t.Fatalf("expected 1 target, got %d", len(targets))
	}
	if !targets[0].Otel {
		t.Error("expected Otel=true")
	}
}

func TestLoadTargets_OtelDefaultFalse(t *testing.T) {
	dir := t.TempDir()
	writeYAML(t, filepath.Join(dir, "plain.yaml"), `
name: plain
command: echo
`)
	targets, err := LoadTargets(dir)
	if err != nil {
		t.Fatalf("LoadTargets: %v", err)
	}
	if targets[0].Otel {
		t.Error("expected Otel=false by default")
	}
}

func TestHasOtelTarget(t *testing.T) {
	none := []RunTarget{{Name: "a"}, {Name: "b"}}
	if HasOtelTarget(none) {
		t.Error("expected false when no target has Otel")
	}

	some := []RunTarget{{Name: "a"}, {Name: "b", Otel: true}}
	if !HasOtelTarget(some) {
		t.Error("expected true when one target has Otel")
	}
}

// helpers

func writeYAML(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}
