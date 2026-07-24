use crate::model::card_model::CardValue;
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

pub struct CpuMetric {
    prev: Option<crate::sources::procfs::CpuStat>,
    smoothed: Option<f64>,
    alpha: f64,
}

impl CpuMetric {
    pub fn new(alpha: Option<f64>) -> Self {
        Self {
            prev: None,
            smoothed: None,
            alpha: alpha.unwrap_or(0.3),
        }
    }

    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        let curr = match ctx.procfs.lock().unwrap().read_stat() {
            Ok(s) => s,
            Err(e) => {
                return MetricResult::error(format!("读取 /proc/stat 失败: {}", e));
            }
        };

        let Some(prev) = self.prev.replace(curr) else {
            return MetricResult {
                value: CardValue::Text("首次采集...".into()),
                subtitle: None,
                tooltip: Some("等待下次刷新获取差值".into()),
                state: MetricState::Loading,
                cached: false,
                metadata: None,
            };
        };

        let prev_total = prev.total();
        let prev_idle = prev.idle_total();
        let curr_total = curr.total();
        let curr_idle = curr.idle_total();

        let total_delta = curr_total.saturating_sub(prev_total);
        let idle_delta = curr_idle.saturating_sub(prev_idle);

        if total_delta == 0 {
            return MetricResult {
                value: CardValue::Percentage(0.0),
                subtitle: Some("统计暂未变化".to_string()),
                tooltip: Some("CPU 统计未变化".into()),
                state: MetricState::Normal,
                cached: false,
                metadata: None,
            };
        }

        let raw = (total_delta - idle_delta) as f64 / total_delta as f64;
        let usage = raw.clamp(0.0, 1.0);

        let value = if self.alpha > 0.0 && self.alpha < 1.0 {
            let s = match self.smoothed {
                Some(s) => self.alpha * usage + (1.0 - self.alpha) * s,
                None => usage,
            };
            self.smoothed = Some(s);
            s
        } else {
            usage
        };

        let pct = (value * 100.0).clamp(0.0, 100.0);

        MetricResult {
            value: CardValue::Percentage(pct),
            subtitle: Some("平滑使用率".to_string()),
            tooltip: Some(format!("CPU 使用率: {:.1}%", pct)),
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }
}
