use std::future::{pending, Future};
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::models::{ConnectRequest, SendRequest};
use crate::reporting::NullLogger;
use crate::transport::cleanup::AttemptCleanup;
use crate::transport::runner::supervise;
use crate::transport::test_support::Sink;

const GRACE: Duration = Duration::from_millis(20);
const FORCED: Duration = Duration::from_millis(80);

fn setup() -> (Arc<Sink>, ConnectionManager) {
    let sink = Arc::new(Sink::default());
    let factory = ConnectionReporterFactory::new(sink.clone(), Arc::new(NullLogger));
    (sink, ConnectionManager::new(factory))
}

struct Controlled {
    connection: ConnectionId,
    queue: SharedMessageOrchestrator,
    force_stop: watch::Receiver<bool>,
    status: watch::Receiver<ConnectionStatus>,
}

async fn install(
    manager: &ConnectionManager,
    worker: impl FnOnce(oneshot::Receiver<()>) -> JoinHandle<ConnectionStatus>,
    job: impl Future<Output = Result<(), String>> + Send + 'static,
) -> Controlled {
    let connection = ConnectionId::parse("s", 1).unwrap();
    let reporter = manager.reporter_factory.create(&connection);
    let queue =
        MessageOrchestrator::shared(reporter.clone(), Arc::new(AutoReplyRuleSet::disabled()));
    queue
        .send_user_message(&SendRequest {
            message: "pending".into(),
        })
        .await
        .unwrap();
    let (shutdown, shutdown_rx) = oneshot::channel();
    let (force_stop, force_rx) = watch::channel(false);
    let (status, status_rx) = watch::channel(ConnectionStatus::Connected);
    let status = Arc::new(status);
    let cleanup = AttemptCleanup::new(force_rx.clone());
    cleanup.spawn(job);
    let worker = worker(shutdown_rx);
    let task_queue = queue.clone();
    let task_reporter = reporter.clone();
    let task_status = status.clone();
    let task_force = force_rx.clone();
    let join = spawn_attempt(
        Arc::downgrade(&manager.registry),
        connection.clone(),
        reporter.clone(),
        status.clone(),
        async move {
            supervise(
                worker,
                task_queue,
                cleanup,
                task_force,
                task_status,
                &task_reporter,
            )
            .await
        },
    );
    manager.registry.lock().await.sessions.insert(
        connection.session_id.clone(),
        Session {
            last_attempt: Some(connection.attempt),
            state: SessionState::Running(Running {
                attempt: connection.attempt,
                message_orchestrator: queue.clone(),
                shutdown: Some(shutdown),
                force_stop,
                status,
                _join: join,
                reporter,
            }),
        },
    );
    Controlled {
        connection,
        queue,
        force_stop: force_rx,
        status: status_rx,
    }
}

async fn stopped(status: &mut watch::Receiver<ConnectionStatus>) {
    let result = timeout(
        Duration::from_secs(2),
        status.wait_for(|status| is_terminal(*status)),
    )
    .await
    .expect("cleanup must finish")
    .expect("status sender retained until completion");
    assert!(is_terminal(*result));
}

async fn assert_closed(queue: &SharedMessageOrchestrator) {
    assert_eq!(queue.recv().await, None);
    assert!(queue
        .send_user_message(&SendRequest {
            message: "late".into()
        })
        .await
        .is_err());
}

fn server_spec() -> ConnectionSpec {
    ConnectionSpec::try_from(ConnectRequest::ServerStartRequest { port: 12345 }).unwrap()
}

#[test]
fn shutdown_budgets_are_five_seconds_plus_fifteen_seconds() {
    assert_eq!(STOP_TIMEOUT, Duration::from_secs(5));
    assert_eq!(FORCED_CLEANUP_TIMEOUT, Duration::from_secs(15));
    assert_eq!(
        STOP_TIMEOUT + FORCED_CLEANUP_TIMEOUT,
        Duration::from_secs(20)
    );
    assert!(!is_terminal(ConnectionStatus::Stopping));
}

#[tokio::test]
async fn graceful_stop_observes_cleanup_before_publishing_completion() {
    let (sink, manager) = setup();
    let (release, cleanup) = oneshot::channel();
    let mut controlled = install(
        &manager,
        |shutdown| {
            tokio::spawn(async move {
                let _ = shutdown.await;
                ConnectionStatus::Disconnected
            })
        },
        async move {
            cleanup.await.unwrap();
            Ok(())
        },
    )
    .await;
    let outcome = manager
        .begin_stop(&controlled.connection.session_id, false, GRACE, FORCED)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*controlled.status.borrow(), ConnectionStatus::Stopping);
    assert!(outcome.borrow().is_none());
    assert!(manager.send_target(&controlled.connection).await.is_err());
    release.send(()).unwrap();
    stopped(&mut controlled.status).await;
    assert_eq!(*outcome.borrow(), Some(Ok(())));
    assert!(!*controlled.force_stop.borrow());
    assert_closed(&controlled.queue).await;
    assert_eq!(
        sink.statuses.lock().unwrap().last().unwrap().status,
        ConnectionStatus::Disconnected
    );
}

struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn forced_stop_awaits_worker_destruction_and_reports_a_warning() {
    let (sink, manager) = setup();
    let destroyed = Arc::new(AtomicBool::new(false));
    let probe = Dropped(destroyed.clone());
    let controlled = install(
        &manager,
        |_| {
            tokio::spawn(async move {
                let _probe = probe;
                pending::<ConnectionStatus>().await
            })
        },
        async { Ok(()) },
    )
    .await;
    manager
        .stop_with_timeouts(&controlled.connection.session_id, false, GRACE, FORCED)
        .await
        .unwrap();
    assert!(destroyed.load(Ordering::SeqCst));
    assert_eq!(*controlled.status.borrow(), ConnectionStatus::Disconnected);
    assert_closed(&controlled.queue).await;
    let messages = sink.messages.lock().unwrap();
    assert!(messages
        .iter()
        .any(|message| message.content.contains("Forced stopping was needed")));
    assert!(!messages
        .iter()
        .any(|message| message.content.contains("failed unexpectedly")));
}

struct ReleaseDestructor {
    state: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl Drop for ReleaseDestructor {
    fn drop(&mut self) {
        *self.state.0.lock().unwrap() = true;
        self.state.1.notify_all();
    }
}

struct BlockedDestructor {
    entered: Option<oneshot::Sender<()>>,
    release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    finished: Arc<AtomicBool>,
}

impl Drop for BlockedDestructor {
    fn drop(&mut self) {
        let _ = self.entered.take().unwrap().send(());
        let mut released = self.release.0.lock().unwrap();
        while !*released {
            let (next, elapsed) = self
                .release
                .1
                .wait_timeout(released, Duration::from_secs(2))
                .unwrap();
            released = next;
            if elapsed.timed_out() {
                break;
            }
        }
        self.finished.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_requested_is_not_completion_while_the_worker_destructor_is_blocked() {
    let (_, manager) = setup();
    let (entered, entered_rx) = oneshot::channel();
    let release = ReleaseDestructor {
        state: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
    };
    let finished = Arc::new(AtomicBool::new(false));
    let probe = BlockedDestructor {
        entered: Some(entered),
        release: release.state.clone(),
        finished: finished.clone(),
    };
    let mut controlled = install(
        &manager,
        |_| {
            tokio::spawn(async move {
                let _probe = probe;
                pending::<ConnectionStatus>().await
            })
        },
        async { Ok(()) },
    )
    .await;
    let outcome = manager
        .begin_stop(&controlled.connection.session_id, false, GRACE, FORCED)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(2), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(!finished.load(Ordering::SeqCst));
    assert!(!matches!(*outcome.borrow(), Some(Ok(()))));
    assert_eq!(*controlled.status.borrow(), ConnectionStatus::Stopping);
    assert!(!finished.load(Ordering::SeqCst));
    drop(release);
    stopped(&mut controlled.status).await;
    assert!(finished.load(Ordering::SeqCst));
    assert_closed(&controlled.queue).await;
}

#[tokio::test]
async fn incomplete_close_retains_ownership_then_completes_without_removing_the_session() {
    let (sink, manager) = setup();
    let (release, cleanup) = oneshot::channel();
    let mut controlled = install(&manager, |_| tokio::spawn(pending()), async move {
        cleanup.await.unwrap();
        Ok(())
    })
    .await;
    let result = manager
        .stop_with_timeouts(&controlled.connection.session_id, true, GRACE, FORCED)
        .await;
    assert_eq!(result, Err(STOP_INCOMPLETE.into()));
    assert_eq!(*controlled.status.borrow(), ConnectionStatus::Stopping);
    assert!(manager.send_target(&controlled.connection).await.is_err());
    let next = ConnectionId::parse("s", 2).unwrap();
    assert!(manager.connect(&next, server_spec()).await.is_err());
    assert!(manager
        .stop(&controlled.connection.session_id, true)
        .await
        .is_err());
    assert!(manager
        .stop(&controlled.connection.session_id, false)
        .await
        .is_err());
    assert_closed(&controlled.queue).await;

    // Another session can be registered even while this cleanup is still pending.
    let other = ConnectionId::parse("other", 1).unwrap();
    manager.connect(&other, server_spec()).await.unwrap();
    manager.stop(&other.session_id, true).await.unwrap();
    release.send(()).unwrap();
    stopped(&mut controlled.status).await;
    assert!(matches!(
        manager
            .registry
            .lock()
            .await
            .sessions
            .get(&controlled.connection.session_id)
            .unwrap()
            .state,
        SessionState::Idle
    ));
    assert!(sink
        .messages
        .lock()
        .unwrap()
        .iter()
        .any(|message| message.content == "Cleanup completed. The connection is now stopped."));
    manager.connect(&next, server_spec()).await.unwrap();
    manager.stop(&next.session_id, true).await.unwrap();
}

#[tokio::test]
async fn abandoning_the_command_waiter_does_not_cancel_the_stop_monitor() {
    let (_, manager) = setup();
    let (release, cleanup) = oneshot::channel();
    let mut controlled = install(&manager, |_| tokio::spawn(pending()), async move {
        cleanup.await.unwrap();
        Ok(())
    })
    .await;
    let outcome = manager
        .begin_stop(&controlled.connection.session_id, true, GRACE, FORCED)
        .await
        .unwrap()
        .unwrap();
    drop(outcome);
    timeout(
        Duration::from_secs(2),
        controlled.force_stop.wait_for(|force| *force),
    )
    .await
    .unwrap()
    .unwrap();
    release.send(()).unwrap();
    stopped(&mut controlled.status).await;
    assert!(!manager
        .registry
        .lock()
        .await
        .sessions
        .contains_key(&controlled.connection.session_id));
}

#[tokio::test]
async fn completion_and_timeout_commit_consistent_close_outcomes() {
    for _ in 0..20 {
        let (_, manager) = setup();
        let (release, cleanup) = oneshot::channel();
        let mut controlled = install(&manager, |_| tokio::spawn(pending()), async move {
            cleanup.await.unwrap();
            Ok(())
        })
        .await;
        let mut outcome = manager
            .begin_stop(
                &controlled.connection.session_id,
                true,
                Duration::ZERO,
                Duration::ZERO,
            )
            .await
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
        let result = outcome
            .wait_for(|result| result.is_some())
            .await
            .unwrap()
            .clone()
            .unwrap();
        stopped(&mut controlled.status).await;
        let registry = manager.registry.lock().await;
        assert_eq!(
            registry
                .sessions
                .contains_key(&controlled.connection.session_id),
            result.is_err()
        );
        assert_eq!(outcome.borrow().as_ref().unwrap().is_err(), result.is_err());
    }
}

#[tokio::test]
async fn worker_panic_closes_the_queue_but_still_waits_for_required_cleanup() {
    let (_, manager) = setup();
    let (release, cleanup) = oneshot::channel();
    let mut controlled = install(
        &manager,
        |_| tokio::spawn(async { panic!("controlled worker panic") }),
        async move {
            cleanup.await.unwrap();
            Ok(())
        },
    )
    .await;
    timeout(
        Duration::from_secs(2),
        controlled
            .status
            .wait_for(|status| *status == ConnectionStatus::Stopping),
    )
    .await
    .unwrap()
    .unwrap();
    assert_closed(&controlled.queue).await;
    assert!(manager
        .connect(&ConnectionId::parse("s", 2).unwrap(), server_spec())
        .await
        .is_err());
    release.send(()).unwrap();
    stopped(&mut controlled.status).await;
    assert_eq!(*controlled.status.borrow(), ConnectionStatus::Error);
}

#[tokio::test]
async fn setup_failure_before_serve_closes_the_attempt_queue() {
    let (_, manager) = setup();
    let occupied = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = occupied.local_addr().unwrap().port();
    let connection = ConnectionId::parse("s", 1).unwrap();
    manager
        .connect(
            &connection,
            ConnectionSpec::try_from(ConnectRequest::ServerStartRequest { port }).unwrap(),
        )
        .await
        .unwrap();
    let (queue, mut status) = {
        let registry = manager.registry.lock().await;
        let SessionState::Running(running) =
            &registry.sessions.get(&connection.session_id).unwrap().state
        else {
            panic!("worker has not run yet")
        };
        (
            running.message_orchestrator.clone(),
            running.status.subscribe(),
        )
    };
    stopped(&mut status).await;
    assert_eq!(*status.borrow(), ConnectionStatus::Error);
    assert_closed(&queue).await;
}

#[tokio::test]
async fn cleanup_panic_never_claims_local_completion() {
    let (sink, manager) = setup();
    let controlled = install(
        &manager,
        |_| tokio::spawn(async { ConnectionStatus::Disconnected }),
        async { panic!("controlled cleanup panic") },
    )
    .await;
    assert!(manager
        .stop_with_timeouts(&controlled.connection.session_id, false, GRACE, FORCED)
        .await
        .is_err());
    assert_eq!(*controlled.status.borrow(), ConnectionStatus::Stopping);
    assert!(sink.messages.lock().unwrap().iter().any(|message| message
        .content
        .contains("Cleanup completion cannot be confirmed")));
}
