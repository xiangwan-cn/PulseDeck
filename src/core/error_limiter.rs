use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

struct ErrorState {
    last_log: Instant,
    suppressed: u64,
}

static ERRORS: LazyLock<Mutex<HashMap<String, ErrorState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn warn(key: impl Into<String>, message: impl AsRef<str>) {
    let key = key.into();
    let Ok(mut errors) = ERRORS.lock() else {
        return;
    };
    match errors.get_mut(&key) {
        Some(state) if state.last_log.elapsed() < Duration::from_secs(60) => {
            state.suppressed = state.suppressed.saturating_add(1);
        }
        Some(state) => {
            tracing::warn!(
                error_key = %key,
                suppressed = state.suppressed,
                "{}",
                message.as_ref()
            );
            state.last_log = Instant::now();
            state.suppressed = 0;
        }
        None => {
            tracing::warn!(error_key = %key, "{}", message.as_ref());
            errors.insert(
                key,
                ErrorState {
                    last_log: Instant::now(),
                    suppressed: 0,
                },
            );
        }
    }
}

pub fn recovered(key: &str) {
    let Ok(mut errors) = ERRORS.lock() else {
        return;
    };
    if let Some(state) = errors.remove(key) {
        tracing::info!(
            error_key = %key,
            suppressed = state.suppressed,
            "component recovered"
        );
    }
}
