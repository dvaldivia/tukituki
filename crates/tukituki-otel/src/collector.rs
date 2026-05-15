//! OTLP log receiver — gRPC + HTTP.
//!
//! Direct port of `internal/otel/collector.go`. The output format
//! (`renderLogRecord`) is reproduced byte-for-byte so a side-by-side
//! `otel-errors.log` diff between the Go and Rust binaries stays clean.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use chrono::DateTime;
use prost::Message;
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::oneshot;
use tonic::transport::Server;

use crate::notify::NotifyHub;
use crate::proto::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
    logs_service_server::{LogsService, LogsServiceServer},
};
use crate::proto::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    metrics_service_server::{MetricsService, MetricsServiceServer},
};
use crate::proto::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
    trace_service_server::{TraceService, TraceServiceServer},
};
use crate::proto::logs::v1::SeverityNumber;
use crate::proto::notify_v1::ErrorEvent;
use crate::proto::notify_v1::notifier_server::NotifierServer;
use crate::severity::severity_label;
use crate::value::{any_value_to_string, extract_service_name, filter_resource_attrs};

/// Multi-line block separator that precedes every rendered log record.
/// Same byte content as Go's `logSeparator`.
pub const LOG_SEPARATOR: &str = "------------------------------------------------------------";

/// Configuration for the OTLP receiver.
pub struct Collector {
    pub port: u16,
    pub protocol: String, // "grpc" | "http"
    pub min_severity: SeverityNumber,
    /// Where to write rendered output. Defaults to stdout.  When run as
    /// the Manager-spawned `otel-errors` child this is captured into
    /// `<state>/logs/otel-errors.log` via the usual `stdout/stderr → file`
    /// redirection.
    pub output: Arc<StdMutex<Box<dyn std::io::Write + Send>>>,
    /// Optional UDS path that, when set, gates a `Notifier` gRPC server
    /// for live TUI consumption.
    pub notify_socket: Option<PathBuf>,
}

impl Collector {
    pub fn new(port: u16, protocol: String, min_severity: SeverityNumber) -> Self {
        Self {
            port,
            protocol,
            min_severity,
            output: Arc::new(StdMutex::new(Box::new(std::io::stdout()))),
            notify_socket: None,
        }
    }

    /// Run the receiver until `cancel` fires. Returns on shutdown.
    pub async fn run(self, cancel: oneshot::Receiver<()>) -> Result<(), String> {
        let hub = NotifyHub::new();

        // Optional UDS notifier — started first so subscribers can
        // attach before the OTLP receiver opens for business.
        let notify_handle = if let Some(socket) = self.notify_socket.clone() {
            Some(
                spawn_notify_server(socket, hub.clone())
                    .await
                    .map_err(|e| format!("start notifier: {e}"))?,
            )
        } else {
            None
        };

        let result = match self.protocol.as_str() {
            "grpc" => run_grpc(&self, hub.clone(), cancel).await,
            "http" => run_http(&self, hub.clone(), cancel).await,
            other => Err(format!("unsupported protocol {other:?}")),
        };

        // Close subscriber channels FIRST so any in-flight `Subscribe`
        // streams complete; otherwise tonic's graceful shutdown of the
        // notify UDS server waits forever on a still-open stream.
        hub.shutdown();
        if let Some(handle) = notify_handle {
            handle.shutdown().await;
        }
        result
    }
}

// ---------------------------------------------------------------------
// gRPC path
// ---------------------------------------------------------------------

async fn run_grpc(
    c: &Collector,
    hub: NotifyHub,
    cancel: oneshot::Receiver<()>,
) -> Result<(), String> {
    let addr: SocketAddr = format!("127.0.0.1:{}", c.port)
        .parse()
        .map_err(|e| format!("parse addr: {e}"))?;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("listen on port {}: {e}", c.port))?;

    write_line(
        &c.output,
        &format!(
            "otel-collector: listening gRPC on :{} (min severity: {})",
            c.port,
            severity_label(c.min_severity)
        ),
    );

    let logs = LogsHandler {
        min_severity: c.min_severity,
        output: c.output.clone(),
        hub: hub.clone(),
    };

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    Server::builder()
        .add_service(LogsServiceServer::new(logs))
        .add_service(MetricsServiceServer::new(NoopMetrics))
        .add_service(TraceServiceServer::new(NoopTrace))
        .serve_with_incoming_shutdown(incoming, async {
            let _ = cancel.await;
        })
        .await
        .map_err(|e| format!("grpc serve: {e}"))
}

// ---------------------------------------------------------------------
// HTTP path
// ---------------------------------------------------------------------

