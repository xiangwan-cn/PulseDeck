use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::config::cache_dir;
use crate::model::metric_result::{MetricResult, MetricState};

#[derive(Debug, Serialize, Deserialize)]
struct DiskMetric {
    version: u8,
    saved_at: u64,
    period: Option<String>,
    result: MetricResult,
}

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
    let entry: DiskMetric = serde_json::from_slice(&fs::read(path(card_id)).ok()?).ok()?;
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
    let bytes = serde_json::to_vec(&entry).map_err(io::Error::other)?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_file_name_is_sanitized() {
        assert!(path("../../bad").ends_with("______bad.json"));
    }
}
