//! Optional per-target liveness probe (`health:` in `.run/*.yaml`).
//!
//! `kill(pid, 0)` on the group leader answers "does a process exist",
//! not "is the service serving". Launchers (`go run`, `npm run`) outlive
//! their real child, and a server can wedge mid-shutdown with every
//! listener already closed. Both looked like `running` for days. A
//! probe answers the question that matters: is anything on the other
//! end of the port.
//!
//! Two forms are accepted:
//!
//! * `tcp://HOST:PORT` — a TCP connect succeeds. HOST may be omitted
//!   (`tcp://:7612`) and defaults to 127.0.0.1.
//! * `http://HOST:PORT/PATH` — a TCP connect succeeds *and* a `GET`
//!   yields any HTTP/1.x status line. 4xx/5xx still count as healthy:
//!   an auth-gated `/v1/...` answering 401 is a serving backend, and
//!   a wedged one answers nothing at all. Plain http only — a dev
//!   server behind TLS should be probed over `tcp://`.
//!
//! Every probe is bounded by [`PROBE_TIMEOUT`].

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use chrono::{DateTime, Utc};

/// Upper bound on one probe, connect + (for http) response.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// A parsed `health:` spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthCheck {
    Tcp {
        host: String,
        port: u16,
    },
    Http {
        host: String,
        port: u16,
        path: String,
    },
}

/// Parse a `health:` string. Errors name the exact defect so a typo in
/// YAML surfaces in `describe` rather than as a permanently "unhealthy"
/// target.
pub fn parse(spec: &str) -> Result<HealthCheck, String> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("tcp://") {
        let (host, port) = split_host_port(rest)?;
        return Ok(HealthCheck::Tcp { host, port });
    }
    if let Some(rest) = spec.strip_prefix("http://") {
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_string()),
            None => (rest, "/".to_string()),
        };
        let (host, port) = split_host_port(authority)?;
        return Ok(HealthCheck::Http { host, port, path });
    }
    if spec.starts_with("https://") {
        return Err(
            "health: https:// is not supported; probe the port with tcp://HOST:PORT".into(),
        );
    }
    Err(format!(
        "health: expected tcp://HOST:PORT or http://HOST:PORT/PATH, got {spec:?}"
    ))
}

fn split_host_port(s: &str) -> Result<(String, u16), String> {
    let Some(i) = s.rfind(':') else {
        return Err(format!("health: missing :PORT in {s:?}"));
    };
    let host = s[..i].trim_matches(|c| c == '[' || c == ']');
    let port: u16 = s[i + 1..]
        .parse()
        .map_err(|_| format!("health: bad port in {s:?}"))?;
    if port == 0 {
        return Err(format!("health: port must be 1-65535 in {s:?}"));
    }
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    Ok((host.to_string(), port))
}

/// True while a target is still inside its post-start grace window.
pub fn in_grace(started_at: DateTime<Utc>, grace_secs: u64) -> bool {
    let elapsed = Utc::now().signed_duration_since(started_at);
    elapsed.num_seconds() < grace_secs as i64
}

/// Run one probe. `Ok(())` means something answered.
pub fn probe(check: &HealthCheck) -> Result<(), String> {
    match check {
        HealthCheck::Tcp { host, port } => connect(host, *port).map(drop),
        HealthCheck::Http { host, port, path } => {
            let mut stream = connect(host, *port)?;
            let req = format!(
                "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: tukituki-health\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(req.as_bytes())
                .map_err(|e| format!("write: {e}"))?;
            let mut buf = [0u8; 64];
            let n = stream.read(&mut buf).map_err(|e| format!("read: {e}"))?;
            let head = String::from_utf8_lossy(&buf[..n]);
            if head.starts_with("HTTP/1.") {
                Ok(())
            } else {
                Err(format!("not an HTTP response: {:?}", head.trim_end()))
            }
        }
    }
}

fn connect(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .collect();
    let mut last = format!("no addresses for {host}:{port}");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(PROBE_TIMEOUT));
                let _ = s.set_write_timeout(Some(PROBE_TIMEOUT));
                return Ok(s);
            }
            Err(e) => last = format!("connect {addr}: {e}"),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parse_forms() {
        assert_eq!(
            parse("tcp://:7612").unwrap(),
            HealthCheck::Tcp {
                host: "127.0.0.1".into(),
                port: 7612
            }
        );
        assert_eq!(
            parse("tcp://lune:5432").unwrap(),
            HealthCheck::Tcp {
                host: "lune".into(),
                port: 5432
            }
        );
        assert_eq!(
            parse("http://localhost:7612/healthz").unwrap(),
            HealthCheck::Http {
                host: "localhost".into(),
                port: 7612,
                path: "/healthz".into()
            }
        );
        assert_eq!(
            parse("http://:80").unwrap(),
            HealthCheck::Http {
                host: "127.0.0.1".into(),
                port: 80,
                path: "/".into()
            }
        );
        assert!(parse("https://x:1/").unwrap_err().contains("tcp://"));
        assert!(parse("7612").is_err());
        assert!(parse("tcp://host").is_err());
        assert!(parse("tcp://:0").is_err());
        assert!(parse("tcp://:70000").is_err());
    }

    #[test]
    fn grace_window() {
        assert!(in_grace(Utc::now(), 60));
        assert!(!in_grace(Utc::now() - chrono::Duration::seconds(61), 60));
        assert!(!in_grace(Utc::now(), 0), "grace 0 disables the window");
    }

    #[test]
    fn tcp_probe_reflects_listener() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let check = HealthCheck::Tcp {
            host: "127.0.0.1".into(),
            port,
        };
        assert!(probe(&check).is_ok(), "listener up → healthy");
        drop(l);
        assert!(probe(&check).is_err(), "listener gone → unhealthy");
    }

    #[test]
    fn http_probe_accepts_any_status_line() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
        });
        let check = HealthCheck::Http {
            host: "127.0.0.1".into(),
            port,
            path: "/v1/auth/session".into(),
        };
        assert!(probe(&check).is_ok(), "401 is still 'something answered'");
        server.join().unwrap();
    }

    #[test]
    fn http_probe_rejects_non_http_peer() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"+PONG\r\n");
        });
        let check = HealthCheck::Http {
            host: "127.0.0.1".into(),
            port,
            path: "/".into(),
        };
        assert!(probe(&check).is_err());
        server.join().unwrap();
    }
}
