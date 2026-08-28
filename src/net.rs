use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use log::{debug, warn};

/// Seconds to allow a fetch or push before abandoning it.
///
/// Generous enough that a slow but working remote is never cut off, short
/// enough that a wedged one is noticed within one debounce cycle rather than
/// after days.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 60;

/// An error the process cannot recover from without being restarted.
///
/// Retrying these is pointless and actively harmful: the daemon keeps
/// committing locally while never pushing, and because it never exits, a
/// supervisor watching for exit - `launchd`'s `KeepAlive`, systemd's `Restart` -
/// has nothing to act on. Attaching this to the error lets the watch loop end
/// instead, so the supervisor starts a clean process.
#[derive(Debug)]
pub struct Unrecoverable;

impl std::fmt::Display for Unrecoverable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gitwatch must restart to recover")
    }
}

impl std::error::Error for Unrecoverable {}

/// The operation that timed out most recently, if it has not finished since.
///
/// See [`run_with_timeout`] for why at most one may be outstanding.
static OUTSTANDING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// Runs a network git operation on its own thread and gives up on it after
/// `timeout`, so a remote that stops answering cannot wedge the daemon.
///
/// A deadline is needed because libssh2 offers no way to bound the SSH
/// handshake. If the SSH agent accepts the connection and then never replies -
/// as a locked 1Password agent does - libssh2 waits inside `agent_transact_unix`
/// forever. Nothing returns an error, so `--retries` never engages and nothing
/// reaches the log: the daemon simply stops committing while still looking
/// alive to `launchd`.
///
/// The operation is *abandoned*, not cancelled. There is no way to interrupt a
/// thread parked in C code, so the worker stays stuck for the life of the
/// process. That is why only one may be outstanding at a time: without the
/// guard, every debounce tick would strand another thread against a remote that
/// is already known to be unresponsive.
pub fn run_with_timeout<T, F>(label: &str, timeout: Duration, operation: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    reserve_slot(label)?;

    let finished = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();

    let worker_finished = Arc::clone(&finished);
    thread::Builder::new()
        .name(format!("gitwatch-{label}"))
        .spawn(move || {
            let result = operation();
            worker_finished.store(true, Ordering::SeqCst);
            // Fails only if we already timed out and stopped listening.
            let _ = tx.send(result);
        })
        .context(format!("Failed to spawn thread for {label}"))?;

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            *OUTSTANDING.lock().unwrap() = Some(finished);
            warn!(
                "Abandoning {label} after {}s; its thread stays stuck until gitwatch restarts",
                timeout.as_secs()
            );
            bail!("{label} timed out after {}s", timeout.as_secs())
        }
        Err(RecvTimeoutError::Disconnected) => bail!("{label} thread panicked"),
    }
}

/// Refuses to start a second operation while an abandoned one is still stuck.
fn reserve_slot(label: &str) -> Result<()> {
    let mut outstanding = OUTSTANDING.lock().unwrap();
    match outstanding.as_ref() {
        // Nothing can clear this except the abandoned thread returning, which a
        // wedged libssh2 call never does. Restarting is the only way out.
        Some(finished) if !finished.load(Ordering::SeqCst) => {
            Err(anyhow::Error::new(Unrecoverable).context(format!(
                "Skipping {label}: an earlier network operation is still stuck"
            )))
        }
        Some(_) => {
            debug!("Earlier abandoned network operation has finished");
            *outstanding = None;
            Ok(())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{channel, Sender};

    use testresult::TestResult;

    use super::*;

    /// Serialises the tests, which share the global [`OUTSTANDING`] slot.
    static GUARD: Mutex<()> = Mutex::new(());

    fn reset() -> std::sync::MutexGuard<'static, ()> {
        let guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        *OUTSTANDING.lock().unwrap() = None;
        guard
    }

    /// Reports that it started, then blocks forever - standing in for the
    /// libssh2 call that cannot be interrupted.
    fn block_forever(started: Sender<()>) -> Result<()> {
        let (_keep_alive, never) = channel::<()>();
        let _ = started.send(());
        let _ = never.recv();
        Ok(())
    }

    #[test]
    fn test_returns_value_when_operation_completes() -> TestResult {
        let _guard = reset();
        let value = run_with_timeout("fetch", Duration::from_secs(30), || Ok(7))?;
        assert_eq!(value, 7);
        Ok(())
    }

    #[test]
    fn test_propagates_operation_error() -> TestResult {
        let _guard = reset();
        let result: Result<()> =
            run_with_timeout("fetch", Duration::from_secs(30), || bail!("remote said no"));
        assert_eq!(result.unwrap_err().to_string(), "remote said no");
        Ok(())
    }

    #[test]
    fn test_times_out_instead_of_hanging() -> TestResult {
        let _guard = reset();
        let (started_tx, started) = channel();

        let result: Result<()> = run_with_timeout("fetch", Duration::from_millis(200), move || {
            block_forever(started_tx)
        });

        assert_eq!(
            result.unwrap_err().to_string(),
            "fetch timed out after 0s",
            "A stuck operation must return an error rather than block"
        );
        started.recv()?;
        Ok(())
    }

    #[test]
    fn test_stuck_thread_makes_further_operations_unrecoverable() -> TestResult {
        let _guard = reset();
        let (started_tx, started) = channel();

        let first: Result<()> = run_with_timeout("fetch", Duration::from_millis(200), move || {
            block_forever(started_tx)
        });
        assert!(first.is_err());
        started.recv()?;

        let second: Result<()> = run_with_timeout("fetch", Duration::from_secs(30), || Ok(()));
        assert!(
            second.unwrap_err().is::<Unrecoverable>(),
            "Being permanently stuck must end the daemon rather than be retried"
        );
        Ok(())
    }

    #[test]
    fn test_timeout_alone_stays_retryable() -> TestResult {
        let _guard = reset();
        let (started_tx, started) = channel();

        let result: Result<()> = run_with_timeout("fetch", Duration::from_millis(200), move || {
            block_forever(started_tx)
        });

        assert!(
            !result.unwrap_err().is::<Unrecoverable>(),
            "A single timeout may be transient, so --retries should still apply"
        );
        started.recv()?;
        Ok(())
    }

    #[test]
    fn test_refuses_to_stack_up_stuck_threads() -> TestResult {
        let _guard = reset();
        let (started_tx, started) = channel();

        let first: Result<()> = run_with_timeout("fetch", Duration::from_millis(200), move || {
            block_forever(started_tx)
        });
        assert!(first.is_err());
        started.recv()?;

        let second: Result<()> = run_with_timeout("fetch", Duration::from_secs(30), || Ok(()));
        assert_eq!(
            second.unwrap_err().to_string(),
            "Skipping fetch: an earlier network operation is still stuck"
        );
        Ok(())
    }

    #[test]
    fn test_frees_the_slot_once_the_operation_finishes() -> TestResult {
        let _guard = reset();
        let (unblock_tx, unblock_rx) = channel();

        let first: Result<()> = run_with_timeout("fetch", Duration::from_millis(200), move || {
            unblock_rx.recv().ok();
            Ok(())
        });
        assert!(first.is_err(), "The slow operation should time out");

        // Let the abandoned thread finish, then wait for it to record that.
        unblock_tx.send(())?;
        let mut attempts = 0;
        while OUTSTANDING
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|finished| !finished.load(Ordering::SeqCst))
        {
            attempts += 1;
            assert!(attempts < 100, "Abandoned thread never finished");
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            run_with_timeout("fetch", Duration::from_secs(30), || Ok(())).is_ok(),
            "The slot must be reusable once the abandoned thread finishes"
        );
        Ok(())
    }
}
