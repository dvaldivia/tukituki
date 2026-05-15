//! Compile vendored OTLP + notify protos into Rust via tonic-build.
//!
//! `protoc` resolution: by default we use `protobuf-src`, which
//! vendors + builds protoc as part of this crate's build. That means
//! `cargo install` / `cargo build` works out of the box on any
//! system that already has a C++ toolchain (essentially every dev
//! machine).  Setting `TUKITUKI_USE_SYSTEM_PROTOC=1` skips the
//! vendored build and falls back to whatever `protoc` is on PATH —
//! useful in CI where saving a few minutes per build matters and the
//! image already ships the system package.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("TUKITUKI_USE_SYSTEM_PROTOC").is_none() {
        // SAFETY: build scripts run single-threaded before any user
        // code; tonic-build reads PROTOC via std::env::var inside the
        // same process, no thread-safety concern.
        unsafe {
            std::env::set_var("PROTOC", protobuf_src::protoc());
        }
    }
    println!("cargo:rerun-if-env-changed=TUKITUKI_USE_SYSTEM_PROTOC");

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
