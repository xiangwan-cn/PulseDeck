use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    LazyLock, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Default, Clone, Copy)]
struct InvalidationState {
    generation: u64,
    dirty: bool,
}

// Keep the invalidation state separate from the value cache. The state lock is
// held while a value is committed, so a slow collection that started before an
// event cannot repopulate a newly invalidated source with its old result.
static INVALIDATIONS: LazyLock<Mutex<HashMap<String, InvalidationState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
// Disk writes are infrequent and already bounded by the per-entry write
// interval. Serialize the final write/rename so consumers sharing a source
// descriptor cannot race through one temporary pathname.
static CACHE_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
pub const DEFAULT_LAST_GOOD_MAX_STALENESS_SECONDS: u64 = 86_400;

/// Cache files are namespaced by a stable descriptor hash, not by a card id.
/// The descriptor may contain paths, URLs, parser options, or command details;
/// none of that potentially sensitive content is written into the filename.
fn path(source_key: &str) -> PathBuf {
    cache_dir().join(format!("source-{:032x}.json", stable_hash(source_key)))
}

pub fn cache_path(source_key: &str) -> PathBuf {
    path(source_key)
}

fn stable_hash(value: &str) -> u128 {
    // Two independent deterministic FNV-1a lanes keep cache namespaces
    // collision-resistant without adding a hashing dependency or exposing
    // descriptor contents in filenames.
    let mut left = 0xcbf29ce484222325u64;
    let mut right = 0x84222325cbf29ce4_u64;
    for byte in value.as_bytes() {
        left ^= u64::from(*byte);
        left = left.wrapping_mul(0x100000001b3);
        right ^= u64::from(*byte).rotate_left(17);
        right = right.wrapping_mul(0x9e3779b185ebca87);
    }
    (u128::from(left) << 64) | u128::from(right)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_entry(source_key: &str) -> Option<DiskMetric> {
    if let Some(entry) = MEMORY_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(source_key).map(|entry| entry.entry.clone()))
    {
        return Some(entry);
    }
    crate::core::power_debug::increment(crate::core::power_debug::Counter::DiskRead);
    let entry: DiskMetric = serde_json::from_slice(&fs::read(path(source_key)).ok()?).ok()?;
    if let Ok(mut cache) = MEMORY_CACHE.lock() {
        cache.insert(
            source_key.to_string(),
            MemoryMetric {
                entry: entry.clone(),
                last_disk_write: Instant::now()
                    .checked_sub(Duration::from_secs(300))
                    .unwrap_or_else(Instant::now),
            },
        );
    }
    Some(entry)
}

fn valid_entry(entry: &DiskMetric, period: Option<&str>) -> bool {
    entry.version == 1
        && entry.period.as_deref() == period
        && entry.result.state == MetricState::Normal
}

fn cached_result(mut result: MetricResult) -> MetricResult {
    result.cached = true;
    if result.state == MetricState::Normal {
        result.state = MetricState::Stale;
    }
    result
}

/// Apply one source-agnostic budget contract to both fresh cache replay and
/// stale last-good fallback. Serialization is conservative: it counts the
/// complete generic metric payload rather than making assumptions about a
/// particular provider's text representation.
pub fn result_within_output_budget(result: &MetricResult, max_output_bytes: usize) -> bool {
    serde_json::to_vec(result)
        .map(|bytes| bytes.len() <= max_output_bytes.max(1))
        .unwrap_or(false)
}

/// Load a fresh-enough cached result. An invalidated source deliberately
/// bypasses this path so a source event cannot immediately replay old data.
pub fn load(
    source_key: &str,
    ttl_seconds: Option<u64>,
    period: Option<&str>,
) -> Option<MetricResult> {
    load_with_budget(source_key, ttl_seconds, period, usize::MAX)
}

/// Load a cache entry under the caller's current output budget. The budget is
/// deliberately checked at replay time rather than only when a result was
/// collected, because app-only hot reloads can lower it without changing a
/// card or source descriptor.
pub fn load_with_budget(
    source_key: &str,
    ttl_seconds: Option<u64>,
    period: Option<&str>,
    max_output_bytes: usize,
) -> Option<MetricResult> {
    let invalidations = INVALIDATIONS.lock().ok()?;
    if invalidations
        .get(source_key)
        .is_some_and(|state| state.dirty)
    {
        return None;
    }
    let entry = read_entry(source_key)?;
    drop(invalidations);
    if !valid_entry(&entry, period) || !result_within_output_budget(&entry.result, max_output_bytes)
    {
        return None;
    }
    if let Some(ttl) = ttl_seconds {
        if now_secs().saturating_sub(entry.saved_at) >= ttl {
            return None;
        }
    }
    Some(cached_result(entry.result))
}

