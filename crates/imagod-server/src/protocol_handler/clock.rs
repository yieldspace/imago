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
