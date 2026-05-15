//! Notify hub: a fan-out of `ErrorEvent`s to every currently-attached
//! TUI over the Unix-domain-socket gRPC service.
//!
//! Direct port of `internal/otel/notify_hub.go`. Slow subscribers drop
//! events instead of stalling the OTLP receive path.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::proto::notify_v1::ErrorEvent;
use crate::proto::notify_v1::notifier_server::Notifier;

/// The maximum size of a subscriber's buffered channel. Matches Go's
/// `make(chan *notifypb.ErrorEvent, 256)`.
const SUBSCRIBER_BUFFER: usize = 256;

/// Holds the live set of subscriber senders. `Publish` walks the set
/// and `try_send`s each event; full or disconnected channels are
/// silently skipped (and pruned).
#[derive(Default, Clone)]
pub struct NotifyHub {
    inner: Arc<Mutex<Vec<mpsc::Sender<ErrorEvent>>>>,
}

impl NotifyHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver `ev` to every connected subscriber. Slow subscribers
    /// (full buffer) drop the event; dead subscribers are pruned.
    pub fn publish(&self, ev: ErrorEvent) {
        let mut subs = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        subs.retain(|tx| match tx.try_send(ev.clone()) {
            Ok(_) => true,
            Err(mpsc::error::TrySendError::Full(_)) => true,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// Register a new subscriber and return its receiver half.
    fn add_subscriber(&self) -> mpsc::Receiver<ErrorEvent> {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_BUFFER);
        let mut subs = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        subs.push(tx);
        rx
    }

    /// Drop every subscriber, terminating any in-flight `Subscribe`
    /// streams on the next send (their receivers will see Closed).
    pub fn shutdown(&self) {
        let mut subs = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        subs.clear();
    }
}

/// Subscribe stream — wraps `ReceiverStream<ErrorEvent>` and re-tags
/// each item as `Ok(ev)` for tonic's Stream<Item = Result<_, Status>>
/// shape.
pub struct SubscribeStream {
    inner: ReceiverStream<ErrorEvent>,
}

impl Stream for SubscribeStream {
    type Item = Result<ErrorEvent, Status>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(ev)) => Poll::Ready(Some(Ok(ev))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[tonic::async_trait]
impl Notifier for NotifyHub {
    type SubscribeStream = SubscribeStream;

    async fn subscribe(
        &self,
        _req: Request<crate::proto::notify_v1::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let rx = self.add_subscriber();
        Ok(Response::new(SubscribeStream {
            inner: ReceiverStream::new(rx),
        }))
    }
}
