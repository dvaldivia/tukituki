---
title: OpenTelemetry Error Collection
weight: 3
---

tukituki includes a built-in OTLP log receiver that collects [OpenTelemetry](https://opentelemetry.io/) log records from your development services and surfaces errors in one place. No external collector or agent is required.

## How It Works

When at least one run target has `otel: true`, tukituki automatically:

1. **Starts a bundled OTLP log receiver** as a background process (just like your other targets).
2. **Injects environment variables** into each otel-enabled target:
   - `OTEL_EXPORTER_OTLP_ENDPOINT` pointing to the receiver
3. **Filters incoming log records** by severity (default: ERROR and above).
4. **Displays matching entries** in a virtual **otel-errors** process at the bottom of the TUI sidebar, prefixed with the originating service name.

The receiver process is detached from the TUI, so it survives `q` (detach) and is reconnected when you reattach.

## Enabling OTel

Add `otel: true` to any run target YAML:

```yaml
name: api
description: "HTTP backend"
command: go
args:
  - run
  - ./cmd/server
otel: true
```

Your application must use an OpenTelemetry SDK that exports logs via OTLP. Most OTel SDKs respect the `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable automatically.

## Configuring Severity

By default, only log records with severity **ERROR** and above are shown. Override this with the `--otel-severity` flag or `TUKITUKI_OTEL_SEVERITY` environment variable:

```sh
tukituki --otel-severity warn    # also show warnings
tukituki --otel-severity info    # info and above
tukituki --otel-severity debug   # debug and above
```

Valid levels (from lowest to highest): `trace`, `debug`, `info`, `warn`, `error`, `fatal`.

## Configuring the Protocol

The receiver supports both gRPC and HTTP OTLP transports.

| Flag | Env var | Default | Port |
|---|---|---|---|
| `--otel-protocol grpc` | `TUKITUKI_OTEL_PROTOCOL=grpc` | yes | 4317 |
| `--otel-protocol http` | `TUKITUKI_OTEL_PROTOCOL=http` | no | 4318 |

Override the port with `--otel-port` or `TUKITUKI_OTEL_PORT`:

```sh
tukituki --otel-protocol grpc --otel-port 14317
```

The HTTP receiver accepts `POST /v1/logs` with either `application/x-protobuf` or `application/json` content types.

## Output Format

Filtered log records are displayed as:

```
[service-name] log body text
```

The `service-name` comes from the `service.name` resource attribute in the OTLP payload. Your application's OTel SDK sets this as part of its resource configuration.

## Headless Access

The virtual **otel-errors** target works with all headless subcommands:

```sh
tukituki logs otel-errors                  # print buffered errors and exit
tukituki logs otel-errors --follow         # stream errors in real time
tukituki status otel-errors                # check collector status
tukituki stop otel-errors                  # stop the collector
```

## Configuration File

OTel settings can also be placed in `.tukitukirc.yaml`:

```yaml
otel_protocol: grpc
otel_severity: error
otel_port: 4317
```

## Troubleshooting

**No errors appearing?**
- Confirm your app is exporting OTel **logs** (not just traces or metrics).
- Check the severity threshold -- your logs may be below the filter.
- Verify the collector is running: `tukituki status otel-errors`.
- Check the collector's own log output: `tukituki logs otel-errors`.

**Port already in use?**
- Another OTel collector or service may be using port 4317. Override with `--otel-port`.

**Collector not starting?**
- Ensure at least one target has `otel: true` in its YAML file.