#[derive(Clone)]
struct HttpState {
    min_severity: SeverityNumber,
    output: Arc<StdMutex<Box<dyn std::io::Write + Send>>>,
    hub: NotifyHub,
}

async fn run_http(
    c: &Collector,
    hub: NotifyHub,
    cancel: oneshot::Receiver<()>,
) -> Result<(), String> {
    let state = HttpState {
        min_severity: c.min_severity,
        output: c.output.clone(),
        hub,
    };
    let app = Router::new()
        .route("/v1/logs", post(http_logs_handler))
        .route("/v1/metrics", post(http_noop_handler))
        .route("/v1/traces", post(http_noop_handler))
        .with_state(state);

    let addr: SocketAddr = format!("127.0.0.1:{}", c.port)
        .parse()
        .map_err(|e| format!("parse addr: {e}"))?;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("listen on port {}: {e}", c.port))?;

    write_line(
        &c.output,
        &format!(
            "otel-collector: listening HTTP on :{} (min severity: {})",
            c.port,
            severity_label(c.min_severity)
        ),
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = cancel.await;
        })
        .await
        .map_err(|e| format!("http serve: {e}"))
}

async fn http_logs_handler(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let req: ExportLogsServiceRequest = match headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
    {
        ct if ct.contains("json") => {
            // The Go binary accepts protojson — we don't (decoding
            // protojson without the descriptor at runtime is a lot of
            // code for a code path the SDKs don't routinely exercise).
            // Return 400 for now; faithful enough since real exporters
            // use application/x-protobuf by default.
            return (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                String::from(r#"{"error":"json content-type unsupported in this build"}"#),
            )
                .into_response();
        }
        _ => match ExportLogsServiceRequest::decode(body) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    [("content-type", "text/plain")],
                    format!("decode protobuf: {e}"),
                )
                    .into_response();
            }
        },
    };

    process_export_request(&state.output, &req, state.min_severity, Some(&state.hub));

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        "{}".to_string(),
    )
        .into_response()
}

async fn http_noop_handler(body: Bytes) -> impl IntoResponse {
    let _ = body;
    (StatusCode::OK, [("content-type", "application/json")], "{}")
}

// ---------------------------------------------------------------------
// gRPC service impls
// ---------------------------------------------------------------------

struct LogsHandler {
    min_severity: SeverityNumber,
    output: Arc<StdMutex<Box<dyn std::io::Write + Send>>>,
    hub: NotifyHub,
}

#[tonic::async_trait]
impl LogsService for LogsHandler {
    async fn export(
        &self,
        request: tonic::Request<ExportLogsServiceRequest>,
    ) -> Result<tonic::Response<ExportLogsServiceResponse>, tonic::Status> {
        let req = request.into_inner();
        process_export_request(&self.output, &req, self.min_severity, Some(&self.hub));
        Ok(tonic::Response::new(ExportLogsServiceResponse::default()))
    }
}

struct NoopMetrics;

#[tonic::async_trait]
impl MetricsService for NoopMetrics {
    async fn export(
        &self,
        _request: tonic::Request<ExportMetricsServiceRequest>,
    ) -> Result<tonic::Response<ExportMetricsServiceResponse>, tonic::Status> {
        Ok(tonic::Response::new(ExportMetricsServiceResponse::default()))
    }
}

struct NoopTrace;

#[tonic::async_trait]
impl TraceService for NoopTrace {
    async fn export(
        &self,
        _request: tonic::Request<ExportTraceServiceRequest>,
    ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
        Ok(tonic::Response::new(ExportTraceServiceResponse::default()))
    }
}

// ---------------------------------------------------------------------
// Notify UDS server
// ---------------------------------------------------------------------

struct NotifyServerHandle {
    cancel: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
    socket_path: PathBuf,
}

impl NotifyServerHandle {
    async fn shutdown(mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = j.await;
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

async fn spawn_notify_server(
    socket: PathBuf,
    hub: NotifyHub,
) -> std::io::Result<NotifyServerHandle> {
    // Clean up any stale socket left behind by a crashed prior collector.
    let _ = std::fs::remove_file(&socket);
    if let Some(dir) = socket.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(&socket)?;
    let stream = tokio_stream::wrappers::UnixListenerStream::new(listener);
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(NotifierServer::new(hub))
            .serve_with_incoming_shutdown(stream, async {
                let _ = cancel_rx.await;
            })
            .await;
    });
    Ok(NotifyServerHandle {
        cancel: Some(cancel_tx),
        join: Some(join),
        socket_path: socket,
    })
}

// ---------------------------------------------------------------------
// Shared processing
// ---------------------------------------------------------------------

