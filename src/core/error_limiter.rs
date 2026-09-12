use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

struct ErrorState {
    last_log: Instant,
    last_touched: Instant,
    suppressed: u64,
}

static ERRORS: LazyLock<Mutex<HashMap<String, ErrorState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const MAX_ERROR_KEYS: usize = 1024;
const ERROR_KEY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn warn(key: impl Into<String>, message: impl AsRef<str>) {
    let key = key.into();
    let Ok(mut errors) = ERRORS.lock() else {
        return;
    };
    errors.retain(|_, state| state.last_touched.elapsed() < ERROR_KEY_TTL);
    if !errors.contains_key(&key) && errors.len() >= MAX_ERROR_KEYS {
        if let Some(oldest) = errors
            .iter()
            .min_by_key(|(_, state)| state.last_touched)
            .map(|(key, _)| key.clone())
        {
            errors.remove(&oldest);
        }
    }
    match errors.get_mut(&key) {
        Some(state) if state.last_log.elapsed() < Duration::from_secs(60) => {
            state.suppressed = state.suppressed.saturating_add(1);
            state.last_touched = Instant::now();
        }
        Some(state) => {
            tracing::warn!(
                error_key = %key,
                suppressed = state.suppressed,
                "{}",
                message.as_ref()
            );
            state.last_log = Instant::now();
            state.last_touched = state.last_log;
            state.suppressed = 0;
        }
        None => {
            tracing::warn!(error_key = %key, "{}", message.as_ref());
            errors.insert(
                key,
                ErrorState {
                    last_log: Instant::now(),
                    last_touched: Instant::now(),
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
