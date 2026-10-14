// Keep cancellation tied to the dispatcher and all of its workers.

use std::time::Duration;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

pub async fn run_until_cancelled(
    cancellation: CancellationToken,
    work: impl std::future::Future<Output = ()>,
) {
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => {}
        _ = work => {}
    }
}

pub struct TranscriptionTask {
    handle: JoinHandle<()>,
    cancellation: CancellationToken,
}

impl TranscriptionTask {
    pub fn new(handle: JoinHandle<()>, cancellation: CancellationToken) -> Self {
        Self {
            handle,
            cancellation,
        }
    }

    /// Return false when pending transcription was cancelled after the deadline.
    /// The dispatcher joins its workers before this method returns, preventing
    /// late transcript events from leaking into the next recording.
    pub async fn finish(mut self, timeout: Duration) -> Result<bool, JoinError> {
        match tokio::time::timeout(timeout, &mut self.handle).await {
            Ok(result) => result.map(|()| true),
            Err(_) => {
                self.cancellation.cancel();
                self.handle.await.map(|()| false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn timeout_cancels_and_joins_child_before_returning() {
        let cancellation = CancellationToken::new();
        let child_token = cancellation.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let child_exited = exited.clone();
        let dispatcher = tokio::spawn(async move {
            let child = tokio::spawn(async move {
                child_token.cancelled().await;
                child_exited.store(true, Ordering::SeqCst);
            });
            child.await.unwrap();
        });
        let task = TranscriptionTask::new(dispatcher, cancellation);
        assert!(!task.finish(Duration::from_millis(1)).await.unwrap());
        assert!(exited.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancellation_drops_pending_result_before_it_can_publish() {
        let cancellation = CancellationToken::new();
        let published = Arc::new(AtomicBool::new(false));
        let child_published = published.clone();
        let (send, receive) = tokio::sync::oneshot::channel::<()>();
        let child_token = cancellation.clone();
        let child = tokio::spawn(run_until_cancelled(child_token, async move {
            let _ = receive.await;
            child_published.store(true, Ordering::SeqCst);
        }));
        let dispatcher = tokio::spawn(async move { child.await.unwrap(); });
        let task = TranscriptionTask::new(dispatcher, cancellation);
        assert!(!task.finish(Duration::from_millis(1)).await.unwrap());
        assert!(send.send(()).is_err());
        assert!(!published.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn completed_task_does_not_cancel() {
        let cancellation = CancellationToken::new();
        let task = TranscriptionTask::new(tokio::spawn(async {}), cancellation.clone());
        assert!(task.finish(Duration::from_secs(1)).await.unwrap());
        assert!(!cancellation.is_cancelled());
    }
}
