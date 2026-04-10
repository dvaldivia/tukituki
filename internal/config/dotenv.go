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
	"fmt"
	"os"
	"strings"
)

// ParseDotEnv parses a .env file and returns a map of key→value pairs.
// Blank lines and lines starting with # are ignored.
// An optional "export " prefix is stripped from each line.
// Values may be unquoted, double-quoted, or single-quoted.
// Inline comments (# ...) are stripped from unquoted values.
// Returns nil, nil when the file does not exist.
func ParseDotEnv(path string) (map[string]string, error) {
	data, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read .env: %w", err)
	}

	out := make(map[string]string)
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		line = strings.TrimPrefix(line, "export ")
		line = strings.TrimSpace(line)

		idx := strings.IndexByte(line, '=')
		if idx < 0 {
			continue // no '=' — bare variable name, skip
		}

		key := strings.TrimSpace(line[:idx])
		if key == "" {
			continue
		}
		out[key] = parseDotEnvValue(line[idx+1:])
	}
	return out, nil
}

// parseDotEnvValue extracts the value portion of a KEY=VALUE line.
// Handles double-quoted, single-quoted, and unquoted values.
func parseDotEnvValue(v string) string {
	v = strings.TrimSpace(v)
	if len(v) == 0 {
		return ""
	}

	switch v[0] {
	case '"':
		// Double-quoted: scan for closing unescaped quote, unescaping \" inside.
		var sb strings.Builder
		i := 1
		for i < len(v) {
			if v[i] == '\\' && i+1 < len(v) && v[i+1] == '"' {
				sb.WriteByte('"')
				i += 2
			} else if v[i] == '"' {
				break
			} else {
				sb.WriteByte(v[i])
				i++
			}
		}
		return sb.String()
	case '\'':
		// Single-quoted: no escape processing.
		end := strings.Index(v[1:], "'")
		if end < 0 {
			return v[1:]
		}
		return v[1 : end+1]
	default:
		// Unquoted: strip inline comment and trailing whitespace.
		if idx := strings.IndexByte(v, '#'); idx >= 0 {
			v = v[:idx]
		}
		return strings.TrimSpace(v)
	}
}

// ExpandEnv returns a copy of targets with ${VAR} references in each target's
// env values expanded. The lookup order is: vars (from .env file) first, then
// the process's inherited OS environment via os.Getenv.
// Targets whose Env map has no interpolation markers are returned as-is.
func ExpandEnv(targets []RunTarget, vars map[string]string) []RunTarget {
	if len(vars) == 0 {
		return targets
	}
	lookup := func(key string) string {
		if v, ok := vars[key]; ok {
			return v
		}
		return os.Getenv(key)
	}
	out := make([]RunTarget, len(targets))
	for i, t := range targets {
		expanded := make(map[string]string, len(t.Env))
		for k, v := range t.Env {
			expanded[k] = os.Expand(v, lookup)
		}
		t.Env = expanded
		out[i] = t
	}
	return out
}