/// Return the last successful value for a source even after an invalidation or
/// a failed refresh. Callers may expose it as a stale value while preserving a
/// failure diagnostic separately. No error/loading/unavailable value can enter
/// this path because store() only accepts Normal results.
pub fn load_last_good(source_key: &str, period: Option<&str>) -> Option<MetricResult> {
    load_last_good_with_max_age(source_key, None, period)
}

pub fn load_last_good_with_max_age(
    source_key: &str,
    max_staleness_seconds: Option<u64>,
    period: Option<&str>,
) -> Option<MetricResult> {
    load_last_good_with_max_age_and_budget(source_key, max_staleness_seconds, period, usize::MAX)
}

pub fn load_last_good_with_max_age_and_budget(
    source_key: &str,
    max_staleness_seconds: Option<u64>,
    period: Option<&str>,
    max_output_bytes: usize,
) -> Option<MetricResult> {
    let entry = read_entry(source_key)?;
    if !valid_entry(&entry, period) || !result_within_output_budget(&entry.result, max_output_bytes)
    {
        return None;
    }
    if max_staleness_seconds
        .is_some_and(|max_age| now_secs().saturating_sub(entry.saved_at) >= max_age)
    {
        return None;
    }
    Some(cached_result(entry.result))
}

/// Mark the source dirty in memory. The persisted last-good entry remains
/// available through load_last_good but cannot satisfy the normal fast path.
pub fn invalidation_token(source_key: &str) -> u64 {
    INVALIDATIONS
        .lock()
        .ok()
        .and_then(|states| states.get(source_key).map(|state| state.generation))
        .unwrap_or_default()
}

pub fn invalidate(source_key: &str) {
    if let Ok(mut states) = INVALIDATIONS.lock() {
        let state = states.entry(source_key.to_string()).or_default();
        state.generation = state.generation.wrapping_add(1);
        state.dirty = true;
    }
}

pub fn store(source_key: &str, period: Option<&str>, result: &MetricResult) -> io::Result<()> {
    let token = invalidation_token(source_key);
    let _ = store_if_current(source_key, period, result, token)?;
    Ok(())
}

