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
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	collogsv1 "go.opentelemetry.io/proto/otlp/collector/logs/v1"
	colmetricsv1 "go.opentelemetry.io/proto/otlp/collector/metrics/v1"
	coltracev1 "go.opentelemetry.io/proto/otlp/collector/trace/v1"
	commonv1 "go.opentelemetry.io/proto/otlp/common/v1"
	logsv1 "go.opentelemetry.io/proto/otlp/logs/v1"
	"google.golang.org/grpc"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

// Collector is a lightweight OTLP log receiver that filters log records by
// severity and writes matching entries to an output writer (default: stdout).
type Collector struct {
	Port        int
	Protocol    string // "grpc" or "http"
	MinSeverity logsv1.SeverityNumber
	Output      io.Writer // defaults to os.Stdout if nil

	grpcServer *grpc.Server
	httpServer *http.Server
}

func (c *Collector) output() io.Writer {
	if c.Output != nil {
		return c.Output
	}
	return os.Stdout
}

// Run starts the OTLP receiver and blocks until ctx is cancelled.
func (c *Collector) Run(ctx context.Context) error {
	switch c.Protocol {
	case "grpc":
		return c.runGRPC(ctx)
	case "http":
		return c.runHTTP(ctx)
	default:
		return fmt.Errorf("unsupported protocol %q", c.Protocol)
	}
}

// ─── gRPC receiver ──────────────────────────────────────────────────────────

func (c *Collector) runGRPC(ctx context.Context) error {
	lis, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", c.Port))
	if err != nil {
		return fmt.Errorf("listen on port %d: %w", c.Port, err)
	}

	c.grpcServer = grpc.NewServer()
	collogsv1.RegisterLogsServiceServer(c.grpcServer, &logsHandler{
		minSeverity: c.MinSeverity,
		out:         c.output(),
	})
	// Register no-op metrics/trace services so SDKs that auto-export
	// all signals don't spam "Unimplemented" errors on every interval.
	colmetricsv1.RegisterMetricsServiceServer(c.grpcServer, &noopMetricsHandler{})
	coltracev1.RegisterTraceServiceServer(c.grpcServer, &noopTraceHandler{})

	fmt.Fprintf(c.output(), "otel-collector: listening gRPC on :%d (min severity: %s)\n",
		c.Port, c.MinSeverity.String())

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		<-ctx.Done()
		c.grpcServer.GracefulStop()
	}()

	err = c.grpcServer.Serve(lis)
	wg.Wait()
	return err
}

// ─── HTTP receiver ──────────────────────────────────────────────────────────

func (c *Collector) runHTTP(ctx context.Context) error {
	handler := &httpLogsHandler{minSeverity: c.MinSeverity, out: c.output()}

	mux := http.NewServeMux()
	mux.Handle("/v1/logs", handler)
	// No-op metrics/trace endpoints: accept any payload and return {}.
	mux.HandleFunc("/v1/metrics", noopHTTPHandler)
	mux.HandleFunc("/v1/traces", noopHTTPHandler)

	c.httpServer = &http.Server{
		Addr:    fmt.Sprintf("127.0.0.1:%d", c.Port),
		Handler: mux,
	}

	fmt.Fprintf(c.output(), "otel-collector: listening HTTP on :%d (min severity: %s)\n",
		c.Port, c.MinSeverity.String())

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		<-ctx.Done()
		c.httpServer.Close()
	}()

	err := c.httpServer.ListenAndServe()
	wg.Wait()
	if err == http.ErrServerClosed {
		return nil
	}
	return err
}

// ─── gRPC handler ───────────────────────────────────────────────────────────

type logsHandler struct {
	collogsv1.UnimplementedLogsServiceServer
	minSeverity logsv1.SeverityNumber
	out         io.Writer
}

func (h *logsHandler) Export(_ context.Context, req *collogsv1.ExportLogsServiceRequest) (*collogsv1.ExportLogsServiceResponse, error) {
	processExportRequest(h.out, req, h.minSeverity)
	return &collogsv1.ExportLogsServiceResponse{}, nil
}

// ─── No-op metrics/trace handlers ──────────────────────────────────────────
//
// The collector intentionally discards metrics and traces; it exists to
// surface error logs only. Registering stub services prevents SDKs with
// auto-exporters for all signals from logging "Unimplemented" errors.

type noopMetricsHandler struct {
	colmetricsv1.UnimplementedMetricsServiceServer
}

func (noopMetricsHandler) Export(context.Context, *colmetricsv1.ExportMetricsServiceRequest) (*colmetricsv1.ExportMetricsServiceResponse, error) {
	return &colmetricsv1.ExportMetricsServiceResponse{}, nil
}

type noopTraceHandler struct {
	coltracev1.UnimplementedTraceServiceServer
}

func (noopTraceHandler) Export(context.Context, *coltracev1.ExportTraceServiceRequest) (*coltracev1.ExportTraceServiceResponse, error) {
	return &coltracev1.ExportTraceServiceResponse{}, nil
}

func noopHTTPHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	_, _ = io.Copy(io.Discard, r.Body)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	fmt.Fprint(w, "{}")
}

// ─── HTTP handler ───────────────────────────────────────────────────────────

type httpLogsHandler struct {
	minSeverity logsv1.SeverityNumber
	out         io.Writer
}

