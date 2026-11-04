//! The background assertion evaluator (port of `internal/ingest/evaluator.go`).
//!
//! One worker owns evaluation off the ingest path: a bounded queue applies
//! backpressure instead of unbounded growth, and a drain on shutdown makes
//! sure a queued batch is not silently dropped.

use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::evals::Endpoint;
use crate::store::Store;

/// Bounded queue: a full queue drops the batch (deterministic assertions can be
/// recovered through the on-demand endpoint) rather than stalling ingest.
const QUEUE_CAPACITY: usize = 256;

pub struct Evaluator {
    sender: mpsc::Sender<Vec<(String, String)>>,
    shutdown: watch::Sender<bool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Evaluator {
    /// Launch the worker. Call [`Evaluator::stop`] before closing the store.
    pub fn start(store: Arc<Store>, judge: Endpoint) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel(QUEUE_CAPACITY);
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    received = receiver.recv() => match received {
                        Some(refs) => evaluate(&store, &judge, refs).await,
                        None => break,
                    },
                }
            }
            while let Ok(refs) = receiver.try_recv() {
                evaluate(&store, &judge, refs).await;
            }
        });
        Arc::new(Self {
            sender,
            shutdown,
            handle: Mutex::new(Some(handle)),
        })
    }

    /// Schedule evaluation of the given `(project, run id)` pairs. Non-blocking:
    /// a full queue logs and drops rather than stalling ingest.
    pub fn enqueue(&self, refs: Vec<(String, String)>) {
        if refs.is_empty() {
            return;
        }
        if self.sender.try_send(refs).is_err() {
            tracing::warn!(
                "evaluator queue full; skipping assertion eval for a batch (use POST /api/assertions/evaluate to backfill)"
            );
        }
    }

    /// Drain the queue and wait for the worker to exit.
    pub async fn stop(&self) {
        let _ = self.shutdown.send(true);
        let handle = self.handle.lock().ok().and_then(|mut handle| handle.take());
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
}

async fn evaluate(store: &Store, judge: &Endpoint, refs: Vec<(String, String)>) {
    if let Err(err) = super::evaluate::evaluate_runs(store, judge, &refs, false).await {
        tracing::error!(error = %err, "assertion evaluation failed");
    }
}
