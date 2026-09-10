use std::collections::{HashMap, HashSet};
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
    last_access: Instant,
    bytes: usize,
}

const MAX_MEMORY_CACHE_ENTRIES: usize = 256;
const MAX_MEMORY_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_DISK_CACHE_ENTRIES: usize = 512;
const MAX_DISK_CACHE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DISK_CACHE_AGE_SECONDS: u64 = 7 * 24 * 60 * 60;
const STALE_TEMPORARY_FILE_AGE_SECONDS: u64 = 60 * 60;

static MEMORY_CACHE: LazyLock<Mutex<HashMap<String, MemoryMetric>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Default, Clone, Copy)]
struct InvalidationState {
    generation: u64,
    dirty: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub memory_entries: usize,
    pub memory_bytes: usize,
    pub invalidation_entries: usize,
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
static INVALIDATION_GENERATION_SEED: AtomicU64 = AtomicU64::new(1);
static DISK_CLEANUP_TICK: AtomicU64 = AtomicU64::new(0);
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

fn entry_size(entry: &DiskMetric) -> usize {
    serde_json::to_vec(entry)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn insert_memory_entry(
    cache: &mut HashMap<String, MemoryMetric>,
    source_key: String,
    entry: DiskMetric,
    last_disk_write: Instant,
) {
    let now = Instant::now();
    cache.insert(
        source_key,
        MemoryMetric {
            bytes: entry_size(&entry),
            entry,
            last_disk_write,
            last_access: now,
        },
    );

    while cache.len() > MAX_MEMORY_CACHE_ENTRIES
        || cache.values().map(|value| value.bytes).sum::<usize>() > MAX_MEMORY_CACHE_BYTES
    {
        let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, value)| value.last_access)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&oldest_key);
    }
}

fn cleanup_disk_cache() {
    let tick = DISK_CLEANUP_TICK.fetch_add(1, Ordering::Relaxed);
    if !tick.is_multiple_of(32) {
        return;
    }

    let dir = cache_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let now = now_secs();
    let mut files: Vec<(PathBuf, u64, u64)> = Vec::new();

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(now);
        let age = now.saturating_sub(modified);

        if file_name.starts_with("source-") && file_name.ends_with(".json") {
            if age > MAX_DISK_CACHE_AGE_SECONDS {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            files.push((entry.path(), modified, size));
        } else if file_name.starts_with("source-")
            && file_name.contains(".json.tmp-")
            && age > STALE_TEMPORARY_FILE_AGE_SECONDS
        {
            let _ = fs::remove_file(entry.path());
        }
    }

    files.sort_by_key(|(_, modified, _)| *modified);
    let mut total_bytes: u64 = files.iter().map(|(_, _, size)| *size).sum();
    while files.len() > MAX_DISK_CACHE_ENTRIES || total_bytes > MAX_DISK_CACHE_BYTES {
        let Some((path, _, size)) = files.first().cloned() else {
            break;
        };
        files.remove(0);
        if fs::remove_file(path).is_ok() {
            total_bytes = total_bytes.saturating_sub(size);
        }
    }
}

/// Run the low-priority disk janitor from a worker instead of the GTK main
/// context. It is intentionally public so application startup can schedule a
/// single cleanup even before the first successful source write.
pub fn cleanup() {
    cleanup_disk_cache();
}