func (h *httpLogsHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "read body: "+err.Error(), http.StatusBadRequest)
		return
	}

	req := &collogsv1.ExportLogsServiceRequest{}
	ct := r.Header.Get("Content-Type")
	switch {
	case strings.Contains(ct, "json"):
		if err := protojson.Unmarshal(body, req); err != nil {
			http.Error(w, "decode json: "+err.Error(), http.StatusBadRequest)
			return
		}
	default:
		// Default to protobuf (application/x-protobuf or empty).
		if err := proto.Unmarshal(body, req); err != nil {
			http.Error(w, "decode protobuf: "+err.Error(), http.StatusBadRequest)
			return
		}
	}

	processExportRequest(h.out, req, h.minSeverity)

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	fmt.Fprint(w, "{}")
}

// ─── Shared processing ─────────────────────────────────────────────────────

// logSeparator precedes every rendered log block so the TUI log viewer (and
// anyone tailing the file) can visually delimit records.
const logSeparator = "------------------------------------------------------------"

// processExportRequest iterates over the OTLP log export request, filters by
// severity, and writes matching records to out.
func processExportRequest(out io.Writer, req *collogsv1.ExportLogsServiceRequest, minSeverity logsv1.SeverityNumber) {
	for _, rl := range req.GetResourceLogs() {
		resourceAttrs := rl.GetResource().GetAttributes()
		serviceName := extractServiceName(resourceAttrs)
		if serviceName == "" {
			serviceName = "unknown"
		}
		for _, sl := range rl.GetScopeLogs() {
			for _, lr := range sl.GetLogRecords() {
				if lr.GetSeverityNumber() < minSeverity {
					continue
				}
				body := anyValueToString(lr.GetBody())
				if body == "" {
					continue
				}
				renderLogRecord(out, serviceName, resourceAttrs, lr, body)
			}
		}
	}
}

// renderLogRecord writes a multi-line block for a single OTLP log record.
// Format:
//
//	------------------------------------------------------------
//	2026-04-18T10:30:45.123Z  ERROR
//	[service-name] log body
//	  trace_id=...
//	  span_id=...
//	  resource:
//	    host.name=...
//	  attributes:
//	    key=value
func renderLogRecord(out io.Writer, serviceName string, resourceAttrs []*commonv1.KeyValue, lr *logsv1.LogRecord, body string) {
	fmt.Fprintln(out, logSeparator)

	ts := lr.GetTimeUnixNano()
	if ts == 0 {
		ts = lr.GetObservedTimeUnixNano()
	}
	sev := lr.GetSeverityText()
	if sev == "" {
		sev = severityLabel(lr.GetSeverityNumber())
	}
	if ts != 0 {
		fmt.Fprintf(out, "%s  %s\n", time.Unix(0, int64(ts)).UTC().Format(time.RFC3339Nano), sev)
	} else {
		fmt.Fprintln(out, sev)
	}

	fmt.Fprintf(out, "[%s] %s\n", serviceName, body)

	if traceID := lr.GetTraceId(); len(traceID) > 0 {
		fmt.Fprintf(out, "  trace_id=%x\n", traceID)
	}
	if spanID := lr.GetSpanId(); len(spanID) > 0 {
		fmt.Fprintf(out, "  span_id=%x\n", spanID)
	}

	extraResource := filterResourceAttrs(resourceAttrs)
	if len(extraResource) > 0 {
		fmt.Fprintln(out, "  resource:")
		for _, kv := range extraResource {
			fmt.Fprintf(out, "    %s=%s\n", kv.GetKey(), anyValueToString(kv.GetValue()))
		}
	}

	if attrs := lr.GetAttributes(); len(attrs) > 0 {
		fmt.Fprintln(out, "  attributes:")
		for _, kv := range attrs {
			fmt.Fprintf(out, "    %s=%s\n", kv.GetKey(), anyValueToString(kv.GetValue()))
		}
	}
}

// severityLabel strips the "SEVERITY_NUMBER_" prefix from the enum name so
// output is "ERROR" instead of "SEVERITY_NUMBER_ERROR".
func severityLabel(n logsv1.SeverityNumber) string {
	return strings.TrimPrefix(n.String(), "SEVERITY_NUMBER_")
}

// filterResourceAttrs returns resource attributes excluding "service.name"
// (which is already shown in the header line).
func filterResourceAttrs(attrs []*commonv1.KeyValue) []*commonv1.KeyValue {
	out := make([]*commonv1.KeyValue, 0, len(attrs))
	for _, kv := range attrs {
		if kv.GetKey() == "service.name" {
			continue
		}
		out = append(out, kv)
	}
	return out
}

// extractServiceName finds the "service.name" attribute in a resource's
// attribute list.
func extractServiceName(attrs []*commonv1.KeyValue) string {
	for _, kv := range attrs {
		if kv.GetKey() == "service.name" {
			return kv.GetValue().GetStringValue()
		}
	}
	return ""
}

// anyValueToString extracts a human-readable string from an OTLP AnyValue.
func anyValueToString(v *commonv1.AnyValue) string {
	if v == nil {
		return ""
	}
	switch val := v.GetValue().(type) {
	case *commonv1.AnyValue_StringValue:
		return val.StringValue
	case *commonv1.AnyValue_IntValue:
		return fmt.Sprintf("%d", val.IntValue)
	case *commonv1.AnyValue_DoubleValue:
		return fmt.Sprintf("%g", val.DoubleValue)
	case *commonv1.AnyValue_BoolValue:
		return fmt.Sprintf("%t", val.BoolValue)
	default:
		return fmt.Sprintf("%v", v)
	}
}
