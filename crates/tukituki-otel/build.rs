//! Compile vendored OTLP + notify protos into Rust via tonic-build.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = "proto";
    let protos = [
        // OTLP messages.
        "proto/opentelemetry/proto/common/v1/common.proto",
        "proto/opentelemetry/proto/resource/v1/resource.proto",
        "proto/opentelemetry/proto/logs/v1/logs.proto",
        // OTLP services.
        "proto/opentelemetry/proto/collector/logs/v1/logs_service.proto",
        "proto/opentelemetry/proto/collector/metrics/v1/metrics_service.proto",
        "proto/opentelemetry/proto/collector/trace/v1/trace_service.proto",
        // In-house notify socket service.
        "proto/tukituki/otel/notify/v1/notify.proto",
    ];
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&protos, &[proto_root])?;

    for p in &protos {
        println!("cargo:rerun-if-changed={p}");
    }
    Ok(())
}
