use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    LazyLock, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
    bytes: usize,
    lru_prev: Option<String>,
    lru_next: Option<String>,
}

struct MemoryCache {
    entries: HashMap<String, MemoryMetric>,
    total_bytes: usize,
    lru_head: Option<String>,
    lru_tail: Option<String>,
}

const MAX_MEMORY_CACHE_ENTRIES: usize = 256;
const MAX_MEMORY_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_DISK_CACHE_ENTRIES: usize = 512;
const MAX_DISK_CACHE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DISK_CACHE_AGE_SECONDS: u64 = 7 * 24 * 60 * 60;
const STALE_TEMPORARY_FILE_AGE_SECONDS: u64 = 60 * 60;

static MEMORY_CACHE: LazyLock<Mutex<MemoryCache>> = LazyLock::new(|| {
    Mutex::new(MemoryCache {
        entries: HashMap::new(),
        total_bytes: 0,
        lru_head: None,
        lru_tail: None,
    })
});

#[derive(Debug, Default, Clone, Copy)]
struct InvalidationState {
    generation: u64,
    dirty: bool,
}

#[cfg(feature = "power-debug")]
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

fn ensure_cache_dir() -> io::Result<PathBuf> {
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Return a stable, non-sensitive identifier for diagnostics. Source
/// descriptors can contain HTTP credentials, request bodies, or command
/// arguments; error keys must not copy those values into journal records.
pub fn diagnostic_key(source_key: &str) -> String {
    format!("cache:{:032x}", stable_hash(source_key))
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
        // A serialization failure must be treated as an uncacheable, very
        // large entry. Counting it as zero would let it bypass byte-based LRU
        // eviction indefinitely.
        .unwrap_or(usize::MAX)
}

fn insert_memory_entry(
    cache: &mut MemoryCache,
    source_key: String,
    entry: DiskMetric,
    last_disk_write: Instant,
) {
    let entry = MemoryMetric {
        bytes: entry_size(&entry),
        entry,
        last_disk_write,
        lru_prev: None,
        lru_next: None,
    };
    let new_bytes = entry.bytes;
    if cache.entries.contains_key(&source_key) {
        detach_memory_entry(cache, &source_key);
    }
    if let Some(previous) = cache.entries.insert(source_key.clone(), entry) {
        cache.total_bytes = cache.total_bytes.saturating_sub(previous.bytes);
    }
    cache.total_bytes = cache.total_bytes.saturating_add(new_bytes);
    attach_memory_entry(cache, &source_key);

    while cache.entries.len() > MAX_MEMORY_CACHE_ENTRIES
        || cache.total_bytes > MAX_MEMORY_CACHE_BYTES
    {
        if let Some(removed) = pop_oldest_memory_entry(cache) {
            cache.total_bytes = cache.total_bytes.saturating_sub(removed.bytes);
        } else {
            break;
        }
    }
}

/// Maintain an intrusive LRU list alongside the hash map. Touches and
/// evictions are O(1), so a large cache cannot turn every insert into a full
/// scan of all entries.
fn detach_memory_entry(cache: &mut MemoryCache, key: &str) {
    let Some((previous, next)) = cache
        .entries
        .get(key)
        .map(|entry| (entry.lru_prev.clone(), entry.lru_next.clone()))
    else {
        return;
    };
    if let Some(previous) = previous.as_deref() {
        if let Some(entry) = cache.entries.get_mut(previous) {
            entry.lru_next = next.clone();
        }
    } else {
        cache.lru_head = next.clone();
    }
    if let Some(next) = next.as_deref() {
        if let Some(entry) = cache.entries.get_mut(next) {
            entry.lru_prev = previous.clone();
        }
    } else {
        cache.lru_tail = previous.clone();
    }
    if let Some(entry) = cache.entries.get_mut(key) {
        entry.lru_prev = None;
        entry.lru_next = None;
    }
}

fn attach_memory_entry(cache: &mut MemoryCache, key: &str) {
    let previous_tail = cache.lru_tail.clone();
    if let Some(previous_tail) = previous_tail.as_deref() {
        if let Some(entry) = cache.entries.get_mut(previous_tail) {
            entry.lru_next = Some(key.to_owned());
        }
    } else {
        cache.lru_head = Some(key.to_owned());
    }
    if let Some(entry) = cache.entries.get_mut(key) {
        entry.lru_prev = previous_tail;
        entry.lru_next = None;
    }
    cache.lru_tail = Some(key.to_owned());
}

fn touch_memory_entry(cache: &mut MemoryCache, key: &str) {
    if cache.lru_tail.as_deref() == Some(key) {
        return;
    }
    detach_memory_entry(cache, key);
    attach_memory_entry(cache, key);
}

fn remove_memory_entry(cache: &mut MemoryCache, key: &str) -> Option<MemoryMetric> {
    if cache.entries.contains_key(key) {
        detach_memory_entry(cache, key);
    }
    cache.entries.remove(key)
}

fn pop_oldest_memory_entry(cache: &mut MemoryCache) -> Option<MemoryMetric> {
    let key = cache.lru_head.clone()?;
    remove_memory_entry(cache, &key)
}

fn cleanup_disk_cache() {
    let tick = DISK_CLEANUP_TICK.fetch_add(1, Ordering::Relaxed);
    if tick % 32 != 0 {
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
    let mut remove_index = 0;
    while files.len().saturating_sub(remove_index) > MAX_DISK_CACHE_ENTRIES
        || total_bytes > MAX_DISK_CACHE_BYTES
    {
        let Some((path, _, size)) = files.get(remove_index).cloned() else {
            break;
        };
        remove_index += 1;
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

fn read_entry(source_key: &str, max_output_bytes: usize) -> Option<DiskMetric> {
    if let Ok(mut cache) = MEMORY_CACHE.lock() {
        if cache.entries.contains_key(source_key) {
            touch_memory_entry(&mut cache, source_key);
            let cached = cache.entries.get(source_key)?;
            if !result_within_output_budget(&cached.entry.result, max_output_bytes) {
                return None;
            }
            return Some(cached.entry.clone());
        }
    }
    crate::core::power_debug::increment(crate::core::power_debug::Counter::DiskRead);
    let cache_path = path(source_key);
    let read_limit = max_output_bytes
        .max(1)
        .saturating_add(64 * 1024)
        .min(MAX_DISK_CACHE_BYTES as usize);
    let metadata = fs::metadata(&cache_path).ok()?;
    if metadata.len() > read_limit as u64 {
        return None;
    }
    let mut file = fs::File::open(&cache_path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take((read_limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > read_limit {
        return None;
    }
    let entry: DiskMetric = match serde_json::from_slice(&bytes) {
        Ok(entry) => entry,
        Err(error) => {
            crate::core::error_limiter::warn(
                diagnostic_key(source_key),
                format!("discarded invalid cache entry: {error}"),
            );
            let _ = fs::remove_file(&cache_path);
            return None;
        }
    };
    if entry.version != 1 {
        crate::core::error_limiter::warn(
            diagnostic_key(source_key),
            format!("discarded unsupported cache version {}", entry.version),
        );
        let _ = fs::remove_file(&cache_path);
        return None;
    }
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
#[cfg(test)]
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
    let entry = read_entry(source_key, max_output_bytes)?;
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
#[cfg(test)]
pub fn load_last_good(source_key: &str, period: Option<&str>) -> Option<MetricResult> {
    load_last_good_with_max_age(source_key, None, period)
}

#[cfg(test)]
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
    let entry = read_entry(source_key, max_output_bytes)?;
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
        if let Some(removed) = remove_memory_entry(&mut cache, source_key) {
            cache.total_bytes = cache.total_bytes.saturating_sub(removed.bytes);
        }
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
        let removed_keys = cache
            .entries
            .keys()
            .filter(|key| !active_source_keys.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed_keys {
            if let Some(removed) = remove_memory_entry(&mut cache, &key) {
                cache.total_bytes = cache.total_bytes.saturating_sub(removed.bytes);
            }
        }
    }
}

#[cfg(feature = "power-debug")]
pub fn stats() -> CacheStats {
    let invalidation_entries = INVALIDATIONS
        .lock()
        .map(|states| states.len())
        .unwrap_or_default();
    let (memory_entries, memory_bytes) = MEMORY_CACHE
        .lock()
        .map(|cache| (cache.entries.len(), cache.total_bytes))
        .unwrap_or_default();
    CacheStats {
        memory_entries,
        memory_bytes,
        invalidation_entries,
    }
}

#[cfg(test)]
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
    ensure_cache_dir()?;

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
        let previous = cache.entries.get(source_key);
        let period = period.map(str::to_owned);
        let unchanged = previous.is_some_and(|previous| {
            previous.entry.period == period && same_result(&previous.entry.result, result)
        });
        let entry = match (unchanged, previous) {
            (true, Some(previous)) => previous.entry.clone(),
            _ => DiskMetric {
                version: 1,
                saved_at: now_secs(),
                period,
                result: result.clone(),
            },
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
    #[cfg(unix)]
    if let Err(error) = fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)) {
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
    #[cfg(unix)]
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;

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

    #[test]
    fn memory_cache_lru_touch_and_byte_accounting_are_consistent() {
        let mut cache = MemoryCache {
            entries: HashMap::new(),
            total_bytes: 0,
            lru_head: None,
            lru_tail: None,
        };
        for index in 0..MAX_MEMORY_CACHE_ENTRIES {
            let key = format!("lru-{index}");
            insert_memory_entry(
                &mut cache,
                key,
                DiskMetric {
                    version: 1,
                    saved_at: 0,
                    period: None,
                    result: normal("value"),
                },
                Instant::now(),
            );
        }
        touch_memory_entry(&mut cache, "lru-0");
        insert_memory_entry(
            &mut cache,
            "lru-new".into(),
            DiskMetric {
                version: 1,
                saved_at: 0,
                period: None,
                result: normal("value"),
            },
            Instant::now(),
        );

        assert_eq!(cache.entries.len(), MAX_MEMORY_CACHE_ENTRIES);
        assert!(!cache.entries.contains_key("lru-1"));
        assert!(cache.entries.contains_key("lru-0"));
        assert_eq!(cache.lru_head.as_deref(), Some("lru-2"));
        assert_eq!(
            cache
                .entries
                .values()
                .map(|entry| entry.bytes)
                .sum::<usize>(),
            cache.total_bytes
        );
        assert!(cache.total_bytes <= MAX_MEMORY_CACHE_BYTES);
    }
}
