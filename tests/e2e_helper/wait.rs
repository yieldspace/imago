use anyhow::{Result, anyhow};
use std::thread;
use std::time::{Duration, Instant};

/// Polls `poll` until it returns `Some(T)` or the timeout expires.
///
/// - `label`: Human-readable name used in timeout errors.
/// - `timeout`: Maximum time to keep polling.
/// - `interval`: Sleep duration between poll attempts.
/// - `poll`: Returns `Ok(None)` while the condition is not ready yet,
///   `Ok(Some(value))` once ready, or `Err(_)` for terminal failures.
///
/// Returns `Ok(T)` on success, and `Err` when polling fails or times out.
pub fn poll_until<T, F>(
    label: &str,
    timeout: Duration,
    interval: Duration,
    mut poll: F,
) -> Result<T>
where
    F: FnMut() -> Result<Option<T>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = poll()? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for {label} within {}ms",
                timeout.as_millis()
            ));
        }
        thread::sleep(interval);
    }
}
