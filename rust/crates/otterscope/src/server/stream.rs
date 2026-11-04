//! Live-tail over Server-Sent Events (ported from the original Go
//! implementation).

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use tokio::sync::broadcast;

/// Fans out "something changed" ticks to connected SSE clients. The broadcast
/// buffer is one tick, so a burst coalesces instead of queueing: a client that
/// is mid-send refetches once when it reads again.
#[derive(Clone)]
pub struct Hub {
    sender: broadcast::Sender<()>,
}

impl Hub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.sender.subscribe()
    }

    /// Wake every subscriber without blocking. No subscribers is not an error.
    pub fn broadcast(&self) {
        let _ = self.sender.send(());
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

pub fn response(hub: Hub) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = hub.subscribe();
    let connected = stream::once(async { Ok(Event::default().comment("connected")) });
    let ticks = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(()) => {
                    return Some((Ok(Event::default().event("runs").data("tick")), receiver));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(connected.chain(ticks)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(25))
            .text("ping"),
    )
}
