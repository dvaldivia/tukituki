package main

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"go.opentelemetry.io/contrib/bridges/otelslog"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/exporters/otlp/otlplog/otlploggrpc"
	sdklog "go.opentelemetry.io/otel/sdk/log"
	"go.opentelemetry.io/otel/sdk/resource"
)

func main() {
	ctx := context.Background()

	res, err := resource.New(ctx,
		resource.WithAttributes(attribute.String("service.name", "go-api")),
	)
	if err != nil {
		fmt.Fprintf(os.Stderr, "resource: %v\n", err)
		os.Exit(1)
	}

	// Read endpoint from env and strip scheme — the gRPC client needs host:port.
	endpoint := os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
	if endpoint == "" {
		endpoint = "127.0.0.1:4317"
	}
	endpoint = strings.TrimPrefix(endpoint, "http://")
	endpoint = strings.TrimPrefix(endpoint, "https://")

	exp, err := otlploggrpc.New(ctx,
		otlploggrpc.WithInsecure(),
		otlploggrpc.WithEndpoint(endpoint),
	)
	if err != nil {
		fmt.Fprintf(os.Stderr, "exporter: %v\n", err)
		os.Exit(1)
	}

	provider := sdklog.NewLoggerProvider(
		sdklog.WithProcessor(sdklog.NewSimpleProcessor(exp)),
		sdklog.WithResource(res),
	)
	defer provider.Shutdown(ctx)

	logger := slog.New(otelslog.NewHandler("go-api", otelslog.WithLoggerProvider(provider)))
	slog.SetDefault(logger)

	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		for i := range 10 {
			slog.Info("processing request", "step", i, "method", r.Method)
		}
		slog.Error("database connection refused", "host", "localhost", "port", 5432)
		for i := 10; i < 20; i++ {
			slog.Info("continuing work", "step", i)
		}
		fmt.Fprintln(w, "ok")
	})

	fmt.Println("go-api listening on :8081")
	go http.ListenAndServe(":8081", nil)

	sig := make(chan os.Signal, 1)
	signal.Notify(sig, os.Interrupt, syscall.SIGTERM)
	<-sig
	fmt.Println("go-api shutting down")
}
