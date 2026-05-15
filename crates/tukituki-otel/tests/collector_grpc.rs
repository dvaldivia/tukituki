//! Live gRPC + HTTP integration tests for the Collector.
//!
//! Each test allocates a fresh port, spawns the collector inside a
//! tokio runtime, drives it via a real gRPC / HTTP client, and asserts
//! the rendered output matches Go's. The notify-socket test also
//! attaches a subscriber and verifies live event delivery.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use prost::Message;
use tokio::sync::oneshot;
use tokio::time::sleep;
use tonic::transport::Endpoint;

use tukituki_otel::Collector;
use tukituki_otel::proto::collector::logs::v1::ExportLogsServiceRequest;
use tukituki_otel::proto::collector::logs::v1::logs_service_client::LogsServiceClient;
use tukituki_otel::proto::common::v1::{AnyValue, KeyValue, any_value::Value as AnyVal};
use tukituki_otel::proto::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use tukituki_otel::proto::notify_v1::SubscribeRequest;
use tukituki_otel::proto::notify_v1::notifier_client::NotifierClient;
use tukituki_otel::proto::resource::v1::Resource;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn av_string(s: &str) -> AnyValue {
    AnyValue {
        value: Some(AnyVal::StringValue(s.into())),
    }
}

fn build_request(service: &str, entries: &[(SeverityNumber, &str)]) -> ExportLogsServiceRequest {
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

struct SharedBuf(Arc<StdMutex<Vec<u8>>>);
impl std::io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        g.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn wait_for_port(port: u16) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("port {port} never opened");
}

