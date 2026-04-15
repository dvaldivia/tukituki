// emitter is a test helper that sends OTLP log records to the endpoint
// specified by OTEL_EXPORTER_OTLP_ENDPOINT. It produces 10 INFO logs,
// 1 ERROR log, then 10 more INFO logs, and exits.
package main

import (
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	collogsv1 "go.opentelemetry.io/proto/otlp/collector/logs/v1"
	commonv1 "go.opentelemetry.io/proto/otlp/common/v1"
	logsv1 "go.opentelemetry.io/proto/otlp/logs/v1"
	resourcev1 "go.opentelemetry.io/proto/otlp/resource/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

func main() {
	endpoint := os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
	serviceName := os.Getenv("OTEL_SERVICE_NAME")
	if endpoint == "" {
		fmt.Fprintln(os.Stderr, "OTEL_EXPORTER_OTLP_ENDPOINT not set")
		os.Exit(1)
	}
	if serviceName == "" {
		serviceName = "emitter"
	}

	// Strip http:// prefix — gRPC wants a bare host:port.
	target := strings.TrimPrefix(endpoint, "http://")
	target = strings.TrimPrefix(target, "https://")
	// Replace "localhost" with "127.0.0.1" to avoid slow DNS lookups in gRPC.
	target = strings.Replace(target, "localhost", "127.0.0.1", 1)

	// Retry connection for up to 10 seconds (collector may still be starting).
	var conn *grpc.ClientConn
	var err error
	for attempt := 0; attempt < 20; attempt++ {
		conn, err = grpc.NewClient("dns:///"+target, grpc.WithTransportCredentials(insecure.NewCredentials()))
		if err == nil {
			break
		}
		time.Sleep(500 * time.Millisecond)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "grpc connect %s: %v\n", target, err)
		os.Exit(1)
	}
	defer conn.Close()

	client := collogsv1.NewLogsServiceClient(conn)

	// Build 21 log records: 10 INFO + 1 ERROR + 10 INFO.
	var records []*logsv1.LogRecord
	for i := 0; i < 10; i++ {
		records = append(records, makeRecord(
			logsv1.SeverityNumber_SEVERITY_NUMBER_INFO,
			fmt.Sprintf("info log %d", i),
		))
	}
	records = append(records, makeRecord(
		logsv1.SeverityNumber_SEVERITY_NUMBER_ERROR,
		"database connection refused",
	))
	for i := 10; i < 20; i++ {
		records = append(records, makeRecord(
			logsv1.SeverityNumber_SEVERITY_NUMBER_INFO,
			fmt.Sprintf("info log %d", i),
		))
	}

	req := &collogsv1.ExportLogsServiceRequest{
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

	// Retry the Export call to handle collector startup delay.
	for attempt := 0; attempt < 20; attempt++ {
		_, err = client.Export(context.Background(), req)
		if err == nil {
			break
		}
		time.Sleep(500 * time.Millisecond)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "export: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("emitter: sent 10 INFO + 1 ERROR + 10 INFO logs")
}

func makeRecord(severity logsv1.SeverityNumber, body string) *logsv1.LogRecord {
	return &logsv1.LogRecord{
		SeverityNumber: severity,
		Body:           &commonv1.AnyValue{Value: &commonv1.AnyValue_StringValue{StringValue: body}},
	}
}
