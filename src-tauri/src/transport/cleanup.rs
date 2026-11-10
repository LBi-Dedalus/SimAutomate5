use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::reporting::ConnectionReporter;

type CleanupJob = JoinHandle<Result<(), String>>;

pub(super) async fn wait_for_force(force_stop: &mut watch::Receiver<bool>) {
    let _ = force_stop.wait_for(|force| *force).await;
}

/// Jobs are registered before their caller can await or abandon them. Only the supervisor
/// drains them, after the worker has ended; jobs may register further cleanup while finishing.
#[derive(Clone)]
pub(super) struct AttemptCleanup {
    jobs: Arc<Mutex<Vec<CleanupJob>>>,
    force_stop: watch::Receiver<bool>,
}

impl AttemptCleanup {
    pub(super) fn new(force_stop: watch::Receiver<bool>) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(Vec::new())),
            force_stop,
        }
    }

    pub(super) fn force_stop(&self) -> watch::Receiver<bool> {
        self.force_stop.clone()
    }

    pub(super) fn spawn(
        &self,
        job: impl Future<Output = Result<(), String>> + Send + 'static,
    ) -> oneshot::Receiver<()> {
        let (done, completion) = oneshot::channel();
        let task = tokio::spawn(async move {
            let result = job.await;
            let _ = done.send(());
            result
        });
        self.jobs.lock().unwrap().push(task);
        completion
    }

    pub(super) async fn finish(&self, reporter: &ConnectionReporter) -> bool {
        let mut succeeded = true;
        let mut unconfirmed = false;
        loop {
            let jobs = std::mem::take(&mut *self.jobs.lock().unwrap());
            if jobs.is_empty() {
                if unconfirmed {
                    // A panicked cleanup may have lost its resource observer. Cancellation is
                    // not proof of completion; retain the attempt rather than publish success.
                    std::future::pending::<()>().await;
                }
                return succeeded;
            }
            for job in jobs {
                let error = match job.await {
                    Ok(Ok(())) => continue,
                    Ok(Err(error)) => error,
                    Err(error) => {
                        unconfirmed = true;
                        format!("Cleanup completion cannot be confirmed: a cleanup task failed: {error}")
                    }
                };
                succeeded = false;
                reporter.error(file!(), line!(), error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn finishing_a_job_cannot_hide_follow_on_cleanup() {
        let (_force, force_stop) = watch::channel(false);
        let cleanup = AttemptCleanup::new(force_stop);
        let nested = cleanup.clone();
        let (registered, registration) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        cleanup.spawn(async move {
            nested.spawn(async move {
                gate.await.unwrap();
                Ok(())
            });
            registered.send(()).unwrap();
            Ok(())
        });
        let (_, reporter) = crate::transport::test_support::reporter();
        let mut finish = tokio::spawn(async move { cleanup.finish(&reporter).await });
        registration.await.unwrap();
        assert!(timeout(Duration::from_millis(10), &mut finish)
            .await
            .is_err());
        release.send(()).unwrap();
        assert!(timeout(Duration::from_secs(2), finish)
            .await
            .unwrap()
            .unwrap());
    }
}
