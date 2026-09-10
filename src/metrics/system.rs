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
            Self::LoadAverage => load_average(ctx),
            Self::Swap => swap_usage(ctx),
            Self::ProcessCount => process_count(ctx),
            Self::CpuTemperature => cpu_temperature(ctx),
            Self::Filesystem => filesystem_usage(),
            Self::NetworkTraffic { previous } => network_traffic(ctx, previous),
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

fn load_average(ctx: &MetricContext) -> MetricResult {
    match ctx.procfs.lock().unwrap().read_load_average() {
        Ok(values) => normal(
            CardValue::Text(values.first().cloned().unwrap_or_else(|| "-".into())),
            Some(format!("1 / 5 / 15 分钟：{}", values.join(" / "))),
        ),
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

fn process_count(ctx: &MetricContext) -> MetricResult {
    match ctx.procfs.lock().unwrap().process_count() {
        Ok(count) => normal(
            CardValue::Number {
                value: count as f64,
                unit: Some("个".into()),
                decimals: 0,
            },
            Some("当前进程数".into()),
        ),
        Err(e) => MetricResult::error(format!("读取进程失败: {e}")),
    }
}

fn cpu_temperature(ctx: &MetricContext) -> MetricResult {
    match crate::sources::thermal::select_cpu_temperature(&ctx.thermal_root) {
        Some(reading) => normal(
            CardValue::Number {
                value: reading.celsius,
                unit: Some("°C".into()),
                decimals: 1,
            },
            Some(reading.kind),
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

fn network_traffic(
    ctx: &MetricContext,
    previous: &mut Option<(Instant, u64, u64)>,
) -> MetricResult {
    let (rx, tx) = match ctx.procfs.lock().unwrap().network_totals() {
        Ok(totals) => totals,
        Err(e) => return MetricResult::error(format!("读取网络流量失败: {e}")),
    };
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
