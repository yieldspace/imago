use anyhow::{Result, anyhow};
use std::thread;
use std::time::{Duration, Instant};

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
