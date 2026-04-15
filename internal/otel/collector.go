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

	collogsv1 "go.opentelemetry.io/proto/otlp/collector/logs/v1"
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

// processExportRequest iterates over the OTLP log export request, filters by
// severity, and writes matching records to out.
func processExportRequest(out io.Writer, req *collogsv1.ExportLogsServiceRequest, minSeverity logsv1.SeverityNumber) {
	for _, rl := range req.GetResourceLogs() {
		serviceName := extractServiceName(rl.GetResource().GetAttributes())
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
				fmt.Fprintf(out, "[%s] %s\n", serviceName, body)
			}
		}
	}
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