fn read_entry(source_key: &str) -> Option<DiskMetric> {
    if let Ok(mut cache) = MEMORY_CACHE.lock() {
        if let Some(cached) = cache.get_mut(source_key) {
            cached.last_access = Instant::now();
            return Some(cached.entry.clone());
        }
    }
    crate::core::power_debug::increment(crate::core::power_debug::Counter::DiskRead);
    let cache_path = path(source_key);
    if fs::metadata(&cache_path)
        .ok()
        .is_some_and(|metadata| metadata.len() > MAX_DISK_CACHE_BYTES)
    {
        return None;
    }
    let entry: DiskMetric = serde_json::from_slice(&fs::read(cache_path).ok()?).ok()?;
    if let Ok(mut cache) = MEMORY_CACHE.lock() {
        insert_memory_entry(
            &mut cache,
            source_key.to_string(),
            entry.clone(),
            Instant::now()
                .checked_sub(Duration::from_secs(300))
                .unwrap_or_else(Instant::now),
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
    let generation = {
        let invalidations = INVALIDATIONS.lock().ok()?;
        if invalidations
            .get(source_key)
            .is_some_and(|state| state.dirty)
        {
            return None;
        }
        invalidations
            .get(source_key)
            .map(|state| state.generation)
            .unwrap_or_default()
    };
    let entry = read_entry(source_key)?;
    let still_current = INVALIDATIONS.lock().ok().is_some_and(|invalidations| {
        invalidations
            .get(source_key)
            .is_none_or(|state| !state.dirty && state.generation == generation)
    });
    if !still_current {
        return None;
    }
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
        .map(|mut states| {
            states
                .entry(source_key.to_string())
                .or_insert_with(|| InvalidationState {
                    generation: INVALIDATION_GENERATION_SEED.fetch_add(1, Ordering::Relaxed),
                    dirty: false,
                })
                .generation
        })
        .unwrap_or_default()
}

/// Check a worker generation without creating bookkeeping for a source that
/// may already have been removed by configuration reconciliation.
pub fn matches_invalidation_generation(source_key: &str, generation: u64) -> bool {
    INVALIDATIONS.lock().ok().is_some_and(|states| {
        states
            .get(source_key)
            .is_some_and(|state| state.generation == generation)
    })
}

pub fn invalidate(source_key: &str) {
    if let Ok(mut states) = INVALIDATIONS.lock() {
        let state = states
            .entry(source_key.to_string())
            .or_insert_with(|| InvalidationState {
                generation: INVALIDATION_GENERATION_SEED.fetch_add(1, Ordering::Relaxed),
                dirty: false,
            });
        state.generation = state.generation.wrapping_add(1);
        state.dirty = true;
    }
}

/// Release in-memory cache and invalidation bookkeeping when a source node is
/// removed by a configuration reload. The disk entry is retained for the
/// bounded janitor so a temporarily removed card does not cause needless
/// external work if it is added again.
pub fn forget(source_key: &str) {
    if let Ok(mut invalidations) = INVALIDATIONS.lock() {
        invalidations.remove(source_key);
    }
    if let Ok(mut cache) = MEMORY_CACHE.lock() {
        cache.remove(source_key);
    }
}

/// Drop in-memory values and invalidation generations that no longer belong to
/// a live card. Disk entries are intentionally left to the janitor so a
/// temporarily removed source can still be reused without making a blocking
/// filesystem operation part of configuration reconciliation.
pub fn prune(active_source_keys: &HashSet<String>) {
    if let Ok(mut invalidations) = INVALIDATIONS.lock() {
        invalidations.retain(|key, _| active_source_keys.contains(key));
    }
    if let Ok(mut cache) = MEMORY_CACHE.lock() {
        cache.retain(|key, _| active_source_keys.contains(key));
    }
}

pub fn stats() -> CacheStats {
    let invalidation_entries = INVALIDATIONS
        .lock()
        .map(|states| states.len())
        .unwrap_or_default();
    let (memory_entries, memory_bytes) = MEMORY_CACHE
        .lock()
        .map(|cache| {
            (
                cache.len(),
                cache.values().map(|entry| entry.bytes).sum::<usize>(),
            )
        })
        .unwrap_or_default();
    CacheStats {
        memory_entries,
        memory_bytes,
        invalidation_entries,
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
        let Some(current) = invalidations.get(source_key) else {
            // A source was removed and its bookkeeping was pruned while this
            // worker was still finishing. A missing state is a tombstone, not
            // a fresh generation; reject the old result instead of allowing it
            // to recreate an inactive cache entry.
            return Ok(false);
        };
        if current.generation != token {
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
        let Some(current) = invalidations.get(source_key).copied() else {
            return Ok(false);
        };
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
    if bytes.len() as u64 > MAX_DISK_CACHE_BYTES {
        return Ok(false);
    }
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
    let Some(current) = invalidations.get(source_key).copied() else {
        let _ = fs::remove_file(&temporary);
        return Ok(false);
    };
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
    insert_memory_entry(&mut cache, source_key.to_owned(), entry, last_disk_write);
    if let Some(state) = invalidations.get_mut(source_key) {
        state.dirty = false;
    }
    drop(cache);
    drop(invalidations);
    cleanup_disk_cache();
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
    fn removed_source_rejects_a_worker_with_an_old_token() {
        let key = format!("test:removed-source:{}", std::process::id());
        let token = invalidation_token(&key);
        forget(&key);
        assert!(!matches_invalidation_generation(&key, token));
        assert!(!store_if_current(&key, None, &normal("stale"), token).unwrap());
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