pub fn process_export_request(
    output: &Arc<StdMutex<Box<dyn std::io::Write + Send>>>,
    req: &ExportLogsServiceRequest,
    min_severity: SeverityNumber,
    hub: Option<&NotifyHub>,
) {
    for rl in &req.resource_logs {
        let resource_attrs = rl
            .resource
            .as_ref()
            .map(|r| r.attributes.as_slice())
            .unwrap_or(&[]);
        let mut service_name = extract_service_name(resource_attrs).to_string();
        if service_name.is_empty() {
            service_name = "unknown".to_string();
        }
        for sl in &rl.scope_logs {
            for lr in &sl.log_records {
                if lr.severity_number < min_severity as i32 {
                    continue;
                }
                let body = any_value_to_string(lr.body.as_ref());
                if body.is_empty() {
                    continue;
                }
                render_log_record(output, &service_name, resource_attrs, lr, &body);

                if let Some(hub) = hub {
                    let mut ts = lr.time_unix_nano;
                    if ts == 0 {
                        ts = lr.observed_time_unix_nano;
                    }
                    let sev = if lr.severity_text.is_empty() {
                        severity_label(lr_severity(lr)).to_string()
                    } else {
                        lr.severity_text.clone()
                    };
                    hub.publish(ErrorEvent {
                        timestamp_unix_nano: ts as i64,
                        service_name: service_name.clone(),
                        severity: sev,
                        body: body.clone(),
                    });
                }
            }
        }
    }
}

fn lr_severity(lr: &crate::proto::logs::v1::LogRecord) -> SeverityNumber {
    SeverityNumber::try_from(lr.severity_number).unwrap_or(SeverityNumber::Unspecified)
}

fn render_log_record(
    output: &Arc<StdMutex<Box<dyn std::io::Write + Send>>>,
    service_name: &str,
    resource_attrs: &[crate::proto::common::v1::KeyValue],
    lr: &crate::proto::logs::v1::LogRecord,
    body: &str,
) {
    let mut buf = String::new();
    buf.push_str(LOG_SEPARATOR);
    buf.push('\n');

    let mut ts = lr.time_unix_nano;
    if ts == 0 {
        ts = lr.observed_time_unix_nano;
    }
    let sev_label = if lr.severity_text.is_empty() {
        severity_label(lr_severity(lr)).to_string()
    } else {
        lr.severity_text.clone()
    };

    if ts != 0 {
        let dt = ts_to_rfc3339(ts);
        buf.push_str(&format!("{dt}  {sev_label}\n"));
    } else {
        buf.push_str(&sev_label);
        buf.push('\n');
    }
    buf.push_str(&format!("[{service_name}] {body}\n"));

    if !lr.trace_id.is_empty() {
        buf.push_str(&format!("  trace_id={}\n", hex_lower(&lr.trace_id)));
    }
    if !lr.span_id.is_empty() {
        buf.push_str(&format!("  span_id={}\n", hex_lower(&lr.span_id)));
    }

    let extra = filter_resource_attrs(resource_attrs);
    if !extra.is_empty() {
        buf.push_str("  resource:\n");
        for kv in extra {
            buf.push_str(&format!(
                "    {}={}\n",
                kv.key,
                any_value_to_string(kv.value.as_ref())
            ));
        }
    }
    if !lr.attributes.is_empty() {
        buf.push_str("  attributes:\n");
        for kv in &lr.attributes {
            buf.push_str(&format!(
                "    {}={}\n",
                kv.key,
                any_value_to_string(kv.value.as_ref())
            ));
        }
    }

    let mut guard = output.lock().unwrap_or_else(|p| p.into_inner());
    let _ = guard.write_all(buf.as_bytes());
    let _ = guard.flush();
}

fn write_line(output: &Arc<StdMutex<Box<dyn std::io::Write + Send>>>, line: &str) {
    let mut guard = output.lock().unwrap_or_else(|p| p.into_inner());
    let _ = writeln!(guard, "{line}");
    let _ = guard.flush();
}

