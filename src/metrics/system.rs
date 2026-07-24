use std::fs;
use std::time::Instant;

use crate::model::card_model::{CardValue, StatusLevel};
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

/// Extra native system metrics. They are registered capabilities, not default cards:
/// users enable them by adding a `source.type = "builtin"` card to config.toml.
pub enum SystemMetric {
    LoadAverage,
    Swap,
    ProcessCount,
    CpuTemperature,
    Filesystem,
    NetworkTraffic {
        previous: Option<(Instant, u64, u64)>,
    },
}

impl SystemMetric {
    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        match self {
            Self::LoadAverage => load_average(),
            Self::Swap => swap_usage(ctx),
            Self::ProcessCount => process_count(),
            Self::CpuTemperature => cpu_temperature(),
            Self::Filesystem => filesystem_usage(),
            Self::NetworkTraffic { previous } => network_traffic(previous),
        }
    }
}

fn normal(value: CardValue, subtitle: Option<String>) -> MetricResult {
    MetricResult {
        value,
        subtitle,
        tooltip: None,
        state: MetricState::Normal,
        cached: false,
        metadata: None,
    }
}

fn load_average() -> MetricResult {
    match fs::read_to_string("/proc/loadavg") {
        Ok(text) => {
            let values: Vec<_> = text.split_whitespace().take(3).collect();
            normal(
                CardValue::Text(values.first().copied().unwrap_or("-").into()),
                Some(format!("1 / 5 / 15 分钟：{}", values.join(" / "))),
            )
        }
        Err(e) => MetricResult::error(format!("读取负载失败: {e}")),
    }
}

fn swap_usage(ctx: &MetricContext) -> MetricResult {
    let info = match ctx.procfs.lock().unwrap().read_meminfo() {
        Ok(value) => value,
        Err(e) => return MetricResult::error(format!("读取交换空间失败: {e}")),
    };
    let total = info.swap_total_kb;
    let free = info.swap_free_kb;
    let used = total.saturating_sub(free);
    let percent = if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    };
    normal(
        CardValue::Percentage(percent),
        Some(format!(
            "已用 {} / {}",
            bytesize::ByteSize(used * 1024),
            bytesize::ByteSize(total * 1024)
        )),
    )
}

fn process_count() -> MetricResult {
    match fs::read_dir("/proc") {
        Ok(entries) => {
            let count = entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .bytes()
                        .all(|b| b.is_ascii_digit())
                })
                .count();
            normal(
                CardValue::Number {
                    value: count as f64,
                    unit: Some("个".into()),
                    decimals: 0,
                },
                Some("当前进程数".into()),
            )
        }
        Err(e) => MetricResult::error(format!("读取进程失败: {e}")),
    }
}

fn cpu_temperature() -> MetricResult {
    let zones = match fs::read_dir("/sys/class/thermal") {
        Ok(v) => v,
        Err(e) => return MetricResult::unavailable(format!("无温度传感器: {e}")),
    };
    let mut readings = Vec::new();
    for zone in zones.flatten() {
        let path = zone.path();
        if !zone
            .file_name()
            .to_string_lossy()
            .starts_with("thermal_zone")
        {
            continue;
        }
        let kind = fs::read_to_string(path.join("type"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        let raw = fs::read_to_string(path.join("temp"))
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok());
        if let Some(raw) = raw {
            readings.push((
                kind,
                if raw.abs() > 1000.0 {
                    raw / 1000.0
                } else {
                    raw
                },
            ));
        }
    }
    let selected = readings
        .iter()
        .find(|(k, _)| {
            let k = k.to_ascii_lowercase();
            k.contains("cpu") || k.contains("soc") || k.contains("package")
        })
        .or_else(|| readings.first());
    match selected {
        Some((kind, temp)) => normal(
            CardValue::Number {
                value: *temp,
                unit: Some("°C".into()),
                decimals: 1,
            },
            Some(kind.clone()),
        ),
        None => MetricResult::unavailable("未发现可用温度传感器"),
    }
}

fn filesystem_usage() -> MetricResult {
    let path = std::ffi::CString::new("/").unwrap();
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return MetricResult::error("读取根文件系统失败");
    }
    let stats = unsafe { stats.assume_init() };
    let total = stats.f_blocks as u64 * stats.f_frsize as u64;
    let available = stats.f_bavail as u64 * stats.f_frsize as u64;
    let used = total.saturating_sub(available);
    let percent = if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    };
    normal(
        CardValue::Percentage(percent),
        Some(format!(
            "已用 {} / {}",
            bytesize::ByteSize(used),
            bytesize::ByteSize(total)
        )),
    )
}

fn network_traffic(previous: &mut Option<(Instant, u64, u64)>) -> MetricResult {
    let text = match fs::read_to_string("/proc/net/dev") {
        Ok(v) => v,
        Err(e) => return MetricResult::error(format!("读取网络流量失败: {e}")),
    };
    let (mut rx, mut tx) = (0u64, 0u64);
    for line in text.lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<_> = values.split_whitespace().collect();
        rx = rx.saturating_add(fields.first().and_then(|v| v.parse().ok()).unwrap_or(0));
        tx = tx.saturating_add(fields.get(8).and_then(|v| v.parse().ok()).unwrap_or(0));
    }
    let now = Instant::now();
    let rates = previous.map(|(at, old_rx, old_tx)| {
        let secs = now.duration_since(at).as_secs_f64().max(0.001);
        (
            (rx.saturating_sub(old_rx) as f64 / secs) as u64,
            (tx.saturating_sub(old_tx) as f64 / secs) as u64,
        )
    });
    *previous = Some((now, rx, tx));
    match rates {
        Some((down, up)) => normal(
            CardValue::Text(format!("↓ {}/s", bytesize::ByteSize(down))),
            Some(format!("↑ {}/s", bytesize::ByteSize(up))),
        ),
        None => normal(
            CardValue::Status {
                label: "采样中".into(),
                level: StatusLevel::Normal,
            },
            Some("等待下一次采样".into()),
        ),
    }
}
