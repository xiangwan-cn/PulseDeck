use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct MemoryMetric;

impl MemoryMetric {
    pub fn new() -> Self {
        Self
    }

    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        let meminfo = match ctx.procfs.lock().unwrap().read_meminfo() {
            Ok(m) => m,
            Err(e) => {
                return MetricResult::error(format!("读取 /proc/meminfo 失败: {}", e));
            }
        };

        if meminfo.total_kb == 0 {
            return MetricResult {
                value: CardValue::Text("不可用".into()),
                subtitle: None,
                tooltip: Some("MemTotal 为 0".into()),
                state: MetricState::Unavailable,
                cached: false,
                metadata: None,
            };
        }

        let total_kb = meminfo.total_kb;
        let available_kb = meminfo.available_kb;
        let used_kb = total_kb.saturating_sub(available_kb);

        let ratio = (used_kb as f64 / total_kb as f64).clamp(0.0, 1.0);
        let pct = (ratio * 100.0).clamp(0.0, 100.0);

        let used_gib = used_kb as f64 / 1_048_576.0;
        let total_gib = total_kb as f64 / 1_048_576.0;
        let subtitle = format!("{:.1} / {:.1} GiB", used_gib, total_gib);

        MetricResult {
            value: CardValue::Percentage(pct),
            subtitle: Some(subtitle),
            tooltip: Some(format!(
                "已用 {:.1} GiB / 总计 {:.1} GiB · {:.1}%",
                used_gib, total_gib, pct
            )),
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }
}