/// `time.Unix(0, int64(ts)).UTC().Format(time.RFC3339Nano)` analogue.
fn ts_to_rfc3339(unix_nanos: u64) -> String {
    let secs = (unix_nanos / 1_000_000_000) as i64;
    let nanos = (unix_nanos % 1_000_000_000) as u32;
    DateTime::from_timestamp(secs, nanos)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .unwrap_or_default()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// Re-export so consumers can construct a Collector with stdout output
// via a single `Collector::new(...).run(...)` call.
//
// Tests use `Collector::new_with_output(...)` to capture output in a
// shared buffer.
impl Collector {
    pub fn new_with_output(
        port: u16,
        protocol: String,
        min_severity: SeverityNumber,
        output: Box<dyn std::io::Write + Send>,
    ) -> Self {
        Self {
            port,
            protocol,
            min_severity,
            output: Arc::new(StdMutex::new(output)),
            notify_socket: None,
        }
    }
}

// `axum::http` re-export so we can name `http::header::CONTENT_TYPE` in
// the handler signature without an extra dep in Cargo.toml.
use http;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::common::v1::{AnyValue, KeyValue, any_value::Value as AnyVal};
    use crate::proto::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use crate::proto::resource::v1::Resource;

    fn av_string(s: &str) -> AnyValue {
        AnyValue {
            value: Some(AnyVal::StringValue(s.into())),
        }
    }

    fn build_export_request(
        service: &str,
        entries: &[(SeverityNumber, &str)],
    ) -> ExportLogsServiceRequest {
        let records = entries
            .iter()
            .map(|(sev, body)| LogRecord {
                severity_number: *sev as i32,
                body: Some(av_string(body)),
                ..Default::default()
            })
            .collect();
        ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".into(),
                        value: Some(av_string(service)),
                    }],
                    ..Default::default()
                }),
                scope_logs: vec![ScopeLogs {
                    log_records: records,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    type SharedWriter = Arc<StdMutex<Box<dyn std::io::Write + Send>>>;
    type SharedBuf = Arc<StdMutex<Vec<u8>>>;

    fn buffer() -> (SharedWriter, SharedBuf) {
        let inner = Arc::new(StdMutex::new(Vec::<u8>::new()));
        let inner_clone = inner.clone();
        let writer: Box<dyn std::io::Write + Send> = Box::new(VecWriter(inner_clone));
        (Arc::new(StdMutex::new(writer)), inner)
    }

    struct VecWriter(Arc<StdMutex<Vec<u8>>>);
    impl std::io::Write for VecWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
            g.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn header_lines(s: &str) -> Vec<&str> {
        s.lines().filter(|l| l.starts_with('[')).collect()
    }

    #[test]
    fn process_export_filters_below_threshold() {
        let (output, buf) = buffer();
        let req = build_export_request(
            "test-svc",
            &[
                (SeverityNumber::Info, "just info"),
                (SeverityNumber::Debug, "debug msg"),
                (SeverityNumber::Warn, "a warning"),
            ],
        );
        process_export_request(&output, &req, SeverityNumber::Error, None);
        let raw = buf.lock().unwrap();
        assert!(
            raw.is_empty(),
            "expected no output, got {:?}",
            String::from_utf8_lossy(&raw)
        );
    }

    #[test]
    fn process_export_passes_above_threshold() {
        let (output, buf) = buffer();
        let req = build_export_request(
            "my-api",
            &[
                (SeverityNumber::Info, "info 1"),
                (SeverityNumber::Error, "something broke"),
                (SeverityNumber::Fatal, "panic"),
                (SeverityNumber::Info, "info 2"),
            ],
        );
        process_export_request(&output, &req, SeverityNumber::Error, None);
        let raw = buf.lock().unwrap().clone();
        let text = String::from_utf8(raw).unwrap();
        let lines = header_lines(&text);
        assert_eq!(lines, vec!["[my-api] something broke", "[my-api] panic"]);
    }

    #[test]
    fn process_export_ten_info_one_error_ten_info() {
        let (output, buf) = buffer();
        let mut entries: Vec<(SeverityNumber, String)> = (0..10)
            .map(|i| (SeverityNumber::Info, format!("info log {i}")))
            .collect();
        entries.push((SeverityNumber::Error, "database connection refused".into()));
        for i in 10..20 {
            entries.push((SeverityNumber::Info, format!("info log {i}")));
        }
        let entries_ref: Vec<(SeverityNumber, &str)> =
            entries.iter().map(|(s, b)| (*s, b.as_str())).collect();
        let req = build_export_request("api", &entries_ref);
        process_export_request(&output, &req, SeverityNumber::Error, None);
        let raw = buf.lock().unwrap().clone();
        let text = String::from_utf8(raw).unwrap();
        let lines = header_lines(&text);
        assert_eq!(lines, vec!["[api] database connection refused"]);
    }

    #[test]
    fn process_export_unknown_service_name() {
        let (output, buf) = buffer();
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource::default()),
                scope_logs: vec![ScopeLogs {
                    log_records: vec![LogRecord {
                        severity_number: SeverityNumber::Error as i32,
                        body: Some(av_string("boom")),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        process_export_request(&output, &req, SeverityNumber::Error, None);
        let raw = buf.lock().unwrap().clone();
        let text = String::from_utf8(raw).unwrap();
        let lines = header_lines(&text);
        assert_eq!(lines, vec!["[unknown] boom"]);
    }
}
