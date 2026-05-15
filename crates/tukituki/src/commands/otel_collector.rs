//! Hidden `tukituki otel-collector` subcommand.
//!
//! The Manager spawns this as a regular detached child whose stdout +
//! stderr land in `<state-dir>/logs/otel-errors.log`. The Collector
//! itself runs an OTLP receiver and (when a notify socket is set) a
//! Unix-domain-socket gRPC server for live TUI consumers.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;

use tukituki_otel::{Collector, parse_severity};

pub fn run(protocol: &str, severity: &str, port: u16, notify_socket: &str) -> ExitCode {
    let min_severity = match parse_severity(severity) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::from(1);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: build runtime: {e}");
            return ExitCode::from(1);
        }
    };

    let mut collector = Collector {
        port,
        protocol: protocol.to_string(),
        min_severity,
        output: Arc::new(StdMutex::new(Box::new(std::io::stdout()))),
        notify_socket: if notify_socket.is_empty() {
            None
        } else {
            Some(PathBuf::from(notify_socket))
        },
    };
    let _ = &mut collector;

    rt.block_on(async move {
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        // Translate SIGINT / SIGTERM into the cancel channel so the
        // collector shuts down gracefully (closes UDS socket, removes
        // the file, drains in-flight RPCs).
        let signal_task = tokio::spawn(async move {
            let mut sigint = signal(SignalKind::interrupt()).ok();
            let mut sigterm = signal(SignalKind::terminate()).ok();
            tokio::select! {
                _ = async {
                    if let Some(s) = sigint.as_mut() {
                        s.recv().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {}
                _ = async {
                    if let Some(s) = sigterm.as_mut() {
                        s.recv().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {}
            }
            let _ = cancel_tx.send(());
        });

        let result = collector.run(cancel_rx).await;
        // Try to wake the signal task if it's still spinning.
        signal_task.abort();
        match result {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e}");
                ExitCode::from(1)
            }
        }
    })
}
