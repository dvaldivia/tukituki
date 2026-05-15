//! Tiny OTLP emitter used by the live integration test.
//!
//! Reads `OTEL_EXPORTER_OTLP_ENDPOINT` from the environment (injected by
//! the Manager when a target has `otel: true`), waits up to 10 seconds
//! for the collector to come up, then ships 10 INFO logs, 1 ERROR, and
//! another 10 INFO. Mirrors the Go test emitter under
//! `internal/otel/testdata/emitter/`.

use std::time::Duration;

use tonic::transport::Endpoint;

use tukituki_otel::proto::collector::logs::v1::ExportLogsServiceRequest;
use tukituki_otel::proto::collector::logs::v1::logs_service_client::LogsServiceClient;
use tukituki_otel::proto::common::v1::{AnyValue, KeyValue, any_value::Value as AnyVal};
use tukituki_otel::proto::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use tukituki_otel::proto::resource::v1::Resource;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .map_err(|_| "OTEL_EXPORTER_OTLP_ENDPOINT not set")?;
    eprintln!("[emitter] endpoint = {endpoint}");

    // Retry the gRPC dial for up to 10 seconds — the collector child
    // may still be coming up when our shell wrapper launches us.
    let mut last_err = None;
    let channel = loop_until(Duration::from_secs(10), || async {
        match Endpoint::from_shared(endpoint.clone())
            .map_err(|e| e.to_string())?
            .connect()
            .await
        {
            Ok(c) => Ok(c),
            Err(e) => Err(e.to_string()),
        }
    })
    .await
    .map_err(|e| {
        last_err = Some(e.clone());
        format!("dial OTLP collector: {e}")
    })?;
    let _ = last_err; // silence unused warning when retries succeed

    let mut client = LogsServiceClient::new(channel);

    let req = build_request("emitter");
    client.export(req).await?;
    eprintln!("[emitter] export ok");
    // Give the gRPC framing a beat to flush before our process exits.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}

fn build_request(service: &str) -> ExportLogsServiceRequest {
    let mut records: Vec<LogRecord> = (0..10)
        .map(|i| log_record(SeverityNumber::Info, &format!("info log {i}")))
        .collect();
    records.push(log_record(
        SeverityNumber::Error,
        "database connection refused",
    ));
    for i in 10..20 {
        records.push(log_record(SeverityNumber::Info, &format!("info log {i}")));
    }
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".into(),
                    value: Some(AnyValue {
                        value: Some(AnyVal::StringValue(service.into())),
                    }),
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

fn log_record(sev: SeverityNumber, body: &str) -> LogRecord {
    LogRecord {
        severity_number: sev as i32,
        body: Some(AnyValue {
            value: Some(AnyVal::StringValue(body.into())),
        }),
        ..Default::default()
    }
}

async fn loop_until<F, Fut, T>(timeout: Duration, mut f: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let deadline = std::time::Instant::now() + timeout;
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = e;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    Err(last)
}
