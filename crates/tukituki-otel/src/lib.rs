//! OpenTelemetry log collector + notify-socket hub.
//!
//! - `severity`: human-name → OTLP `SeverityNumber` mapping.
//! - `collector`: gRPC + HTTP OTLP receivers, severity filtering, output
//!   formatting that matches the Go binary line-for-line.
//! - `notify`: Unix-domain-socket gRPC server that pushes filtered error
//!   events to attached TUIs.
//!
//! Phase-5 scope of the port from `internal/otel/` in the Go tree.

pub mod collector;
pub mod notify;
pub mod severity;
pub mod value;

/// Generated protobuf modules. Re-exported through this single root
/// module so consumers don't need to know the internal nesting.
pub mod proto {
    pub mod common {
        pub mod v1 {
            tonic::include_proto!("opentelemetry.proto.common.v1");
        }
    }
    pub mod resource {
        pub mod v1 {
            tonic::include_proto!("opentelemetry.proto.resource.v1");
        }
    }
    pub mod logs {
        pub mod v1 {
            tonic::include_proto!("opentelemetry.proto.logs.v1");
        }
    }
    pub mod collector {
        pub mod logs {
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.collector.logs.v1");
            }
        }
        pub mod metrics {
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.collector.metrics.v1");
            }
        }
        pub mod trace {
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.collector.trace.v1");
            }
        }
    }
    pub mod notify_v1 {
        tonic::include_proto!("tukituki.otel.notify.v1");
    }
}

pub use collector::Collector;
pub use severity::{ParseSeverityError, SEVERITY_NAMES, parse_severity};
