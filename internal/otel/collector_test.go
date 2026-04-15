package otel

import (
	"bytes"
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	collogsv1 "go.opentelemetry.io/proto/otlp/collector/logs/v1"
	commonv1 "go.opentelemetry.io/proto/otlp/common/v1"
	logsv1 "go.opentelemetry.io/proto/otlp/logs/v1"
	resourcev1 "go.opentelemetry.io/proto/otlp/resource/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/protobuf/proto"
)

// ─── Unit tests ─────────────────────────────────────────────────────────────

func TestExtractServiceName(t *testing.T) {
	attrs := []*commonv1.KeyValue{
		{Key: "host.name", Value: &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: "localhost"}}},
		{Key: "service.name", Value: &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: "my-api"}}},
	}
	got := extractServiceName(attrs)
	if got != "my-api" {
		t.Errorf("extractServiceName = %q, want %q", got, "my-api")
	}
}

func TestExtractServiceName_Missing(t *testing.T) {
	attrs := []*commonv1.KeyValue{
		{Key: "host.name", Value: &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: "localhost"}}},
	}
	got := extractServiceName(attrs)
	if got != "" {
		t.Errorf("extractServiceName = %q, want empty", got)
	}
}

func TestAnyValueToString(t *testing.T) {
	cases := []struct {
		name  string
		value *commonv1.AnyValue
		want  string
	}{
		{"nil", nil, ""},
		{"string", &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: "hello"}}, "hello"},
		{"int", &commonv1.AnyValue{Value: &commonv1.AnyValue_IntValue{IntValue: 42}}, "42"},
		{"double", &commonv1.AnyValue{Value: &commonv1.AnyValue_DoubleValue{DoubleValue: 3.14}}, "3.14"},
		{"bool", &commonv1.AnyValue{Value: &commonv1.AnyValue_BoolValue{BoolValue: true}}, "true"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := anyValueToString(tc.value)
			if got != tc.want {
				t.Errorf("anyValueToString = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestProcessExportRequest_FiltersBelow(t *testing.T) {
	var buf bytes.Buffer
	req := buildExportRequest("test-svc", []logEntry{
		{severity: logsv1.SeverityNumber_SEVERITY_NUMBER_INFO, body: "just info"},
		{severity: logsv1.SeverityNumber_SEVERITY_NUMBER_DEBUG, body: "debug msg"},
		{severity: logsv1.SeverityNumber_SEVERITY_NUMBER_WARN, body: "a warning"},
	})
	processExportRequest(&buf, req, logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR)

	if buf.Len() != 0 {
		t.Errorf("expected no output for sub-ERROR logs, got: %q", buf.String())
	}
}

func TestProcessExportRequest_PassesAboveThreshold(t *testing.T) {
	var buf bytes.Buffer
	req := buildExportRequest("my-api", []logEntry{
		{severity: logsv1.SeverityNumber_SEVERITY_NUMBER_INFO, body: "info 1"},
		{severity: logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR, body: "something broke"},
		{severity: logsv1.SeverityNumber_SEVERITY_NUMBER_FATAL, body: "panic"},
		{severity: logsv1.SeverityNumber_SEVERITY_NUMBER_INFO, body: "info 2"},
	})
	processExportRequest(&buf, req, logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR)

	lines := nonEmptyLines(buf.String())
	if len(lines) != 2 {
		t.Fatalf("expected 2 lines, got %d: %v", len(lines), lines)
	}
	if !strings.Contains(lines[0], "[my-api] something broke") {
		t.Errorf("line[0] = %q, want [my-api] something broke", lines[0])
	}
	if !strings.Contains(lines[1], "[my-api] panic") {
		t.Errorf("line[1] = %q, want [my-api] panic", lines[1])
	}
}

func TestProcessExportRequest_TenInfoOneErrorTenInfo(t *testing.T) {
	var buf bytes.Buffer
	var entries []logEntry
	for i := 0; i < 10; i++ {
		entries = append(entries, logEntry{
			severity: logsv1.SeverityNumber_SEVERITY_NUMBER_INFO,
			body:     fmt.Sprintf("info log %d", i),
		})
	}
	entries = append(entries, logEntry{
		severity: logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR,
		body:     "database connection refused",
	})
	for i := 10; i < 20; i++ {
		entries = append(entries, logEntry{
			severity: logsv1.SeverityNumber_SEVERITY_NUMBER_INFO,
			body:     fmt.Sprintf("info log %d", i),
		})
	}
	req := buildExportRequest("api", entries)
	processExportRequest(&buf, req, logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR)

	lines := nonEmptyLines(buf.String())
	if len(lines) != 1 {
		t.Fatalf("expected exactly 1 error line, got %d: %v", len(lines), lines)
	}
	if lines[0] != "[api] database connection refused" {
		t.Errorf("line = %q, want %q", lines[0], "[api] database connection refused")
	}
}

func TestProcessExportRequest_UnknownServiceName(t *testing.T) {
	var buf bytes.Buffer
	// Build a request with no service.name attribute.
	req := &collogsv1.ExportLogsServiceRequest{
		ResourceLogs: []*logsv1.ResourceLogs{{
			Resource: &resourcev1.Resource{},
			ScopeLogs: []*logsv1.ScopeLogs{{
				LogRecords: []*logsv1.LogRecord{{
					SeverityNumber: logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR,
					Body:           &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: "boom"}},
				}},
			}},
		}},
	}
	processExportRequest(&buf, req, logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR)

	lines := nonEmptyLines(buf.String())
	if len(lines) != 1 || lines[0] != "[unknown] boom" {
		t.Errorf("got %q, want %q", buf.String(), "[unknown] boom\n")
	}
}

