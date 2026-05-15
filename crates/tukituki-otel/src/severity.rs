//! Human-readable severity name → OTLP `SeverityNumber` mapping.
//!
//! Direct port of `internal/otel/severity.go`. The mapping picks the
//! *minimum* severity number that qualifies, so "error" admits ERROR,
//! ERROR2, ERROR3, ERROR4, and the four FATAL sub-levels too.

use crate::proto::logs::v1::SeverityNumber;

/// Recognised severity-level names, in ascending order.
pub const SEVERITY_NAMES: &[&str] = &["trace", "debug", "info", "warn", "error", "fatal"];

#[derive(Debug)]
pub struct ParseSeverityError {
    pub input: String,
}

impl std::fmt::Display for ParseSeverityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown severity {:?} (valid: trace, debug, info, warn, error, fatal)",
            self.input
        )
    }
}

impl std::error::Error for ParseSeverityError {}

/// `ParseSeverity` analogue. Case-insensitive.
pub fn parse_severity(name: &str) -> Result<SeverityNumber, ParseSeverityError> {
    let lc = name.to_ascii_lowercase();
    let sev = match lc.as_str() {
        "trace" => SeverityNumber::Trace,
        "debug" => SeverityNumber::Debug,
        "info" => SeverityNumber::Info,
        "warn" => SeverityNumber::Warn,
        "error" => SeverityNumber::Error,
        "fatal" => SeverityNumber::Fatal,
        _ => {
            return Err(ParseSeverityError {
                input: name.to_string(),
            });
        }
    };
    Ok(sev)
}

/// Strip the `SEVERITY_NUMBER_` prefix from the OTLP enum name so output
/// reads "ERROR" instead of "SEVERITY_NUMBER_ERROR" — matches Go's
/// `severityLabel`.
pub fn severity_label(n: SeverityNumber) -> &'static str {
    match n {
        SeverityNumber::Unspecified => "UNSPECIFIED",
        SeverityNumber::Trace => "TRACE",
        SeverityNumber::Trace2 => "TRACE2",
        SeverityNumber::Trace3 => "TRACE3",
        SeverityNumber::Trace4 => "TRACE4",
        SeverityNumber::Debug => "DEBUG",
        SeverityNumber::Debug2 => "DEBUG2",
        SeverityNumber::Debug3 => "DEBUG3",
        SeverityNumber::Debug4 => "DEBUG4",
        SeverityNumber::Info => "INFO",
        SeverityNumber::Info2 => "INFO2",
        SeverityNumber::Info3 => "INFO3",
        SeverityNumber::Info4 => "INFO4",
        SeverityNumber::Warn => "WARN",
        SeverityNumber::Warn2 => "WARN2",
        SeverityNumber::Warn3 => "WARN3",
        SeverityNumber::Warn4 => "WARN4",
        SeverityNumber::Error => "ERROR",
        SeverityNumber::Error2 => "ERROR2",
        SeverityNumber::Error3 => "ERROR3",
        SeverityNumber::Error4 => "ERROR4",
        SeverityNumber::Fatal => "FATAL",
        SeverityNumber::Fatal2 => "FATAL2",
        SeverityNumber::Fatal3 => "FATAL3",
        SeverityNumber::Fatal4 => "FATAL4",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_severity_known_names() {
        let cases = [
            ("trace", SeverityNumber::Trace),
            ("debug", SeverityNumber::Debug),
            ("info", SeverityNumber::Info),
            ("warn", SeverityNumber::Warn),
            ("error", SeverityNumber::Error),
            ("fatal", SeverityNumber::Fatal),
        ];
        for (input, want) in cases {
            let got = parse_severity(input).expect(input);
            assert_eq!(got, want, "input={input:?}");
        }
    }

    #[test]
    fn parse_severity_case_insensitive() {
        assert_eq!(parse_severity("ERROR").unwrap(), SeverityNumber::Error);
        assert_eq!(parse_severity("Warn").unwrap(), SeverityNumber::Warn);
    }

    #[test]
    fn parse_severity_invalid() {
        let err = parse_severity("bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

    #[test]
    fn severity_names_count() {
        assert_eq!(SEVERITY_NAMES.len(), 6);
    }
}
