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

// ---------------------------------------------------------------------------
// ParseDotEnv
// ---------------------------------------------------------------------------

func TestParseDotEnv_BasicKeyValue(t *testing.T) {
	path := writeDotEnv(t, `
APP_DOMAIN=myhost
PORT=8080
`)
	vars, err := ParseDotEnv(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	assertEnv(t, vars, "APP_DOMAIN", "myhost")
	assertEnv(t, vars, "PORT", "8080")
}

func TestParseDotEnv_Quoted(t *testing.T) {
	path := writeDotEnv(t, `
DOUBLE="hello world"
SINGLE='foo bar'
ESCAPED="she said \"hi\""
`)
	vars, err := ParseDotEnv(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	assertEnv(t, vars, "DOUBLE", "hello world")
	assertEnv(t, vars, "SINGLE", "foo bar")
	assertEnv(t, vars, "ESCAPED", `she said "hi"`)
}

func TestParseDotEnv_Comments(t *testing.T) {
	path := writeDotEnv(t, `
# this is a comment
KEY=value # inline comment
OTHER=plain
`)
	vars, err := ParseDotEnv(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	assertEnv(t, vars, "KEY", "value")
	assertEnv(t, vars, "OTHER", "plain")
	if _, ok := vars["# this is a comment"]; ok {
		t.Error("comment line should not produce a key")
	}
}

func TestParseDotEnv_ExportPrefix(t *testing.T) {
	path := writeDotEnv(t, `export APP_DOMAIN=remotehost`)
	vars, err := ParseDotEnv(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	assertEnv(t, vars, "APP_DOMAIN", "remotehost")
}

func TestParseDotEnv_MissingFile(t *testing.T) {
	vars, err := ParseDotEnv("/nonexistent/path/.env")
	if err != nil {
		t.Fatalf("missing file should return nil error, got: %v", err)
	}
	if vars != nil {
		t.Errorf("missing file should return nil map, got: %v", vars)
	}
}

func TestParseDotEnv_EmptyFile(t *testing.T) {
	path := writeDotEnv(t, "")
	vars, err := ParseDotEnv(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(vars) != 0 {
		t.Errorf("empty file should produce empty map, got %v", vars)
	}
}

func TestParseDotEnv_BlankLines(t *testing.T) {
	path := writeDotEnv(t, `

KEY=value

OTHER=123

`)
	vars, err := ParseDotEnv(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(vars) != 2 {
		t.Errorf("expected 2 keys, got %d: %v", len(vars), vars)
	}
}

// ---------------------------------------------------------------------------
// ExpandEnv
// ---------------------------------------------------------------------------

func TestExpandEnv_SubstitutesVars(t *testing.T) {
	targets := []RunTarget{
		{
			Name:    "server",
			Command: "go",
			Env: map[string]string{
				"S3_ENDPOINT":  "http://${APP_DOMAIN}:9001",
				"S3_PUBLIC_URL": "http://${APP_DOMAIN}:9001/bucket",
				"STATIC_KEY":   "no-substitution",
			},
		},
	}
	vars := map[string]string{"APP_DOMAIN": "myhost"}

	result := ExpandEnv(targets, vars)

	assertEnv(t, result[0].Env, "S3_ENDPOINT", "http://myhost:9001")
	assertEnv(t, result[0].Env, "S3_PUBLIC_URL", "http://myhost:9001/bucket")
	assertEnv(t, result[0].Env, "STATIC_KEY", "no-substitution")
}

func TestExpandEnv_FallsBackToOSEnv(t *testing.T) {
	t.Setenv("TEST_HOST_FALLBACK", "fromshell")
	targets := []RunTarget{
		{
			Name:    "worker",
			Command: "go",
			Env:     map[string]string{"URL": "http://${TEST_HOST_FALLBACK}:1234"},
		},
	}
	result := ExpandEnv(targets, map[string]string{})
	// Empty vars map → early return, no expansion.
	assertEnv(t, result[0].Env, "URL", "http://${TEST_HOST_FALLBACK}:1234")

	// With a non-empty (but irrelevant) vars map, OS env fallback kicks in.
	result2 := ExpandEnv(targets, map[string]string{"UNRELATED": "x"})
	assertEnv(t, result2[0].Env, "URL", "http://fromshell:1234")
}

func TestExpandEnv_DotEnvTakesPrecedenceOverOSEnv(t *testing.T) {
	t.Setenv("DOMAIN_PRECEDENCE_TEST", "from-os")
	targets := []RunTarget{
		{
			Name:    "api",
			Command: "go",
			Env:     map[string]string{"BASE": "http://${DOMAIN_PRECEDENCE_TEST}"},
		},
	}
	vars := map[string]string{"DOMAIN_PRECEDENCE_TEST": "from-dotenv"}
	result := ExpandEnv(targets, vars)
	assertEnv(t, result[0].Env, "BASE", "http://from-dotenv")
}

func TestExpandEnv_NoVarsReturnsOriginal(t *testing.T) {
	targets := []RunTarget{
		{Name: "x", Command: "go", Env: map[string]string{"A": "${FOO}"}},
	}
	result := ExpandEnv(targets, nil)
	// Value must be unexpanded because vars is nil → early return.
	assertEnv(t, result[0].Env, "A", "${FOO}")
}

func TestExpandEnv_DoesNotMutateOriginal(t *testing.T) {
	original := map[string]string{"URL": "http://${HOST}"}
	targets := []RunTarget{{Name: "s", Command: "go", Env: original}}
	_ = ExpandEnv(targets, map[string]string{"HOST": "new"})
	if original["URL"] != "http://${HOST}" {
		t.Error("ExpandEnv must not mutate the original Env map")
	}
}

func TestExpandEnv_SubstitutesArgs(t *testing.T) {
	targets := []RunTarget{
		{
			Name:    "docs",
			Command: "hugo",
			Args:    []string{"server", "--baseURL", "http://${APP_DOMAIN}:5313"},
		},
	}
	result := ExpandEnv(targets, map[string]string{"APP_DOMAIN": "myhost"})

	want := []string{"server", "--baseURL", "http://myhost:5313"}
	if len(result[0].Args) != len(want) {
		t.Fatalf("args len = %d, want %d", len(result[0].Args), len(want))
	}
	for i, got := range result[0].Args {
		if got != want[i] {
			t.Errorf("args[%d] = %q, want %q", i, got, want[i])
		}
	}
}

func TestExpandEnv_SubstitutesCommand(t *testing.T) {
	targets := []RunTarget{
		{Name: "run", Command: "${BINARY}", Args: []string{}},
	}
	result := ExpandEnv(targets, map[string]string{"BINARY": "myapp"})
	if result[0].Command != "myapp" {
		t.Errorf("Command = %q, want %q", result[0].Command, "myapp")
	}
}

func TestExpandEnv_SubstitutesWorkdir(t *testing.T) {
	targets := []RunTarget{
		{Name: "run", Command: "make", Workdir: "${PROJECT_DIR}/docs"},
	}
	result := ExpandEnv(targets, map[string]string{"PROJECT_DIR": "/home/user/app"})
	if result[0].Workdir != "/home/user/app/docs" {
		t.Errorf("Workdir = %q, want %q", result[0].Workdir, "/home/user/app/docs")
	}
}

func TestExpandEnv_SubstitutesCleanup(t *testing.T) {
	targets := []RunTarget{
		{
			Name:    "server",
			Command: "node",
			Cleanup: []string{"lsof -ti:${PORT} | xargs kill -9 2>/dev/null || true"},
		},
	}
	result := ExpandEnv(targets, map[string]string{"PORT": "3000"})
	want := "lsof -ti:3000 | xargs kill -9 2>/dev/null || true"
	if result[0].Cleanup[0] != want {
		t.Errorf("Cleanup[0] = %q, want %q", result[0].Cleanup[0], want)
	}
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

func writeDotEnv(t *testing.T, content string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), ".env")
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatalf("write .env: %v", err)
	}
	return path
}

func assertEnv(t *testing.T, env map[string]string, key, want string) {
	t.Helper()
	got, ok := env[key]
	if !ok {
		t.Errorf("key %q not found in env map", key)
		return
	}
	if got != want {
		t.Errorf("env[%q]: got %q, want %q", key, got, want)
	}
}
