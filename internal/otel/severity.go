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

package otel

import (
	"fmt"
	"strings"

	logsv1 "go.opentelemetry.io/proto/otlp/logs/v1"
)

// severityThresholds maps human-readable severity names to the minimum OTLP
// SeverityNumber that qualifies. For example, "error" maps to 17 which means
// ERROR, ERROR2, ERROR3, ERROR4, FATAL, and FATAL2-4 all pass the filter.
var severityThresholds = map[string]logsv1.SeverityNumber{
	"trace": logsv1.SeverityNumber_SEVERITY_NUMBER_TRACE,
	"debug": logsv1.SeverityNumber_SEVERITY_NUMBER_DEBUG,
	"info":  logsv1.SeverityNumber_SEVERITY_NUMBER_INFO,
	"warn":  logsv1.SeverityNumber_SEVERITY_NUMBER_WARN,
	"error": logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR,
	"fatal": logsv1.SeverityNumber_SEVERITY_NUMBER_FATAL,
}

// ParseSeverity converts a human-readable severity name to its OTLP
// SeverityNumber threshold. Returns an error for unrecognised names.
func ParseSeverity(name string) (logsv1.SeverityNumber, error) {
	sev, ok := severityThresholds[strings.ToLower(name)]
	if !ok {
		return 0, fmt.Errorf("unknown severity %q (valid: trace, debug, info, warn, error, fatal)", name)
	}
	return sev, nil
}

// SeverityNames returns the list of recognised severity level names.
func SeverityNames() []string {
	return []string{"trace", "debug", "info", "warn", "error", "fatal"}
}
