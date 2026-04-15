package otel

import (
	"testing"

	logsv1 "go.opentelemetry.io/proto/otlp/logs/v1"
)

func TestParseSeverity(t *testing.T) {
	cases := []struct {
		input string
		want  logsv1.SeverityNumber
	}{
		{"trace", logsv1.SeverityNumber_SEVERITY_NUMBER_TRACE},
		{"debug", logsv1.SeverityNumber_SEVERITY_NUMBER_DEBUG},
		{"info", logsv1.SeverityNumber_SEVERITY_NUMBER_INFO},
		{"warn", logsv1.SeverityNumber_SEVERITY_NUMBER_WARN},
		{"error", logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR},
		{"fatal", logsv1.SeverityNumber_SEVERITY_NUMBER_FATAL},
	}
	for _, tc := range cases {
		t.Run(tc.input, func(t *testing.T) {
			got, err := ParseSeverity(tc.input)
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if got != tc.want {
				t.Errorf("ParseSeverity(%q) = %d, want %d", tc.input, got, tc.want)
			}
		})
	}
}

func TestParseSeverity_CaseInsensitive(t *testing.T) {
	got, err := ParseSeverity("ERROR")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got != logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR {
		t.Errorf("ParseSeverity(\"ERROR\") = %d, want %d", got, logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR)
	}
}

func TestParseSeverity_Invalid(t *testing.T) {
	_, err := ParseSeverity("bogus")
	if err == nil {
		t.Fatal("expected error for unknown severity, got nil")
	}
}

func TestSeverityNames(t *testing.T) {
	names := SeverityNames()
	if len(names) != 6 {
		t.Errorf("expected 6 severity names, got %d", len(names))
	}
}