async fn wait_for_socket(path: &std::path::Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if path.exists() {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("unix socket {} never appeared", path.display());
}

#[tokio::test]
async fn collector_grpc_filters_and_renders() {
    let port = free_port();
    let buf = Arc::new(StdMutex::new(Vec::<u8>::new()));
    let writer = Box::new(SharedBuf(buf.clone()));
    let c = Collector::new_with_output(port, "grpc".into(), SeverityNumber::Error, writer);

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(c.run(cancel_rx));

    wait_for_port(port).await;

    let endpoint = Endpoint::from_shared(format!("http://127.0.0.1:{port}")).unwrap();
    let channel = endpoint.connect().await.expect("dial collector");
    let mut client = LogsServiceClient::new(channel);

    let mut entries: Vec<(SeverityNumber, String)> = (0..10)
        .map(|i| (SeverityNumber::Info, format!("info {i}")))
        .collect();
    entries.push((SeverityNumber::Error, "critical failure in database".into()));
    for i in 10..20 {
        entries.push((SeverityNumber::Info, format!("info {i}")));
    }
    let entries_ref: Vec<(SeverityNumber, &str)> =
        entries.iter().map(|(s, b)| (*s, b.as_str())).collect();
    let req = build_request("my-service", &entries_ref);
    client.export(req).await.expect("Export");

    sleep(Duration::from_millis(100)).await;
    let _ = cancel_tx.send(());
    let _ = handle.await;

    let raw = buf.lock().unwrap().clone();
    let text = String::from_utf8(raw).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[my-service] critical failure in database"]);
}

#[tokio::test]
async fn collector_http_filters_and_renders() {
    let port = free_port();
    let buf = Arc::new(StdMutex::new(Vec::<u8>::new()));
    let writer = Box::new(SharedBuf(buf.clone()));
    let c = Collector::new_with_output(port, "http".into(), SeverityNumber::Error, writer);

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(c.run(cancel_rx));

    wait_for_port(port).await;

    let mut entries: Vec<(SeverityNumber, String)> = (0..10)
        .map(|i| (SeverityNumber::Info, format!("http info {i}")))
        .collect();
    entries.push((
        SeverityNumber::Error,
        "http error: connection timeout".into(),
    ));
    for i in 10..20 {
        entries.push((SeverityNumber::Info, format!("http info {i}")));
    }
    let entries_ref: Vec<(SeverityNumber, &str)> =
        entries.iter().map(|(s, b)| (*s, b.as_str())).collect();
    let req = build_request("web-frontend", &entries_ref);
    let body = req.encode_to_vec();

    let resp = reqwest_lite_post(&format!("http://127.0.0.1:{port}/v1/logs"), body).await;
    assert_eq!(resp.0, 200, "status {} body {}", resp.0, resp.1);

    sleep(Duration::from_millis(100)).await;
    let _ = cancel_tx.send(());
    let _ = handle.await;

    let raw = buf.lock().unwrap().clone();
    let text = String::from_utf8(raw).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[web-frontend] http error: connection timeout"]);
}

#[tokio::test]
async fn collector_notify_socket_streams_errors() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("otel-notify.sock");

    let buf = Arc::new(StdMutex::new(Vec::<u8>::new()));
    let writer = Box::new(SharedBuf(buf.clone()));
    let mut c = Collector::new_with_output(port, "grpc".into(), SeverityNumber::Error, writer);
    c.notify_socket = Some(socket.clone());

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(c.run(cancel_rx));

    wait_for_port(port).await;
    wait_for_socket(&socket).await;

    // Attach a notify subscriber over UDS. tonic has no built-in UDS
    // support, so a custom `tower::service_fn` connector converts each
    // dial into a `TokioIo<UnixStream>`. The URI is purely cosmetic
    // (used for the HTTP/2 :authority header).
    let socket_clone = socket.clone();
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let path = socket_clone.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(&path).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
        .expect("dial unix socket");

    let mut notify_client = NotifierClient::new(channel);
    let mut stream = notify_client
        .subscribe(SubscribeRequest {})
        .await
        .expect("Subscribe")
        .into_inner();

    // Wait a beat to make sure the subscriber is registered before we
    // fire the OTLP request; otherwise events fan out before any sub
    // is in the hub's set.
    sleep(Duration::from_millis(100)).await;

    // Fire the OTLP request.
    let otlp_endpoint = Endpoint::from_shared(format!("http://127.0.0.1:{port}")).unwrap();
    let otlp_channel = otlp_endpoint.connect().await.expect("dial OTLP");
    let mut logs_client = LogsServiceClient::new(otlp_channel);

    let req = build_request(
        "svc-X",
        &[
            (SeverityNumber::Info, "ignored info"),
            (SeverityNumber::Error, "boom 1"),
            (SeverityNumber::Error, "boom 2"),
            (SeverityNumber::Fatal, "ded"),
        ],
    );
    logs_client.export(req).await.expect("Export");

    // Pull the three expected events.
    let mut received = Vec::new();
    for _ in 0..3 {
        let ev = tokio::time::timeout(Duration::from_secs(2), stream.message())
            .await
            .expect("timeout waiting for notify event")
            .expect("recv error")
            .expect("stream closed early");
        received.push(ev);
    }

    let _ = cancel_tx.send(());
    let _ = handle.await;

    assert_eq!(received.len(), 3);
    let bodies: Vec<&str> = received.iter().map(|e| e.body.as_str()).collect();
    assert_eq!(bodies, vec!["boom 1", "boom 2", "ded"]);
    for ev in &received {
        assert_eq!(ev.service_name, "svc-X");
    }
}

// ---------------------------------------------------------------------
// Tiny HTTP POST helper — avoids pulling in reqwest just for one test.
// ---------------------------------------------------------------------

async fn reqwest_lite_post(url: &str, body: Vec<u8>) -> (u16, String) {
    // Parse host + port out of `http://host:port/path`.
    let stripped = url.strip_prefix("http://").expect("http:// prefix");
    let (host_port, path) = stripped
        .split_once('/')
        .map(|(hp, rest)| (hp, format!("/{rest}")))
        .unwrap_or((stripped, "/".to_string()));

    let mut stream = tokio::net::TcpStream::connect(host_port).await.unwrap();

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8_lossy(&response).into_owned();
    // Parse "HTTP/1.1 STATUS ..."
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}
