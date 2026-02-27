use std::time::UNIX_EPOCH;

pub(crate) trait ServerClock: Send + Sync {
    fn now_unix_secs(&self) -> String;
}

pub(crate) struct SystemServerClock;

impl ServerClock for SystemServerClock {
    fn now_unix_secs(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        now.as_secs().to_string()
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use super::{ServerClock, SystemServerClock};

    #[test]
    fn given_system_clock__when_now_unix_secs__then_returns_numeric_string() {
        let clock = SystemServerClock;
        let now = clock.now_unix_secs();

        assert!(
            now.chars().all(|ch| ch.is_ascii_digit()),
            "timestamp should be digits only, got: {now}"
        );
        assert!(!now.is_empty(), "timestamp string should not be empty");
    }
}