// ─── gRPC integration test ─────────────────────────────────────────────────

func TestCollectorGRPC_Integration(t *testing.T) {
	port := freePort(t)

	var buf bytes.Buffer
	c := &Collector{
		Port:        port,
		Protocol:    "grpc",
		MinSeverity: logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR,
		Output:      &buf,
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	errCh := make(chan error, 1)
	go func() { errCh <- c.Run(ctx) }()

	// Wait for the server to be ready.
	waitForPort(t, port)

	// Connect a gRPC client and send 10 INFO + 1 ERROR + 10 INFO.
	conn, err := grpc.NewClient(
		fmt.Sprintf("dns:///127.0.0.1:%d", port),
		grpc.WithTransportCredentials(insecure.NewCredentials()),
	)
	if err != nil {
		t.Fatalf("grpc dial: %v", err)
	}
	defer conn.Close()

	client := collogsv1.NewLogsServiceClient(conn)

	var entries []logEntry
	for i := 0; i < 10; i++ {
		entries = append(entries, logEntry{logsv1.SeverityNumber_SEVERITY_NUMBER_INFO, fmt.Sprintf("info %d", i)})
	}
	entries = append(entries, logEntry{logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR, "critical failure in database"})
	for i := 10; i < 20; i++ {
		entries = append(entries, logEntry{logsv1.SeverityNumber_SEVERITY_NUMBER_INFO, fmt.Sprintf("info %d", i)})
	}

	req := buildExportRequest("my-service", entries)
	_, err = client.Export(context.Background(), req)
	if err != nil {
		t.Fatalf("Export: %v", err)
	}

	// Give the server a moment to flush output.
	time.Sleep(100 * time.Millisecond)
	cancel()
	<-errCh

	lines := nonEmptyLines(buf.String())
	// First line is the "listening" banner; the rest are filtered logs.
	var logLines []string
	for _, l := range lines {
		if strings.HasPrefix(l, "[") {
			logLines = append(logLines, l)
		}
	}
	if len(logLines) != 1 {
		t.Fatalf("expected 1 error line, got %d: %v", len(logLines), logLines)
	}
	if logLines[0] != "[my-service] critical failure in database" {
		t.Errorf("got %q, want %q", logLines[0], "[my-service] critical failure in database")
	}
}

// ─── HTTP integration test ──────────────────────────────────────────────────

func TestCollectorHTTP_Integration(t *testing.T) {
	port := freePort(t)

	var buf bytes.Buffer
	c := &Collector{
		Port:        port,
		Protocol:    "http",
		MinSeverity: logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR,
		Output:      &buf,
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	errCh := make(chan error, 1)
	go func() { errCh <- c.Run(ctx) }()

	waitForPort(t, port)

	// Build and send an OTLP HTTP request with 10 INFO + 1 ERROR + 10 INFO.
	var entries []logEntry
	for i := 0; i < 10; i++ {
		entries = append(entries, logEntry{logsv1.SeverityNumber_SEVERITY_NUMBER_INFO, fmt.Sprintf("http info %d", i)})
	}
	entries = append(entries, logEntry{logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR, "http error: connection timeout"})
	for i := 10; i < 20; i++ {
		entries = append(entries, logEntry{logsv1.SeverityNumber_SEVERITY_NUMBER_INFO, fmt.Sprintf("http info %d", i)})
	}

	req := buildExportRequest("web-frontend", entries)
	body, err := proto.Marshal(req)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}

	resp, err := http.Post(
		fmt.Sprintf("http://localhost:%d/v1/logs", port),
		"application/x-protobuf",
		bytes.NewReader(body),
	)
	if err != nil {
		t.Fatalf("POST: %v", err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status = %d, want 200", resp.StatusCode)
	}

	time.Sleep(100 * time.Millisecond)
	cancel()
	<-errCh

	lines := nonEmptyLines(buf.String())
	var logLines []string
	for _, l := range lines {
		if strings.HasPrefix(l, "[") {
			logLines = append(logLines, l)
		}
	}
	if len(logLines) != 1 {
		t.Fatalf("expected 1 error line, got %d: %v", len(logLines), logLines)
	}
	if logLines[0] != "[web-frontend] http error: connection timeout" {
		t.Errorf("got %q, want %q", logLines[0], "[web-frontend] http error: connection timeout")
	}
}

// ─── Live tukituki integration test ─────────────────────────────────────────
// This test builds the tukituki binary, creates a project with an OTel-emitting
// process, starts everything headlessly, and verifies the error appears in the
// otel-errors log.

func TestLiveTukituki_OtelCollector(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping live integration test in short mode")
	}

	root := projectRoot(t)

	// Build tukituki binary.
	binDir := t.TempDir()
	binPath := binDir + "/tukituki"
	if out, err := runCmd(root, "go", "build", "-o", binPath, "./cmd/tukituki/").CombinedOutput(); err != nil {
		t.Fatalf("build tukituki: %v\n%s", err, out)
	}

	// Build the test emitter program.
	emitterPath := binDir + "/otel-emitter"
	if out, err := runCmd(root, "go", "build", "-o", emitterPath, "./internal/otel/testdata/emitter/").CombinedOutput(); err != nil {
		t.Fatalf("build emitter: %v\n%s", err, out)
	}

	// Create a temp project directory.
	projDir := t.TempDir()
	runDir := projDir + "/.run"
	stateDir := projDir + "/.tukituki"
	if err := os.MkdirAll(runDir, 0o755); err != nil {
		t.Fatal(err)
	}

	otelPort := freePort(t)

	// Write the emitter target config.
	writeFile(t, runDir+"/emitter.yaml", fmt.Sprintf(`name: emitter
command: %s
otel: true
`, emitterPath))

	// Start tukituki headlessly.
	startOut, err := runCmd(projDir, binPath, "start",
		"--run-dir", runDir,
		"--state-dir", stateDir,
		"--otel-port", fmt.Sprintf("%d", otelPort),
	).CombinedOutput()
	if err != nil {
		t.Fatalf("tukituki start: %v\n%s", err, startOut)
	}
	t.Logf("tukituki start output:\n%s", startOut)

	// Wait for the emitter to finish and logs to flush.
	// The emitter retries gRPC connection for up to 10s, then sends logs.
	// Give it plenty of time.
	time.Sleep(5 * time.Second)

	// Read the otel-errors log file directly (more reliable than using the
	// `logs` subcommand which creates a fresh Manager with an empty ring buffer).
	otelLogFile := stateDir + "/logs/otel-errors.log"
	logData, err := os.ReadFile(otelLogFile)
	if err != nil {
		// Dump diagnostics.
		statusOut, _ := runCmd(projDir, binPath, "status",
			"--run-dir", runDir,
			"--state-dir", stateDir,
		).CombinedOutput()
		t.Logf("tukituki status:\n%s", statusOut)

		emitterLog, _ := os.ReadFile(stateDir + "/logs/emitter.log")
		t.Logf("emitter log:\n%s", emitterLog)

		t.Fatalf("read otel-errors.log: %v", err)
	}

	// Also log the emitter output for diagnostics.
	emitterLog, _ := os.ReadFile(stateDir + "/logs/emitter.log")
	t.Logf("emitter.log content:\n%s", emitterLog)

	output := string(logData)
	t.Logf("otel-errors.log content:\n%s", output)

	// Verify the error log was captured.
	if !strings.Contains(output, "[emitter] database connection refused") {
		t.Errorf("otel-errors log does not contain expected error")
	}

	// Verify info logs were NOT captured.
	for i := 0; i < 20; i++ {
		infoMsg := fmt.Sprintf("[emitter] info log %d", i)
		if strings.Contains(output, infoMsg) {
			t.Errorf("otel-errors log should not contain info log %q", infoMsg)
		}
	}

	// Clean up: stop all processes.
	_ = runCmd(projDir, binPath, "stop",
		"--run-dir", runDir,
		"--state-dir", stateDir,
	).Run()
}

// ─── Test helpers ───────────────────────────────────────────────────────────

type logEntry struct {
	severity logsv1.SeverityNumber
	body     string
}

func buildExportRequest(serviceName string, entries []logEntry) *collogsv1.ExportLogsServiceRequest {
	records := make([]*logsv1.LogRecord, len(entries))
	for i, e := range entries {
		records[i] = &logsv1.LogRecord{
			SeverityNumber: e.severity,
			Body:           &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: e.body}},
		}
	}
	return &collogsv1.ExportLogsServiceRequest{
		ResourceLogs: []*logsv1.ResourceLogs{{
			Resource: &resourcev1.Resource{
				Attributes: []*commonv1.KeyValue{{
					Key:   "service.name",
					Value: &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: serviceName}},
				}},
			},
			ScopeLogs: []*logsv1.ScopeLogs{{
				LogRecords: records,
			}},
		}},
	}
}

func nonEmptyLines(s string) []string {
	var out []string
	for _, l := range strings.Split(s, "\n") {
		if strings.TrimSpace(l) != "" {
			out = append(out, l)
		}
	}
	return out
}

func freePort(t *testing.T) int {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("freePort: %v", err)
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	return port
}

func waitForPort(t *testing.T, port int) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("tcp", fmt.Sprintf("localhost:%d", port), 100*time.Millisecond)
		if err == nil {
			conn.Close()
			return
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatalf("port %d did not become available", port)
}

func projectRoot(t *testing.T) string {
	t.Helper()
	// We're in internal/otel/, so project root is ../../
	return "../../"
}

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

// runCmd is a convenience for exec.Command with Dir set.
func runCmd(dir, name string, args ...string) *exec.Cmd {
	cmd := exec.Command(name, args...)
	cmd.Dir = dir
	return cmd
}
