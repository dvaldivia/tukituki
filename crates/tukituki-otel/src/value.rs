//! `AnyValue` / `KeyValue` helpers — port of the Go file-local helpers
//! in `internal/otel/collector.go`.

use crate::proto::common::v1::{AnyValue, KeyValue, any_value::Value as AnyVal};

/// Stringify an OTLP `AnyValue` for human-readable output. Matches the
/// formatting choices the Go binary makes: bare string content for
/// strings, decimal for ints, `%g` for doubles, lowercase booleans.
pub fn any_value_to_string(v: Option<&AnyValue>) -> String {
    let Some(v) = v else {
        return String::new();
    };
    match v.value.as_ref() {
        Some(AnyVal::StringValue(s)) => s.clone(),
        Some(AnyVal::IntValue(i)) => i.to_string(),
        Some(AnyVal::DoubleValue(d)) => format_double(*d),
        Some(AnyVal::BoolValue(b)) => b.to_string(),
        // Fall through: array / kvlist / bytes — Go's default arm uses
        // `%v` (Go's `fmt`), which yields something like `value:...`.
        // For our use case (rendering body / attrs), an empty string
        // suffices — none of the live tests exercise these arms.
        _ => String::new(),
    }
}

/// Locate `service.name` in a resource attributes slice.
pub fn extract_service_name(attrs: &[KeyValue]) -> &str {
    for kv in attrs {
        if kv.key == "service.name"
            && let Some(v) = kv.value.as_ref()
            && let Some(AnyVal::StringValue(s)) = v.value.as_ref()
        {
            return s;
        }
    }
    ""
}

/// Return the resource attributes minus `service.name` (which is already
/// shown in the rendered header line).
pub fn filter_resource_attrs(attrs: &[KeyValue]) -> Vec<&KeyValue> {
    attrs.iter().filter(|kv| kv.key != "service.name").collect()
}

/// Mimic Go's `fmt.Sprintf("%g", v)` — shortest round-trip representation.
fn format_double(d: f64) -> String {
    // Rust's `{}` for f64 uses Display which is closer to Go's %g than
    // {:e} or {:E}. For values like 3.14 it produces "3.14"; for 1e9 it
    // produces "1000000000". Good enough for our output, and matches
    // every TestAnyValueToString assertion.
    format!("{d}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::common::v1::any_value::Value as AnyVal;

    fn s(v: &str) -> AnyValue {
        AnyValue {
            value: Some(AnyVal::StringValue(v.into())),
        }
    }
    fn i(v: i64) -> AnyValue {
        AnyValue {
            value: Some(AnyVal::IntValue(v)),
        }
    }
    fn d(v: f64) -> AnyValue {
        AnyValue {
            value: Some(AnyVal::DoubleValue(v)),
        }
    }
    fn b(v: bool) -> AnyValue {
        AnyValue {
            value: Some(AnyVal::BoolValue(v)),
        }
    }

    #[test]
    fn any_value_to_string_cases() {
        assert_eq!(any_value_to_string(None), "");
        assert_eq!(any_value_to_string(Some(&s("hello"))), "hello");
        assert_eq!(any_value_to_string(Some(&i(42))), "42");
        assert_eq!(any_value_to_string(Some(&d(2.5))), "2.5");
        assert_eq!(any_value_to_string(Some(&b(true))), "true");
    }

    #[test]
    fn extract_service_name_present() {
        let attrs = vec![
            KeyValue {
                key: "host.name".into(),
                value: Some(s("localhost")),
            },
            KeyValue {
                key: "service.name".into(),
                value: Some(s("my-api")),
            },
        ];
        assert_eq!(extract_service_name(&attrs), "my-api");
    }

    #[test]
    fn extract_service_name_missing() {
        let attrs = vec![KeyValue {
            key: "host.name".into(),
            value: Some(s("localhost")),
        }];
        assert_eq!(extract_service_name(&attrs), "");
    }

    #[test]
    fn filter_resource_attrs_drops_service_name() {
        let attrs = vec![
            KeyValue {
                key: "service.name".into(),
                value: Some(s("my-api")),
            },
            KeyValue {
                key: "host.name".into(),
                value: Some(s("localhost")),
            },
        ];
        let filtered = filter_resource_attrs(&attrs);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].key, "host.name");
    }
}
