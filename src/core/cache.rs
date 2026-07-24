use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::config::cache_dir;
use crate::model::metric_result::{MetricResult, MetricState};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskMetric {
    version: u8,
    saved_at: u64,
    period: Option<String>,
    result: MetricResult,
}

struct MemoryMetric {
    entry: DiskMetric,
    last_disk_write: Instant,
}

static MEMORY_CACHE: LazyLock<Mutex<HashMap<String, MemoryMetric>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn path(card_id: &str) -> PathBuf {
    let safe_id: String = card_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    cache_dir().join(format!("{}.json", safe_id))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn load(card_id: &str, ttl_seconds: Option<u64>, period: Option<&str>) -> Option<MetricResult> {
    let entry = if let Some(entry) = MEMORY_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(card_id).map(|entry| entry.entry.clone()))
    {
        entry
    } else {
        crate::core::power_debug::increment(crate::core::power_debug::Counter::DiskRead);
        let entry: DiskMetric = serde_json::from_slice(&fs::read(path(card_id)).ok()?).ok()?;
        if let Ok(mut cache) = MEMORY_CACHE.lock() {
            cache.insert(
                card_id.to_string(),
                MemoryMetric {
                    entry: entry.clone(),
                    last_disk_write: Instant::now()
                        .checked_sub(Duration::from_secs(300))
                        .unwrap_or_else(Instant::now),
                },
            );
        }
        entry
    };
    if entry.version != 1 {
        return None;
    }
    if let Some(expected) = period {
        if entry.period.as_deref() != Some(expected) {
            return None;
        }
    }
    if let Some(ttl) = ttl_seconds {
        if now_secs().saturating_sub(entry.saved_at) >= ttl {
            return None;
        }
    }
    let mut result = entry.result;
    if result.state == MetricState::Error {
        return None;
    }
    result.cached = true;
    if result.state == MetricState::Normal {
        result.state = MetricState::Stale;
    }
    Some(result)
}

pub fn store(card_id: &str, period: Option<&str>, result: &MetricResult) -> io::Result<()> {
    if result.state == MetricState::Error {
        return Ok(());
    }
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;
    let target = path(card_id);
    let temporary = target.with_extension("json.tmp");
    let entry = DiskMetric {
        version: 1,
        saved_at: now_secs(),
        period: period.map(str::to_owned),
        result: result.clone(),
    };
    let should_write = {
        let mut cache = MEMORY_CACHE
            .lock()
            .map_err(|_| io::Error::other("cache lock poisoned"))?;
        let should_write = cache.get(card_id).map_or(true, |previous| {
            previous.entry.period != entry.period
                || !same_result(&previous.entry.result, result)
                || previous.last_disk_write.elapsed() >= Duration::from_secs(300)
        });
        let last_disk_write = if should_write {
            Instant::now()
        } else {
            cache
                .get(card_id)
                .map(|previous| previous.last_disk_write)
                .unwrap_or_else(Instant::now)
        };
        cache.insert(
            card_id.to_string(),
            MemoryMetric {
                entry: entry.clone(),
                last_disk_write,
            },
        );
        should_write
    };
    if !should_write {
        return Ok(());
    }
    crate::core::power_debug::increment(crate::core::power_debug::Counter::DiskWrite);
    let bytes = serde_json::to_vec(&entry).map_err(io::Error::other)?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, target)
}

fn same_result(left: &MetricResult, right: &MetricResult) -> bool {
    left.value == right.value
        && left.subtitle == right.subtitle
        && left.tooltip == right.tooltip
        && left.state == right.state
        && left.cached == right.cached
        && left.metadata == right.metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_file_name_is_sanitized() {
        assert!(path("../../bad").ends_with("______bad.json"));
    }
}