/// Store a successful result only if no invalidation has happened since the
/// caller captured `token`. A `false` result is a benign stale-worker discard,
/// not an I/O failure.
pub fn store_if_current(
    source_key: &str,
    period: Option<&str>,
    result: &MetricResult,
    token: u64,
) -> io::Result<bool> {
    // A successful source result is the only thing allowed to replace the
    // last-good cache. Loading/unavailable/error states never age or overwrite
    // it, including after a source invalidation.
    if result.state != MetricState::Normal || result.cached {
        return Ok(false);
    }
    let _write_lock = CACHE_WRITE_LOCK
        .lock()
        .map_err(|_| io::Error::other("cache write lock poisoned"))?;
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;

    {
        let invalidations = INVALIDATIONS
            .lock()
            .map_err(|_| io::Error::other("cache invalidation lock poisoned"))?;
        if invalidations
            .get(source_key)
            .copied()
            .unwrap_or_default()
            .generation
            != token
        {
            return Ok(false);
        }
    }

    let (entry, should_write) = {
        let cache = MEMORY_CACHE
            .lock()
            .map_err(|_| io::Error::other("cache lock poisoned"))?;
        let previous = cache.get(source_key);
        let period = period.map(str::to_owned);
        let unchanged = previous.is_some_and(|previous| {
            previous.entry.period == period && same_result(&previous.entry.result, result)
        });
        let entry = if unchanged {
            previous
                .expect("unchanged implies previous entry")
                .entry
                .clone()
        } else {
            DiskMetric {
                version: 1,
                saved_at: now_secs(),
                period,
                result: result.clone(),
            }
        };
        let should_write = !unchanged
            || previous.is_none_or(|previous| {
                previous.last_disk_write.elapsed() >= Duration::from_secs(300)
            });
        (entry, should_write)
    };

    if !should_write {
        let mut invalidations = INVALIDATIONS
            .lock()
            .map_err(|_| io::Error::other("cache invalidation lock poisoned"))?;
        let current = invalidations.get(source_key).copied().unwrap_or_default();
        if current.generation != token {
            return Ok(false);
        }
        if let Some(state) = invalidations.get_mut(source_key) {
            state.dirty = false;
        }
        return Ok(true);
    }
    crate::core::power_debug::increment(crate::core::power_debug::Counter::DiskWrite);
    let target = path(source_key);
    let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = target.with_extension(format!("json.tmp-{}-{sequence}", std::process::id()));
    let bytes = serde_json::to_vec(&entry).map_err(io::Error::other)?;
    if let Err(error) = fs::write(&temporary, bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    // Keep invalidation and the final rename/commit ordered. Serialization and
    // the potentially slower write happen before this lock, while the final
    // generation check prevents an old worker from replacing a newer event's
    // value in either memory or on disk.
    let mut invalidations = INVALIDATIONS
        .lock()
        .map_err(|_| io::Error::other("cache invalidation lock poisoned"))?;
    let current = invalidations.get(source_key).copied().unwrap_or_default();
    if current.generation != token {
        let _ = fs::remove_file(&temporary);
        return Ok(false);
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    let last_disk_write = Instant::now();
    let mut cache = MEMORY_CACHE
        .lock()
        .map_err(|_| io::Error::other("cache lock poisoned"))?;
    cache.insert(
        source_key.to_owned(),
        MemoryMetric {
            entry,
            last_disk_write,
        },
    );
    if let Some(state) = invalidations.get_mut(source_key) {
        state.dirty = false;
    }
    Ok(true)
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

    fn normal(value: &str) -> MetricResult {
        MetricResult {
            value: crate::model::card_model::CardValue::Text(value.into()),
            subtitle: None,
            tooltip: None,
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }

    #[test]
    fn cache_file_name_contains_no_raw_source_descriptor() {
        let key = "http:url=https://user:secret@example.test/x";
        let file = path(key);
        let name = file.file_name().unwrap().to_string_lossy();
        assert!(!name.contains("secret"));
        assert!(name.starts_with("source-"));
    }

    #[test]
    fn descriptor_hash_is_stable_and_distinguishes_similar_keys() {
        assert_eq!(stable_hash("one"), stable_hash("one"));
        assert_ne!(stable_hash("one"), stable_hash("two"));
    }

    #[test]
    fn non_success_states_do_not_replace_last_good_value() {
        let key = format!("test:last-good:{}", std::process::id());
        let _ = store(&key, None, &normal("good"));
        let failed = MetricResult::error("failed");
        let _ = store(&key, None, &failed);
        assert_eq!(
            load_last_good(&key, None).unwrap().value,
            normal("good").value
        );
    }

    #[test]
    fn invalidation_bypasses_fast_load_but_keeps_last_good() {
        let key = format!("test:invalidate:{}", std::process::id());
        let _ = store(&key, None, &normal("old"));
        invalidate(&key);
        assert!(load(&key, None, None).is_none());
        assert_eq!(
            load_last_good(&key, None).unwrap().value,
            normal("old").value
        );
        let _ = store(&key, None, &normal("new"));
        assert_eq!(load(&key, None, None).unwrap().value, normal("new").value);
    }

    #[test]
    fn scheduled_and_unscheduled_periods_do_not_share_cache_entries() {
        let key = format!("test:period-boundary:{}", std::process::id());
        let _ = store(&key, Some("2024-01-01T08:00"), &normal("scheduled"));
        assert!(load(&key, None, None).is_none());
        assert!(load_last_good(&key, None).is_none());
        assert_eq!(
            load(&key, None, Some("2024-01-01T08:00")).unwrap().value,
            normal("scheduled").value
        );
    }

    #[test]
    fn stale_worker_token_cannot_clear_a_new_invalidation() {
        let key = format!("test:stale-token:{}", std::process::id());
        let _ = store(&key, None, &normal("old"));
        let token = invalidation_token(&key);
        invalidate(&key);
        assert!(!store_if_current(&key, None, &normal("stale"), token).unwrap());
        assert!(load(&key, None, None).is_none());
        assert_eq!(
            load_last_good(&key, None).unwrap().value,
            normal("old").value
        );
    }

    #[test]
    fn lowering_output_budget_rejects_replay_and_last_good_cache() {
        let key = format!("test:budget-lowering:{}", std::process::id());
        let result = normal(&"x".repeat(256));
        let _ = store(&key, None, &result);
        let serialized_size = serde_json::to_vec(&result).unwrap().len();
        assert!(load_with_budget(&key, None, None, serialized_size).is_some());
        assert!(load_with_budget(&key, None, None, 32).is_none());
        assert!(load_last_good_with_max_age_and_budget(&key, None, None, 32).is_none());
    }
}
